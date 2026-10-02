//! S12.1 组件库首站验收：E-1 颜色管线 + Theme 节点 + Button 四态。
//!
//! 深色主题窗口：背景/面板（Control 槽位着色）、三个按钮（悬停 =
//! accent 边框、按下 = accent 填充 —— UiVm 状态机驱动，提取层着色）、
//! 点击经激活钩子更新状态行。窗口 512x288、世界 1:1（相机置中）。
//!
//! `NES_GAME_FRAMES=N` 自动退出（冒烟）；缺省玩到关窗。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use nes_render_api::{FrameInfo, Vec2};
use nes_render_wgpu::bmp;
use nes_render_wgpu::FontParams;
use nes_runtime::NesRuntime;
use nes_scene::{NodeId, NodeKind, Transform2D, Value, Vec2 as SVec2};

fn main() {
    let font_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../nes-render-wgpu/examples/assets");
    let mut rt = NesRuntime::open_windowed_with_root(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets"),
        "NES 2.0 - S12.1 Widgets",
        512,
        288,
    )
    .expect("窗口装配");
    {
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

    let buttons = build_scene(&mut rt);

    // 激活钩子：UiVm 帧内回调零写权 —— 按钮名（搭场景时已知）经共享
    // 缓冲传出，帧后宿主写状态行，借用不打结。
    let clicked: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let sink = clicked.clone();
    rt.ui_vm_mut().on_activate(move |btn| {
        if let Some(name) = buttons.get(&btn) {
            *sink.borrow_mut() = Some(name.clone());
        }
    });

    let total: u64 = std::env::var("NES_GAME_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
    let mut transient = 0u64;
    const TRANSIENT_LIMIT: u64 = 120;
    for index in 0..total {
        let snap = rt.collect_input();
        let _ = rt.emit_input_signals(&snap);
        let frame = FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(512.0, 288.0));
        match rt.frame_windowed_with(&frame, &mut nes_scene::NoObserver) {
            Ok(Some(_)) => transient = 0,
            Ok(None) => break,
            Err(err) => {
                transient += 1;
                eprintln!("[帧 {index}] 失败（{transient}/{TRANSIENT_LIMIT}）：{err}");
                if transient >= TRANSIENT_LIMIT {
                    std::process::exit(1);
                }
            }
        }
        if let Some(name) = clicked.borrow_mut().take() {
            if let Some(status) = rt.tree_mut().find_by_name("status") {
                let text = format!("CLICKED: {name}");
                let _ = rt
                    .tree_mut()
                    .set_prop(status, "text", Value::Str(text));
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    println!("[完成] S12.1 Widgets 退出");
}

/// 搭 UI 场景；返回 按钮NodeId -> 名字（激活钩子用）。
fn build_scene(rt: &mut NesRuntime) -> BTreeMap<NodeId, String> {
    let tree = rt.tree_mut();
    let root = tree.root();

    // 主题节点（八槽位深色缺省 —— 显式存在示范序列化形态；换它即换肤）。
    let _theme = tree.add_node(root, "theme", NodeKind::Theme);

    // 相机置中（世界 1:1）。
    let cam = tree.add_node(root, "cam", NodeKind::Camera2D);
    tree.set_local(cam, Transform2D::from_pos(256.0, 144.0));

    // 背景（铺满）+ 面板。
    let bg = tree.add_node(root, "bg", NodeKind::Control);
    tree.set_prop(bg, "size", Value::Vec2(SVec2::new(512.0, 288.0)))
        .unwrap();
    tree.set_prop(bg, "fill_slot", Value::Str("bg".into())).unwrap();
    let panel = tree.add_node(root, "panel", NodeKind::Control);
    tree.set_prop(panel, "offset", Value::Vec2(SVec2::new(16.0, 16.0)))
        .unwrap();
    tree.set_prop(panel, "size", Value::Vec2(SVec2::new(480.0, 256.0)))
        .unwrap();
    tree.set_prop(panel, "fill_slot", Value::Str("panel".into()))
        .unwrap();

    // 标题 / 状态行。
    let title = tree.add_node(root, "title", NodeKind::Label);
    tree.set_local(title, Transform2D::from_pos(32.0, 32.0));
    tree.set_prop(title, "text", Value::Str("S12.1 WIDGETS".into()))
        .unwrap();
    let status = tree.add_node(root, "status", NodeKind::Label);
    tree.set_local(status, Transform2D::from_pos(32.0, 240.0));
    tree.set_prop(status, "text", Value::Str("CLICK A BUTTON".into()))
        .unwrap();
    tree.set_prop(status, "color_slot", Value::Str("text_dim".into()))
        .unwrap();

    // 三个按钮（4px 栅格；四态着色由 UiVm + 提取层自动完成）。
    let mut buttons = BTreeMap::new();
    for (i, (name, text)) in [("ok", "OK"), ("cancel", "CANCEL"), ("danger", "DANGER")]
        .iter()
        .enumerate()
    {
        let b = tree.add_node(root, name, NodeKind::Button);
        tree.set_prop(
            b,
            "offset",
            Value::Vec2(SVec2::new(32.0 + i as f32 * 152.0, 120.0)),
        )
        .unwrap();
        tree.set_prop(b, "size", Value::Vec2(SVec2::new(140.0, 28.0)))
            .unwrap();
        tree.set_prop(b, "text", Value::Str((*text).into())).unwrap();
        if *name == "danger" {
            tree.set_prop(b, "border_slot", Value::Str("danger".into()))
                .unwrap();
            tree.set_prop(b, "text_slot", Value::Str("danger".into()))
                .unwrap();
        }
        buttons.insert(b, (*name).to_string());
    }
    tree.apply_pending();
    buttons
}
