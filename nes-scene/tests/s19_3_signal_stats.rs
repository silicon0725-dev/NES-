//! S19.3 契约回归：信号送达计数表（SIGNALS 面板的数据面）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-SS-01 | 直发：宿主 emit_signal -> 泵交付即计数；未交付帧不计数 |
//! | T-SS-02 | 级联各计：处理器再发射回同泵，逐条各计；读面降序 + 同计数字典序 |
//! | T-SS-03 | 路由计次：每条命中连接各计一（观察者交付 + 方法级分发同口径）|
//! | T-SS-04 | 不交付不计：订阅过滤（NoObserver 口径）的信号不进计数表 |
//! | T-SS-05 | 指纹不受影响：同轨迹两跑，计数表一空一满，逐帧指纹逐位同 |

use nes_scene::{
    scene_fingerprint, NoObserver, NodeCtx, NodeKind, SceneObserver, SceneTree, Signal, SignalCtx,
    Value,
};

/// 空场景（只有根；计数测试不需要结构）。
fn bare_tree() -> SceneTree {
    SceneTree::new("root")
}

/// 只记录交付名的观察者（零副作用 —— 指纹对照跑的行为面）。
#[derive(Default)]
struct NamedRecorder {
    delivered: Vec<String>,
}

impl SceneObserver for NamedRecorder {
    fn on_process(&mut self, _ctx: &mut NodeCtx<'_>, _delta: f32) {}
    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.delivered.push(sig.name.clone());
    }
}

/// T-SS-01：宿主预发 -> 帧末泵交付即计数；无发射的帧不增长。
#[test]
fn t_ss_01_host_emit_counted_on_delivery() {
    let mut t = bare_tree();
    assert!(t.signal_stats_sorted().is_empty(), "初始计数表为空");
    t.emit_signal("ping", Value::I64(1));
    // 预发未 tick = 未交付，不计数。
    assert!(t.signal_stats_sorted().is_empty(), "未交付不计数");
    let mut obs = Noop;
    let stats = t.tick(0.016, &mut obs);
    assert_eq!(stats.signals_delivered, 1);
    assert_eq!(t.signal_stats_sorted(), vec![("ping".to_string(), 1u64)]);
    // 同帧双发 = 两次交付两次计。
    t.emit_signal("ping", Value::I64(2));
    t.emit_signal("ping", Value::I64(3));
    t.tick(0.016, &mut obs);
    assert_eq!(t.signal_stats_sorted(), vec![("ping".to_string(), 3u64)]);
    // 静默帧：不发射就不增长（计数挂在交付沿，不挂在帧沿）。
    t.tick(0.016, &mut obs);
    assert_eq!(t.signal_stats_sorted(), vec![("ping".to_string(), 3u64)]);
}

/// 零副作用观察者（指纹对照跑用）。
#[derive(Default)]
struct Noop;

impl SceneObserver for Noop {
    fn on_process(&mut self, _ctx: &mut NodeCtx<'_>, _delta: f32) {}
}

/// 级联观察者：process 发 a；收到 a 发 b；收到 b 发 c（T-Sig-02 同款链）。
#[derive(Default)]
struct Cascader {
    node: Option<nes_scene::NodeId>,
}

impl SceneObserver for Cascader {
    fn on_ready(&mut self, ctx: &mut NodeCtx<'_>) {
        self.node.get_or_insert(ctx.this());
    }
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if Some(ctx.this()) == self.node {
            ctx.emit("a", Value::I64(1));
        }
    }
    fn on_signal(&mut self, ctx: &mut SignalCtx<'_>, sig: &Signal) {
        match sig.name.as_str() {
            "a" => ctx.emit("b", Value::I64(2)),
            "b" => ctx.emit("c", Value::I64(3)),
            _ => {}
        }
    }
}

/// T-SS-02：级联各计（a/b/c 各一次交付各一计）+ 排序口径
/// （计数降序；同计数按名字典序）。
#[test]
fn t_ss_02_cascade_counted_per_delivery_sorted() {
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "a", NodeKind::Node2D);
    t.apply_pending();
    let mut obs = Cascader { node: Some(a) };
    t.tick(0.016, &mut obs);
    // 三条各计一次；计数并列 -> 字典序 a, b, c。
    assert_eq!(
        t.signal_stats_sorted(),
        vec![
            ("a".to_string(), 1u64),
            ("b".to_string(), 1u64),
            ("c".to_string(), 1u64),
        ]
    );
    // 计数差异 -> 降序在前（ping 3 次 vs pong 1 次）。
    let mut t2 = bare_tree();
    let mut noop = Noop;
    for _ in 0..3 {
        t2.emit_signal("ping", Value::I64(0));
        t2.tick(0.016, &mut noop);
    }
    t2.emit_signal("pong", Value::I64(0));
    t2.tick(0.016, &mut noop);
    assert_eq!(
        t2.signal_stats_sorted(),
        vec![("ping".to_string(), 3u64), ("pong".to_string(), 1u64)]
    );
}

/// T-SS-03：路由交付计次 —— 广播一次 + 每条命中连接各一次；
/// 方法级分发（不经观察者）同样计。
#[test]
fn t_ss_03_routed_deliveries_counted_per_connection() {
    let mut t = SceneTree::new("root");
    let dst = t.add_node(t.root(), "dst", NodeKind::Node2D);
    t.apply_pending();
    // 双连接同目标（观察者交付位）：一条 "hit" -> 广播 1 + 路由 2 = 计 3。
    t.connect_signal("hit", None, dst);
    t.connect_signal("hit", None, dst);
    let mut rec = NamedRecorder::default();
    t.emit_signal("hit", Value::Bool(true));
    let stats = t.tick(0.016, &mut rec);
    assert_eq!(stats.signals_delivered, 3, "广播 1 + 路由 2");
    assert_eq!(stats.signals_routed, 2);
    assert_eq!(t.signal_stats_sorted(), vec![("hit".to_string(), 3u64)]);
    // 方法级分发：处理器不经观察者照常计。本帧观察者换 NoObserver
    //（订阅全滤）—— 广播不交付不计数，路由交付照常发生（S6.19：显式
    // 接线不受订阅过滤影响）且计一。
    let calls = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let calls2 = std::rc::Rc::clone(&calls);
    t.set_signal_handler(
        dst,
        "on_hit",
        Box::new(move |_ctx: &mut SignalCtx<'_>, _sig: &Signal| {
            calls2.set(calls2.get() + 1);
        }),
    );
    t.connect_signal_to("meth", None, dst, "on_hit");
    t.emit_signal("meth", Value::Bool(true));
    t.tick(0.016, &mut NoObserver);
    assert_eq!(calls.get(), 1, "方法级处理器真实调用");
    assert_eq!(
        t.signal_stats_sorted(),
        vec![("hit".to_string(), 3u64), ("meth".to_string(), 1u64)]
    );
}

/// T-SS-04：不交付不计数 —— 观察者订阅全滤（NoObserver 口径）且无连接
/// 的信号只进 signals_filtered，计数表保持为空。
#[test]
fn t_ss_04_filtered_delivery_not_counted() {
    let mut t = bare_tree();
    t.emit_signal("ghost", Value::I64(9));
    let stats = t.tick(0.016, &mut NoObserver);
    assert_eq!(stats.signals_delivered, 0);
    assert_eq!(stats.signals_filtered, 1, "订阅全滤：只记账不交付");
    assert!(t.signal_stats_sorted().is_empty(), "未交付不进计数表");
}

/// T-SS-05：计数表不进语义指纹 —— 同一轨迹跑两遍：场景经 to_doc ->
/// instantiate_doc 双实例化（uid 随文档往返，两跑身份逐位同源），跑 A
/// 全收（NamedRecorder 缺省订阅 = All，零副作用，计数表累积），跑 B
/// NoObserver（订阅全滤，计数表恒空）。两跑树面语义同轨，逐帧指纹
/// 必须逐位相同；差异的只有计数表本身 —— 即计数不影响指纹。
#[test]
fn t_ss_05_signal_stats_outside_fingerprint() {
    use nes_scene::scene_io;
    // 双跑共源：同一棵装配树 -> 文档 -> 两个实例（uid 逐位一致）。
    let proto = SceneTree::new("root");
    let doc = scene_io::to_doc(&proto);
    let mut run_a = scene_io::instantiate_doc(&doc).expect("实例化 A");
    let mut run_b = scene_io::instantiate_doc(&doc).expect("实例化 B");
    let script = ["alpha", "beta", "alpha", "gamma", "beta"];
    let mut rec = NamedRecorder::default();
    for &name in script.iter() {
        run_a.emit_signal(name, Value::I64(1));
        run_b.emit_signal(name, Value::I64(1));
        run_a.tick(0.016, &mut rec);
        run_b.tick(0.016, &mut NoObserver);
        let fa = scene_fingerprint(&run_a, None);
        let fb = scene_fingerprint(&run_b, None);
        assert_eq!(fa, fb, "帧 {} 指纹逐位同（计数表一空一满）", run_a.frame());
    }
    // 前提自证：A 的计数表确实累积了（跑法有效，不是两边都空）。
    assert_eq!(rec.delivered.len(), script.len());
    assert_eq!(
        run_a.signal_stats_sorted(),
        vec![
            ("alpha".to_string(), 2u64),
            ("beta".to_string(), 2u64),
            ("gamma".to_string(), 1u64),
        ]
    );
    assert!(run_b.signal_stats_sorted().is_empty());
}
