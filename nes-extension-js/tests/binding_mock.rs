//! 能力注入的端到端（mock 宿主）：**不需要引擎在场 —— API 冻结的价值**。
//!
//! mock traits 即可驱动完整链路：注入 `nes` 对象 -> JS 扩展调用能力 ->
//! Rust 侧断言宿主收到正确参数。

use std::cell::RefCell;
use std::rc::Rc;

use nes_extension_api::{
    AudioCapability, ExtensionLifecycle, InputCapability, JsRuntime, NesValue, NodeCapability,
    NodeRef, SceneCapability, SignalCapability,
};
use nes_extension_js::{CapabilityBinding, JsExtension, RquickjsRuntime};

/// mock 宿主：记账每次能力调用（读侧快照 + 写侧队列与 nes-runtime 的
/// 引擎桥同构 —— 见 crate 文档"快照/队列"口径）。
#[derive(Default)]
struct MockHost {
    // RefCell 记账：Scene/Input/Audio 的能力方法只有 &self（触发型能力
    // 天然内部可变性 —— 与 nes-extension-api 测试同一口径）。
    find_calls: RefCell<Vec<String>>,
    set_pos: RefCell<Vec<(u64, f32, f32)>>,
    set_visible: RefCell<Vec<(u64, bool)>>,
    pressed: RefCell<Vec<String>>,
    pressed_answer: RefCell<Vec<bool>>,
    played: RefCell<Vec<(String, f32)>>,
}

impl SceneCapability for MockHost {
    fn find(&self, name: &str) -> Option<NodeRef> {
        // 只认 "obj1"（模拟场景里恰好有一个同名节点）。
        self.find_calls.borrow_mut().push(name.to_string());
        (name == "obj1").then_some(NodeRef(4242))
    }
}

impl NodeCapability for MockHost {
    fn get_pos(&self, r: NodeRef) -> Option<(f32, f32)> {
        (r.0 == 4242).then_some((10.0, 20.0))
    }
    fn set_pos(&mut self, r: NodeRef, x: f32, y: f32) {
        self.set_pos.borrow_mut().push((r.0, x, y));
    }
    fn set_visible(&mut self, r: NodeRef, v: bool) {
        self.set_visible.borrow_mut().push((r.0, v));
    }
    fn get_name(&self, r: NodeRef) -> Option<String> {
        (r.0 == 4242).then(|| "obj1".to_string())
    }
}

impl InputCapability for MockHost {
    fn is_pressed(&self, name: &str) -> bool {
        self.pressed.borrow_mut().push(name.to_string());
        let hit = name == "Space";
        self.pressed_answer.borrow_mut().push(hit);
        hit
    }
}

impl AudioCapability for MockHost {
    fn play(&self, key: &str, volume: f32) {
        self.played.borrow_mut().push((key.to_string(), volume));
    }
}

/// S17.2 信号能力 mock：订阅名去重 + 发射记账（与引擎桥同构）。
#[derive(Default)]
struct MockSignals {
    subscribed: RefCell<Vec<String>>,
    emitted: RefCell<Vec<(String, NesValue)>>,
}

impl SignalCapability for MockSignals {
    fn on_signal(&mut self, name: &str) {
        let mut subs = self.subscribed.borrow_mut();
        if !subs.iter().any(|n| n == name) {
            subs.push(name.to_string());
        }
    }
    fn emit(&mut self, name: &str, payload: NesValue) {
        self.emitted.borrow_mut().push((name.to_string(), payload));
    }
}

const TEST_EXTENSION: &str = r#"
nes.registerExtension("mock-ext");
nes.onUpdate(function () {
  var ref = nes.scene.find("obj1");
  var pos = nes.node.getPos(ref);
  nes.node.setPos(ref, pos[0] + 1, pos[1] + 2);
  if (nes.input.isPressed("Space")) {
    nes.audio.play("beep", 0.5);
  }
  if (!nes.input.isPressed("KeyA")) {
    nes.node.setVisible(ref, false);
  }
  last_seen = nes.node.getName(ref);
});
var last_seen = null;
"#;

#[test]
fn js_extension_drives_mock_capabilities_end_to_end() {
    let host = Rc::new(RefCell::new(MockHost::default()));
    let signals = Rc::new(RefCell::new(MockSignals::default()));
    let binding = CapabilityBinding::new(
        Rc::clone(&host) as Rc<RefCell<dyn SceneCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn NodeCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn InputCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn AudioCapability>>,
        Rc::clone(&signals) as Rc<RefCell<dyn SignalCapability>>,
    );

    let runtime = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
    let ctx = runtime.borrow_mut().create_context().unwrap();
    binding.install(&mut runtime.borrow_mut(), ctx).unwrap();

    let mut ext = JsExtension::new(Rc::clone(&runtime), ctx);
    ext.load_source(TEST_EXTENSION).unwrap();
    assert!(ext.take_last_error().is_none());

    // JS 自报 id -> 宿主经生命周期注册（汇合同一全局槽）。
    let id = ext.extension_id_from_js().unwrap();
    assert_eq!(id, "mock-ext");
    ext.register(&id);
    assert!(ext.take_last_error().is_none());
    assert_eq!(ext.id(), "mock-ext");

    // 帧一：Space 按住 -> setPos + play 都应到达宿主
    //（mock 输入按 is_pressed 内判定编排：本帧 "Space" 即按住）。
    ext.update();
    assert!(ext.take_last_error().is_none(), "update #1 must be clean");
    {
        let h = host.borrow();
        assert_eq!(h.find_calls.borrow().last().map(String::as_str), Some("obj1"));
        // getPos 读回 (10, 20)，JS 加偏移后 setPos(11, 22)。
        assert_eq!(
            h.set_pos.borrow().as_slice(),
            &[(4242, 11.0, 22.0)],
            "setPos must arrive with JS-side arithmetic applied"
        );
        assert_eq!(h.played.borrow().as_slice(), &[("beep".to_string(), 0.5)]);
        assert_eq!(h.set_visible.borrow().as_slice(), &[(4242, false)]);
        assert_eq!(h.pressed.borrow().as_slice(), &["Space".to_string(), "KeyA".to_string()]);
        assert_eq!(
            h.pressed_answer.borrow().as_slice(),
            &[true, false],
            "Space=true, KeyA=false"
        );
    }

    ext.update();
    assert!(ext.take_last_error().is_none(), "update #2 must be clean");
    {
        let h = host.borrow();
        // 第二帧：getPos 仍是 mock 的固定读数（mock 无状态推进），写侧再记一笔。
        assert_eq!(h.set_pos.borrow().len(), 2);
        assert_eq!(h.played.borrow().len(), 2);
    }

    // GC 跑一遍不炸（collect 属冻结面）。
    runtime.borrow_mut().collect();
}

#[test]
fn missing_capability_object_is_script_visible_not_fatal() {
    // 只有 scene 能力可用、其余不给：JS 侧调用应让扩展报错可取，
    // 而不是把宿主进程带走。
    struct NullScene;
    impl SceneCapability for NullScene {
        fn find(&self, _name: &str) -> Option<NodeRef> {
            None
        }
    }

    let scene = Rc::new(RefCell::new(NullScene));
    // 其余四个能力用 Default mock（永远空记账）。
    let host = Rc::new(RefCell::new(MockHost::default()));
    let signals = Rc::new(RefCell::new(MockSignals::default()));
    let binding = CapabilityBinding::new(
        scene,
        Rc::clone(&host) as Rc<RefCell<dyn NodeCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn InputCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn AudioCapability>>,
        Rc::clone(&signals) as Rc<RefCell<dyn SignalCapability>>,
    );

    let runtime = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
    let ctx = runtime.borrow_mut().create_context().unwrap();
    binding.install(&mut runtime.borrow_mut(), ctx).unwrap();

    let mut ext = JsExtension::new(Rc::clone(&runtime), ctx);
    // find 返回 null：脚本侧对 null 取下标 -> JS 运行期异常 -> update 报错可取。
    ext.load_source("nes.onUpdate(function () { var r = nes.scene.find(\"obj1\"); r[0]; });")
        .unwrap();
    ext.update();
    let err = ext.take_last_error();
    assert!(err.is_some(), "script error must surface via lifecycle");
    // 宿主进程健在：同一扩展上下文还能继续跑。
    let probe = runtime
        .borrow_mut()
        .call(ctx, "__nes_get_extension_id", &[])
        .unwrap();
    assert_eq!(probe, nes_extension_api::NesValue::Null);
}

const HAT_EXTENSION: &str = r#"
nes.registerExtension("hat-ext");
var got = null;
nes.onSignal("enemy-died", function (payload) { got = payload; });
nes.onSignal("enemy-died", function (payload) { got = got + 1; });
nes.onSignal("relay", function (payload) { nes.emitSignal("hat-relay", payload); });
"#;

#[test]
fn signal_hat_dispatch_and_reverse_emit_reach_mock_host() {
    // S17.2 绑定层端到端（无引擎）：注册 hat -> 派发 -> 反向发射。
    let signals = Rc::new(RefCell::new(MockSignals::default()));
    let host = Rc::new(RefCell::new(MockHost::default()));
    let binding = CapabilityBinding::new(
        Rc::clone(&host) as Rc<RefCell<dyn SceneCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn NodeCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn InputCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn AudioCapability>>,
        Rc::clone(&signals) as Rc<RefCell<dyn SignalCapability>>,
    );
    let runtime = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
    let ctx = runtime.borrow_mut().create_context().unwrap();
    binding.install(&mut runtime.borrow_mut(), ctx).unwrap();

    let mut ext = JsExtension::new(Rc::clone(&runtime), ctx);
    ext.load_source(HAT_EXTENSION).unwrap();
    assert!(ext.take_last_error().is_none());

    // 订阅声明到达宿主（同名两个 handler 只声明一次 —— JS 侧去重）。
    {
        let s = signals.borrow();
        assert_eq!(
            s.subscribed.borrow().as_slice(),
            &["enemy-died".to_string(), "relay".to_string()]
        );
    }

    // 派发：同名 handler 按注册序都调（42 -> 43）。
    let called = ext
        .dispatch_signal("enemy-died", &NesValue::F64(42.0))
        .expect("dispatch must succeed");
    assert_eq!(called, 2, "both registered handlers must run");
    let seen = runtime
        .borrow_mut()
        .call(ctx, "__nes_get_extension_id", &[])
        .unwrap();
    assert_eq!(seen, NesValue::str("hat-ext")); // 上下文仍健康

    // 反向发射到达宿主能力（载荷过 NesValue 边界）。
    ext.dispatch_signal("relay", &NesValue::str("ping")).unwrap();
    {
        let s = signals.borrow();
        assert_eq!(
            s.emitted.borrow().as_slice(),
            &[("hat-relay".to_string(), NesValue::str("ping"))]
        );
    }
    assert!(ext.take_last_error().is_none());

    // 未订阅的名字：蹦床早退（0 命中，无 JS 调用）。
    assert_eq!(ext.dispatch_signal("nobody-listens", &NesValue::Null).unwrap(), 0);
}

#[test]
fn handler_throw_surfaces_through_the_trampoline() {
    // 隔离继承的绑定层半边：handler 抛错 -> 蹦床重抛 -> Err 携带异常文本；
    // 抛错者**之后**的 handler 仍然跑到（记账 payload）。
    const THROWER: &str = r#"
nes.registerExtension("throwhat");
var after = null;
nes.onSignal("boom", function () { throw new Error("hat-boom"); });
nes.onSignal("boom", function (p) { after = p; });
function __nes_hat_after() { return after; }
"#;
    let signals = Rc::new(RefCell::new(MockSignals::default()));
    let host = Rc::new(RefCell::new(MockHost::default()));
    let binding = CapabilityBinding::new(
        Rc::clone(&host) as Rc<RefCell<dyn SceneCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn NodeCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn InputCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn AudioCapability>>,
        Rc::clone(&signals) as Rc<RefCell<dyn SignalCapability>>,
    );
    let runtime = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
    let ctx = runtime.borrow_mut().create_context().unwrap();
    binding.install(&mut runtime.borrow_mut(), ctx).unwrap();

    let mut ext = JsExtension::new(Rc::clone(&runtime), ctx);
    ext.load_source(THROWER).unwrap();
    let err = ext
        .dispatch_signal("boom", &NesValue::F64(1.0))
        .expect_err("throwing handler must fault");
    let text = err.to_string();
    assert!(text.contains("hat-boom"), "exception text missing: {text}");
    // 抛错后的同表 handler 已跑到（蹦床 try/catch 继续）—— 用全局探针核实。
    let after = runtime.borrow_mut().call(ctx, "__nes_hat_after", &[]).unwrap();
    assert_eq!(after, NesValue::F64(1.0));
    // 上下文存活：下一次派发照常。
    assert_eq!(ext.dispatch_signal("nobody", &NesValue::Null).unwrap(), 0);
}
