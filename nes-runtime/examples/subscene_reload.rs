//! S6.7 演示：子场景热重载 —— 改子场景文件，整树自动重载重展开。
//!
//! 流程：加载父场景（引用子场景）-> 渲染出图 -> **改写子场景文件**
//!（精灵挪位 + 加一个新精灵）-> `poll_scene_reload` 触发整树重载 ->
//! 下一帧新结构入画。两个渲染 PNG 是取证产物：
//!
//! 1. `nes-runtime/output/subscene_before.png`（单精灵，包装 (8,8) 复合）
//! 2. `nes-runtime/output/subscene_after.png`（双精灵，新布局）
//!
//! 运行：`cargo run --example subscene_reload`

use nes_render_api::{FrameInfo, Vec2};
use nes_runtime::{write_bmp_rgba, NesRuntime};

/// 四象限 16x16：黄 / 青 / 亮灰 / 暗灰。
fn quadrant_rgba() -> Vec<u8> {
    let colors = [
        [255, 255, 0, 255],
        [0, 255, 255, 255],
        [200, 200, 200, 255],
        [80, 80, 80, 255],
    ];
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            rgba.extend_from_slice(&colors[if y < 8 { 0 } else { 2 } + if x >= 8 { 1 } else { 0 }]);
        }
    }
    rgba
}

const CHILD_V1: &str = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/quads.bmp", kind: "Texture"),
    ],
    root: Node(
        name: "child_root",
        kind: "Node2D",
        children: [
            Node(
                name: "sprite",
                kind: "Sprite2D",
                props: { "texture": Resource(1), },
                children: [],
            ),
        ],
    ),
)
"#;

/// 热重载后的子场景：精灵挪到 (16,0)，并新增一个 (0,24) 的精灵。
const CHILD_V2: &str = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/quads.bmp", kind: "Texture"),
    ],
    root: Node(
        name: "child_root",
        kind: "Node2D",
        children: [
            Node(
                name: "sprite",
                kind: "Sprite2D",
                local: (x: 16.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "texture": Resource(1), },
                children: [],
            ),
            Node(
                name: "sprite2",
                kind: "Sprite2D",
                local: (x: 0.0, y: 24.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "texture": Resource(1), },
                children: [],
            ),
        ],
    ),
)
"#;

const PARENT: &str = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Scenes/child.ron", kind: "Scene"),
    ],
    root: Node(
        name: "main",
        kind: "Node",
        children: [
            Node(
                name: "cam",
                kind: "Camera2D",
                local: (x: 32.0, y: 32.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                children: [],
            ),
            Node(
                name: "instance",
                kind: "Node2D",
                local: (x: 8.0, y: 8.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "sub_scene": Resource(1), },
                children: [],
            ),
        ],
    ),
)
"#;

fn main() {
    // 1) 资产根：纹理 + 父子场景文件。
    let root = std::env::temp_dir().join("nes_runtime_subscene_reload");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    std::fs::create_dir_all(root.join("Scenes")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("quads.bmp"), 16, 16, &quadrant_rgba())
        .expect("写四象限纹理");
    std::fs::write(root.join("Scenes").join("child.ron"), CHILD_V1).expect("写子场景");
    std::fs::write(root.join("Scenes").join("parent.ron"), PARENT).expect("写父场景");

    let mut rt = match NesRuntime::open_with_root(&root, 64, 64) {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("装配失败（如实报告）：{err}");
            std::process::exit(1);
        }
    };
    let report = rt.load_scene("Scenes/parent.ron").expect("加载父场景");
    assert!(report.is_clean(), "引用应有声明：{report:?}");
    let bound = rt.bind_assets();
    println!("[加载] 资产 {} 项（含子场景文件本身）", bound.loaded.len());
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    // 2) 首帧 + 取证 PNG。
    let frame = FrameInfo::new(0, 1.0 / 60.0, 0.0, Vec2::new(64.0, 64.0));
    let before = rt.frame(&frame).expect("首帧");
    println!("[首帧] drawn={}（单精灵，包装 (8,8) 复合）", before.stats.drawn);
    nes_render_wgpu::write_rgba8_png(
        std::path::Path::new("output/subscene_before.png"),
        64,
        64,
        &before.image.rgba,
    )
    .expect("写 PNG");

    // 3) 改写子场景文件（编辑器保存的模拟），轮询触发整树重载。
    std::fs::write(root.join("Scenes").join("child.ron"), CHILD_V2).expect("改写子场景");
    let reloaded = rt
        .poll_scene_reload()
        .expect("轮询")
        .expect("子场景变化应触发整树重载");
    println!("[重载] 来源 {reloaded}：重新解析 + 重新展开 + 全量替换");
    let uploaded = rt.upload_pending_textures().expect("重传");
    println!("[重传] {uploaded} 张纹理（账目随替换清零）");

    // 4) 重载后首帧 + 取证 PNG。
    let frame1 = FrameInfo::new(1, 1.0 / 60.0, 1.0 / 60.0, Vec2::new(64.0, 64.0));
    let after = rt.frame(&frame1).expect("重载后首帧");
    println!("[重载帧] drawn={}（双精灵：sprite 挪到 (24,8)、sprite2 在 (8,32)）", after.stats.drawn);
    nes_render_wgpu::write_rgba8_png(
        std::path::Path::new("output/subscene_after.png"),
        64,
        64,
        &after.image.rgba,
    )
    .expect("写 PNG");

    // 5) 像素锚点断言。
    assert_eq!(after.stats.drawn, 2, "新子场景的双精灵");
    assert_eq!(after.image.pixel(26, 10), Some([255, 255, 0, 255]), "sprite 新位左上（黄）");
    assert_eq!(after.image.pixel(10, 34), Some([255, 255, 0, 255]), "sprite2 左上（黄）");
    assert_eq!(after.image.pixel(10, 10), Some([13, 13, 25, 255]), "旧位已空");
    println!("[锚点] (26,10) 黄 / (10,34) 黄 / (10,10) 背景 —— 子场景热重载直达像素");
}
