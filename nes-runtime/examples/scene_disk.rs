//! S6.3 演示：**磁盘场景文件驱动渲染**（场景序列化闭环）。
//!
//! 与 E-Loop 测试的分工：本例面向人 —— 手写一份 .ron 场景落盘（含资源声明
//! 段、父子容器复合），`load_scene` 实例化后渲染出 PNG，再把**当前树**回存成
//! 第二份场景文件。三个产物可直接查看：
//!
//! 1. 手写场景（事实来源）：`%TEMP%/nes_runtime_scene_disk/Scenes/handwritten.ron`
//! 2. 渲染结果：`nes-runtime/output/scene_disk.png`
//! 3. 回存场景（树 -> 文件）：`%TEMP%/nes_runtime_scene_disk/Scenes/roundtrip.ron`
//!
//! 运行：`cargo run --example scene_disk`

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

/// 橙/深蓝 4px 棋盘 16x16。
fn checker_rgba() -> Vec<u8> {
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            rgba.extend_from_slice(if (x / 4 + y / 4) % 2 == 0 {
                &[255, 140, 0, 255]
            } else {
                &[40, 40, 60, 255]
            });
        }
    }
    rgba
}

/// 手写场景：容器(8,8) 下两个精灵（四象限 + 棋盘），单位相机。
///
/// 语法与 `doc_to_ron` 输出一致 —— 手写文件与打包文件是同一门语言。
/// 槽位 1/2 经 `resources` 段声明，属性里的 `Resource(n)` 与磁盘文件对应。
const SCENE: &str = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/quads.bmp", kind: "Texture"),
        Res(id: 2, path: "Textures/checker.bmp", kind: "Texture"),
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
                name: "world",
                kind: "Node2D",
                local: (x: 8.0, y: 8.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: {},
                children: [
                    Node(
                        name: "quads",
                        kind: "Sprite2D",
                        local: (x: 0.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                        props: { "texture": Resource(1), },
                        children: [],
                    ),
                    Node(
                        name: "checker",
                        kind: "Sprite2D",
                        local: (x: 24.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                        props: { "texture": Resource(2), },
                        children: [],
                    ),
                ],
            ),
        ],
    ),
)
"#;

fn main() {
    // 1) 资产根：两张演示纹理 + 手写场景文件。
    let root = std::env::temp_dir().join("nes_runtime_scene_disk");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    std::fs::create_dir_all(root.join("Scenes")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("quads.bmp"), 16, 16, &quadrant_rgba())
        .expect("写四象限纹理");
    write_bmp_rgba(&root.join("Textures").join("checker.bmp"), 16, 16, &checker_rgba())
        .expect("写棋盘纹理");
    std::fs::write(root.join("Scenes").join("handwritten.ron"), SCENE).expect("写场景文件");
    println!(
        "[场景] 手写文件已落盘：{}",
        root.join("Scenes").join("handwritten.ron").display()
    );

    // 2) 从磁盘实例化 -> 绑定 -> 上传。
    let mut rt = match NesRuntime::open_with_root(&root, 64, 64) {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("装配失败（如实报告）：{err}");
            std::process::exit(1);
        }
    };
    let report = rt.load_scene("Scenes/handwritten.ron").expect("加载场景");
    assert!(report.is_clean(), "手写场景的引用应全部有声明：{report:?}");
    let bound = rt.bind_assets();
    println!("[资产] 加载 {} 张纹理", bound.loaded.len());
    assert_eq!(rt.upload_pending_textures().expect("上传"), 2);

    // 3) 渲染首帧。
    let frame = FrameInfo::new(0, 1.0 / 60.0, 0.0, Vec2::new(64.0, 64.0));
    let outcome = rt.frame(&frame).expect("渲染首帧");
    println!(
        "[渲染] drawn={} from_registry={} camera={} driver_errors={}",
        outcome.stats.drawn,
        outcome.stats.from_registry,
        outcome.stats.camera_applied,
        outcome.stats.driver_errors
    );

    // 4) 产物一：渲染 PNG。
    let out = std::path::Path::new("output/scene_disk.png");
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    nes_render_wgpu::write_rgba8_png(out, 64, 64, &outcome.image.rgba).expect("写 PNG");
    println!(
        "[产物] 渲染图：{}",
        std::fs::canonicalize(out).unwrap_or_else(|_| out.to_path_buf()).display()
    );

    // 5) 产物二：当前树回存（磁盘 -> 渲染 -> 磁盘）。
    rt.save_scene("Scenes/roundtrip.ron").expect("回存场景");
    println!("[产物] 回存场景：{}", root.join("Scenes").join("roundtrip.ron").display());

    // 6) 像素锚点断言（示例自带验证，不依赖人工目视）：
    //    容器 (8,8) 复合子 local -> quads 世界 (8,8)-[24)、checker 世界 (32,8)-[48)。
    let image = &outcome.image;
    assert_eq!(outcome.stats.drawn, 2, "两个精灵");
    assert_eq!(outcome.stats.from_registry, 2, "都采样磁盘纹理");
    assert_eq!(outcome.stats.driver_errors, 0);
    assert_eq!(image.pixel(10, 10), Some([255, 255, 0, 255]), "四象限左上（黄）");
    assert_eq!(image.pixel(34, 10), Some([255, 140, 0, 255]), "棋盘首格（橙）");
    assert_eq!(image.pixel(4, 4), Some([13, 13, 25, 255]), "精灵外是背景");
    println!("[锚点] (10,10) 黄 / (34,10) 橙 / (4,4) 背景 —— 全部通过");
}
