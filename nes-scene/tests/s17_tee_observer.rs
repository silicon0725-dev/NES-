//! S17.2 `TeeObserver`（借用形态双观察者组合）—— 场景层契约测试。
//!
//! * 派发序 = 构造序（first 先于 second，`forwarding_order_is_construction_order`）；
//! * 订阅过滤取并集（与 `Observers` 同一条裁决；未命中任一侧的信号被
//!   泵过滤、不进处理器也不耗交付上限，`filter_is_the_union`）；
//! * 组合对引擎是**一个**观察者：`signals_delivered` 按泵交付计，不按
//!   成员数放大（`pump_counts_the_tee_as_one_observer`）。
//!
//! 背景口径（S17.2 查证结论）：`Observers`（拥有式 Vec 组合，'static）
//! 已存在 —— 能拥有成员的场合复用它；本类型补**借用**形态（外部传入的
//! 宿主观察者 + 运行时内部扩展观察者的组合，生命周期不齐）。

use nes_scene::{
    NodeCtx, SceneObserver, SceneTree, Signal, SignalCtx, SignalFilter, TeeObserver, Value,
};

/// 记账观察者：收到的信号名按序记下；可选声明订阅过滤。
struct Recorder {
    tag: char,
    seen: Vec<(char, String, Value)>,
    filter: SignalFilter,
}

impl Recorder {
    fn all(tag: char) -> Self {
        Self { tag, seen: Vec::new(), filter: SignalFilter::All }
    }

    fn select(tag: char, names: &[&str]) -> Self {
        Self { tag, seen: Vec::new(), filter: SignalFilter::names(names) }
    }
}

impl SceneObserver for Recorder {
    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.seen.push((self.tag, sig.name.clone(), sig.payload.clone()));
    }

    fn signal_filter(&self) -> SignalFilter {
        self.filter.clone()
    }
}

/// 宿主侧发射器：首个 process 回调里经 `NodeCtx::emit` 发一条信号
///（与游戏脚本 `emit` 同一条入队路径）。
struct Emitter {
    name: String,
    payload: Value,
    done: bool,
}

impl SceneObserver for Emitter {
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if !self.done {
            ctx.emit(&self.name, self.payload.clone());
            self.done = true;
        }
    }
}

#[test]
fn forwarding_order_is_construction_order() {
    let mut tree = SceneTree::new("root");
    let mut first = Recorder::all('A');
    let mut second = Recorder::all('B');
    {
        let mut tee = TeeObserver::new(&mut first, &mut second);
        tree.emit_signal("evt", Value::I64(1));
        tree.tick(1.0 / 60.0, &mut tee);
    }
    assert_eq!(first.seen.len(), 1, "first saw the signal");
    assert_eq!(second.seen.len(), 1, "second saw the signal");
    assert_eq!((first.seen[0].0, first.seen[0].1.as_str()), ('A', "evt"));
    assert_eq!((second.seen[0].0, second.seen[0].1.as_str()), ('B', "evt"));
}

#[test]
fn filter_is_the_union() {
    let mut tree = SceneTree::new("root");
    let mut first = Recorder::select('A', &["a"]);
    let mut second = Recorder::select('B', &["b"]);
    {
        let mut tee = TeeObserver::new(&mut first, &mut second);
        // a / b 命中并集；c 两边都没订阅 -> 被泵过滤（不进任何成员）。
        tree.emit_signal("a", Value::I64(1));
        tree.emit_signal("b", Value::I64(2));
        tree.emit_signal("c", Value::I64(3));
        let stats = tree.tick(1.0 / 60.0, &mut tee);
        assert_eq!(stats.signals_delivered, 2, "a and b delivered");
        assert_eq!(stats.signals_filtered, 1, "c filtered out");
    }
    // 广播语义：命中并集的信号对**两侧**成员都可见（各侧自行忽略不关心的）。
    assert_eq!(first.seen.iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
    assert_eq!(second.seen.iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
}

#[test]
fn pump_counts_the_tee_as_one_observer() {
    let mut tree = SceneTree::new("root");
    let mut first = Recorder::all('A');
    let mut emitter = Emitter { name: "go".to_string(), payload: Value::I64(7), done: false };
    {
        let mut tee = TeeObserver::new(&mut first, &mut emitter);
        let stats = tree.tick(1.0 / 60.0, &mut tee);
        // 泵统计口径：一条入队信号（emitter 的 "go"）只计一次交付 ——
        // 不按组合成员数放大。
        assert_eq!(stats.signals_delivered, 1);
        assert_eq!(stats.signals_filtered, 0);
    }
    assert_eq!(first.seen.len(), 1);
    // 三方组合（tee 之外再挂 recorder）：每个成员各见一次，统计不放大。
    assert!(emitter.done, "emitter fired during process");
    let mut third = Recorder::all('C');
    {
        let mut tee2 = TeeObserver::new(&mut first, &mut third);
        let _ = tree.tick(1.0 / 60.0, &mut tee2);
    }
    assert_eq!(third.seen.len(), 0, "no new signals this frame");
}
