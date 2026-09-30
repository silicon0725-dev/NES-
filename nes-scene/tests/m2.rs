//! M2 出口准则验收：属性反射 + 场景序列化 / 实例化 / 打包。
//!
//! 四条准则：
//! 1. 任意节点的属性可被枚举、读、写、校验（含全部错误路径）；
//! 2. 同一棵树序列化两次逐字节相同，且不受构造顺序影响；
//! 3. 场景回读实例化后，在结构、属性、世界变换上与原始树等价；
//! 4. 未来版本被明确拒绝，引擎不认识的扩展属性原样保留。

use nes_scene::scene_io::{self, PackOptions, PackedScene, FORMAT_VERSION};
use nes_scene::{instantiate, NodeKind, NodeKindTag, PropError, SceneTree, Transform2D, Value, Vec2};

// ---------- 样例场景 ----------
//
// root
// ├─ Hero (Sprite2D)    local=(3,4) rot=0.5 scale=2 · texture=7 flip_h=true
// │   └─ Gun (Sprite2D)  local=(10,0) · z_index=-1
// └─ UI (Control)       size=(320,120)
//     └─ Title (Label)  local=(0,24) · text="NES 2.0" font_size=24

fn sample_tree() -> SceneTree {
    build_sample(false)
}

/// `reversed_props`：属性按相反顺序写入。序列化输出顺序必须与写入顺序无关。
fn build_sample(reversed_props: bool) -> SceneTree {
    let mut tree = SceneTree::new("root");
    let hero = tree.add_node(tree.root(), "Hero", NodeKind::Sprite2D);
    let gun = tree.add_node(hero, "Gun", NodeKind::Sprite2D);
    let ui = tree.add_node(tree.root(), "UI", NodeKind::Control);
    let title = tree.add_node(ui, "Title", NodeKind::Label);
    tree.apply_pending();

    tree.set_local(
        hero,
        Transform2D {
            pos: Vec2::new(3.0, 4.0),
            rot: 0.5,
            scale: Vec2::new(2.0, 2.0),
            skew: 0.0,
        },
    );
    tree.set_local(gun, Transform2D::from_pos(10.0, 0.0));
    tree.set_local(title, Transform2D::from_pos(0.0, 24.0));

    if reversed_props {
        tree.set_prop(title, "font_size", Value::I64(24)).unwrap();
        tree.set_prop(title, "text", Value::str("NES 2.0")).unwrap();
        tree.set_prop(ui, "size", Value::vec2(320.0, 120.0)).unwrap();
        tree.set_prop(gun, "z_index", Value::I64(-1)).unwrap();
        tree.set_prop(hero, "flip_h", Value::Bool(true)).unwrap();
        tree.set_prop(hero, "texture", Value::Resource(7)).unwrap();
    } else {
        tree.set_prop(hero, "texture", Value::Resource(7)).unwrap();
        tree.set_prop(hero, "flip_h", Value::Bool(true)).unwrap();
        tree.set_prop(gun, "z_index", Value::I64(-1)).unwrap();
        tree.set_prop(ui, "size", Value::vec2(320.0, 120.0)).unwrap();
        tree.set_prop(title, "text", Value::str("NES 2.0")).unwrap();
        tree.set_prop(title, "font_size", Value::I64(24)).unwrap();
    }

    tree.refresh_transforms();
    tree
}

/// 两棵树必须逐节点等价：结构、寻址、本地变换、世界矩阵、属性表。
fn assert_tree_equiv(a: &SceneTree, b: &SceneTree, why: &str) {
    assert_eq!(a.len(), b.len(), "{why}：节点总数");
    let (pa, pb) = (a.preorder(), b.preorder());
    for (x, y) in pa.iter().zip(pb.iter()) {
        let name = a.name(*x).unwrap();
        assert_eq!(name, b.name(*y).unwrap(), "{why}：{name} 的名字");
        assert_eq!(a.kind_tag(*x), b.kind_tag(*y), "{why}：{name} 的类型");
        assert_eq!(a.path_of(*x), b.path_of(*y), "{why}：{name} 的路径");
        assert_eq!(a.local(*x), b.local(*y), "{why}：{name} 的本地变换");
        assert_eq!(a.world(*x), b.world(*y), "{why}：{name} 的世界矩阵");
        assert_eq!(a.props(*x), b.props(*y), "{why}：{name} 的属性表");
    }
}

// ---------- 准则 1：属性反射 ----------

#[test]
fn criterion_1_properties_are_reflectable() {
    let mut tree = SceneTree::new("root");
    let hero = tree.add_node(tree.root(), "Hero", NodeKind::Sprite2D);
    tree.apply_pending();

    // 枚举：schema 聚合整条继承链，基类在前，并带类型与默认值。
    let schema = tree.schema_of(hero).expect("有 schema");
    assert_eq!(
        schema.chain(),
        &[NodeKindTag::Node, NodeKindTag::Node2D, NodeKindTag::Sprite2D]
    );
    let names: Vec<&str> = schema.props().iter().map(|p| p.name()).collect();
    assert_eq!(
        names,
        vec!["visible", "z_index", "texture", "flip_h", "flip_v"]
    );

    // 读：建节点时默认值已按 schema 填好，脚本读到的不会是"未定义"。
    assert_eq!(tree.prop(hero, "visible"), Some(&Value::Bool(true)));
    assert_eq!(tree.prop(hero, "texture"), Some(&Value::Resource(0)));

    // 写：合法值立即生效。
    tree.set_prop(hero, "texture", Value::Resource(42)).unwrap();
    tree.set_prop(hero, "flip_h", Value::Bool(true)).unwrap();
    assert_eq!(tree.prop(hero, "texture"), Some(&Value::Resource(42)));
    assert_eq!(tree.prop(hero, "flip_h"), Some(&Value::Bool(true)));

    // 数值按 hint 夹取（zoom 的 schema 上限是 16）。
    let cam = tree.add_node(tree.root(), "Cam", NodeKind::Camera2D);
    tree.apply_pending();
    tree.set_prop(cam, "zoom", Value::F32(999.0)).unwrap();
    assert_eq!(tree.prop(cam, "zoom"), Some(&Value::F32(16.0)));

    // 校验：拼错属性名、类型不符都必须被挡住，且不动原值。
    assert_eq!(
        tree.set_prop(hero, "txeture", Value::Resource(1)),
        Err(PropError::UnknownProp("txeture".into()))
    );
    assert!(matches!(
        tree.set_prop(hero, "texture", Value::str("nope")),
        Err(PropError::TypeMismatch { .. })
    ));
    assert_eq!(tree.prop(hero, "texture"), Some(&Value::Resource(42)));

    // 过期 NodeId 写入报 NoSuchNode，而不是 panic。
    let ghost = tree.add_node(tree.root(), "Ghost", NodeKind::Node);
    tree.apply_pending();
    tree.remove_node(ghost, false);
    tree.apply_pending();
    assert_eq!(
        tree.set_prop(ghost, "visible", Value::Bool(false)),
        Err(PropError::NoSuchNode)
    );

    // 属性表版本号：真实变更才递增，是编辑器刷新面板的依据。
    let v0 = tree.props(hero).unwrap().version();
    tree.set_prop(hero, "flip_h", Value::Bool(true)).unwrap();
    assert_eq!(
        tree.props(hero).unwrap().version(),
        v0,
        "同值写入不该抬版本"
    );
    tree.set_prop(hero, "flip_h", Value::Bool(false)).unwrap();
    assert_eq!(tree.props(hero).unwrap().version(), v0 + 1);
}

// ---------- 准则 2：确定性打包 ----------

#[test]
fn criterion_2_packing_is_byte_deterministic() {
    let tree = sample_tree();
    let a = scene_io::write_ron(&tree, &PackOptions::verbose());
    let b = scene_io::write_ron(&tree, &PackOptions::verbose());
    assert_eq!(a, b, "同一棵树两次打包必须逐字节相同");

    // 属性写入顺序不同、内容相同的两棵树，打包结果必须相同：
    // 输出顺序由 schema 与场景顺序决定，与构造过程无关。
    let twin = build_sample(true);
    assert_eq!(scene_io::write_ron(&twin, &PackOptions::verbose()), a);

    // 头部元信息：版本与节点数。
    assert!(a.starts_with(&format!("// nes-scene packed v{FORMAT_VERSION}")));
    assert!(a.contains(&format!("version: {FORMAT_VERSION},")));

    // compact 省略默认值：更短，且语义完全等价（回读后两棵树等价）。
    let compact = scene_io::write_ron(&tree, &PackOptions::compact());
    assert!(compact.len() < a.len(), "省略默认值应让文件更短");
    let from_verbose = instantiate(&a).unwrap();
    let from_compact = instantiate(&compact).unwrap();
    assert_tree_equiv(&from_verbose, &from_compact, "verbose 与 compact");
}

// ---------- 准则 3：实例化等价 ----------

#[test]
fn criterion_3_instantiate_roundtrip_is_equivalent() {
    let tree = sample_tree();
    let ron = scene_io::write_ron(&tree, &PackOptions::verbose());
    // 变换确实写进了文本（漏掉它的话，所有节点都会叠在原点）。
    assert!(
        ron.contains("local: (x: 3.0, y: 4.0, rot: 0.5, sx: 2.0, sy: 2.0, skew: 0.0),"),
        "第 3 行应为 Hero 的变换：{}",
        ron.lines().nth(6).unwrap_or("")
    );

    let packed = PackedScene::pack(&tree, &PackOptions::verbose());
    assert_eq!(packed.node_count(), tree.len());
    assert_eq!(packed.format_version(), FORMAT_VERSION);

    let back = packed.instantiate().expect("回读成功");
    assert_tree_equiv(&tree, &back, "原树与实例化结果");

    // 变换真的还原了：Gun 的世界位置含父级旋转 + 缩放 + 自身位移。
    let gun = back.find_str("root/Hero/Gun").expect("按路径找到");
    let world = back.world_position(gun).unwrap();
    let expect = Vec2::new(3.0 + 20.0 * 0.5f32.cos(), 4.0 + 20.0 * 0.5f32.sin());
    assert!(
        (world.x - expect.x).abs() < 1e-4 && (world.y - expect.y).abs() < 1e-4,
        "Gun 世界位置 {world:?} 应等于 {expect:?}"
    );

    // 存盘 / 装载往返。
    let file = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m2_roundtrip.ron");
    packed.save(&file).expect("存盘");
    let loaded = PackedScene::load(&file).expect("装载");
    assert_eq!(loaded.ron(), packed.ron(), "存盘不改变内容");
    assert_eq!(loaded.node_count(), packed.node_count());
    assert_tree_equiv(&tree, &loaded.instantiate().unwrap(), "存盘往返");
    let _ = std::fs::remove_file(&file);
}

// ---------- 准则 4：版本闸门与扩展属性 ----------

#[test]
fn criterion_4_version_guard_and_forward_compat() {
    let tree = sample_tree();
    let ron = scene_io::write_ron(&tree, &PackOptions::verbose());

    // 未来版本：明确拒绝。静默按老版本硬读会悄悄丢字段，比报错危险得多。
    let future = ron.replacen(
        &format!("version: {FORMAT_VERSION},"),
        &format!("version: {},", FORMAT_VERSION + 1),
        1,
    );
    assert_ne!(future, ron);
    let err = match instantiate(&future) {
        Ok(_) => panic!("未来版本必须被拒绝，不能按当前版本硬读"),
        Err(e) => e,
    };
    assert!(err.is_semantic(), "版本错误不该带语法位置：{err}");
    assert!(err.to_string().contains("高于当前引擎支持"), "{err}");
    assert!(PackedScene::from_ron(future).is_err());

    // 扩展属性：引擎不认识的键必须原样留下 —— "我不认识"不等于"可以丢"。
    let extended = ron.replacen(
        "\"flip_h\": Bool(true),",
        "\"scratch_broadcast\": Str(\"whenFlagClicked\"),\n                \"flip_h\": Bool(true),",
        1,
    );
    assert_ne!(extended, ron);
    let back = instantiate(&extended).expect("扩展属性不阻塞加载");
    let hero = back.find_str("root/Hero").unwrap();
    assert_eq!(
        back.prop(hero, "scratch_broadcast"),
        Some(&Value::str("whenFlagClicked"))
    );

    // 再打包一轮：扩展属性仍在，且排在 schema 属性之后，顺序确定。
    let repacked = scene_io::write_ron(&back, &PackOptions::verbose());
    assert!(repacked.contains("scratch_broadcast"));
    let doc = scene_io::parse_ron(&repacked).unwrap();
    let hero_doc = &doc.root.children[0];
    assert_eq!(hero_doc.name, "Hero");
    assert_eq!(
        hero_doc.props.last().unwrap().0.as_str(),
        "scratch_broadcast"
    );
}
