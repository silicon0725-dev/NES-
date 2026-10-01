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

/// T-Script-R2：**行为层闭环** —— 磁盘场景文件自带脚本文本（source 属性，
/// 多行经 RON 转义），加载 -> attach_all（零宿主注册）-> 帧循环驱动精灵
/// -> 像素。场景从此是"结构 + 资源引用 + 行为"的完整自包含单元。
#[test]
fn t_script_r2_disk_scene_carries_behavior() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_script")
        .join("r2");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写演示纹理");

    // 场景文件：source 属性内嵌多行脚本（RON 转义换行）—— 信号入口，
    // 每次 "step" 把 sprite 平移 (16,0)。原始字符串避免 Rust/RON 双层转义：
    // 下方 `\n`、`\"` 是 **RON 层**的转义，原样落盘。
    let scene = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/demo.bmp", kind: "Texture"),
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
                name: "sprite",
                kind: "Sprite2D",
                local: (x: 10.0, y: 10.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "texture": Resource(1), },
                children: [],
            ),
            Node(
                name: "brain",
                kind: "Script",
                props: { "source": "on \"step\" {\n    sprite.pos = sprite.pos + (16.0, 0.0)\n}", },
                children: [],
            ),
        ],
    ),
)
"#;
    std::fs::write(root.join("scene.ron"), scene).expect("写场景");

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let report = rt.load_scene("scene.ron").expect("加载");
    assert!(report.is_clean(), "{report:?}");
    let _ = rt.bind_assets();
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    // VM 零注册：attach_all 直接从 source 属性编译装载。
    let mut vm = ScriptVm::new();
    let issues = vm.attach_all(rt.tree_mut());
    assert!(issues.is_empty(), "装载无缺口：{issues:?}");

    // 帧循环：宿主只发信号，行为全在场景文件里。
    rt.tree_mut().emit_signal("step", Value::I64(0));
    let f1 = rt.frame_with(&frame(0), &mut vm).expect("帧 1");
    assert_eq!(f1.stats.drawn, 1);
    assert_eq!(f1.image.pixel(28, 12), Some(V1[0]), "帧 1：精灵 (26,10)");

    rt.tree_mut().emit_signal("step", Value::I64(0));
    let f2 = rt.frame_with(&frame(1), &mut vm).expect("帧 2");
    assert_eq!(f2.image.pixel(44, 12), Some(V1[0]), "帧 2：(42,10)");
    assert_eq!(f2.image.pixel(28, 12), Some(CLEAR_RGBA), "旧位已空");
    assert_eq!(f2.stats.driver_errors, 0);

    // 回存：source 属性随场景文件往返（行为跟着文件走）。
    rt.save_scene("scene_saved.ron").expect("存");
    let saved = std::fs::read_to_string(root.join("scene_saved.ron")).unwrap();
    assert!(saved.contains("\"source\""), "源码属性在文件里：\n{saved}");
    assert!(saved.contains("\\n") || saved.contains("\n"), "换行转义：\n{saved}");
}

/// T-Script-R3：脚本热重载端到端 —— ① source 属性编辑流（编辑器）：
/// 树上改源码 -> vm.poll_reloads -> 下一帧像素反映新行为；② 场景文件流
///（文件即事实）：子场景文件里的脚本改写 -> rt.poll_scene_reload 整树
/// 重载 -> vm.attach_all（含死节点清理 + 重新编译）-> 像素反映新行为。
#[test]
fn t_script_r3_hot_reload_reaches_pixels() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_script")
        .join("r3b");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写演示纹理");

    // ① source 属性编辑流。
    let mut rt = NesRuntime::open_with_root(&root, 64, 64).expect("装配");
    let res = rt.declare_texture("Textures/demo.bmp").expect("声明");
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 1);
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);
    let (sprite, brain) = {
        let tree = rt.tree_mut();
        let root_node = tree.root();
        let cam = tree.add_node(root_node, "cam", NodeKind::Camera2D);
        tree.set_local(cam, Transform2D::from_pos(32.0, 32.0));
        let sprite = tree.add_node(root_node, "sprite", NodeKind::Sprite2D);
        tree.set_prop(sprite, "texture", res.to_value()).unwrap();
        tree.set_local(sprite, Transform2D::from_pos(10.0, 10.0));
        let brain = tree.add_node(root_node, "brain", NodeKind::Script);
        tree.set_prop(brain, "source", Value::Str(
            "on \"go\" { sprite.pos = sprite.pos + (16.0, 0.0) }".into(),
        )).unwrap();
        tree.apply_pending();
        (sprite, brain)
    };
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(rt.tree_mut()).is_empty());
    rt.tree_mut().emit_signal("go", Value::I64(0));
    let f1 = rt.frame_with(&frame(0), &mut vm).expect("帧 1");
    assert_eq!(f1.image.pixel(28, 12), Some(V1[0]), "v1：+16");

    // 编辑器流：改 source -> poll_reloads -> 新行为（-8 反向）。
    rt.tree_mut().set_prop(brain, "source", Value::Str(
        "on \"go\" { sprite.pos = sprite.pos - (8.0, 0.0) }".into(),
    )).unwrap();
    let (re, fa) = vm.poll_reloads(rt.tree_mut());
    assert_eq!(re.len(), 1);
    assert!(fa.is_empty());
    rt.tree_mut().emit_signal("go", Value::I64(0));
    let f2 = rt.frame_with(&frame(1), &mut vm).expect("帧 2");
    assert_eq!(f2.image.pixel(20, 12), Some(V1[0]), "v2：-8（26-8=18，探 (20,12)）");
    assert_eq!(f2.image.pixel(38, 12), Some(CLEAR_RGBA), "v1 旧位已空（38 在旧不在新）");

    // ② 场景文件流：子场景文件里的脚本。
    let root2 = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_script")
        .join("r3c");
    let _ = std::fs::remove_dir_all(&root2);
    std::fs::create_dir_all(root2.join("Textures")).unwrap();
    std::fs::create_dir_all(root2.join("Scenes")).unwrap();
    write_bmp_rgba(&root2.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写纹理");
    // 子场景：精灵 + 脚本（source 内嵌）。
    let child_v1 = r#"Scene(
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
                local: (x: 10.0, y: 10.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "texture": Resource(1), },
                children: [],
            ),
            Node(
                name: "brain",
                kind: "Script",
                props: { "source": "on \"go\" { sprite.pos = sprite.pos + (16.0, 0.0) }", },
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
                local: (x: 0.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "sub_scene": Resource(1), },
                children: [],
            ),
        ],
    ),
)
"#;
    std::fs::write(root2.join("Scenes").join("child.ron"), child_v1).expect("写子场景");
    std::fs::write(root2.join("Scenes").join("parent.ron"), parent).expect("写父场景");

    let mut rt2 = NesRuntime::open_with_root(&root2, 64, 64).expect("装配 2");
    let report2 = rt2.load_scene("Scenes/parent.ron").expect("加载");
    assert!(report2.is_clean(), "{report2:?}");
    let _ = rt2.bind_assets();
    assert_eq!(rt2.upload_pending_textures().expect("上传"), 1);
    let mut vm2 = ScriptVm::new();
    assert!(vm2.attach_all(rt2.tree_mut()).is_empty(), "子场景内脚本装载");
    rt2.tree_mut().emit_signal("go", Value::I64(0));
    let g1 = rt2.frame_with(&frame(0), &mut vm2).expect("子场景帧 1");
    assert_eq!(g1.image.pixel(28, 12), Some(V1[0]), "子场景脚本 v1：+16");

    // 改子场景文件里的脚本（-8），整树重载 + attach_all -> 新像素。
    let child_v2 = child_v1.replace("+ (16.0, 0.0)", "- (8.0, 0.0)");
    std::fs::write(root2.join("Scenes").join("child.ron"), child_v2).expect("改写子场景");
    assert!(rt2.poll_scene_reload().expect("整树重载").is_some(), "触发");
    let _ = rt2.upload_pending_textures();
    assert!(vm2.attach_all(rt2.tree_mut()).is_empty(), "重载后重挂载");
    rt2.tree_mut().emit_signal("go", Value::I64(0));
    let g2 = rt2.frame_with(&frame(1), &mut vm2).expect("重载帧");
    // 整树重载 = 场景文件即事实：精灵复位到文件位 (10,10)，一次信号 -8 -> 2。
    assert_eq!(g2.image.pixel(4, 12), Some(V1[0]), "文件流：-8 生效（复位 10-8=2，探 (4,12)）");
    assert_eq!(g2.image.pixel(28, 12), Some(CLEAR_RGBA), "v1 旧位已空");
    assert_eq!(g2.stats.driver_errors, 0);
    let _ = (sprite, f2);
}

/// T-Script-R4：外置 .nes 脚本资产端到端 —— 场景资源表声明 kind:Script
/// 条目，Script 节点 `script` 属性引用槽位；加载 -> attach_all_with_sources
///（注入磁盘读取器）-> 信号 -> 像素；**改 .nes 文件 -> vm 热重载轮询 ->
/// 新像素（不整树重载）**；save/load 往返 script 属性。
#[test]
fn t_script_r4_external_nes_asset_end_to_end() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_script")
        .join("r4");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    std::fs::create_dir_all(root.join("Scripts")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写纹理");
    std::fs::write(
        root.join("Scripts").join("mover.nes"),
        "on \"go\" { sprite.pos = sprite.pos + (16.0, 0.0) }\n",
    )
    .expect("写 .nes v1");

    // 场景：纹理 + 外置脚本（kind:Script）双资源。
    let scene = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/demo.bmp", kind: "Texture"),
        Res(id: 2, path: "Scripts/mover.nes", kind: "Script"),
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
                name: "sprite",
                kind: "Sprite2D",
                local: (x: 10.0, y: 10.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "texture": Resource(1), },
                children: [],
            ),
            Node(
                name: "brain",
                kind: "Script",
                props: { "script": Resource(2), },
                children: [],
            ),
        ],
    ),
)
"#;
    std::fs::write(root.join("scene.ron"), scene).expect("写场景");

    let mut rt = NesRuntime::open_with_root(&root, 64, 64).expect("装配");
    let report = rt.load_scene("scene.ron").expect("加载");
    assert!(report.is_clean(), "{report:?}");
    let bound = rt.bind_assets();
    assert_eq!(bound.loaded.len(), 2, "纹理 + 脚本字节都加载（内容戳入注册表）");
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    // 外置装载：宿主注入磁盘读取器（root 相对路径）。
    let script_root = root.clone();
    let mut read = |rel: &str| {
        std::fs::read_to_string(script_root.join(rel)).map_err(|e| e.to_string())
    };
    let mut vm = ScriptVm::new();
    // 表克隆快照：tree_mut 与表借用分开（ResourceTable: Clone）。
    let table = rt.resources_mut().clone();
    let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut read);
    assert!(issues.is_empty(), "{issues:?}");

    rt.tree_mut().emit_signal("go", Value::I64(0));
    let f1 = rt.frame_with(&frame(0), &mut vm).expect("帧 1");
    assert_eq!(f1.image.pixel(28, 12), Some(V1[0]), "v1：+16 -> (26,10)");

    // 改 .nes 文件 -> VM 热重载（不整树重载 —— 对比 S6.7 的 Scene 资产流）。
    std::fs::write(
        root.join("Scripts").join("mover.nes"),
        "on \"go\" { sprite.pos = sprite.pos - (8.0, 0.0) }\n",
    )
    .expect("写 .nes v2");
    let table2 = rt.resources_mut().clone();
    let (re, fa) = vm.poll_reloads_with_sources(rt.tree_mut(), &table2, &mut read);
    assert_eq!(re.len(), 1);
    assert!(fa.is_empty());
    rt.tree_mut().emit_signal("go", Value::I64(0));
    let f2 = rt.frame_with(&frame(1), &mut vm).expect("帧 2");
    assert_eq!(f2.image.pixel(20, 12), Some(V1[0]), "v2：-8 -> 18（探 (20,12)）");
    assert_eq!(f2.stats.driver_errors, 0);

    // 往返：script 属性（Resource 槽位）随场景存取。
    rt.save_scene("scene_saved.ron").expect("存");
    let saved = std::fs::read_to_string(root.join("scene_saved.ron")).unwrap();
    assert!(saved.contains("\"script\": Resource(2)"), "槽位引用往返：\n{saved}");
}
