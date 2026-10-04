//! S17.3 双件（C4 协程让出 + B3 权限模型）—— 引擎级契约测试。
//!
//! * T-COR-01：普通 onUpdate 回归照旧（每帧推进）；生成器 onUpdate：
//!   `yield 2` -> 推进周期 3 帧（推进帧 + 两帧停顿），逐帧位置断言；
//! * T-COR-02：生成器 hat —— 触发即推首段（in-tick，同帧可见 = S17.2
//!   裁决 A 继承）+ `yield 3` + 续跑完成；同 hat 二次触发 = 新实例并发
//!   （两实例各自推进互不干扰；完成序 = 生成序）；上限 32 的引擎级
//!   面板 = 自级联 hat 自我繁殖到 33 实例时拒新 + fault 计数 + 泵存活；
//! * T-COR-03：生成器续跑段 throw -> fault 隔离（S17.1 语义：计数 +
//!   诊断、坏实例出表、其余扩展与引擎照常）；
//! * T-PERM-01：声明无 "audio" -> `nes.audio.play` 抛 permission denied
//!   -> fault 计数 + 诊断（无声音副作用的绑定半边见 nes-extension-js
//!   `tests/perms_mock.rs`）；已授予能力照常工作；
//! * T-PERM-02：声明全五项 -> 全能力照旧；缺省（无声明）= 全授予回归
//!   （hello.js 兼容，ext_demo 冒烟同证）；
//! * T-PERM-03：onSignal / emitSignal 需 "signal" —— 未授予分别报错
//!   （注册期 = 装载失败且订阅不进泵过滤器；发射期 = fault 计数）。
//!
//! JS 字面量全 ASCII（仓库纪律）；生成器推进在同一次 `rt.call` 内 =
//! ExecBudget 预算继承（S17.1 三道防线全数覆盖两条新路径）。

use nes_render_api::input::Key;
use nes_runtime::NesRuntime;
use nes_scene::{NodeCtx, NodeKind, SceneObserver, Signal, SignalCtx, Transform2D, Value};

/// 宿主探针：发射队列（每次 on_process 回调弹一条 —— 与游戏脚本 `emit`
/// 同一条 `NodeCtx::emit` 入队路径）+ 广播记录。
struct QueueProbe {
    emissions: Vec<(String, Value)>,
    seen: Vec<(String, Value)>,
}

impl QueueProbe {
    fn emitting(queue: Vec<(&str, Value)>) -> Self {
        Self {
            emissions: queue
                .into_iter()
                .map(|(n, v)| (n.to_string(), v))
                .collect(),
            seen: Vec::new(),
        }
    }

    fn listening() -> Self {
        Self { emissions: Vec::new(), seen: Vec::new() }
    }

    /// 按名提取收到的载荷（断言辅助）。
    fn payloads_of(&self, name: &str) -> Vec<Value> {
        self.seen
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .collect()
    }
}

impl SceneObserver for QueueProbe {
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if !self.emissions.is_empty() {
            let (name, v) = self.emissions.remove(0);
            ctx.emit(&name, v);
        }
    }

    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.seen.push((sig.name.clone(), sig.payload.clone()));
    }
}

/// 搭一台 headless 引擎：root + 具名节点（带初始位置）+ 内联 JS 扩展
///（临时目录落盘装载 —— 真实 `load_extension_file` 路径）+ 预热一帧
///（读快照就绪）。
fn engine_with(tag: &str, nodes: &[(&str, f32, f32)], exts: &[(&str, &str)]) -> NesRuntime {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("s17_3_{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut rt = NesRuntime::open_headless(&root).expect("headless engine");
    {
        let tree = rt.tree_mut();
        let r = tree.root();
        for (name, x, y) in nodes {
            let id = tree.add_node(r, name, NodeKind::Node);
            tree.set_local(id, Transform2D::from_pos(*x, *y));
        }
        // add_node 是 pending 意图 —— 就地落地（真引擎在 tick 帧首做同一件事）。
        tree.apply_pending();
    }
    for (name, source) in exts {
        let path = root.join(format!("{name}.js"));
        std::fs::write(&path, source).unwrap();
        let id = rt.load_extension_file(&path).expect(name);
        assert_eq!(id, *name, "extension must self-report its id");
    }
    // 预热一帧扩展面：刷新读快照（生成器首段 / hat 首段的 find 才见得到树）。
    rt.update_extensions();
    rt
}

/// 推进一帧：headless step（内含 hat 接线的 tick 咽喉 + 泵内写当步落地）
/// + 扩展 update 面（生成器协程在此帧驱动；update 期发射尾部落树）。
fn step(rt: &mut NesRuntime, probe: &mut QueueProbe) {
    let _ = rt.step_headless(1.0 / 60.0, probe);
    rt.update_extensions();
}

/// 读某节点当前位置（断言辅助）。
fn pos_of(rt: &mut NesRuntime, name: &str) -> (f32, f32) {
    let tree = rt.tree_mut();
    let id = tree.find_by_name(name).unwrap_or_else(|| panic!("{name} must exist"));
    let t = tree.local(id).expect("local transform");
    (t.pos.x, t.pos.y)
}

/// T-COR-01：普通 onUpdate 回归（每帧 +1）+ 生成器 onUpdate `yield 2`
/// （推进周期 3 帧）。两条路径同场逐帧断言。
#[test]
fn t_cor_01_generator_update_yield_two_advances_on_a_three_frame_cycle() {
    const NORMAL: &str = r#"
nes.registerExtension("spin");
nes.onUpdate(function () {
  var r = nes.scene.find("obj1");
  var p = nes.node.getPos(r);
  nes.node.setPos(r, p[0] + 1, p[1]);
});
"#;
    const GENERATOR: &str = r#"
nes.registerExtension("stepper");
function* stepper() {
  var r = nes.scene.find("obj2");
  while (true) {
    var p = nes.node.getPos(r);
    nes.node.setPos(r, p[0] + 10, p[1]);
    yield 2;
  }
}
nes.onUpdate(stepper);
"#;
    let mut rt = engine_with(
        "cor1",
        &[("obj1", 100.0, 200.0), ("obj2", 0.0, 0.0)],
        &[("spin", NORMAL), ("stepper", GENERATOR)],
    );
    // 预热帧（引擎内含）：普通钩子推进一次；生成器 spawn + 首段立即推进。
    assert_eq!(pos_of(&mut rt, "obj1"), (101.0, 200.0), "normal onUpdate runs at prewarm");
    assert_eq!(pos_of(&mut rt, "obj2"), (10.0, 0.0), "generator first segment runs at spawn");

    let mut probe = QueueProbe::listening();
    let expected: [(f32, f32); 6] = [
        (102.0, 10.0), // f1: wait 2->1（停顿）
        (103.0, 10.0), // f2: wait 1->0（停顿；归零当帧仍停）
        (104.0, 20.0), // f3: 推进（第二次 +10）—— 周期恰 3 帧
        (105.0, 20.0), // f4: 停顿
        (106.0, 20.0), // f5: 停顿
        (107.0, 30.0), // f6: 推进（第三次 +10）
    ];
    for (i, (want_obj1, want_obj2)) in expected.into_iter().enumerate() {
        step(&mut rt, &mut probe);
        assert_eq!(pos_of(&mut rt, "obj1"), (want_obj1, 200.0), "frame {}: normal per-frame", i + 1);
        assert_eq!(pos_of(&mut rt, "obj2"), (want_obj2, 0.0), "frame {}: generator cycle", i + 1);
    }
    assert_eq!(rt.extension_faults(), 0, "both paths must be clean");
}

/// T-COR-02：生成器 hat —— 触发即推首段（in-tick 同帧可见）+ yield 3 +
/// 续跑完成；同 hat 二次触发 = 新实例并发。
#[test]
fn t_cor_02_generator_hat_instances_are_concurrent_and_frame_driven() {
    const HAT: &str = r#"
nes.registerExtension("hathat");
function* worker(p) {
  var r = nes.scene.find("marker");
  if (r === null) { return; }
  var pos = nes.node.getPos(r);
  nes.node.setPos(r, pos[0] + p, pos[1] + 1);
  yield 3;
  var pos2 = nes.node.getPos(r);
  nes.node.setPos(r, pos2[0] + 100, pos2[1] + 1);
}
nes.onSignal("go", worker);
"#;
    let mut rt = engine_with("cor2", &[("marker", 0.0, 0.0)], &[("hathat", HAT)]);
    // 帧 1：触发（p=1）—— 首段 in-tick 同帧落地（S17.2 裁决 A 继承）。
    let mut probe1 = QueueProbe::emitting(vec![("go", Value::I64(1))]);
    step(&mut rt, &mut probe1);
    assert_eq!(pos_of(&mut rt, "marker"), (1.0, 1.0), "first segment lands in-tick");

    // 帧 2：二次触发（p=2）= 新实例并发；实例 A 仍在 wait 中（互不干扰）。
    let mut probe2 = QueueProbe::emitting(vec![("go", Value::I64(2))]);
    step(&mut rt, &mut probe2);
    assert_eq!(pos_of(&mut rt, "marker"), (3.0, 2.0), "second instance starts concurrently");

    // 帧 3：两实例都在停顿（A wait 1->0、B 2->1）。
    let mut probe3 = QueueProbe::listening();
    step(&mut rt, &mut probe3);
    assert_eq!(pos_of(&mut rt, "marker"), (3.0, 2.0), "both paused");

    // 帧 4：A 续跑完成（+100）；B 仍停顿（1->0）。
    let mut probe4 = QueueProbe::listening();
    step(&mut rt, &mut probe4);
    assert_eq!(pos_of(&mut rt, "marker"), (103.0, 3.0), "instance A (p=1) completes first");

    // 帧 5：B 续跑完成（+100）—— 完成序 = 生成序（确定性）。
    let mut probe5 = QueueProbe::listening();
    step(&mut rt, &mut probe5);
    assert_eq!(pos_of(&mut rt, "marker"), (203.0, 4.0), "instance B (p=2) completes second");

    // 帧 6：两实例都已出表 —— 无重放。
    let mut probe6 = QueueProbe::listening();
    step(&mut rt, &mut probe6);
    assert_eq!(pos_of(&mut rt, "marker"), (203.0, 4.0), "finished instances must not replay");
    assert_eq!(rt.extension_faults(), 0);
}

/// T-COR-02 附面板（引擎级上限）：自级联 hat 自我繁殖（每次触发生成
/// 长命实例并发射下一条）。上限是**自我截断**的：第 33 个实例在
/// `coro_start` 处被拒（首段未跑 —— 本应发射下一条信号的正是首段），
/// 级联到此为止 —— 恰一次拒新即一次 fault；泵存活、已接受的 32 实例
/// 不受影响。
#[test]
fn t_cor_02b_self_cascading_hats_hit_the_cap_and_the_pump_survives() {
    const SPAWNER: &str = r#"
nes.registerExtension("spawnhat");
function* selfspawner(p) {
  var r = nes.scene.find("marker");
  if (r !== null) { nes.node.setPos(r, p, 0); }
  if (p < 35) { nes.emitSignal("go", p + 1); }
  yield 99;
}
nes.onSignal("go", selfspawner);
"#;
    let mut rt = engine_with("cor2b", &[("marker", 0.0, 0.0)], &[("spawnhat", SPAWNER)]);
    // 一次触发 -> 同泵级联自我繁殖；#33 起被拒（上限 32），级联自截止。
    let mut probe = QueueProbe::emitting(vec![("go", Value::I64(1))]);
    step(&mut rt, &mut probe);

    // 拒新即 fault：#33 的首段未跑 -> "go"34 无人发射 -> 恰一次拒绝。
    assert_eq!(rt.extension_faults(), 1, "exactly one rejected spawn (cascade self-limits)");
    let last = rt.last_fault().expect("cap fault recorded");
    assert!(last.contains("coroutine cap"), "cap text missing: {last}");
    assert!(last.contains("spawnhat"), "ext id missing: {last}");
    // 已接受的 32 实例各自写过一次首段（同帧快照读数相同 -> 提交序末笔
    // = p=32 生效）；被拒实例首段未跑（33 无写）。
    assert_eq!(pos_of(&mut rt, "marker"), (32.0, 0.0), "last accepted spawn wins the write");

    // 泵存活：后续帧照常 tick、实例照常挂起（无新 fault、无新写）。
    for _ in 0..3 {
        let mut quiet = QueueProbe::listening();
        step(&mut rt, &mut quiet);
    }
    assert_eq!(rt.extension_faults(), 1, "no new faults while instances stay parked");
    assert_eq!(pos_of(&mut rt, "marker"), (32.0, 0.0));
}

/// T-COR-03：生成器续跑段 throw -> fault 隔离（计数 + 诊断、坏实例出表、
/// 其余扩展照常、引擎照活）。
#[test]
fn t_cor_03_generator_throw_is_isolated_and_counted() {
    const FRAGILE: &str = r#"
nes.registerExtension("fragile");
function* fragile() {
  var r = nes.scene.find("log");
  var p = nes.node.getPos(r);
  nes.node.setPos(r, p[0] + 1, p[1]);
  yield 1;
  throw new Error("late-boom");
}
nes.onUpdate(fragile);
"#;
    const SPINNER: &str = r#"
nes.registerExtension("spin3");
nes.onUpdate(function () {
  var r = nes.scene.find("obj1");
  var p = nes.node.getPos(r);
  nes.node.setPos(r, p[0] + 1, p[1]);
});
"#;
    let mut rt = engine_with(
        "cor3",
        &[("log", 0.0, 0.0), ("obj1", 100.0, 0.0)],
        &[("fragile", FRAGILE), ("spin3", SPINNER)],
    );
    // 预热帧：fragile 首段（写一次，yield 1 挂起 —— throw 在续跑段）。
    assert_eq!(pos_of(&mut rt, "log"), (1.0, 0.0));
    let obj1_prewarm = pos_of(&mut rt, "obj1").0;

    let mut probe = QueueProbe::listening();
    step(&mut rt, &mut probe); // f1：停顿帧（wait 1->0），spin3 照常
    assert_eq!(pos_of(&mut rt, "log"), (1.0, 0.0));
    assert_eq!(rt.extension_faults(), 0);

    step(&mut rt, &mut probe); // f2：恢复执行 -> 续跑段 throw -> fault + 出表
    assert_eq!(rt.extension_faults(), 1, "exactly one fault");
    let last = rt.last_fault().expect("fault recorded");
    assert!(last.contains("fragile"), "ext id missing: {last}");
    assert!(last.contains("late-boom"), "exception text missing: {last}");

    step(&mut rt, &mut probe); // f3：坏实例已出表 + 钩子已退役 -> 干净
    assert_eq!(rt.extension_faults(), 1, "dead thread must not re-fire");
    // 其余扩展照常：spin3 每帧都在推进（含 fault 帧之后）。
    let obj1_after = pos_of(&mut rt, "obj1").0;
    assert!(obj1_after > obj1_prewarm, "healthy extension keeps advancing");
    let obj1_before = obj1_after;
    step(&mut rt, &mut probe);
    assert!(
        pos_of(&mut rt, "obj1").0 > obj1_before,
        "engine and neighbors keep running after the fault"
    );
}

/// T-PERM-01：声明无 "audio" -> `nes.audio.play` 抛 permission denied ->
/// fault 计数 + 诊断；已授予能力（scene.read/write）照常工作；其余扩展
/// 与引擎照常。
#[test]
fn t_perm_01_denied_audio_faults_and_engine_continues() {
    const NOAUDIO: &str = r#"
nes.registerExtension("noaudio", ["scene.read", "scene.write"]);
nes.onUpdate(function () {
  var r = nes.scene.find("log");
  var p = nes.node.getPos(r);
  nes.node.setPos(r, p[0] + 1, p[1]);
  nes.audio.play("Audio/beep", 0.5);
});
"#;
    const SPINNER: &str = r#"
nes.registerExtension("spin4");
nes.onUpdate(function () {
  var r = nes.scene.find("obj1");
  var p = nes.node.getPos(r);
  nes.node.setPos(r, p[0] + 1, p[1]);
});
"#;
    let mut rt = engine_with(
        "perm1",
        &[("log", 0.0, 0.0), ("obj1", 0.0, 0.0)],
        &[("noaudio", NOAUDIO), ("spin4", SPINNER)],
    );
    // 预热帧即拒绝（audio 行抛；此前已授予的写已入队并照常落地）。
    assert!(rt.extension_faults() >= 1, "denied play must count as a fault");
    let last = rt.last_fault().expect("fault recorded");
    assert!(last.contains("noaudio"), "ext id missing: {last}");
    assert!(last.contains("permission denied: audio"), "message missing: {last}");
    assert_eq!(pos_of(&mut rt, "log"), (1.0, 0.0), "granted writes before the throw still land");

    let mut probe = QueueProbe::listening();
    step(&mut rt, &mut probe);
    assert!(rt.extension_faults() >= 2, "denial repeats every call");
    // 其余扩展与引擎照常。
    assert!(pos_of(&mut rt, "obj1").0 >= 1.0, "healthy extension keeps advancing");
    assert_eq!(rt.extension_count(), 2, "denied extension stays loaded (fault, not unload)");
}

/// T-PERM-02：声明全五项 = 全能力照旧（含 signal 面往返）；缺省（无声明）
/// = 全授予回归（hello.js 兼容形态）。
#[test]
fn t_perm_02_full_declaration_and_default_grant_everything() {
    const FULL: &str = r#"
nes.registerExtension("fullperm", ["scene.read", "scene.write", "input", "audio", "signal"]);
nes.onUpdate(function () {
  var r = nes.scene.find("log");
  var p = nes.node.getPos(r);
  nes.node.setPos(r, p[0] + 1, p[1]);
});
nes.onSignal("ask", function () { nes.emitSignal("perm-seen", 1); });
"#;
    const DEFAULT: &str = r#"
nes.registerExtension("defaultperm");
nes.onUpdate(function () {
  var r = nes.scene.find("obj1");
  var p = nes.node.getPos(r);
  nes.node.setPos(r, p[0] + 1, p[1]);
});
"#;
    let mut rt = engine_with(
        "perm2",
        &[("log", 0.0, 0.0), ("obj1", 0.0, 0.0)],
        &[("fullperm", FULL), ("defaultperm", DEFAULT)],
    );
    assert_eq!(rt.extension_faults(), 0, "both must run clean");

    // signal 面（需 "signal"）：游戏侧 emit -> hat -> 反向发射 -> 游戏侧。
    // （JS 数值 1 过 NesValue 边界整数值落树 I64 —— 与 S17.2 载荷映射同口径。）
    let mut probe = QueueProbe::emitting(vec![("ask", Value::Bool(true))]); // 无载荷占位
    step(&mut rt, &mut probe);
    assert_eq!(
        probe.payloads_of("perm-seen"),
        vec![Value::I64(1)],
        "signal round-trip under full declaration"
    );
    // 两个扩展的 update 写都照常。
    assert_eq!(pos_of(&mut rt, "log").0, 2.0, "prewarm + one step of fullperm writes");
    assert_eq!(pos_of(&mut rt, "obj1").0, 2.0, "prewarm + one step of default writes");
    assert_eq!(rt.extension_faults(), 0);
}

/// T-PERM-03：onSignal / emitSignal 需 "signal" —— 未授予分别报错：
/// （a）注册期 onSignal = 装载失败，订阅不进泵过滤器，引擎照常；
/// （b）发射期 emitSignal = fault 计数 + 诊断，载荷不落地。
#[test]
fn t_perm_03_signal_permission_required_for_subscribe_and_emit() {
    // (a) 顶层 onSignal 未授予 -> 装载失败（扩展不入册）。
    const NO_SUB: &str = r#"
nes.registerExtension("nosub", ["audio"]);
nes.onSignal("x", function () { });
"#;
    let mut rt = engine_with("perm3a", &[("log", 0.0, 0.0)], &[]);
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("s17_3_perm3a");
    let path = root.join("nosub.js");
    std::fs::write(&path, NO_SUB).unwrap();
    let err = rt.load_extension_file(&path).expect_err("denied onSignal must fail the load");
    assert!(err.contains("permission denied: signal"), "message missing: {err}");
    assert_eq!(rt.extension_count(), 0, "failed extension must not be registered");
    assert!(!rt.extension_has_signal_hats(), "denied subscription must not open the gate");

    // 引擎照常：装载一个健康扩展并推进。
    const HEALTHY: &str = r#"
nes.registerExtension("healthy3");
nes.onUpdate(function () {
  var r = nes.scene.find("log");
  var p = nes.node.getPos(r);
  nes.node.setPos(r, p[0] + 1, p[1]);
});
"#;
    let path2 = root.join("healthy3.js");
    std::fs::write(&path2, HEALTHY).unwrap();
    assert_eq!(rt.load_extension_file(&path2).unwrap(), "healthy3");
    rt.update_extensions();
    assert_eq!(pos_of(&mut rt, "log"), (1.0, 0.0));

    // (b) update 期 emitSignal 未授予 -> fault 计数 + 诊断。
    const NO_EMIT: &str = r#"
nes.registerExtension("noemit", ["scene.read"]);
nes.onUpdate(function () { nes.emitSignal("out", 1); });
"#;
    let root_b = root.parent().unwrap().to_path_buf();
    let dir_b = root_b.join("s17_3_perm3b");
    let _ = std::fs::remove_dir_all(&dir_b);
    std::fs::create_dir_all(&dir_b).unwrap();
    let mut rt_b = NesRuntime::open_headless(&dir_b).expect("headless engine b");
    {
        let tree = rt_b.tree_mut();
        let r = tree.root();
        tree.add_node(r, "log", NodeKind::Node);
        tree.apply_pending();
    }
    let path3 = dir_b.join("noemit.js");
    std::fs::write(&path3, NO_EMIT).unwrap();
    assert_eq!(rt_b.load_extension_file(&path3).unwrap(), "noemit");
    rt_b.update_extensions();
    assert!(rt_b.extension_faults() >= 1, "denied emit must count as a fault");
    let last = rt_b.last_fault().expect("fault recorded");
    assert!(last.contains("permission denied: signal"), "message missing: {last}");
    assert!(last.contains("noemit"), "ext id missing: {last}");
}

/// 输入面（含 "input" 权限的引擎级通态/拒态）：held 键经快照投影进扩展。
#[test]
fn input_permission_gates_is_pressed() {
    // 进程级输入队列清残留（本二进制只有本测试走输入面）。
    let _ = nes_render_wgpu::window::drain_input();
    const PRESSED_OK: &str = r#"
nes.registerExtension("inok", ["scene.read", "scene.write", "input"]);
nes.onUpdate(function () {
  var r = nes.scene.find("log");
  if (nes.input.isPressed("Space")) { nes.node.setPos(r, 42, 0); }
});
"#;
    let mut rt = engine_with("permin", &[("log", 0.0, 0.0)], &[("inok", PRESSED_OK)]);
    // 注入 Space 按住 -> 收集进输入快照 -> 扩展读到并写位置。
    nes_render_wgpu::window::inject_input(nes_render_api::input::InputEvent::Key {
        key: Key::Space,
        down: true,
    });
    let _ = rt.collect_input();
    let mut probe = QueueProbe::listening();
    step(&mut rt, &mut probe);
    assert_eq!(pos_of(&mut rt, "log"), (42.0, 0.0), "granted input must read the snapshot");
    assert_eq!(rt.extension_faults(), 0);

    // 未声明 "input"：isPressed 拒绝（fault），快照同条件。
    const NO_INPUT: &str = r#"
nes.registerExtension("inno", ["scene.read", "scene.write"]);
nes.onUpdate(function () {
  var r = nes.scene.find("log");
  if (nes.input.isPressed("Space")) { nes.node.setPos(r, 99, 0); }
});
"#;
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("s17_3_permin2");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut rt2 = NesRuntime::open_headless(&dir).expect("headless engine 2");
    {
        let tree = rt2.tree_mut();
        let r = tree.root();
        tree.add_node(r, "log", NodeKind::Node);
        tree.apply_pending();
    }
    let path = dir.join("inno.js");
    std::fs::write(&path, NO_INPUT).unwrap();
    assert_eq!(rt2.load_extension_file(&path).unwrap(), "inno");
    rt2.update_extensions();
    // 注入 + 收集 + 推进（与上面同条件）：读被拒 -> 无写、有 fault。
    nes_render_wgpu::window::inject_input(nes_render_api::input::InputEvent::Key {
        key: Key::Space,
        down: true,
    });
    let _ = rt2.collect_input();
    let mut probe2 = QueueProbe::listening();
    step(&mut rt2, &mut probe2);
    assert_eq!(pos_of(&mut rt2, "log"), (0.0, 0.0), "denied read must not write");
    let last = rt2.last_fault().expect("fault recorded");
    assert!(last.contains("permission denied: input"), "message missing: {last}");
}
