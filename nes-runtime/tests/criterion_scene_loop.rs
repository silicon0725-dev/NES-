//! T-Scene 契约回归：场景序列化闭环 —— SceneDoc 从磁盘实例化 -> 渲染（S6.3）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Scene-01 | 手写 RON 落盘 -> `load_scene` -> bind -> upload -> 首帧像素：纹理槽位身份经磁盘往返后三处同源 |
//! | T-Scene-02 | 悬垂引用如实报告：`Resource(n)` 无声明 -> `undeclared` 非空、无纹理可传、精灵不入画 |
//! | T-Scene-03 | 往返闭环：程序化搭树 -> `save_scene` -> 全新运行时 `load_scene` -> 像素与源一致 |
//! | T-Scene-04 | 替换语义：`load_scene` 全量替换树/表/上传账目 —— 新场景布局生效、纹理重传 |
//!
//! `scene_io` 的解析/打包语义已在 `nes-scene/tests/m2.rs`/`m3.rs` 钉死；
//! 本组证明的是**组装层把它接进帧循环**：磁盘文件成为场景的事实来源。

use nes_render_api::{FrameInfo, Vec2};
use nes_runtime::{write_bmp_rgba, NesRuntime};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
/// 四象限纹理：黄 / 青 / 亮灰 / 暗灰。
const V1: [[u8; 4]; 4] = [
    [255, 255, 0, 255],
    [0, 255, 255, 255],
    [200, 200, 200, 255],
    [80, 80, 80, 255],
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

/// 建资产根（含 demo.bmp），返回根路径（独立子目录，沿用 M5 §3.1 隔离裁决）。
fn make_root(dir: &str) -> std::path::PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_scene")
        .join(dir);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    std::fs::create_dir_all(root.join("Scenes")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写演示纹理");
    root
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(64.0, 64.0))
}

/// 手写场景文件（与 `doc_to_ron` 同语法）：单位相机 + (10,10) 精灵引用槽位 1。
const SCENE_A: &str = r#"Scene(
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
                local: (x: 10.0, y: 10.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "texture": Resource(1), },
                children: [],
            ),
        ],
    ),
)
"#;

/// T-Scene-01：磁盘场景文件 -> 首帧像素。`Resource(1)` 经声明表 -> 资产键 ->
/// GPU 注册表三处位对齐，磁盘往返后身份同源。
#[test]
fn t_scene_01_disk_scene_first_frame() {
    let root = make_root("s1");
    std::fs::write(root.join("Scenes").join("a.ron"), SCENE_A).expect("写场景文件");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let report = rt.load_scene("Scenes/a.ron").expect("加载场景");
    assert!(report.is_clean(), "体检报告应干净：{report:?}");
    let bound = rt.bind_assets();
    assert_eq!(bound.loaded.len(), 1, "磁盘纹理加载成功");
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    let outcome = rt.frame(&frame(0)).expect("首帧");
    assert_eq!(outcome.stats.drawn, 1, "场景里一个精灵");
    assert_eq!(outcome.stats.from_registry, 1, "采样真实纹理");
    assert!(outcome.stats.camera_applied, "场景相机生效");
    let image = &outcome.image;
    assert_eq!(image.pixel(12, 12), Some(V1[0]), "左上象限（黄）");
    assert_eq!(image.pixel(22, 12), Some(V1[1]), "右上象限（青）");
    assert_eq!(image.pixel(12, 22), Some(V1[2]), "左下象限（亮灰）");
    assert_eq!(image.pixel(22, 22), Some(V1[3]), "右下象限（暗灰）");
    assert_eq!(image.pixel(9, 9), Some(CLEAR_RGBA), "精灵外是背景");
}

/// T-Scene-02：悬垂引用（`Resource(7)` 无声明）如实报告 —— 不静默、不崩溃、
/// 该精灵不入画。
#[test]
fn t_scene_02_dangling_reference_reported() {
    let root = make_root("s2");
    let scene = r#"Scene(
    version: 1,
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
                name: "ghost",
                kind: "Sprite2D",
                local: (x: 10.0, y: 10.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "texture": Resource(7), },
                children: [],
            ),
        ],
    ),
)
"#;
    std::fs::write(root.join("Scenes").join("dangling.ron"), scene).expect("写场景文件");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let report = rt.load_scene("Scenes/dangling.ron").expect("加载场景");
    assert_eq!(report.undeclared.len(), 1, "悬垂引用要指名：{report:?}");
    assert!(report.mismatches.is_empty());
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 0, "无纹理可传");

    let outcome = rt.frame(&frame(0)).expect("首帧");
    assert_eq!(outcome.stats.drawn, 0, "悬垂精灵不入画");
    assert_eq!(outcome.image.pixel(12, 12), Some(CLEAR_RGBA), "画面是背景");
}

/// T-Scene-03：往返闭环 —— 程序化搭树 -> `save_scene` 落盘 -> 全新运行时
/// `load_scene` -> 像素与直接搭树一致（磁盘 -> 渲染 -> 磁盘 -> 渲染）。
#[test]
fn t_scene_03_roundtrip_through_disk() {
    let root = make_root("s3");

    // 运行时 A：程序化搭树（声明纹理 -> 场景 -> 打包落盘）。
    let Ok(mut rt_a) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let res = rt_a.declare_texture("Textures/demo.bmp").expect("声明纹理");
    {
        let tree = rt_a.tree_mut();
        let root_node = tree.root();
        let camera = tree.add_node(root_node, "cam", nes_scene::NodeKind::Camera2D);
        tree.set_local(camera, nes_scene::Transform2D::from_pos(32.0, 32.0));
        let player = tree.add_node(root_node, "player", nes_scene::NodeKind::Sprite2D);
        tree.set_prop(player, "texture", res.to_value()).unwrap();
        tree.set_local(player, nes_scene::Transform2D::from_pos(10.0, 10.0));
        // add_node 是延迟队列操作：先落地结构再打包（save_scene 序列化的是
        // 已落地的树 —— 见其文档口径）。
        tree.apply_pending();
    }
    rt_a.save_scene("Scenes/round.ron").expect("打包落盘");

    // 运行时 B：从磁盘读回同一场景（共享资产根）。
    let mut rt_b = NesRuntime::open_with_root(&root, 64, 64).expect("运行时 B 装配");
    let report = rt_b.load_scene("Scenes/round.ron").expect("读回场景");
    assert!(report.is_clean(), "往返后的场景应无悬垂：{report:?}");
    let _ = rt_b.bind_assets();
    assert_eq!(rt_b.upload_pending_textures().expect("上传"), 1);

    let outcome = rt_b.frame(&frame(0)).expect("首帧");
    assert_eq!(outcome.stats.drawn, 1);
    let image = &outcome.image;
    assert_eq!(image.pixel(12, 12), Some(V1[0]), "左上象限与源一致");
    assert_eq!(image.pixel(22, 22), Some(V1[3]), "右下象限与源一致");
    assert_eq!(image.pixel(9, 9), Some(CLEAR_RGBA), "布局一致（精灵外背景）");
}

/// T-Scene-04：替换语义 —— `load_scene` 是全量替换：树换新、上传账目清零
/// （同纹理也重传一次）、新布局立即生效、旧节点消失。
#[test]
fn t_scene_04_load_replaces_state() {
    let root = make_root("s4");
    // 场景 B：同一纹理，精灵挪到 (32,32)。
    let scene_b = SCENE_A.replace("x: 10.0, y: 10.0", "x: 32.0, y: 32.0");
    std::fs::write(root.join("Scenes").join("a.ron"), SCENE_A).expect("写场景 A");
    std::fs::write(root.join("Scenes").join("b.ron"), scene_b).expect("写场景 B");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    rt.load_scene("Scenes/a.ron").expect("加载 A");
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传 A"), 1);
    let first = rt.frame(&frame(0)).expect("A 首帧");
    assert_eq!(first.image.pixel(12, 12), Some(V1[0]), "A 布局：(10,10) 处精灵");

    rt.load_scene("Scenes/b.ron").expect("加载 B（替换）");
    let _ = rt.bind_assets();
    assert_eq!(
        rt.upload_pending_textures().expect("上传 B"),
        1,
        "账目清零后同纹理重传一次"
    );
    let second = rt.frame(&frame(1)).expect("B 首帧");
    assert_eq!(second.stats.drawn, 1, "仍是单精灵（旧树已丢弃）");
    assert_eq!(second.image.pixel(34, 34), Some(V1[0]), "B 布局：(32,32) 处精灵");
    assert_eq!(second.image.pixel(12, 12), Some(CLEAR_RGBA), "旧位置已空");
}

/// T-Scene-05：子场景嵌套端到端 —— 父场景引用子场景文件，`load_scene`
/// 递归展开（槽位去重合并）；渲染像素经包装节点复合到位；`save_scene`
/// 回写只留引用（无展开内容）；重载像素一致（磁盘两侧幂等）。
#[test]
fn t_scene_05_nested_subscene_end_to_end() {
    let root = make_root("s5");
    // 子场景：根 Node2D + 精灵(0,0) 引用自己的纹理槽位 1。
    let child = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/demo.bmp", kind: "Texture"),
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
    // 父场景：相机 + 包装节点(8,8) 引用子场景文件（槽位 1 = Scene）。
    let parent = r#"Scene(
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
    std::fs::write(root.join("Scenes").join("child.ron"), child).expect("写子场景");
    std::fs::write(root.join("Scenes").join("parent.ron"), parent).expect("写父场景");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let report = rt.load_scene("Scenes/parent.ron").expect("加载父场景（含展开）");
    assert!(report.is_clean(), "展开后的引用应全部有声明：{report:?}");
    let bound = rt.bind_assets();
    assert_eq!(bound.loaded.len(), 2, "纹理 + 场景文件字节都加载");
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1, "只有纹理上传");

    // 渲染：包装 (8,8) ∘ 子根(单位) ∘ 精灵(0,0) -> 世界 (8,8)。
    let outcome = rt.frame(&frame(0)).expect("首帧");
    assert_eq!(outcome.stats.drawn, 1, "子场景的精灵入画");
    assert_eq!(outcome.stats.from_registry, 1);
    assert_eq!(outcome.image.pixel(10, 10), Some(V1[0]), "精灵经包装复合到 (8,8)");
    assert_eq!(outcome.image.pixel(22, 22), Some(V1[3]), "右下象限");
    assert_eq!(outcome.image.pixel(4, 4), Some(CLEAR_RGBA), "精灵外是背景");

    // 回写：包装节点只留引用，子树不进文件；重载像素一致。
    rt.save_scene("Scenes/parent_saved.ron").expect("回存");
    let saved = std::fs::read_to_string(root.join("Scenes").join("parent_saved.ron")).unwrap();
    assert!(saved.contains("\"sub_scene\": Resource(1)"), "引用保留：\n{saved}");
    assert!(!saved.contains("child_root"), "展开内容不进文件：\n{saved}");

    let mut rt2 = NesRuntime::open_with_root(&root, 64, 64).expect("运行时 2");
    let report2 = rt2.load_scene("Scenes/parent_saved.ron").expect("重载回存文件");
    assert!(report2.is_clean());
    let _ = rt2.bind_assets();
    assert_eq!(rt2.upload_pending_textures().expect("上传"), 1);
    let second = rt2.frame(&frame(0)).expect("重载首帧");
    assert_eq!(second.stats.drawn, 1);
    assert_eq!(second.image.pixel(10, 10), Some(V1[0]), "重载后像素一致");
    assert_eq!(second.image.pixel(4, 4), Some(CLEAR_RGBA));
}

/// T-Scene-06：diff 式回写端到端 —— 加载（无覆盖）-> 运行时把实例内精灵
/// 挪到 (24,0)（世界 (32,8)）-> `sync_overrides` 烘焙 -> `save_scene` 落盘
/// （文件含 Override 记录）-> 全新运行时加载 -> 像素复现编辑后状态。
#[test]
fn t_scene_06_diff_bake_roundtrip() {
    let root = make_root("s6");
    let child = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/demo.bmp", kind: "Texture"),
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
    let parent = r#"Scene(
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
    std::fs::write(root.join("Scenes").join("child.ron"), child).expect("写子场景");
    std::fs::write(root.join("Scenes").join("parent.ron"), parent).expect("写父场景");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    rt.load_scene("Scenes/parent.ron").expect("加载");
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);
    let first = rt.frame(&frame(0)).expect("首帧");
    assert_eq!(first.image.pixel(10, 10), Some(V1[0]), "初始：精灵世界 (8,8)");

    // 运行时编辑实例内部节点：sprite -> (24,0)（世界 (32,8)）。
    let sprite = rt.tree_mut().find_by_name("sprite").expect("精灵");
    rt.tree_mut()
        .set_local(sprite, nes_scene::Transform2D::from_pos(24.0, 0.0));
    let edited = rt.frame(&frame(1)).expect("编辑后帧");
    assert_eq!(edited.image.pixel(34, 10), Some(V1[0]), "编辑生效：世界 (32,8)");

    // 不烘焙就保存 = 显式丢弃（文件无覆盖）。
    rt.save_scene("Scenes/unsynced.ron").expect("存（未烘焙）");
    let unsynced = std::fs::read_to_string(root.join("Scenes").join("unsynced.ron")).unwrap();
    assert!(!unsynced.contains("overrides"), "未烘焙的保存不含覆盖：\n{unsynced}");

    // 烘焙 -> 保存：文件带覆盖记录（用当前父槽位编号的纹理不产生假记录）。
    assert_eq!(rt.sync_overrides().expect("烘焙"), 1, "一个包装节点");
    rt.save_scene("Scenes/baked.ron").expect("存（已烘焙）");
    let baked = std::fs::read_to_string(root.join("Scenes").join("baked.ron")).unwrap();
    assert!(baked.contains("overrides: ["), "覆盖块写出：\n{baked}");
    assert!(baked.contains("Override(path: \"sprite\""), "sprite 记录：\n{baked}");
    assert!(baked.contains("x: 24.0"), "编辑值：\n{baked}");
    assert!(!baked.contains("texture"), "纹理指向同一文件：不产生假覆盖：\n{baked}");

    // 全新运行时加载烘焙文件：像素复现编辑后状态。
    let mut rt2 = NesRuntime::open_with_root(&root, 64, 64).expect("运行时 2");
    rt2.load_scene("Scenes/baked.ron").expect("加载烘焙文件");
    let _ = rt2.bind_assets();
    assert_eq!(rt2.upload_pending_textures().expect("上传"), 1);
    let after = rt2.frame(&frame(0)).expect("复现帧");
    assert_eq!(after.stats.drawn, 1);
    assert_eq!(after.image.pixel(34, 10), Some(V1[0]), "烘焙后：精灵仍在 (32,8)");
    assert_eq!(after.image.pixel(10, 10), Some(CLEAR_RGBA), "初始位已空");
}

/// T-Scene-07：结构性覆盖端到端 —— 磁盘父场景带 add/remove 记录 -> 加载
/// 渲染（移除的精灵消失、追加的精灵入画）；运行时结构编辑（实例内新增带
/// 纹理的精灵）-> `sync_overrides` 烘焙 -> 存盘 -> 全新运行时加载复现。
#[test]
fn t_scene_07_structural_override_end_to_end() {
    let root = make_root("s7");
    let child = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/demo.bmp", kind: "Texture"),
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
    // 父场景：移除实例里的 sprite；在子场景根下追加 kept（用父纹理槽位 2）。
    let parent = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Scenes/child.ron", kind: "Scene"),
        Res(id: 2, path: "Textures/demo.bmp", kind: "Texture"),
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
                overrides: [
                    Override(path: "sprite", remove: true),
                    Override(path: "", add: [
                        Node(
                            name: "kept",
                            kind: "Sprite2D",
                            local: (x: 16.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                            props: { "texture": Resource(2), },
                            children: [],
                        ),
                    ]),
                ],
                children: [],
            ),
        ],
    ),
)
"#;
    std::fs::write(root.join("Scenes").join("child.ron"), child).expect("写子场景");
    std::fs::write(root.join("Scenes").join("parent.ron"), parent).expect("写父场景");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let report = rt.load_scene("Scenes/parent.ron").expect("加载");
    assert!(report.is_clean());
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1, "去重后同一纹理一张");

    // 首帧：sprite 被移除（无 (8,8) 精灵）；kept 在包装 (8,8) ∘ (16,0) = (24,8)。
    let first = rt.frame(&frame(0)).expect("首帧");
    assert_eq!(first.stats.drawn, 1, "移除一个、追加一个 -> 仍是一个精灵");
    assert_eq!(first.image.pixel(10, 10), Some(CLEAR_RGBA), "sprite 已被移除");
    assert_eq!(first.image.pixel(26, 10), Some(V1[0]), "kept 在 (24,8)");

    // 运行时结构编辑：实例根下再加 late（(0,16) -> 世界 (8,24)）。
    let child_root = rt.tree_mut().find_by_name("child_root").expect("子场景根");
    let late = rt.tree_mut().add_node(child_root, "late", nes_scene::NodeKind::Sprite2D);
// 绑纹理：用资源表里的槽位 2（父编号，diff 生成的记录也用它）。
    rt.tree_mut()
        .set_prop(late, "texture", nes_scene::Value::Resource(2))
        .expect("绑纹理");
    rt.tree_mut()
        .set_local(late, nes_scene::Transform2D::from_pos(0.0, 16.0));
    rt.tree_mut().apply_pending();
    let edited = rt.frame(&frame(1)).expect("编辑后帧");
    assert_eq!(edited.stats.drawn, 2, "late 入画");
    assert_eq!(edited.image.pixel(10, 26), Some(V1[0]), "late 在 (8,24)");

    // 烘焙 + 存盘 -> 全新运行时加载复现（两个精灵、移除保持）。
    assert_eq!(rt.sync_overrides().expect("烘焙"), 1);
    rt.save_scene("Scenes/baked_struct.ron").expect("存");
    let baked = std::fs::read_to_string(root.join("Scenes").join("baked_struct.ron")).unwrap();
    assert!(baked.contains("add: [") && baked.contains("name: \"late\""), "late 进 add 记录：\n{baked}");

    let mut rt2 = NesRuntime::open_with_root(&root, 64, 64).expect("运行时 2");
    rt2.load_scene("Scenes/baked_struct.ron").expect("加载");
    let _ = rt2.bind_assets();
    assert_eq!(rt2.upload_pending_textures().expect("上传"), 1);
    let after = rt2.frame(&frame(0)).expect("复现帧");
    assert_eq!(after.stats.drawn, 2, "kept + late 复现");
    assert_eq!(after.image.pixel(26, 10), Some(V1[0]), "kept 仍在 (24,8)");
    assert_eq!(after.image.pixel(10, 26), Some(V1[0]), "late 复现在 (8,24)");
    assert_eq!(after.image.pixel(10, 10), Some(CLEAR_RGBA), "sprite 仍被移除");
}

/// T-Scene-08：rename 端到端 —— 磁盘 rename 记录加载（改名 + 纹理从子场景
/// 流入改名节点）；运行时改名 -> 烘焙存盘（文件含 rename 记录而非
/// remove+add）-> 子场景更新触发热重载后，**改名节点跟随子场景新位置**
///（保留跟踪的核心收益）。
#[test]
fn t_scene_08_rename_keeps_tracking_end_to_end() {
    let root = make_root("s8");
    let child = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/demo.bmp", kind: "Texture"),
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
    let parent = r#"Scene(
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
                overrides: [
                    Override(path: "sprite", rename: "hero"),
                ],
                children: [],
            ),
        ],
    ),
)
"#;
    std::fs::write(root.join("Scenes").join("child.ron"), child).expect("写子场景");
    std::fs::write(root.join("Scenes").join("parent.ron"), parent).expect("写父场景");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    rt.load_scene("Scenes/parent.ron").expect("加载");
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);
    let first = rt.frame(&frame(0)).expect("首帧");
    assert_eq!(first.image.pixel(10, 10), Some(V1[0]), "hero（原 sprite）在 (8,8)，纹理从子场景流入");

    // 子场景更新：sprite 挪到 (16,0)（世界 (24,8)）。
    let child_v2 = child.replace(
        "name: \"sprite\",\n                kind: \"Sprite2D\",\n                props:",
        "name: \"sprite\",\n                kind: \"Sprite2D\",\n                local: (x: 16.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),\n                props:",
    );
    std::fs::write(root.join("Scenes").join("child.ron"), child_v2).expect("改子场景");
    assert!(rt.poll_scene_reload().expect("热重载").is_some(), "触发整树重载");
    let _ = rt.upload_pending_textures();
    let second = rt.frame(&frame(1)).expect("重载帧");
    assert_eq!(second.stats.drawn, 1);
    assert_eq!(
        second.image.pixel(26, 10), Some(V1[0]),
        "改名节点跟随子场景新位置 (24,8) —— 保留跟踪"
    );
    assert_eq!(second.image.pixel(10, 10), Some(CLEAR_RGBA), "旧位置已空");

    // 运行时再改名 hero->player -> 烘焙存盘：文件记录是 rename（不是 remove+add）。
    let hero = rt.tree_mut().find_by_name("hero").expect("hero");
    rt.tree_mut().queue(nes_scene::TreeOp::Rename { node: hero, name: "player".to_string() });
    rt.tree_mut().apply_pending();
    assert_eq!(rt.sync_overrides().expect("烘焙"), 1);
    rt.save_scene("Scenes/baked_rename.ron").expect("存");
    let baked = std::fs::read_to_string(root.join("Scenes").join("baked_rename.ron")).unwrap();
    assert!(baked.contains("rename: \"player\""), "rename 记录写出：\n{baked}");
    assert!(!baked.contains("remove: true"), "不是 remove+add：\n{baked}");
    assert!(!baked.contains("add: ["), "无冻结副本：\n{baked}");

    // 全新运行时加载烘焙文件：改名链保持（sprite -> hero -> player），
    // 且子场景位置更新仍在（(24,8)，因为烘焙前已重载）。
    let mut rt2 = NesRuntime::open_with_root(&root, 64, 64).expect("运行时 2");
    rt2.load_scene("Scenes/baked_rename.ron").expect("加载");
    let _ = rt2.bind_assets();
    assert_eq!(rt2.upload_pending_textures().expect("上传"), 1);
    let after = rt2.frame(&frame(0)).expect("复现帧");
    assert_eq!(after.stats.drawn, 1);
    assert_eq!(after.image.pixel(26, 10), Some(V1[0]), "player 在 (24,8)");
    assert!(rt2.tree_mut().find_by_name("player").is_some(), "最终名可查");
}
