//! T-Sub 契约回归：子场景嵌套（草案 §10，S6.6）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Sub-01 | 展开与槽位重编号：包装节点的子树来自被引文档；子文档资源追加重编号、嫁接子树引用按映射改写（父子槽位号不串） |
//! | T-Sub-02 | 递归展开：孙辈经子辈整体嫁接（两级重编号）；循环引用如实报错并指名链条 |
//! | T-Sub-03 | 写回边界：绑定的包装节点回写只留引用（children 空）；再展开幂等（结构等价） |
//! | T-Sub-04 | 未绑定（`Resource(0)`）不展开；悬垂槽位（未声明）报错指名 |

use std::collections::BTreeMap;

use nes_scene::{
    expand_subscenes, parse_ron, NodeKindTag, PackOptions, Value,
    PROP_SUB_SCENE,
};

/// 内存文档源：路径 -> RON 文本（`load` 闭包由此取子文档）。
#[derive(Default)]
struct Src {
    files: BTreeMap<&'static str, String>,
}

impl Src {
    fn load(&mut self, rel: &str) -> Result<nes_scene::SceneDoc, String> {
        let text = self
            .files
            .get(rel)
            .cloned()
            .ok_or_else(|| format!("文件不存在：{rel}"))?;
        parse_ron(&text).map_err(|e| e.to_string())
    }
}

fn parent_doc() -> String {
    // 槽位 1 = 子场景文件（Scene）；包装节点在 (8,8)。
    r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Scenes/child.ron", kind: "Scene"),
    ],
    root: Node(
        name: "main",
        kind: "Node",
        children: [
            Node(
                name: "instance",
                kind: "Node2D",
                local: (x: 8.0, y: 8.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                props: { "sub_scene": Resource(1), },
                children: [],
            ),
        ],
    ),
)"#
    .to_string()
}

fn child_doc() -> String {
    // 子文档自己的槽位 1 = 纹理（与父文档槽位 1 撞号 —— 重编号的靶子）。
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
                props: { "texture": Resource(1), },
                children: [],
            ),
        ],
    ),
)"#
    .to_string()
}

fn prop_of(doc: &nes_scene::SceneDoc, path: &[&str], name: &str) -> Option<Value> {
    let mut node = &doc.root;
    for seg in &path[1..] {
        node = node.children.iter().find(|c| c.name == *seg)?;
    }
    node.props.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

fn node_at<'a>(doc: &'a nes_scene::SceneDoc, path: &[&str]) -> &'a nes_scene::NodeDoc {
    let mut node = &doc.root;
    for seg in &path[1..] {
        node = node
            .children
            .iter()
            .find(|c| c.name == *seg)
            .unwrap_or_else(|| panic!("路径段 {seg} 不存在"));
    }
    node
}

/// T-Sub-01：展开 + 重编号。展开后：父表两条（槽 1 Scene + 追加的槽 2
/// Texture），嫁接子树的 `texture` 引用从子文档的 1 改写到 2 —— 不与父
/// 槽 1（场景文件）串号。
#[test]
fn t_sub_01_expansion_renumbers_slots() {
    let mut src = Src::default();
    src.files.insert("Scenes/child.ron", child_doc());
    let parent = parse_ron(&parent_doc()).expect("父文档");

    let expanded = expand_subscenes(&parent, &mut |rel| src.load(rel)).expect("展开");
    // 资源表：父 1（Scene）+ 子追加 2（Texture）
    assert_eq!(expanded.resources.len(), 2);
    assert_eq!(expanded.resources[0].kind, "Scene");
    assert_eq!(expanded.resources[1].id, 2, "子资源追加重编号");
    assert_eq!(expanded.resources[1].kind, "Texture");

    // 结构：main -> instance(包装) -> child_root -> sprite
    let wrapper = node_at(&expanded, &["main", "instance"]);
    assert_eq!(wrapper.children.len(), 1, "包装节点的子树来自被引文档");
    assert_eq!(wrapper.children[0].name, "child_root");
    assert_eq!(
        node_at(&expanded, &["main", "instance", "child_root", "sprite"]).kind,
        NodeKindTag::Sprite2D
    );
    // 引用改写：sprite 的 texture 从 Resource(1) 变 Resource(2)
    assert_eq!(
        prop_of(&expanded, &["main", "instance", "child_root", "sprite"], "texture"),
        Some(Value::Resource(2)),
        "嫁接子树引用按映射改写"
    );
    // 包装节点自身的 sub_scene 引用保持父编号 1
    assert_eq!(
        prop_of(&expanded, &["main", "instance"], PROP_SUB_SCENE),
        Some(Value::Resource(1))
    );
}

/// T-Sub-02：递归（孙辈两级重编号）+ 循环引用检测。
#[test]
fn t_sub_02_recursion_and_cycle_detection() {
    // child 引用 grandchild（孙辈），grandchild 引回 parent（环）分两段测。
    let grandchild = r#"Scene(
    version: 1,
    root: Node(
        name: "grand_root",
        kind: "Node2D",
        children: [
            Node(name: "deep", kind: "Sprite2D", props: {}, children: []),
        ],
    ),
)"#;
    let child_referring = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Scenes/grandchild.ron", kind: "Scene"),
        Res(id: 2, path: "Textures/demo.bmp", kind: "Texture"),
    ],
    root: Node(
        name: "child_root",
        kind: "Node2D",
        children: [
            Node(
                name: "nested_instance",
                kind: "Node2D",
                props: { "sub_scene": Resource(1), "z_index": I64(0) },
                children: [],
            ),
            Node(
                name: "sprite",
                kind: "Sprite2D",
                props: { "texture": Resource(2), },
                children: [],
            ),
        ],
    ),
)"#;
    let mut src = Src::default();
    src.files.insert("Scenes/child.ron", child_referring.to_string());
    src.files.insert("Scenes/grandchild.ron", grandchild.to_string());
    let parent = parse_ron(&parent_doc()).expect("父文档");

    let expanded = expand_subscenes(&parent, &mut |rel| src.load(rel)).expect("递归展开");
    // 三级：main -> instance -> child_root -> nested_instance -> grand_root -> deep
    let deep = node_at(
        &expanded,
        &["main", "instance", "child_root", "nested_instance", "grand_root", "deep"],
    );
    assert_eq!(deep.kind, NodeKindTag::Sprite2D, "孙辈经两级嫁接到位");
    // 两级重编号：父 1(Scene child)；子 1(Scene grand)->2、子 2(Texture)->3
    assert_eq!(expanded.resources.len(), 3);
    assert_eq!(expanded.resources[1].path, "Scenes/grandchild.ron");
    assert_eq!(expanded.resources[2].path, "Textures/demo.bmp");
    // 子树引用两级改写：nested_instance 的 sub_scene 1->2；sprite 的 texture 2->3
    assert_eq!(
        prop_of(&expanded, &["main", "instance", "child_root", "nested_instance"], PROP_SUB_SCENE),
        Some(Value::Resource(2))
    );
    assert_eq!(
        prop_of(&expanded, &["main", "instance", "child_root", "sprite"], "texture"),
        Some(Value::Resource(3))
    );

    // 环：自引用文件（child 引用它自身）。
    let cyclic = child_referring.replace("Scenes/grandchild.ron", "Scenes/selfloop.ron");
    src.files.insert("Scenes/selfloop.ron", cyclic);
    let parent2 = parent_doc().replace("Scenes/child.ron", "Scenes/selfloop.ron");
    let parent2 = parse_ron(&parent2).expect("父文档");
    let err = expand_subscenes(&parent2, &mut |rel| src.load(rel))
        .expect_err("循环引用必须报错");
    assert!(err.contains("循环引用"), "指名问题：{err}");
    assert!(err.contains("Scenes/selfloop.ron -> Scenes/selfloop.ron"), "指名链条：{err}");
}

/// T-Sub-03：写回边界与幂等 —— 展开后的树回写只留引用；重新展开结构等价。
#[test]
fn t_sub_03_writeback_boundary_is_idempotent() {
    let mut src = Src::default();
    src.files.insert("Scenes/child.ron", child_doc());
    let parent = parse_ron(&parent_doc()).expect("父文档");
    let expanded = expand_subscenes(&parent, &mut |rel| src.load(rel)).expect("展开");

    // 展开后的文档实例化成树+表再打包：包装节点应只留引用（children 空）。
    let (mut tree, table, _report) =
        nes_scene::instantiate_doc_with_resources(&expanded).expect("实例化展开文档");
    tree.apply_pending();
    let written = nes_scene::write_ron_with_resources(&tree, &table, &PackOptions::verbose());
    let reparse = parse_ron(&written).expect("回读");
    let wrapper = node_at(&reparse, &["main", "instance"]);
    assert_eq!(
        wrapper.children.len(),
        0,
        "包装节点回写只留引用（子树归子场景文件）"
    );
    assert_eq!(
        wrapper.props.iter().find(|(k, _)| k == PROP_SUB_SCENE).map(|(_, v)| v.clone()),
        Some(Value::Resource(1))
    );
    // 资源表仍保留（含子资源），但子树不在文件里 —— 它由引用在加载时重建。
    assert_eq!(reparse.resources.len(), 2);

    // 幂等：再展开回读的文档，结构与第一次展开等价（名字与深度）。
    let re_expanded = expand_subscenes(&reparse, &mut |rel| src.load(rel)).expect("再展开");
    let a = node_at(&expanded, &["main", "instance", "child_root", "sprite"]);
    let b = node_at(&re_expanded, &["main", "instance", "child_root", "sprite"]);
    assert_eq!(a.name, b.name);
    assert_eq!(a.kind, b.kind);
    assert_eq!(
        prop_of(&re_expanded, &["main", "instance", "child_root", "sprite"], "texture"),
        Some(Value::Resource(2)),
        "引用编号在 循环 后保持稳定"
    );
}

/// T-Sub-04：未绑定（Resource(0)）不展开；悬垂槽位报错指名。
#[test]
fn t_sub_04_unbound_and_dangling() {
    let unbound = parent_doc().replace("Resource(1), },\n                children: []", "Resource(0), },\n                children: []");
    let unbound = parse_ron(&unbound).expect("父文档");
    let expanded = expand_subscenes(&unbound, &mut |rel| {
        let _ = rel;
        Err("不应读取任何文件".to_string())
    })
    .expect("未绑定不展开、不触盘");
    assert_eq!(node_at(&expanded, &["main", "instance"]).children.len(), 0);

    let dangling = parent_doc().replace(
        "Res(id: 1, path: \"Scenes/child.ron\", kind: \"Scene\"),",
        "",
    );
    let dangling = parse_ron(&dangling).expect("父文档");
    let err = expand_subscenes(&dangling, &mut |rel| {
        let _ = rel;
        Ok(parse_ron(&child_doc()).unwrap())
    })
    .expect_err("悬垂必须报错");
    assert!(err.contains("未声明"), "指名悬垂：{err}");
}
