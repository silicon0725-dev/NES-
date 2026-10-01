//! T-Cmp 契约回归：文本脚本语法与编译（S6.20）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Cmp-01 | 信号脚本文本 == 手写 Op 行为等价（T-VM-01 场景）：编译产物逐指令相等 + 运行像素级一致 |
//! | T-Cmp-02 | process 入口 + 局部 + if：T-VM-02 场景的文本等价（帧间计数，第 3 帧分支移动+发射） |
//! | T-Cmp-03 | 属性读写（节点.属性 = 字面量 / 读属性做条件）|
//! | T-Cmp-04 | 优先级与字面量：`1 + 2 * 3` == 7、整数/F32/Vec2/字符串/布尔字面量 |
//! | T-Cmp-05 | 语法错误如实报错（行定位、保留字、非法字符、结构缺失） |

use nes_scene::{
    compile_script, NodeKind, Op, SceneTree, ScriptEntry, ScriptVm, Value, HALT_LOCAL,
};

fn tree2d() -> (SceneTree, nes_scene::NodeId, nes_scene::NodeId) {
    let mut t = SceneTree::new("root");
    let sprite = t.add_node(t.root(), "sprite", NodeKind::Node2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    (t, sprite, brain)
}

/// T-Cmp-01：文本编译产物与手写 Op **逐指令相等**，且行为等价（T-VM-01）。
#[test]
fn t_cmp_01_text_equals_handwritten_ops() {
    let src = r#"
on "go" {
    sprite.pos = sprite.pos + arg
}
"#;
    let script = compile_script(src).expect("编译");
    assert_eq!(script.entry, ScriptEntry::Signal("go".into()));
    assert_eq!(
        script.ops,
        vec![
            Op::NodeByName("sprite".into()),
            Op::NodeByName("sprite".into()),
            Op::GetT,
            Op::Arg,
            Op::Add,
            Op::SetT,
        ],
        "赋值目标节点先压 + RHS 双压：栈序按构造正确"
    );

    // 行为等价：同 T-VM-01。
    let (mut t, sprite, brain) = tree2d();
    t.set_prop(brain, "registry_key", Value::Str("mover".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register("mover", script);
    assert!(vm.attach(&mut t, brain).is_ok());
    t.emit_signal("go", Value::Vec2(nes_scene::Vec2::new(16.0, 0.0)));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sprite).unwrap().pos.x, 16.0, "同帧移动");
    t.emit_signal("go", Value::Vec2(nes_scene::Vec2::new(16.0, 0.0)));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sprite).unwrap().pos.x, 32.0, "累计");
}

/// T-Cmp-02：process 入口 + 局部 + if（T-VM-02 文本等价）。
#[test]
fn t_cmp_02_process_if_and_locals() {
    let src = r#"
every {
    n = n + 1
    if 2 < n {
        this.pos = this.pos + (8.0, 0.0)
        emit "done" n
    }
}
"#;
    let (mut t, _sprite, brain) = tree2d();
    t.set_prop(brain, "registry_key", Value::Str("counter".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("counter", src).expect("编译+登记");
    assert!(vm.attach(&mut t, brain).is_ok());

    t.tick(0.016, &mut vm);
    t.tick(0.016, &mut vm);
    assert_eq!(t.local(brain).unwrap().pos.x, 0.0, "前两帧只计数");
    assert_eq!(vm.locals(brain).unwrap().get("n"), Some(&Value::I64(2)));

    let stats = t.tick(0.016, &mut vm);
    assert_eq!(t.local(brain).unwrap().pos.x, 8.0, "第 3 帧分支移动");
    assert!(stats.signals_delivered >= 1, "done 入泵");
    t.tick(0.016, &mut vm);
    assert_eq!(t.local(brain).unwrap().pos.x, 16.0, "每帧分支");
}

/// T-Cmp-03：属性读写 —— 赋值字面量、读属性做条件。
#[test]
fn t_cmp_03_property_read_write() {
    let src = r#"
on "cfg" {
    hero.flip_h = true
    if hero.z_index == 0 {
        hero.z_index = 5
    }
}
"#;
    // 属性走 schema：flip_h/z_index 在 Sprite2D 的表里（Node2D 没有 flip_h）。
    let mut t = SceneTree::new("root");
    let sprite = t.add_node(t.root(), "hero", NodeKind::Sprite2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("cfg".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("cfg", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.emit_signal("cfg", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(
        t.prop(sprite, "flip_h"),
        Some(&Value::Bool(true)),
        "属性写入"
    );
    assert_eq!(t.prop(sprite, "z_index"), Some(&Value::I64(5)), "读属性条件分支后写入");
}

/// T-Cmp-04：优先级与字面量族。
#[test]
fn t_cmp_04_precedence_and_literals() {
    // `1 + 2 * 3 == 7`（乘先于加）驱动一次可观测的移动。
    let src = r#"
on "m" {
    a = 1 + 2 * 3
    if a == 7 {
        sprite.pos = (1.5, -2.0)
        label.text = "done"
        hero.flip_v = true
    }
}
"#;
    let (mut t, sprite, brain) = tree2d();
    let hero = t.add_node(t.root(), "hero", nes_scene::NodeKind::Sprite2D);
    let label = t.add_node(t.root(), "label", nes_scene::NodeKind::Label);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("math".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("math", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.emit_signal("m", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    let p = t.local(sprite).unwrap().pos;
    assert!((p.x - 1.5).abs() < 1e-6 && (p.y + 2.0).abs() < 1e-6, "Vec2 字面量 + 条件成立");
    assert_eq!(t.prop(label, "text"), Some(&Value::Str("done".into())), "字符串属性（schema 内）");
    assert_eq!(t.prop(hero, "flip_v"), Some(&Value::Bool(true)), "布尔属性（Sprite2D schema）");
}

/// T-Cmp-05：语法错误如实报错（行定位与原因）。
#[test]
fn t_cmp_05_syntax_errors_reported() {
    // ① 非法字符。
    let e1 = compile_script("on \"x\" { a = 1 # 2 }").expect_err("非法字符");
    assert!(e1.to_string().contains("非法字符"), "{e1}");

    // ② 保留字做局部名（行定位）。
    let e2 = compile_script("on \"x\" {\n    arg = 1\n}").expect_err("保留字");
    let msg2 = format!("{e2}");
    assert!(msg2.contains("保留字"), "{msg2}");
    assert!(e2.line >= 2, "行定位：{:?}", e2.line);

    // ③ 结构缺失：缺 `{`。
    let e3 = compile_script("on \"x\" a = 1 }").expect_err("缺 {");
    assert!(format!("{e3}").contains("期望 `{`"), "{e3}");

    // ④ 入口写法错。
    let e4 = compile_script("when \"x\" { }").expect_err("入口");
    assert!(format!("{e4}").contains("on"), "{e4}");

    // ⑤ 字符串未闭合。
    let e5 = compile_script("on \"x\" { a = \"open }").expect_err("字符串");
    assert!(format!("{e5}").contains("引号"), "{e5}");
    let _ = HALT_LOCAL;
}

// ---------------------------------------------------------------- S6.21
// else / while / 逻辑运算符（&& || !，按值 eager）+ 比较五族组合编译。

/// T-Cmp-06：if-else 双分支 + else if 链。
#[test]
fn t_cmp_06_else_and_else_if_chain() {
    // 分支标记：pick = 1/2/3，由 v 的值决定；三轮信号驱动到三个分支。
    let src = r#"
on "pick" {
    if v == 1 {
        r = 10
    } else if v == 2 {
        r = 20
    } else {
        r = 30
    }
}
"#;
    let mut t = SceneTree::new("root");
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("chain".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("chain", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());

    for (v, expect) in [(1, 10), (2, 20), (9, 30)] {
        let mut locals = vm.locals(brain).unwrap_or_default();
        locals.insert("v".into(), Value::I64(v));
        // 直接预置局部：借 states 不可 —— 用第二轮发射前注入的可行法：
        // 简化：重挂载带初始局部。
        let _ = locals;
        // 用脚本侧不改，重写 v：把 v 当初始局部经重挂载注入。
        drop(locals);
        // 重新登记带 v 的脚本并重挂载（重挂载复位局部 = 注入路径）。
        let mut seeded = compile_script(src).unwrap();
        seeded.locals.insert("v".into(), Value::I64(v));
        vm.register("chain", seeded);
        assert!(vm.attach(&mut t, brain).is_ok(), "重挂载（v={v}）");
        t.emit_signal("pick", Value::I64(0));
        t.tick(0.016, &mut nes_scene::NoObserver);
        assert_eq!(
            vm.locals(brain).unwrap().get("r"),
            Some(&Value::I64(expect)),
            "v={v} -> r={expect}"
        );
    }
}

/// T-Cmp-07：while 累加；死循环由步数上限兜底（不挂帧、__halt 可观测）。
#[test]
fn t_cmp_07_while_and_infinite_loop_guard() {
    let src = r#"
every {
    i = 0
    s = 0
    while i < 4 {
        i = i + 1
        s = s + i
    }
    d = s * 2
}
"#;
    let mut t = SceneTree::new("root");
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("acc".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("acc", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.tick(0.016, &mut vm);
    assert_eq!(vm.locals(brain).unwrap().get("s"), Some(&Value::I64(10)), "1+2+3+4");
    assert_eq!(vm.locals(brain).unwrap().get("i"), Some(&Value::I64(4)), "循环退出条件");
    assert_eq!(vm.locals(brain).unwrap().get("d"), Some(&Value::I64(20)), "循环后置计算");

    // 死循环：while true { } -> 步数上限停机。
    let bad = r#"
every {
    while true {
        n = n + 1
    }
}
"#;
    let mut t2 = SceneTree::new("root");
    let brain2 = t2.add_node(t2.root(), "b", NodeKind::Script);
    t2.apply_pending();
    t2.set_prop(brain2, "registry_key", Value::Str("loop".into())).unwrap();
    let mut vm2 = ScriptVm::new();
    vm2.register_text("loop", bad).expect("编译");
    assert!(vm2.attach(&mut t2, brain2).is_ok());
    t2.tick(0.016, &mut vm2); // 不挂起即通过
    assert_eq!(
        vm2.locals(brain2).unwrap().get(HALT_LOCAL),
        Some(&Value::Str("step budget exceeded".into())),
        "死循环步数兜底"
    );
}

/// T-Cmp-08：逻辑运算符（&& || !，按值 eager）组合条件。
#[test]
fn t_cmp_08_logical_operators() {
    let src = r#"
on "go" {
    if a == 1 && b == 2 {
        sprite.pos = sprite.pos + (4.0, 0.0)
    }
    if a == 9 || b == 2 {
        buddy.pos = buddy.pos + (8.0, 0.0)
    }
    if !(a == 1) {
        sprite.pos = sprite.pos + (16.0, 0.0)
    }
}
"#;
    let (mut t, sprite, brain) = tree2d();
    let buddy = t.add_node(t.root(), "buddy", NodeKind::Node2D);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("logic".into())).unwrap();
    let mut seeded = compile_script(src).unwrap();
    seeded.locals.insert("a".into(), Value::I64(1));
    seeded.locals.insert("b".into(), Value::I64(2));
    let mut vm = ScriptVm::new();
    vm.register("logic", seeded);
    assert!(vm.attach(&mut t, brain).is_ok());
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    // 三条件写**不同节点**：同一脚本内多次写同一节点是"末写覆盖"（Cmd 按
    // 回调批次落地，脚本内 GetT 读陈值 —— 引擎既有批次语义，见 S6.21 文档）。
    assert_eq!(t.local(sprite).unwrap().pos.x, 4.0, "&& 命中（! 未命中不叠加）");
    assert_eq!(t.local(buddy).unwrap().pos.x, 8.0, "|| 命中");
}

/// T-Cmp-09：比较五族组合编译（产物断言 + 行为抽查）。
#[test]
fn t_cmp_09_comparison_family_composition() {
    // 产物断言：`a >= b` -> [a, b, Lt, Not]；`a > b` -> [b, a, Lt]。
    let ge = compile_script("on \"x\" { if a >= b { } }").unwrap();
    assert_eq!(
        ge.ops,
        vec![
            Op::Local("a".into()),
            Op::Local("b".into()),
            Op::Lt,
            Op::Not,
            Op::JumpIfNot(5),
        ]
    );
    let gt = compile_script("on \"x\" { if a > b { } }").unwrap();
    assert_eq!(
        gt.ops,
        vec![
            Op::Local("b".into()),
            Op::Local("a".into()),
            Op::Lt,
            Op::JumpIfNot(4),
        ],
        "交换族：b 在前"
    );

    // 行为抽查：s = 0; if 3 != 3 { s = 1 }; if 2 <= 2 { s = s + 5 }; if 3 >= 4 { s = 100 }
    let src = r#"
on "c" {
    s = 0
    if 3 != 3 {
        s = 1
    }
    if 2 <= 2 {
        s = s + 5
    }
    if 3 >= 4 {
        s = 100
    }
}
"#;
    let mut t = SceneTree::new("root");
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("cmp".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("cmp", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.emit_signal("c", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(
        vm.locals(brain).unwrap().get("s"),
        Some(&Value::I64(5)),
        "!= 假、<= 真（+5）、>= 假"
    );
}
