//! 引擎演示：一条真实的帧循环 —— 场景树 + 磁盘资产驱动整条渲染管线。
//!
//! 演示内容（256x128 离屏画布，产物 PNG 落盘）：
//! - 两张程序化生成的纹理（磁盘 BMP，经 nes-asset FsLoader 加载）；
//! - 容器节点 + 子精灵（父子变换复合）与翻转精灵（左右镜像）；
//! - 场景相机（单位口径）；
//! - **热重载**：跑两帧之间改写磁盘纹理，第三帧画面变化。
//!
//! 运行：`cargo run --example engine_frame`（产物：`output/engine_frame.png`）。

use std::path::PathBuf;

use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::{PROP_FLIP_H, PROP_TEXTURE};
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::{NodeKind, Transform2D};

/// 16x16 棋盘格纹理（两色）。
fn checkerboard(a: [u8; 4], b: [u8; 4]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            rgba.extend_from_slice(if (x / 4 + y / 4) % 2 == 0 { &a } else { &b });
        }
    }
    rgba
}

fn main() {
    // 1) 演示资产：临时目录里的两张磁盘纹理。
    let root = std::env::temp_dir().join("nes_runtime_example");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("board.bmp"), 16, 16,
        &checkerboard([255, 140, 0, 255], [40, 40, 60, 255])).expect("写棋盘 A");
    write_bmp_rgba(&root.join("Textures").join("grid.bmp"), 16, 16,
        &checkerboard([0, 200, 120, 255], [20, 30, 50, 255])).expect("写棋盘 B");

    // 2) 装配引擎 + 声明/加载/上传纹理。
    let mut rt = match NesRuntime::open_with_root(&root, 256, 128) {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("引擎装配失败（如实报告）：{err}");
            std::process::exit(1);
        }
    };
    let board = rt.declare_texture("Textures/board.bmp").expect("声明 A");
    let grid = rt.declare_texture("Textures/grid.bmp").expect("声明 B");
    let report = rt.bind_assets();
    println!("[资产] 加载 {} 张（失败 {}）", report.loaded.len(), report.failed.len());
    let uploaded = rt.upload_pending_textures().expect("上传");
    println!("[资产] 上传 {uploaded} 张到 GPU 注册表");

    // 3) 搭场景：相机 + 容器（棋盘 A 两个子精灵，验证父子复合）+ 翻转精灵（棋盘 B）。
    {
        let tree = rt.tree_mut();
        let root_node = tree.root();
        let camera = tree.add_node(root_node, "cam", NodeKind::Camera2D);
        tree.set_local(camera, Transform2D::from_pos(128.0, 64.0));

        let world = tree.add_node(root_node, "world", NodeKind::Node2D);
        tree.set_local(world, Transform2D::from_pos(24.0, 24.0));
        let child_a = tree.add_node(world, "child_a", NodeKind::Sprite2D);
        tree.set_prop(child_a, PROP_TEXTURE, board.to_value()).unwrap();
        tree.set_local(child_a, Transform2D::from_pos(0.0, 0.0));
        let child_b = tree.add_node(world, "child_b", NodeKind::Sprite2D);
        tree.set_prop(child_b, PROP_TEXTURE, board.to_value()).unwrap();
        tree.set_local(child_b, Transform2D::from_pos(24.0, 0.0));

        let flipped = tree.add_node(root_node, "flipped", NodeKind::Sprite2D);
        tree.set_prop(flipped, PROP_TEXTURE, grid.to_value()).unwrap();
        tree.set_local(flipped, Transform2D::from_pos(160.0, 40.0));
        tree.set_prop(flipped, PROP_FLIP_H, nes_scene::Value::Bool(true)).unwrap();
    }

    // 4) 跑三帧：首帧 -> 改磁盘纹理 -> 热重载帧。
    let frame = |i: u64| FrameInfo::new(i, 1.0 / 60.0, i as f64 / 60.0, Vec2::new(256.0, 128.0));
    let first = rt.frame(&frame(0)).expect("首帧");
    println!(
        "[帧 0] drawn={} from_registry={} camera={} driver_errors={}",
        first.stats.drawn, first.stats.from_registry, first.stats.camera_applied,
        first.stats.driver_errors
    );

    write_bmp_rgba(&root.join("Textures").join("grid.bmp"), 16, 16,
        &checkerboard([200, 40, 40, 255], [50, 20, 20, 255])).expect("改写棋盘 B");
    let reloaded = rt.poll_reloads();
    println!("[资产] 热重载 {} 张", reloaded.reloaded.len());
    rt.upload_pending_textures().expect("重传");

    let second = rt.frame(&frame(1)).expect("热重载帧");
    println!(
        "[帧 1] drawn={} driver_errors={}",
        second.stats.drawn, second.stats.driver_errors
    );

    // 5) 落盘证据。
    let out = PathBuf::from("output").join("engine_frame.png");
    let written = second.write_png(&out).expect("PNG 落盘");
    println!("[产物] {}（{written} 字节）", out.display());
    println!("\n引擎帧循环演示：完成（场景树 -> 资产 -> 提取 -> 命令 -> GPU -> PNG）");
}
