//! S16 动画补间第 1 期 harness：**tween_demo** —— 引擎补间 vs 手写步进。
//!
//! 画面（384x216）：
//! - 蓝色方块：`tween_pos` 驱动的左右往返（脚本只在到站时换程，引擎逐
//!   tick 确定性推进 —— S16 的正主）；
//! - 红色标记：手写逐帧步进对照（`on "tick"` 累加，真实项目的旧写法），
//!   方向键/WASD 可动。
//!
//! 运行：`cargo run --release --example tween_demo`（方向键/WASD）。
//! 冒烟：`NES_GAME_FRAMES=180 cargo run --release --example tween_demo`。
//!
//! 确定性验证（同一场景、零输入轨迹）：
//! ```text
//! ./target/release/nes.exe --headless examples/assets/tween_demo.ron --frames 300
//! ```

use std::path::Path;
use std::time::Duration;

use nes_render_api::{FrameInfo, Vec2};
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::ScriptVm;

/// 单色 16x16 纹理。
fn solid_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    [r, g, b, 255].repeat(16 * 16)
}

fn main() {
    // 资产根 = 仓库内的示例目录（场景/纹理同目录，headless CLI 可直接跑）。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    let tex = root.join("Textures");
    std::fs::create_dir_all(&tex).unwrap();
    if !tex.join("tween_box.bmp").exists() {
        write_bmp_rgba(&tex.join("tween_box.bmp"), 16, 16, &solid_rgba(90, 130, 255))
            .expect("写方块纹理");
    }
    if !tex.join("manual_tri.bmp").exists() {
        write_bmp_rgba(&tex.join("manual_tri.bmp"), 16, 16, &solid_rgba(255, 80, 80))
            .expect("写标记纹理");
    }

    let mut rt = NesRuntime::open_windowed_with_root(&root, "NES 2.0 - Tween demo (S16)", 384, 216)
        .expect("窗口装配");
    let r1 = rt.declare_texture("Textures/tween_box.bmp").expect("声明方块纹理");
    let r2 = rt.declare_texture("Textures/manual_tri.bmp").expect("声明标记纹理");
    let _ = (r1, r2);
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 2, "纹理绑定：{report:?}");
    assert_eq!(rt.upload_pending_textures().expect("上传"), 2);

    rt.load_scene("tween_demo.ron").expect("加载场景");
    let mut vm = ScriptVm::new();
    {
        let table = rt.resources_mut().clone();
        let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
            std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
        });
        assert!(issues.is_empty(), "脚本装载：{issues:?}");
    }
    rt.mount_input_view(&mut vm);

    // 冒烟口：NES_GAME_FRAMES 限制帧数（缺省跑到窗口关闭）。
    let total: u64 = std::env::var("NES_GAME_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
    let mut transient = 0u64;
    const TRANSIENT_LIMIT: u64 = 120;
    for index in 0..total {
        let snap = rt.collect_input();
        let _ = rt.emit_input_signals(&snap);
        // tick 由引擎内建发射（S8.1）—— 补间推进在 tick 专属阶段（S16）。
        let frame = FrameInfo::new(
            index,
            1.0 / 60.0,
            index as f64 / 60.0,
            Vec2::new(384.0, 216.0),
        );
        match rt.frame_windowed_with(&frame, &mut vm) {
            Ok(Some(stats)) => {
                if stats.driver_errors > 0 {
                    eprintln!("[帧 {index}] driver_errors={}", stats.driver_errors);
                }
                transient = 0;
            }
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
    println!("[完成] tween_demo 退出");
}
