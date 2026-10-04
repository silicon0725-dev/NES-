//! 能力对象注入：把宿主实现的能力 traits 绑成 JS 全局对象 `nes`。
//!
//! # 冻结的 JS 面（S17 第 1 期，P0）
//!
//! ```text
//! nes.scene.find(name)                    -> NodeRef(number) | null
//! nes.node.getPos(ref)                    -> {x, y} 形态省略，P0 返回 [x, y] | null
//! nes.node.setPos(ref, x, y)              -> undefined
//! nes.node.setVisible(ref, v)             -> undefined
//! nes.node.getName(ref)                   -> string | null
//! nes.input.isPressed(name)               -> bool
//! nes.audio.play(key, volume)             -> undefined
//! nes.registerExtension(id)               -> undefined   （扩展自报身份）
//! nes.onUpdate(fn)                        -> undefined   （P0 单回调槽）
//! nes.onSignal(name, fn)                  -> undefined   （S17.2 注册 hat；
//!                                            同名多个 handler = 都调，注册序）
//! nes.emitSignal(name, payload)           -> undefined   （S17.2 反向发射；
//!                                            落地时序由宿主取走时机决定）
//! ```
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

/// `nes` 对象组装 + update 蹦床 + 信号 hat 表（ASCII；在能力函数注入**之后**求值）。
///
/// S17.2 信号面（照 onUpdate 的形态：JS 值留在 JS 堆，Rust 侧零句柄）：
/// * `__nes_signal_handlers`：名字 -> handler 数组（`nes.onSignal` 注册序）；
/// * `nes.onSignal(name, fn)`：handler 入 JS 堆表 + 经 `__nes_signal_subscribe`
///   向宿主声明订阅名一次（泵过滤器据此装配）；
/// * `__nes_signal_dispatch(name, payload)`：宿主派发入口 —— 逐 handler
///   try/catch（单个 handler 抛错不殃及同表后续 handler），首个错误在循环
///   后重抛 —— 错误文本沿标准 `rt.call` 错误路径浮出成 fault（S17.1 隔离），
///   预算中断（uncatchable）直接浮出；
/// * `nes.emitSignal(name, payload)`：经 `__nes_signal_emit` 进宿主
///   SignalCapability（泵内派发期间 = 同泵级联；update 期 = 下帧泵）。
pub const NES_BOOTSTRAP_JS: &str = r#"
globalThis.__nes_signal_handlers = {};
globalThis.nes = {
  scene: { find: globalThis.__nes_scene_find },
  node: {
    getPos: globalThis.__nes_node_get_pos,
    setPos: globalThis.__nes_node_set_pos,
    setVisible: globalThis.__nes_node_set_visible,
    getName: globalThis.__nes_node_get_name
  },
  input: { isPressed: globalThis.__nes_input_is_pressed },
  audio: { play: globalThis.__nes_audio_play },
  registerExtension: function (id) { globalThis.__nes_extension_id = id; },
  onUpdate: function (fn) { globalThis.__nes_update_hook = fn; },
  onSignal: function (name, fn) {
    if (typeof fn !== "function") { throw new Error("onSignal: handler must be a function"); }
    var t = globalThis.__nes_signal_handlers;
    if (t[name] === undefined) {
      t[name] = [];
      globalThis.__nes_signal_subscribe(name);
    }
    t[name].push(fn);
  },
  emitSignal: globalThis.__nes_signal_emit
};
globalThis.__nes_set_extension_id = function (id) { globalThis.__nes_extension_id = id; };
globalThis.__nes_get_extension_id = function () {
  return globalThis.__nes_extension_id === undefined ? null : globalThis.__nes_extension_id;
};
globalThis.__nes_update = function () {
  var hook = globalThis.__nes_update_hook;
  if (typeof hook === "function") { hook(); }
};
globalThis.__nes_signal_dispatch = function (name, payload) {
  var list = globalThis.__nes_signal_handlers[name];
  if (list === undefined) { return 0; }
  var called = 0;
  var firstError = null;
  for (var i = 0; i < list.length; i++) {
    try {
      list[i](payload);
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
        // 经全局蹦床调 JS 注册的 onUpdate 回调（P0 单回调槽）。
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
