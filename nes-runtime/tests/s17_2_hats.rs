//! S17.2 hat 触发（事件重入）—— 契约测试（扫描文档 P1 块的落地验收）。
//!
//! * T-HAT-01：游戏侧 emit -> JS hat 收到（载荷值断言；in-tick 重入 =
//!   裁决 A，hat 里的写当步落地）；
//! * T-HAT-02：JS hat 内 emitSignal -> 游戏侧收到（反向桥；泵内 = 同泵
//!   级联）、update 期 emitSignal = 下帧泵（两种时序都钉住）；
//! * T-HAT-03：级联深度 3（s1->s2->s3->s4，每跳 = 一次 QuickJS 序贯重入）
//!   不炸、次序确定；同名多 handler 注册序；
//! * T-HAT-04：hat 内 throw -> fault 计数 + 诊断，同表后续 handler 与
//!   其余扩展照常（S17.1 隔离继承），引擎下一帧照活；
//! * T-HAT-05：死循环 hat -> 50ms 预算中断、泵存活、其余扩展照常；
//! * T-HAT-06：无 hat 注册 = 零开销（闸读数 false，tick 不组装扩展观察者，
//!   基线行为不变）。
//!
//! **QuickJS 重入实测结论**（T-HAT-02/03 即证据）：泵 -> JS -> 泵 -> JS
//! 的重入是序贯的 —— 每次派发都是独立的一次 `rt.call`，hat 的发射经缓冲
//! 在两次派发之间取走，不存在嵌套 QuickJS 栈帧（裁决 A 因此不依赖嵌套
//! 重入支持，rquickjs 的 RefCell 能力闭包在序贯重入下无冲突）。
//!
//! JS 字面量全 ASCII（仓库纪律）；真设备/真文件不需要 —— 源码内联。

use std::time::{Duration, Instant};

use nes_runtime::NesRuntime;
use nes_scene::{NodeCtx, NodeKind, SceneObserver, Signal, SignalCtx, Value};

/// 宿主探针观察者：可选地在**首个** process 回调里发射一条信号（与游戏
/// 脚本 `emit` 同一条 `NodeCtx::emit` 入队路径），并记录广播收到的信号。
struct HostProbe {
    emit: Option<(String, Value)>,
    emitted: bool,
    seen: Vec<(String, Value)>,
}

impl HostProbe {
    /// 发射一条信号的探针（emit-once —— 确定性断言的基础）。
    fn emitting(name: &str, payload: Value) -> Self {
        Self {
            emit: Some((name.to_string(), payload)),
            emitted: false,
            seen: Vec::new(),
        }
    }

    /// 纯监听探针。
    fn listening() -> Self {
        Self { emit: None, emitted: false, seen: Vec::new() }
    }

    /// 按名提取收到的载荷（断言辅助；广播里还混有内建 tick 等信号）。
    fn payloads_of(&self, name: &str) -> Vec<Value> {
        self.seen
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .collect()
    }
}

impl SceneObserver for HostProbe {
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if let Some((name, payload)) = self.emit.take() {
            ctx.emit(&name, payload);
            self.emitted = true;
        }
    }

    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.seen.push((sig.name.clone(), sig.payload.clone()));
    }
}

/// 搭一台 headless 引擎：root + log 节点 + 内联 JS 扩展（临时目录落盘装载
/// —— 真实 `load_extension_file` 路径）。
fn engine_with(tag: &str, exts: &[(&str, &str)]) -> NesRuntime {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("s17_2_{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut rt = NesRuntime::open_headless(&root).expect("headless engine");
    {
        let tree = rt.tree_mut();
        let r = tree.root();
        tree.add_node(r, "log", NodeKind::Node);
        // add_node 是 pending 意图 —— 就地落地，让随后的快照预热见得到
        //（真引擎在 tick 帧首做同一件事；这里不走 tick 就地落地）。
        tree.apply_pending();
    }
    for (name, source) in exts {
        let path = root.join(format!("{name}.js"));
        std::fs::write(&path, source).unwrap();
        let id = rt.load_extension_file(&path).expect(name);
        assert_eq!(id, *name, "extension must self-report its id");
    }
    // 预热一帧扩展面：刷新读快照（hat 里 scene.find 才能见到搭好的树）。
    rt.update_extensions();
    rt
}

/// 推进一帧：headless step（内含 hat 接线的 tick 咽喉 + 泵内写当步落地）
/// + 扩展 update 面（update 期发射在尾部落树 —— 下帧泵交付）。
fn step(rt: &mut NesRuntime, probe: &mut HostProbe) {
    let _ = rt.step_headless(1.0 / 60.0, probe);
    rt.update_extensions();
}

/// T-HAT-01：游戏侧 emit -> JS hat 收到，载荷值保真（I64(42) -> JS 42）。
#[test]
fn t_hat_01_game_emit_reaches_js_hat_with_payload() {
    const HAT: &str = r#"
nes.registerExtension("hat1");
nes.onSignal("enemy-died", function (payload) {
  var ref = nes.scene.find("log");
  if (ref === null) { return; }
  var ok = (typeof payload === "number") && (payload === 42);
  nes.node.setPos(ref, ok ? 42 : -1, 0);
});
"#;
    let mut rt = engine_with("hat1", &[("hat1", HAT)]);
    let mut probe = HostProbe::emitting("enemy-died", Value::I64(42));
    step(&mut rt, &mut probe);

    {
        let tree = rt.tree_mut();
        let log = tree.find_by_name("log").expect("log 在场");
        let local = tree.local(log).expect("log 变换");
        // in-tick 重入（裁决 A）：hat 在泵内同步跑，写当步落地 —— 同一帧
        // 断言即可（无滞后）。
        assert_eq!(local.pos.x, 42.0, "JS hat must receive payload 42");
        assert_eq!(local.pos.y, 0.0);
    }
    // 宿主广播也照常收到（TeeObserver 两侧都投递）。
    assert_eq!(probe.payloads_of("enemy-died").len(), 1);
    assert!(probe.emitted);
    assert_eq!(rt.extension_faults(), 0);
}

/// T-HAT-02：JS hat 内 emitSignal -> 游戏侧收到（泵内 = 同泵级联）；
/// update 期 emitSignal = 下帧泵（两种发射时序都钉住）。
#[test]
fn t_hat_02_js_emit_reaches_game_side_both_timings() {
    const RELAY: &str = r#"
nes.registerExtension("relay");
nes.onSignal("enemy-died", function (payload) {
  nes.emitSignal("hat-seen", payload);
});
var fired = false;
nes.onUpdate(function () {
  if (!fired) { nes.emitSignal("from-update", 9); fired = true; }
});
"#;
    let mut rt = engine_with("hat2", &[("relay", RELAY)]);
    let mut probe = HostProbe::emitting("enemy-died", Value::I64(42));

    // 泵内路径（裁决 A）：hat 的发射在派发后取走，落回同一泵。
    // （setup 预热的 update 面已发射过一次 from-update，正好落在 step 1
    // 的泵里 —— 这本身就是"update 期发射 = 下帧泵"的第一次实证。）
    step(&mut rt, &mut probe);
    let pump_payloads = probe.payloads_of("hat-seen");
    assert_eq!(pump_payloads, vec![Value::I64(42)], "pump-cascade payload must round-trip");
    assert_eq!(
        probe.payloads_of("from-update"),
        vec![Value::I64(9)],
        "primer's update-phase emit lands in this frame's pump (next-pump timing)"
    );

    // 唯一一次 update 期发射已被上一帧消费：后续帧不得再见到它
    //（不重放、不滞留 —— 单帧时序的另一半）。
    let mut probe2 = HostProbe::listening();
    step(&mut rt, &mut probe2);
    assert!(
        probe2.payloads_of("from-update").is_empty(),
        "a consumed update-phase emit must not replay"
    );
    assert_eq!(rt.extension_faults(), 0);
}

/// T-HAT-03：级联深度 3（s1 -> s2 -> s3 -> s4，每跳一次 QuickJS 序贯重入）
/// 不炸、次序确定；同名多 handler 按注册序都调（发射序可观测）。
#[test]
fn t_hat_03_cascade_depth_three_is_ordered_and_bounded() {
    const CHAIN: &str = r#"
nes.registerExtension("chain");
nes.onSignal("s1", function (p) { nes.emitSignal("s2", p + 1); });
nes.onSignal("s2", function (p) { nes.emitSignal("s3", p + 1); });
nes.onSignal("s3", function (p) { nes.emitSignal("s4", p + 1); });
nes.onSignal("m", function () { nes.emitSignal("m2", 1); });
nes.onSignal("m", function () { nes.emitSignal("m2", 2); });
"#;
    let mut rt = engine_with("hat3", &[("chain", CHAIN)]);

    // 深度 3：一次发射，四级信号，三级级联 —— QuickJS 重入实测半边。
    let mut probe = HostProbe::emitting("s1", Value::I64(0));
    step(&mut rt, &mut probe);
    assert_eq!(
        probe.payloads_of("s2"),
        vec![Value::I64(1)],
        "hop 1 (pump -> JS -> pump)"
    );
    assert_eq!(probe.payloads_of("s3"), vec![Value::I64(2)], "hop 2");
    assert_eq!(probe.payloads_of("s4"), vec![Value::I64(3)], "hop 3");
    assert_eq!(rt.extension_faults(), 0, "cascade must not fault");

    // 同名双 handler：同一次派发内注册序执行，两笔发射按提交序入泵。
    let mut probe2 = HostProbe::emitting("m", Value::I64(0));
    step(&mut rt, &mut probe2);
    assert_eq!(
        probe2.payloads_of("m2"),
        vec![Value::I64(1), Value::I64(2)],
        "registration order must be observable"
    );
    assert_eq!(rt.extension_faults(), 0);
}

/// T-HAT-04：hat 内 throw -> fault 计数 + 诊断（异常文本随行）；同表后续
/// handler 与其余扩展照常；引擎下一帧照活（S17.1 隔离继承）。
#[test]
fn t_hat_04_throwing_hat_is_isolated_and_counted() {
    const THROWER: &str = r#"
nes.registerExtension("throwhat");
nes.onSignal("boom", function () { throw new Error("hat-boom"); });
nes.onSignal("boom", function (p) { nes.emitSignal("boom-after", p); });
"#;
    const GOOD: &str = r#"
nes.registerExtension("goodhat");
nes.onSignal("boom", function () { nes.emitSignal("boom-good", 7); });
"#;
    let mut rt = engine_with("hat4", &[("throwhat", THROWER), ("goodhat", GOOD)]);
    let mut probe = HostProbe::emitting("boom", Value::I64(1));
    step(&mut rt, &mut probe);

    // fault 计数 + 诊断（扩展 id + 异常文本）。
    assert!(rt.extension_faults() >= 1, "throw must count as a fault");
    let last = rt.last_fault().expect("fault recorded");
    assert!(last.contains("throwhat"), "ext id missing: {last}");
    assert!(last.contains("hat-boom"), "exception text missing: {last}");

    // 同表后续 handler 照跑（蹦床逐 handler try/catch）。
    assert_eq!(
        probe.payloads_of("boom-after"),
        vec![Value::I64(1)],
        "later handler in the same table must still run"
    );
    // 其余扩展照常（派发循环不因单扩展异常中断）。
    assert_eq!(
        probe.payloads_of("boom-good"),
        vec![Value::I64(7)],
        "other extensions must still be dispatched"
    );

    // 引擎下一帧照活：再发一次，健康扩展照常应答、故障照常计数。
    let mut probe2 = HostProbe::emitting("boom", Value::I64(2));
    step(&mut rt, &mut probe2);
    assert_eq!(probe2.payloads_of("boom-good"), vec![Value::I64(7)]);
    assert!(rt.extension_faults() >= 2, "faults keep counting");
}

/// T-HAT-05：死循环 hat -> 50ms 预算中断（uncatchable，直接浮出）、
/// 泵存活、其余扩展照常（防线 2 继承）。
#[test]
fn t_hat_05_deadloop_hat_is_interrupted_and_pump_survives() {
    const DEAD: &str = r#"
nes.registerExtension("deadhat");
nes.onSignal("boom", function () { while (true) { } });
"#;
    const GOOD: &str = r#"
nes.registerExtension("goodhat2");
nes.onSignal("boom", function () { nes.emitSignal("boom-good", 7); });
"#;
    let mut rt = engine_with("hat5", &[("deadhat", DEAD), ("goodhat2", GOOD)]);
    let mut probe = HostProbe::emitting("boom", Value::I64(1));
    let start = Instant::now();
    step(&mut rt, &mut probe);
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "deadloop must be cut within budget, took {elapsed:?}"
    );
    let last = rt.last_fault().expect("interrupt must surface as a fault");
    assert!(last.contains("interrupted"), "interrupt marker missing: {last}");
    assert!(last.contains("deadhat"), "ext id missing: {last}");

    // 派发循环继续：死循环扩展之后的扩展照常收到信号。
    assert_eq!(
        probe.payloads_of("boom-good"),
        vec![Value::I64(7)],
        "dispatch must continue past the interrupted extension"
    );

    // 泵存活：下一帧 tick 照常、树状态可读写。
    let mut probe2 = HostProbe::emitting("boom", Value::I64(2));
    step(&mut rt, &mut probe2);
    assert_eq!(probe2.payloads_of("boom-good"), vec![Value::I64(7)]);
    {
        let tree = rt.tree_mut();
        assert_eq!(tree.name(tree.root()), Some("root"));
    }
}

/// T-HAT-06：无 hat 注册 = 零开销 —— 闸读数 false（tick 不组装扩展观察者，
/// 路径与 S17.1 基线逐位一致）；注册 hat 后读数翻真（闸生效的证据）。
#[test]
fn t_hat_06_no_hats_registered_keeps_the_baseline_path() {
    const SPINNER: &str = r#"
nes.registerExtension("spin6");
var n = 0;
nes.onUpdate(function () { n = n + 1; });
"#;
    let mut rt = engine_with("hat6", &[("spin6", SPINNER)]);
    // 有扩展、无 hat：闸 false —— 泵不进扩展，基线路径。
    assert!(!rt.extension_has_signal_hats(), "gate must be closed without hats");

    let mut probe = HostProbe::emitting("plain", Value::I64(5));
    step(&mut rt, &mut probe);
    // 基线行为不变：宿主广播照常一次；扩展零参与（无 fault、无派发副作用）。
    assert_eq!(probe.payloads_of("plain"), vec![Value::I64(5)]);
    assert_eq!(rt.extension_faults(), 0);

    // 注册 hat：闸翻真 —— 下一帧泵开始派发（闸生效的直接证据）。
    const HAT: &str = r#"
nes.registerExtension("hat6b");
nes.onSignal("plain", function () { nes.emitSignal("hat6-echo", 1); });
"#;
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("s17_2_hat6");
    let path = root.join("hat6b.js");
    std::fs::write(&path, HAT).unwrap();
    let id = rt.load_extension_file(&path).unwrap();
    assert_eq!(id, "hat6b");
    assert!(rt.extension_has_signal_hats(), "gate must open after onSignal");
    rt.update_extensions(); // 刷新快照（订阅在装载期已进能力桥）

    let mut probe2 = HostProbe::emitting("plain", Value::I64(6));
    step(&mut rt, &mut probe2);
    assert_eq!(
        probe2.payloads_of("hat6-echo"),
        vec![Value::I64(1)],
        "hat registered mid-session must receive the next delivery"
    );
    assert_eq!(rt.extension_faults(), 0);
}

/// 载荷映射边界（nes -> 树方向）的单元面：整数值 -> I64、小数 -> F32、
/// {x,y} 对象 -> Vec2、其余对象 P0 拒绝、Null -> 占位。
#[test]
fn payload_conversion_boundary_is_explicit() {
    use nes_runtime::{nes_to_value, value_to_nes};
    use nes_scene::Value;

    // 出界：树 -> JS。
    assert_eq!(value_to_nes(&Value::I64(42)), nes_extension_api::NesValue::F64(42.0));
    assert_eq!(value_to_nes(&Value::F32(1.5)), nes_extension_api::NesValue::F64(1.5));
    assert_eq!(value_to_nes(&Value::Bool(true)), nes_extension_api::NesValue::Bool(true));
    assert_eq!(
        value_to_nes(&Value::Str("hi".into())),
        nes_extension_api::NesValue::str("hi")
    );

    // 入界：JS -> 树。
    assert_eq!(nes_to_value(&nes_extension_api::NesValue::F64(42.0)), Some(Value::I64(42)));
    assert_eq!(nes_to_value(&nes_extension_api::NesValue::F64(1.5)), Some(Value::F32(1.5)));
    assert_eq!(
        nes_to_value(&nes_extension_api::NesValue::obj([
            ("x", nes_extension_api::NesValue::F64(3.0)),
            ("y", nes_extension_api::NesValue::F64(4.0)),
        ])),
        Some(Value::Vec2(nes_scene::Vec2::new(3.0, 4.0)))
    );
    // P0 拒绝：非 {x,y} 对象不落树（返回 None，落地处计数）。
    assert_eq!(
        nes_to_value(&nes_extension_api::NesValue::obj([
            ("a", nes_extension_api::NesValue::F64(1.0)),
        ])),
        None
    );
    // Null -> Bool(true) 占位（与树桥信号同一约定）。
    assert_eq!(nes_to_value(&nes_extension_api::NesValue::Null), Some(Value::Bool(true)));
}
