//! T-Sig 契约回归：信号总线（草案 §12，S6.14）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Sig-01 | on_process 里发射 -> 帧末泵同帧交付（FIFO）；源自动填发射节点；统计计数 |
//! | T-Sig-02 | 级联：处理器再发射 -> 同泵继续交付（迭代，非同步递归）；载荷值语义 |
//! | T-Sig-03 | 处理器的 Cmd（set_local）立即落地且**同帧**进入变换冲洗 |
//! | T-Sig-04 | runaway 级联到上限即丢弃并计数，帧循环不挂起 |
//! | T-Sig-05 | 宿主预发（emit_signal）本帧交付；队列交付后清空不重投 |

use nes_scene::{
    NodeCtx, NodeId, NodeKind, SceneObserver, SceneTree, Signal, SignalCtx, Transform2D, Value,
    SIGNAL_DELIVERY_CAP,
};

fn tree2d() -> (SceneTree, NodeId) {
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "a", NodeKind::Node2D);
    t.apply_pending();
    (t, a)
}

/// 记录交付的信号（名字, 源, 载荷）。
#[derive(Default)]
struct Recorder {
    delivered: Vec<(String, Option<NodeId>, Value)>,
    node: Option<NodeId>,
}

impl SceneObserver for Recorder {
    fn on_ready(&mut self, ctx: &mut NodeCtx<'_>) {
        self.node.get_or_insert(ctx.this());
    }
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if ctx.this() == self.node.unwrap() {
            ctx.emit("ping", Value::I64(7));
        }
    }
    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.delivered
            .push((sig.name.clone(), sig.src, sig.payload.clone()));
    }
}

/// T-Sig-01：同帧交付（FIFO）+ 源 + 统计。
#[test]
fn t_sig_01_emit_in_process_delivered_same_tick() {
    let (mut t, a) = tree2d();
    let mut rec = Recorder { node: Some(a), delivered: Vec::new() };
    let stats = t.tick(0.016, &mut rec);
    assert_eq!(rec.delivered.len(), 1, "process 发射、帧末交付：{:?}", rec.delivered);
    assert_eq!(rec.delivered[0].0, "ping");
    assert_eq!(rec.delivered[0].1, Some(a), "源自动填发射节点");
    assert_eq!(rec.delivered[0].2, Value::I64(7), "载荷值语义");
    assert_eq!(stats.signals_delivered, 1);
    assert_eq!(stats.signals_dropped, 0);
    // 第二帧：不发射的观察者 -> 无交付。
    let mut quiet = Quiet::default();
    let stats2 = t.tick(0.016, &mut quiet);
    assert_eq!(quiet.delivered, 0);
    assert_eq!(stats2.signals_delivered, 0);
}

/// 级联 + 载荷多样性。
#[derive(Default)]
struct Cascader {
    node: Option<NodeId>,
    log: Vec<String>,
}

impl SceneObserver for Cascader {
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if ctx.this() == self.node.unwrap() {
            ctx.emit("a", Value::Str("开始".into()));
        }
    }
    fn on_signal(&mut self, ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.log.push(sig.name.clone());
        match sig.name.as_str() {
            "a" => ctx.emit("b", Value::F32(0.5)),
            "b" => ctx.emit("c", Value::Bool(true)),
            _ => {}
        }
    }
}

/// T-Sig-02：级联同泵交付（a -> b -> c），顺序即发射序；迭代非递归。
#[test]
fn t_sig_02_cascade_delivered_in_order() {
    let (mut t, a) = tree2d();
    let mut cas = Cascader { node: Some(a), log: Vec::new() };
    let stats = t.tick(0.016, &mut cas);
    assert_eq!(cas.log, vec!["a", "b", "c"], "级联按发射序交付");
    assert_eq!(stats.signals_delivered, 3);
}

/// 处理器 Cmd 同帧落地。
#[derive(Default)]
struct Mover {
    node: Option<NodeId>,
    target: Option<NodeId>,
}

impl SceneObserver for Mover {
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if ctx.this() == self.node.unwrap() {
            ctx.emit("go", Value::I64(1));
        }
    }
    fn on_signal(&mut self, ctx: &mut SignalCtx<'_>, sig: &Signal) {
        if sig.name == "go" {
            ctx.set_local(self.target.unwrap(), Transform2D::from_pos(42.0, 0.0));
        }
    }
}

/// T-Sig-03：信号处理器的 set_local 在**同一帧**生效（泵先于变换冲洗）。
#[test]
fn t_sig_03_handler_cmd_lands_same_frame() {
    let (mut t, a) = tree2d();
    let b = t.add_node(t.root(), "b", NodeKind::Node2D);
    t.apply_pending();
    let mut m = Mover { node: Some(a), target: Some(b) };
    let stats = t.tick(0.016, &mut m);
    assert_eq!(stats.signals_delivered, 1);
    // tick 结束即可见（不是下一帧）：世界位置随泵后冲洗生效。
    assert_eq!(t.local(b).expect("local").pos.x, 42.0);
    assert_eq!(t.world_position(b).expect("world").x, 42.0, "同帧进入变换冲洗");
}

/// runaway 级联。
#[derive(Default)]
struct Looper {
    node: Option<NodeId>,
}

impl SceneObserver for Looper {
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if ctx.this() == self.node.unwrap() {
            ctx.emit("loop", Value::I64(0));
        }
    }
    fn on_signal(&mut self, ctx: &mut SignalCtx<'_>, sig: &Signal) {
        if sig.name == "loop" {
            ctx.emit("loop", sig.payload.clone()); // 每收一条再发一条 -> 无限
        }
    }
}

/// 不发射的观察者（对照帧用：只记录交付）。
#[derive(Default)]
struct Quiet {
    delivered: usize,
}

impl SceneObserver for Quiet {
    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, _sig: &Signal) {
        self.delivered += 1;
    }
}

/// T-Sig-04：上限截断 + 丢弃计数 + 帧循环不挂起。
#[test]
fn t_sig_04_runaway_capped_and_counted() {
    let (mut t, a) = tree2d();
    let mut lo = Looper { node: Some(a) };
    let stats = t.tick(0.016, &mut lo);
    assert_eq!(stats.signals_delivered, SIGNAL_DELIVERY_CAP, "恰好交付到上限");
    assert!(stats.signals_dropped >= 1, "丢弃被如实计数：{}", stats.signals_dropped);
    // 泵后队列已清空：下一帧用不发射的观察者 -> 零交付。
    let mut quiet = Quiet::default();
    let stats2 = t.tick(0.016, &mut quiet);
    assert_eq!(stats2.signals_delivered, 0, "残留清空不重投");
}

/// T-Sig-05：宿主预发本帧交付、交付后清空；未 tick 前可查 pending。
#[test]
fn t_sig_05_host_preemit_delivered_once() {
    let (mut t, _a) = tree2d();
    t.emit_signal("boot", Value::Str("hi".into()));
    assert_eq!(t.pending_signals().len(), 1, "未 tick 前可查");

    let mut quiet = Quiet::default();
    let stats = t.tick(0.016, &mut quiet);
    assert_eq!(quiet.delivered, 1, "预发信号本帧交付");
    assert_eq!(stats.signals_delivered, 1);
    assert!(t.pending_signals().is_empty(), "交付后清空");

    let mut quiet2 = Quiet::default();
    let stats2 = t.tick(0.016, &mut quiet2);
    assert_eq!(stats2.signals_delivered, 0, "不重投");
}

// ---------------------------------------------------------------- 信号桥
// S6.15：TreeEvent -> `tree/*` 桥信号（草案 TreeEvent 文档"SignalBus 的上游"）。

/// 收集桥信号（名字 + 事件原文）。
#[derive(Default)]
struct BridgeSpy {
    tree_events: usize,
    signals: Vec<(String, Option<nes_scene::TreeEvent>)>,
}

impl SceneObserver for BridgeSpy {
    fn on_tree_event(&mut self, _tree: &SceneTree, _ev: &nes_scene::TreeEvent) {
        self.tree_events += 1;
    }
    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.signals.push((sig.name.clone(), sig.event.clone()));
    }
}

/// T-Sig-06：桥交付 —— tick 前挂起的结构变更在阶段 1 落地，`on_tree_event`
/// 即时回调与 `tree/added` 桥信号**双通道同时**送达；信号携带事件原文，
/// src = None（引擎源）。
#[test]
fn t_sig_06_bridge_delivers_event_verbatim() {
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "a", NodeKind::Node2D); // 挂起，未落地
    let mut spy = BridgeSpy::default();
    let stats = t.tick(0.016, &mut spy);

    assert_eq!(spy.tree_events, 1, "on_tree_event 照旧");
    assert_eq!(spy.signals.len(), 1, "桥信号帧末交付");
    let (name, event) = &spy.signals[0];
    assert_eq!(name, "tree/added");
    assert_eq!(
        event.as_ref(),
        Some(&nes_scene::TreeEvent::Added {
            node: a,
            parent: t.root(),
        }),
        "事件原文随行"
    );
    assert_eq!(stats.signals_delivered, 1);
}

/// T-Sig-07：全变体映射 + 顺序 —— 一批挂起操作（重名自动调整 + 换位），
/// 桥信号按事件序、名字与 `signal_name` 一一对应。
#[test]
fn t_sig_07_bridge_covers_all_variants_in_order() {
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "a", NodeKind::Node2D);
    let b = t.add_node(t.root(), "a", NodeKind::Node2D); // 重名 -> NameAdjusted
    t.apply_pending(); // 先落地一批（a, a2）

    // 下一批：重命名 + 换位（挂起，等 tick 落地）。
    t.queue(nes_scene::TreeOp::Rename { node: a, name: "hero".to_string() });
    t.queue(nes_scene::TreeOp::Move { node: b, new_index: 0 });
    let mut spy = BridgeSpy::default();
    let stats = t.tick(0.016, &mut spy);

    let names: Vec<&str> = spy.signals.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["tree/renamed", "tree/moved"],
        "桥信号按事件序，名字一一映射：{names:?}"
    );
    // 事件原文可判别（改名含新旧名；换位含下标）。
    assert!(matches!(
        &spy.signals[0].1,
        Some(nes_scene::TreeEvent::Renamed { node, old, new, .. })
            if *node == a && old == "a" && new == "hero"
    ));
    assert!(matches!(
        &spy.signals[1].1,
        Some(nes_scene::TreeEvent::Moved { from: 1, to: 0, .. })
    ));
    assert_eq!(stats.events, 2);
    assert_eq!(stats.signals_delivered, 2);
}

/// 信号处理器做结构变更 -> 下帧桥信号回流（跨帧链路闭环）。
#[derive(Default)]
struct StructuringSpy {
    names: Vec<String>,
    armed: bool,
}

impl SceneObserver for StructuringSpy {
    fn on_signal(&mut self, ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.names.push(sig.name.clone());
        if sig.name == "tree/added" && !self.armed {
            self.armed = true;
            // 借事件原文拿节点，对它排队一次改名（延迟落地）。
            if let Some(nes_scene::TreeEvent::Added { node, .. }) = &sig.event {
                let node = *node;
                ctx.queue(nes_scene::TreeOp::Rename { node, name: "renamed".to_string() });
            }
        }
    }
}

/// T-Sig-08：处理器响应桥信号再改结构 -> 命令延迟到下一帧落地 -> 该帧
/// 桥又发出 `tree/renamed` —— 事件驱动的结构变更跨帧闭环，且不构成
/// runaway（每帧至多一条新事件）。
#[test]
fn t_sig_08_handler_struct_change_reflows_next_frame() {
    let mut t = SceneTree::new("root");
    let _a = t.add_node(t.root(), "a", NodeKind::Node2D); // 挂起

    let mut spy = StructuringSpy::default();
    let stats1 = t.tick(0.016, &mut spy); // 帧 1：added 落地 -> 桥 -> 处理器排 Rename
    assert_eq!(spy.names, vec!["tree/added"]);
    assert_eq!(stats1.signals_delivered, 1);

    let stats2 = t.tick(0.016, &mut spy); // 帧 2：Rename 落地 -> tree/renamed 桥
    assert_eq!(spy.names, vec!["tree/added", "tree/renamed"], "跨帧回流闭环");
    assert_eq!(stats2.signals_delivered, 1);
    assert_eq!(stats2.events, 1);
    assert!(t.find_by_name("renamed").is_some(), "改名已生效");

    let stats3 = t.tick(0.016, &mut spy); // 帧 3：无新事件，无新信号
    assert_eq!(stats3.signals_delivered, 0, "不 runaway");
}

// ---------------------------------------------------------------- 订阅过滤
// S6.16：观察者声明订阅，泵只交付命中项（未命中不进处理器、不耗上限）。

use nes_scene::{SignalFilter, SIGNAL_DELIVERY_CAP as CAP};

/// 只订阅指定名字，记录实际收到的。
struct NameOnly {
    filter: SignalFilter,
    got: Vec<String>,
    node: Option<NodeId>,
}

impl SceneObserver for NameOnly {
    fn signal_filter(&self) -> SignalFilter {
        self.filter.clone()
    }
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if ctx.this() == self.node.unwrap() {
            ctx.emit("go", Value::I64(1));
            ctx.emit("ui/tick", Value::I64(2));
        }
    }
    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.got.push(sig.name.clone());
    }
}

/// T-Sig-09：精确名订阅 —— 只有 "go" 进处理器；桥信号与未订阅用户信号
/// 被过滤计数（对账：delivered + filtered == 总发射）。
#[test]
fn t_sig_09_name_subscription_filters_rest() {
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "a", NodeKind::Node2D); // 挂起 -> 首帧 tree/added
    let mut obs = NameOnly {
        filter: SignalFilter::names(&["go"]),
        got: Vec::new(),
        node: Some(a),
    };
    let stats = t.tick(0.016, &mut obs);
    assert_eq!(obs.got, vec!["go"], "只收到订阅项");
    assert_eq!(stats.signals_delivered, 1);
    assert_eq!(stats.signals_filtered, 2, "tree/added + ui/tick 被滤");
    assert_eq!(stats.signals_dropped, 0);
    // 对账恒等式：交付 + 过滤 == 总发射（3 条：1 桥 + 2 用户）。
    assert_eq!(stats.signals_delivered + stats.signals_filtered, 3);
}

/// T-Sig-10：前缀订阅 —— `tree/` 只收桥信号；用户信号被滤。
struct TreeOnly {
    got: Vec<String>,
    node: Option<NodeId>,
}

impl SceneObserver for TreeOnly {
    fn signal_filter(&self) -> SignalFilter {
        SignalFilter::prefixes(&["tree/"])
    }
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if ctx.this() == self.node.unwrap() {
            ctx.emit("user", Value::I64(0));
        }
    }
    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.got.push(sig.name.clone());
    }
}

#[test]
fn t_sig_10_prefix_subscription_gets_bridge_only() {
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "a", NodeKind::Node2D);
    let mut obs = TreeOnly { got: Vec::new(), node: Some(a) };
    let stats = t.tick(0.016, &mut obs);
    assert_eq!(obs.got, vec!["tree/added"], "前缀命中桥信号");
    assert_eq!(stats.signals_filtered, 1, "user 被滤");
    assert_eq!(stats.signals_delivered, 1);
}

/// T-Sig-11：过滤切断级联 —— handler 只订阅 "x"，收 x 后发射 "y"（未订阅）
/// -> y 不进任何处理器、不再引发发射：交付停在 1，无 runaway；被滤信号
/// 也不消耗上限（对照 CAP）。
struct ChainCut {
    got: Vec<String>,
    node: Option<NodeId>,
}

impl SceneObserver for ChainCut {
    fn signal_filter(&self) -> SignalFilter {
        SignalFilter::names(&["x"])
    }
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if ctx.this() == self.node.unwrap() {
            ctx.emit("x", Value::I64(0));
        }
    }
    fn on_signal(&mut self, ctx: &mut SignalCtx<'_>, _sig: &Signal) {
        self.got.push("x".to_string());
        ctx.emit("y", Value::I64(0)); // 未订阅：到此为止
    }
}

#[test]
fn t_sig_11_filtered_signals_never_cascade_or_burn_cap() {
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "a", NodeKind::Node2D);
    let mut obs = ChainCut { got: Vec::new(), node: Some(a) };
    let stats = t.tick(0.016, &mut obs);
    // 首帧：tree/added 滤 + x 交付 -> 发 y -> y 滤（不再级联）。
    assert_eq!(obs.got.len(), 1, "x 恰好交付一次");
    assert_eq!(stats.signals_delivered, 1);
    assert_eq!(stats.signals_filtered, 2, "tree/added + y 被滤");
    assert!(stats.signals_delivered < CAP, "过滤不消耗上限");
    assert_eq!(stats.signals_dropped, 0);
}

/// NoObserver 的缺省订阅 = NONE：泵只记账不进回调。
#[test]
fn t_sig_12_no_observer_default_is_none() {
    let mut t = SceneTree::new("root");
    let _a = t.add_node(t.root(), "a", NodeKind::Node2D);
    t.emit_signal("boot", Value::Bool(true));
    let stats = t.tick(0.016, &mut nes_scene::NoObserver);
    assert_eq!(stats.signals_delivered, 0, "无回调");
    assert_eq!(stats.signals_filtered, 2, "tree/added + boot 全部过滤记账");
}
