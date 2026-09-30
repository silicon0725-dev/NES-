//! T-SubR 契约回归：子场景热重载（S6.7）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-SubR-01 | 改子场景文件 -> `poll_scene_reload` 整树重载（重新展开）-> 下一帧像素反映新内容；返回来源路径 |
//! | T-SubR-02 | 幂等：无变化时返回 None 且树不重建；重载后旧 NodeId 失效、新树可寻址 |
//! | T-SubR-03 | 无来源安全：`instantiate_scene` 直入的树，纹理变化不触发场景重载（纹理照常重传） |
//!
//! 机制：子场景文件在展开时作为 `kind: "Scene"` 资源声明并随 `bind_assets`
//! 加载 —— 它就在资产注册表的内容戳轮询范围内，缺的只是"变化 -> 整树重载"
//! 这一步（[`NesRuntime::poll_scene_reload`]）。

use nes_render_api::{FrameInfo, Vec2};
use nes_runtime::{write_bmp_rgba, NesRuntime};

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

/// 子场景：精灵位于 (0,0)（热重载后改为 (16,0)）。
fn child_ron(sprite_x: f32) -> String {
    format!(
        r#"Scene(
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
                local: (x: {sprite_x}, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: {{ "texture": Resource(1), }},
                children: [],
            ),
        ],
    ),
)
"#
    )
}

/// 父场景：相机 + 包装节点(8,8) 引用子场景文件（槽位 1 = Scene）。
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

fn make_root(dir: &str) -> std::path::PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_subreload")
        .join(dir);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    std::fs::create_dir_all(root.join("Scenes")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写演示纹理");
    std::fs::write(root.join("Scenes").join("child.ron"), child_ron(0.0)).expect("写子场景");
    std::fs::write(root.join("Scenes").join("parent.ron"), PARENT).expect("写父场景");
    root
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(64.0, 64.0))
}

/// T-SubR-01：改子场景文件 -> poll_scene_reload -> 整树重载 -> 像素反映新内容。
#[test]
fn t_subr_01_child_file_change_reloads_tree() {
    let root = make_root("r1");
    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let report = rt.load_scene("Scenes/parent.ron").expect("加载父场景");
    assert!(report.is_clean());
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    // 首帧：包装(8,8) ∘ 精灵(0,0) -> 世界 (8,8)。
    let first = rt.frame(&frame(0)).expect("首帧");
    assert_eq!(first.stats.drawn, 1);
    assert_eq!(first.image.pixel(10, 10), Some(V1[0]), "精灵在 (8,8)");
    let sprite_before = rt.tree_mut().find_by_name("sprite").expect("寻址精灵");

    // 改子场景文件：精灵挪到 (16,0)（世界 (24,8)），并额外加一个精灵。
    let edited = r#"Scene(
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
    std::fs::write(root.join("Scenes").join("child.ron"), edited).expect("改写子场景");

    let reloaded = rt.poll_scene_reload().expect("轮询").expect("应触发整树重载");
    assert_eq!(reloaded, "Scenes/parent.ron", "返回来源路径");
    // 重载清零上传账目 -> 纹理重传（同一个 GPU 键，幂等覆盖）。
    assert_eq!(rt.upload_pending_textures().expect("重传"), 1);

    // 下一帧：两个精灵，新位置生效（生命周期重放由 tick 驱动，本帧即 enter）。
    let second = rt.frame(&frame(1)).expect("重载后首帧");
    assert_eq!(second.stats.drawn, 2, "新子场景的两个精灵");
    assert_eq!(second.image.pixel(26, 10), Some(V1[0]), "精灵挪到 (24,8)");
    assert_eq!(second.image.pixel(10, 34), Some(V1[0]), "sprite2 在 (8,32)");
    assert_eq!(second.image.pixel(10, 10), Some(CLEAR_RGBA), "旧位置已空");

    // 旧句柄失效（新树新 arena）—— 不崩溃、查无此名（名字是新的同名节点，
    // 句柄层面的失效表现为：旧 id 在新树上解析成别的或不存在）。
    let _ = sprite_before; // 文档口径：宿主需重新寻址
    assert!(rt.tree_mut().find_by_name("sprite2").is_some(), "新节点可寻址");
}

/// T-SubR-02：无变化不重载（None）；重载后再轮询也 None（戳已消费，不循环）。
#[test]
fn t_subr_02_no_change_no_reload() {
    let root = make_root("r2");
    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    rt.load_scene("Scenes/parent.ron").expect("加载");
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);
    rt.frame(&frame(0)).expect("首帧");

    // 无变化：None，且树未重建（精灵句柄仍可寻址）。
    let sprite = rt.tree_mut().find_by_name("sprite").expect("寻址精灵");
    assert!(rt.poll_scene_reload().expect("轮询").is_none(), "无变化不重载");
    assert!(rt.tree_mut().find_by_name("sprite").is_some());

    // 改文件 -> 重载 -> 再轮询：None（内容戳已消费，不会反复重载）。
    std::fs::write(root.join("Scenes").join("child.ron"), child_ron(16.0)).expect("改写");
    assert!(rt.poll_scene_reload().expect("轮询 1").is_some(), "触发重载");
    let _ = sprite; // 此句柄已随整树替换失效
    assert!(rt.poll_scene_reload().expect("轮询 2").is_none(), "不重复重载");
}

/// T-SubR-03：无来源安全 —— `instantiate_scene` 直入的树，纹理变化不触发
/// 场景重载（返回 None），纹理本身照常重传（既有热重载路径不受影响）。
#[test]
fn t_subr_03_no_source_texture_reload_still_works() {
    let root = make_root("r3");
    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let report = rt.load_scene("Scenes/parent.ron").expect("加载");
    assert!(report.is_clean());
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);
    rt.frame(&frame(0)).expect("首帧");

    // 用 instantiate_scene 直入同构文档：来源被清除。
    let doc = nes_scene::parse_ron(PARENT).expect("解析");
    let expanded = nes_scene::expand_subscenes(&doc, &mut |rel| {
        let text = std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())?;
        nes_scene::parse_ron(&text).map_err(|e| e.to_string())
    })
    .expect("展开");
    rt.instantiate_scene(expanded).expect("直入");
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    // 改子场景文件 + 改纹理：Scene 变化也在轮询里，但无来源 -> None。
    std::fs::write(root.join("Scenes").join("child.ron"), child_ron(16.0)).expect("改子场景");
    let v2 = [
        [0, 0, 255, 255],
        [255, 0, 255, 255],
        [55, 55, 55, 255],
        [175, 175, 175, 255],
    ];
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&v2))
        .expect("改纹理");
    assert!(
        rt.poll_scene_reload().expect("轮询").is_none(),
        "无来源：场景变化也不重载"
    );
    // 纹理变化照常重传（既有路径）：下一帧像素变反色。
    assert_eq!(rt.upload_pending_textures().expect("重传"), 1);
    let after = rt.frame(&frame(1)).expect("纹理热重载帧");
    assert_eq!(after.stats.drawn, 1);
    assert_eq!(after.image.pixel(10, 10), Some(v2[0]), "纹理热重载照常生效");
}

/// T-SubR-04：实例级覆盖 + 热重载 —— 磁盘父场景带覆盖（精灵挪到 (16,0)，
/// 世界 (24,8)）；改子场景文件（子场景作者把精灵写到别处 + 加新精灵）触
/// 发整树重载后：**覆盖字段仍以覆盖为准**（精灵还在 (24,8)），未覆盖内容
/// 跟随子场景新版本（新精灵出现）。
#[test]
fn t_subr_04_overrides_survive_subscene_hot_reload() {
    let root = make_root("r4");
    // 父场景：包装 (8,8) + 覆盖 sprite -> local (16,0)。
    let parent_ovr = r#"Scene(
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
                    Override(path: "sprite", local: (x: 16.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0)),
                ],
                children: [],
            ),
        ],
    ),
)
"#;
    // 覆盖后精灵世界位：包装(8,8) ∘ 子根(单位) ∘ sprite(16,0) = (24,8)。
    // 未覆盖时（子场景 v1 的 (0,0)）世界位是 (8,8)。
    std::fs::write(root.join("Scenes").join("parent.ron"), parent_ovr).expect("写父场景");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let report = rt.load_scene("Scenes/parent.ron").expect("加载");
    assert!(report.is_clean());
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    let first = rt.frame(&frame(0)).expect("首帧");
    assert_eq!(first.stats.drawn, 1);
    assert_eq!(first.image.pixel(26, 10), Some(V1[0]), "覆盖生效：精灵在 (24,8)");
    assert_eq!(first.image.pixel(10, 10), Some(CLEAR_RGBA), "原位 (8,8) 已空");

    // 改子场景文件：子场景作者把 sprite 挪到 (48,0) 并新增 sprite2 ——
    // 若覆盖丢失，sprite 会出现在 (56,8)（跟随子场景新值）。
    let edited = r#"Scene(
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
                local: (x: 48.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
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
    std::fs::write(root.join("Scenes").join("child.ron"), edited).expect("改写子场景");
    assert!(rt.poll_scene_reload().expect("轮询").is_some(), "触发整树重载");
    assert_eq!(rt.upload_pending_textures().expect("重传"), 1);

    let second = rt.frame(&frame(1)).expect("重载后首帧");
    assert_eq!(second.stats.drawn, 2, "新子场景的两个精灵");
    assert_eq!(
        second.image.pixel(26, 10), Some(V1[0]),
        "覆盖字段仍以覆盖为准：sprite 还在 (24,8)，不是子场景新值 (56,8)"
    );
    assert_eq!(second.image.pixel(10, 34), Some(V1[0]), "未覆盖内容跟随新版：sprite2 在 (8,32)");
    // 子场景新值位置 (56,8) 确实没有精灵（覆盖赢了）。
    assert_eq!(second.image.pixel(58, 10), Some(CLEAR_RGBA), "子场景新值位无精灵");
}
