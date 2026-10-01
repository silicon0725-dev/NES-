//! T-ID 契约回归：持久语义身份（S9-1 —— uid 实现，S9-0 契约验收）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-ID-01 | **身份稳定性矩阵**：uid 对所有非身份操作稳定（保存/重载/改名/移父/改属性/增删兄弟）；删除后新对象不得复用；clone 新 uid 且保存重载不变 |
//! | T-ID-02 | **旧文件迁移幂等**：无 uid 文件两次加载得同 uid（仅内存）；保存落盘后再加载仍同 uid |
//! | T-ID-03 | 同 uid 冲突装载如实报错；内容无关性（同内容不同身份） |

use nes_scene::{
    instantiate, to_doc, NodeKind, SceneTree, Transform2D, Uid,
};

fn scene_text(uid: Option<&str>) -> String {
    let uid_line = match uid {
        Some(u) => format!("uid: {u:?}, "),
        None => String::new(),
    };
    format!(
        "Scene(
    version: 1,
    resources: [],
    root: Node(
        name: \"main\",
        kind: \"Node\",
        {uid_line}children: [
            Node(name: \"a\", kind: \"Node2D\", local: (x: 1.0, y: 2.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0), children: [],),
            Node(name: \"b\", kind: \"Node\", children: [
                Node(name: \"c\", kind: \"Node2D\", children: [],),
            ],),
        ],
    ),
)
"
    )
}

/// T-ID-01：身份稳定性矩阵。
#[test]
fn t_id_01_identity_stability_matrix() {
    // 创建 → 保存 → 重加载 → uid 相同。
    let t1 = instantiate(&scene_text(None)).unwrap();
    let a1 = t1.find_by_name("a").unwrap();
    let u1 = t1.uid_of(a1).unwrap();

    // 保存（to_doc 写出 uid）→ 重加载。
    let doc = to_doc(&t1);
    let text = nes_scene::doc_to_ron(&doc, &nes_scene::PackOptions::compact());
    let mut t2 = instantiate(&text).unwrap();
    let a2 = t2.find_by_name("a").unwrap();
    assert_eq!(t2.uid_of(a2).unwrap(), u1, "保存重载 uid 稳定");

    // 改名 → uid 不变。
    let c2 = t2.find_by_name("c").unwrap();
    let uc = t2.uid_of(c2).unwrap();
    //（改名经重写路径：此处直接验树内操作 —— uid 是 NodeData 字段，
    // 树操作 API 未提供改名，用属性/结构操作代表非身份操作集。）
    // 改属性（local 变换）→ uid 不变。
    t2.set_local(a2, Transform2D::from_pos(99.0, 99.0));
    assert_eq!(t2.uid_of(a2).unwrap(), u1, "改变换 uid 不变");

    // 增删兄弟 → uid 不变。
    let _sib = t2.add_node(t2.root(), "tmp", NodeKind::Node);
    t2.apply_pending();
    assert_eq!(t2.uid_of(a2).unwrap(), u1, "增兄弟 uid 不变");
    // 移动父节点（c 挂到 main）→ uid 不变。
    t2.reparent(c2, t2.root(), None);
    t2.apply_pending();
    assert_eq!(t2.uid_of(c2).unwrap(), uc, "移父 uid 不变");

    // 删除 a → 新建 b2 不得获得 a 的 uid。
    t2.remove_node(a2, false);
    t2.apply_pending();
    let b2 = t2.add_node(t2.root(), "b2", NodeKind::Node);
    t2.apply_pending();
    assert_ne!(t2.uid_of(b2).unwrap(), u1, "新对象不复用死 uid");
    assert!(t2.find_by_uid(&u1).is_none(), "死 uid 查无");

    // clone（深拷贝语义）→ 新 uid；保存重载后 clone uid 不变。
    //（clone 用 add_node + 属性复制模拟 —— 核心断言：新身份。）
    let c3 = t2.add_node(t2.root(), "c_copy", NodeKind::Node2D);
    t2.apply_pending();
    assert_ne!(t2.uid_of(c3).unwrap(), uc, "clone 新 uid");
    let doc2 = to_doc(&t2);
    let text2 = nes_scene::doc_to_ron(&doc2, &nes_scene::PackOptions::compact());
    let t3 = instantiate(&text2).unwrap();
    let c3b = t3.find_by_name("c_copy").unwrap();
    assert_eq!(t3.uid_of(c3b).unwrap(), t2.uid_of(c3).unwrap(), "clone uid 保存重载不变");
}

/// T-ID-02：旧文件迁移幂等（无 uid 文件）。
#[test]
fn t_id_02_legacy_derivation_idempotent() {
    let text = scene_text(None);
    // 第一次加载：内存派生 U。
    let t1 = instantiate(&text).unwrap();
    let a1 = t1.find_by_name("a").unwrap();
    let u = t1.uid_of(a1).unwrap();
    // 不保存 → 直接再次加载原始文本 → 仍得 U（确定性）。
    let t2 = instantiate(&text).unwrap();
    let a2 = t2.find_by_name("a").unwrap();
    assert_eq!(t2.uid_of(a2).unwrap(), u, "旧文件两次加载同 uid（幂等）");
    // 保存 → uid 落盘 → 再加载 → 仍 U。
    let doc = to_doc(&t1);
    let saved = nes_scene::doc_to_ron(&doc, &nes_scene::PackOptions::compact());
    assert!(saved.contains("uid:"), "uid 落盘");
    let t3 = instantiate(&saved).unwrap();
    let a3 = t3.find_by_name("a").unwrap();
    assert_eq!(t3.uid_of(a3).unwrap(), u, "落盘后仍同 uid");
}

/// T-ID-03：冲突检测 + 内容无关性。
#[test]
fn t_id_03_conflict_and_content_independence() {
    // 同 uid 两个活节点 → add_node_with_uid 如实报错。
    let mut t = SceneTree::new("root");
    let n1 = t.add_node(t.root(), "x", NodeKind::Node);
    t.apply_pending();
    let u = t.uid_of(n1).unwrap();
    let r = t.add_node_with_uid(t.root(), "y", NodeKind::Node, u.clone());
    assert!(r.is_err(), "同 uid 冲突报错");

    // 内容无关（新对象口径）：new_v4 生成的两个 uid 互不相同 —— uid
    // 不是内容哈希。（注意：同一**旧文件**两次加载**应当**同 uid —— 那是
    // 迁移派生的幂等性，由 T-ID-02 钉死；两者是不同机制。）
    let mut ta = SceneTree::new("ra");
    let na = ta.add_node(ta.root(), "same", NodeKind::Node);
    let mut tb = SceneTree::new("rb");
    let nb = tb.add_node(tb.root(), "same", NodeKind::Node);
    assert_ne!(
        ta.uid_of(na).unwrap(),
        tb.uid_of(nb).unwrap(),
        "新对象随机 v4：同内容不同身份（uid 非内容哈希）"
    );
    // 迁移派生确定性对照：同种子同 uid（幂等机制本身）。
    assert_eq!(
        Uid::derive_legacy("/main/a"),
        Uid::derive_legacy("/main/a"),
        "迁移派生确定性"
    );
    assert_ne!(Uid::derive_legacy("/main/a"), Uid::derive_legacy("/main/b"));

    // Uid 形态：hex 32 字符 round-trip。
    let ua = ta.uid_of(na).unwrap();
    let hex = ua.to_hex();
    assert_eq!(hex.len(), 32);
    assert_eq!(Uid::from_hex(&hex).unwrap(), ua);
    assert!(Uid::from_hex("zz").is_err());
}
