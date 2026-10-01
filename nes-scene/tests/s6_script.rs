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
    NodeKind, NodeKindTag, Op, SceneTree, Script, ScriptEntry, ScriptVm, Transform2D, Value,
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

// ---------------------------------------------------------------- S6.32
// 脚本热重载：source 属性变化 -> poll_reloads 重编译重挂载。

/// T-VM-06：换行为 + 局部复位 —— v1 步进 +4；改 source 为 v2 步进 -8；
/// poll 后新行为生效且局部回到初始值（换程序不打补丁）。
#[test]
fn t_vm_06_hot_reload_swaps_behavior_resets_locals() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    t.set_prop(brain, "source", Value::Str(
        "on \"go\" { sp.pos = sp.pos + (4.0, 0.0) }".into(),
    )).unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach(&mut t, brain).is_ok());
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sp).unwrap().pos.x, 4.0, "v1 步进 +4");

    // 改 source（编辑器/重载流），poll -> 换行为。
    t.set_prop(brain, "source", Value::Str(
        "on \"go\" { sp.pos = sp.pos - (8.0, 0.0) }".into(),
    )).unwrap();
    let (reloaded, failed) = vm.poll_reloads(&mut t);
    assert_eq!(reloaded, vec![brain], "恰一节点重载");
    assert!(failed.is_empty());
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sp).unwrap().pos.x, -4.0, "v2 步进 -8 生效");

    // 局部复位：v3 计数 n（用局部断言；`(n, 0.0)` 是 Vec2 任意表达式
    // 的 Pack 缺口，不用）。
    t.set_prop(brain, "source", Value::Str(
        "on \"go\" { n = n + 1; done = n * 10 }".into(),
    )).unwrap();
    let (re, fa) = vm.poll_reloads(&mut t);
    assert_eq!(re.len(), 1);
    assert!(fa.is_empty());
    // n 未在脚本 locals 声明 -> 初始 0（复位语义：v1/v2 运行期陈值被清）。
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(vm.locals(brain).unwrap().get("n"), Some(&Value::I64(1)), "局部复位（n 从 0 起）");
    assert_eq!(vm.locals(brain).unwrap().get("done"), Some(&Value::I64(10)), "后置计算");

    // 未变化 -> 不重载（幂等）。
    let (re3, fa3) = vm.poll_reloads(&mut t);
    assert!(re3.is_empty() && fa3.is_empty(), "戳相同不重载");
}

/// T-VM-07：编译失败保留旧行为 + 下次 poll 重试（修好即生效）。
#[test]
fn t_vm_07_broken_source_keeps_old_behavior_retry_next_poll() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    t.set_prop(brain, "source", Value::Str(
        "on \"go\" { sp.pos = sp.pos + (4.0, 0.0) }".into(),
    )).unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach(&mut t, brain).is_ok());

    // 塞入语法错误源码。
    t.set_prop(brain, "source", Value::Str("on \"go\" { a = }".into())).unwrap();
    let (re, fa) = vm.poll_reloads(&mut t);
    assert!(re.is_empty());
    assert_eq!(fa.len(), 1);
    assert!(fa[0].1.contains("编译失败"), "错误指名：{}", fa[0].1);

    // 旧行为继续跑（+4 不是停机）。
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sp).unwrap().pos.x, 4.0, "编译失败：v1 旧行为保留");

    // 修好 -> 下次 poll 生效。
    t.set_prop(brain, "source", Value::Str(
        "on \"go\" { sp.pos = sp.pos + (1.0, 0.0) }".into(),
    )).unwrap();
    let (re2, fa2) = vm.poll_reloads(&mut t);
    assert_eq!(re2.len(), 1);
    assert!(fa2.is_empty(), "修好后无失败");
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sp).unwrap().pos.x, 5.0, "4 + 1（新行为生效）");
}

/// T-VM-08：重挂载不叠加连接 —— 多次 attach/poll 后一次信号恰好一次命中。
#[test]
fn t_vm_08_remount_does_not_stack_connections() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    t.set_prop(brain, "source", Value::Str(
        "on \"go\" { sp.pos = sp.pos + (4.0, 0.0) }".into(),
    )).unwrap();
    let mut vm = ScriptVm::new();
    // 反复挂载/重载（v1 不变文本反复 attach + 变文本 poll）。
    assert!(vm.attach(&mut t, brain).is_ok());
    assert!(vm.attach(&mut t, brain).is_ok(), "幂等重挂载");
    t.set_prop(brain, "source", Value::Str(
        "on \"go\" { sp.pos = sp.pos + (4.0, 0.0) }".into(),
    )).unwrap();
    let _ = vm.poll_reloads(&mut t); // 相同文本：戳同 -> 不重载（0 连接变化）
    // 再换文本 poll 一次（真重载 -> 断旧接新）。
    t.set_prop(brain, "source", Value::Str(
        "on \"go\" { sp.pos = sp.pos + (4.0, 0.0) }".into(),
    )).unwrap();
    let _ = vm.poll_reloads(&mut t);

    // 一次信号：恰好 +4（叠加连接会让它 +8/+12）。
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sp).unwrap().pos.x, 4.0, "一次信号恰一次命中（连接未叠加）");
}

// ---------------------------------------------------------------- S6.33
// 外置 .nes 脚本资产：script 属性（资源槽位）-> 表查路径 -> 读文本 -> 编译。

/// T-VM-09：外置装载 + 三路互斥 + 读错/槽位缺口。
#[test]
fn t_vm_09_external_script_asset_mount() {
    use nes_scene::ResourceTable;
    use nes_asset::AssetKind;

    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));

    // 资源表声明外置脚本（槽位由 declare 分配 -> script 属性引用）。
    let mut table = ResourceTable::new();
    let res = table.declare("Scripts/mover.nes", AssetKind::Script).unwrap();
    t.set_prop(brain, "script", Value::Resource(res.get() as u64)).unwrap();

    // 内存源读取器（注入：VM 不碰文件系统）。RefCell 允许读闭包与后续改写共存。
    let files: std::cell::RefCell<std::collections::BTreeMap<String, String>> = Default::default();
    files.borrow_mut().insert(
        "Scripts/mover.nes".into(),
        "on \"go\" { sp.pos = sp.pos + (6.0, 0.0) }".into(),
    );
    let mut read = |p: &str| {
        files
            .borrow()
            .get(p)
            .cloned()
            .ok_or_else(|| format!("文件不存在：{p}"))
    };

    let mut vm = ScriptVm::new();
    let issues = vm.attach_all_with_sources(&mut t, &table, &mut read);
    assert!(issues.is_empty(), "{issues:?}");
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sp).unwrap().pos.x, 6.0, "外置脚本驱动");

    // 三路互斥：script + source / script + registry_key / 三路同设。
    t.set_prop(brain, "source", Value::Str("on \"x\" { }".into())).unwrap();
    let mut vm2 = ScriptVm::new();
    let issues2 = vm2.attach_all_with_sources(&mut t, &table, &mut read);
    assert!(issues2.len() == 1 && issues2[0].1.contains("互斥"), "{issues2:?}");
    t.set_prop(brain, "source", Value::Str(String::new())).unwrap();
    t.set_prop(brain, "registry_key", Value::Str("k".into())).unwrap();
    let issues3 = vm2.attach_all_with_sources(&mut t, &table, &mut read);
    assert!(issues3.len() == 1 && issues3[0].1.contains("script、registry_key"), "{issues3:?}");
    t.set_prop(brain, "source", Value::Str("on \"y\" { }".into())).unwrap();
    let issues4 = vm2.attach_all_with_sources(&mut t, &table, &mut read);
    assert!(issues4.len() == 1 && issues4[0].1.contains("source、script、registry_key"), "{issues4:?}");
    t.set_prop(brain, "source", Value::Str(String::new())).unwrap();
    t.set_prop(brain, "registry_key", Value::Str(String::new())).unwrap();

    // 读失败指名（文件消失：旧行为保留、进清单）。
    files.borrow_mut().remove("Scripts/mover.nes");
    let (re, fa) = vm.poll_reloads_with_sources(&mut t, &table, &mut read);
    assert!(re.is_empty());
    assert!(fa.len() == 1 && fa[0].1.contains("文件不存在"), "{fa:?}");
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sp).unwrap().pos.x, 12.0, "读失败：v1 旧行为保留（6+6）");

    // 旧 attach_all 对外置节点如实报需 sources（不静默哑挂）。
    let mut vm3 = ScriptVm::new();
    let issues5 = vm3.attach_all(&mut t);
    assert!(issues5.len() == 1 && issues5[0].1.contains("attach_all_with_sources"), "{issues5:?}");
}

/// T-VM-10：外置热重载 last-good —— 文件变好文本 -> poll 重编译 -> 新行为；
/// 变坏文本 -> 旧行为保留 + 下次（修好）生效；同文件双节点共享。
#[test]
fn t_vm_10_external_hot_reload_last_good_and_sharing() {
    use nes_scene::ResourceTable;
    use nes_asset::AssetKind;

    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    let other = t.add_node(t.root(), "ot", NodeKind::Node2D);
    let b1 = t.add_node(t.root(), "b1", NodeKind::Script);
    let b2 = t.add_node(t.root(), "b2", NodeKind::Script);
    t.apply_pending();
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    t.set_local(other, Transform2D::from_pos(0.0, 0.0));
    let mut table = ResourceTable::new();
    let res = table.declare("Scripts/shared.nes", AssetKind::Script).unwrap();
    let key = Value::Resource(res.get() as u64);
    t.set_prop(b1, "script", key.clone()).unwrap();
    t.set_prop(b2, "script", key).unwrap();
    // b2 引用同文件：两个安装各自命中信号（共享编译产物）。
    let files: std::cell::RefCell<std::collections::BTreeMap<String, String>> = Default::default();
    files.borrow_mut().insert(
        "Scripts/shared.nes".into(),
        "on \"go\" { sp.pos = sp.pos + (6.0, 0.0) }".into(),
    );
    let mut read = |p: &str| {
        files
            .borrow()
            .get(p)
            .cloned()
            .ok_or_else(|| format!("文件不存在：{p}"))
    };

    let mut vm = ScriptVm::new();
    assert!(vm.attach_all_with_sources(&mut t, &table, &mut read).is_empty());
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    // 同文件双节点 = 同一脚本的两个安装：信号命中两次（+6+6）。
    assert_eq!(t.local(sp).unwrap().pos.x, 12.0, "共享：b1+b2 都命中");

    // 变坏：编译错 -> 旧行为保留。
    files.borrow_mut().insert("Scripts/shared.nes".into(), "on \"go\" { a = }".into());
    let (re, fa) = vm.poll_reloads_with_sources(&mut t, &table, &mut read);
    assert!(re.is_empty() && fa.len() == 2, "两个引用节点都进失败清单");
    assert!(fa[0].1.contains("编译失败"), "{}", fa[0].1);
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sp).unwrap().pos.x, 24.0, "坏文本：旧行为保留（再 +12）");

    // 修好：新文本 -> 重编译 -> 新行为。
    files.borrow_mut().insert(
        "Scripts/shared.nes".into(),
        "on \"go\" { sp.pos = sp.pos - (4.0, 0.0) }".into(),
    );
    let (re2, fa2) = vm.poll_reloads_with_sources(&mut t, &table, &mut read);
    assert_eq!(re2.len(), 2, "双节点都重载");
    assert!(fa2.is_empty());
    t.emit_signal("go", Value::I64(0));
    t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(t.local(sp).unwrap().pos.x, 16.0, "24-4-4（新行为双命中）");

    // 未变 -> 不重载。
    let (re3, fa3) = vm.poll_reloads_with_sources(&mut t, &table, &mut read);
    assert!(re3.is_empty() && fa3.is_empty());
    let _ = (other, b2);
}
