//! 引擎闭环出口准则：**真实场景树 + 磁盘资产**驱动整条渲染管线（E-Loop 系列）。
//!
//! | 编号 | 验证什么 |
//! |---|---|
//! | E-Loop-01 | 全链路首帧：场景精灵（磁盘 BMP 经 nes-asset 加载 -> GPU 注册）-> 提取 -> 命令 -> 像素 |
//! | E-Loop-02 | 热重载闭环：改磁盘文件 -> poll_reloads -> 重传 -> 下一帧像素变化 |
//! | E-Loop-03 | 场景结构变化：删节点 -> 下一帧渲染物消失（生命周期贯通） |
//! | E-Loop-04 | 父子变换复合：容器平移 + 子精灵局部变换 -> 世界矩阵贯通到像素 |
//!
//! 资产全部在临时目录内程序化生成（`write_bmp_rgba`），无外部文件依赖，
//! 断言可精确到像素。

use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::{PROP_FLIP_H, PROP_TEXTURE};
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::{NodeKind, Transform2D, Value};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
/// 首版纹理四象限：黄 / 青 / 亮灰 / 暗灰。
const V1: [[u8; 4]; 4] = [
    [255, 255, 0, 255],
    [0, 255, 255, 255],
    [200, 200, 200, 255],
    [80, 80, 80, 255],
];
/// 热重载后四象限：反转色（任何采样错乱都会立刻显形）。
const V2: [[u8; 4]; 4] = [
    [0, 0, 255, 255],
    [255, 0, 255, 255],
    [55, 55, 55, 255],
    [175, 175, 175, 255],
];

/// 16x16 四象限纹理（象限色按 [TL, TR, BL, BR]）。
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

/// 装配运行时 + 一张已上传的纹理 + 精灵 + 相机，返回 (runtime, 精灵节点)。
/// 在独立子目录装配。
///
/// **为什么必须隔离**（M5 收官记录 §3.1 的竞态实证）：Asset Pipeline 已真实
/// 对磁盘内容变化敏感 —— 多用例共享临时目录时，"改写磁盘资产的用例"与
/// "正在加载资产的用例"会互相污染（首帧用例读到热重载后的颜色）。裁决是
/// **修测试层、不改 Runtime 语义**：引擎对磁盘敏感是热重载的特性，不是缺陷。
fn assemble_in(dir: &str, colors: &[[u8; 4]; 4]) -> Option<(NesRuntime, nes_scene::NodeId)> {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_engine")
        .join(dir);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(colors))
        .expect("写演示纹理");

    let mut rt = NesRuntime::open_with_root(&root, 64, 64).ok()?;
    let res = rt.declare_texture("Textures/demo.bmp").expect("声明纹理");
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 1, "磁盘纹理应加载成功");
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    let tree = rt.tree_mut();
    let root_node = tree.root();
    let sprite = tree.add_node(root_node, "player", NodeKind::Sprite2D);
    tree.set_prop(sprite, PROP_TEXTURE, res.to_value())
        .expect("绑定纹理属性");
    tree.set_local(sprite, Transform2D::from_pos(10.0, 10.0));
    let camera = tree.add_node(root_node, "cam", NodeKind::Camera2D);
    tree.set_local(camera, Transform2D::from_pos(32.0, 32.0));
    Some((rt, sprite))
}

/// E-Loop-01：全链路首帧 —— 磁盘 BMP -> 资产注册表 -> GPU 注册表 -> 提取 ->
/// 命令流 -> 像素，四象限逐象限核对。
#[test]
fn e_loop_01_first_frame_end_to_end() {
    let Some((mut rt, _sprite)) = assemble_in("e1", &V1) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let frame = FrameInfo::new(0, 1.0 / 60.0, 0.0, Vec2::new(64.0, 64.0));
    let outcome = rt.frame(&frame).expect("首帧");
    assert_eq!(outcome.stats.drawn, 1, "场景里一个精灵");
    assert_eq!(outcome.stats.from_registry, 1, "采样的是真实纹理（非内建格）");
    assert!(outcome.stats.camera_applied, "场景相机生效");
    assert_eq!(outcome.stats.driver_errors, 0);

    let image = &outcome.image;
    // 精灵 (10,10)-(25,25)，四象限 8px 分界。
    assert_eq!(image.pixel(12, 12), Some(V1[0]), "左上象限（黄）");
    assert_eq!(image.pixel(22, 12), Some(V1[1]), "右上象限（青）");
    assert_eq!(image.pixel(12, 22), Some(V1[2]), "左下象限（亮灰）");
    assert_eq!(image.pixel(22, 22), Some(V1[3]), "右下象限（暗灰）");
    assert_eq!(image.pixel(9, 9), Some(CLEAR_RGBA), "精灵外是背景");
}

/// E-Loop-02：热重载闭环 —— 改磁盘文件 -> poll -> 重传 -> 下一帧像素反转。
#[test]
fn e_loop_02_hot_reload_reaches_pixels() {
    let Some((mut rt, _sprite)) = assemble_in("e2", &V1) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let frame = FrameInfo::new(0, 1.0 / 60.0, 0.0, Vec2::new(64.0, 64.0));
    let before = rt.frame(&frame).expect("首帧");
    assert_eq!(before.image.pixel(12, 12), Some(V1[0]));

    // 改磁盘文件（内容变化 -> 内容戳变化 -> 重载）。
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_engine")
        .join("e2");
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V2))
        .expect("改写纹理");
    let report = rt.poll_reloads();
    assert_eq!(report.reloaded.len(), 1, "应检测到一次重载");
    assert_eq!(rt.upload_pending_textures().expect("重传"), 1, "新版本应重传");

    let after = rt.frame(&frame).expect("热重载帧");
    let image = &after.image;
    assert_eq!(image.pixel(12, 12), Some(V2[0]), "左上变蓝");
    assert_eq!(image.pixel(22, 12), Some(V2[1]), "右上变品红");
    assert_eq!(image.pixel(22, 22), Some(V2[3]), "右下变亮");
    assert_eq!(after.stats.driver_errors, 0);
}

/// E-Loop-03：场景结构变化 —— 删精灵节点 -> 下一帧渲染物消失。
#[test]
fn e_loop_03_node_removal_stops_rendering() {
    let Some((mut rt, sprite)) = assemble_in("e3", &V1) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let frame = FrameInfo::new(0, 1.0 / 60.0, 0.0, Vec2::new(64.0, 64.0));
    let alive = rt.frame(&frame).expect("首帧");
    assert_eq!(alive.stats.drawn, 1);

    rt.tree_mut().remove_node(sprite, false);
    let dead = rt.frame(&frame).expect("删除后帧");
    assert_eq!(dead.stats.drawn, 0, "节点删除 -> 渲染物销毁");
    assert_eq!(dead.image.pixel(12, 12), Some(CLEAR_RGBA));
}

/// E-Loop-04：父子变换复合 —— 容器 (16,16) + 子精灵局部 (0,0) + 水平翻转。
#[test]
fn e_loop_04_parent_transform_and_flip() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_engine")
        .join("e4");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写演示纹理");

    let mut rt = match NesRuntime::open_with_root(&root, 64, 64) {
        Ok(rt) => rt,
        Err(_) => {
            eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
            return;
        }
    };
    let res = rt.declare_texture("Textures/demo.bmp").expect("声明");
    rt.bind_assets();
    rt.upload_pending_textures().expect("上传");

    let tree = rt.tree_mut();
    let root_node = tree.root();
    let container = tree.add_node(root_node, "world", NodeKind::Node2D);
    tree.set_local(container, Transform2D::from_pos(16.0, 16.0));
    let sprite = tree.add_node(container, "child", NodeKind::Sprite2D);
    tree.set_prop(sprite, PROP_TEXTURE, res.to_value())
        .expect("绑定纹理");
    tree.set_local(sprite, Transform2D::from_pos(0.0, 0.0));
    // 水平翻转：象限左右互换（绕精灵原点镜像，画面占据 x ∈ [0,16)）。
    tree.set_prop(sprite, PROP_FLIP_H, Value::Bool(true)).expect("翻转");
    let camera = tree.add_node(root_node, "cam", NodeKind::Camera2D);
    tree.set_local(camera, Transform2D::from_pos(32.0, 32.0));

    let frame = FrameInfo::new(0, 1.0 / 60.0, 0.0, Vec2::new(64.0, 64.0));
    let outcome = rt.frame(&frame).expect("复合帧");
    assert_eq!(outcome.stats.drawn, 1);
    let image = &outcome.image;
    // 世界 = 容器(16,16) ∘ 子(0,0) ∘ flip 镜像 -> 精灵占据 x ∈ [0,16)、y ∈ [16,32)。
    // 翻转后：画面左半（x<8）采到纹理右列象限（TR/BR），右半采到左列（TL/BL）。
    assert_eq!(image.pixel(2, 18), Some(V1[1]), "画面左上 <- 纹理右上（青）");
    assert_eq!(image.pixel(12, 18), Some(V1[0]), "画面右上 <- 纹理左上（黄）");
    assert_eq!(image.pixel(2, 28), Some(V1[3]), "画面左下 <- 纹理右下（暗灰）");
    assert_eq!(image.pixel(12, 28), Some(V1[2]), "画面右下 <- 纹理左下（亮灰）");
    assert_eq!(image.pixel(18, 18), Some(CLEAR_RGBA), "精灵（翻转后）右侧是背景");
}
