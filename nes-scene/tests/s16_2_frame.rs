//! S16.2 图集帧动画 · 场景层契约回归：帧补间通道（`tween_frame`）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-FR-01 | `tween_frame` loop 循环：600ms 4 帧（0→4，回绕归渲染侧）走两圈断言帧序 —— floor 取整序列 `[0,0,1,1,1,2,2,2,3,3,3,0]` ×2；loop 永不移除登记 |
//! | T-FR-02 | once 时满落位 to + 移除；yoyo 去程到 to、回程回 from、回零精确落位 from + 移除 |
//! | T-FR-03 | last-wins 按（节点，frame 通道）二元组：进行中重发替换不打断他通道；`tween_stop` 停在当前帧 |
//! | T-FR-04 | 解析面：语法 / 弹序（from、to、ms）、可选缓动+模式、字面量 ms <= 0 解析期报错、目标不存在停机记录 |
//! | T-FR-05 | 指纹：含帧补间与不含的指纹必不同；同轨迹双跑逐位相同（Frame 通道 i64 位形混入） |
//!
//! 帧写入走**既有属性写路径**（`frame` 是真实树状态）—— 与 alpha 同口径
//! 进语义指纹；终点越界回绕是渲染侧（提取层）的职责，本层不钳制。

use nes_scene::{
    compile_script, scene_fingerprint, NodeId, NodeKind, Op, SceneTree, ScriptEntry, ScriptVm,
    TweenChannel, TweenEasing, TweenMode, Uid, Value, HALT_LOCAL,
};

/// 搭一棵最小树：root + 精灵（Sprite2D，frame 属性的载体）+ 脚本节点。
fn sheet_tree() -> (SceneTree, NodeId, NodeId) {
    let mut t = SceneTree::new("root");
    let sprite = t.add_node(t.root(), "spr", NodeKind::Sprite2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    (t, sprite, brain)
}

fn frame_of(t: &SceneTree, sprite: NodeId) -> i64 {
    t.prop(sprite, "frame")
        .and_then(|v| v.as_i64())
        .expect("frame 属性已按 schema 物化")
}

/// T-FR-01：600ms 4 帧 loop 走两圈 —— 帧序断言 + 登记永不移除。
///
/// 推进算式（冻结）：`p = (elapsed/600) % 1`、`frame = floor(p * 4)`。
/// dt = 0.05s（50ms/tick）：一个循环 12 tick，floor 边界（p*4 恰为整数）
/// 全部带正 epsilon（0.05f32 -> f64 略大于 0.05），确定性取上界 ——
/// 12 tick 序列 = `[0,0,1,1,1,2,2,2,3,3,3,0]`（末位 = 回绕回 0，
/// 渲染侧 `frame % (cols*rows)` 的正主通道）。
#[test]
fn t_fr_01_loop_walk_cycle_two_rounds() {
    let (mut t, sprite, brain) = sheet_tree();
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_frame "spr" 0 4 600 "linear" "loop" }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());

    // 发射 + 首 tick：Cmd 在信号泵落地（登记 elapsed = 0，本帧未推进）。
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens().len(), 1, "帧补间已登记");
    assert!(
        matches!(&t.tweens()[0].channel, TweenChannel::Frame { from, to }
            if *from == 0 && *to == 4),
        "from/to = 语句字面量：{:?}",
        t.tweens()[0].channel
    );
    assert_eq!(t.tweens()[0].easing, TweenEasing::Linear);
    assert_eq!(t.tweens()[0].mode, TweenMode::Loop);
    assert_eq!(frame_of(&t, sprite), 0, "登记帧未推进");

    // 两圈 = 24 tick，逐 tick 断言帧序（floor 取整、第 12/24 tick 回绕 0）。
    let cycle = [0i64, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 0];
    let mut seen = Vec::new();
    for _ in 0..24 {
        let _ = t.tick(0.05, &mut vm);
        seen.push(frame_of(&t, sprite));
    }
    let expected: Vec<i64> = cycle.iter().chain(cycle.iter()).copied().collect();
    assert_eq!(seen, expected, "两圈帧序（floor + 回绕）");
    assert_eq!(t.tweens().len(), 1, "loop 永不移除登记");
}

/// T-FR-02：once 时满落位 to；yoyo 去程/回程/回零精确落位 from。
#[test]
fn t_fr_02_once_lands_and_yoyo_round_trip() {
    // once：300ms 0→3，中点 floor(1.5)=1，时满精确落位 3 并移除。
    let (mut t, sprite, brain) = sheet_tree();
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_frame "spr" 0 3 300 }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let _ = t.tick(0.15, &mut vm); // elapsed = 150ms -> t = 0.5 -> floor(1.5) = 1
    assert_eq!(frame_of(&t, sprite), 1, "中点插值取整 floor(1.5) = 1");
    let _ = t.tick(0.15, &mut vm); // 时满 -> 精确落位 to
    assert_eq!(frame_of(&t, sprite), 3, "once 时满落位 to");
    assert!(t.tweens().is_empty(), "once 时满移除登记");

    // yoyo：去程向 to、回程向 from、回零精确落位 from（总时长 2x）。
    // 注意离散帧的取整边界：折返点 p=1 处 shape = 2-p 带负 epsilon，
    // v = 4.999… -> floor = 4（`to` 只在 land 时精确写 —— yoyo 不在
    // to 落位，本就该读不到 5）。
    let (mut t, sprite, brain) = sheet_tree();
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_frame "spr" 2 5 300 "linear" "yoyo" }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let _ = t.tick(0.15, &mut vm); // 去程中点：2 + 3*0.5 = 3.5 -> floor = 3
    assert_eq!(frame_of(&t, sprite), 3, "yoyo 去程向 to 推进");
    let _ = t.tick(0.15, &mut vm); // 折返点：v = 4.999… -> floor = 4
    assert_eq!(frame_of(&t, sprite), 4, "yoyo 折返点（floor 取整边界）");
    let _ = t.tick(0.15, &mut vm); // 回程中点：5 - 3*0.5 = 3.5 -> floor = 3
    assert_eq!(frame_of(&t, sprite), 3, "yoyo 回程向 from 折返");
    let _ = t.tick(0.15, &mut vm); // 回零：精确落位 from = 2 + 移除
    assert_eq!(frame_of(&t, sprite), 2, "yoyo 回零精确落位 from");
    assert!(t.tweens().is_empty(), "yoyo 完成移除登记");
}

/// T-FR-03：last-wins 按（节点，frame 通道）二元组；与 pos 通道并存；
/// `tween_stop` 停在当前帧。
#[test]
fn t_fr_03_last_wins_coexist_and_stop() {
    let (mut t, sprite, brain) = sheet_tree();
    // 一脚本一入口（S6 冻结）：四个信号入口各一节点（照 S16.1 demo 修正
    // 的同一写法）。brain 本身给空操作（非空 source 才可装载）。
    t.set_prop(brain, "source", Value::Str(r#"on "go" { }"#.into()))
        .unwrap();
    let add = |t: &mut SceneTree, name: &str, src: &str| {
        let n = t.add_node(t.root(), name, NodeKind::Script);
        t.apply_pending();
        t.set_prop(n, "source", Value::Str(src.to_string())).unwrap();
    };
    add(&mut t, "start", r#"on "go" { tween_frame "spr" 0 8 800 }"#);
    add(&mut t, "redo", r#"on "redo" { tween_frame "spr" 6 2 800 }"#);
    add(&mut t, "mover", r#"on "pos" { tween_pos "spr" 10.0 0.0 800 }"#);
    add(&mut t, "halter", r#"on "halt" { tween_stop "spr" }"#);
    let mut vm = ScriptVm::new();
    let issues = vm.attach_all(&mut t);
    assert!(issues.is_empty(), "attach：{issues:?}");

    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let _ = t.tick(0.2, &mut vm); // 200ms：floor(8 * 0.25) = 2
    assert_eq!(frame_of(&t, sprite), 2);

    // 重发（同节点同 frame 通道）：替换登记（不叠加），按新字面量起终点走。
    t.emit_signal("redo", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens().len(), 1, "last-wins：同通道替换不叠加");
    assert!(
        matches!(&t.tweens()[0].channel, TweenChannel::Frame { from, to }
            if *from == 6 && *to == 2),
        "新程 from/to = 重发字面量：{:?}",
        t.tweens()[0].channel
    );
    let _ = t.tick(0.2, &mut vm); // 200ms：6 + (2-6)*0.25 = 5 -> floor = 5
    assert_eq!(frame_of(&t, sprite), 5, "新程按新起终点推进");

    // 并存：pos 通道与 frame 通道互不干扰（各占一条登记）。
    t.emit_signal("pos", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.tweens().len(), 2, "frame 与 pos 通道并存");

    // tween_stop：全部通道丢弃，帧停在当前值。
    t.emit_signal("halt", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let frozen = frame_of(&t, sprite);
    assert!(t.tweens().is_empty(), "tween_stop 清空全部通道");
    let _ = t.tick(0.2, &mut vm);
    assert_eq!(frame_of(&t, sprite), frozen, "停补间后帧保持");
}

/// T-FR-04：解析面 —— 弹序（from、to、ms）、可选缓动 + 模式、字面量
/// ms <= 0 解析期报错、目标不存在停机记录。
#[test]
fn t_fr_04_parse_surface() {
    // 语法 + 编译序（压序 from、to、ms）+ 可选缓动/模式。
    let script = compile_script(r#"every { tween_frame "s" 0 4 600 "ease_in" "yoyo" }"#)
        .expect("合法语句");
    assert_eq!(script.entry, ScriptEntry::Process);
    assert_eq!(
        script.ops,
        vec![
            Op::Const(Value::I64(0)),
            Op::Const(Value::I64(4)),
            Op::Const(Value::I64(600)),
            Op::TweenFrame {
                name: "s".to_string(),
                easing: TweenEasing::EaseIn,
                mode: TweenMode::Yoyo,
            },
        ]
    );
    // 缺省缓动/模式。
    let script = compile_script(r#"every { tween_frame "s" 0 4 600 }"#).expect("合法语句");
    assert_eq!(
        script.ops,
        vec![
            Op::Const(Value::I64(0)),
            Op::Const(Value::I64(4)),
            Op::Const(Value::I64(600)),
            Op::TweenFrame {
                name: "s".to_string(),
                easing: TweenEasing::Linear,
                mode: TweenMode::Once,
            },
        ]
    );

    // 字面量 ms <= 0：解析期报错（0 / 负整数 / 0.0 形态）。
    for src in [
        r#"every { tween_frame "s" 0 4 0 }"#,
        r#"every { tween_frame "s" 0 4 -5 }"#,
        r#"every { tween_frame "s" 0 4 0.0 }"#,
    ] {
        assert!(
            compile_script(src).is_err(),
            "ms 必须 > 0（解析期）：{src}"
        );
    }

    // 未知缓动/模式名：解析期报错。
    assert!(compile_script(r#"every { tween_frame "s" 0 4 600 "quad" }"#).is_err());
    assert!(
        compile_script(r#"every { tween_frame "s" 0 4 600 "linear" "forever" }"#).is_err()
    );

    // 目标不存在：运行时停机记录（照 tween_pos 家法）。
    let (mut t, _sprite, brain) = sheet_tree();
    t.set_prop(
        brain,
        "source",
        Value::Str(r#"on "go" { tween_frame "ghost" 0 4 600 }"#.into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let halt = vm.locals(brain).unwrap_or_default().get(HALT_LOCAL).cloned();
    assert!(
        halt.is_some(),
        "找不到目标节点 -> 停机记录（VM 家法）"
    );
    assert!(t.tweens().is_empty(), "停机不登记");
}

/// T-FR-05：指纹 —— 含帧补间与不含必不同；同轨迹双跑逐位相同。
#[test]
fn t_fr_05_fingerprint_mixes_frame_channel() {
    // uid 钉成确定性派生身份（照 T-TW-03 口径 —— 缺省 Uid 是持久身份，
    // 每棵树各自随机，不钉则跨树指纹必不同）。
    let build = |with_tween: bool| -> (SceneTree, ScriptVm) {
        let (mut t, _sprite, brain) = sheet_tree();
        for (i, n) in t.preorder().into_iter().enumerate() {
            t.set_uid(n, Uid::derive_legacy(&format!("/p{i}"))).unwrap();
        }
        let src = if with_tween {
            r#"on "go" { tween_frame "spr" 0 4 600 }"#
        } else {
            r#"on "go" { }"#
        };
        t.set_prop(brain, "source", Value::Str(src.into())).unwrap();
        let mut vm = ScriptVm::new();
        assert!(vm.attach_all(&mut t).is_empty());
        t.emit_signal("go", Value::I64(0));
        let _ = t.tick(1.0 / 60.0, &mut vm);
        (t, vm)
    };
    let (a, va) = build(true);
    let (b, vb) = build(true);
    assert_eq!(
        scene_fingerprint(&a, Some(&va)),
        scene_fingerprint(&b, Some(&vb)),
        "同轨迹双跑指纹逐位相同（Frame i64 位形混入）"
    );
    let (c, vc) = build(false);
    assert_ne!(
        scene_fingerprint(&a, Some(&va)),
        scene_fingerprint(&c, Some(&vc)),
        "含帧补间与不含的指纹必不同"
    );
}
