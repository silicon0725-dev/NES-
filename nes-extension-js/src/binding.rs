//! 能力对象注入：把宿主实现的能力 traits 绑成 JS 全局对象 `nes`。
//!
//! # 冻结的 JS 面（S17 第 1 期，P0）
//!
//! ```text
//! nes.scene.find(name)                    -> NodeRef(number) | null   （需 "scene.read"）
//! nes.node.getPos(ref)                    -> {x, y} 形态省略，P0 返回 [x, y] | null
//!                                                                  （需 "scene.read"）
//! nes.node.setPos(ref, x, y)              -> undefined               （需 "scene.write"）
//! nes.node.setVisible(ref, v)             -> undefined               （需 "scene.write"）
//! nes.node.getName(ref)                   -> string | null           （需 "scene.read"）
//! nes.input.isPressed(name)               -> bool                    （需 "input"）
//! nes.audio.play(key, volume)             -> undefined               （需 "audio"）
//! nes.registerExtension(id[, perms[, opts]]) -> undefined（扩展自报身份 + S17.3 权限
//!                                             声明；缺省 perms = 全授予，声明期静态；
//!                                             S17.5 第三参 { strict: true } = 缺省
//!                                             perms 改为全拒——default-deny 选入）
//! nes.onUpdate(fn)                        -> undefined   （P0 单回调槽；fn 可为生成器函数
//!                                             —— S17.3 C4 帧驱动协程）
//! nes.util.{clamp,lerp,sign,dist,rand,randInt}  -> S17.5 C6 工具函数集（纯 JS；
//!                                             rand/randInt 为**非确定性分区**——
//!                                             扩展自行选入，引擎确定性承诺不覆盖）
//! nes.storage.{get,set,has,remove,keys}   -> S17.5 C7 扩展级存储（本扩展 id 命名
//!                                             空间；声明期自动创建；值域 = JSON 面；
//!                                             生命周期 = 运行时会话）
//! nes.storage.at(id)                      -> 显式跨扩展访问（同上下文互信模型）
//! nes.onSignal(name, fn)                  -> undefined   （S17.2 注册 hat，需 "signal"；
//!                                            同名多个 handler = 都调，注册序；fn 可为
//!                                            生成器函数 —— 每次触发新建实例）
//! nes.emitSignal(name, payload)           -> undefined   （S17.2 反向发射，需 "signal"；
//!                                            落地时序由宿主取走时机决定）
//! ```
//!
//! 权限名全集（S17.3 B3）：`scene.read` / `scene.write` / `input` /
//! `audio` / `signal`。未授予的能力调用抛
//! `Error("permission denied: <cap>")` → 既有 fault 隔离路径（不炸不静默）。
//!
//! NodeRef 在 JS 侧是**不透明 number**（P0 位形 < 2^53，double 精确承载）。
//!
//! # 设计取舍
//!
//! * 绑定经 `__nes_*` 扁平全局函数 + 一段 [`NES_BOOTSTRAP_JS`] 组装 ——
//!   所有 **JS 值（回调函数等）都留在 JS 堆**（`__nes_update_hook` 全局槽），
//!   Rust 侧不持有任何 JS 句柄：GC 安全由构造保证，无需 Persistent 根。
//! * 闭包持 `Rc<RefCell<dyn Capability>>`；**闭包体零 panic**（`try_borrow`
//!   失败静默让路 —— panic 跨 C FFI 边界是未定义行为，不可赌）。
//! * `getPos` P0 返回 `[x, y]` 数组（值边界已有 Array，无需新变体）。

use std::cell::RefCell;
use std::rc::Rc;

use rquickjs::function::Func;
use rquickjs::IntoJs;
use rquickjs::{Array, Ctx, Value};
#[allow(unused_imports)]
use nes_extension_api::JsRuntime;

use nes_extension_api::{
    AudioCapability, ExtError, ExtensionLifecycle, InputCapability, JsContextId, NesValue,
    NodeCapability, NodeRef, SceneCapability, SignalCapability,
};

use crate::runtime::RquickjsRuntime;
use crate::value::js_to_nes;

/// 扩展协程容量上界（S17.3 C4；**每扩展**的活动生成器实例数）。
///
/// 真源是 [`NES_BOOTSTRAP_JS`] 里的 `__nes_coro_cap`（调度器在 JS 侧，
/// Rust 不参与驱动）—— 本常量是它与 Rust 世界的同步锚（集成测试核对
/// 两侧字面量一致），超限行为 = 拒新留旧 + 抛错（走既有 fault 路径）。
pub const COROUTINE_CAP: u32 = 32;

/// `nes` 对象组装 + update 蹦床 + 信号 hat 表（ASCII；在能力函数注入**之后**求值）。
///
/// S17.2 信号面（照 onUpdate 的形态：JS 值留在 JS 堆，Rust 侧零句柄）：
/// * `__nes_signal_handlers`：名字 -> handler 数组（`nes.onSignal` 注册序）；
/// * `nes.onSignal(name, fn)`：handler 入 JS 堆表 + 经 `__nes_signal_subscribe`
///   向宿主声明订阅名一次（泵过滤器据此装配）；**需 "signal" 权限**（S17.3）；
/// * `__nes_signal_dispatch(name, payload)`：宿主派发入口 —— 逐 handler
///   try/catch（单个 handler 抛错不殃及同表后续 handler），首个错误在循环
///   后重抛 —— 错误文本沿标准 `rt.call` 错误路径浮出成 fault（S17.1 隔离），
///   预算中断（uncatchable）直接浮出；
/// * `nes.emitSignal(name, payload)`：经 `__nes_signal_emit` 进宿主
///   SignalCapability（泵内派发期间 = 同泵级联；update 期 = 下帧泵）；
///   **需 "signal" 权限**（S17.3）。
///
/// S17.3 C4 生成器协程（纯 JS 侧调度器，每扩展一份 —— 上下文即沙箱边界）：
/// * `onUpdate` / `onSignal` 的处理器**可为生成器函数**（`function*`）——
///   包装判定：调用返回值有 `.next`（生成器对象）→ 进入调度器；普通返回值
///   = 一次性执行（现状）。语法糖不需要存在：`yield n` 即 "停 n 帧"；
/// * `__nes_coros`：活动生成器表 `{gen, wait}`；`__nes_update` 每帧先
///   [`drive`](C4 帧驱动) 全表再调一次性钩子 —— `wait > 0` 减一（归零当帧
///   仍停，次帧推进）；`wait == 0` 推进 `gen.next()`；`done` 移除；`value`
///   为正数 → `wait = floor(value)`，其余（裸 yield / 非数 / 非正数）→ 1/0
///   （裸 yield = 停一帧；`yield 0` = 不停顿）；
/// * hat 触发 = **新建生成器实例**（Scratch startHats 重入语义）：派发即推
///   首段（预算内），后续帧随 `__nes_update` 推进；同 hat 并发多实例；总
///   量超 [`COROUTINE_CAP`] → 拒新留旧 + 抛错（fault 计数）；
/// * 生成器内 throw → 逐表项 try/catch、首错循环后重抛 → 既有 fault 隔离
///   （S17.1）；生成器推进在 `__nes_update`/派发同一次 `rt.call` 内 =
///   同一份 ExecBudget 预算。
///
/// S17.3 B3 权限模型（声明期静态；裁决点 = 能力注入处的方法包装）：
/// * `nes.registerExtension(id[, perms[, opts]])`：第二参为权限名数组（如
///   `["scene.read", "scene.write", "input", "audio", "signal"]`）——
///   **缺省 = 全授予**（hello.js 兼容）；S17.5 起第三参可选
///   `{ strict: true }` = **default-deny 选入**：strict 模式下 perms 缺省
///   从"全授予"变为"全拒"（声明了什么才有什么；显式数组两侧模式同义）；
///   空数组 = 全拒绝；未知名忽略（前向兼容）；注册期定死，运行期无提权；
/// * `__nes_guard(cap, fn)`：逐调用裁决 —— 未授予抛
///   `Error("permission denied: <cap>")` → 既有 fault 隔离路径（不炸不静默）。
///
/// S17.5 C6 工具函数集（`nes.util.*`，bootstrap 内纯 JS —— 零 Rust 句柄）：
/// * `clamp(v, min, max)` / `lerp(a, b, t)` / `sign(v)`：数学三件套
///   （clamp 对 min > max 做交换 = 全函数；sign 返回 -1/0/1，NaN → 0）；
/// * `dist(x1, y1, x2, y2)`：欧氏距离（shake.js 手搓 sqrt 的收编点）；
/// * `rand(min, max)` / `randInt(min, max)`：均匀随机（randInt 含两端整数，
///   两者对 min > max 均做交换）——**非确定性分区**：扩展自行选入，引擎
///   核心确定性承诺（headless 基线/regression）不覆盖扩展内部随机。
///
/// S17.5 C7 扩展级存储（`nes.storage`，纯 JS —— 值留 JS 堆，Rust 零句柄）：
/// * 声明期自动命名空间：`registerExtension(id, ...)` 即建
///   `__nes_storage[String(id)]`（隔离单元 = 扩展 id；根表与各命名空间都用
///   `Object.create(null)` —— `__proto__`/`constructor` 等键按普通属性处理，
///   无原型链走私）；重复注册同 id 不清库（会话内幂等）；
/// * 每 id 的 store 面：`get(key, defaultValue)`（缺省缺省值 = null）、
///   `set(key, value)`、`has(key)`、`remove(key)`、`keys()`；
/// * 值域 = JSON 可序列化面（NesValue 同族）：set 经 `JSON.stringify`
///   试编码（函数/undefined/环 → 抛 TypeError，嵌套函数由 replacer 抓），
///   入库值 = `JSON.parse` 重建的纯 JSON 面；get 返回深拷贝（改返回值不落
///   库，也无法把活对象/函数走私进存储）；
/// * `nes.storage.at(id)`：显式跨扩展访问（返回同款 store 面）——信任模型
///   = 同上下文互信（隐私隔离归权限后续期）；
/// * 生命周期 = **运行时会话**：上下文在即存续；扩展停用不清、运行时重建
///   即清（落盘持久化归后续期）。
pub const NES_BOOTSTRAP_JS: &str = r#"
globalThis.__nes_signal_handlers = {};
globalThis.__nes_coros = [];
globalThis.__nes_coro_cap = 32;
globalThis.__nes_grants = null;
globalThis.__nes_storage = Object.create(null);
globalThis.__nes_is_generator = function (v) {
  return v !== null && typeof v === "object" && typeof v.next === "function";
};
globalThis.__nes_allowed = function (cap) {
  var g = globalThis.__nes_grants;
  return g === null || g.indexOf(cap) !== -1;
};
globalThis.__nes_guard = function (cap, fn) {
  return function () {
    if (!globalThis.__nes_allowed(cap)) {
      throw new Error("permission denied: " + cap);
    }
    return fn.apply(null, arguments);
  };
};
globalThis.__nes_coro_step = function (e) {
  var r = e.gen.next();
  if (r.done) { return false; }
  var w = 1;
  if (typeof r.value === "number" && isFinite(r.value)) {
    w = Math.floor(r.value);
    if (w < 0) { w = 0; }
  }
  e.wait = w;
  return true;
};
globalThis.__nes_coro_start = function (gen) {
  if (globalThis.__nes_coros.length >= globalThis.__nes_coro_cap) {
    throw new Error("coroutine cap exceeded: max " + globalThis.__nes_coro_cap + " active generators per extension");
  }
  var e = { gen: gen, wait: 0 };
  if (globalThis.__nes_coro_step(e)) { globalThis.__nes_coros.push(e); }
};
globalThis.__nes_coro_drive = function () {
  var table = globalThis.__nes_coros;
  var keep = [];
  var firstError = null;
  for (var i = 0; i < table.length; i++) {
    var e = table[i];
    var alive = true;
    try {
      if (e.wait > 0) { e.wait = e.wait - 1; }
      else { alive = globalThis.__nes_coro_step(e); }
    } catch (err) {
      alive = false;
      if (firstError === null) { firstError = err; }
    }
    if (alive) { keep.push(e); }
  }
  globalThis.__nes_coros = keep;
  if (firstError !== null) { throw firstError; }
};
globalThis.__nes_store_methods = function (idOf) {
  var slot = function () {
    var s = globalThis.__nes_storage[idOf()];
    return s === undefined ? null : s;
  };
  var ensure = function () {
    var root = globalThis.__nes_storage;
    var id = idOf();
    if (root[id] === undefined) { root[id] = Object.create(null); }
    return root[id];
  };
  return {
    get: function (key, defaultValue) {
      var s = slot();
      if (s === null || !Object.prototype.hasOwnProperty.call(s, key)) {
        return defaultValue === undefined ? null : defaultValue;
      }
      return JSON.parse(JSON.stringify(s[key]));
    },
    set: function (key, value) {
      var text = JSON.stringify(value, function (k, v) {
        if (typeof v === "function") {
          throw new TypeError("storage: values must be JSON-serializable");
        }
        return v;
      });
      if (text === undefined) {
        throw new TypeError("storage: values must be JSON-serializable");
      }
      ensure()[key] = JSON.parse(text);
    },
    has: function (key) {
      var s = slot();
      return s !== null && Object.prototype.hasOwnProperty.call(s, key);
    },
    remove: function (key) {
      var s = slot();
      if (s !== null) { delete s[key]; }
    },
    keys: function () {
      var s = slot();
      return s === null ? [] : Object.keys(s);
    }
  };
};
globalThis.__nes_own_store_id = function () {
  var id = globalThis.__nes_extension_id;
  if (id === undefined || id === null || id === "") {
    throw new Error("storage: register the extension before using its own store");
  }
  return String(id);
};
globalThis.nes = {
  scene: { find: globalThis.__nes_guard("scene.read", globalThis.__nes_scene_find) },
  node: {
    getPos: globalThis.__nes_guard("scene.read", globalThis.__nes_node_get_pos),
    setPos: globalThis.__nes_guard("scene.write", globalThis.__nes_node_set_pos),
    setVisible: globalThis.__nes_guard("scene.write", globalThis.__nes_node_set_visible),
    getName: globalThis.__nes_guard("scene.read", globalThis.__nes_node_get_name)
  },
  input: { isPressed: globalThis.__nes_guard("input", globalThis.__nes_input_is_pressed) },
  audio: { play: globalThis.__nes_guard("audio", globalThis.__nes_audio_play) },
  util: {
    clamp: function (v, min, max) {
      v = +v; min = +min; max = +max;
      if (min > max) { var t = min; min = max; max = t; }
      if (v < min) { return min; }
      if (v > max) { return max; }
      return v;
    },
    lerp: function (a, b, t) { return +a + (+b - +a) * +t; },
    sign: function (v) { v = +v; return (v > 0) - (v < 0); },
    dist: function (x1, y1, x2, y2) {
      var dx = +x2 - +x1;
      var dy = +y2 - +y1;
      return Math.sqrt(dx * dx + dy * dy);
    },
    rand: function (min, max) {
      min = +min; max = +max;
      if (min > max) { var t = min; min = max; max = t; }
      return min + Math.random() * (max - min);
    },
    randInt: function (min, max) {
      min = +min; max = +max;
      if (min > max) { var t = min; min = max; max = t; }
      return Math.floor(min + Math.random() * (max - min + 1));
    }
  },
  storage: globalThis.__nes_store_methods(globalThis.__nes_own_store_id),
  registerExtension: function (id, perms, opts) {
    globalThis.__nes_extension_id = id;
    var strict = opts !== undefined && opts !== null && opts.strict === true;
    if (perms === undefined || perms === null) {
      globalThis.__nes_grants = strict ? [] : null;
    } else {
      globalThis.__nes_grants = Array.from(perms, function (p) { return String(p); });
    }
    var root = globalThis.__nes_storage;
    var sid = String(id);
    if (root[sid] === undefined) { root[sid] = Object.create(null); }
  },
  onUpdate: function (fn) { globalThis.__nes_update_hook = fn; },
  onSignal: globalThis.__nes_guard("signal", function (name, fn) {
    if (typeof fn !== "function") { throw new Error("onSignal: handler must be a function"); }
    var t = globalThis.__nes_signal_handlers;
    if (t[name] === undefined) {
      t[name] = [];
      globalThis.__nes_signal_subscribe(name);
    }
    t[name].push(fn);
  }),
  emitSignal: globalThis.__nes_guard("signal", globalThis.__nes_signal_emit)
};
globalThis.__nes_set_extension_id = function (id) { globalThis.__nes_extension_id = id; };
globalThis.__nes_get_extension_id = function () {
  return globalThis.__nes_extension_id === undefined ? null : globalThis.__nes_extension_id;
};
globalThis.nes.storage.at = function (id) {
  if (id === undefined || id === null || id === "") {
    throw new Error("storage.at: extension id required");
  }
  return globalThis.__nes_store_methods(function () { return String(id); });
};
globalThis.__nes_update = function () {
  globalThis.__nes_coro_drive();
  var hook = globalThis.__nes_update_hook;
  if (typeof hook === "function") {
    var ret = hook();
    if (globalThis.__nes_is_generator(ret)) {
      globalThis.__nes_update_hook = null;
      globalThis.__nes_coro_start(ret);
    }
  }
};
globalThis.__nes_signal_dispatch = function (name, payload) {
  var list = globalThis.__nes_signal_handlers[name];
  if (list === undefined) { return 0; }
  var called = 0;
  var firstError = null;
  for (var i = 0; i < list.length; i++) {
    try {
      var ret = list[i](payload);
      if (globalThis.__nes_is_generator(ret)) { globalThis.__nes_coro_start(ret); }
      called = called + 1;
    } catch (e) {
      if (firstError === null) { firstError = e; }
    }
  }
  if (firstError !== null) { throw firstError; }
  return called;
};
"#;

/// 能力绑定集：五个能力 trait 的宿主实现（共享句柄形态）。
///
/// `Rc<RefCell<...>>` 形态的理由：同一个宿主实现要同时被"装进 QuickJS
/// 闭包"（'static）与"每帧刷新"（宿主侧 &mut）两端触达 —— 共享计数 +
/// 运行期借用检查是最小足够机制（单线程纪律与 QuickJS 一致）。
pub struct CapabilityBinding {
    scene: Rc<RefCell<dyn SceneCapability>>,
    node: Rc<RefCell<dyn NodeCapability>>,
    input: Rc<RefCell<dyn InputCapability>>,
    audio: Rc<RefCell<dyn AudioCapability>>,
    signal: Rc<RefCell<dyn SignalCapability>>,
}

/// `nes.node.getPos(ref)` 的实现体（具名生命周期统一 ctx 与返回值）。
fn node_get_pos_js<'js>(
    node: &Rc<RefCell<dyn NodeCapability>>,
    ctx: Ctx<'js>,
    r: f64,
) -> rquickjs::Result<Value<'js>> {
    let pos = match node.try_borrow() {
        Ok(ok) => ok.get_pos(NodeRef(r as u64)),
        // 借用冲突 = 宿主侧重入 bug，如实抛 JS 异常（不吞）。
        Err(_) => {
            return Err(rquickjs::Error::FromJs {
                from: "capability",
                to: "RefCell",
                message: Some("node capability busy".into()),
            })
        }
    };
    let Some((x, y)) = pos else {
        return Ok(Value::new_null(ctx));
    };
    let arr = Array::new(ctx.clone())?;
    arr.set(0, x as f64)?;
    arr.set(1, y as f64)?;
    Ok(arr.into_value())
}

/// `nes.scene.find(name)` 的实现体：Some -> number，None -> null。
fn scene_find_js<'js>(
    scene: &Rc<RefCell<dyn SceneCapability>>,
    ctx: Ctx<'js>,
    name: String,
) -> rquickjs::Result<Value<'js>> {
    let found = match scene.try_borrow() {
        // 借用冲突 = 宿主侧重入 bug，如实抛 JS 异常（不吞）。
        Ok(ok) => ok.find(&name),
        Err(_) => {
            return Err(rquickjs::Error::FromJs {
                from: "capability",
                to: "RefCell",
                message: Some("scene capability busy".into()),
            })
        }
    };
    Ok(match found {
        Some(r) => (r.0 as f64).into_js(&ctx)?,
        None => Value::new_null(ctx),
    })
}

/// `nes.node.getName(ref)` 的实现体：Some -> string，None -> null。
fn node_get_name_js<'js>(
    node: &Rc<RefCell<dyn NodeCapability>>,
    ctx: Ctx<'js>,
    r: f64,
) -> rquickjs::Result<Value<'js>> {
    let name = match node.try_borrow() {
        Ok(ok) => ok.get_name(NodeRef(r as u64)),
        Err(_) => {
            return Err(rquickjs::Error::FromJs {
                from: "capability",
                to: "RefCell",
                message: Some("node capability busy".into()),
            })
        }
    };
    Ok(match name {
        Some(s) => s.into_js(&ctx)?,
        None => Value::new_null(ctx),
    })
}

impl CapabilityBinding {
    /// 由宿主实现组装（宿主持有 Rc 端，本 binding 持另一端）。
    pub fn new(
        scene: Rc<RefCell<dyn SceneCapability>>,
        node: Rc<RefCell<dyn NodeCapability>>,
        input: Rc<RefCell<dyn InputCapability>>,
        audio: Rc<RefCell<dyn AudioCapability>>,
        signal: Rc<RefCell<dyn SignalCapability>>,
    ) -> Self {
        Self { scene, node, input, audio, signal }
    }

    /// 把 `nes` 对象注入指定上下文（能力函数 + 引导脚本）。
    pub fn install(&self, rt: &mut RquickjsRuntime, ctx: JsContextId) -> Result<(), ExtError> {
        rt.with_context(ctx, |ctx: &Ctx<'_>| self.install_in_ctx(ctx))
    }

    /// 实际注入（在 `Context::with` 栈内执行）。
    fn install_in_ctx(&self, ctx: &Ctx<'_>) -> Result<(), rquickjs::Error> {
        let globals = ctx.globals();

        let scene = Rc::clone(&self.scene);
        globals.set(
            "__nes_scene_find",
            // None -> **null**（不是 undefined：rquickjs 0.9 的 Option IntoJs
            // 把 None 映射成 undefined，而能力契约里"未找到"是个值语义）。
            Func::new(move |ctx, name: String| scene_find_js(&scene, ctx, name)),
        )?;

        let node = Rc::clone(&self.node);
        globals.set(
            "__nes_node_get_pos",
            // 返回 [x, y] 数组（rquickjs 0.9 无裸元组 IntoJs —— 手工组 Array；
            // 生命周期经具名辅助函数统一，闭包不做返回类型注解）。
            Func::new(move |ctx, r| node_get_pos_js(&node, ctx, r)),
        )?;

        let node = Rc::clone(&self.node);
        globals.set(
            "__nes_node_set_pos",
            Func::new(move |r: f64, x: f64, y: f64| {
                if let Ok(mut ok) = node.try_borrow_mut() {
                    ok.set_pos(NodeRef(r as u64), x as f32, y as f32);
                }
            }),
        )?;

        let node = Rc::clone(&self.node);
        globals.set(
            "__nes_node_set_visible",
            Func::new(move |r: f64, v: bool| {
                if let Ok(mut ok) = node.try_borrow_mut() {
                    ok.set_visible(NodeRef(r as u64), v);
                }
            }),
        )?;

        let node = Rc::clone(&self.node);
        globals.set(
            "__nes_node_get_name",
            // 同 find：None -> null（不是 undefined）。
            Func::new(move |ctx, r: f64| node_get_name_js(&node, ctx, r)),
        )?;

        let input = Rc::clone(&self.input);
        globals.set(
            "__nes_input_is_pressed",
            Func::new(move |name: String| -> bool {
                match input.try_borrow() {
                    Ok(ok) => ok.is_pressed(&name),
                    Err(_) => false,
                }
            }),
        )?;

        let audio = Rc::clone(&self.audio);
        globals.set(
            "__nes_audio_play",
            Func::new(move |key: String, volume: f64| {
                if let Ok(ok) = audio.try_borrow() {
                    ok.play(&key, volume as f32);
                }
            }),
        )?;

        // S17.2 信号面：订阅声明 + 反向发射（载荷过 NesValue 边界）。
        let signal = Rc::clone(&self.signal);
        globals.set(
            "__nes_signal_subscribe",
            Func::new(move |name: String| {
                if let Ok(mut ok) = signal.try_borrow_mut() {
                    ok.on_signal(&name);
                }
            }),
        )?;

        let signal = Rc::clone(&self.signal);
        globals.set(
            "__nes_signal_emit",
            // 载荷就地过 js_to_nes（函数值读作 Null —— 与返回值边界同一口径）；
            // 借用冲突静默让路（与写能力同口径 —— 闭包体零 panic）。
            Func::new(move |_ctx: Ctx<'_>, name: String, v: Value<'_>| -> rquickjs::Result<()> {
                let payload = js_to_nes(&v).map_err(|_| rquickjs::Error::FromJs {
                    from: "payload",
                    to: "NesValue",
                    message: Some("emitSignal payload conversion failed".into()),
                })?;
                if let Ok(mut ok) = signal.try_borrow_mut() {
                    ok.emit(&name, payload);
                }
                Ok(())
            }),
        )?;

        // 引导脚本：组装 `nes` 对象 + update 蹦床 + 扩展身份槽 + 信号 hat 表。
        ctx.eval::<(), _>(NES_BOOTSTRAP_JS.to_string())
    }
}

/// 一个已装载的 JS 扩展（[`ExtensionLifecycle`] 的 JS 实现）。
///
/// 与 `RquickjsRuntime` 共享（`Rc<RefCell>`）：扩展句柄只占一个上下文
/// 号，执行统一走运行时的冻结面（`call`）—— 换 backend 时本类型只改
/// 构造端。
pub struct JsExtension {
    runtime: Rc<RefCell<RquickjsRuntime>>,
    ctx: JsContextId,
    id: String,
    last_error: Option<String>,
}

impl JsExtension {
    /// 关联运行时与上下文（上下文已装载源码并注入能力）。
    pub fn new(runtime: Rc<RefCell<RquickjsRuntime>>, ctx: JsContextId) -> Self {
        Self { runtime, ctx, id: String::new(), last_error: None }
    }

    /// 扩展 id（注册后有效）。
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 所属上下文句柄。
    pub fn context(&self) -> JsContextId {
        self.ctx
    }

    /// 装载扩展源码（顶层求值；扩展在此期间可调 `nes.registerExtension`
    /// 与 `nes.onUpdate`）。
    pub fn load_source(&mut self, source: &str) -> Result<(), ExtError> {
        self.runtime.borrow_mut().load_module(self.ctx, source)
    }

    /// 读 JS 侧自报的扩展 id（`nes.registerExtension(id)` 的落点；
    /// 未注册返回 `None`）。
    pub fn extension_id_from_js(&mut self) -> Option<String> {
        match self.runtime.borrow_mut().call(self.ctx, "__nes_get_extension_id", &[]) {
            Ok(NesValue::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// 取走并清空最后一次生命周期调用的错误（成功路径为 `None`）。
    pub fn take_last_error(&mut self) -> Option<String> {
        self.last_error.take()
    }
}

impl ExtensionLifecycle for JsExtension {
    fn register(&mut self, id: &str) {
        self.id = id.to_string();
        // 宿主驱动的注册（与 JS 侧 nes.registerExtension 汇合同一全局槽）。
        match self
            .runtime
            .borrow_mut()
            .call(self.ctx, "__nes_set_extension_id", &[NesValue::str(id)])
        {
            Ok(_) => {}
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    fn update(&mut self) {
        // 错误状态按次自洽（与 dispatch_signal 同一口径：每次调用先清上次
        // 残留 —— 成功路径的 last_error 必为 None）。
        self.last_error = None;
        // 经全局蹦床调 JS 注册的 onUpdate 回调（P0 单回调槽；S17.3 起蹦床
        // 先驱动生成器协程表，再调一次性钩子 —— 错误沿同一通道浮出）。
        match self.runtime.borrow_mut().call(self.ctx, "__nes_update", &[]) {
            Ok(_) => {}
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }
}

impl JsExtension {
    /// 向本扩展派发一条信号（S17.2 hat 重入；`__nes_signal_dispatch`
    /// 蹦床逐 handler try/catch，首个错误循环后重抛）。
    ///
    /// 返回 `Ok(命中 handler 数)`（蹦床返回值；仅观测用）或
    /// `Err(ExtError::CallFailed)`（handler 异常 / 预算中断 / 内存超限
    /// —— 三者同形态，隔离语义由宿主侧继承 S17.1）。
    pub fn dispatch_signal(&mut self, name: &str, payload: &NesValue) -> Result<usize, ExtError> {
        self.last_error = None;
        match self.runtime.borrow_mut().call(
            self.ctx,
            "__nes_signal_dispatch",
            &[NesValue::str(name), payload.clone()],
        ) {
            Ok(ret) => Ok(match ret {
                NesValue::F64(n) => n as usize,
                _ => 0,
            }),
            Err(e) => {
                self.last_error = Some(e.to_string());
                Err(e)
            }
        }
    }
}
