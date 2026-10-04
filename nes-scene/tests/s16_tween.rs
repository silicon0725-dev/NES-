//! S16 动画补间第 1 期 · 场景层契约回归：位置补间成为引擎一等公民。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-TW-01 | `tween_pos` 登记 -> 每 tick 推进（中点插值精确）-> 时满落位终值 + 注册表移除 |
//! | T-TW-02 | last-wins：进行中再发，从**当前实际位置**起新程（不跳变、不叠加）；`tween_stop` 停在当前值 |
//! | T-TW-03 | 确定性：同轨迹双跑指纹逐位相同；含补间与不含的指纹必不同；目标节点删除 -> 补间自动清（NodeHandle resolve 失败） |
//! | T-TW-04 | 解析面：`tween_pos` / `tween_stop` 语法、字面量 ms <= 0 解析期报错、目标不存在停机记录、运行时非法 ms 树侧拒收 |
//! | T-TW-05 | 推进阶段时序：补间推进后、process 前 —— 同一 tick 内脚本读到新位置（先推进后脚本；时满落位同帧可读终值） |
//!
//! 补间是**游戏可见状态**（推进在 `SceneTree::tick` 专属阶段：结构落地后、
//! enter/process 前，每 tick 直写 local 并进语义指纹）；序列化面是**会话态**
//!（不进 RON 往返 —— 补间不在 `NodeData`，`to_doc` 天然不携带）。

use nes_scene::{
    compile_script, scene_fingerprint, NodeId, NodeKind, Op, SceneTree, ScriptEntry, ScriptVm,
    Uid, Value, Vec2, HALT_LOCAL,
};

/// 搭一棵最小树：root + 方块（Node2D）+ 脚本节点。结构即刻落地。
fn tree2d() -> (SceneTree, NodeId, NodeId) {
    let mut t = SceneTree::new("root");
    let sprite = t.add_node(t.root(), "box", NodeKind::Node2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    (t, sprite, brain)
}

/// 追加一个信号入口脚本节点（即刻落地）。
fn add_signal_script(t: &mut SceneTree, name: &str, signal: &str, src: &str) -> NodeId {
    let n = t.add_node(t.root(), name, NodeKind::Script);
    t.apply_pending();
    t.set_prop(n, "source", Value::Str(src.to_string())).unwrap();
    let _ = signal;
    n
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// T-TW-01：单程补间的完整生命周期 —— 登记（from = 发射时当前位置）->
/// 每 tick 推进（中点插值精确）-> 时满落位终值 + 注册表移除 -> 之后不再移动。
#[test]
fn t_tw_01_tween_registers_advances_and_lands() {
    let (mut t, box_node, brain) = tree2d();
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_pos "box" 100.0 50.0 1000 }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());

    // 发射 + 首 tick：Cmd 在信号泵落地（登记时 elapsed = 0，位置未动）。
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens().len(), 1, "补间已登记");
    assert_eq!(t.tweens()[0].from, Vec2::new(0.0, 0.0), "from = 当前实际位置");
    assert_eq!(t.tweens()[0].to, Vec2::new(100.0, 50.0));
    assert_eq!(t.tweens()[0].duration_ms, 1000.0);
    assert_eq!(t.tweens()[0].elapsed_ms, 0.0);

    // 单 tick 推进 500ms -> t = 0.5 -> 中点 (50, 25)（f32 精确）。
    t.tick(0.5, &mut vm);
    assert_eq!(t.local(box_node).unwrap().pos, Vec2::new(50.0, 25.0), "中点插值");
    assert_eq!(t.tweens()[0].elapsed_ms, 500.0);

    // 时满落位：再推 500ms -> t = 1 -> 精确终值 + 注册表移除。
    t.tick(0.5, &mut vm);
    assert_eq!(t.local(box_node).unwrap().pos, Vec2::new(100.0, 50.0), "落位终值");
    assert!(t.tweens().is_empty(), "时满移除登记");

    // 注册表已空：额外 tick 零行为（位置保持，无补间可推进）。
    t.tick(1.0, &mut vm);
    assert_eq!(t.local(box_node).unwrap().pos, Vec2::new(100.0, 50.0));
}

/// T-TW-02：last-wins（进行中再发 -> 从当前实际位置起新程，注册表不叠加、
/// 不跳变）+ `tween_stop`（位置停在当前值）。
#[test]
fn t_tw_02_last_wins_and_stop_hold_position() {
    let (mut t, box_node, brain) = tree2d();
    let s2 = add_signal_script(
        &mut t,
        "redo_brain",
        "redo",
        r#"on "redo" { tween_pos "box" 0.0 80.0 400 }"#,
    );
    let s3 = add_signal_script(&mut t, "halt_brain", "halt", r#"on "halt" { tween_stop "box" }"#);
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_pos "box" 200.0 0.0 1000 }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    let _ = (s2, s3);

    // 登记 + 推进 250ms -> x = 50。
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    t.tick(0.25, &mut vm);
    assert!(approx(t.local(box_node).unwrap().pos.x, 50.0));

    // 进行中再发（last-wins）：本 tick 先推进旧程（1.75 阶段），信号泵后落地
    // 新程 —— from 采样 = 含本帧推进的当前实际位置（落地时点），注册表长度
    // 仍 1（替换非叠加）。发射前的位置快照只用来证明推进发生过（50 -> 53.33）。
    t.emit_signal("redo", Value::I64(0));
    let pos_before = t.local(box_node).unwrap().pos;
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens().len(), 1, "last-wins：替换而非叠加");
    let tw = t.tweens()[0].clone();
    assert_eq!(tw.to, Vec2::new(0.0, 80.0));
    assert_eq!(tw.duration_ms, 400.0);
    let pos_after = t.local(box_node).unwrap().pos;
    assert!(
        approx(tw.from.x, pos_after.x) && approx(tw.from.y, pos_after.y),
        "登记不挪节点：from = 落地时当前实际位置（不跳变）from={:?} pos={:?}",
        tw.from,
        pos_after
    );
    assert!(
        tw.from.x > pos_before.x && tw.from.x > 50.0,
        "from 含本帧推进（53.33 一类），不是旧起点 0：from={:?} before={:?}",
        tw.from,
        pos_before
    );

    // 新程推进 200ms -> t = 0.5 -> 中点 (26.67, 40)（53.33 与 0 的中点）。
    t.tick(0.2, &mut vm);
    let p = t.local(box_node).unwrap().pos;
    assert!(approx(p.x, 26.667) && approx(p.y, 40.0), "新程中点：{:?}", p);

    // tween_stop：本 tick 先推进（1.75 阶段）后停（信号泵）—— 停止之后
    // 位置定格：下一 tick 零推进、注册表恒空。
    t.emit_signal("halt", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(t.tweens().is_empty(), "stop 移除登记");
    let p = t.local(box_node).unwrap().pos;
    t.tick(1.0, &mut vm);
    let p2 = t.local(box_node).unwrap().pos;
    assert!(
        approx(p.x, p2.x) && approx(p.y, p2.y),
        "停在当前值：{:?} vs {:?}",
        p,
        p2
    );
}

/// T-TW-03：确定性 —— 同轨迹双跑逐帧指纹逐位相同；含补间与不含的指纹必不同
///（条件混入只在登记表非空时生效）；目标节点删除 -> 补间自动清。
#[test]
fn t_tw_03_determinism_and_dead_target_cleanup() {
    // --- 同轨迹双跑 + 含/不含对照组（uid 钉成确定性派生身份）。
    let run = |emit_go: bool| -> Vec<u64> {
        let (mut t, _box, brain) = tree2d();
        for (i, n) in t.preorder().into_iter().enumerate() {
            t.set_uid(n, Uid::derive_legacy(&format!("/p{i}"))).unwrap();
        }
        t.set_prop(
            brain,
            "source",
            Value::Str(r#"on "go" { tween_pos "box" 120.0 60.0 500 }"#.into()),
        )
        .unwrap();
        let mut vm = ScriptVm::new();
        assert!(vm.attach_all(&mut t).is_empty());
        if emit_go {
            t.emit_signal("go", Value::I64(0));
        }
        let mut hashes = Vec::new();
        for _ in 0..5 {
            let _ = t.tick(1.0 / 60.0, &mut vm);
            hashes.push(scene_fingerprint(&t, Some(&vm)));
        }
        hashes
    };
    let a = run(true);
    let b = run(true);
    assert_eq!(a, b, "同轨迹两跑：逐帧指纹逐位相同");
    let quiet = run(false);
    assert_ne!(
        a, quiet,
        "含补间与不含的指纹必不同（条件混入：登记表非空才摺进）"
    );

    // --- 目标节点删除 -> 补间自动清（NodeHandle resolve 失败即移除）。
    let (mut t, box_node, brain) = tree2d();
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_pos "box" 90.0 0.0 1000 }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens().len(), 1);
    // 删除目标（结构变更下一帧帧首落地）；落地后的推进阶段 resolve 失败即清。
    t.remove_node(box_node, false);
    assert!(!t.tweens().is_empty(), "落地前登记仍在");
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(!t.contains(box_node), "节点已销毁");
    assert!(t.tweens().is_empty(), "死节点补间自动清");
}

/// T-TW-04：解析面 —— 编译产物形状、字面量 ms <= 0 解析期报错（含一元负号）、
/// 保留字、目标不存在停机记录、运行时非法 ms 树侧拒收。
#[test]
fn t_tw_04_parse_surface() {
    // 编译产物：x/y/ms 三表达式 + TweenPos（压序 = 源序）。
    let script = compile_script(r#"every { tween_pos "box" 32 48 500 }"#).expect("编译");
    assert_eq!(script.entry, ScriptEntry::Process);
    assert_eq!(
        script.ops,
        vec![
            Op::Const(Value::I64(32)),
            Op::Const(Value::I64(48)),
            Op::Const(Value::I64(500)),
            Op::TweenPos { name: "box".into() },
        ]
    );
    // tween_stop：零栈交互（照 play 形态）。
    let script = compile_script(r#"every { tween_stop "box" }"#).expect("编译");
    assert_eq!(script.ops, vec![Op::TweenStop { name: "box".into() }]);

    // 字面量 ms <= 0：解析期报错（0 / 负整数 / 0.0 / 一元负号形态）。
    assert!(compile_script(r#"every { tween_pos "box" 0 0 0 }"#).is_err());
    assert!(compile_script(r#"every { tween_pos "box" 0 0 -5 }"#).is_err());
    assert!(compile_script(r#"every { tween_pos "box" 0 0 0.0 }"#).is_err());
    // 保留字：不得作变量名。
    assert!(compile_script(r#"every { tween_pos = 1 }"#).is_err());
    assert!(compile_script(r#"every { tween_stop = 1 }"#).is_err());
    assert!(compile_script(r#"every { x = tween_pos }"#).is_err());

    // 目标不存在：停机记录（照 NodeByName 既有纪律），注册表零登记。
    let (mut t, _box, brain) = tree2d();
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_pos "ghost" 1 2 100 }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let halt = vm.locals(brain).unwrap_or_default().get(HALT_LOCAL).cloned();
    assert!(
        matches!(halt, Some(Value::Str(ref s)) if s.contains("ghost")),
        "停机记录指名目标：{:?}",
        halt
    );
    assert!(t.tweens().is_empty(), "停机路径零登记");

    // 运行时非法 ms（非字面量，解析期拦不住）：树侧拒收 —— 不停机、零登记
    //（非法请求不落地，与属性写错静默同家法）。
    let (mut t, _box, brain) = tree2d();
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { ms = 0; tween_pos "box" 1 2 ms }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(
        !vm.locals(brain).unwrap_or_default().contains_key(HALT_LOCAL),
        "运行时拒收不是停机错误"
    );
    assert!(t.tweens().is_empty(), "duration <= 0 拒收：零登记");
}

/// T-TW-05：推进阶段时序 —— 补间推进在 process 之前：同一 tick 内脚本读到的
/// 位置已含本帧推进；时满落位的那一帧，脚本读到的就是终值。
#[test]
fn t_tw_05_advance_runs_before_process() {
    let mut t = SceneTree::new("root");
    let _box = t.add_node(t.root(), "box", NodeKind::Node2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    let reader = t.add_node(t.root(), "reader", NodeKind::Script);
    t.apply_pending();
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_pos "box" 400.0 0.0 1000 }"#.into()),
    )
    .unwrap();
    // 读侧：每帧把 box 的 x 记进局部 seen（process 入口）。
    t.set_prop(
        reader,
        "source",
        Value::Str(r#"every { seen = node("box").pos.x }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());

    let seen_of = |vm: &ScriptVm| -> f32 {
        match vm.locals(reader).unwrap_or_default().get("seen") {
            Some(Value::F32(x)) => *x,
            Some(Value::I64(i)) => *i as f32,
            other => panic!("seen 缺失或类型不符：{:?}", other),
        }
    };

    // tick 1：reader 的 process（阶段 4）先于信号泵（阶段 5）—— 本帧读 0，
    // 补间在泵里才登记（推进阶段已过）。
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(0.25, &mut vm);
    assert_eq!(seen_of(&vm), 0.0, "登记发生在本帧 process 之后：读到旧值");

    // tick 2：推进阶段先走（elapsed 0 -> 250ms，t = 0.25，x = 100），reader
    // 随后在同一 tick 读到新位置 —— 先推进后脚本。
    let _ = t.tick(0.25, &mut vm);
    assert_eq!(seen_of(&vm), 100.0, "同 tick 读到本帧推进后的位置");

    // 续推：每 tick +250ms；时满那一帧（elapsed = 1000 -> t = 1）脚本读终值。
    let _ = t.tick(0.25, &mut vm);
    assert_eq!(seen_of(&vm), 200.0);
    let _ = t.tick(0.25, &mut vm);
    assert_eq!(seen_of(&vm), 300.0);
    assert_eq!(t.tweens().len(), 1, "落位前登记仍在");
    let _ = t.tick(0.25, &mut vm);
    assert_eq!(seen_of(&vm), 400.0, "落位终值同帧可读（推进先于 process）");
    assert!(t.tweens().is_empty(), "落位即移除");
}
