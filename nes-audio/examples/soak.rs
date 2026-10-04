//! 音频设备长跑浸泡（soak）：设备线程硬化的回归门 + 定时器分辨率实测。
//!
//! 模拟宿主真实压力形态，统计 [`nes_audio::device::underruns`]：
//!
//! ```text
//! 阶段 1  定时器分辨率实测（设备未开：环境本底）
//! 阶段 2  开设备（48000Hz / 立体声，与 nes-runtime::open_audio 同参）
//!         循环背景音 + 短音效，连续播放
//! 阶段 3  宿主抖动风暴：风暴线程周期性 锁mixer → play/register覆盖/stop_key
//!         （与 nes-runtime 持锁形态同构：锁内只有微秒级操作）；
//!         另起 CPU 脉冲线程（间歇忙旋）模拟渲染/游戏线程尖峰
//! 阶段 4  停风暴 → 定时器复测（设备仍开：1ms 提升生效中）→ 关设备
//! 阶段 5  报告 underruns（硬化后必须为 0，>0 以非零码退出）
//! ```
//!
//! 运行（默认 60 秒）：
//!
//! ```text
//! cargo run --release --example soak
//! NES_SOAK_SECS=10 cargo run --release --example soak   # 快速
//! ```
//!
//! 无 waveOut 设备的环境（CI / 无声卡）如实打印一行后以 0 退出
//! （与 device_smoke 同一跳过纪律）。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use nes_audio::device::{underruns, waveout_device_count};
use nes_audio::{AudioDevice, Mixer, Wav};

/// 生成单声道整数近似锯齿波（440Hz、22050Hz；与 audio_demo 蜂鸣同一
/// 生成式：无浮点三角依赖、确定性）。soak 只关心"有非静音数据在播"，
/// 音色不参与判据。
fn tone(duration_ms: u32, amplitude: i16) -> Arc<Wav> {
    let rate = 22050u32;
    let frames = (u64::from(rate) * u64::from(duration_ms) / 1000) as usize;
    let samples = (0..frames)
        .map(|i| {
            let t = (i as i64 * 440) % rate as i64;
            ((t * amplitude as i64) / rate as i64) as i16
        })
        .collect();
    Arc::new(Wav { sample_rate: rate, channels: 1, samples })
}

/// 实测睡眠节拍：连睡 N 次 10ms，返回 (最小/平均/最大) 实际毫秒。
/// Windows 默认系统定时器分辨率 ~15.6ms 时，min/avg 会落在 15-16ms
/// 而不是 10ms —— 这正是设备卡顿根因的直接证据。
fn probe_sleep_ms(rounds: u32) -> (f64, f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = 0.0f64;
    let mut total = 0.0f64;
    for _ in 0..rounds {
        let t0 = Instant::now();
        std::thread::sleep(Duration::from_millis(10));
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        min = min.min(ms);
        max = max.max(ms);
        total += ms;
    }
    (min, total / f64::from(rounds), max)
}

fn main() {
    let secs: u64 = std::env::var("NES_SOAK_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    // ---- 阶段 1：设备未开，测环境本底定时器分辨率 ----
    let (min, avg, max) = probe_sleep_ms(30);
    println!("[timer] 设备未开（环境本底）：sleep(10ms) 实测 min/avg/max = {min:.1}/{avg:.1}/{max:.1} ms");

    if waveout_device_count() == 0 {
        println!("[跳过] 没有 waveOut 输出设备（waveOutGetNumDevs = 0），soak 无从跑起");
        return;
    }

    // ---- 阶段 2：装配混音器 + 设备（48000Hz/立体声 = open_audio 同参）----
    let mixer = Arc::new(Mutex::new(Mixer::new()));
    let device = AudioDevice::open(Arc::clone(&mixer), 48_000, 2)
        .expect("系统有 waveOut 设备时打开必须成功");
    {
        let mut m = mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        m.register("bg", tone(1500, 5000)); // 循环背景音（保证全程有非静音数据）
        m.register("fx", tone(120, 9000)); // 短音效（风暴反复开停）
        m.play("bg", 0.4, true).expect("bg 已注册");
    }
    println!("[audio] 设备已开（48000Hz stereo），背景音循环播放 {secs}s ……");

    // ---- 阶段 3a：宿主抖动风暴（与 nes-runtime 持锁形态同构）----
    let storm_stop = Arc::new(AtomicBool::new(false));
    let storm = {
        let mixer = Arc::clone(&mixer);
        let storm_stop = Arc::clone(&storm_stop);
        std::thread::Builder::new()
            .name("soak-storm".into())
            .spawn(move || {
                let mut round = 0u64;
                while !storm_stop.load(Ordering::Relaxed) {
                    // 锁外先把覆盖用的声音造好（与 register_pending_sounds
                    // 同律：解码/构造在锁外，锁内只有登记本身）。
                    let hot = tone(120, 9000);
                    {
                        let mut m = mixer
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let _ = m.play("fx", 0.8, false);
                        m.register("fx", hot); // 同键覆盖 = 热重载扰动
                        if round % 4 == 3 {
                            m.stop_key("fx");
                        }
                    }
                    round += 1;
                    std::thread::sleep(Duration::from_millis(150));
                }
            })
            .expect("风暴线程装配")
    };

    // ---- 阶段 3b：CPU 脉冲（间歇忙旋，模拟渲染/游戏线程尖峰）----
    let load_stop = Arc::new(AtomicBool::new(false));
    let mut loads = Vec::new();
    for id in 0..2 {
        let load_stop = Arc::clone(&load_stop);
        loads.push(
            std::thread::Builder::new()
                .name(format!("soak-load-{id}"))
                .spawn(move || {
                    while !load_stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(40));
                        // ~15ms 普通优先级忙旋：制造调度压力，但不主动饿别人
                        //（真实宿主的渲染线程就是这个形态）。
                        let until = Instant::now() + Duration::from_millis(15);
                        while Instant::now() < until {
                            std::hint::spin_loop();
                        }
                    }
                })
                .expect("负载线程装配"),
        );
    }

    // ---- 主浸泡窗口 ----
    std::thread::sleep(Duration::from_secs(secs));

    // ---- 阶段 4：停风暴/负载 → 定时器复测（设备仍开）→ 关设备 ----
    storm_stop.store(true, Ordering::Relaxed);
    load_stop.store(true, Ordering::Relaxed);
    let _ = storm.join();
    for handle in loads.drain(..) {
        let _ = handle.join();
    }
    let (min, avg, max) = probe_sleep_ms(30);
    println!("[timer] 设备已开（提升生效中）：sleep(10ms) 实测 min/avg/max = {min:.1}/{avg:.1}/{max:.1} ms");

    device.close();
    let total = underruns();
    println!("[soak] {secs}s 连续播放 + 宿主抖动风暴：underruns = {total}");
    if total > 0 {
        eprintln!("[soak] 欠载 > 0：设备队列被打干（硬化回归门失败）");
        std::process::exit(1);
    }
    println!("[soak] 通过：全程无欠载");
}
