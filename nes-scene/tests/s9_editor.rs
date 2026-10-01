//! T-SEL / T-INS / T-HIER 契约回归：编辑器核心状态层（S9-3a）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-SEL-01 | 选择稳定性：select 后 primary 命中；悬空保留 |
//! | T-SEL-02 | undo/redo 悬空恢复：删除→选择悬空（live 空）→undo 自动重新有效→redo 再悬空；**条目始终在集** |
//! | T-INS-01 | Inspector 单字段修改 = 一条事务（undo 恢复旧值） |
//! | T-INS-02 | gizmo 拖拽合并提交：中间帧直写（preview 不入账），落点一次 modify_local + commit = **一条事务** |
//! | T-HIER-01 | 层级拖拽（drag_to）保 uid：重排/移父一条 Reparented；undo 回原位 |
//! | T-HIER-02 | 兄弟序保存恢复：create/drag/保存/重载后序一致 |

use nes_scene::{
    editor::{Hierarchy, Inspector, Selection},
    transaction::TransactionLog,
    NodeKind, SceneTree, Transform2D, Value,
};

fn tree_with_a() -> (SceneTree, nes_scene::NodeId, nes_scene::Uid) {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let a = t.add_node(t.root(), "a", NodeKind::Node2D);
    t.apply_pending();
    let uid = t.uid_of(a).unwrap();
    (t, a, uid)
}

/// T-SEL-01：选择稳定性 + 悬空保留。
#[test]
fn t_sel_01_selection_stability() {
    let (mut t, a, uid_a) = tree_with_a();
    let b = t.add_node(t.root(), "b", NodeKind::Node);
    t.apply_pending();
    let uid_b = t.uid_of(b).unwrap();

    let mut sel = Selection::new();
    assert!(sel.is_empty());
    sel.select(uid_a.clone());
    sel.select(uid_b.clone());
    assert_eq!(sel.len(), 2);
    assert_eq!(sel.primary(&t), Some(b), "后选成主选");
    // 重选已选条目 -> 移回首（主选）不移除。
    sel.select(uid_a.clone());
    assert_eq!(sel.len(), 2, "重选不重复");
    assert_eq!(sel.primary(&t), Some(a));
    assert!(sel.contains(&uid_a));

    // toggle 移除。
    sel.toggle(uid_b.clone());
    assert_eq!(sel.len(), 1);
    // 悬空：删除 a 后条目保留、live/primary 空。
    t.remove_node(a, false);
    t.apply_pending();
    assert!(sel.contains(&uid_a), "悬空条目保留");
    assert!(sel.live(&t).is_empty());
    assert_eq!(sel.primary(&t), None);
}

/// T-SEL-02：undo/redo 悬空恢复（身份连续性 -> 选择连续性）。
#[test]
fn t_sel_02_dangling_recovery() {
    let (mut t, _a, uid_a) = tree_with_a();

    let mut sel = Selection::new();
    sel.select(uid_a.clone());
    let mut log = TransactionLog::new();
    {
        let mut h = Hierarchy::new(&mut t, &mut log);
        h.begin().unwrap();
        h.delete_subtree(&uid_a).unwrap();
        // 选择不属于事务：commit 后事务只有一条 Removed。
        let _ = &mut h.commit();
    }
    assert!(sel.live(&t).is_empty(), "删除后选择悬空（live 空）");
    assert!(sel.contains(&uid_a), "条目保留");
    assert_eq!(sel.primary(&t), None);

    // Undo：a 复活 -> 选择自动重新有效（无需重选）。
    assert!(log.undo(&mut t).unwrap());
    assert!(sel.primary(&t).is_some(), "undo 后选择自动恢复");
    assert!(sel.contains(&uid_a));

    // Redo：再删 -> 悬空但条目仍在（往返不丢编辑器状态）。
    assert!(log.redo(&mut t).unwrap());
    assert!(sel.primary(&t).is_none());
    assert!(sel.contains(&uid_a), "redo 后条目仍保留");
}

/// T-INS-01：Inspector 单字段修改 = 一条事务（undo 恢复旧值）。
#[test]
fn t_ins_01_single_field_transaction() {
    let (mut t, a, uid_a) = tree_with_a();
    t.set_local(a, Transform2D::from_pos(1.0, 1.0));
    t.apply_pending();

    let mut log = TransactionLog::new();
    {
        let mut ins = Inspector::new(&mut t, &mut log);
        ins.begin().unwrap();
        ins.modify_prop(&uid_a, "visible", Value::Bool(false)).unwrap();
        ins.modify_local(&uid_a, Transform2D::from_pos(9.0, 9.0)).unwrap();
        ins.commit().unwrap();
    }
    // 两字段各自一条 Modified，同一事务。
    assert!(log.can_undo() && !log.can_redo());
    assert_eq!(t.prop(a, "visible"), Some(&Value::Bool(false)));

    assert!(log.undo(&mut t).unwrap());
    let id = t.find_by_uid(&uid_a).unwrap();
    assert_eq!(t.local(id).unwrap().pos.x, 1.0, "undo 回旧值");
    assert_eq!(t.prop(id, "visible"), Some(&Value::Bool(true)));
}

/// T-INS-02：gizmo 拖拽合并提交 —— 中间帧 preview 直写不入账，
/// 落点一次 modify_local + commit = 一条事务。
#[test]
fn t_ins_02_gizmo_drag_merged() {
    let (mut t, a, uid_a) = tree_with_a();
    let mut log = TransactionLog::new();

    // 拖拽会话：begin -> 100 次 preview（直写）-> 落点记账 -> commit。
    // preview 在事务外直写（会话态 —— 适配器不持借用），
    // 落点一次记账 + commit = 一条事务。
    log.begin().unwrap();
    for i in 1..=100u32 {
        t.set_local(a, Transform2D::from_pos(i as f32, 0.0));
    }
    {
        let mut ins = Inspector::new(&mut t, &mut log);
        ins.modify_local(&uid_a, Transform2D::from_pos(100.0, 0.0)).unwrap();
    }
    log.commit().unwrap();
    assert!(log.can_undo());
    // undo 一步回到拖拽前（事务只有一条）。
    assert!(log.undo(&mut t).unwrap());
    assert!(!log.can_undo(), "100 帧拖拽 = 恰一条事务");
}

/// T-HIER-01：层级拖拽保 uid；undo 回原位。
#[test]
fn t_hier_01_drag_keeps_uid() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let root_uid = t.uid_of(t.root()).unwrap();
    let x = t.add_node(t.root(), "x", NodeKind::Node);
    let y = t.add_node(t.root(), "y", NodeKind::Node);
    let n = t.add_node(x, "n", NodeKind::Node2D);
    t.apply_pending();
    let (uid_n, _uid_x, uid_y) = (
        t.uid_of(n).unwrap(),
        t.uid_of(x).unwrap(),
        t.uid_of(y).unwrap(),
    );

    let mut log = TransactionLog::new();
    {
        let mut h = Hierarchy::new(&mut t, &mut log);
        h.begin().unwrap();
        h.drag_to(&uid_n, &uid_y, None).unwrap();
        let _ = &mut h.commit();
    }
    let id = t.find_by_uid(&uid_n).unwrap();
    assert_eq!(t.parent(id), Some(y), "拖到 y 下");
    assert_eq!(t.uid_of(id).unwrap(), uid_n, "uid 不变");

    // undo：回原父。
    assert!(log.undo(&mut t).unwrap());
    let id = t.find_by_uid(&uid_n).unwrap();
    assert_eq!(t.parent(id), Some(x), "undo 回原父");
    // redo：再到 y。
    assert!(log.redo(&mut t).unwrap());
    assert_eq!(t.parent(t.find_by_uid(&uid_n).unwrap()), Some(y));
    let _ = root_uid;
}

/// T-HIER-02：兄弟序保存恢复 —— create/drag 后序列化重载，序一致。
#[test]
fn t_hier_02_sibling_order_roundtrip() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let root_uid = t.uid_of(t.root()).unwrap();
    let mut log = TransactionLog::new();
    let mut uids = Vec::new();
    {
        let mut h = Hierarchy::new(&mut t, &mut log);
        h.begin().unwrap();
        for name in ["c2", "c0", "c1"] {
            uids.push(h.create_child(&root_uid, name, NodeKind::Node).unwrap());
        }
        // 重排：把 c2（首位）拖到末位。
        h.drag_to(&uids[0], &root_uid, Some(2)).unwrap();
        let _ = &mut h.commit();
    }
    let kids: Vec<&str> = t.children(t.root()).iter().filter_map(|&c| t.name(c)).collect();
    assert_eq!(kids, vec!["c0", "c1", "c2"], "拖拽后兄弟序");

    // 保存 -> 重载 -> 序一致 + uid 一致。
    let doc = nes_scene::to_doc(&t);
    let text = nes_scene::doc_to_ron(&doc, &nes_scene::PackOptions::compact());
    let t2 = nes_scene::instantiate(&text).unwrap();
    let kids2: Vec<&str> = t2.children(t2.root()).iter().filter_map(|&c| t2.name(c)).collect();
    assert_eq!(kids2, vec!["c0", "c1", "c2"], "重载后兄弟序一致");
    for u in &uids {
        assert!(t2.find_by_uid(u).is_some(), "uid {} 重载命中", u.to_hex());
    }
}
