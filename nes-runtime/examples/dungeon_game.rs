//! S8.2 移植压力测试：**Mini Dungeon** —— raylib 风格 top-down shooter
//! 的结构移植（Player/Enemy/Projectile/Pickup/Door/UI）。
//!
//! 这是"用真实项目压 Runtime"的窗口宿主：场景与行为全部在
//! `examples/assets/dungeon.ron`（手写场景文件，非代码搭场景），
//! 宿主只做四件事：装配、装载、逐帧（输入 → `input/*` 信号 → `tick`
//! 节拍 → 渲染）、瞬态容忍。确定性验证走 headless CLI：
//!
//! ```text
//! ./target/release/nes.exe --headless examples/assets/dungeon.ron \
//!     --frames 600 --trace examples/assets/dungeon_trace.txt
//! ```
//!
//! 运行：`cargo run --example first_game`（方向键/WASD 移动）。

use std::path::Path;
use std::time::Duration;

use nes_render_api::{FrameInfo, Vec2};
use nes_render_wgpu::bmp;
use nes_render_wgpu::FontParams;
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::ScriptVm;

/// 单色 16x16 纹理。
fn solid_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    [r, g, b, 255].repeat(16 * 16)
}

fn main() {
    // 资产根 = 仓库内的游戏目录（场景/轨迹/纹理都在这儿 —— headless
    // CLI 直接跑同一份场景文件）。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    let tex = root.join("Textures");
    std::fs::create_dir_all(&tex).unwrap();
    for (name, rgb) in [
        ("player.bmp", (90, 130, 255)),
        ("enemy.bmp", (255, 80, 80)),
        ("bullet.bmp", (255, 220, 60)),
        ("heart.bmp", (255, 120, 200)),
        ("door.bmp", (90, 220, 120)),
    ] {
        if !tex.join(name).exists() {
            let (r, g, b) = rgb;
            write_bmp_rgba(&tex.join(name), 16, 16, &solid_rgba(r, g, b))
                .expect("写纹理");
        }
    }

    let mut rt = NesRuntime::open_windowed_with_root(&root, "NES 2.0 - Mini Dungeon (S8.2)", 384, 216)
        .expect("窗口装配");
    for t in ["player", "enemy", "bullet", "heart", "door"] {
        let _ = rt.declare_texture(&format!("Textures/{t}.bmp")).expect("声明纹理");
    }
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 5, "纹理绑定：{report:?}");
    assert_eq!(rt.upload_pending_textures().expect("上传"), 5);
    {
        // 默认字体（HUD Label）。
        let font_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../nes-render-wgpu/examples/assets");
        let (w, h, sheet) =
            bmp::load_rgba(&std::fs::read(font_dir.join("font_atlas.bmp")).expect("读字形表"))
                .expect("解码字形表");
        let metrics =
            std::fs::read_to_string(font_dir.join("font_metrics.txt")).expect("读字形表参数");
        let field = |k: &str| -> f32 {
            metrics
                .split_whitespace()
                .find_map(|t| t.strip_prefix(&format!("{k}=")))
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("font_metrics 缺 {k}"))
        };
        let cell = metrics
            .split_whitespace()
            .find_map(|t| t.strip_prefix("cell="))
            .and_then(|c| c.split_once('x'))
            .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
            .expect("cell 格式");
        rt.consumer_mut()
            .expect("GPU 消费器")
            .set_default_font(
                FontParams {
                    width: w,
                    height: h,
                    cell_w: cell.0,
                    cell_h: cell.1,
                    cols: field("cols") as u32,
                    first_char: field("first") as u32,
                    count: field("count") as u32,
                    advance: field("advance"),
                    line_height: field("line_height"),
                },
                &sheet,
            )
            .expect("登记默认字体");
    }

    rt.load_scene("dungeon.ron").expect("加载场景");
    let mut vm = ScriptVm::new();
    {
        let table = rt.resources_mut().clone();
        let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
            std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
        });
        assert!(issues.is_empty(), "脚本装载：{issues:?}");
    }
    rt.mount_key_probe(&mut vm);

    let total: u64 = std::env::var("NES_GAME_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
    let mut transient = 0u64;
    const TRANSIENT_LIMIT: u64 = 120;
    for index in 0..total {
        let snap = rt.collect_input();
        let _ = rt.emit_input_signals(&snap);
        // tick 由引擎内建发射（S8.1）—— 宿主不再手发。
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
    println!("[完成] Mini Dungeon 退出");
}
