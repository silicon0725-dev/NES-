//! S17.3 B3 权限模型 —— 绑定层契约测试（mock 宿主，无引擎在场）。
//!
//! * T-PERM-01（绑定半边）：声明无 "audio" → `nes.audio.play` 抛
//!   `permission denied: audio` → 错误可取（fault 半边）；**无声音副作用**
//!   （mock 记账为空）；已授予能力照常工作；
//! * T-PERM-02（绑定半边）：声明全五项 → 全能力照旧；缺省（无声明）=
//!   全授予回归（hello.js 兼容）；
//! * T-PERM-03（绑定半边）：`onSignal` / `emitSignal` 需 "signal" ——
//!   未授予分别报错（注册期 = 装载失败；发射期 = 调用错误）；
//! * 附加边界：空数组 = 全拒绝；守卫是**逐调用**裁决（声明前/后调用行为
//!   一致按注册期定死口径）；`nes.registerExtension` / `nes.onUpdate`
//!   本身不受权限约束（生命周期面不是能力面）。
//!
//! JS 字面量全 ASCII（仓库纪律）；引擎级半边（fault 计数/诊断文本）见
//! nes-runtime `tests/s17_3_coroutine_perms.rs`。

use std::cell::RefCell;
use std::rc::Rc;

use nes_extension_api::{
    AudioCapability, ExtensionLifecycle, InputCapability, JsContextId, JsRuntime, NesValue,
    NodeCapability, NodeRef, SceneCapability, SignalCapability,
};
use nes_extension_js::{CapabilityBinding, JsExtension, RquickjsRuntime};

/// mock 宿主：五项能力全记账（断言"授予的照常、拒绝的零副作用"）。
#[derive(Default)]
struct MockHost {
    find_calls: RefCell<Vec<String>>,
    set_pos: RefCell<Vec<(u64, f32, f32)>>,
    pressed: RefCell<Vec<String>>,
    played: RefCell<Vec<(String, f32)>>,
}

impl SceneCapability for MockHost {
    fn find(&self, name: &str) -> Option<NodeRef> {
        self.find_calls.borrow_mut().push(name.to_string());
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
    fn is_pressed(&self, name: &str) -> bool {
        self.pressed.borrow_mut().push(name.to_string());
        name == "Space"
    }
}

impl AudioCapability for MockHost {
    fn play(&self, key: &str, volume: f32) {
        self.played.borrow_mut().push((key.to_string(), volume));
    }
}

/// 信号能力记账（订阅 + 发射都观测 —— "signal" 权限的两侧证据）。
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

struct Rig {
    ext: JsExtension,
    rt: Rc<RefCell<RquickjsRuntime>>,
    ctx: JsContextId,
    host: Rc<RefCell<MockHost>>,
    signals: Rc<RefCell<MockSignals>>,
}

fn setup(source: &str) -> Rig {
    let host = Rc::new(RefCell::new(MockHost::default()));
    let signals = Rc::new(RefCell::new(MockSignals::default()));
    let binding = CapabilityBinding::new(
        Rc::clone(&host) as Rc<RefCell<dyn SceneCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn NodeCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn InputCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn AudioCapability>>,
        Rc::clone(&signals) as Rc<RefCell<dyn SignalCapability>>,
    );
    let rt = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
    let ctx = rt.borrow_mut().create_context().unwrap();
    binding.install(&mut rt.borrow_mut(), ctx).unwrap();
    let mut ext = JsExtension::new(Rc::clone(&rt), ctx);
    ext.load_source(source).unwrap_or_else(|e| panic!("load failed: {e}"));
    assert!(ext.take_last_error().is_none(), "load must be clean");
    Rig { ext, rt, ctx, host, signals }
}

impl Rig {
    /// 调全局探针函数（无参，返回 NesValue）。
    fn probe(&self, name: &str) -> NesValue {
        self.rt.borrow_mut().call(self.ctx, name, &[]).unwrap()
    }
}

/// T-PERM-01（绑定半边）：声明无 "audio" —— play 抛 permission denied、
/// mock 零播放记账；同帧已授予能力（scene.read/write）照常记账。
#[test]
fn t_perm_01_denied_audio_throws_without_side_effect() {
    const PARTIAL: &str = r#"
nes.registerExtension("p1", ["scene.read", "scene.write"]);
nes.onUpdate(function () {
  var r = nes.scene.find("obj1");
  nes.node.setPos(r, 11, 22);
  nes.audio.play("beep", 0.5);
});
"#;
    let mut rig = setup(PARTIAL);
    rig.ext.update();
    let err = rig.ext.take_last_error().expect("denied call must surface");
    assert!(err.contains("permission denied: audio"), "message missing: {err}");
    {
        let h = rig.host.borrow();
        // 授予的能力照常：find + setPos 都已记账（抛点在 audio 行）。
        assert_eq!(h.find_calls.borrow().as_slice(), &["obj1".to_string()]);
        assert_eq!(h.set_pos.borrow().as_slice(), &[(4242, 11.0, 22.0)]);
        // **无声音副作用**：play 从未到达宿主能力。
        assert!(h.played.borrow().is_empty(), "denied play must not reach the host");
    }
    // 上下文存活（S17.1 隔离口径）：下一帧照常（仍会拒绝，但健康探针可用）。
    rig.ext.update();
    assert!(rig.ext.take_last_error().is_some(), "denial repeats every call");
}

/// T-PERM-02（绑定半边）：声明全五项 = 全能力照旧；缺省（无声明）= 全授予
/// 回归（hello.js 的装载形态）。
#[test]
fn t_perm_02_full_declaration_and_default_grant_everything() {
    const FULL: &str = r#"
nes.registerExtension("p2", ["scene.read", "scene.write", "input", "audio", "signal"]);
nes.onUpdate(function () {
  var r = nes.scene.find("obj1");
  var p = nes.node.getPos(r);
  nes.node.setPos(r, p[0] + 1, p[1]);
  if (nes.input.isPressed("Space")) { nes.audio.play("beep", 0.5); }
});
nes.onSignal("ping", function () { nes.emitSignal("pong", 1); });
"#;
    let mut rig = setup(FULL);
    rig.ext.update(); // mock 里 Space 恒按住 -> 全链走通：find/getPos/setPos/isPressed/audio.play
    assert!(rig.ext.take_last_error().is_none(), "full declaration grants everything");
    {
        let h = rig.host.borrow();
        assert_eq!(h.set_pos.borrow().as_slice(), &[(4242, 11.0, 20.0)]);
        assert_eq!(h.pressed.borrow().as_slice(), &["Space".to_string()]);
        assert_eq!(
            h.played.borrow().as_slice(),
            &[("beep".to_string(), 0.5)],
            "audio must be granted under full declaration"
        );
    }
    // signal 面：hat 派发 + 反向发射都通。
    rig.ext.dispatch_signal("ping", &NesValue::Null).unwrap();
    {
        let s = rig.signals.borrow();
        assert_eq!(s.subscribed.borrow().as_slice(), &["ping".to_string()]);
        assert_eq!(
            s.emitted.borrow().as_slice(),
            &[("pong".to_string(), NesValue::F64(1.0))]
        );
    }

    // 缺省（无第二参）= 全授予：hello.js 兼容回归。
    const DEFAULT: &str = r#"
nes.registerExtension("p2b");
nes.onUpdate(function () {
  nes.audio.play("beep", 0.5);
  nes.input.isPressed("Space");
});
nes.onSignal("ping", function () { nes.emitSignal("pong", 2); });
"#;
    let mut rig2 = setup(DEFAULT);
    rig2.ext.update();
    assert!(rig2.ext.take_last_error().is_none(), "no declaration = all granted");
    {
        let h = rig2.host.borrow();
        assert_eq!(h.played.borrow().len(), 1, "audio must be granted by default");
    }
    rig2.ext.dispatch_signal("ping", &NesValue::Null).unwrap();
    assert!(rig2.ext.take_last_error().is_none());
}

/// T-PERM-03（绑定半边）：onSignal / emitSignal 需 "signal" —— 未授予分别
/// 报错：注册期（顶层 onSignal）= 装载失败；发射期（update 内 emitSignal）
/// = 调用错误。授予后两侧都通（T-PERM-02 已覆盖通态）。
#[test]
fn t_perm_03_signal_permission_required_for_subscribe_and_emit() {
    // (a) 未授予 "signal"：顶层 onSignal -> 装载失败（异常文本随行）。
    const NO_SIGNAL_SUB: &str = r#"
nes.registerExtension("p3a", ["audio"]);
nes.onSignal("ping", function () { });
"#;
    let host = Rc::new(RefCell::new(MockHost::default()));
    let signals = Rc::new(RefCell::new(MockSignals::default()));
    let binding = CapabilityBinding::new(
        Rc::clone(&host) as Rc<RefCell<dyn SceneCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn NodeCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn InputCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn AudioCapability>>,
        Rc::clone(&signals) as Rc<RefCell<dyn SignalCapability>>,
    );
    let rt = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
    let ctx = rt.borrow_mut().create_context().unwrap();
    binding.install(&mut rt.borrow_mut(), ctx).unwrap();
    let mut ext = JsExtension::new(Rc::clone(&rt), ctx);
    let err = ext.load_source(NO_SIGNAL_SUB).expect_err("denied onSignal must fail the load");
    let text = err.to_string();
    assert!(text.contains("permission denied: signal"), "message missing: {text}");
    // 订阅声明未到达宿主（拒在门口 —— 泵过滤器不装配）。
    assert!(signals.borrow().subscribed.borrow().is_empty());

    // (b) 未授予 "signal"：update 内 emitSignal -> 调用错误（fault 半边）。
    const NO_SIGNAL_EMIT: &str = r#"
nes.registerExtension("p3b", []);
nes.onUpdate(function () { nes.emitSignal("out", 1); });
"#;
    let mut rig = setup(NO_SIGNAL_EMIT);
    rig.ext.update();
    let err = rig.ext.take_last_error().expect("denied emit must surface");
    assert!(err.contains("permission denied: signal"), "message missing: {err}");
    assert!(rig.signals.borrow().emitted.borrow().is_empty(), "denied emit must not land");
}

/// 声明期静态：权限在 `registerExtension` 时定死 —— 运行期无提权接口
/// （重复注册换不来新权限之外的语义；最后声明生效且同样只在此后生效）。
#[test]
fn grants_are_static_from_registration_and_last_declaration_wins() {
    const REDECLARE: &str = r#"
nes.registerExtension("p4", ["scene.read"]);
var result = null;
function probe() {
  try { nes.audio.play("beep", 0.5); result = "ok"; }
  catch (e) { result = "" + e; }
}
nes.registerExtension("p4", ["scene.read", "audio"]);
nes.onUpdate(function () { probe(); });
function __nes_result() { return result; }
"#;
    let mut rig = setup(REDECLARE);
    rig.ext.update(); // 第二次声明（含 audio）已生效 -> play 通过
    assert!(rig.ext.take_last_error().is_none());
    assert_eq!(
        rig.probe("__nes_result"),
        NesValue::str("ok"),
        "re-declaration with audio grants play (static, last wins)"
    );
    {
        let h = rig.host.borrow();
        assert_eq!(h.played.borrow().len(), 1);
    }
}

/// 守卫不遮蔽返回值/参数：包装后的能力调用参数原样透传、返回值原样返回
/// （find -> null 的值语义不因包装变形）。
#[test]
fn guard_is_transparent_for_args_and_returns() {
    const TRANSPARENT: &str = r#"
nes.registerExtension("p5", ["scene.read"]);
nes.onUpdate(function () {
  seen = nes.scene.find("missing");
});
var seen = 42;
function __nes_seen() { return seen; }
"#;
    let mut rig = setup(TRANSPARENT);
    rig.ext.update();
    assert!(rig.ext.take_last_error().is_none());
    // find("missing") = null（值语义 —— 不是 undefined、不被包装吞掉）。
    assert_eq!(rig.probe("__nes_seen"), NesValue::Null);
}
