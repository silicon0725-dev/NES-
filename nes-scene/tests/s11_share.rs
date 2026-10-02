//! T-SHARE 契约回归：脚本级只读共享面（S11-0/F-1 → S11-1）。
//!
//! 语义（冻结）：点读 `game.gold` 的取值优先级 = **属性表（schema 键）
//! 优先于 Script 节点局部**；查自己（node == host）时看手中局部（执行期
//! 已被取出，与跨脚本同一语义）；缺名回 I64(0) 不停机；非 Script 节点
//! 无回退。写侧不变：SetProp 走既有属性纪律 —— schema 键落地、未知键
//! 静默丢弃（写错不崩帧）、属主局部只有自己的 SetLocal 能写
//!（无第二状态总线，S8.2b v1.1 裁决）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-SHARE-01 | 跨脚本读：`game.gold` 读别的 Script 节点的局部；process 先跑、信号泵后读，读到当帧终值 |
//! | T-SHARE-02 | 写纪律：跨脚本写落属性表既有纪律（schema 键落地 / 未知键丢弃）；属主局部免疫、计数持续；同名属性遮蔽点读 |
//! | T-SHARE-03 | 缺省与自引用：缺名回 I64(0) 不停机；自引用看手中局部；非 Script 节点维持旧契约 |

use nes_scene::{NodeKind, SceneTree, ScriptVm, Value, HALT_LOCAL};

fn mount(tree: &mut SceneTree, vm: &mut ScriptVm, name: &str, src: &str) -> nes_scene::NodeId {
    let n = tree.add_node(tree.root(), name, NodeKind::Script);
    tree.apply_pending();
    tree.set_prop(n, "source", Value::Str(src.into())).unwrap();
    assert!(vm.attach(tree, n).is_ok(), "{name} 装载");
    n
}

fn local_of(vm: &ScriptVm, node: nes_scene::NodeId, name: &str) -> Value {
    vm.locals(node)
        .and_then(|l| l.get(name).cloned())
        .expect("局部存在")
}

/// T-SHARE-01：跨脚本读 —— ui（信号）读 game（every）的局部 gold。
#[test]
fn t_share_01_cross_script_read_sees_current_frame_value() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let mut vm = ScriptVm::new();
    mount(
        &mut t,
        &mut vm,
        "game",
        "init { gold = 100 }\nevery { gold += 1 }",
    );
    let ui = mount(&mut t, &mut vm, "ui", "on \"query\" { seen = game.gold }");

    // 预发 query；tick 1：process 先跑（init 100 -> 101），泵后 ui 读到 101。
    t.emit_signal("query", Value::I64(0));
    t.tick(0.016, &mut vm);
    assert_eq!(
        local_of(&vm, ui, "seen"),
        Value::I64(101),
        "信号脚本读到 game 当帧终值（process 先、泵后）"
    );

    // 再走 3 帧（102..104）+ 预发；末帧 process 先到 105，泵读到 105。
    t.tick(0.016, &mut vm);
    t.tick(0.016, &mut vm);
    t.tick(0.016, &mut vm);
    t.emit_signal("query", Value::I64(0));
    t.tick(0.016, &mut vm);
    assert_eq!(local_of(&vm, ui, "seen"), Value::I64(105), "持续跟随");
}

/// T-SHARE-02：写纪律 —— 属性平面照旧，属主局部免疫，同名属性遮蔽。
#[test]
fn t_share_02_write_discipline_local_immune() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    t.apply_pending();
    let mut vm = ScriptVm::new();
    let game = mount(
        &mut t,
        &mut vm,
        "game",
        "init { gold = 7 }\nevery { gold += 1 }",
    );
    mount(&mut t, &mut vm, "ui", "on \"poke\" { game.gold = 99 }");
    mount(&mut t, &mut vm, "ui2", "on \"poke2\" { sp.visible = false }");
    let ui3 = mount(&mut t, &mut vm, "ui3", "on \"check\" { seen = game.gold }");

    // 3 帧计数：局部 gold = 10。
    for _ in 0..3 {
        t.tick(0.016, &mut vm);
    }
    // poke：跨脚本写未知键 —— 既有 SetProp 纪律：静默丢弃（写错不崩帧）。
    t.emit_signal("poke", Value::I64(0));
    // poke2：跨脚本写 schema 键（Node2D visible）—— 属性平面照旧可用。
    t.emit_signal("poke2", Value::I64(0));
    t.tick(0.016, &mut vm);
    // tick 4：process 先跑（局部 7+4=11），泵里两个写各自落地/丢弃。
    assert_eq!(
        local_of(&vm, game, "gold"),
        Value::I64(11),
        "属主局部不被跨脚本写穿透"
    );
    assert_eq!(t.prop(game, "gold"), None, "未知键丢弃（不静默进表）");
    assert_eq!(
        t.prop(sp, "visible"),
        Some(&Value::Bool(false)),
        "schema 键跨脚本写照旧落地（属性 = 可写共享平面）"
    );

    // 属主继续计数（局部免疫）；同名属性遮蔽点读（优先级：属性 > 局部）。
    t.set_prop_raw(game, "gold", Value::I64(50));
    t.emit_signal("check", Value::I64(0));
    t.tick(0.016, &mut vm);
    assert_eq!(local_of(&vm, game, "gold"), Value::I64(12), "计数持续");
    assert_eq!(
        local_of(&vm, ui3, "seen"),
        Value::I64(50),
        "同名属性遮蔽点读（属性 > 局部）"
    );
}

/// T-SHARE-03：缺省 / 自引用 / 非 Script 节点。
#[test]
fn t_share_03_defaults_self_reference_and_non_script() {
    let mut t = SceneTree::new("root");
    t.add_node(t.root(), "sp", NodeKind::Node2D);
    t.apply_pending();
    let mut vm = ScriptVm::new();
    let game = mount(
        &mut t,
        &mut vm,
        "game",
        "init { gold = 5 }\nevery { a = game.gold\n b = game.nope }",
    );
    let ui = mount(&mut t, &mut vm, "ui", "on \"q\" { c = sp.nada }");

    // 自引用：game.gold 按名查自己 —— 看手中局部（执行期已被取出，
    // 与跨脚本读同一语义）。缺名 nope -> I64(0)，不停机。
    t.tick(0.016, &mut vm);
    assert_eq!(local_of(&vm, game, "a"), Value::I64(5), "自引用看局部");
    assert_eq!(local_of(&vm, game, "b"), Value::I64(0), "缺名回 I64(0)");
    assert!(
        !vm.locals(game).unwrap().contains_key(HALT_LOCAL),
        "缺名读不停机"
    );

    // 非 Script 节点：无回退，维持旧契约（I64(0)）。
    t.emit_signal("q", Value::I64(0));
    t.tick(0.016, &mut vm);
    assert_eq!(local_of(&vm, ui, "c"), Value::I64(0), "非 Script 节点无回退");
}
