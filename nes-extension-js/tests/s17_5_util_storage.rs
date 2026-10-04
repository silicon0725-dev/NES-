//! S17.5 扩展生态 P2 收尾 —— 绑定层契约测试（mock 宿主，无引擎在场）。
//!
//! * T-U-01：`nes.util.*` 工具函数集 —— clamp 边界（含 min>max 交换）、
//!   lerp 端点与中点、sign 三态、dist 直角三角形 3-4-5、rand 值域内
//!   100 次抽样、randInt 整数且含两端；
//! * T-S-01：`nes.storage` 隔离 —— 同上下文两个扩展 id 各自 set/get 互
//!   不可见；`at(id)` 显式跨访成立；**跨上下文**完全不可见（上下文即
//!   沙箱边界 —— 生产宿主每扩展一上下文）；
//! * T-S-02：生命周期 = 运行时会话 —— set 后模拟停用（不再 update +
//!   onUpdate(null)）→ get 仍在；同 id 重注册不清库；新上下文（运行时
//!   重建）即清；
//! * T-D-01：default-deny 选入 —— strict + 无 perms → 能力调用抛
//!   permission denied；strict + 声明 scene.read → find 可用、setPos
//!   仍拒；显式 `{ strict: false }` = 全授予（非 strict 行为不变）；
//! * 值域纪律：函数 / undefined / 环引用 set 即抛；get 返回 JSON 深拷贝
//!   （改返回值不落库）；`__proto__` 键按普通属性处理（无原型走私）；
//!   未注册就取本扩展 store 面 → 显式报错。
//!
//! JS 字面量全 ASCII（仓库纪律）；引擎级半边见 nes-runtime
//! `tests/s17_5_game_ext.rs`（shake.js dogfooding 回归 T-GE-01..03 照绿）。

use std::cell::RefCell;
use std::rc::Rc;

use nes_extension_api::{
    AudioCapability, ExtError, ExtensionLifecycle, InputCapability, JsContextId, JsRuntime,
    NesValue, NodeCapability, NodeRef, SceneCapability, SignalCapability,
};
use nes_extension_js::{CapabilityBinding, JsExtension, RquickjsRuntime};

/// mock 宿主：五项能力全记账（只需要 find/getPos/setPos/audio 记账）。
#[derive(Default)]
struct MockHost {
    find_calls: RefCell<Vec<String>>,
    set_pos: RefCell<Vec<(u64, f32, f32)>>,
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
    fn is_pressed(&self, _name: &str) -> bool {
        false
    }
}

impl AudioCapability for MockHost {
    fn play(&self, key: &str, volume: f32) {
        self.played.borrow_mut().push((key.to_string(), volume));
    }
}

#[derive(Default)]
struct MockSignals;

impl SignalCapability for MockSignals {
    fn on_signal(&mut self, _name: &str) {}
    fn emit(&mut self, _name: &str, _payload: NesValue) {}
}

/// 一份会话 = 一个上下文 + 能力注入 + 扩展装载 + 宿主注册。
struct Session {
    ext: JsExtension,
    rt: Rc<RefCell<RquickjsRuntime>>,
    ctx: JsContextId,
    host: Rc<RefCell<MockHost>>,
}

/// 在同一运行时里开一份新会话（T-S-02 的"运行时重建"与 T-S-01 的跨上下文
/// 隔离都要第二/第三份上下文 —— 一个 RquickjsRuntime 管多上下文是既有面）。
fn spawn(
    rt: &Rc<RefCell<RquickjsRuntime>>,
    source: &str,
    fallback_id: &str,
) -> Session {
    let host = Rc::new(RefCell::new(MockHost::default()));
    let binding = CapabilityBinding::new(
        Rc::clone(&host) as Rc<RefCell<dyn SceneCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn NodeCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn InputCapability>>,
        Rc::clone(&host) as Rc<RefCell<dyn AudioCapability>>,
        Rc::new(RefCell::new(MockSignals)) as Rc<RefCell<dyn SignalCapability>>,
    );
    let ctx = rt.borrow_mut().create_context().unwrap();
    binding.install(&mut rt.borrow_mut(), ctx).unwrap();
    let mut ext = JsExtension::new(Rc::clone(rt), ctx);
    ext.load_source(source)
        .unwrap_or_else(|e| panic!("load failed: {e}"));
    assert!(ext.take_last_error().is_none(), "load must be clean");
    ext.register(fallback_id);
    Session { ext, rt: Rc::clone(rt), ctx, host }
}

impl Session {
    /// 调全局探针（带参，返回 NesValue；探针异常原样浮出）。
    fn call(&self, name: &str, args: &[NesValue]) -> Result<NesValue, ExtError> {
        self.rt.borrow_mut().call(self.ctx, name, args)
    }

    /// 数值探针（期望 F64）。
    fn num(&self, name: &str, args: &[NesValue]) -> f64 {
        match self.call(name, args).unwrap() {
            NesValue::F64(x) => x,
            other => panic!("{name} must return a number, got {other:?}"),
        }
    }
}

const UTIL_PROBES: &str = r#"
nes.registerExtension("u1");
function pClamp(v, min, max) { return nes.util.clamp(v, min, max); }
function pLerp(a, b, t) { return nes.util.lerp(a, b, t); }
function pSign(v) { return nes.util.sign(v); }
function pDist(x1, y1, x2, y2) { return nes.util.dist(x1, y1, x2, y2); }
function pRand(min, max) { return nes.util.rand(min, max); }
function pRandInt(min, max) { return nes.util.randInt(min, max); }
"#;

/// T-U-01：六个工具函数的数学断言（边界 / 端点 / 三态 / 3-4-5 / 抽样）。
#[test]
fn t_u_01_util_math_functions() {
    let rt = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
    let s = spawn(&rt, UTIL_PROBES, "u1");

    let f = |x| NesValue::F64(x);
    // clamp：内部值原样、下溢归 min、上溢归 max、两端边界、min>max 交换。
    assert_eq!(s.call("pClamp", &[f(5.0), f(0.0), f(10.0)]).unwrap(), f(5.0));
    assert_eq!(s.call("pClamp", &[f(-3.0), f(0.0), f(10.0)]).unwrap(), f(0.0));
    assert_eq!(s.call("pClamp", &[f(15.0), f(0.0), f(10.0)]).unwrap(), f(10.0));
    assert_eq!(s.call("pClamp", &[f(0.0), f(0.0), f(10.0)]).unwrap(), f(0.0));
    assert_eq!(s.call("pClamp", &[f(10.0), f(0.0), f(10.0)]).unwrap(), f(10.0));
    assert_eq!(s.call("pClamp", &[f(5.0), f(10.0), f(0.0)]).unwrap(), f(5.0));

    // lerp：两端点精确、中点线性。
    assert_eq!(s.call("pLerp", &[f(0.0), f(10.0), f(0.0)]).unwrap(), f(0.0));
    assert_eq!(s.call("pLerp", &[f(0.0), f(10.0), f(1.0)]).unwrap(), f(10.0));
    assert_eq!(s.call("pLerp", &[f(0.0), f(10.0), f(0.25)]).unwrap(), f(2.5));
    assert_eq!(s.call("pLerp", &[f(-2.0), f(2.0), f(0.5)]).unwrap(), f(0.0));

    // sign：三态（-1/0/1）。
    assert_eq!(s.call("pSign", &[f(-7.0)]).unwrap(), f(-1.0));
    assert_eq!(s.call("pSign", &[f(0.0)]).unwrap(), f(0.0));
    assert_eq!(s.call("pSign", &[f(3.5)]).unwrap(), f(1.0));

    // dist：直角三角形 3-4-5（IEEE 下 sqrt(25) 精确）。
    assert_eq!(s.call("pDist", &[f(0.0), f(0.0), f(3.0), f(4.0)]).unwrap(), f(5.0));
    assert_eq!(s.num("pDist", &[f(1.0), f(1.0), f(1.0), f(1.0)]), 0.0);

    // rand：100 次抽样全部落在 [min, max)。
    for _ in 0..100 {
        let v = s.num("pRand", &[f(2.0), f(5.0)]);
        assert!((2.0..5.0).contains(&v), "rand out of range: {v}");
    }

    // randInt：100 次抽样全部为整数且含两端 [min, max]。
    let mut saw_low = false;
    let mut saw_high = false;
    for _ in 0..100 {
        let v = s.num("pRandInt", &[f(1.0), f(6.0)]);
        assert_eq!(v.fract(), 0.0, "randInt must be an integer, got {v}");
        assert!((1.0..=6.0).contains(&v), "randInt out of range: {v}");
        saw_low |= v == 1.0;
        saw_high |= v == 6.0;
    }
    assert!(saw_low && saw_high, "randInt must be able to hit both ends");
}

const STORAGE_PROBES: &str = r#"
nes.registerExtension("extA");
function sOwnSet(k, v) { nes.storage.set(k, v); }
function sOwnGet(k) { return nes.storage.get(k); }
function sOwnGetDef(k, d) { return nes.storage.get(k, d); }
function sOwnHas(k) { return nes.storage.has(k); }
function sOwnRemove(k) { nes.storage.remove(k); }
function sOwnKeys() { return nes.storage.keys(); }
function sAtSet(id, k, v) { nes.storage.at(id).set(k, v); }
function sAtGet(id, k) { return nes.storage.at(id).get(k); }
function sAtKeys(id) { return nes.storage.at(id).keys(); }
function sAtHas(id, k) { return nes.storage.at(id).has(k); }
"#;

/// T-S-01：同上下文按 id 隔离 + `at(id)` 显式跨访；跨上下文完全不可见。
#[test]
fn t_s_01_storage_isolation_and_explicit_cross_access() {
    let rt = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
    let a = spawn(&rt, STORAGE_PROBES, "extA");

    let s = |x: &str| NesValue::str(x);
    // 各自写入各自命名空间。
    a.call("sOwnSet", &[s("onlyA"), NesValue::F64(1.0)]).unwrap();
    a.call("sAtSet", &[s("extB"), s("onlyB"), NesValue::F64(2.0)])
        .unwrap();

    // 本扩展面只见自己的键。
    assert_eq!(a.call("sOwnGet", &[s("onlyA")]).unwrap(), NesValue::F64(1.0));
    assert_eq!(a.call("sOwnGet", &[s("onlyB")]).unwrap(), NesValue::Null);
    assert_eq!(
        a.call("sOwnKeys", &[]).unwrap(),
        NesValue::arr([s("onlyA")]),
        "own keys must not leak the other namespace"
    );

    // at(id) 显式跨访：读得到对方键，也看不到自己的。
    assert_eq!(
        a.call("sAtGet", &[s("extB"), s("onlyB")]).unwrap(),
        NesValue::F64(2.0)
    );
    assert_eq!(
        a.call("sAtGet", &[s("extB"), s("onlyA")]).unwrap(),
        NesValue::Null
    );
    assert_eq!(
        a.call("sAtGet", &[s("extA"), s("onlyA")]).unwrap(),
        NesValue::F64(1.0)
    );
    assert_eq!(
        a.call("sAtKeys", &[s("extB")]).unwrap(),
        NesValue::arr([s("onlyB")])
    );
    assert_eq!(a.call("sAtHas", &[s("extB"), s("onlyB")]).unwrap(), NesValue::Bool(true));
    assert_eq!(a.call("sAtHas", &[s("extA"), s("onlyB")]).unwrap(), NesValue::Bool(false));

    // 跨上下文：第二个上下文（= 生产宿主里第二个扩展）同名 id 同键不同值
    // 完全不可见 —— 上下文即沙箱边界，__nes_storage 是上下文内全局。
    const EXT_B: &str = r#"
nes.registerExtension("extB");
function bSet(k, v) { nes.storage.set(k, v); }
function bGet(k) { return nes.storage.get(k); }
function bGetOther(id, k) { return nes.storage.at(id).get(k); }
"#;
    let b = spawn(&rt, EXT_B, "extB");
    b.call("bSet", &[s("onlyB"), NesValue::F64(99.0)]).unwrap();
    assert_eq!(b.call("bGet", &[s("onlyB")]).unwrap(), NesValue::F64(99.0));
    // 上下文 B 里 at("extA") 只见 B 自己上下文的空命名空间 —— 看不到 A 的值。
    assert_eq!(
        b.call("bGetOther", &[s("extA"), s("onlyA")]).unwrap(),
        NesValue::Null,
        "cross-context storage must be invisible (context = sandbox boundary)"
    );
    // A 侧同理看不到 B 上下文里刚写的 99。
    assert_eq!(
        a.call("sAtGet", &[s("extB"), s("onlyB")]).unwrap(),
        NesValue::F64(2.0),
        "A still sees its own context's extB namespace"
    );
}

/// T-S-02：生命周期 = 运行时会话 —— 停用不清、重注册不清、新上下文即清。
#[test]
fn t_s_02_storage_lives_for_the_runtime_session() {
    let rt = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
    const LIFE: &str = r#"
nes.registerExtension("life");
function lSet() { nes.storage.set("persist", 7); return nes.storage.get("persist"); }
function lGet() { return nes.storage.get("persist"); }
function lDisable() { nes.onUpdate(null); return nes.storage.get("persist"); }
function lReregister() { nes.registerExtension("life"); return nes.storage.get("persist"); }
"#;
    let s1 = spawn(&rt, LIFE, "life");
    let sv = |x| NesValue::F64(x);

    assert_eq!(s1.call("lSet", &[]).unwrap(), sv(7.0));
    // 模拟停用：宿主侧 disabled = 不再 update（本测试从此不调 update）；
    // JS 侧再显式摘掉 update 钩子 —— 存储不受影响。
    assert_eq!(s1.call("lDisable", &[]).unwrap(), sv(7.0));
    // 同 id 重注册（会话内幂等）：命名空间不清。
    assert_eq!(s1.call("lReregister", &[]).unwrap(), sv(7.0));
    assert_eq!(s1.call("lGet", &[]).unwrap(), sv(7.0));

    // 运行时重建 = 新上下文：同名 id 从空库开始（会话语义，非落盘）。
    let s2 = spawn(&rt, LIFE, "life2");
    assert_eq!(
        s2.call("lGet", &[]).unwrap(),
        NesValue::Null,
        "a rebuilt runtime starts with an empty namespace"
    );
    assert_eq!(s2.call("lSet", &[]).unwrap(), sv(7.0));
}

const DOMAIN_PROBES: &str = r#"
function dSetFn() {
  try { nes.storage.set("bad", function () {}); return "no-throw"; }
  catch (e) { return "" + e; }
}
function dSetNestedFn() {
  try { nes.storage.set("bad2", { cb: function () {} }); return "no-throw"; }
  catch (e) { return "" + e; }
}
function dSetUndefined() {
  try { nes.storage.set("bad3", undefined); return "no-throw"; }
  catch (e) { return "" + e; }
}
function dSetCircular() {
  var o = {};
  o.self = o;
  try { nes.storage.set("bad4", o); return "no-throw"; }
  catch (e) { return "" + e; }
}
function dNothingLanded() { return nes.storage.keys().length; }
function dCopySemantics() {
  nes.storage.set("cfg", { n: 1 });
  var v = nes.storage.get("cfg");
  v.n = 99;
  var back = nes.storage.get("cfg");
  return back.n;
}
function dJsonFaceSurvives() {
  nes.storage.set("mixed", { a: [1, "x", null, true], b: 2.5 });
  return nes.storage.get("mixed").a[2] === null && nes.storage.get("mixed").a[3] === true;
}
function dDefaultValues() {
  return nes.storage.get("missing") === null && nes.storage.get("missing", 5) === 5;
}
function dProtoKey() {
  nes.storage.set("__proto__", { polluted: true });
  return ({}).polluted === undefined && nes.storage.has("__proto__");
}
var unregError = "";
function dUnregError() { return unregError; }
try { nes.storage.get("k"); } catch (e) { unregError = "" + e; }
nes.registerExtension("dom-after");
function dAfterReg() { nes.storage.set("k", 1); return nes.storage.get("k"); }
"#;

/// 值域 = JSON 面：函数/undefined/环 set 即抛；get 是深拷贝；__proto__
/// 键不走私；未注册取本扩展面报显式错（顶层捕获时机 = 注册之前）。
#[test]
fn t_s_03_storage_value_domain_is_json_only() {
    let rt = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));
    let s = spawn(&rt, DOMAIN_PROBES, "dom");
    let txt = |name: &str| match s.call(name, &[]).unwrap() {
        NesValue::Str(t) => t,
        other => panic!("{name} must return a string, got {other:?}"),
    };

    // 顶层求值期（注册前）取本扩展 store 面 -> 显式报错（不是 undefined 漏网）。
    assert!(
        txt("dUnregError").contains("register the extension"),
        "unregistered own-store access must throw a clear error"
    );
    // 注册后同一上下文即可用（错误只在注册前）。
    assert_eq!(s.call("dAfterReg", &[]).unwrap(), NesValue::F64(1.0));

    // 函数（顶层与嵌套）与 undefined 拒收；环引用 stringify 必抛（文本
    // 随实现 —— 只钉"抛了且没落库"）。
    assert!(txt("dSetFn").contains("JSON-serializable"), "top-level function must throw");
    assert!(txt("dSetNestedFn").contains("JSON-serializable"), "nested function must throw");
    assert!(txt("dSetUndefined").contains("JSON-serializable"), "undefined must throw");
    let circular = txt("dSetCircular");
    assert!(!circular.contains("no-throw"), "circular reference must throw, got: {circular}");
    // 前面四次拒绝都没落库。
    assert_eq!(s.call("dNothingLanded", &[]).unwrap(), NesValue::F64(1.0));

    // get = JSON 深拷贝：改返回值不落库。
    assert_eq!(s.call("dCopySemantics", &[]).unwrap(), NesValue::F64(1.0));

    // JSON 面往返（数组/字符串/null/布尔/浮点都在 NesValue 同族）。
    assert_eq!(s.call("dJsonFaceSurvives", &[]).unwrap(), NesValue::Bool(true));

    // 缺省值口径：无第二参 = null，有第二参 = 调用者给值。
    assert_eq!(s.call("dDefaultValues", &[]).unwrap(), NesValue::Bool(true));

    // __proto__ 键 = 普通属性（Object.create(null) 命名空间），原型不被污染。
    assert_eq!(s.call("dProtoKey", &[]).unwrap(), NesValue::Bool(true));
}

const STRICT_DENY: &str = r#"
nes.registerExtension("d1", undefined, { strict: true });
nes.onUpdate(function () {
  nes.node.getPos(nes.scene.find("obj1"));
});
"#;

const STRICT_READ_ONLY: &str = r#"
nes.registerExtension("d2", ["scene.read"], { strict: true });
nes.onUpdate(function () {
  var r = nes.scene.find("obj1");
  nes.node.setPos(r, 1, 2);
});
"#;

const STRICT_FALSE: &str = r#"
nes.registerExtension("d3", undefined, { strict: false });
nes.onUpdate(function () {
  nes.audio.play("beep", 0.5);
});
"#;

/// T-D-01：default-deny 选入 —— strict + 无 perms = 全拒；strict + 声明
/// scene.read = 只读可用写仍拒；显式 strict:false = 全授予（缺省行为不变）。
#[test]
fn t_d_01_strict_mode_default_deny_opt_in() {
    let rt = Rc::new(RefCell::new(RquickjsRuntime::new().unwrap()));

    // (a) strict + 无声明：能力调用抛 permission denied（fault 路径）。
    let mut a = spawn(&rt, STRICT_DENY, "d1");
    a.ext.update();
    let err = a.ext.take_last_error().expect("strict mode must deny undeclared caps");
    assert!(
        err.contains("permission denied: scene.read"),
        "message missing: {err}"
    );
    assert!(a.host.borrow().find_calls.borrow().is_empty());

    // (b) strict + ["scene.read"]：find 真实到达宿主，setPos 仍拒（零写副作用）。
    let mut b = spawn(&rt, STRICT_READ_ONLY, "d2");
    b.ext.update();
    let err = b.ext.take_last_error().expect("scene.write must stay denied");
    assert!(err.contains("permission denied: scene.write"), "message missing: {err}");
    {
        let h = b.host.borrow();
        assert_eq!(h.find_calls.borrow().as_slice(), &["obj1".to_string()]);
        assert!(h.set_pos.borrow().is_empty(), "denied write must not land");
    }

    // (c) 显式 { strict: false }：缺省 = 全授予（hello.js 兼容两种模式的钉子）。
    let mut c = spawn(&rt, STRICT_FALSE, "d3");
    c.ext.update();
    assert!(c.ext.take_last_error().is_none(), "strict:false keeps grant-all default");
    assert_eq!(
        c.host.borrow().played.borrow().as_slice(),
        &[("beep".to_string(), 0.5)]
    );

    // (d) strict + 空数组 = 同全拒（空数组语义两模式一致）。
    const STRICT_EMPTY: &str = r#"
nes.registerExtension("d4", [], { strict: true });
nes.onUpdate(function () { nes.input.isPressed("Space"); });
"#;
    let mut d = spawn(&rt, STRICT_EMPTY, "d4");
    d.ext.update();
    let err = d.ext.take_last_error().expect("empty perms must deny under strict");
    assert!(err.contains("permission denied: input"), "message missing: {err}");
}
