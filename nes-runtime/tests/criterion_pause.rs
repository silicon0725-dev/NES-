//! T-Pause-R 契约回归：暂停/时间缩放接入**运行时帧循环**（S6.4）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Pause-R1 | 暂停期间帧循环照常渲染（drawn/像素/驱动健康不变），行为回调停跳（动画冻结在暂停点） |
//! | T-Pause-R2 | `time_scale` 单点缩放：宿主每帧传恒定 delta，观察者收到的是缩放后的值（帧循环不做二次缩放） |
//!
//! 场景层语义由 `nes-scene/tests/s6_process.rs`（T-Pause-01..08）钉死；
//! 本组证明组装层接线：暂停是 `tree_mut()` 上的一个开关，帧循环形状不变。

use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::PROP_TEXTURE;
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::{NodeCtx, NodeId, NodeKind, SceneObserver, Transform2D};

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

/// 装配：纹理 + 精灵(10,10) + 单位相机（独立子目录）。
fn assemble_in(dir: &str) -> Option<(NesRuntime, nes_scene::NodeId)> {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_pause")
        .join(dir);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写演示纹理");

    let mut rt = NesRuntime::open_with_root(&root, 64, 64).ok()?;
    let res = rt.declare_texture("Textures/demo.bmp").expect("声明纹理");
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 1);
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);
    let tree = rt.tree_mut();
    let sprite = tree.add_node(tree.root(), "player", NodeKind::Sprite2D);
    tree.set_prop(sprite, PROP_TEXTURE, res.to_value()).unwrap();
    tree.set_local(sprite, Transform2D::from_pos(10.0, 10.0));
    let camera = tree.add_node(tree.root(), "cam", NodeKind::Camera2D);
    tree.set_local(camera, Transform2D::from_pos(32.0, 32.0));
    Some((rt, sprite))
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(64.0, 64.0))
}

/// 每帧把精灵平移 16px（Pausable 生效模式 —— 缺省 Inherit）。
struct Mover {
    sprite: NodeId,
}

impl SceneObserver for Mover {
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if ctx.this() == self.sprite {
            ctx.translate(16.0, 0.0);
        }
    }
}

/// 记录观察者收到的 delta 序列（最后一个精灵回调为准）。
struct DeltaProbe {
    deltas: Vec<f32>,
}

impl SceneObserver for DeltaProbe {
    fn on_process(&mut self, _ctx: &mut NodeCtx<'_>, delta: f32) {
        self.deltas.push(delta);
    }
}

/// T-Pause-R1：暂停 = 行为停跳 + 渲染照常。像素冻结在暂停点，
/// `drawn`/`driver_errors` 逐帧不变。
#[test]
fn t_pause_r1_pause_freezes_behavior_not_rendering() {
    let Some((mut rt, sprite)) = assemble_in("r1") else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let mut mover = Mover { sprite };

    // 正常两帧：精灵 (10,10) -> (26,10) -> (42,10)。
    let f1 = rt.frame_with(&frame(0), &mut mover).expect("帧 0");
    assert_eq!(f1.image.pixel(28, 12), Some(V1[0]), "帧 0 后精灵在 (26,10)");
    let f2 = rt.frame_with(&frame(1), &mut mover).expect("帧 1");
    assert_eq!(f2.image.pixel(44, 12), Some(V1[0]), "帧 1 后精灵在 (42,10)");

    // 暂停：行为停跳，渲染照常出帧。
    rt.tree_mut().set_paused(true);
    let f3 = rt.frame_with(&frame(2), &mut mover).expect("暂停帧 2");
    let f4 = rt.frame_with(&frame(3), &mut mover).expect("暂停帧 3");
    for (label, outcome) in [("帧2", &f3), ("帧3", &f4)] {
        assert_eq!(outcome.stats.drawn, 1, "{label}：渲染照常");
        assert_eq!(outcome.stats.driver_errors, 0, "{label}：驱动健康");
        assert_eq!(outcome.image.pixel(44, 12), Some(V1[0]), "{label}：像素冻结在暂停点");
        assert_eq!(outcome.image.pixel(28, 12), Some(CLEAR_RGBA), "{label}：旧位置已空");
    }

    // 恢复：从冻结点继续走。
    rt.tree_mut().set_paused(false);
    let f5 = rt.frame_with(&frame(4), &mut mover).expect("恢复帧 4");
    assert_eq!(f5.image.pixel(60, 12), Some(V1[0]), "恢复后精灵在 (58,10)");
}

/// T-Pause-R2：`time_scale` 是树内的**单点**缩放 —— 帧循环照传原始 delta，
/// 观察者收到缩放后的值（无二次缩放）。
#[test]
fn t_pause_r2_time_scale_single_point() {
    let Some((mut rt, _sprite)) = assemble_in("r2") else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let mut probe = DeltaProbe { deltas: Vec::new() };
    rt.frame_with(&frame(0), &mut probe).expect("帧 0");
    let base = probe.deltas[0];
    assert!((base - 1.0 / 60.0).abs() < 1e-6, "缺省直通");

    probe.deltas.clear();
    rt.tree_mut().set_time_scale(0.5);
    rt.frame_with(&frame(1), &mut probe).expect("帧 1（半速）");
    assert!(
        probe.deltas.iter().all(|d| (d - base * 0.5).abs() < 1e-6),
        "全部回调收到半速 delta：{:?}",
        probe.deltas
    );
}

/// T-Pause-R3：调度语义经磁盘往返后生效 —— 场景文件里 `process_mode:
/// "Always"` 的精灵，加载后暂停期间**照常被行为回调驱动**（像素继续移动），
/// 对照缺省（Inherit->Pausable）节点冻结。序列化口径的端到端证明。
#[test]
fn t_pause_r3_process_mode_survives_disk_roundtrip() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_pause")
        .join("r3");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    std::fs::create_dir_all(root.join("Scenes")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写演示纹理");
    let scene = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/demo.bmp", kind: "Texture"),
    ],
    root: Node(
        name: "main",
        kind: "Node",
        props: {},
        children: [
            Node(
                name: "cam",
                kind: "Camera2D",
                local: (x: 32.0, y: 32.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: {},
                children: [],
            ),
            Node(
                name: "player",
                kind: "Sprite2D",
                process_mode: "Always",
                local: (x: 10.0, y: 10.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "texture": Resource(1), },
                children: [],
            ),
        ],
    ),
)
"#;
    std::fs::write(root.join("Scenes").join("paused.ron"), scene).expect("写场景");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let report = rt.load_scene("Scenes/paused.ron").expect("加载场景");
    assert!(report.is_clean());
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    let sprite = rt
        .tree_mut()
        .find_by_name("player")
        .expect("场景里的精灵");
    // 加载后模式复原为 Always（未暂停先验证一次生效模式）
    assert_eq!(
        rt.tree_mut().effective_process_mode(sprite),
        nes_scene::ProcessMode::Always,
        "磁盘往返后模式复原"
    );

    // 暂停：Always 精灵**照常**被行为回调驱动（像素继续移动）。
    rt.tree_mut().set_paused(true);
    let mut mover = Mover { sprite };
    let f1 = rt.frame_with(&frame(0), &mut mover).expect("暂停帧 0");
    assert_eq!(f1.stats.drawn, 1, "渲染照常");
    assert_eq!(f1.image.pixel(28, 12), Some(V1[0]), "暂停期间精灵移动到 (26,10)");
    let f2 = rt.frame_with(&frame(1), &mut mover).expect("暂停帧 1");
    assert_eq!(f2.image.pixel(44, 12), Some(V1[0]), "继续移动到 (42,10)");
    assert_eq!(f2.stats.driver_errors, 0);
}
