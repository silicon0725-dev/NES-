//! S6.1 演示：引擎帧循环跑在**真实窗口**里。
//!
//! 与 `engine_frame`（离屏）的分工：本例把同一条「场景树 -> tick（生命周期，
//! 动画由 `SceneObserver` 回调驱动）-> 提取 -> 命令 -> 渲染」管线画到 Win32
//! 窗口表面并逐帧呈现，中途做一次磁盘热重载 —— 关掉窗口或跑满 300 帧
//!（约 5 秒；`NES_WINDOW_FRAMES` 可延长）后干净退出。
//!
//! 运行：`cargo run --example engine_window`

use std::time::Duration;

use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::{PROP_FLIP_H, PROP_TEXTURE};
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::{NodeCtx, NodeId, NodeKind, SceneObserver, Transform2D, Value};

/// 16x16 棋盘格。
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
    // 1) 演示资产（临时目录）。
    let root = std::env::temp_dir().join("nes_runtime_window");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("board.bmp"), 16, 16,
        &checkerboard([255, 140, 0, 255], [40, 40, 60, 255])).expect("写棋盘 A");
    write_bmp_rgba(&root.join("Textures").join("grid.bmp"), 16, 16,
        &checkerboard([0, 200, 120, 255], [20, 30, 50, 255])).expect("写棋盘 B");

    // 2) 窗口模式装配（资产根 = 上面的演示目录）。
    let mut rt = match NesRuntime::open_windowed_with_root(
        &root,
        "NES 2.0 - engine window (S6.1)",
        512,
        288,
    ) {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("窗口模式装配失败（如实报告）：{err}");
            std::process::exit(1);
        }
    };
    println!("[窗口] 已创建并显示");

    // 3) 资产：声明/加载/上传。
    let board = rt.declare_texture("Textures/board.bmp").expect("声明 A");
    let grid = rt.declare_texture("Textures/grid.bmp").expect("声明 B");
    rt.bind_assets();
    rt.upload_pending_textures().expect("上传");

    // 4) 场景：相机 + 世界容器（两个棋盘精灵 + 一个翻转精灵）+ 一个 HUD 控件。
    let world = {
        let tree = rt.tree_mut();
        let root_node = tree.root();
        let camera = tree.add_node(root_node, "cam", NodeKind::Camera2D);
        // 相机视口 = 窗口客户区，中心 = 半尺寸 -> 单位视图（世界 == 像素）。
        tree.set_local(camera, Transform2D::from_pos(256.0, 144.0));

        let world = tree.add_node(root_node, "world", NodeKind::Node2D);
        tree.set_local(world, Transform2D::from_pos(32.0, 32.0));
        let a = tree.add_node(world, "a", NodeKind::Sprite2D);
        tree.set_prop(a, PROP_TEXTURE, board.to_value()).unwrap();
        tree.set_local(a, Transform2D::from_pos(0.0, 0.0));
        let b = tree.add_node(world, "b", NodeKind::Sprite2D);
        tree.set_prop(b, PROP_TEXTURE, board.to_value()).unwrap();
        tree.set_local(b, Transform2D::from_pos(24.0, 0.0));

        let flipped = tree.add_node(root_node, "flipped", NodeKind::Sprite2D);
        tree.set_prop(flipped, PROP_TEXTURE, grid.to_value()).unwrap();
        tree.set_local(flipped, Transform2D::from_pos(200.0, 48.0));
        tree.set_prop(flipped, PROP_FLIP_H, Value::Bool(true)).unwrap();

        let hud = tree.add_node(root_node, "hud", NodeKind::Control);
        tree.set_local(hud, Transform2D::from_pos(0.0, 0.0));
        nes_runtime_set_rect(tree, hud);
        world
    };

    // 5) 行为代码：世界容器每帧沿正弦轨道平移（生命周期回调驱动，
    //    宿主不再逐帧直改树 —— 这是 tick 接线前后的分水岭）。
    let mut sine = SineDrift { world, t: 0.0 };

    // 6) 帧循环：默认 300 帧（约 5 秒；NES_WINDOW_FRAMES 可延长），
    //    第 150 帧热重载棋盘 B（绿 -> 红）。
    let total = std::env::var("NES_WINDOW_FRAMES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300);
    let mut rendered = 0u64;
    for index in 0..total {
        if index == 150 {
            write_bmp_rgba(&root.join("Textures").join("grid.bmp"), 16, 16,
                &checkerboard([200, 40, 40, 255], [50, 20, 20, 255])).expect("热重载改写");
            let report = rt.poll_reloads();
            println!("[帧 {index}] 检查 {} 张，热重载 {} 张（右侧翻转精灵应变红）",
                report.checked, report.reloaded.len());
            let uploaded = rt.upload_pending_textures().expect("重传");
            println!("[帧 {index}] 重传 {uploaded} 张");
        }
        let frame = FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(512.0, 288.0));
        match rt.frame_windowed_with(&frame, &mut sine) {
            Ok(Some(stats)) => {
                if stats.driver_errors > 0 {
                    eprintln!("[帧 {index}] driver_errors={}", stats.driver_errors);
                }
                rendered += 1;
            }
            Ok(None) => {
                println!("[帧 {index}] 窗口已关闭，退出");
                break;
            }
            Err(err) => {
                eprintln!("帧 {index} 失败（如实报告）：{err}");
                std::process::exit(1);
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    println!("[完成] 共呈现 {rendered} 帧到窗口表面");
}

/// 世界容器的行为：每帧沿正弦轨道平移（2 rad/s，振幅 24px）。
struct SineDrift {
    world: NodeId,
    t: f32,
}

impl SceneObserver for SineDrift {
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, delta: f32) {
        if ctx.this() != self.world {
            return;
        }
        self.t += delta;
        let x = 32.0 + (self.t * 2.0).sin() * 24.0;
        ctx.set_local(Transform2D::from_pos(x, 32.0));
    }
}

/// Control 节点的锚点/偏移属性（经提取层 -> set_rect -> HUD 边框）。
fn nes_runtime_set_rect(tree: &mut nes_scene::SceneTree, node: nes_scene::NodeId) {
    use nes_scene::Value;
    use nes_render_extract::{PROP_CONTROL_ANCHOR, PROP_CONTROL_OFFSET, PROP_CONTROL_SIZE};
    // 200x64 的边框，锚在客户区右上：anchor(1,0) + offset(-224,16) + size(208,48)。
    let _ = tree.set_prop(node, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::new(1.0, 0.0)));
    let _ = tree.set_prop(node, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(-224.0, 16.0)));
    let _ = tree.set_prop(node, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(208.0, 48.0)));
}
