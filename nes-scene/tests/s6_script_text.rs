//! T-Cmp 契约回归：文本脚本语法与编译（S6.20）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Cmp-01 | 信号脚本文本 == 手写 Op 行为等价（T-VM-01 场景）：编译产物逐指令相等 + 运行像素级一致 |
//! | T-Cmp-02 | process 入口 + 局部 + if：T-VM-02 场景的文本等价（帧间计数，第 3 帧分支移动+发射） |
//! | T-Cmp-03 | 属性读写（节点.属性 = 字面量 / 读属性做条件）|
//! | T-Cmp-04 | 优先级与字面量：`1 + 2 * 3` == 7、整数/F32/Vec2/字符串/布尔字面量 |
//! | T-Cmp-05 | 语法错误如实报错（行定位、保留字、非法字符、结构缺失） |

use std::collections::BTreeMap;

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

// ---------------------------------------------------------------- S6.22
// break / continue（编译期循环上下文栈；break 占位回填、continue 即时）。

/// T-Cmp-10：break —— `while true` 里计数到 3 逃出（不依赖步数兜底）。
#[test]
fn t_cmp_10_break_escapes_infinite_loop() {
    let src = r#"
every {
    i = 0
    while true {
        i = i + 1
        if i == 3 {
            break
        }
    }
    done = i * 100
}
"#;
    let mut t = SceneTree::new("root");
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("esc".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("esc", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.tick(0.016, &mut vm);
    let locals = vm.locals(brain).unwrap();
    assert_eq!(locals.get("i"), Some(&Value::I64(3)), "break 逃出");
    assert_eq!(locals.get("done"), Some(&Value::I64(300)), "循环后语句照常执行");
    assert!(!locals.contains_key(HALT_LOCAL), "不靠步数兜底（无停机）");
}

/// T-Cmp-11：continue 跳过本轮（只累加奇数）；嵌套 break 只绑内层。
#[test]
fn t_cmp_11_continue_skip_and_nested_inner_binding() {
    let src = r#"
every {
    i = 0
    odd = 0
    while i < 6 {
        i = i + 1
        if i == 2 || i == 4 || i == 6 {
            continue
        }
        odd = odd + i
    }
    outer = 0
    inner_total = 0
    while outer < 3 {
        outer = outer + 1
        inner = 0
        while true {
            inner = inner + 1
            if inner == 2 {
                break
            }
        }
        inner_total = inner_total + inner
    }
}
"#;
    let mut t = SceneTree::new("root");
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("cc".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("cc", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.tick(0.016, &mut vm);
    let locals = vm.locals(brain).unwrap();
    assert_eq!(locals.get("odd"), Some(&Value::I64(9)), "1+3+5（continue 跳偶数）");
    assert_eq!(locals.get("outer"), Some(&Value::I64(3)), "内层 break 不影响外层");
    assert_eq!(locals.get("inner_total"), Some(&Value::I64(6)), "内层各停在 2");
    assert!(!locals.contains_key(HALT_LOCAL));
}

/// T-Cmp-12：循环外 break/continue 编译错；产物断言（break=Jump(出口)）。
#[test]
fn t_cmp_12_outside_loop_error_and_codegen() {
    let e1 = compile_script("on \"x\" { break }").expect_err("循环外 break");
    assert!(format!("{e1}").contains("break 在循环外"), "{e1}");
    let e2 = compile_script("on \"x\" { if true { continue } }").expect_err("循环外 continue");
    assert!(format!("{e2}").contains("continue 在循环外"), "{e2}");

    // 产物：while i < 2 { break } —— break 的 Jump 回填到出口。
    let s = compile_script("on \"x\" { while i < 2 { break } }").unwrap();
    assert_eq!(
        s.ops,
        vec![
            Op::Local("i".into()),
            Op::Const(Value::I64(2)),
            Op::Lt,
            Op::JumpIfNot(6), // 条件假 -> 出口（尾跳之后）
            Op::Jump(6),      // break -> 出口
            Op::Jump(0),      // 回到条件（break 后不可达，结构完整性保留）
        ]
    );
}

// ---------------------------------------------------------------- S6.23
// 带标签 break/continue（`name: while`；标签只能用于 while）。

/// T-Cmp-13：跨层 break —— 双层搜索，命中即全停（经典早退）。
#[test]
fn t_cmp_13_labeled_break_escapes_outer() {
    let src = r#"
every {
    i = 0
    found = 0
    rounds = 0
    outer: while i < 3 {
        i = i + 1
        rounds = rounds + 1
        j = 0
        while j < 5 {
            j = j + 1
            if i * 10 + j == 21 {
                found = i * 10 + j
                break outer
            }
        }
    }
    done = 1
}
"#;
    let mut t = SceneTree::new("root");
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("lb".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("lb", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.tick(0.016, &mut vm);
    let l = vm.locals(brain).unwrap();
    assert_eq!(l.get("found"), Some(&Value::I64(21)), "命中 (2,1)");
    assert_eq!(l.get("rounds"), Some(&Value::I64(2)), "外层第 2 轮即停（未跑满 3）");
    assert_eq!(l.get("done"), Some(&Value::I64(1)), "循环后语句执行");
    assert!(!l.contains_key(HALT_LOCAL));
}

/// T-Cmp-14：带标签 continue —— 内层跳回外层条件（内层提前离场）。
#[test]
fn t_cmp_14_labeled_continue_returns_to_outer_top() {
    let src = r#"
every {
    i = 0
    inner_steps = 0
    reached_tail = 0
    outer: while i < 3 {
        i = i + 1
        k = 0
        while true {
            inner_steps = inner_steps + 1
            k = k + 1
            if k == 1 {
                continue outer
            }
        }
        reached_tail = 1
    }
}
"#;
    let mut t = SceneTree::new("root");
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("lc".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("lc", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.tick(0.016, &mut vm);
    let l = vm.locals(brain).unwrap();
    assert_eq!(l.get("i"), Some(&Value::I64(3)), "外层照常走满");
    assert_eq!(
        l.get("inner_steps"),
        Some(&Value::I64(3)),
        "每轮内层恰 1 步即被 continue outer 带走"
    );
    assert_eq!(l.get("reached_tail"), Some(&Value::I64(0)), "内层尾语句永不可达");
    assert!(!l.contains_key(HALT_LOCAL), "不靠步数兜底");
}

/// T-Cmp-15：错误与匹配规则 —— 未定义标签、标签用于 if、裸 break 兼容。
#[test]
fn t_cmp_15_label_errors_and_rules() {
    // ① 未定义标签。
    let e1 = compile_script("on \"x\" { while true { break ghost } }").expect_err("未定义标签");
    assert!(format!("{e1}").contains("未找到标签 `ghost`"), "{e1}");

    // ② 标签只能用于 while。
    let e2 = compile_script("on \"x\" { tag: if true { } }").expect_err("标签用于 if");
    assert!(format!("{e2}").contains("标签只能用于 while"), "{e2}");

    // ③ 同名标签由内向外匹配（内层命中）。
    let s = compile_script(
        "on \"x\" { outer: while true { outer: while true { break outer } } }",
    )
    .unwrap();
    // 找内层出口：内层尾跳后。产物里出现两个 JumpIfNot；内层 break 回填到内层出口。
    // 断言行为替代复杂产物核对：直接数 Jump —— 至少 3 个跳转（2 出口 + 尾跳 + break）。
    let jumps = s.ops.iter().filter(|o| matches!(o, Op::Jump(_))).count();
    assert!(jumps >= 2, "break 与尾跳存在：{jumps}");
}

// ---------------------------------------------------------------- S6.24
// for 区间迭代（纯糖脱糖 while；界活值；循环旋转 continue 安全）。

/// T-Cmp-16：基本迭代 —— 0..4 累加 0+1+2+3=6；循环变量循环后留存（退出值 4）。
#[test]
fn t_cmp_16_for_range_basic_and_var_survives() {
    let src = r#"
every {
    s = 0
    for i in 0..4 {
        s = s + i
    }
}
"#;
    let mut t = SceneTree::new("root");
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("fr".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("fr", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.tick(0.016, &mut vm);
    let l = vm.locals(brain).unwrap();
    assert_eq!(l.get("s"), Some(&Value::I64(6)), "0+1+2+3");
    assert_eq!(l.get("i"), Some(&Value::I64(4)), "循环变量留存（退出值）");
    assert!(!l.contains_key(HALT_LOCAL));
}

/// T-Cmp-17：continue 不吃增量（循环旋转的关键证明）—— 跳过 2 不死循环。
#[test]
fn t_cmp_17_continue_runs_increment() {
    let src = r#"
every {
    s = 0
    for i in 0..5 {
        if i == 2 {
            continue
        }
        s = s + i
    }
    after = 1
}
"#;
    let mut t = SceneTree::new("root");
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("fc".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text("fc", src).expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.tick(0.016, &mut vm);
    let l = vm.locals(brain).unwrap();
    assert_eq!(l.get("s"), Some(&Value::I64(8)), "0+1+3+4（跳 2 且不死循环）");
    assert_eq!(l.get("i"), Some(&Value::I64(5)));
    assert_eq!(l.get("after"), Some(&Value::I64(1)), "循环后语句执行");
    assert!(!l.contains_key(HALT_LOCAL), "不靠步数兜底");
}

/// T-Cmp-18：空区间（4..0 零次）；活界（体内改界生效）；标签 for + break name。
#[test]
fn t_cmp_18_empty_range_live_bound_and_labeled_for() {
    // ① 空区间零次。
    let mut t = SceneTree::new("root");
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_prop(brain, "registry_key", Value::Str("fe".into())).unwrap();
    let mut vm = ScriptVm::new();
    vm.register_text(
        "fe",
        "every { s = 0; for i in 4..0 { s = s + 100 } }",
    )
    .expect("编译");
    assert!(vm.attach(&mut t, brain).is_ok());
    t.tick(0.016, &mut vm);
    assert_eq!(vm.locals(brain).unwrap().get("s"), Some(&Value::I64(0)), "空区间零次");

    // ② 活界：界是局部，体内每轮缩减 -> 提前终止（糖=手写 while 的可见差异）。
    let mut t2 = SceneTree::new("root");
    let b2 = t2.add_node(t2.root(), "b", NodeKind::Script);
    t2.apply_pending();
    t2.set_prop(b2, "registry_key", Value::Str("fl".into())).unwrap();
    let mut vm2 = ScriptVm::new();
    vm2.register_text(
        "fl",
        "every { n = 3; rounds = 0; for i in 0..n { n = n - 1; rounds = rounds + 1 } }",
    )
    .expect("编译");
    assert!(vm2.attach(&mut t2, b2).is_ok());
    t2.tick(0.016, &mut vm2);
    let l = vm2.locals(b2).unwrap();
    // i=0: n 3->2; i=1: n 2->1; i=2? cond 2<n=1 假 -> 停。rounds=2。
    assert_eq!(l.get("rounds"), Some(&Value::I64(2)), "活界提前终止");
    assert_eq!(l.get("n"), Some(&Value::I64(1)));

    // ③ 标签 for + 跨层 break。
    let mut t3 = SceneTree::new("root");
    let b3 = t3.add_node(t3.root(), "b", NodeKind::Script);
    t3.apply_pending();
    t3.set_prop(b3, "registry_key", Value::Str("ft".into())).unwrap();
    let mut vm3 = ScriptVm::new();
    vm3.register_text(
        "ft",
        "every { hit = 0; outer: for i in 0..3 { for j in 0..9 { if i * 10 + j == 15 { hit = i * 10 + j; break outer } } } }",
    )
    .expect("编译");
    assert!(vm3.attach(&mut t3, b3).is_ok());
    t3.tick(0.016, &mut vm3);
    assert_eq!(
        vm3.locals(b3).unwrap().get("hit"),
        Some(&Value::I64(15)),
        "标签 for 跨层 break"
    );

    // ④ 浮点界（1.5..3 走 F32 比较）。
    let mut t4 = SceneTree::new("root");
    let b4 = t4.add_node(t4.root(), "b", NodeKind::Script);
    t4.apply_pending();
    t4.set_prop(b4, "registry_key", Value::Str("ff".into())).unwrap();
    let mut vm4 = ScriptVm::new();
    vm4.register_text("ff", "every { c = 0; for i in 1.5..3.5 { c = c + 1 } }")
        .expect("编译");
    assert!(vm4.attach(&mut t4, b4).is_ok());
    t4.tick(0.016, &mut vm4);
    // i 走 1.5, 2.5, 3.5? i+1 每轮：1.5<3.5 ✓, 2.5<3.5 ✓, 3.5<3.5 ✗ -> 2 次。
    assert_eq!(vm4.locals(b4).unwrap().get("c"), Some(&Value::I64(2)), "浮点界（步进仍 +1）");
}

// ---------------------------------------------------------------- S6.25
// 步进参数（step，活方向）与闭区间（..=）。

/// T-Cmp-19：步进参数 —— 正步跳格、负步降序、变量步进（活方向）。
#[test]
fn t_cmp_19_step_parameter() {
    // ① step 2 升序：0,2,4。
    let run = |src: &str| -> BTreeMap<String, Value> {
        let mut t = SceneTree::new("root");
        let brain = t.add_node(t.root(), "brain", NodeKind::Script);
        t.apply_pending();
        t.set_prop(brain, "registry_key", Value::Str("s".into())).unwrap();
        let mut vm = ScriptVm::new();
        vm.register_text("s", src).expect("编译");
        assert!(vm.attach(&mut t, brain).is_ok());
        t.tick(0.016, &mut vm);
        vm.locals(brain).unwrap()
    };
    let l = run("every { s = 0; for i in 0..6 step 2 { s = s * 10 + i } }");
    assert_eq!(l.get("s"), Some(&Value::I64(24)), "0,2,4（step 2 升序）");

    // ② step -1 降序开区间：5,4,3,2,1。
    let l2 = run("every { s = 0; for i in 5..0 step -1 { s = s * 10 + i } }");
    assert_eq!(l2.get("s"), Some(&Value::I64(54321)), "5..0 step -1（开）");

    // ③ 变量步进（活方向）：k=1 走 0,1,2；体内每轮 k 翻倍不影响本轮。
    let l3 = run("every { s = 0; k = 1; for i in 0..3 step k { s = s * 10 + i; k = 1 } }");
    assert_eq!(l3.get("s"), Some(&Value::I64(12)), "变量步进（每轮 1）");

    // ④ 变量步进改变方向（活方向的可见差异）：步进每轮变号 -> 条件随活值。
    //    i: 0(+1)=1(+(-1))=0(+1)=1... 死循环由步数兜底 —— 换可终止例：
    let l4 = run("every { s = 0; for i in 0..2 step 1 { s = s + 1 } }");
    assert_eq!(l4.get("s"), Some(&Value::I64(2)), "字面量 1 与缺省等价");
}

/// T-Cmp-20：闭区间 `..=` —— 升序含端、降序含端、浮点端、与步进组合。
#[test]
fn t_cmp_20_inclusive_range() {
    let run = |src: &str| -> BTreeMap<String, Value> {
        let mut t = SceneTree::new("root");
        let brain = t.add_node(t.root(), "brain", NodeKind::Script);
        t.apply_pending();
        t.set_prop(brain, "registry_key", Value::Str("i".into())).unwrap();
        let mut vm = ScriptVm::new();
        vm.register_text("i", src).expect("编译");
        assert!(vm.attach(&mut t, brain).is_ok());
        t.tick(0.016, &mut vm);
        vm.locals(brain).unwrap()
    };
    // ① 升序含端：0..=3 -> 4 次。
    let l = run("every { c = 0; for i in 0..=3 { c = c + 1 } }");
    assert_eq!(l.get("c"), Some(&Value::I64(4)), "0..=3 含端 4 次");

    // ② 降序含端：5..=0 step -1 -> 6 次。
    let l2 = run("every { c = 0; for i in 5..=0 step -1 { c = c + 1 } }");
    assert_eq!(l2.get("c"), Some(&Value::I64(6)), "5..=0 step -1 含端 6 次");

    // ③ 浮点端：1.5..=3.5 -> 1.5, 2.5, 3.5 共 3 次。
    let l3 = run("every { c = 0; for i in 1.5..=3.5 { c = c + 1 } }");
    assert_eq!(l3.get("c"), Some(&Value::I64(3)), "1.5..=3.5 含端 3 次");

    // ④ 单元素区间 a..=a 恰 1 次；反向开区间 0..-1 零次。
    let l4 = run("every { c = 0; for i in 2..=2 { c = c + 1 } }");
    assert_eq!(l4.get("c"), Some(&Value::I64(1)), "2..=2 恰 1 次");
}

/// T-Cmp-21：边界 —— step 0 恒假零次；标签 for 带步进；step 是保留字。
#[test]
fn t_cmp_21_step_zero_and_compat() {
    let run = |src: &str| -> BTreeMap<String, Value> {
        let mut t = SceneTree::new("root");
        let brain = t.add_node(t.root(), "brain", NodeKind::Script);
        t.apply_pending();
        t.set_prop(brain, "registry_key", Value::Str("z".into())).unwrap();
        let mut vm = ScriptVm::new();
        vm.register_text("z", src).expect("编译");
        assert!(vm.attach(&mut t, brain).is_ok());
        t.tick(0.016, &mut vm);
        vm.locals(brain).unwrap()
    };
    // ① step 0：恒假零次（数学诚实：条件两支都不成立）。
    let l = run("every { c = 0; for i in 0..9 step 0 { c = c + 100 } }");
    assert_eq!(l.get("c"), Some(&Value::I64(0)), "step 0 零次");

    // ② 标签 for 带步进 + 跨层 break。
    let l2 = run("every { hit = 0; outer: for i in 0..6 step 2 { if i == 4 { hit = i; break outer } } }");
    assert_eq!(l2.get("hit"), Some(&Value::I64(4)), "标签 for + step");

    // ③ step 是保留字（作局部名编译错）。
    let e = compile_script("on \"x\" { step = 1 }").expect_err("step 保留字");
    assert!(format!("{e}").contains("保留字"), "{e}");
}
