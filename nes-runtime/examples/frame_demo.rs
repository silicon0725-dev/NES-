//! S16.2 图集帧动画 harness：**frame_demo** —— 走路循环。
//!
//! 画面（384x216）：
//! - 蓝色小方块角色：2x4 图集（32x64 BMP，16px 帧格）的 4 帧位移图案，
//!   `sheet_cols=2 / sheet_rows=4` + `tween_frame "walker" 0 4 600
//!   "linear" "loop"` —— 终点 4 越出 `cols*rows=4` 由渲染侧模运算回绕
//!   到 0，即**无缝走路循环**（loop 永不移除登记）；
//! - 角色放大 4 倍显示（16px 帧格 -> 64px 四边形 —— 子矩形采样随世界
//!   变换一起缩放），方块逐帧右移一格 + 明暗交替，循环肉眼可见。
//!
//! 运行：`cargo run --release --example frame_demo`。
//! 冒烟：`NES_GAME_FRAMES=180 cargo run --release --example frame_demo`。
//!
//! 确定性验证（同一场景、零输入轨迹）：
//! ```text
//! ./target/release/nes.exe --headless examples/assets/frame_demo.ron --frames 300
//! ```

use std::path::Path;
use std::time::Duration;

use nes_render_api::{FrameInfo, Vec2};
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::ScriptVm;

/// 帧格常量：2 列 x 4 行、每格 16px。
const CELL: u32 = 16;
const COLS: u32 = 2;
const ROWS: u32 = 4;

/// 生成 2x4 走图图集（代码生成，与 tween_demo 的纯色 BMP 同一手法）：
/// 前 4 格 = 四帧位移图案（小方块逐帧右移 3px + 明暗交替 + 地面线），
/// 后 4 格 = 斜线纹理的未用格（与帧区可辨，防误读）。
fn walk_sheet_rgba() -> Vec<u8> {
    let (w, h) = (COLS * CELL, ROWS * CELL);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let mut put = |x: u32, y: u32, c: [u8; 4]| {
        let i = ((y * w + x) * 4) as usize;
        rgba[i..i + 4].copy_from_slice(&c);
    };
    for frame in 0..4u32 {
        let cx = (frame % COLS) * CELL;
        let cy = (frame / COLS) * CELL;
        // 帧底色（近黑）+ 地面线（暗灰）：位移的参照物。
        for y in 0..CELL {
            for x in 0..CELL {
                let ground = y == CELL - 3 || y == CELL - 2;
                let c = if ground {
                    [70, 70, 80, 255]
                } else {
                    [16, 18, 28, 255]
                };
                put(cx + x, cy + y, c);
            }
        }
        // 角色：6x6 小方块，逐帧右移 3px（3 -> 6 -> 9 -> 12，回绕由
        // tween_frame 的 loop + 渲染侧取模完成），奇数帧提亮 —— 交替感。
        let bx = 3 + frame * 3;
        let bright = frame % 2 == 1;
        for y in 5..11 {
            for x in bx..bx + 6 {
                let c = if bright {
                    [120, 170, 255, 255]
                } else {
                    [70, 110, 220, 255]
                };
                put(cx + x, cy + y, c);
            }
        }
    }
    // 未用格（4..7）：暗品红斜线 —— 与帧区一眼可辨。
    for cell in 4..(COLS * ROWS) {
        let cx = (cell % COLS) * CELL;
        let cy = (cell / COLS) * CELL;
        for y in 0..CELL {
            for x in 0..CELL {
                let c = if (x + y) % 8 < 2 {
                    [70, 30, 60, 255]
                } else {
                    [24, 14, 22, 255]
                };
                put(cx + x, cy + y, c);
            }
        }
    }
    rgba
}

fn main() {
    // 资产根 = 仓库内的示例目录（场景/纹理同目录，headless CLI 可直接跑）。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    let tex = root.join("Textures");
    std::fs::create_dir_all(&tex).unwrap();
    let sheet = tex.join("walk_sheet.bmp");
    if !sheet.exists() {
        write_bmp_rgba(&sheet, COLS * CELL, ROWS * CELL, &walk_sheet_rgba())
            .expect("写走图图集");
    }

    let mut rt = NesRuntime::open_windowed_with_root(&root, "NES 2.0 - Frame demo (S16.2)", 384, 216)
        .expect("窗口装配");
    let r1 = rt.declare_texture("Textures/walk_sheet.bmp").expect("声明图集纹理");
    let _ = r1;
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 1, "纹理绑定：{report:?}");
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    rt.load_scene("frame_demo.ron").expect("加载场景");
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
        // tick 由引擎内建发射（S8.1）—— 帧补间推进在 tick 专属阶段（S16）。
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
    println!("[完成] frame_demo 退出");
}
