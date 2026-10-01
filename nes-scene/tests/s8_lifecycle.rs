//! T-LC 契约回归：脚本生命周期状态（S8.0 —— `init` 块）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-LC-01 | `init` 在**首次 process 派发前**执行一次；哨兵 `__initialized` 可观测 |
//! | T-LC-02 | `init` 在**首次信号处理器调用前**执行一次 |
//! | T-LC-03 | 重挂载（热重载）= 局部复位 + **重跑 init**（与 S6.32"换程序不打补丁"同一条语义） |
//! | T-LC-04 | init 与入口同节点共存（一脚本一入口不变 —— init 是块不是入口）；无 init 的脚本零变化；校验口径 |
//! | T-LC-05 | `num_to_str`（S8.2 第一块板）：I64/F32 → 十进制 Str；非数值停机；拼接闭环（HUD 形态） |

use nes_scene::{NodeKind, ScriptVm, SceneTree, Transform2D, Value, INIT_LOCAL};

fn src(t: &mut SceneTree, node: nes_scene::NodeId, text: &str) {
    t.set_prop(node, "source", Value::Str(text.to_string())).unwrap();
}

/// T-LC-01：process 入口 —— init 先于首次 every 执行。
#[test]
fn t_lc_01_init_before_first_process() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    src(
        &mut t,
        b,
        "init { speed = 2.5; x = 10.0 }\nevery { x = x + speed; this.pos = xy(x, 0.0) }",
    );
    t.apply_pending();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());

    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.local(b).unwrap().pos.x, 12.5, "首帧即带 init 出发（10+2.5，写自身）");
    assert!(
        vm.locals(b).is_some_and(|l| l.contains_key(INIT_LOCAL)),
        "哨兵可观测"
    );
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.local(b).unwrap().pos.x, 15.0, "init 只跑一次");
}

/// T-LC-02：信号入口 —— 首次处理器调用前带头跑 init。
#[test]
fn t_lc_02_init_before_first_signal() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    src(&mut t, b, "init { stride = 4.0 }\non \"go\" { sp.pos = xy(arg * stride, 0.0) }");
    t.apply_pending();
    let mut vm = ScriptVm::new();
    let issues = vm.attach_all(&mut t);
    assert!(issues.is_empty(), "{issues:?}");

    t.emit_signal("go", Value::F32(3.0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.local(sp).unwrap().pos.x, 12.0, "首信号已带 init 的 step");
}

/// T-LC-03：热重载（重挂载）—— 局部复位 + init 重跑。
#[test]
fn t_lc_03_remount_reinitializes() {
    let mut t = SceneTree::new("root");
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    src(&mut t, b, "init { n = 100 }\nevery { n = n + 1 }");
    t.apply_pending();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(vm.locals(b).unwrap().get("n"), Some(&Value::I64(102)));

    src(&mut t, b, "init { n = 200 }\nevery { n = n + 1 }");
    let (re, fa) = vm.poll_reloads(&mut t);
    assert_eq!(re, vec![b]);
    assert!(fa.is_empty());
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(vm.locals(b).unwrap().get("n"), Some(&Value::I64(201)), "init 重跑");
}

/// T-LC-04：init 块与入口同脚本共存；无 init 存量零变化；校验口径。
#[test]
fn t_lc_04_init_coexists_and_validation() {
    let mut t = SceneTree::new("root");
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    src(&mut t, b, "every { n = n + 1 }");
    t.apply_pending();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(
        !vm.locals(b).is_some_and(|l| l.contains_key(INIT_LOCAL)),
        "无 init 无哨兵"
    );

    assert!(nes_scene::compile_script("init { x = 1 }").is_err(), "init 不是入口");
    assert!(
        nes_scene::compile_script("every { }\ninit { x = 1 }").is_err(),
        "init 必须在入口前"
    );
}

/// T-LC-05：num_to_str —— Script→Text 闭环（HUD 形态）。
#[test]
fn t_lc_05_num_to_str() {
    let mut t = SceneTree::new("root");
    let hud = t.add_node(t.root(), "hud", NodeKind::Label);
    t.set_prop(hud, "text", Value::Str(String::new())).unwrap();
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    src(
        &mut t,
        b,
        "init { hp = 3 }\non \"damage\" { hp = hp - 1; hud.text = \"HP: \" + num_to_str(hp) + \"/3\" }",
    );
    t.apply_pending();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());

    t.emit_signal("damage", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(
        t.prop(hud, "text"),
        Some(&Value::Str("HP: 2/3".to_string()))
    );
    t.emit_signal("damage", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(
        t.prop(hud, "text"),
        Some(&Value::Str("HP: 1/3".to_string()))
    );

    src(&mut t, b, "on \"go\" { hud.text = num_to_str(1.5) + \"/\" + num_to_str(2) }");
    let _ = vm.poll_reloads(&mut t);
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(
        t.prop(hud, "text"),
        Some(&Value::Str("1.5/2".to_string())),
        "F32 最短往返表示"
    );

    src(&mut t, b, "on \"go\" { hud.text = num_to_str(\"nope\") }");
    let _ = vm.poll_reloads(&mut t);
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(vm.locals(b).is_some_and(|l| l.contains_key(nes_scene::HALT_LOCAL)));
}
