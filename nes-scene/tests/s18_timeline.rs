//! S18.1 编辑器时间轴 dock · 场景层宿主 API 契约回归：补间登记/停止/投影
//! 三件套公开化（编辑器宿主面与脚本 Cmd 面同一条登记表路径）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-TL-01 | `register_tween_channel` 登记 -> `tween_rows` 反映（通道/缓动/模式稳定名 + 进度随 tick 推进增长）-> 时满落位 + 行清空；行序 = 注册序；无关节点 = 空行集 |
//! | T-TL-02 | `stop_tweens` 清行（全通道停在当前值）；last-wins 经宿主面登记同通道替换不叠加；frame 通道 from/to 字面量直通（不落地采样）+ alpha 终点夹取 |
//! | T-TL-03 | 无效参数/节点返回 `false` 不落地（duration <= 0 / 非有限、alpha 终点 NaN、死节点 NodeId）|
//!
//! 边界（文档 §3）：投影行（[`nes_scene::TweenRow`]）是视图不是状态 ——
//! 指纹采样面仍是 `tweens()` 内部切片（本文件不重复 S16 的指纹契约，
//! T-TW-03 已钉）；会话态不进 RON。

use nes_scene::{NodeId, NodeKind, NoObserver, SceneTree, TweenChannel, TweenEasing, TweenMode, Vec2};

/// 搭一棵最小树：root + 方块（Node2D，可承载 pos/scale 通道）。结构即刻落地。
fn tree2d() -> (SceneTree, NodeId) {
    let mut t = SceneTree::new("root");
    let sprite = t.add_node(t.root(), "box", NodeKind::Node2D);
    t.apply_pending();
    (t, sprite)
}

/// T-TL-01：宿主登记 -> rows 反映（进度随 tick 增长、字段稳定名、行序 =
/// 注册序）-> 时满落位终值 + 行清空；无关节点投影为空行集。
#[test]
fn t_tl_01_register_rows_progress_and_removal() {
    let (mut t, box_node) = tree2d();
    let other = t.add_node(t.root(), "other", NodeKind::Node2D);
    t.apply_pending();

    // 登记两条（pos 先、scale 后 —— 行序 = 注册序的确定性断言载体）。
    assert!(t.register_tween_channel(
        box_node,
        TweenChannel::Pos { from: Vec2::ZERO, to: Vec2::new(100.0, 50.0) },
        1000.0,
        TweenEasing::EaseOut,
        TweenMode::Once,
    ));
    assert!(t.register_tween_channel(
        box_node,
        TweenChannel::Scale { from: Vec2::ZERO, to: Vec2::new(2.0, 2.0) },
        800.0,
        TweenEasing::Linear,
        TweenMode::Once,
    ));

    // rows 反映：两条、通道名/缓动/模式 = 稳定名、初始进度 0。
    let rows = t.tween_rows(box_node);
    assert_eq!(rows.len(), 2, "两通道并存，行序 = 注册序");
    assert_eq!(rows[0].channel, "pos");
    assert_eq!(rows[1].channel, "scale");
    assert_eq!(rows[0].easing, "ease_out");
    assert_eq!(rows[0].mode, "once");
    assert_eq!(rows[0].progress, 0.0);
    assert_eq!(rows[0].elapsed_ms, 0.0);
    assert_eq!(rows[0].duration_ms, 1000.0);
    assert_eq!(rows[1].duration_ms, 800.0);
    // 无关节点 = 空行集（投影按目标过滤，不串行）。
    assert!(t.tween_rows(other).is_empty(), "无关节点无行");

    // 推进：0.25s -> pos 行进度单调增长（scale 行更快 —— 时长更短）。
    let _ = t.tick(0.25, &mut NoObserver);
    let rows = t.tween_rows(box_node);
    assert!(
        rows[0].progress > 0.2 && rows[0].progress < 0.3,
        "pos 进度 ≈ 0.25：{}",
        rows[0].progress
    );
    assert!(rows[1].progress > rows[0].progress, "scale 行进度领先（时长短）");

    // 时满（pos 1000ms）落位终值 + 行移除；scale 800ms 更早完成 —— 先推
    // 0.8s 断 scale 行清空，再推到 1.0s 断 pos 落位 (100,50) 且全清。
    let _ = t.tick(0.55, &mut NoObserver); // 累计 0.80s
    let rows = t.tween_rows(box_node);
    assert_eq!(rows.len(), 1, "scale 到站移除，只剩 pos 行");
    assert_eq!(rows[0].channel, "pos");
    let _ = t.tick(0.20, &mut NoObserver); // 累计 1.00s
    assert!(t.tween_rows(box_node).is_empty(), "pos 到站移除，行清空");
    assert_eq!(
        t.local(box_node).unwrap().pos,
        Vec2::new(100.0, 50.0),
        "落位终值（精确写 to，不经插值）"
    );
}

/// T-TL-02：`stop_tweens` 全通道清行（停在当前值）；last-wins 经宿主面
/// 同通道替换；frame 通道 from/to 字面量直通；alpha 终点越界夹取（登记
/// 路径既有口径，宿主面同表）。
#[test]
fn t_tl_02_stop_clears_rows_and_channel_semantics() {
    let (mut t, box_node) = tree2d();

    // 双通道登记 + 推进半程 -> stop -> 行全清、位置停在当前值。
    assert!(t.register_tween_channel(
        box_node,
        TweenChannel::Pos { from: Vec2::ZERO, to: Vec2::new(200.0, 0.0) },
        1000.0,
        TweenEasing::Linear,
        TweenMode::Once,
    ));
    assert!(t.register_tween_channel(
        box_node,
        TweenChannel::Alpha { from: 1.0, to: 0.0 },
        500.0,
        TweenEasing::Linear,
        TweenMode::Once,
    ));
    let _ = t.tick(0.25, &mut NoObserver);
    let held_x = t.local(box_node).unwrap().pos.x;
    assert!(held_x > 0.0 && held_x < 200.0, "推进半程：x={held_x}");
    t.stop_tweens(box_node);
    assert!(t.tween_rows(box_node).is_empty(), "stop 清行（全通道）");
    assert!(t.tweens().is_empty());
    assert_eq!(
        t.local(box_node).unwrap().pos.x,
        held_x,
        "停在当前值（不落 to）"
    );
    // 幂等：再停一次无事发生。
    t.stop_tweens(box_node);
    assert!(t.tweens().is_empty());

    // last-wins（宿主面）：同通道（pos）替换不叠加 —— from 重采样为当前
    // 值（换程不跳变，S16 冻结语义经公开面逐位同源）。
    assert!(t.register_tween_channel(
        box_node,
        TweenChannel::Pos { from: Vec2::ZERO, to: Vec2::new(200.0, 0.0) },
        1000.0,
        TweenEasing::Linear,
        TweenMode::Once,
    ));
    let _ = t.tick(0.25, &mut NoObserver);
    assert!(t.register_tween_channel(
        box_node,
        TweenChannel::Pos { from: Vec2::ZERO, to: Vec2::new(-100.0, 0.0) },
        1000.0,
        TweenEasing::Linear,
        TweenMode::Once,
    ));
    let rows = t.tween_rows(box_node);
    assert_eq!(rows.len(), 1, "last-wins：单行");
    assert_eq!(rows[0].channel, "pos");
    assert!(
        matches!(&t.tweens()[0].channel, TweenChannel::Pos { from, to }
            if *from == t.local(box_node).unwrap().pos && *to == Vec2::new(-100.0, 0.0)),
        "from 重采样 = 当前实际位置、to = 新终点：{:?}",
        t.tweens()[0].channel
    );
    t.stop_tweens(box_node);

    // frame 通道：from/to 是字面量直通（不落地采样）—— 行通道名 "frame"。
    assert!(t.register_tween_channel(
        box_node,
        TweenChannel::Frame { from: 2, to: 9 },
        700.0,
        TweenEasing::EaseInOut,
        TweenMode::Loop,
    ));
    let rows = t.tween_rows(box_node);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].channel, "frame");
    assert_eq!(rows[0].easing, "ease_in_out");
    assert_eq!(rows[0].mode, "loop");
    assert!(matches!(&t.tweens()[0].channel, TweenChannel::Frame { from, to }
        if *from == 2 && *to == 9), "frame 字面量直通");
    t.stop_tweens(box_node);

    // alpha 终点越界夹到 0..1（与 Cmd 落地同表）；进度行照常反映。
    assert!(t.register_tween_channel(
        box_node,
        TweenChannel::Alpha { from: 0.0, to: 1.5 },
        400.0,
        TweenEasing::Linear,
        TweenMode::Once,
    ));
    assert!(
        matches!(&t.tweens()[0].channel, TweenChannel::Alpha { to, .. } if *to == 1.0),
        "alpha 终点夹取"
    );
}

/// T-TL-03：无效参数/节点返回 `false` 不落地（duration <= 0 / 非有限、
/// alpha 终点 NaN、死节点 NodeId）—— 拒收口径与脚本面逐位同表。
#[test]
fn t_tl_03_invalid_node_and_params_rejected() {
    let (mut t, box_node) = tree2d();

    // duration <= 0 / 非有限：拒收。
    assert!(!t.register_tween_channel(
        box_node,
        TweenChannel::Pos { from: Vec2::ZERO, to: Vec2::new(1.0, 1.0) },
        0.0,
        TweenEasing::Linear,
        TweenMode::Once,
    ));
    assert!(!t.register_tween_channel(
        box_node,
        TweenChannel::Pos { from: Vec2::ZERO, to: Vec2::new(1.0, 1.0) },
        -5.0,
        TweenEasing::Linear,
        TweenMode::Once,
    ));
    assert!(!t.register_tween_channel(
        box_node,
        TweenChannel::Pos { from: Vec2::ZERO, to: Vec2::new(1.0, 1.0) },
        f64::NAN,
        TweenEasing::Linear,
        TweenMode::Once,
    ));
    // alpha 终点 NaN：拒收（非有限不夹取 —— 与落地口径同）。
    assert!(!t.register_tween_channel(
        box_node,
        TweenChannel::Alpha { from: 1.0, to: f32::NAN },
        100.0,
        TweenEasing::Linear,
        TweenMode::Once,
    ));

    // 死节点：移除后旧 NodeId 拒收。
    t.remove_node(box_node, false);
    t.apply_pending();
    assert!(!t.register_tween_channel(
        box_node,
        TweenChannel::Pos { from: Vec2::ZERO, to: Vec2::new(1.0, 1.0) },
        100.0,
        TweenEasing::Linear,
        TweenMode::Once,
    ));

    // 全程零落地：登记表保持空、无行。
    assert!(t.tweens().is_empty(), "无效请求一律不落地");
}