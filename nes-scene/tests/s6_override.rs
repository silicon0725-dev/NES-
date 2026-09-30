//! T-Ovr 契约回归：实例级属性覆盖（S6.8，Godot override 语义的最小口径）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Ovr-01 | 应用：包装节点的覆盖记录在实例化时落到展开子树（local/process_mode/props 各自命中）；未覆盖字段保持子场景值 |
//! | T-Ovr-02 | 路径解析：深层路径逐段命中、`""` 命中子场景根；悬垂路径报语义错误并指名 |
//! | T-Ovr-03 | 回写往返：实例化后的树 -> 写出 -> 回读，覆盖记录原样保留，再实例化值等价 |
//! | T-Ovr-04 | 热重载语义（子场景更新）：同一父文档 + 新版子场景 -> 覆盖字段仍以覆盖为准，未覆盖字段跟随新内容 |

use nes_scene::{
    expand_subscenes, instantiate_doc_with_resources, parse_ron, write_ron_with_resources,
    NodeKind, PackOptions, ProcessMode, SceneTree, Value,
};

/// 子场景：根(带孙) + 精灵。精灵 local (0,0)、visible 默认、纹理槽 1。
const CHILD: &str = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Textures/demo.bmp", kind: "Texture"),
    ],
    root: Node(
        name: "child_root",
        kind: "Node2D",
        children: [
            Node(
                name: "holder",
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
        ],
    ),
)
"#;

/// 父场景：包装节点带覆盖 —— 精灵挪到 (16,0)、根自转 90°、holder 常显。
const PARENT_OVR: &str = r#"Scene(
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
                overrides: [
                    Override(path: "holder/sprite", local: (x: 16.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0)),
                    Override(path: "", local: (x: 0.0, y: 0.0, rot: 1.5707964, sx: 1.0, sy: 1.0, skew: 0.0)),
                    Override(path: "holder", process_mode: "Always"),
                ],
                children: [],
            ),
        ],
    ),
)
"#;

fn expand_with(child: &str) -> nes_scene::SceneDoc {
    let parent = parse_ron(PARENT_OVR).expect("父文档");
    expand_subscenes(&parent, &mut |rel| {
        assert_eq!(rel, "Scenes/child.ron");
        parse_ron(child).map_err(|e| e.to_string())
    })
    .expect("展开")
}

fn find(tree: &SceneTree, name: &str) -> nes_scene::NodeId {
    tree.find_by_name(name)
        .unwrap_or_else(|| panic!("找不到 {name}"))
}

/// T-Ovr-01：覆盖落到展开子树；未覆盖字段保持子场景值。
#[test]
fn t_ovr_01_overrides_apply_to_expanded_subtree() {
    let expanded = expand_with(CHILD);
    let (tree, _table, report) = instantiate_doc_with_resources(&expanded).expect("实例化");
    assert!(report.is_clean());

    let sprite = find(&tree, "sprite");
    let child_root = find(&tree, "child_root");
    let holder = find(&tree, "holder");

    // local 覆盖：精灵 (0,0) -> (16,0)。
    assert_eq!(tree.local(sprite).expect("local").pos.x, 16.0);
    // "" 命中子场景根：rot 90°。
    assert!((tree.local(child_root).expect("local").rot - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    // process_mode 覆盖：holder Always；未覆盖的 sprite 保持 Inherit。
    assert_eq!(tree.process_mode(holder), Some(ProcessMode::Always));
    assert_eq!(tree.process_mode(sprite), Some(ProcessMode::Inherit));
    // 未覆盖的属性保持子场景值（纹理仍在，种类标签未动）。
    assert_eq!(
        tree.get(sprite).expect("节点").props.get("texture"),
        Some(&Value::Resource(2)),
        "子场景纹理经展开合并重编号到槽 2"
    );
    assert_eq!(tree.kind(sprite).expect("kind"), &NodeKind::Sprite2D);
}

/// T-Ovr-02：悬垂路径报语义错误并指名（包装节点 + 完整路径 + 未命中的段）。
#[test]
fn t_ovr_02_dangling_override_path_is_semantic_error() {
    let bad = PARENT_OVR.replace(
        "Override(path: \"holder\", process_mode: \"Always\")",
        "Override(path: \"holder/ghost\", process_mode: \"Always\")",
    );
    let parent = parse_ron(&bad).expect("父文档");
    let expanded = expand_subscenes(&parent, &mut |rel| {
        assert_eq!(rel, "Scenes/child.ron");
        parse_ron(CHILD).map_err(|e| e.to_string())
    })
    .expect("展开（覆盖不在此阶段应用）");
    let err = instantiate_doc_with_resources(&expanded)
        .err()
        .expect("悬垂路径必须报错");
    let msg = format!("{err}");
    assert!(msg.contains("未命中"), "指名问题：{msg}");
    assert!(msg.contains("holder/ghost"), "指名路径：{msg}");
    assert!(msg.contains("instance"), "指名包装节点：{msg}");
}

/// T-Ovr-03：回写往返 —— 覆盖记录原样保留，再实例化值等价。
#[test]
fn t_ovr_03_writeback_roundtrip_preserves_records() {
    let expanded = expand_with(CHILD);
    let (mut tree, table, _report) = instantiate_doc_with_resources(&expanded).expect("实例化");
    tree.apply_pending();
    let written = write_ron_with_resources(&tree, &table, &PackOptions::verbose());
    assert!(written.contains("overrides: ["), "覆盖块写出：\n{written}");
    assert!(written.contains("holder/sprite"), "路径保留：\n{written}");

    let reparse = parse_ron(&written).expect("回读");
    let wrapper = &reparse.root.children[0];
    assert_eq!(wrapper.overrides.len(), 3, "三条记录原样保留");
    assert_eq!(wrapper.overrides[0].path, "holder/sprite");
    assert!(wrapper.overrides[0].local.is_some());
    assert_eq!(wrapper.overrides[2].process_mode, Some(ProcessMode::Always));

    // 再展开（回写只留引用，须重新嫁接）-> 再实例化：结果等价（精灵仍在 (16,0)）。
    let re_expanded = expand_subscenes(&reparse, &mut |_| parse_ron(CHILD).map_err(|e| e.to_string()))
        .expect("再展开");
    let (tree2, _t2, _r2) = instantiate_doc_with_resources(&re_expanded).expect("再实例化");
    let sprite2 = find(&tree2, "sprite");
    assert_eq!(tree2.local(sprite2).expect("local").pos.x, 16.0);
}

/// T-Ovr-04：热重载语义 —— 子场景更新后，覆盖字段仍以覆盖为准，
/// 未覆盖字段跟随新内容（这正是覆盖记录放在父文件里的回报）。
#[test]
fn t_ovr_04_subscene_update_keeps_overrides() {
    // 新版子场景：精灵在子场景里被挪到 (48,0)（子场景作者改的），根加了缩放 2x。
    let child_v2 = CHILD.replace(
        "name: \"sprite\",\n                        kind: \"Sprite2D\",\n                        props:",
        "name: \"sprite\",\n                        kind: \"Sprite2D\",\n                        local: (x: 48.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),\n                        props:",
    );
    assert!(child_v2.contains("48.0"), "改写应生效");

    let expanded = expand_with(&child_v2);
    let (tree, _table, _report) = instantiate_doc_with_resources(&expanded).expect("实例化");
    let sprite = find(&tree, "sprite");
    // 覆盖赢：local (16,0) 而不是子场景新版里的 (48,0)。
    assert_eq!(
        tree.local(sprite).expect("local").pos.x, 16.0,
        "覆盖字段以覆盖为准（不是子场景新值 48）"
    );
    // 根的覆盖也保持：rot 90°（子场景没动根的 rot，覆盖本来就在）。
    let child_root = find(&tree, "child_root");
    assert!(
        (tree.local(child_root).expect("local").rot - std::f32::consts::FRAC_PI_2).abs() < 1e-5,
        "根覆盖保持"
    );
    // 未覆盖字段跟随子场景：精灵的纹理仍是槽 2（去重合并后的子纹理）。
    assert_eq!(
        tree.get(sprite).expect("节点").props.get("texture"),
        Some(&Value::Resource(2)),
        "未覆盖属性跟随子场景（重编号后的槽位）"
    );
}

// ---------------------------------------------------------------- diff 式回写
// S6.9：运行时对实例内部节点的编辑 -> 与"当前磁盘子场景的独立实例化"diff
// -> 全量重生成覆盖记录。保存仍是纯读（烘焙是显式动作）。

use nes_scene::{diff_instance_overrides, Transform2D};

/// 参照：CHILD 的独立实例化（树 + 子场景自己的槽位表）。
fn reference_of(child: &str) -> (SceneTree, nes_scene::ResourceTable) {
    let doc = parse_ron(child).expect("子文档");
    let (tree, table, _report) = instantiate_doc_with_resources(&doc).expect("参照实例化");
    (tree, table)
}

/// T-Ovr-05：diff 生成 —— 运行时编辑（sprite 挪位 + 改模式）被捕获为记录；
/// 已有覆盖（根自转 / holder Always）仍被重新捕获；**纹理槽位不同但指向
/// 同一文件不误报**（展开合并后父槽 2 vs 参照槽 1）。
#[test]
fn t_ovr_05_diff_captures_runtime_edits() {
    let expanded = expand_with(CHILD);
    let (mut tree, table, _report) = instantiate_doc_with_resources(&expanded).expect("实例化");
    // 已有三条覆盖（sprite 16 / 根自转 / holder Always）已应用。

    // 运行时编辑：sprite 再挪到 (30,0) 并改为 Disabled。
    let sprite = find(&tree, "sprite");
    tree.set_local(sprite, Transform2D::from_pos(30.0, 0.0));
    tree.set_process_mode(sprite, ProcessMode::Disabled);

    let (ref_tree, ref_table) = reference_of(CHILD);
    let wrapper = find(&tree, "instance");
    let records = diff_instance_overrides(&tree, wrapper, &table, &ref_tree, &ref_table);

    // 恰好三条：sprite / 根 / holder（纹理属性不产生记录）。
    assert_eq!(records.len(), 3, "记录数：{records:#?}");
    let by_path = |p: &str| records.iter().find(|r| r.path == p).unwrap_or_else(|| panic!("缺 {p}"));

    let sp = by_path("holder/sprite");
    assert_eq!(sp.local.expect("local 覆盖").pos.x, 30.0, "运行时编辑值");
    assert_eq!(sp.process_mode, Some(ProcessMode::Disabled));
    assert!(sp.props.is_empty(), "纹理指向同一文件：不误报为覆盖");

    let root_ov = by_path("");
    assert!(
        (root_ov.local.expect("根 local").rot - std::f32::consts::FRAC_PI_2).abs() < 1e-5,
        "已有覆盖被重新捕获"
    );
    let holder_ov = by_path("holder");
    assert_eq!(holder_ov.process_mode, Some(ProcessMode::Always));
}

/// T-Ovr-06：无差异 -> 空记录（含"从未覆盖"的干净实例）；幂等 ——
/// 应用 diff 结果后再 diff，记录不再增长。
#[test]
fn t_ovr_06_diff_is_empty_when_clean_and_idempotent() {
    // 干净父场景（无覆盖）。
    let parent_clean = PARENT_OVR.replace(
        "overrides: [\n                    Override(path: \"holder/sprite\", local: (x: 16.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0)),\n                    Override(path: \"\", local: (x: 0.0, y: 0.0, rot: 1.5707964, sx: 1.0, sy: 1.0, skew: 0.0)),\n                    Override(path: \"holder\", process_mode: \"Always\"),\n                ],\n",
        "",
    );
    let parent = parse_ron(&parent_clean).expect("父文档");
    let expanded = expand_subscenes(&parent, &mut |_| parse_ron(CHILD).map_err(|e| e.to_string()))
        .expect("展开");
    let (tree, table, _report) = instantiate_doc_with_resources(&expanded).expect("实例化");

    let (ref_tree, ref_table) = reference_of(CHILD);
    let records = diff_instance_overrides(&tree, find(&tree, "instance"), &table, &ref_tree, &ref_table);
    assert!(records.is_empty(), "无编辑无覆盖：{records:#?}");

    // 幂等：编辑 -> diff 得 n 条 -> 把记录**应用**回去（模拟烘焙后的树）-> 再 diff 仍 n 条。
    let mut tree2 = tree;
    let sprite = find(&tree2, "sprite");
    tree2.set_local(sprite, Transform2D::from_pos(30.0, 0.0));
    let first = diff_instance_overrides(&tree2, find(&tree2, "instance"), &table, &ref_tree, &ref_table);
    assert_eq!(first.len(), 1);
    // 烘焙后的树 == 编辑后的树（记录已在树上生效），再 diff 不会翻倍。
    let second = diff_instance_overrides(&tree2, find(&tree2, "instance"), &table, &ref_tree, &ref_table);
    assert_eq!(second, first, "幂等：不增长、不漂移");
}

// ---------------------------------------------------------------- 结构性覆盖
// S6.10：实例内增删节点成为覆盖记录（add / remove），与字段覆盖同一管道。

/// 带结构性覆盖的父场景：holder 下追加 extra（含属性），移除 holder/sprite。
const PARENT_STRUCT: &str = r#"Scene(
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
                overrides: [
                    Override(path: "holder", add: [
                        Node(
                            name: "extra",
                            kind: "Sprite2D",
                            local: (x: 0.0, y: 40.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),
                            props: { "flip_h": Bool(true), },
                            children: [],
                        ),
                    ]),
                    Override(path: "holder/sprite", remove: true),
                ],
                children: [],
            ),
        ],
    ),
)
"#;

/// T-Ovr-07：结构性覆盖应用 —— extra 落在 holder 下（含属性与变换），
/// sprite 整棵移除；包装节点的记录镜像在树上可读回。
#[test]
fn t_ovr_07_structural_overrides_apply() {
    let parent = parse_ron(PARENT_STRUCT).expect("父文档");
    let expanded = expand_subscenes(&parent, &mut |_| parse_ron(CHILD).map_err(|e| e.to_string()))
        .expect("展开");
    let (tree, _table, report) = instantiate_doc_with_resources(&expanded).expect("实例化");
    assert!(report.is_clean());

    assert!(tree.find_by_name("extra").is_some(), "追加节点落地");
    let extra = find(&tree, "extra");
    assert_eq!(tree.local(extra).expect("local").pos.y, 40.0, "追加节点变换");
    assert_eq!(
        tree.get(extra).expect("节点").props.get("flip_h"),
        Some(&Value::Bool(true)),
        "追加节点属性"
    );
    assert!(tree.find_by_name("sprite").is_none(), "移除节点消失");
    assert!(tree.find_by_name("holder").is_some(), "目标父节点仍在");

    // 记录镜像在树上（写回来源）。
    let wrapper = find(&tree, "instance");
    assert_eq!(tree.instance_overrides(wrapper).map(<[_]>::len), Some(2));
}

/// T-Ovr-08：diff 生成结构记录 —— 运行时删 sprite、在 holder 下新增节点 ->
/// remove 记录 + add 记录（子树全量导出）；无结构编辑时无结构记录。
#[test]
fn t_ovr_08_diff_generates_structural_records() {
    // 干净加载（无覆盖）。
    let parent_clean = PARENT_STRUCT.replace(
        "overrides: [\n                    Override(path: \"holder\", add: [\n                        Node(\n                            name: \"extra\",\n                            kind: \"Sprite2D\",\n                            local: (x: 0.0, y: 40.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),\n                            props: { \"flip_h\": Bool(true), },\n                            children: [],\n                        ),\n                    ]),\n                    Override(path: \"holder/sprite\", remove: true),\n                ],\n",
        "",
    );
    let parent = parse_ron(&parent_clean).expect("父文档");
    let expanded = expand_subscenes(&parent, &mut |_| parse_ron(CHILD).map_err(|e| e.to_string()))
        .expect("展开");
    let (mut tree, table, _report) = instantiate_doc_with_resources(&expanded).expect("实例化");
    let (ref_tree, ref_table) = reference_of(CHILD);

    // 无结构编辑：无任何记录。
    let clean = diff_instance_overrides(&tree, find(&tree, "instance"), &table, &ref_tree, &ref_table);
    assert!(clean.is_empty(), "干净实例无记录：{clean:#?}");

    // 运行时结构编辑：删 sprite；holder 下加 badge（带子节点，验证子树导出）。
    let sprite = find(&tree, "sprite");
    tree.queue(nes_scene::TreeOp::Remove { node: sprite, keep_children: false });
    tree.apply_pending();
    let holder = find(&tree, "holder");
    let badge = tree.add_node(holder, "badge", NodeKind::Node2D);
    tree.set_local(badge, Transform2D::from_pos(5.0, 6.0));
    let dot = tree.add_node(badge, "dot", NodeKind::Sprite2D);
    tree.set_prop(dot, "flip_h", Value::Bool(true)).unwrap();
    tree.apply_pending();

    let records = diff_instance_overrides(&tree, find(&tree, "instance"), &table, &ref_tree, &ref_table);
    let rm = records.iter().find(|r| r.remove).expect("remove 记录");
    assert_eq!(rm.path, "holder/sprite");
    let add = records.iter().find(|r| !r.add.is_empty()).expect("add 记录");
    assert_eq!(add.path, "holder");
    assert_eq!(add.add.len(), 1);
    assert_eq!(add.add[0].name, "badge");
    assert_eq!(add.add[0].local.pos.y, 6.0, "子树变换导出");
    assert_eq!(add.add[0].children.len(), 1, "子树后代导出");
    assert_eq!(add.add[0].children[0].name, "dot");
}

/// T-Ovr-09：结构覆盖回写往返 + 再应用复现 —— 写出的文件回读记录相等，
/// 再展开实例化后结构与编辑后的树一致（extra 在、sprite 无）。
#[test]
fn t_ovr_09_structural_roundtrip() {
    let parent = parse_ron(PARENT_STRUCT).expect("父文档");
    let expanded = expand_subscenes(&parent, &mut |_| parse_ron(CHILD).map_err(|e| e.to_string()))
        .expect("展开");
    let (mut tree, table, _report) = instantiate_doc_with_resources(&expanded).expect("实例化");
    tree.apply_pending();
    let written = write_ron_with_resources(&tree, &table, &PackOptions::verbose());
    assert!(written.contains("add: ["), "add 写出：\n{written}");
    assert!(written.contains("remove: true"), "remove 写出：\n{written}");

    let reparse = parse_ron(&written).expect("回读");
    let wrapper = &reparse.root.children[0];
    assert_eq!(wrapper.overrides.len(), 2);
    assert_eq!(wrapper.overrides[0].add.len(), 1);
    assert_eq!(wrapper.overrides[0].add[0].name, "extra");
    assert!(wrapper.overrides[1].remove);

    // 再展开（回写只留引用）-> 实例化：结构复现。
    let re_expanded = expand_subscenes(&reparse, &mut |_| parse_ron(CHILD).map_err(|e| e.to_string()))
        .expect("再展开");
    let (tree2, _t2, _r2) = instantiate_doc_with_resources(&re_expanded).expect("再实例化");
    assert!(tree2.find_by_name("extra").is_some(), "extra 复现");
    assert!(tree2.find_by_name("sprite").is_none(), "sprite 仍被移除");
}

// ---------------------------------------------------------------- 重命名覆盖
// S6.11：rename 记录 —— 保留跟踪（节点仍属子场景），不像 remove+add 冻结副本。

/// T-Ovr-10：rename 应用 —— 节点按记录改名（旧名寻址失效、新名可查）；
/// rename 与字段覆盖可同记录；与 remove 互斥由解析层保证。
#[test]
fn t_ovr_10_rename_applies_and_keeps_fields() {
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
                name: "instance",
                kind: "Node2D",
                props: { "sub_scene": Resource(1), },
                overrides: [
                    Override(path: "holder/sprite", rename: "hero", local: (x: 16.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0)),
                ],
                children: [],
            ),
        ],
    ),
)
"#;
    let parent = parse_ron(parent).expect("父文档");
    let expanded = expand_subscenes(&parent, &mut |_| parse_ron(CHILD).map_err(|e| e.to_string()))
        .expect("展开");
    let (tree, _table, report) = instantiate_doc_with_resources(&expanded).expect("实例化");
    assert!(report.is_clean());

    let hero = tree.find_by_name("hero").expect("新名可查");
    assert!(tree.find_by_name("sprite").is_none(), "旧名寻址失效");
    assert_eq!(tree.local(hero).expect("local").pos.x, 16.0, "同记录字段覆盖生效");
    assert_eq!(
        tree.get(hero).expect("节点").props.get("texture"),
        Some(&Value::Resource(2)),
        "节点仍属子场景（纹理经合并槽位流入）"
    );

    // 互斥：rename + remove 同时声明 -> 解析层拒绝并指名。
    let err = parse_ron(&parent_src_with_rename_and_remove())
        .expect_err("rename+remove 必须被解析层拒绝");
    assert!(err.to_string().contains("互斥"), "指名问题：{err}");
}

/// 构造 rename+remove 同记录的非法父文档（供互斥校验断言）。
fn parent_src_with_rename_and_remove() -> String {
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
                props: { "sub_scene": Resource(1), },
                overrides: [
                    Override(path: "holder/sprite", rename: "hero", remove: true),
                ],
                children: [],
            ),
        ],
    ),
)
"#
    .to_string()
}

/// T-Ovr-11：diff 识别 rename —— 同种类未配对 -> 单条 rename 记录（不是
/// remove+add），字段差异并入；不同种类 -> 仍走 remove+add。
#[test]
fn t_ovr_11_diff_detects_rename() {
    // 干净父场景加载。
    let parent_clean = r#"Scene(
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
                props: { "sub_scene": Resource(1), },
                children: [],
            ),
        ],
    ),
)
"#;
    let parent = parse_ron(parent_clean).expect("父文档");
    let expanded = expand_subscenes(&parent, &mut |_| parse_ron(CHILD).map_err(|e| e.to_string()))
        .expect("展开");
    let (mut tree, table, _report) = instantiate_doc_with_resources(&expanded).expect("实例化");
    let (ref_tree, ref_table) = reference_of(CHILD);

    // 运行时重命名 sprite -> hero（并挪到 (16,0)）。
    let sprite = find(&tree, "sprite");
    tree.queue(nes_scene::TreeOp::Rename { node: sprite, name: "hero".to_string() });
    tree.set_local(sprite, Transform2D::from_pos(16.0, 0.0));
    tree.apply_pending();

    let records = diff_instance_overrides(&tree, find(&tree, "instance"), &table, &ref_tree, &ref_table);
    let ren = records
        .iter()
        .find(|r| r.rename.is_some())
        .expect("rename 记录（不是 remove+add）");
    assert_eq!(ren.path, "holder/sprite", "路径按子场景原名");
    assert_eq!(ren.rename.as_deref(), Some("hero"));
    assert_eq!(ren.local.expect("字段并入").pos.x, 16.0);
    assert!(!ren.remove && ren.add.is_empty(), "不是 remove+add");
    assert!(records.iter().all(|r| !r.remove), "无 remove 记录");

    // 对照：删 Sprite2D + 加 Node2D（不同种类）仍走 remove+add。
    let (mut tree2, table2, _r2) = instantiate_doc_with_resources(&expanded2()).expect("实例化2");
    let _ = table2;
    let sp2 = find(&tree2, "sprite");
    tree2.queue(nes_scene::TreeOp::Remove { node: sp2, keep_children: false });
    let holder2 = find(&tree2, "holder");
    tree2.add_node(holder2, "badge", NodeKind::Node2D);
    tree2.apply_pending();
    let rec2 = diff_instance_overrides(&tree2, find(&tree2, "instance"), &table, &ref_tree, &ref_table);
    assert!(rec2.iter().any(|r| r.remove), "不同种类 -> remove");
    assert!(rec2.iter().any(|r| !r.add.is_empty()), "不同种类 -> add");
}

fn expanded2() -> nes_scene::SceneDoc {
    let parent = parse_ron(r#"Scene(
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
                props: { "sub_scene": Resource(1), },
                children: [],
            ),
        ],
    ),
)
"#).expect("父文档");
    expand_subscenes(&parent, &mut |_| parse_ron(CHILD).map_err(|e| e.to_string())).expect("展开")
}

/// T-Ovr-12：rename 往返 + **保留跟踪** —— 子场景更新后，被改名节点的
/// 未覆盖字段跟随新内容（这是 rename 相对 remove+add 的核心收益）。
#[test]
fn t_ovr_12_rename_keeps_tracking_across_updates() {
    // 子场景 v2：sprite 挪到 (48,0)（子场景作者的更新）。
    let child_v2 = CHILD.replace(
        "name: \"sprite\",\n                        kind: \"Sprite2D\",\n                        props:",
        "name: \"sprite\",\n                        kind: \"Sprite2D\",\n                        local: (x: 48.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0),\n                        props:",
    );

    // 父场景带 rename（无字段覆盖）。
    let parent_src = r#"Scene(
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
                props: { "sub_scene": Resource(1), },
                overrides: [
                    Override(path: "holder/sprite", rename: "hero"),
                ],
                children: [],
            ),
        ],
    ),
)
"#;
    let parent = parse_ron(parent_src).expect("父文档");
    let expanded = expand_subscenes(&parent, &mut |_| parse_ron(&child_v2).map_err(|e| e.to_string()))
        .expect("展开（新子场景）");
    let (tree, _table, _report) = instantiate_doc_with_resources(&expanded).expect("实例化");

    let hero = tree.find_by_name("hero").expect("改名生效");
    assert_eq!(
        tree.local(hero).expect("local").pos.x, 48.0,
        "保留跟踪：未覆盖字段跟随子场景新值（rename 不是冻结副本）"
    );
}
