//! T-VM 契约回归：脚本 VM（S6.19）—— `Script` 节点 + registry_key 装载 +
//! 栈式字节码双入口（信号跨节点 / process 自身）+ 停机保护。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-VM-01 | 信号脚本跨节点：attach 后命中信号即运行，SetT 移动**其他节点**同帧生效；载荷经 Arg 入栈 |
//! | T-VM-02 | process 脚本：VM 作观察者逐帧驱动；局部跨调用持久（计数器）；条件跳转 + Emit |
//! | T-VM-03 | 停机保护：类型错/节点找不到/死循环 —— 不崩帧、`__halt` 可观测、步数上限截断 |
//! | T-VM-04 | attach_all：registry_key 装载；坏键进缺口清单不挡其他节点；重挂载复位局部 |
//! | T-VM-05 | process 入口纪律：脚本试图写**别的节点** -> 停机记录（NodeCtx 纪律不可绕） |

use nes_scene::{
    NodeKind, NodeKindTag, Op, SceneTree, Script, ScriptEntry, ScriptVm, Value,
    HALT_LOCAL, SCRIPT_MAX_STEPS,
};

fn tree_with_script_node() -> (SceneTree, nes_scene::NodeId, nes_scene::NodeId) {
    let mut t = SceneTree::new("root");
    let sprite = t.add_node(t.root(), "sprite", NodeKind::Node2D);
    let script_node = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    (t, sprite, script_node)
}

/// T-VM-01：信号脚本跨节点 —— "go" 命中 -> sprite 移动 dx（载荷 Vec2）。
#[test]
fn t_vm_01_signal_script_moves_other_node_same_frame() {
    let (mut t, sprite, script_node) = tree_with_script_node();
    t.set_prop(script_node, "registry_key", Value::Str("mover".into()))
        .unwrap();

    let mut vm = ScriptVm::new();
    // 脚本：NodeByName("sprite"); GetT; Arg; Add; NodeByName("sprite"); SetT
    //（栈序：SetT 弹值、弹节点 —— 节点要在值下面，倒序压）。
    vm.register(
        "mover",
        Script::new(
            ScriptEntry::Signal("go".into()),
            vec![
                // 栈序：GetT 消费节点 —— 节点压两次（一次留给 SetT 弹）。
                Op::NodeByName("sprite".into()),
                Op::NodeByName("sprite".into()),
                Op::GetT,
                Op::Arg,
                Op::Add,
                Op::SetT,
            ],
        ),
    );
    assert!(vm.attach(&mut t, script_node).is_ok(), "装载");

    // 宿主预发：载荷 (16,0)；tick -> 泵 -> 方法连接 -> 闭包 -> Cmd 落地 -> 冲洗。
    t.emit_signal("go", Value::Vec2(nes_scene::Vec2::new(16.0, 0.0)));
    let stats = t.tick(0.016, &mut nes_scene::NoObserver);
    assert!(stats.signals_routed >= 1, "方法连接路由");
    assert_eq!(t.local(sprite).unwrap().pos.x, 16.0, "同帧移动");
    assert_eq!(t.world_position(sprite).unwrap().x, 16.0, "同帧冲洗");

    // 再发一次：状态无关（无局部），累计移动。
    t.emit_signal("go", Value::Vec2(nes_scene::Vec2::new(16.0, 0.0)));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sprite).unwrap().pos.x, 32.0, "第二次移动");
    assert!(vm.locals(script_node).map_or(true, |l| !l.contains_key(HALT_LOCAL)), "无停机");
}

/// T-VM-02：process 脚本 —— VM 作观察者逐帧驱动；局部持久；条件跳转。
#[test]
fn t_vm_02_process_script_persistent_locals_and_branch() {
    let (mut t, _sprite, script_node) = tree_with_script_node();
    // process 脚本挂在自己（Script 节点）上：n = n+1；n<3 -> 跳回不动；
    // n>=3 -> 自身右移 8 并发 "done"。
    // 指令：0 Local(n) 1 Const(1) 2 Add 3 SetLocal(n) 4 Local(n) 5 Const(3)
    //       6 Lt 7 JumpIfNot(9) 8 Jump(18)  （n<3 -> 本帧结束；n>=3 落到 This）
    //       10 This 11 GetT 12 Const(Vec2(8,0)) 13 Add 14 This 15 SetT
    //       16 Const(I64(1)) 17 Emit("done")
    let script = Script::new(
        ScriptEntry::Process,
        vec![
            Op::Local("n".into()),
            Op::Const(Value::I64(1)),
            Op::Add,
            Op::SetLocal("n".into()),
            Op::Local("n".into()),
            Op::Const(Value::I64(3)),
            Op::Lt,
            Op::JumpIfNot(9), // n>=3 -> 落到 This（下标 9）
            Op::Jump(18), // n<3 -> 本帧结束（帧间计数，非帧内循环）
            Op::This,
            Op::This, // GetT 消费节点：自身压两次
            Op::GetT,
            Op::Const(Value::Vec2(nes_scene::Vec2::new(8.0, 0.0))),
            Op::Add,
            Op::SetT,
            Op::Const(Value::I64(1)),
            Op::Emit("done".into()),
        ],
    );
    let mut vm = ScriptVm::new();
    vm.register("counter", script);
    // 挂载前先设键。
    t.set_prop(script_node, "registry_key", Value::Str("counter".into()))
        .unwrap();
    assert!(vm.attach(&mut t, script_node).is_ok());

    // 帧 1/2：计数，不动（Script 节点位置不变）。
    t.tick(0.016, &mut vm);
    t.tick(0.016, &mut vm);
    assert_eq!(t.local(script_node).unwrap().pos.x, 0.0);
    assert_eq!(
        vm.locals(script_node).unwrap().get("n"),
        Some(&Value::I64(2)),
        "局部跨调用持久"
    );

    // 帧 3：n=3 -> 不再跳回 -> 自身移动 + 发 done。
    let stats = t.tick(0.016, &mut vm);
    assert_eq!(t.local(script_node).unwrap().pos.x, 8.0, "条件分支生效");
    assert!(stats.signals_delivered >= 1, "done 已入泵（广播）");
    assert_eq!(
        vm.locals(script_node).unwrap().get("n"),
        Some(&Value::I64(3))
    );

    // 帧 4：n=4 -> 又回循环（n<3 假 -> 落到 10）再动一次。
    t.tick(0.016, &mut vm);
    assert_eq!(t.local(script_node).unwrap().pos.x, 16.0, "每帧分支");
}

/// T-VM-03：停机保护 —— 类型错 / 节点找不到 / 死循环。
#[test]
fn t_vm_03_halt_protection() {
    let (mut t, _sprite, script_node) = tree_with_script_node();
    t.set_prop(script_node, "registry_key", Value::Str("bad".into()))
        .unwrap();
    let mut vm = ScriptVm::new();

    // ① 节点找不到。
    vm.register(
        "bad",
        Script::new(
            ScriptEntry::Signal("x".into()),
            vec![Op::NodeByName("ghost".into()), Op::GetT],
        ),
    );
    assert!(vm.attach(&mut t, script_node).is_ok());
    t.emit_signal("x", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    let locals = vm.locals(script_node).unwrap();
    assert_eq!(
        locals.get(HALT_LOCAL),
        Some(&Value::Str("node not found".into())),
        "① 停机可观测"
    );

    // ② 死循环：Jump 自环 -> 步数上限截断，不挂帧。
    t.set_prop(script_node, "registry_key", Value::Str("loop".into()))
        .unwrap();
    vm.register(
        "loop",
        Script::new(
            ScriptEntry::Signal("y".into()),
            vec![Op::Jump(0)],
        ),
    );
    assert!(vm.attach(&mut t, script_node).is_ok(), "重挂载（① 的处理器被替换）");
    t.emit_signal("y", Value::I64(0));
    let stats = t.tick(0.016, &mut nes_scene::NoObserver); // 不挂起即通过
    assert!(stats.signals_delivered >= 1, "运行发生");
    assert_eq!(
        vm.locals(script_node).unwrap().get(HALT_LOCAL),
        Some(&Value::Str("step budget exceeded".into())),
        "② 步数上限"
    );

    // ③ 类型错：Bool + I64 不可加（Str+Str 自 S6.26 合法 —— 拼接）。
    t.set_prop(script_node, "registry_key", Value::Str("ty".into()))
        .unwrap();
    vm.register(
        "ty",
        Script::new(
            ScriptEntry::Signal("z".into()),
            vec![
                Op::Const(Value::Bool(true)),
                Op::Const(Value::I64(1)),
                Op::Add,
            ],
        ),
    );
    assert!(vm.attach(&mut t, script_node).is_ok());
    t.emit_signal("z", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(
        vm.locals(script_node).unwrap().get(HALT_LOCAL),
        Some(&Value::Str("Add 类型不符".into())),
        "③ 类型停机"
    );
}

/// T-VM-04：attach_all —— registry_key 批量装载；坏键进缺口清单不挡好键；
/// 重挂载复位局部。
#[test]
fn t_vm_04_attach_all_reports_gaps() {
    let mut t = SceneTree::new("root");
    let good = t.add_node(t.root(), "good", NodeKind::Script);
    let bad_key = t.add_node(t.root(), "badkey", NodeKind::Script);
    let _empty = t.add_node(t.root(), "empty", NodeKind::Script); // 键留空
    t.apply_pending();
    t.set_prop(good, "registry_key", Value::Str("ok".into())).unwrap();
    t.set_prop(bad_key, "registry_key", Value::Str("missing".into())).unwrap();
    // empty：键留空。

    let mut vm = ScriptVm::new();
    vm.register(
        "ok",
        Script::new(ScriptEntry::Signal("ping".into()), vec![]), // 空脚本：命中即成功返回
    );
    let issues = vm.attach_all(&mut t);
    assert_eq!(issues.len(), 2, "两个缺口：{issues:?}");
    let reasons: Vec<String> = issues.iter().map(|(_, r)| r.clone()).collect();
    assert!(reasons.iter().any(|r| r.contains("missing")), "未知键：{reasons:?}");
    assert!(reasons.iter().any(|r| r.contains("空")), "空键：{reasons:?}");

    // 好键照常工作。
    t.emit_signal("ping", Value::I64(0));
    let stats = t.tick(0.016, &mut nes_scene::NoObserver);
    assert!(stats.signals_routed >= 1, "好键不受坏键影响");

    // 重挂载复位：局部 n=9 挂载 -> locals 未见（attach 注入初始）；带初始局部验证复位。
    t.set_prop(good, "registry_key", Value::Str("seeded".into())).unwrap();
    let mut seeded = Script::new(ScriptEntry::Signal("ping".into()), vec![]);
    seeded.locals.insert("n".into(), Value::I64(9));
    vm.register("seeded", seeded);
    assert!(vm.attach(&mut t, good).is_ok());
    assert_eq!(vm.locals(good).unwrap().get("n"), Some(&Value::I64(9)), "初始局部注入");
}

/// T-VM-05：process 入口纪律 —— 写别的节点 -> 停机记录（不可绕）。
#[test]
fn t_vm_05_process_cannot_write_other_nodes() {
    let (mut t, sprite, script_node) = tree_with_script_node();
    t.set_prop(script_node, "registry_key", Value::Str("selfish".into()))
        .unwrap();
    let mut vm = ScriptVm::new();
    vm.register(
        "selfish",
        Script::new(
            ScriptEntry::Process,
            vec![
                Op::NodeByName("sprite".into()),
                Op::NodeByName("sprite".into()),
                Op::GetT,
                Op::Const(Value::Vec2(nes_scene::Vec2::new(9.0, 9.0))),
                Op::Add,
                Op::SetT,
            ],
        ),
    );
    assert!(vm.attach(&mut t, script_node).is_ok());
    t.tick(0.016, &mut vm);
    assert_eq!(
        vm.locals(script_node).unwrap().get(HALT_LOCAL),
        Some(&Value::Str("process 入口只许写自身".into())),
        "跨节点写被拒（NodeCtx 纪律）"
    );
    assert_eq!(t.local(sprite).unwrap().pos.x, 0.0, "sprite 未被动");
    let _ = NodeKindTag::Script; // 引用一下避免未用告警
    let _ = SCRIPT_MAX_STEPS;
}
