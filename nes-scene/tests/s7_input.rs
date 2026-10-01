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
        halt_of(&vm, brain).is_some_and(|h| h.contains("未接输入读面")),
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
    struct HeldW(Rc<Cell<bool>>);
    impl nes_scene::InputView for HeldW {
        fn key(&self, name: &str) -> bool {
            name == "W" && self.0.get()
        }
        fn mouse(&self) -> (f32, f32) {
            (0.0, 0.0)
        }
        fn mouse_delta(&self) -> (f32, f32) {
            (0.0, 0.0)
        }
        fn button(&self, _name: &str) -> bool {
            false
        }
        fn text_len(&self) -> usize {
            0
        }
    }
    vm.set_input_view(Rc::new(HeldW(held.clone()))); // 后注入 —— 共享槽，立即生效

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

// ---------------------------------------------------------------- S8.2b-3

/// 测试用全量视图（值由构造给定）。
struct FixedView {
    held: Vec<&'static str>,
    mouse: (f32, f32),
    delta: (f32, f32),
    buttons: Vec<&'static str>,
    text: usize,
}
impl nes_scene::InputView for FixedView {
    fn key(&self, name: &str) -> bool {
        self.held.contains(&name)
    }
    fn mouse(&self) -> (f32, f32) {
        self.mouse
    }
    fn mouse_delta(&self) -> (f32, f32) {
        self.delta
    }
    fn button(&self, name: &str) -> bool {
        self.buttons.contains(&name)
    }
    fn text_len(&self) -> usize {
        self.text
    }
}

/// T-IR-01（S8.2b-3）：输入读面全家 —— mouse_x/y/dx/dy、button、
/// text_len 同帧只读、与 key() 同一注入槽（一条链：快照 -> 视图 ->
/// 内建 -> 脚本；无信号、无隐藏局部中转）。
#[test]
fn t_ir_01_input_view_read_face() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    t.set_prop(
        b,
        "source",
        Value::Str(
            "on \"go\" { mx = mouse_x(); my = mouse_y(); dx = mouse_dx(); dy = mouse_dy(); lb = button(\"left\"); rb = button(\"right\"); tl = text_len(); k = key(\"W\") }"
                .to_string(),
        ),
    )
    .unwrap();
    t.apply_pending();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    vm.set_input_view(Rc::new(FixedView {
        held: vec!["W"],
        mouse: (120.0, 40.0),
        delta: (3.0, -2.0),
        buttons: vec!["left"],
        text: 4,
    }));
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let l = vm.locals(b).unwrap();
    assert_eq!(l.get("mx"), Some(&Value::F32(120.0)));
    assert_eq!(l.get("my"), Some(&Value::F32(40.0)));
    assert_eq!(l.get("dx"), Some(&Value::F32(3.0)));
    assert_eq!(l.get("dy"), Some(&Value::F32(-2.0)));
    assert_eq!(l.get("lb"), Some(&Value::Bool(true)));
    assert_eq!(l.get("rb"), Some(&Value::Bool(false)), "未列举按钮 = 假");
    assert_eq!(l.get("tl"), Some(&Value::I64(4)));
    assert_eq!(l.get("k"), Some(&Value::Bool(true)), "key() 与读面同槽");

    // 读面只读：脚本写不了（无写路径），未接时新内建同样如实停机。
    let mut vm2 = ScriptVm::new();
    assert!(vm2.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm2);
    assert!(
        vm2.locals(b).is_some_and(|l| l.contains_key(nes_scene::HALT_LOCAL)),
        "未接读面停机"
    );
}

/// T-IR-02：读面口径 = 快照口径 —— delta 是**本帧增量**（非累计）、
/// button 是 held、text_len 是当前帧元素数（注入纪律的语义对齐）。
#[test]
fn t_ir_02_view_semantics_align_snapshot() {
    let mut t = SceneTree::new("root");
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    t.set_prop(
        b,
        "source",
        Value::Str("on \"go\" { a = mouse_dx(); b2 = mouse_dy(); t1 = text_len() }".to_string()),
    )
    .unwrap();
    t.apply_pending();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    // 两次注入不同快照读数（共享槽即时生效 —— 同帧一致性由调用时快照定）。
    vm.set_input_view(Rc::new(FixedView {
        held: vec![],
        mouse: (10.0, 10.0),
        delta: (1.0, 1.0),
        buttons: vec![],
        text: 2,
    }));
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    vm.set_input_view(Rc::new(FixedView {
        held: vec![],
        mouse: (14.0, 13.0),
        delta: (4.0, 3.0),
        buttons: vec![],
        text: 0,
    }));
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let l = vm.locals(b).unwrap();
    assert_eq!(l.get("a"), Some(&Value::F32(4.0)), "后一次读数（即时生效）");
    assert_eq!(l.get("b2"), Some(&Value::F32(3.0)));
    assert_eq!(l.get("t1"), Some(&Value::I64(0)));
}
