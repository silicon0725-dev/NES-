//! S16 动画补间第 1 期 · 场景层契约回归：位置补间成为引擎一等公民。
//! S16.1 补间后续：缓动族 + yoyo/loop 模式 + scale/alpha 通道 + 到站信号。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-TW-01 | `tween_pos` 登记 -> 每 tick 推进（中点插值精确）-> 时满落位终值 + 注册表移除 |
//! | T-TW-02 | last-wins：进行中再发，从**当前实际位置**起新程（不跳变、不叠加）；`tween_stop` 停在当前值 |
//! | T-TW-03 | 确定性：同轨迹双跑指纹逐位相同；含补间与不含的指纹必不同；目标节点删除 -> 补间自动清（NodeHandle resolve 失败） |
//! | T-TW-04 | 解析面：`tween_pos` / `tween_stop` 语法、字面量 ms <= 0 解析期报错、目标不存在停机记录、运行时非法 ms 树侧拒收 |
//! | T-TW-05 | 推进阶段时序：补间推进后、process 前 —— 同一 tick 内脚本读到新位置（先推进后脚本；时满落位同帧可读终值） |
//! | T-E-01 | 缓动族：五种缓动 t=0.5 各自断言（smoothstep=0.5、ease_in=0.25、ease_out=0.75、ease_in_out=0.5、linear=0.5）；未知缓动/模式名解析期报错（附合法名单） |
//! | T-M-01 | 模式：yoyo 前半程到 to、后半程回 from、回零落位 + 移除 + 到站信号；loop 永不移除、位置周期性、永不到站 |
//! | T-S-01 | `tween_scale` 中点断言（scale 插值）+ 与 pos 通道并存互不干扰（last-wins 按（节点，通道）二元组） |
//! | T-A-01 | `tween_alpha` 推进中 alpha 属性值变化（schema 读面，真实树状态）+ 越界夹取 + 到站落位 |
//! | T-SIG-01 | 到站信号：载荷 = 节点名 Str；每通道完成各发一条、次序 = 注册序（同帧双通道完成 = 两条 tween_done） |
//!
//! 补间是**游戏可见状态**（推进在 `SceneTree::tick` 专属阶段：结构落地后、
//! enter/process 前，每 tick 直写 local/属性并进语义指纹）；序列化面是
//! **会话态**（不进 RON 往返 —— 补间不在 `NodeData`，`to_doc` 天然不携带）。

use nes_scene::{
    compile_script, scene_fingerprint, NodeId, NodeKind, Op, SceneTree, ScriptEntry, ScriptVm,
    TweenChannel, TweenEasing, TweenMode, Uid, Value, Vec2, HALT_LOCAL,
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
    assert!(
        matches!(&t.tweens()[0].channel, TweenChannel::Pos { from, to }
            if *from == Vec2::new(0.0, 0.0) && *to == Vec2::new(100.0, 50.0)),
        "from = 当前实际位置、to = 终点：{:?}",
        t.tweens()[0].channel
    );
    assert_eq!(t.tweens()[0].duration_ms, 1000.0);
    assert_eq!(t.tweens()[0].elapsed_ms, 0.0);
    assert_eq!(t.tweens()[0].easing, TweenEasing::Linear, "缺省缓动 = linear");
    assert_eq!(t.tweens()[0].mode, TweenMode::Once, "缺省模式 = once");

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
    let (tw_from, tw_to) = match &tw.channel {
        TweenChannel::Pos { from, to } => (*from, *to),
        other => panic!("pos 通道预期，实际 {other:?}"),
    };
    assert_eq!(tw_to, Vec2::new(0.0, 80.0));
    assert_eq!(tw.duration_ms, 400.0);
    let pos_after = t.local(box_node).unwrap().pos;
    assert!(
        approx(tw_from.x, pos_after.x) && approx(tw_from.y, pos_after.y),
        "登记不挪节点：from = 落地时当前实际位置（不跳变）from={:?} pos={:?}",
        tw_from,
        pos_after
    );
    assert!(
        tw_from.x > pos_before.x && tw_from.x > 50.0,
        "from 含本帧推进（53.33 一类），不是旧起点 0：from={:?} before={:?}",
        tw_from,
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
    // 编译产物：x/y/ms 三表达式 + TweenPos（压序 = 源序；缺省缓动/模式）。
    let script = compile_script(r#"every { tween_pos "box" 32 48 500 }"#).expect("编译");
    assert_eq!(script.entry, ScriptEntry::Process);
    assert_eq!(
        script.ops,
        vec![
            Op::Const(Value::I64(32)),
            Op::Const(Value::I64(48)),
            Op::Const(Value::I64(500)),
            Op::TweenPos {
                name: "box".into(),
                easing: TweenEasing::Linear,
                mode: TweenMode::Once,
            },
        ]
    );
    // 可选尾缀：缓动名 / 模式名按位落进编译产物。
    let script = compile_script(r#"every { tween_pos "box" 32 48 500 "ease_out" "loop" }"#)
        .expect("编译");
    assert_eq!(
        script.ops,
        vec![
            Op::Const(Value::I64(32)),
            Op::Const(Value::I64(48)),
            Op::Const(Value::I64(500)),
            Op::TweenPos {
                name: "box".into(),
                easing: TweenEasing::EaseOut,
                mode: TweenMode::Loop,
            },
        ]
    );
    // tween_stop：零栈交互（照 play 形态）。
    let script = compile_script(r#"every { tween_stop "box" }"#).expect("编译");
    assert_eq!(script.ops, vec![Op::TweenStop { name: "box".into() }]);
    // tween_scale / tween_alpha（S16.1）：编译产物形状与栈序。
    let script = compile_script(r#"every { tween_scale "box" 2 3 500 }"#).expect("编译");
    assert_eq!(
        script.ops,
        vec![
            Op::Const(Value::I64(2)),
            Op::Const(Value::I64(3)),
            Op::Const(Value::I64(500)),
            Op::TweenScale {
                name: "box".into(),
                easing: TweenEasing::Linear,
                mode: TweenMode::Once,
            },
        ]
    );
    let script = compile_script(r#"every { tween_alpha "box" 0.5 500 "smoothstep" }"#).expect("编译");
    assert_eq!(
        script.ops,
        vec![
            Op::Const(Value::F32(0.5)),
            Op::Const(Value::I64(500)),
            Op::TweenAlpha {
                name: "box".into(),
                easing: TweenEasing::Smoothstep,
                mode: TweenMode::Once,
            },
        ]
    );

    // 字面量 ms <= 0：解析期报错（0 / 负整数 / 0.0 / 一元负号形态）。
    assert!(compile_script(r#"every { tween_pos "box" 0 0 0 }"#).is_err());
    assert!(compile_script(r#"every { tween_pos "box" 0 0 -5 }"#).is_err());
    assert!(compile_script(r#"every { tween_pos "box" 0 0 0.0 }"#).is_err());
    // 保留字：不得作变量名。
    assert!(compile_script(r#"every { tween_pos = 1 }"#).is_err());
    assert!(compile_script(r#"every { tween_stop = 1 }"#).is_err());
    assert!(compile_script(r#"every { x = tween_pos }"#).is_err());
    assert!(compile_script(r#"every { tween_scale = 1 }"#).is_err());
    assert!(compile_script(r#"every { tween_alpha = 1 }"#).is_err());

    // 新语句的解析期 ms 检查与 tween_pos 同口径。
    assert!(compile_script(r#"every { tween_scale "box" 1 1 0 }"#).is_err());
    assert!(compile_script(r#"every { tween_alpha "box" 1 0 }"#).is_err());

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

/// 信号观察脚本：把 tween_done 的载荷与次序记进局部（init 复位）。
const TWEEN_DONE_WATCH: &str = concat!(
    "init { n = 0; first = \"\"; second = \"\" }\n",
    "on \"tween_done\" { n = n + 1; if n == 1 { first = arg }; if n == 2 { second = arg } }",
);

/// 读观察脚本的局部（init 在首次派发才跑 —— 之前缺省 `None`）。
fn local_of(vm: &ScriptVm, node: NodeId, name: &str) -> Option<Value> {
    vm.locals(node).unwrap_or_default().get(name).cloned()
}

/// T-E-01：缓动族 —— 五种缓动在 t = 0.5 的值各自断言（from (0,0) ->
/// to (100,0)，中点值即形状函数的指纹值）+ 未知缓动/模式名解析期报错
///（附合法名单 —— 拼写错误不静默降级 linear）。
#[test]
fn t_e_01_easing_family_midpoint_and_unknown_name() {
    // (缓动名, 中点 x 期望)：smoothstep(0.5)=0.5、ease_in(0.5)=0.25、
    // ease_out(0.5)=0.75、ease_in_out(0.5)=0.5（分支边界）、linear=0.5。
    let cases: [(&str, f32); 5] = [
        ("linear", 50.0),
        ("smoothstep", 50.0),
        ("ease_in", 25.0),
        ("ease_out", 75.0),
        ("ease_in_out", 50.0),
    ];
    for (name, mid) in cases {
        let (mut t, box_node, brain) = tree2d();
        let src = format!(r#"on "go" {{ tween_pos "box" 100.0 0.0 1000 "{}" }}"#, name);
        t.set_prop(brain, "source", Value::Str(src)).unwrap();
        let mut vm = ScriptVm::new();
        assert!(vm.attach_all(&mut t).is_empty());
        t.emit_signal("go", Value::I64(0));
        let _ = t.tick(1.0 / 60.0, &mut vm);
        assert_eq!(t.tweens().len(), 1);
        assert_eq!(t.tweens()[0].easing.as_str(), name, "缓动名应落进登记");
        let _ = t.tick(0.5, &mut vm);
        let x = t.local(box_node).unwrap().pos.x;
        assert_eq!(x, mid, "缓动 {name} 在 t=0.5 的值应为 {mid}");
    }

    // 未知缓动名：解析期报错，文案指名输入并附合法名单。
    let err = compile_script(r#"every { tween_pos "box" 0 0 100 "wobble" }"#)
        .expect_err("未知缓动名必须解析期报错");
    let msg = err.to_string();
    assert!(msg.contains("wobble"), "报错应指名输入：{msg}");
    assert!(msg.contains("ease_in_out"), "报错应附合法名单：{msg}");
    // 第二个尾缀槽 = 模式名：未知模式同样报错并附名单。
    let err = compile_script(r#"every { tween_pos "box" 0 0 100 "linear" "bounce" }"#)
        .expect_err("未知模式名必须解析期报错");
    let msg = err.to_string();
    assert!(
        msg.contains("bounce") && msg.contains("yoyo"),
        "模式报错应指名输入并附名单：{msg}"
    );
}

/// T-M-01：模式 —— yoyo（前半程到 to、后半程回 from、回零落位 from +
/// 移除 + 到站信号恰一次）；loop（永不移除、位置周期性、永不到站）。
#[test]
fn t_m_01_yoyo_and_loop_modes() {
    // --- yoyo：duration 1000ms -> 总时长 2000ms。
    let (mut t, box_node, brain) = tree2d();
    let watch = add_signal_script(&mut t, "watch", "tween_done", TWEEN_DONE_WATCH);
    let halt = add_signal_script(
        &mut t,
        "halt_brain",
        "halt",
        r#"on "halt" { tween_stop "box" }"#,
    );
    let _ = halt;
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_pos "box" 100.0 0.0 1000 "linear" "yoyo" }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens()[0].mode, TweenMode::Yoyo, "模式落进登记");

    // 前半程：p = 0.5 -> x = 50；p = 1.0 -> 到 to（不移除、不到站）。
    let _ = t.tick(0.5, &mut vm);
    assert_eq!(t.local(box_node).unwrap().pos.x, 50.0);
    let _ = t.tick(0.5, &mut vm);
    assert_eq!(t.local(box_node).unwrap().pos.x, 100.0, "前半程到 to");
    assert_eq!(t.tweens().len(), 1, "yoyo 换向不落位不移除");
    assert_eq!(local_of(&vm, watch, "n"), None, "换向点不是到站（init 未派发零记录）");

    // 后半程：p = 1.5 -> shape = 0.5 -> x = 50；p = 2.0 -> 回零落位 from +
    // 移除 + 到站信号（推进阶段泵前发出，本帧阶段 5 送达）。
    let _ = t.tick(0.5, &mut vm);
    assert_eq!(t.local(box_node).unwrap().pos.x, 50.0, "后半程回程");
    let _ = t.tick(0.5, &mut vm);
    assert_eq!(t.local(box_node).unwrap().pos.x, 0.0, "回零落位 from");
    assert!(t.tweens().is_empty(), "yoyo 完成移除登记");
    assert_eq!(local_of(&vm, watch, "n"), Some(Value::I64(1)), "到站信号恰一次");
    assert_eq!(
        local_of(&vm, watch, "first"),
        Some(Value::Str("box".into())),
        "载荷 = 节点名"
    );

    // --- loop：duration 500ms —— 永不移除、位置周期性、永不到站。
    //     tick 全用 f32 精确值（0.25 -> 250ms 整），进度取模无浮点噪声。
    let (mut t, box_node, brain) = tree2d();
    let watch = add_signal_script(&mut t, "watch", "tween_done", TWEEN_DONE_WATCH);
    let halt = add_signal_script(
        &mut t,
        "halt_brain",
        "halt",
        r#"on "halt" { tween_stop "box" }"#,
    );
    let _ = halt;
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_pos "box" 100.0 0.0 500 "linear" "loop" }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    // 周期性：elapsed 0.5s 与 1.0s 时进度取模回 0 -> x = 0；中途 p=0.5 -> x = 50。
    let _ = t.tick(0.25, &mut vm);
    let _ = t.tick(0.25, &mut vm);
    assert_eq!(t.local(box_node).unwrap().pos.x, 0.0, "整周期回零");
    let _ = t.tick(0.25, &mut vm);
    assert_eq!(t.local(box_node).unwrap().pos.x, 50.0);
    let _ = t.tick(0.25, &mut vm);
    assert_eq!(
        t.local(box_node).unwrap().pos.x,
        0.0,
        "第二周期回零（周期性）"
    );
    assert_eq!(t.tweens().len(), 1, "loop 永不自动移除");
    assert_eq!(
        local_of(&vm, watch, "n"),
        None,
        "loop 永不到站（信号不发，init 亦未派发）"
    );
    // 停用走 tween_stop（信号入口，既有语义：停在当前值）。
    t.emit_signal("halt", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(t.tweens().is_empty(), "tween_stop 停 loop");
}

/// T-S-01：scale 通道 —— 中点断言（scale 插值）+ 与 pos 通道并存互不
/// 干扰（last-wins 按（节点，通道）二元组：重发 scale 不动 pos）。
#[test]
fn t_s_01_scale_channel_and_pos_coexist() {
    let (mut t, box_node, brain) = tree2d();
    t.set_prop(
        brain,
        "source",
        Value::Str(
            r#"on "go" { tween_scale "box" 4.0 6.0 1000; tween_pos "box" 100.0 50.0 1000 }"#
                .into(),
        ),
    )
    .unwrap();
    let redo = add_signal_script(
        &mut t,
        "redo",
        "redo",
        r#"on "redo" { tween_scale "box" 2.0 2.0 1000 }"#,
    );
    let _ = redo;
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());

    // 登记：pos 与 scale 各占一条（并存互不干扰）。
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens().len(), 2, "pos 与 scale 通道并存");
    assert!(
        t.tweens().iter().any(|tw| matches!(tw.channel, TweenChannel::Scale { from, to }
            if from == Vec2::new(1.0, 1.0) && to == Vec2::new(4.0, 6.0))),
        "scale from = 当前 scale"
    );

    // 推进 500ms：两通道各自推进；中点断言（from = (1,1) 缺省 scale）。
    let _ = t.tick(0.25, &mut vm);
    let _ = t.tick(0.25, &mut vm);
    let scale = t.local(box_node).unwrap().scale;
    assert_eq!(scale, Vec2::new(2.5, 3.5), "scale 中点插值（x/y 同步插值）");
    let pos = t.local(box_node).unwrap().pos;
    assert_eq!(pos, Vec2::new(50.0, 25.0), "pos 通道同时推进不受 scale 干扰");

    // 重发 scale（last-wins 按通道）：scale 被替换（elapsed 归零、新 from =
    // 当前 scale），pos 通道原样保留（elapsed 连续）。
    t.emit_signal("redo", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens().len(), 2, "按通道替换而非叠加");
    let scale_tw = t
        .tweens()
        .iter()
        .find(|tw| matches!(tw.channel, TweenChannel::Scale { .. }))
        .expect("scale 通道仍在");
    assert_eq!(scale_tw.elapsed_ms, 0.0, "scale 重发从头计");
    let cur_scale = t.local(box_node).unwrap().scale;
    assert!(
        matches!(&scale_tw.channel, TweenChannel::Scale { from, .. } if *from == cur_scale),
        "新 from = 落地时当前 scale（不跳变，含本帧推进）：{:?} vs {:?}",
        scale_tw.channel,
        cur_scale
    );
    let pos_tw = t
        .tweens()
        .iter()
        .find(|tw| matches!(tw.channel, TweenChannel::Pos { .. }))
        .expect("pos 通道仍在");
    assert!(
        (pos_tw.elapsed_ms - 500.0 - 1000.0 / 60.0).abs() < 1e-3,
        "pos 未被 scale 重发打断（elapsed 连续）：{}",
        pos_tw.elapsed_ms
    );
}

/// T-A-01：alpha 通道 —— 推进中 alpha 属性值变化（schema 读面：alpha 是
/// 真实树状态，与 pos 同口径进指纹）+ 越界夹取 + 到站落位 + tween_stop
/// 全通道语义。
#[test]
fn t_a_01_alpha_channel_advances_prop() {
    let mut t = SceneTree::new("root");
    let spr = t.add_node(t.root(), "spr", NodeKind::Sprite2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    // 三个辅助信号脚本（脚本在 attach 时编译 —— 换源不改已挂载字节码，
    // 照既有纪律各用独立节点）：
    let clamp_brain = t.add_node(t.root(), "clamp_brain", NodeKind::Script);
    let both_brain = t.add_node(t.root(), "both_brain", NodeKind::Script);
    let halt_brain = t.add_node(t.root(), "halt_brain", NodeKind::Script);
    let watch = t.add_node(t.root(), "watch", NodeKind::Script);
    t.apply_pending();
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_alpha "spr" 0.0 1000 }"#.into()),
    )
    .unwrap();
    t.set_prop(
        clamp_brain,
        "source",
        Value::Str(r#"on "clamp" { tween_alpha "spr" 2.0 100 }"#.into()),
    )
    .unwrap();
    t.set_prop(
        both_brain,
        "source",
        Value::Str(
            r#"on "both" { tween_alpha "spr" 0.0 1000; tween_pos "spr" 9.0 9.0 1000 }"#.into(),
        ),
    )
    .unwrap();
    t.set_prop(
        halt_brain,
        "source",
        Value::Str(r#"on "halt" { tween_stop "spr" }"#.into()),
    )
    .unwrap();
    t.set_prop(watch, "source", Value::Str(TWEEN_DONE_WATCH.into()))
        .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    assert_eq!(
        t.prop(spr, "alpha"),
        Some(&Value::F32(1.0)),
        "schema 缺省物化 alpha=1.0"
    );

    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(
        matches!(&t.tweens()[0].channel, TweenChannel::Alpha { from, to } if *from == 1.0 && *to == 0.0),
        "from = 当前 alpha"
    );

    // 推进 500ms -> alpha = 0.5（经既有属性写路径，schema 可读）。
    let _ = t.tick(0.5, &mut vm);
    assert_eq!(
        t.prop(spr, "alpha"),
        Some(&Value::F32(0.5)),
        "推进中属性值变化"
    );
    // 时满落位 0.0 + 到站信号。
    let _ = t.tick(0.5, &mut vm);
    assert_eq!(t.prop(spr, "alpha"), Some(&Value::F32(0.0)), "落位终值");
    assert!(t.tweens().is_empty());
    assert_eq!(local_of(&vm, watch, "n"), Some(Value::I64(1)));

    // 越界夹取：终点 2.0 落地时夹到 1.0（schema 口径）。
    t.emit_signal("clamp", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(
        matches!(&t.tweens()[0].channel, TweenChannel::Alpha { to, .. } if *to == 1.0),
        "终点越界夹到 0..1"
    );
    let _ = t.tick(0.2, &mut vm);
    assert_eq!(t.prop(spr, "alpha"), Some(&Value::F32(1.0)), "夹取后到站");

    // tween_stop 语义（S16.1）：停该节点全部通道 —— alpha + pos 并存时一并停。
    t.emit_signal("both", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens().len(), 2, "alpha + pos 并存");
    t.emit_signal("halt", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(t.tweens().is_empty(), "tween_stop = 全部通道一并停");
}

/// T-SIG-01：到站信号 —— 载荷 = 节点名（Str）；每通道完成各发一条、
/// 次序 = 注册序（pos + alpha 同帧完成 = 两条 tween_done，先登记先送达）。
#[test]
fn t_sig_01_tween_done_payload_and_order() {
    let mut t = SceneTree::new("root");
    let _box = t.add_node(t.root(), "aaa", NodeKind::Node2D);
    let _spr = t.add_node(t.root(), "bbb", NodeKind::Sprite2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    let watch = t.add_node(t.root(), "watch", NodeKind::Script);
    t.apply_pending();
    t.set_prop(
        brain,
        "source",
        Value::Str(
            r#"on "go" { tween_pos "aaa" 10.0 0.0 100; tween_alpha "bbb" 0.0 100 }"#.into(),
        ),
    )
    .unwrap();
    t.set_prop(watch, "source", Value::Str(TWEEN_DONE_WATCH.into()))
        .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());

    // 登记（pos 先、alpha 后 —— 注册序）。
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens().len(), 2);
    assert_eq!(local_of(&vm, watch, "n"), None, "未完成零信号（init 未派发）");

    // 同帧双通道完成：两条 tween_done，载荷与次序 = 注册序。
    let _ = t.tick(0.1, &mut vm);
    assert!(t.tweens().is_empty(), "两通道同时满");
    assert_eq!(local_of(&vm, watch, "n"), Some(Value::I64(2)), "每通道完成各发一条");
    assert_eq!(
        local_of(&vm, watch, "first"),
        Some(Value::Str("aaa".into())),
        "先登记先送达"
    );
    assert_eq!(
        local_of(&vm, watch, "second"),
        Some(Value::Str("bbb".into())),
        "次序 = 注册序"
    );
}
