//! [`JsRuntime`] 的 QuickJS-NG 实现（rquickjs 0.9 绑定）。
//!
//! 冻结面就是 nes-extension-api 的四个方法；本文件之外（含错误文本）
//! 不出现任何 rquickjs 类型 —— 换 backend 只动本 crate。

use rquickjs::function::Args;
use rquickjs::{Context, Ctx, Exception, Runtime, Value};

use nes_extension_api::{ExtError, JsContextId, JsRuntime, NesValue};

use crate::value::{global_function, js_to_nes, nes_to_js};

/// QuickJS-NG 运行时（`JsRuntime` 的第一个 backend 实现）。
///
/// 资源纪律：进程内一个 [`Self`] 管多个上下文；内存上限 64 MiB、栈上限
/// 1 MiB（恶意/失控脚本的损失上界 —— P0 沙箱分区之外的第一道闸）。
pub struct RquickjsRuntime {
    rt: Runtime,
    /// 上下文表（下标即 [`JsContextId`]；P0 只增不减 —— 上下文生命周期
    /// 与扩展一致，扩展卸载留待后续期次）。
    contexts: Vec<Context>,
}

impl RquickjsRuntime {
    /// 构造（失败 = 引擎进程初始化问题，[`ExtError::RuntimeInit`]）。
    pub fn new() -> Result<Self, ExtError> {
        let rt = Runtime::new().map_err(|e| ExtError::RuntimeInit(e.to_string()))?;
        rt.set_memory_limit(64 * 1024 * 1024);
        rt.set_max_stack_size(1024 * 1024);
        Ok(Self { rt, contexts: Vec::new() })
    }

    /// 在指定上下文内执行一段闭包（能力注入的统一入口）。
    ///
    /// 错误一律归一为 [`ExtError::CallFailed`]；JS 异常文本就地捕获
    ///（`Ctx` 句柄只在 `with` 栈内有效，出了栈就拿不到挂起异常）。
    pub fn with_context<R>(
        &mut self,
        id: JsContextId,
        f: impl FnOnce(&Ctx<'_>) -> Result<R, rquickjs::Error>,
    ) -> Result<R, ExtError> {
        let idx = id.0 as usize;
        let ctx = self.contexts.get_mut(idx).ok_or(ExtError::UnknownContext(id))?;
        // Context::with 进入 QuickJS 栈并落回 —— 跨 FFI 边界保持单线程纪律。
        ctx.with(|ctx| f(&ctx).map_err(|e| classify_err(&ctx, e, ExtError::CallFailed)))
    }
}

/// rquickjs 错误 -> 带异常文本的 `ExtError`（JS 异常挂起在 ctx 上，就地取）。
fn classify_err<E>(
    ctx: &Ctx<'_>,
    e: rquickjs::Error,
    wrap: impl FnOnce(String) -> E,
) -> E {
    if matches!(e, rquickjs::Error::Exception) {
        let caught: Value = ctx.catch();
        let text = caught
            .into_object()
            .and_then(Exception::from_object)
            .map(|ex| ex.to_string())
            .unwrap_or_else(|| "unknown exception".to_string());
        wrap(format!("JS exception: {text}"))
    } else {
        wrap(e.to_string())
    }
}

/// `call` 的栈内实现（动态参数经 `Args` 逐个压入 —— rquickjs 0.9 无切片
/// 展开传参）。
fn call_in_ctx<'js>(
    ctx: &Ctx<'js>,
    function: &str,
    args: &[NesValue],
) -> Result<NesValue, ExtError> {
    let func = match global_function(ctx, function) {
        Ok(f) => f,
        Err(_) => {
            return Err(ExtError::CallFailed(format!(
                "global function not found: {function}"
            )))
        }
    };
    let mut js_args = Args::new_unsized(ctx.clone());
    for a in args {
        let v = nes_to_js(ctx, a)
            .map_err(|e| classify_err(ctx, e, ExtError::Convert))?;
        js_args
            .push_arg(v)
            .map_err(|e| classify_err(ctx, e, ExtError::CallFailed))?;
    }
    let ret: Value = func
        .call_arg(js_args)
        .map_err(|e| classify_err(ctx, e, ExtError::CallFailed))?;
    js_to_nes(&ret).map_err(|e| classify_err(ctx, e, ExtError::Convert))
}

impl JsRuntime for RquickjsRuntime {
    fn create_context(&mut self) -> Result<JsContextId, ExtError> {
        let ctx =
            Context::full(&self.rt).map_err(|e| ExtError::ContextCreate(e.to_string()))?;
        self.contexts.push(ctx);
        Ok(JsContextId((self.contexts.len() - 1) as u64))
    }

    fn load_module(&mut self, ctx: JsContextId, source: &str) -> Result<(), ExtError> {
        // 全局脚本求值：顶层 function/var 落成全局符号（P0 扩展即全局脚本，
        // 不是 ES 模块 —— 见 crate 文档）。语法/求值失败归 [`ExtError::Load`]。
        let idx = ctx.0 as usize;
        let handle = self.contexts.get_mut(idx).ok_or(ExtError::UnknownContext(ctx))?;
        handle.with(|ctx| {
            ctx.eval::<(), _>(source.to_string())
                .map_err(|e| classify_err(&ctx, e, ExtError::Load))
        })
    }

    fn call(
        &mut self,
        ctx: JsContextId,
        function: &str,
        args: &[NesValue],
    ) -> Result<NesValue, ExtError> {
        let idx = ctx.0 as usize;
        let handle = self.contexts.get_mut(idx).ok_or(ExtError::UnknownContext(ctx))?;
        handle.with(|handle| call_in_ctx(&handle, function, args))
    }

    fn collect(&mut self) {
        // 全量 GC（QuickJS 是引用计数 + 周期回收器；宿主每帧或按需调用）。
        self.rt.run_gc();
    }
}

#[cfg(test)]
mod tests {
    use nes_extension_api::{ExtError, JsContextId, JsRuntime, NesValue};

    use crate::runtime::RquickjsRuntime;

    #[test]
    fn eval_arithmetic_load_call_round_trip() {
        let mut rt = RquickjsRuntime::new().unwrap();
        let ctx = rt.create_context().unwrap();
        // 算术求值 + 函数定义 + 带参调用（返回对象 = 值边界往返）。
        rt.load_module(
            ctx,
            "function add(a, b) { return a + b; }
             function describe(x) { return { kind: \"point\", x: x }; }
             function sum3(a, b, c) { return a + b + c; }",
        )
        .unwrap();
        assert_eq!(
            rt.call(ctx, "add", &[NesValue::F64(2.0), NesValue::F64(3.0)]).unwrap(),
            NesValue::F64(5.0)
        );
        // 多参（验证 Args 动态压参不是单参误装）。
        assert_eq!(
            rt.call(
                ctx,
                "sum3",
                &[NesValue::F64(1.0), NesValue::F64(2.0), NesValue::F64(3.0)]
            )
            .unwrap(),
            NesValue::F64(6.0)
        );
        match rt.call(ctx, "describe", &[NesValue::F64(7.0)]).unwrap() {
            NesValue::Object(pairs) => {
                assert_eq!(pairs.len(), 2);
                assert_eq!(pairs[0], ("kind".to_string(), NesValue::str("point")));
                assert_eq!(pairs[1], ("x".to_string(), NesValue::F64(7.0)));
            }
            other => panic!("expected object, got {other:?}"),
        }
        rt.collect();
    }

    #[test]
    fn multiple_contexts_are_isolated() {
        let mut rt = RquickjsRuntime::new().unwrap();
        let a = rt.create_context().unwrap();
        let b = rt.create_context().unwrap();
        rt.load_module(a, "function who() { return \"a\"; }").unwrap();
        rt.load_module(b, "function who() { return \"b\"; }").unwrap();
        assert_eq!(rt.call(a, "who", &[]).unwrap(), NesValue::str("a"));
        assert_eq!(rt.call(b, "who", &[]).unwrap(), NesValue::str("b"));
    }

    #[test]
    fn failures_surface_as_ext_errors() {
        let mut rt = RquickjsRuntime::new().unwrap();
        let ctx = rt.create_context().unwrap();
        // 语法错误 -> Load。
        let err = rt.load_module(ctx, "function ( {").unwrap_err();
        assert!(matches!(err, ExtError::Load(_)), "{err}");
        // 缺函数 -> CallFailed。
        let err = rt.call(ctx, "no_such_function", &[]).unwrap_err();
        assert!(matches!(err, ExtError::CallFailed(_)), "{err}");
        // 脚本运行期异常 -> CallFailed（异常文本随行）。
        rt.load_module(ctx, "function boom() { throw new Error(\"oops\"); }")
            .unwrap();
        let err = rt.call(ctx, "boom", &[]).unwrap_err();
        match err {
            ExtError::CallFailed(text) => {
                assert!(text.contains("oops"), "exception text missing: {text}");
            }
            other => panic!("expected CallFailed, got {other:?}"),
        }
        // 未知上下文 -> UnknownContext。
        let err = rt.call(JsContextId(99), "x", &[]).unwrap_err();
        assert_eq!(err, ExtError::UnknownContext(JsContextId(99)));
    }
}
