//! S13 第 2 期音频演示：**运行时开音频 + 资产 Sound 装载 + 脚本 play 全链**。
//!
//! 照 first_game 的宿主形态（装配 -> 装载 -> 逐帧），只多一步 `open_audio`：
//!
//! ```text
//! Audio/beep.wav（代码生成）──▶ Res(kind:"Sound") ──▶ bind：读字节 -> WAV 解码
//!     ──▶ Mixer::register("Audio/beep", …) ──▶ 脚本 play "Audio/beep"
//!     ──▶ Cmd::PlaySound ──▶ tick 后转交混音器 ──▶ waveOut 出声
//! ```
//!
//! 行为：
//! - 装载完成即播一声（脚本 `init { play … }` —— init 在首次派发前执行，
//!   S8.0 既有语义）；
//! - 按空格再播一声（`on "input/key_down"`）；
//! - 无音频设备时如实报一行并照常跑（play 静默丢弃 —— headless 语义）。
//!
//! 冒烟：`NES_GAME_FRAMES=180 cargo run --release --example audio_demo`
//! （180 帧内 init 的蜂鸣已走完 Cmd 全链；确定性面不涉音频 —— 指纹
//! 契约见 `tests/s13_audio.rs`）。

use std::path::Path;
use std::time::Duration;

use nes_audio::wav::write_wav;
use nes_render_api::{FrameInfo, Vec2};
use nes_runtime::NesRuntime;
use nes_scene::ScriptVm;

/// 440Hz 蜂鸣样本（0.3 秒、单声道 16-bit；与 editor_shell 的生成式同一家法
/// —— 代码生成、缺了再写，不引外部二进制资产管线）。
fn beep_samples(duration_ms: u32, amplitude: i16) -> Vec<i16> {
    let rate = 22050u32;
    let frames = (u64::from(rate) * u64::from(duration_ms) / 1000) as usize;
    (0..frames)
        .map(|i| {
            // 整数近似的 440Hz 正弦相位（无浮点三角依赖；确定性生成）。
            let t = (i as i64 * 440) % rate as i64;
            ((t * amplitude as i64) / rate as i64) as i16
        })
        .collect()
}

fn main() {
    // 资产根 = 仓库内演示目录；演示声音资产缺了再写（bmp 同口径）。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    let audio_dir = root.join("Audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let beep = audio_dir.join("beep.wav");
    if !beep.exists() {
        let wav =
            nes_audio::Wav { sample_rate: 22050, channels: 1, samples: beep_samples(300, 8000) };
        write_wav(&beep, &wav).expect("写蜂鸣 WAV");
    }

    let mut rt = NesRuntime::open_windowed_with_root(&root, "NES 2.0 - Audio Demo (S13)", 384, 216)
        .expect("窗口装配");
    // 先开音频再装载：装载链 bind 时即注册（晚开也有 open_audio 的补注册
    // 路径 —— 两侧同键约定，顺序无关）。失败如实报行、演示照常（play
    // 静默丢弃 —— headless 语义，场景仍正常渲染与跑脚本）。
    match rt.open_audio() {
        Ok(()) => println!("[audio] on (waveOut 48000Hz stereo)"),
        Err(e) => eprintln!("[audio] 不可用：{e}（play 将静默丢弃，演示照常）"),
    }
    rt.load_scene("audio_demo.ron").expect("加载场景");
    // 装载链取证：bind 应把 Sound 资源按路径键注册进混音器（恰 1 条）。
    println!(
        "[audio] registered {} sound(s)",
        nes_runtime::registered_sound_count(&rt)
    );
    let mut vm = ScriptVm::new();
    {
        let table = rt.resources_mut().clone();
        let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
            std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
        });
        assert!(issues.is_empty(), "脚本装载：{issues:?}");
    }
    rt.mount_input_view(&mut vm);

    let total: u64 = std::env::var("NES_GAME_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
    let mut transient = 0u64;
    const TRANSIENT_LIMIT: u64 = 120;
    for index in 0..total {
        let snap = rt.collect_input();
        let _ = rt.emit_input_signals(&snap);
        // tick 由引擎内建发射（S8.1）；play 的 Cmd 在 tick 后由运行时消费。
        let frame = FrameInfo::new(
            index,
            1.0 / 60.0,
            index as f64 / 60.0,
            Vec2::new(384.0, 216.0),
        );
        match rt.frame_windowed_with(&frame, &mut vm) {
            Ok(Some(_stats)) => transient = 0,
            Ok(None) => {
                println!("[帧 {index}] 窗口已关闭，退出");
                break;
            }
            Err(err) => {
                transient += 1;
                eprintln!("[帧 {index}] 失败（{transient}/{TRANSIENT_LIMIT}）：{err}");
                if transient >= TRANSIENT_LIMIT {
                    std::process::exit(1);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    println!("[完成] Audio Demo 退出");
}
