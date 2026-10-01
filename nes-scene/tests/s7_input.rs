//! T-In-VM 契约回归：脚本输入探针（S7.2 —— `key("名")` 内建）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-In-VM-01 | `key(..)` 文法编译为 `Op::Key`；未注入探针时**停机**（`__halt` 指名"未接输入探针"），不装恒假 |
//! | T-In-VM-02 | 注入探针后 `key("W")` 按探针求值（真驱移动、假不动）；attach 后注入也立即生效（共享槽） |

use std::cell::Cell;
use std::rc::Rc;

use nes_scene::{NodeKind, ScriptVm, SceneTree, Transform2D, Value, HALT_LOCAL};

fn halt_of(vm: &ScriptVm, node: nes_scene::NodeId) -> Option<String> {
    vm.locals(node).and_then(|l| {
        l.get(HALT_LOCAL).map(|v| match v {
            Value::Str(s) => s.clone(),
            other => format!("{other:?}"),
        })
    })
}

/// T-In-VM-01：未接探针 = 诚实停机（"没接就是没有"，不返回假）。
#[test]
fn t_in_vm_01_probe_unset_halts() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.set_prop(
        brain,
        "source",
        Value::Str("on \"go\" { if key(\"W\") { sp.pos += (1.0, 0.0) } }".into()),
    )
    .unwrap();
    t.apply_pending();

    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(
        halt_of(&vm, brain).is_some_and(|h| h.contains("未接输入探针")),
        "停机指名：{:?}",
        halt_of(&vm, brain)
    );
    assert_eq!(t.local(sp).unwrap().pos.x, 0.0, "未动");
}

/// T-In-VM-02：探针求值 + attach 后注入立即生效。
#[test]
fn t_in_vm_02_probe_drives_script() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.set_prop(
        brain,
        "source",
        Value::Str("on \"go\" { if key(\"W\") { sp.pos += (2.0, 0.0) } }".into()),
    )
    .unwrap();
    t.apply_pending();

    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty()); // 先装载（处理器闭包已捕获槽）

    let held = Rc::new(Cell::new(false));
    vm.set_key_probe(Rc::new({
        let held = held.clone();
        move |name: &str| name == "W" && held.get()
    })); // 后注入 —— 共享槽，立即生效

    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.local(sp).unwrap().pos.x, 0.0, "W 未按住：不动");
    assert!(halt_of(&vm, brain).is_none(), "探针在，无停机");

    held.set(true);
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.local(sp).unwrap().pos.x, 2.0, "W 按住：移动");

    // 未列举键名 = 假（探针自己的口径，与快照 is_down 一致）。
    held.set(true);
    let brain2 = t.add_node(t.root(), "brain2", NodeKind::Script);
    t.set_prop(
        brain2,
        "source",
        Value::Str("on \"go\" { if key(\"NoSuchKey\") { sp.pos += (100.0, 0.0) } }".into()),
    )
    .unwrap();
    t.apply_pending();
    let issues = vm.attach_all(&mut t);
    assert!(issues.is_empty(), "{issues:?}");
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.local(sp).unwrap().pos.x, 4.0, "未列举键不触发（只在 W 探针真时 +2）");
}
