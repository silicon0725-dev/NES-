//! T-Pause 契约回归：暂停与时间缩放（草案 §9，S6.4）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Pause-01 | 缺省全 `Inherit` 解析为 `Pausable`：暂停后 `process` 不派发、计数进 `process_skipped`；恢复后照常 |
//! | T-Pause-02 | `Always` 不受暂停影响：照常派发且 delta 为原始缩放值 |
//! | T-Pause-03 | `WhenPaused` 仅暂停时派发且 `delta == 0`（时间冻结）；非暂停期不派发 |
//! | T-Pause-04 | `Disabled` 无论暂停与否都不派发 |
//! | T-Pause-05 | 继承解析：父 `Always` 下 `Inherit` 子照常跑；同父显式 `Pausable` 子仍跳过 |
//! | T-Pause-06 | `time_scale` 只乘 delta 不改遍历次数；负值与 NaN 钳制 |
//! | T-Pause-07 | 暂停期间结构变更与生命周期照常：`Always` 节点回调里 Spawn，下一帧落地并 enter |
//! | T-Pause-08 | 生命周期（enter/ready）不受暂停影响：暂停帧里新节点照样完成 enter/ready |

use nes_scene::{NodeCtx, NodeKind, ProcessMode, SceneObserver, SceneTree};

/// 记录每次 process 的 (名字, delta)。
#[derive(Default)]
struct Log {
    process: Vec<(&'static str, f32)>,
    enter: Vec<&'static str>,
}

impl SceneObserver for Log {
    fn on_enter_tree(&mut self, ctx: &mut NodeCtx<'_>) {
        self.enter
            .push(Box::leak(ctx.name().to_string().into_boxed_str()));
    }
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, delta: f32) {
        self.process
            .push((Box::leak(ctx.name().to_string().into_boxed_str()), delta));
    }
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

/// T-Pause-01：缺省（全 Inherit -> Pausable）暂停即停、恢复即续，
/// 且被跳过的节点在 `process_skipped` 里可见（不静默消失）。
#[test]
fn t_pause_01_default_inherit_resolves_pausable() {
    let mut t = SceneTree::new("root");
    let _a = t.add_node(t.root(), "A", NodeKind::Node2D);
    let _ = t.tick(0.016, &mut Log::default()); // 完成首帧生命周期

    t.set_paused(true);
    let mut log = Log::default();
    let stats = t.tick(0.016, &mut log);
    assert!(log.process.is_empty(), "暂停后 Pausable 不派发");
    assert_eq!(stats.processed, 0);
    assert_eq!(stats.process_skipped, 2, "root + A 都计入跳过");
    // processed + skipped == 遍历节点数（对账恒等式）
    assert_eq!(stats.processed + stats.process_skipped, 2);

    t.set_paused(false);
    let mut log = Log::default();
    let stats = t.tick(0.016, &mut log);
    assert_eq!(log.process.len(), 2, "恢复后照常派发");
    assert_eq!(stats.process_skipped, 0);
    assert!(log.process.iter().all(|(_, d)| approx(*d, 0.016)));
}

/// T-Pause-02：`Always` 暂停期间照常派发，delta 不受暂停影响（UI、存档点必需）。
#[test]
fn t_pause_02_always_runs_during_pause() {
    let mut t = SceneTree::new("root");
    let ui = t.add_node(t.root(), "ui", NodeKind::Control);
    t.set_process_mode(ui, ProcessMode::Always);
    let _ = t.tick(0.016, &mut Log::default());

    t.set_paused(true);
    let mut log = Log::default();
    t.tick(0.016, &mut log);
    let ui_calls: Vec<_> = log.process.iter().filter(|(n, _)| *n == "ui").collect();
    assert_eq!(ui_calls.len(), 1, "Always 节点照常派发");
    assert!(approx(ui_calls[0].1, 0.016), "delta 不受暂停影响");
    // root（缺省 Pausable）不派发
    assert!(!log.process.iter().any(|(n, _)| *n == "root"));
}

/// T-Pause-03：`WhenPaused` 仅暂停时派发、delta 冻结为 0；非暂停期不派发。
#[test]
fn t_pause_03_when_paused_frozen_delta() {
    let mut t = SceneTree::new("root");
    let menu = t.add_node(t.root(), "menu", NodeKind::Control);
    t.set_process_mode(menu, ProcessMode::WhenPaused);
    let _ = t.tick(0.016, &mut Log::default());

    // 非暂停期：不派发
    let mut log = Log::default();
    let stats = t.tick(0.016, &mut log);
    assert!(!log.process.iter().any(|(n, _)| *n == "menu"));
    assert_eq!(stats.process_skipped, 1);

    // 暂停期：派发但 delta == 0（时间冻结，逻辑/结构仍可做）
    t.set_paused(true);
    let mut log = Log::default();
    t.tick(0.016, &mut log);
    let menu_calls: Vec<_> = log.process.iter().filter(|(n, _)| *n == "menu").collect();
    assert_eq!(menu_calls.len(), 1);
    assert!(approx(menu_calls[0].1, 0.0), "WhenPaused 暂停期 delta 冻结为 0");
}

/// T-Pause-04：`Disabled` 无论暂停与否都不派发。
#[test]
fn t_pause_04_disabled_never_processes() {
    let mut t = SceneTree::new("root");
    let dead = t.add_node(t.root(), "dead", NodeKind::Node2D);
    t.set_process_mode(dead, ProcessMode::Disabled);
    let _ = t.tick(0.016, &mut Log::default());

    for paused in [false, true] {
        t.set_paused(paused);
        let mut log = Log::default();
        let stats = t.tick(0.016, &mut log);
        assert!(!log.process.iter().any(|(n, _)| *n == "dead"), "paused={paused}");
        // 未暂停：只有 dead（Disabled）跳过；暂停：root（Pausable）也一起跳过。
        assert_eq!(stats.process_skipped, if paused { 2 } else { 1 });
    }
}

/// T-Pause-05：继承解析 —— 父 `Always` 的 `Inherit` 子照常跑；
/// 同父下显式 `Pausable` 的子仍跳过；整链 Inherit 解析为 Pausable。
#[test]
fn t_pause_05_inheritance_resolution() {
    let mut t = SceneTree::new("root");
    let ui_root = t.add_node(t.root(), "ui_root", NodeKind::Control);
    t.set_process_mode(ui_root, ProcessMode::Always);
    let child = t.add_node(ui_root, "child", NodeKind::Node2D); // Inherit
    let stubborn = t.add_node(ui_root, "stubborn", NodeKind::Node2D);
    t.set_process_mode(stubborn, ProcessMode::Pausable);
    let _ = t.tick(0.016, &mut Log::default());

    // 生效模式解析正确性（直查）
    assert_eq!(t.effective_process_mode(ui_root), ProcessMode::Always);
    assert_eq!(t.effective_process_mode(child), ProcessMode::Always, "子继承父");
    assert_eq!(t.effective_process_mode(stubborn), ProcessMode::Pausable);
    assert_eq!(t.effective_process_mode(t.root()), ProcessMode::Pausable, "链尾默认");

    t.set_paused(true);
    let mut log = Log::default();
    t.tick(0.016, &mut log);
    let names: Vec<_> = log.process.iter().map(|(n, _)| *n).collect();
    assert!(names.contains(&"ui_root"), "Always 父照常");
    assert!(names.contains(&"child"), "Inherit 子继承 Always");
    assert!(!names.contains(&"stubborn"), "显式 Pausable 仍跳过");
    assert!(!names.contains(&"root"), "链尾 Inherit -> Pausable 跳过");
}

/// T-Pause-06：`time_scale` 只乘 delta、不改遍历次数；负值与 NaN 钳制。
#[test]
fn t_pause_06_time_scale_only_scales_delta() {
    let mut t = SceneTree::new("root");
    let ui = t.add_node(t.root(), "ui", NodeKind::Node2D);
    t.set_process_mode(ui, ProcessMode::Always);
    let _ = t.tick(0.016, &mut Log::default());

    t.set_time_scale(0.5);
    let mut log = Log::default();
    let stats = t.tick(0.016, &mut log);
    assert_eq!(log.process.len(), 2, "遍历次数不变（确定性优先）");
    assert!(log.process.iter().all(|(_, d)| approx(*d, 0.008)), "delta = 0.016*0.5");
    assert_eq!(stats.processed, 2);

    // 负值 -> 0（时间冻结而非倒流）；NaN -> 1.0（缺省，拒绝无定义值）
    t.set_time_scale(-2.0);
    assert_eq!(t.time_scale(), 0.0);
    t.set_time_scale(f32::NAN);
    assert_eq!(t.time_scale(), 1.0);
}

/// T-Pause-07：暂停期间结构变更照常 —— `Always` 节点回调里 Spawn，
/// 下一帧落地（暂停不冻结结构，否则暂停期间 UI 无法增删节点）。
#[test]
fn t_pause_07_structure_changes_land_while_paused() {
    struct Spawner {
        fired: bool,
    }
    impl SceneObserver for Spawner {
        fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
            if !self.fired && ctx.name() == "ui" {
                self.fired = true;
                ctx.spawn_child("pause_label", NodeKind::Label);
            }
        }
    }

    let mut t = SceneTree::new("root");
    let ui = t.add_node(t.root(), "ui", NodeKind::Control);
    t.set_process_mode(ui, ProcessMode::Always);
    let _ = t.tick(0.016, &mut Log::default());

    t.set_paused(true);
    let mut spawner = Spawner { fired: false };
    t.tick(0.016, &mut spawner);
    assert!(spawner.fired);
    assert!(t.find_by_name("pause_label").is_none(), "本帧未落地（延迟一帧）");

    let mut log = Log::default();
    t.tick(0.016, &mut log);
    assert!(t.find_by_name("pause_label").is_some(), "下一帧结构落地");
    assert!(
        log.enter.contains(&"pause_label"),
        "落地帧完成 enter（暂停不影响生命周期）"
    );
}

/// T-Pause-08：生命周期不受暂停影响 —— 暂停帧里入树的节点照样完成
/// enter/ready（在 process 之前），即使它的生效模式是 Pausable。
#[test]
fn t_pause_08_lifecycle_unaffected_by_pause() {
    let mut t = SceneTree::new("root");
    t.set_paused(true);
    let _late = t.add_node(t.root(), "late", NodeKind::Node2D);
    t.apply_pending();

    let mut log = Log::default();
    let stats = t.tick(0.016, &mut log);
    // 这是树的首帧：root 与 late 都要完成 enter/ready（自顶向下 / 自底向上）。
    assert_eq!(stats.entered, 2, "暂停帧完成 enter");
    assert_eq!(stats.readied, 2, "暂停帧完成 ready");
    assert_eq!(stats.process_skipped, 2, "但 process 仍按 Pausable 跳过");
    assert_eq!(log.enter, vec!["root", "late"]);
}

// ---------------------------------------------------------------- 序列化口径
// 裁决（S6.5）：process_mode 是 NodeDoc 一等字段（与 local 同裁决 —— 固有
// 调度数据不进属性表）。缺省 Inherit 不写出，存量文件逐字节不变。

use nes_scene::{instantiate, parse_ron, write_ron, PackOptions};

/// T-PM-01：往返 —— 树上设置的模式经 写出 -> 解析 -> 实例化 原样复原，
/// 生效模式（继承解析）随之贯通。
#[test]
fn t_pm_01_roundtrip_preserves_process_mode() {
    let mut t = SceneTree::new("root");
    let ui = t.add_node(t.root(), "ui", NodeKind::Control);
    t.set_process_mode(ui, ProcessMode::Always);
    let menu = t.add_node(ui, "menu", NodeKind::Control);
    t.set_process_mode(menu, ProcessMode::WhenPaused);
    let world = t.add_node(t.root(), "world", NodeKind::Node2D);
    t.set_process_mode(world, ProcessMode::Disabled);
    t.apply_pending();

    let ron = write_ron(&t, &PackOptions::verbose());
    let mut back = instantiate(&ron).expect("解析回读");
    let ui2 = back.find_by_name("ui").unwrap();
    let menu2 = back.find_by_name("menu").unwrap();
    let world2 = back.find_by_name("world").unwrap();
    assert_eq!(back.process_mode(ui2), Some(ProcessMode::Always));
    assert_eq!(back.process_mode(menu2), Some(ProcessMode::WhenPaused));
    assert_eq!(back.process_mode(world2), Some(ProcessMode::Disabled));
    // Inherit 子的生效模式继承自复原后的 Always 父
    let inherit_child = back.add_node(ui2, "late_child", NodeKind::Node2D);
    back.apply_pending();
    assert_eq!(back.effective_process_mode(inherit_child), ProcessMode::Always);
}

/// T-PM-02：存量兼容 —— 不含 process_mode 字段的场景解析为 Inherit
///（缺省即继承，语义不丢）。
#[test]
fn t_pm_02_legacy_file_defaults_to_inherit() {
    let legacy = r#"Scene(
    version: 1,
    root: Node(
        name: "Hero",
        kind: "Sprite2D",
        props: {},
        children: [],
    ),
)"#;
    let doc = parse_ron(legacy).expect("存量场景必须能解析");
    assert_eq!(doc.root.process_mode, ProcessMode::Inherit);
    let t = instantiate(legacy).expect("实例化");
    assert_eq!(t.process_mode(t.root()), Some(ProcessMode::Inherit));
}

/// T-PM-03：compact 省略 —— 缺省 Inherit 不写出（存量字节不变），
/// 非 Inherit 必写出；全量模式（verbose）全部写出。
#[test]
fn t_pm_03_compact_omits_default() {
    let mut t = SceneTree::new("root");
    let ui = t.add_node(t.root(), "ui", NodeKind::Control);
    t.set_process_mode(ui, ProcessMode::Always);
    t.apply_pending();

    let compact = write_ron(&t, &PackOptions::compact());
    assert!(!compact.contains("process_mode: \"Inherit\""), "缺省不写出：\n{compact}");
    assert!(compact.contains("process_mode: \"Always\""), "非缺省必写出：\n{compact}");

    let verbose = write_ron(&t, &PackOptions::verbose());
    assert!(verbose.contains("process_mode: \"Inherit\""), "全量模式带缺省：\n{verbose}");
}

/// T-PM-04：未知值如实报语义错误 —— 调度语义不是可容忍的前向兼容数据，
/// 不静默回落。
#[test]
fn t_pm_04_unknown_value_is_semantic_error() {
    let bad = r#"Scene(
    version: 1,
    root: Node(
        name: "root",
        kind: "Node",
        process_mode: "Sometimes",
        props: {},
        children: [],
    ),
)"#;
    let err = parse_ron(bad).expect_err("未知模式必须报错");
    assert!(err.to_string().contains("Sometimes"), "指名未知值：{err}");
}
