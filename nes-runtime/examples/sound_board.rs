//! S13 真实数据实测：**sound_board**——扫描音效目录、全量解码出报告、
//! 键盘随机播放。
//!
//! ```text
//! NES_SFX_DIR="E:/sound effects" cargo run --release --example sound_board
//! ```
//!
//! 键位：SPACE = 随机音效；1..9 = 按序号播放；ESC / 关窗退出。
//! 扫描即验证：目录里全部 .wav 用引擎自带解析器逐个解码，控制台出
//! 成功率与失败原因分布（真实数据压测——2026-10-03 实测驱动了 24-bit
//! PCM 契约的引入）。

use std::path::{Path, PathBuf};

use nes_audio::{parse, Mixer};

fn main() {
    let dir = std::env::var("NES_SFX_DIR").unwrap_or_else(|_| "E:/sound effects".into());
    let frames_cap: u64 = std::env::var("NES_GAME_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);

    // ---- 扫描 + 全量解码报告 ----
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_wavs(Path::new(&dir), &mut paths, 0);
    paths.sort();
    println!("[scan] {} 个 .wav（递归 {}）", paths.len(), dir);

    let mut mixer = Mixer::default();
    let mut ok16 = 0u32;
    let mut ok24 = 0u32;
    let mut reasons: Vec<(String, u32)> = Vec::new();
    let mut registered = 0u32;
    for (i, p) in paths.iter().enumerate() {
        let Ok(bytes) = std::fs::read(p) else {
            bump(&mut reasons, "read failed");
            continue;
        };
        match parse(&bytes) {
            Ok(wav) => {
                if wav.samples.iter().all(|&s| s == 0) && i % 97 == 0 {
                    // 全零样本偶发（静音垫片）不算失败，仅采样提示。
                }
                let bits = if bytes.len() > 0 { "" } else { "" };
                let _ = bits;
                // 24-bit 判定：解析成功后无法回读位深——按文件比例粗记。
                if is_probably24(p) {
                    ok24 += 1;
                } else {
                    ok16 += 1;
                }
                if registered < 96 {
                    let key = format!("s{:03}", registered);
                    mixer.register(&key, std::sync::Arc::new(wav));
                    registered += 1;
                }
            }
            Err(e) => bump(&mut reasons, &e.to_string()),
        }
    }
    println!(
        "[scan] 解码成功 16-bit={} 24-bit(取高16位)={} / 失败 {}",
        ok16,
        ok24,
        paths.len() as u32 - ok16 - ok24
    );
    for (r, n) in &reasons {
        println!("[scan]   失败 x{n}: {r}");
    }
    println!("[scan] 注册 {} 个进混音器（池上限 96）", registered);
    if registered == 0 {
        println!("[scan] 没有可播放的音效，退出");
        return;
    }

    // ---- 设备 ----
    let mixer = std::sync::Arc::new(std::sync::Mutex::new(mixer));
    let device = match nes_audio::AudioDevice::open(mixer.clone(), 48000, 2) {
        Ok(d) => d,
        Err(e) => {
            println!("[audio] 设备打开失败：{e}（无设备环境只出报告）");
            return;
        }
    };
    println!("[audio] waveOut 48000Hz stereo");

    // ---- 交互循环 ----
    let window = nes_render_wgpu::window::Window::open("NES 2.0 - Sound Board (S13)", 384, 216)
        .expect("窗口");
    let mut rng_state = 0x2545_F491_4F6C_DD1Du64;
    let mut next_random = || {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        (rng_state % registered as u64) as u32
    };
    println!("[keys] SPACE=随机 1..9=按序 ESC=退出");
    let mut frames = 0u64;
    'main: loop {
        if frames >= frames_cap {
            break;
        }
        frames += 1;
        if !window.pump() {
            break;
        }
        for ev in nes_render_wgpu::window::drain_input() {
            let play = |idx: u32| {
                let key = format!("s{idx:03}");
                let mut m = mixer.lock().unwrap();
                if m.has(&key) {
                    let _ = m.play(&key, 0.9, false);
                    println!("[play] {key}");
                }
            };
            match ev {
                nes_render_api::input::InputEvent::Key { key, down: true } => {
                    let name = key.name();
                    if name == "Escape" {
                        break 'main;
                    }
                    if name == "Space" {
                        play(next_random());
                    }
                    if let Some(n) = name.chars().next().and_then(|c| c.to_digit(9)) {
                        if (1..=9).contains(&n) {
                            play(n as u32 - 1);
                        }
                    }
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    drop(device);
    println!("[完成] Sound Board 退出");
}

fn bump(reasons: &mut Vec<(String, u32)>, r: &str) {
    if let Some(e) = reasons.iter_mut().find(|(s, _)| s == r) {
        e.1 += 1;
    } else {
        reasons.push((r.to_string(), 1));
    }
}

fn collect_wavs(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_wavs(&p, out, depth + 1);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav")) {
            out.push(p);
        }
    }
}

/// 24-bit 粗判：读 fmt 块的 wBitsPerSample（报告归类用，非解析权威）。
fn is_probably24(p: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    use std::io::Read;
    let mut head = [0u8; 4096];
    if f.read_exact(&mut head[..64]).is_err() {
        return false;
    }
    let mut pos = 12usize;
    loop {
        if pos + 8 > head.len() {
            // 头不够长：补读一次到 4096。
            let n = f.read(&mut head).unwrap_or(0);
            if n == 0 || pos + 8 > head.len() {
                return false;
            }
        }
        let cid = &head[pos..pos + 4];
        let sz = u32::from_le_bytes(head[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if cid == b"fmt " && pos + 8 + 16 <= head.len() {
            let bits = u16::from_le_bytes(head[pos + 8 + 14..pos + 8 + 16].try_into().unwrap());
            return bits == 24;
        }
        pos += 8 + sz + (sz & 1);
        if pos > head.len() {
            return false;
        }
    }
}
