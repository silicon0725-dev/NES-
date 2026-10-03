//! S13 第 2 期契约回归：运行时音频装配 + 脚本 `play` 全链。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Aud-01 | `open_audio` 幂等（已开返回 Ok）；headless 同一运行时也能开 |
//! | T-Aud-02 | Sound 资源装载链：声明 -> bind -> 解码 -> 按路径键注册进混音器 |
//! | T-Aud-03 | 脚本 `play` 的 Cmd::PlaySound 消费：开音频转混音器，未开静默丢弃 |
//! | T-Aud-04 | **音频不进语义指纹**：同轨迹开/不开音频 trace_hash 逐位相同；跑两遍同 |
//! | T-Aud-05 | 坏 WAV 不阻塞：解码失败进 bind 缺口清单、不 panic、键未注册 |
//!
//! 设备纪律（照 nes-audio 冒烟用例）：`waveout_device_count() == 0` 的环境
//! 只跑免设备路径（丢弃/指纹/缺口），设备用例如实跳过 —— "没有设备"与
//! "有设备但跑不通"不许互装。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_audio::device::waveout_device_count;
use nes_audio::wav::write_wav;
use nes_runtime::NesRuntime;

/// 设备用例串行锁：waveOut 设备是进程级单例（nes-audio 纪律），并行的
/// 三个用例必须串行开音频，否则互相顶出 `AlreadyOpen`。
fn device_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 每个用例独立的临时资产根（进程内计数器保证目录唯一，测试可并行）。
fn assets_root(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "nes_s13_audio_{tag}_{}_{}",
        n,
        std::process::id()
    ));
    std::fs::create_dir_all(dir.join("Audio")).expect("建临时资产根");
    dir
}

/// 440Hz / 0.05s / 单声道 16-bit 的最小 WAV（write_wav 生成，零二进制资产）。
fn write_beep(root: &std::path::Path, name: &str) {
    let rate = 22050u32;
    let frames = (rate / 20) as usize; // 0.05 秒
    let samples: Vec<i16> = (0..frames)
        .map(|i| {
            // 相位累进的整数近似正弦（无浮点三角依赖；幅度 8000 够测）
            let t = (i as i64 * 440) % rate as i64;
            ((t * 8000) / rate as i64) as i16
        })
        .collect();
    let wav = nes_audio::Wav { sample_rate: rate, channels: 1, samples };
    write_wav(&root.join("Audio").join(name), &wav).expect("写蜂鸣 WAV");
}

/// 最小场景：一个 Sound 资源 + 一个 `play` 一次的脚本节点。
fn write_scene(root: &std::path::Path, sound_file: &str, key: &str) {
    let scene = format!(
        r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Audio/{sound_file}", kind: "Sound"),
    ],
    root: Node(
        name: "main",
        kind: "Node",
        children: [
            Node(name: "game", kind: "Script", props: {{ "source": "every {{ if n < 1 {{ n = n + 1\n  play \"{key}\" }} }}" }}, children: [],),
        ],
    ),
)
"#
    );
    std::fs::write(root.join("audio_it.ron"), scene).expect("写场景");
}

/// T-Aud-01：open_audio 幂等；开之前混音器未构造（audio() 如实 None）。
#[test]
fn t_aud_01_open_audio_is_idempotent() {
    let mut rt = NesRuntime::open_headless(&assets_root("open")).expect("headless 装配");
    assert!(!rt.audio_open(), "装配不开音频（逐位同基线）");
    assert!(rt.audio().is_none(), "未开音频：混音器未构造");

    if waveout_device_count() == 0 {
        println!("[skip] 无 waveOut 设备：开音频路径如实跳过（丢弃/指纹面由其余用例覆盖）");
        return;
    }
    let _dev = device_lock(); // 设备单例：用例间串行
    rt.open_audio().expect("开音频");
    assert!(rt.audio_open());
    // 幂等：第二次 open 返回 Ok（设备单例不顶出 AlreadyOpen）。
    rt.open_audio().expect("重复开音频幂等");
    // 设备已开：填充线程持有 Arc 克隆 —— audio() 如实 None（不假装可变借用）。
    assert!(rt.audio().is_none(), "设备开着时 audio() 如实 None");
    // 显式先关设备再放锁：作用域收尾逆序 drop（_dev 先于 rt），不显式
    // drop 会让下一个用例拿到锁时设备还开着（AlreadyOpen 假阳性）。
    drop(rt);
}

/// T-Aud-02 + T-Aud-03：装载链注册 + 脚本 play 全链（headless 未开音频的
/// 丢弃语义 + 开音频的转交语义），并覆盖 T-Aud-04 的指纹面。
#[test]
fn t_aud_02_play_chain_and_fingerprint_ignores_audio() {
    let root = assets_root("chain");
    write_beep(&root, "beep.wav");
    write_scene(&root, "beep.wav", "Audio/beep");

    // 未开音频：play 照常执行、键被丢弃，指纹确定（跑两遍逐位同）。
    let mut rt = NesRuntime::open_headless(&root).expect("headless 装配");
    let r1 = rt.run_headless("audio_it.ron", &[], 10, 1.0 / 60.0).expect("跑一遍");
    let r2 = rt.run_headless("audio_it.ron", &[], 10, 1.0 / 60.0).expect("跑两遍");
    assert_eq!(r1.trace_hash, r2.trace_hash, "同轨迹两遍指纹逐位相同");

    if waveout_device_count() == 0 {
        println!("[skip] 无 waveOut 设备：开音频对照路径如实跳过");
        return;
    }
    let _dev = device_lock(); // 设备单例：用例间串行（rt 活到用例结束）
    // 开音频后（晚开：bind 已发生过 —— 覆盖 open_audio 的补注册路径），
    // 同一轨迹的指纹必须与未开音频**逐位相同** —— 音频不进语义指纹。
    rt.open_audio().expect("开音频");
    let r3 = rt.run_headless("audio_it.ron", &[], 10, 1.0 / 60.0).expect("开音频跑");
    assert_eq!(
        r1.trace_hash, r3.trace_hash,
        "开/不开音频同轨迹指纹逐位相同（音频不是语义状态）"
    );
    // 装载链确凿走通：open_audio 后的 run_headless（内含 load_scene ->
    // instantiate_scene -> bind_assets）应把 Sound 资源按路径键注册进混音器；
    // 再 load 一次手动验证注册账目幂等（版本未变不再重复注册）。
    assert_eq!(
        nes_runtime::registered_sound_count(&rt),
        1,
        "装载链注册恰一条（诊断账目与纹理上传同构）"
    );
    rt.load_scene("audio_it.ron").expect("重载场景");
    let report = rt.bind_assets();
    assert!(report.is_clean(), "绑定干净：{report:?}");
    let (registered, failures) = rt.register_pending_sounds();
    assert!(failures.is_empty(), "解码无失败：{failures:?}");
    assert_eq!(registered, 0, "bind_assets 已注册过：版本账目内幂等（零重复注册）");
    assert_eq!(nes_runtime::registered_sound_count(&rt), 1);
    drop(rt); // 先关设备再放锁（drop 序说明同 t_aud_01）
}

/// T-Aud-05：坏 WAV（非 PCM 假 bytes）不阻塞 —— 解码失败进 bind 缺口清单、
/// 场景照常跑、脚本 play 未注册键静默丢弃、指纹仍确定。
#[test]
fn t_aud_05_bad_wav_lands_in_gap_ledger() {
    let root = assets_root("badwav");
    // 故意写非 WAV 字节（文件存在、内容坏 —— 走"注册时解码失败"路径）。
    std::fs::write(root.join("Audio").join("broken.wav"), b"this is not a riff").unwrap();
    write_scene(&root, "broken.wav", "Audio/broken");

    let mut rt = NesRuntime::open_headless(&root).expect("headless 装配");
    let _dev = device_lock(); // 设备单例：用例间串行
    if waveout_device_count() > 0 {
        rt.open_audio().expect("开音频（坏文件不阻塞开设备）");
    }
    rt.load_scene("audio_it.ron").expect("场景照常加载");
    let report = rt.bind_assets();
    if rt.audio_open() {
        assert!(
            report.failed.iter().any(|(_, why)| why.contains("解码失败")),
            "解码失败进缺口清单：{report:?}"
        );
    }
    // 坏键 play 不崩帧、不影响确定性。
    let r1 = rt.run_headless("audio_it.ron", &[], 8, 1.0 / 60.0).expect("跑一遍");
    let r2 = rt.run_headless("audio_it.ron", &[], 8, 1.0 / 60.0).expect("跑两遍");
    assert_eq!(r1.trace_hash, r2.trace_hash, "坏 WAV 下指纹仍确定");
    drop(rt); // 先关设备再放锁（drop 序说明同 t_aud_01）
}
