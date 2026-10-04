//! S17.3 C4 生成器协程 —— 绑定层契约测试（mock 宿主，无引擎在场）。
//!
//! * T-COR-01（绑定半边）：生成器 onUpdate 帧驱动 —— `yield 2` 的推进
//!   周期为 3 帧（推进帧 + 两帧停顿），逐帧写序被精确钉住；裸 yield = 停
//!   一帧、`yield 0` = 不停顿；普通 onUpdate 回归由既有 `binding_mock.rs`
//!   覆盖；
//! * T-COR-02（绑定半边）：生成器 hat —— 触发即推首段；同 hat 二次触发 =
//!   新实例并发（各推进互不干扰，完成序 = 生成序）；活动实例上限 32 =
//!   拒新留旧 + 抛错（fault 文本随 dispatch 错误浮出）；
//! * T-COR-03（绑定半边）：生成器内 throw → 错误沿蹦床/驱动循环浮出
//!   （S17.1 隔离语义），坏实例出表、后续帧干净、上下文存活；
//! * 调度器真源同步锚：JS 侧 `__nes_coro_cap` 与 Rust 侧
//!   [`nes_extension_js::COROUTINE_CAP`] 字面量一致。
//!
//! JS 字面量全 ASCII（仓库纪律）；引擎级半边见 nes-runtime
//! `tests/s17_3_coroutine_perms.rs`（真树 + fault 簿记）。

use std::cell::RefCell;
use std::rc::Rc;

use nes_extension_api::{
    AudioCapability, ExtensionLifecycle, InputCapability, JsContextId, JsRuntime, NesValue,
    NodeCapability, NodeRef, SceneCapability, SignalCapability,
};
use nes_extension_js::{CapabilityBinding, COROUTINE_CAP, JsExtension, RquickjsRuntime};

/// mock 宿主：读快照固定（find 只认 "obj1" -> 4242，getPos 恒 (10, 20)）、
/// 写侧记账（与 binding_mock.rs 同构 —— 快照/队列口径）。
#[derive(Default)]
struct MockHost {
    set_pos: RefCell<Vec<(u64, f32, f32)>>,
    played: RefCell<Vec<(String, f32)>>,
}

impl SceneCapability for MockHost {
    fn find(&self, name: &str) -> Option<NodeRef> {
        (name == "obj1").then_some(NodeRef(4242))
    }
}

impl NodeCapability for MockHost {
    fn get_pos(&self, _r: NodeRef) -> Option<(f32, f32)> {
        Some((10.0, 20.0))
    }
    fn set_pos(&mut self, r: NodeRef, x: f32, y: f32) {
        self.set_pos.borrow_mut().push((r.0, x, y));
    }
    fn set_visible(&mut self, _r: NodeRef, _v: bool) {}
    fn get_name(&self, _r: NodeRef) -> Option<String> {
        Some("obj1".into())
    }
}

impl InputCapability for MockHost {
    fn is_pressed(&self, _name: &str) -> bool {
        false
    }
}

impl AudioCapability for MockHost {
    fn play(&self, key: &str, volume: f32) {
        self.played.borrow_mut().push((key.to_string(), volume));
    }
}

/// 信号能力恒空实现（本文件只看调度器行为，不核对订阅面 —— 那是
/// binding_mock.rs 的事）。
struct NopSignals;
impl SignalCapability for NopSignals {
    fn on_signal(&mut self, _name: &str) {}
    fn emit(&mut self, _name: &str, _payload: NesValue) {}
}

/// 一套测试装置：扩展句柄 + 运行时 + 上下文号 + 宿主记账端。
struct Rig {
    ext: JsExtension,
    rt: Rc<RefCell<RquickjsRuntime>>,
    ctx: JsContextId,
    host: Rc<RefCell<MockHost>>,
}

impl Rig {
    /// 装一份扩展（能力注入 + 顶层求值；装载必须干净）。
    fn with(source: &str) -> Self {
        let host = Rc::new(RefCell::new(MockHost::default()));
        let binding = CapabilityBinding::new(
            Rc::clone(&host) as Rc<RefCell<dyn SceneCapability>>,
            Rc::clone(&host) as Rc<RefCell<dyn NodeCapability>>,
            Rc::clone(&host) as Rc<RefCell<dyn InputCapability>>,
            Rc::clone(&host) as Rc<RefCell<dyn AudioCapability>>,
            Rc::new(RefCell::new(NopSignals)) as Rc<RefCell<dyn SignalCapability>>,
        );
        let rt = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
        let ctx = rt.borrow_mut().create_context().unwrap();
        binding.install(&mut rt.borrow_mut(), ctx).unwrap();
        let mut ext = JsExtension::new(Rc::clone(&rt), ctx);
        ext.load_source(source).unwrap();
        assert!(ext.take_last_error().is_none(), "load must be clean");
        Self { ext, rt, ctx, host }
    }

    /// 本帧落进写队列的 setPos（取走即清 —— 帧粒度断言口）。
    fn take_writes(&self) -> Vec<(u64, f32, f32)> {
        std::mem::take(&mut *self.host.borrow_mut().set_pos.borrow_mut())
    }

    /// 调全局探针函数（无参，返回 NesValue）。
    fn probe(&self, name: &str) -> NesValue {
        self.rt.borrow_mut().call(self.ctx, name, &[]).unwrap()
    }
}

/// T-COR-01（绑定半边）：生成器 onUpdate 帧驱动 —— `yield 2` 推进周期 =
/// 3 帧；逐帧写序精确钉住（写落第 1、4、7 帧，停顿帧零写）。
#[test]
fn t_cor_01_generator_update_is_frame_driven_with_yield_two() {
    const GEN_UPDATE: &str = r#"
nes.registerExtension("coro1");
function* stepper() {
  var r = nes.scene.find("obj1");
  while (true) {
    var p = nes.node.getPos(r);
    nes.node.setPos(r, p[0] + 1, p[1]);
    yield 2;
  }
}
nes.onUpdate(stepper);
"#;
    let mut rig = Rig::with(GEN_UPDATE);

    // 帧 1：钩子调用返回生成器对象 -> 入调度器 + 首段立即推进（一次写）。
    rig.ext.update();
    assert!(rig.ext.take_last_error().is_none());
    assert_eq!(
        rig.take_writes().as_slice(),
        &[(4242, 11.0, 20.0)],
        "frame 1: first segment runs at spawn"
    );

    // 帧 2、3：wait=2 递减（2->1->0），停顿帧无写。
    rig.ext.update();
    assert!(rig.ext.take_last_error().is_none());
    assert!(rig.take_writes().is_empty(), "frame 2: paused");
    rig.ext.update();
    assert!(rig.ext.take_last_error().is_none());
    assert!(rig.take_writes().is_empty(), "frame 3: still paused");

    // 帧 4：wait 归零后的下一帧推进（第二次写）—— 推进周期恰 3 帧。
    rig.ext.update();
    assert!(rig.ext.take_last_error().is_none());
    assert_eq!(
        rig.take_writes().as_slice(),
        &[(4242, 11.0, 20.0)],
        "frame 4: resumes after exactly two paused frames"
    );

    // 周期重复：帧 5、6 停顿，帧 7 第三次推进。
    rig.ext.update();
    rig.ext.update();
    assert!(rig.take_writes().is_empty(), "frames 5-6: paused");
    rig.ext.update();
    assert_eq!(rig.take_writes().len(), 1, "frame 7: third advance");
}

/// 裸 yield = 停一帧；`yield 0` = 不停顿；done = 出表（无重放）。
#[test]
fn bare_yield_pauses_one_frame_and_yield_zero_does_not_pause() {
    const MIXED: &str = r#"
nes.registerExtension("coro1b");
var mode = 0;
var bumps = 0;
function bump() { bumps = bumps + 1; }
function* mixed() {
  bump();
  if (mode === 0) { yield; }
  bump();
  if (mode === 0) { yield 0; }
  bump();
  mode = 1;
}
nes.onUpdate(mixed);
function __nes_bumps() { return bumps; }
"#;
    let mut rig = Rig::with(MIXED);
    // 帧 1：首段（bumps=1）-> 裸 yield（停一帧）。
    rig.ext.update();
    // 帧 2：停顿帧（bumps 仍 1）。
    rig.ext.update();
    assert_eq!(rig.probe("__nes_bumps"), NesValue::F64(1.0), "bare yield pauses one frame");
    // 帧 3：第二段（bumps=2）-> yield 0（不停顿）。
    rig.ext.update();
    assert_eq!(rig.probe("__nes_bumps"), NesValue::F64(2.0));
    // 帧 4：yield 0 = 下一帧立即推进（bumps=3，随后 mode=1、隐式 done）。
    rig.ext.update();
    assert_eq!(rig.probe("__nes_bumps"), NesValue::F64(3.0), "yield 0 does not pause");
    // 帧 5：生成器已结束 —— 出表，无新推进。
    rig.ext.update();
    assert_eq!(rig.probe("__nes_bumps"), NesValue::F64(3.0), "finished generator is removed");
}

/// T-COR-02（绑定半边）：生成器 hat 触发即推首段；同 hat 二次触发 = 新实例
/// 并发；上限 32 = 拒新留旧 + 抛错。
#[test]
fn t_cor_02_generator_hats_spawn_concurrent_instances_and_cap_at_32() {
    const GEN_HAT: &str = r#"
nes.registerExtension("coro2");
globalThis.__trace = [];
function* worker(p) {
  globalThis.__trace.push("start" + p);
  yield 3;
  globalThis.__trace.push("done" + p);
}
nes.onSignal("go", worker);
function __nes_trace() { return globalThis.__trace.join(","); }
"#;
    let mut rig = Rig::with(GEN_HAT);

    // 触发一：首段立即推进（start1），实例 A 挂起 wait=3。
    rig.ext.dispatch_signal("go", &NesValue::F64(1.0)).unwrap();
    assert_eq!(rig.probe("__nes_trace"), NesValue::str("start1"));
    // 触发二：**新建实例**（Scratch startHats 重入语义）—— start2 立即出现，
    // 两实例并发挂起、互不干扰。
    rig.ext.dispatch_signal("go", &NesValue::F64(2.0)).unwrap();
    assert_eq!(rig.probe("__nes_trace"), NesValue::str("start1,start2"));

    // 帧 1..3：wait=3 三帧停顿，无新推进。
    for _ in 0..3 {
        rig.ext.update();
        assert!(rig.ext.take_last_error().is_none());
    }
    assert_eq!(rig.probe("__nes_trace"), NesValue::str("start1,start2"));

    // 帧 4：两实例各自推进到完成段 —— 完成序 = 表序 = 生成序（确定性）。
    rig.ext.update();
    assert_eq!(rig.probe("__nes_trace"), NesValue::str("start1,start2,done1,done2"));
    // 帧 5：done 实例已出表 —— 无重放。
    rig.ext.update();
    assert_eq!(rig.probe("__nes_trace"), NesValue::str("start1,start2,done1,done2"));

    // 上限：32 次"长命"触发把表填满（各实例 yield 100 挂起），第 33 次 =
    // 拒新（首段不跑）+ 抛错（fault 文本沿 dispatch 错误浮出）；留旧 =
    // 既有 32 实例不受影响。
    for p in 10..42i64 {
        rig.ext
            .dispatch_signal("go", &NesValue::F64(p as f64))
            .expect("fill up to cap");
    }
    let err = rig
        .ext
        .dispatch_signal("go", &NesValue::F64(99.0))
        .expect_err("33rd spawn must be rejected");
    let text = err.to_string();
    assert!(text.contains("coroutine cap"), "cap fault text missing: {text}");
    let trace = match rig.probe("__nes_trace") {
        NesValue::Str(s) => s,
        other => panic!("trace must be a string, got {other:?}"),
    };
    assert!(!trace.contains("start99"), "rejected instance's first segment must not run");
    // 留旧：填进去的 32 个实例照常挂起（推进帧无错误、无额外完成）。
    rig.ext.update();
    assert!(rig.ext.take_last_error().is_none(), "kept instances must stay healthy");
}

/// T-COR-03（绑定半边）：生成器内 throw —— 首段抛 / 续跑段抛都沿既有
/// fault 隔离路径浮出（S17.1），坏实例出表、后续帧干净、上下文存活。
#[test]
fn t_cor_03_generator_throw_is_fault_isolated() {
    // 首段即抛：spawn 当帧浮出；钩子已退役（生成器 onUpdate 一次性起线程），
    // 后续帧干净。
    const FIRST_SEG_THROW: &str = r#"
nes.registerExtension("coro3a");
function* boom() { throw new Error("seg-boom"); }
nes.onUpdate(boom);
"#;
    let mut rig = Rig::with(FIRST_SEG_THROW);
    rig.ext.update();
    let err = rig.ext.take_last_error().expect("first-segment throw must fault");
    assert!(err.contains("seg-boom"), "exception text missing: {err}");
    rig.ext.update();
    assert!(rig.ext.take_last_error().is_none(), "dead thread must not re-fire");

    // 续跑段抛：驱动循环逐表项 try/catch（坏实例出表）+ 首错重抛。
    const LATE_THROW: &str = r#"
nes.registerExtension("coro3b");
function* late() { yield 1; throw new Error("late-boom"); }
nes.onUpdate(late);
"#;
    let mut rig2 = Rig::with(LATE_THROW);
    rig2.ext.update(); // 帧 1：首段（yield 1）
    assert!(rig2.ext.take_last_error().is_none());
    rig2.ext.update(); // 帧 2：停顿帧
    assert!(rig2.ext.take_last_error().is_none());
    rig2.ext.update(); // 帧 3：推进 -> throw -> 出表 + 重抛
    let err2 = rig2.ext.take_last_error().expect("late throw must fault");
    assert!(err2.contains("late-boom"), "exception text missing: {err2}");
    rig2.ext.update(); // 帧 4：表已清 —— 干净
    assert!(
        rig2.ext.take_last_error().is_none(),
        "faulted entry must leave the table"
    );
}

/// hat 侧生成器抛错：蹦床逐 handler try/catch —— 抛错的生成器 handler
/// 不殃及同表后续 handler（首错循环后重抛，与普通 handler 同口径）。
#[test]
fn generator_hat_throw_does_not_block_later_handlers() {
    const TWO_HATS: &str = r#"
nes.registerExtension("coro3c");
var after = null;
nes.onSignal("boom", function* () { throw new Error("hat-gen-boom"); });
nes.onSignal("boom", function (p) { after = p; });
function __nes_hat_after() { return after; }
"#;
    let mut rig = Rig::with(TWO_HATS);
    let err = rig
        .ext
        .dispatch_signal("boom", &NesValue::F64(7.0))
        .expect_err("throwing generator handler must fault");
    assert!(err.to_string().contains("hat-gen-boom"));
    // 同表后续 handler 照跑（蹦床隔离粒度 = handler）。
    assert_eq!(rig.probe("__nes_hat_after"), NesValue::F64(7.0));
}

/// 调度器容量真源同步锚：JS 侧 `__nes_coro_cap` 与 Rust 侧
/// [`COROUTINE_CAP`] 必须字面量一致（防两处漂移）。
#[test]
fn coroutine_cap_literal_is_in_sync_between_js_and_rust() {
    assert!(
        nes_extension_js::NES_BOOTSTRAP_JS.contains(&format!("__nes_coro_cap = {COROUTINE_CAP}")),
        "JS scheduler cap must mirror COROUTINE_CAP = {COROUTINE_CAP}"
    );
}

/// 生命周期 trait 面回归：生成器驱动照常经 `Box<dyn ExtensionLifecycle>`
/// 统一句柄工作（宿主视角不感知生成器与普通钩子的差别）。
#[test]
fn generator_update_works_through_lifecycle_trait_object() {
    const GEN_UPDATE: &str = r#"
nes.registerExtension("coro4");
var ticks = 0;
function* ticker() {
  while (true) { ticks = ticks + 1; yield 1; }
}
nes.onUpdate(ticker);
function __nes_ticks() { return ticks; }
"#;
    let rig = Rig::with(GEN_UPDATE);
    // 运行时句柄先克隆（Box 拿走 ext 后探针仍可达 —— Rc 共享同一运行时）。
    let rt = Rc::clone(&rig.rt);
    let ctx = rig.ctx;
    let mut boxed: Box<dyn ExtensionLifecycle> = Box::new(rig.ext);
    boxed.update(); // 帧 1：spawn + 首段（ticks=1）
    boxed.update(); // 帧 2：停顿（yield 1）
    assert_eq!(
        rt.borrow_mut().call(ctx, "__nes_ticks", &[]).unwrap(),
        NesValue::F64(1.0)
    );
    boxed.update(); // 帧 3：推进（ticks=2）
    assert_eq!(
        rt.borrow_mut().call(ctx, "__nes_ticks", &[]).unwrap(),
        NesValue::F64(2.0)
    );
}
