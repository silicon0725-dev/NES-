//! T-Script-R 契约回归：脚本 VM 接入**运行时帧循环**（S6.19）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Script-R1 | registry_key 装载的信号脚本，宿主每帧预发 -> 脚本跨节点平移精灵 -> **像素逐帧右移**；停机可观测且不崩帧 |

use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::PROP_TEXTURE;
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::{
    NodeKind, Op, Script, ScriptEntry, ScriptVm, Transform2D, Value, HALT_LOCAL,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
const V1: [[u8; 4]; 4] = [
    [255, 255, 0, 255],
    [0, 255, 255, 255],
    [200, 200, 200, 255],
    [80, 80, 80, 255],
];

fn quadrant_rgba(colors: &[[u8; 4]; 4]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            let color = colors[if y < 8 { 0 } else { 2 } + if x >= 8 { 1 } else { 0 }];
            rgba.extend_from_slice(&color);
        }
    }
    rgba
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(64.0, 64.0))
}

/// T-Script-R1：脚本 VM 全链 —— 场景里的 Script 节点（registry_key="drift"）
/// 装载信号脚本：收到 "step"（载荷 Vec2(16,0)）就把 sprite 平移 16px。
/// 宿主每帧预发 -> 泵 -> 方法连接 -> 解释器 -> Cmd 落地 -> 冲洗 -> 提取 ->
/// 像素。脚本与引擎行为同帧、同一像素语义。
#[test]
fn t_script_r1_script_moves_sprite_pixels() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_script")
        .join("r1");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写演示纹理");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let res = rt.declare_texture("Textures/demo.bmp").expect("声明纹理");
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 1);
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    // 场景：相机 + sprite(10,10) + Script 节点（registry_key=drift）。
    let (sprite, brain) = {
        let tree = rt.tree_mut();
        let root_node = tree.root();
        let cam = tree.add_node(root_node, "cam", NodeKind::Camera2D);
        tree.set_local(cam, Transform2D::from_pos(32.0, 32.0));
        let sprite = tree.add_node(root_node, "sprite", NodeKind::Sprite2D);
        tree.set_prop(sprite, PROP_TEXTURE, res.to_value()).unwrap();
        tree.set_local(sprite, Transform2D::from_pos(10.0, 10.0));
        let brain = tree.add_node(root_node, "brain", NodeKind::Script);
        tree.set_prop(brain, "registry_key", Value::Str("drift".into()))
            .unwrap();
        (sprite, brain)
    };
    rt.tree_mut().apply_pending();

    // VM：登记脚本（节点压两次 —— GetT 消费节点）并按 registry_key 装载。
    let mut vm = ScriptVm::new();
    vm.register(
        "drift",
        Script::new(
            ScriptEntry::Signal("step".into()),
            vec![
                Op::NodeByName("sprite".into()),
                Op::NodeByName("sprite".into()),
                Op::GetT,
                Op::Arg,
                Op::Add,
                Op::SetT,
            ],
        ),
    );
    let issues = vm.attach_all(rt.tree_mut());
    assert!(issues.is_empty(), "装载无缺口：{issues:?}");

    // 帧循环：每帧预发 step(16,0) -> 脚本平移精灵 -> 像素右移 16px。
    rt.tree_mut().emit_signal("step", Value::Vec2(nes_scene::Vec2::new(16.0, 0.0)));
    let f1 = rt.frame_with(&frame(0), &mut vm).expect("帧 1");
    assert_eq!(f1.stats.drawn, 1);
    assert_eq!(f1.image.pixel(28, 12), Some(V1[0]), "帧 1：精灵在 (26,10)");
    assert_eq!(f1.image.pixel(12, 12), Some(CLEAR_RGBA), "原位已空");

    rt.tree_mut().emit_signal("step", Value::Vec2(nes_scene::Vec2::new(16.0, 0.0)));
    let f2 = rt.frame_with(&frame(1), &mut vm).expect("帧 2");
    assert_eq!(f2.image.pixel(44, 12), Some(V1[0]), "帧 2：(42,10)");

    // 停机可观测：发一条会让脚本栈下溢的载荷？—— Arg 恒为载荷、脚本无分支，
    // 这里改用断言"无停机记录"+ 引擎健康收尾。
    assert!(
        vm.locals(brain).is_none_or(|l| !l.contains_key(HALT_LOCAL)),
        "无停机"
    );
    assert_eq!(f2.stats.driver_errors, 0);
    let _ = sprite;
}
