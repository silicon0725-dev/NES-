//! T-TX 契约回归：事务与撤销/重做（S9-2 —— 身份连续性七问）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-TX-01 | Create → Undo → Redo：uid 全程不变 |
//! | T-TX-02 | Modify → Undo → Redo：uid 不变、值回到各版本 |
//! | T-TX-03 | Delete → Undo（原 uid 复活）；期间新建不抢占；Redo/Undo 循环 |
//! | T-TX-04 | 子树删除 → Undo：整子树 uid 集合完全恢复 |
//! | T-TX-05 | Reparent → Undo：uid 不变回原位 |
//! | T-TX-06 | 连续事务 A→B→C 逐级 Undo/Redo：每级 uid 与状态对应 |
//! | T-TX-07 | Undo 后新建对象：与历史死 uid 不冲突（两规则兼容） |

use nes_scene::{
    transaction::{SubtreeSnapshot, TransactionLog, TxCapture},
    NodeKind, SceneTree, Transform2D, Uid, Value,
};

fn snap(tree: &SceneTree, id: nes_scene::NodeId, parent_uid: Uid, at: usize) -> SubtreeSnapshot {
    SubtreeSnapshot::capture(tree, id, parent_uid, at).unwrap()
}

fn node_data_of(tree: &SceneTree, id: nes_scene::NodeId) -> nes_scene::transaction::NodeData2 {
    nes_scene::transaction::NodeData2 {
        uid: tree.uid_of(id).unwrap(),
        kind: tree.kind_tag(id).unwrap().kind().clone(),
        name: tree.name(id).unwrap().to_string(),
        local: tree.local(id).unwrap(),
        process_mode: tree.process_mode(id).unwrap_or_default(),
        props: tree.props(id).cloned().unwrap(),
    }
}

/// T-TX-01：Create → Undo → Redo，uid 不变。
#[test]
fn t_tx_01_create_undo_redo() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let root_uid = t.uid_of(t.root()).unwrap();

    // 事务 1：创建 a。
    let mut log = TransactionLog::new();
    log.begin().unwrap();
    let a = t.add_node(t.root(), "a", NodeKind::Node2D);
    t.apply_pending();
    let s = snap(&t, a, root_uid.clone(), 0);
    log.record(TxCapture::Created { snapshot: s }).unwrap();
    log.commit().unwrap();
    let uid_a = t.uid_of(a).unwrap();
    assert!(t.find_by_uid(&uid_a).is_some());

    // Undo：a 消失。
    assert!(log.undo(&mut t).unwrap());
    assert!(t.find_by_uid(&uid_a).is_none(), "undo 后 a 消失");

    // Redo：a 回来且同 uid。
    assert!(log.redo(&mut t).unwrap());
    let back = t.find_by_uid(&uid_a).expect("redo 后 a 回来");
    assert_eq!(t.name(back), Some("a"));
}

/// T-TX-02：Modify → Undo → Redo，uid 不变 + 值版本对应。
#[test]
fn t_tx_02_modify_undo_redo() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let a = t.add_node(t.root(), "a", NodeKind::Node2D);
    t.set_local(a, Transform2D::from_pos(1.0, 1.0));
    t.apply_pending();
    let uid = t.uid_of(a).unwrap();

    let mut log = TransactionLog::new();
    log.begin().unwrap();
    let before = node_data_of(&t, a);
    t.set_local(a, Transform2D::from_pos(9.0, 9.0));
    t.set_prop(a, "visible", Value::Bool(false)).unwrap();
    let after = node_data_of(&t, a);
    log.record(TxCapture::Modified { uid: uid.clone(), before, after }).unwrap();
    log.commit().unwrap();

    // Undo：回到旧值。
    assert!(log.undo(&mut t).unwrap());
    let id = t.find_by_uid(&uid).unwrap();
    assert_eq!(t.local(id).unwrap().pos.x, 1.0, "undo 回旧 local");
    assert_eq!(t.prop(id, "visible"), Some(&Value::Bool(true)), "undo 回旧属性");

    // Redo：到新值。
    assert!(log.redo(&mut t).unwrap());
    let id = t.find_by_uid(&uid).unwrap();
    assert_eq!(t.local(id).unwrap().pos.x, 9.0);
    assert_eq!(t.prop(id, "visible"), Some(&Value::Bool(false)));
    // uid 全程不变（同一 NodeId 身份经 uid 验证）。
    assert_eq!(t.uid_of(id).unwrap(), uid);
}

/// T-TX-03：Delete → Undo 复活原 uid；期间新建不抢占；Redo/Undo 循环。
#[test]
fn t_tx_03_delete_undo_no_steal() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let root_uid = t.uid_of(t.root()).unwrap();
    let a = t.add_node(t.root(), "a", NodeKind::Node);
    t.apply_pending();
    let uid = t.uid_of(a).unwrap();

    // 事务：删除 a。
    let mut log = TransactionLog::new();
    log.begin().unwrap();
    let s = snap(&t, a, root_uid.clone(), 0);
    t.remove_node(a, false);
    t.apply_pending();
    log.record(TxCapture::Removed { snapshot: s }).unwrap();
    log.commit().unwrap();

    // 删除后：新建 b —— 不得抢占 a 的 uid。
    let b = t.add_node(t.root(), "b", NodeKind::Node);
    t.apply_pending();
    assert_ne!(t.uid_of(b).unwrap(), uid, "新建不抢占死 uid");

    // Undo：a 复活（原 uid）—— 与 b 共存不冲突。
    assert!(log.undo(&mut t).unwrap());
    let back = t.find_by_uid(&uid).expect("undo 复活原 uid");
    assert_eq!(t.name(back), Some("a"));
    assert!(t.find_by_uid(&t.uid_of(b).unwrap()).is_some(), "b 仍在");

    // Redo：a 再删（uid 死亡不被 b 占）→ Undo 再活。
    assert!(log.redo(&mut t).unwrap());
    assert!(t.find_by_uid(&uid).is_none());
    assert!(log.undo(&mut t).unwrap());
    assert!(t.find_by_uid(&uid).is_some(), "再次复活仍原 uid");
}

/// T-TX-04：子树删除 → Undo 整子树 uid 集合完全恢复。
#[test]
fn t_tx_04_subtree_restore() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let root_uid = t.uid_of(t.root()).unwrap();
    // 3 层 5 节点：p > (c1, c2)，c1 > (g1, g2)。
    let p = t.add_node(t.root(), "p", NodeKind::Node);
    let c1 = t.add_node(p, "c1", NodeKind::Node2D);
    let c2 = t.add_node(p, "c2", NodeKind::Node);
    let g1 = t.add_node(c1, "g1", NodeKind::Node2D);
    let g2 = t.add_node(c1, "g2", NodeKind::Node);
    t.set_local(g1, Transform2D::from_pos(5.0, 6.0));
    t.apply_pending();
    let uids: Vec<Uid> = [p, c1, c2, g1, g2].iter().map(|&n| t.uid_of(n).unwrap()).collect();

    let mut log = TransactionLog::new();
    log.begin().unwrap();
    let s = snap(&t, p, root_uid, 0);
    t.remove_node(p, false);
    t.apply_pending();
    log.record(TxCapture::Removed { snapshot: s }).unwrap();
    log.commit().unwrap();
    for u in &uids {
        assert!(t.find_by_uid(u).is_none(), "删除后整子树消失");
    }

    // Undo：整子树 uid 集合完全恢复（含父子关系与数据）。
    assert!(log.undo(&mut t).unwrap());
    for u in &uids {
        assert!(t.find_by_uid(u).is_some(), "uid {:?} 恢复", u.to_hex());
    }
    let g1b = t.find_by_uid(&uids[3]).unwrap();
    assert_eq!(t.local(g1b).unwrap().pos.x, 5.0, "孙辈数据恢复");
    let pb = t.find_by_uid(&uids[0]).unwrap();
    let kids: Vec<&str> = t.children(pb).iter().filter_map(|&c| t.name(c)).collect();
    assert_eq!(kids, vec!["c1", "c2"], "兄弟序恢复");
}

/// T-TX-05：Reparent → Undo 回原位原父。
#[test]
fn t_tx_05_reparent_undo() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let root_uid = t.uid_of(t.root()).unwrap();
    let x = t.add_node(t.root(), "x", NodeKind::Node);
    let y = t.add_node(t.root(), "y", NodeKind::Node);
    let n = t.add_node(x, "n", NodeKind::Node2D);
    t.apply_pending();
    let uid = t.uid_of(n).unwrap();
    let x_uid = t.uid_of(x).unwrap();

    let mut log = TransactionLog::new();
    log.begin().unwrap();
    t.reparent(n, y, None);
    t.apply_pending();
    log.record(TxCapture::Reparented {
        uid: uid.clone(),
        old_parent: x_uid,
        old_at: 0,
        new_parent: t.uid_of(y).unwrap(),
    })
    .unwrap();
    log.commit().unwrap();
    assert_eq!(t.parent(t.find_by_uid(&uid).unwrap()), Some(y));

    // Undo：回原父原位。
    assert!(log.undo(&mut t).unwrap());
    let back = t.find_by_uid(&uid).unwrap();
    assert_eq!(t.parent(back), Some(x), "回原父");
    assert_eq!(t.children(x).len(), 1);
    let _ = root_uid;
}

/// T-TX-06：连续事务逐级 Undo/Redo，每级 uid 与状态对应。
#[test]
fn t_tx_06_multi_step_history() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let root_uid = t.uid_of(t.root()).unwrap();
    let mut log = TransactionLog::new();

    // A：创建 n（pos 1,1）。
    log.begin().unwrap();
    let n = t.add_node(t.root(), "n", NodeKind::Node2D);
    t.set_local(n, Transform2D::from_pos(1.0, 1.0));
    t.apply_pending();
    let uid = t.uid_of(n).unwrap();
    log.record(TxCapture::Created { snapshot: snap(&t, n, root_uid.clone(), 0) }).unwrap();
    log.commit().unwrap();

    // B：改 pos 到 5,5。
    log.begin().unwrap();
    let before = node_data_of(&t, n);
    t.set_local(n, Transform2D::from_pos(5.0, 5.0));
    let after = node_data_of(&t, n);
    log.record(TxCapture::Modified { uid: uid.clone(), before, after }).unwrap();
    log.commit().unwrap();

    // C：删除。
    log.begin().unwrap();
    let s = snap(&t, n, root_uid.clone(), 0);
    t.remove_node(n, false);
    t.apply_pending();
    log.record(TxCapture::Removed { snapshot: s }).unwrap();
    log.commit().unwrap();
    assert!(t.find_by_uid(&uid).is_none(), "末态：已删");

    // Undo C：复活，pos 应为 B 末值 5,5。
    assert!(log.undo(&mut t).unwrap());
    let id = t.find_by_uid(&uid).unwrap();
    assert_eq!(t.local(id).unwrap().pos.x, 5.0, "回到 B 末态");

    // Undo B：pos 1,1。
    assert!(log.undo(&mut t).unwrap());
    assert_eq!(t.local(t.find_by_uid(&uid).unwrap()).unwrap().pos.x, 1.0, "回到 A 末态");

    // Undo A：消失。
    assert!(log.undo(&mut t).unwrap());
    assert!(t.find_by_uid(&uid).is_none(), "回到初态");

    // 逐级 Redo：A → B → C。
    assert!(log.redo(&mut t).unwrap());
    assert_eq!(t.local(t.find_by_uid(&uid).unwrap()).unwrap().pos.x, 1.0);
    assert!(log.redo(&mut t).unwrap());
    assert_eq!(t.local(t.find_by_uid(&uid).unwrap()).unwrap().pos.x, 5.0);
    assert!(log.redo(&mut t).unwrap());
    assert!(t.find_by_uid(&uid).is_none(), "末态再删除");
}

/// T-TX-07：Undo 后新建对象与历史死 uid 不冲突（两规则兼容）。
#[test]
fn t_tx_07_undo_then_create_no_conflict() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let root_uid = t.uid_of(t.root()).unwrap();
    let a = t.add_node(t.root(), "a", NodeKind::Node);
    t.apply_pending();
    let uid_a = t.uid_of(a).unwrap();

    // 删 a → undo 复活 → redo 再删（此刻 a 的 uid 在历史中"死"着）。
    let mut log = TransactionLog::new();
    log.begin().unwrap();
    let s = snap(&t, a, root_uid, 0);
    t.remove_node(a, false);
    t.apply_pending();
    log.record(TxCapture::Removed { snapshot: s }).unwrap();
    log.commit().unwrap();
    assert!(log.undo(&mut t).unwrap());
    assert!(log.redo(&mut t).unwrap());

    // 新建 b：与历史死 uid 无冲突（add_node 随机 v4 本就不占；显式
    // add_node_with_uid(uid_a) 才会冲突 —— 两规则兼容的完整验证）。
    let b = t.add_node(t.root(), "b", NodeKind::Node);
    t.apply_pending();
    assert_ne!(t.uid_of(b).unwrap(), uid_a);
    // 显式复用死 uid：机械上 uid 空闲可建（无墓碑表），但与事务历史
    // 冲突 —— **undo 时身份冲突被如实检测**（不做静默双身份）。
    let steal = t.add_node_with_uid(t.root(), "steal", NodeKind::Node, uid_a.clone()).unwrap();
    t.apply_pending();
    assert!(log.undo(&mut t).is_err(), "ad-hoc 复用后 undo 如实报身份冲突");
    // 移除复用者 → undo 恢复正常（原 uid 与 b 共存）。
    t.remove_node(steal, false);
    t.apply_pending();
    assert!(log.undo(&mut t).unwrap());
    assert!(t.find_by_uid(&uid_a).is_some(), "合法恢复与 b 共存");
}
