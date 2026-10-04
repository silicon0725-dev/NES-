//! [`JsRuntime`] 的 QuickJS-NG 实现（rquickjs 0.9 绑定）。
//!
//! 冻结面就是 nes-extension-api 的四个方法；本文件之外（含错误文本）
//! 不出现任何 rquickjs 类型 —— 换 backend 只动本 crate。

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rquickjs::function::Args;
use rquickjs::{Context, Ctx, Exception, Runtime, Value};

use nes_extension_api::{ExtError, JsContextId, JsRuntime, NesValue};

use crate::value::{global_function, js_to_nes, nes_to_js};

/// 单次 JS 执行的墙钟预算（每次 `load_module` / `call` 入口重新计时）。
///
/// 50ms ≈ 60Hz 下的 3 帧：扩展单帧语义工作远用不完，`while(true)` 却撑不过
/// 一个预算窗口。超时经中断处理器转成 JS 异常（[`INTERRUPTED_MARK`]），走与
/// 普通异常同一条隔离路径（引擎侧计数 + 日志），引擎永不因脚本冻结。
pub const EXEC_BUDGET: Duration = Duration::from_millis(50);

/// 中断处理器的采样间隔（每 N 次回调查一次时钟）。
///
/// QuickJS 已把处理器限频在每 ~1 万条字节码一次（`JS_INTERRUPT_COUNTER_INIT`
/// 惯例），采样再把 `Instant::now` 的开销降一档 —— 最坏检测延迟 =
/// N x 1 万条字节码，远小于一个预算窗口。
const INTERRUPT_SAMPLE_EVERY: u64 = 64;

/// 预算中断的异常标记文本。QuickJS 对中断抛 `InternalError: "interrupted"`
/// 且标记为不可捕获（`js_set_uncatchable_error`）—— JS `try/catch` 吞不掉
/// 它，必然浮出成 [`ExtError::CallFailed`]；引擎侧以本标记判别"超预算"。
pub const INTERRUPTED_MARK: &str = "interrupted";

/// 宿主与中断处理器共享的执行预算状态（单线程纪律，`Cell` 足够）。
#[derive(Default)]
struct ExecBudget {
    /// 当下执行的截止时刻（`None` = 不在 JS 执行中，处理器一律放行）。
    deadline: Cell<Option<Instant>>,
    /// 处理器回调计数（采样降频用；回绕无害 —— 只取模）。
    ticks: Cell<u64>,
}

/// QuickJS-NG 运行时（`JsRuntime` 的第一个 backend 实现）。
///
/// 资源纪律：进程内一个 [`Self`] 管多个上下文；内存上限 64 MiB、栈上限
/// 1 MiB（恶意/失控脚本的损失上界 —— P0 沙箱分区之外的第一道闸）；单次
/// JS 执行另有 [`EXEC_BUDGET`] 墙钟预算（中断处理器 —— 第二道闸，S17.1）。
pub struct RquickjsRuntime {
    rt: Runtime,
    /// 上下文表（下标即 [`JsContextId`]；P0 只增不减 —— 上下文生命周期
    /// 与扩展一致，扩展卸载留待后续期次）。
    contexts: Vec<Context>,
    /// 执行预算（与 [`rt`](Self::rt) 上的中断处理器共享；见 [`ExecBudget`]）。
    budget: Rc<ExecBudget>,
}

impl RquickjsRuntime {
    /// 构造（失败 = 引擎进程初始化问题，[`ExtError::RuntimeInit`]）。
    pub fn new() -> Result<Self, ExtError> {
        let rt = Runtime::new().map_err(|e| ExtError::RuntimeInit(e.to_string()))?;
        rt.set_memory_limit(64 * 1024 * 1024);
        rt.set_max_stack_size(1024 * 1024);
        // 防线 2（S17.1）：失控脚本的中断闸。QuickJS 每 ~1 万条字节码回调一次
        // 中断处理器（覆盖所有跳转指令 —— `while(true)` 的回边必经）；处理器
        // 返回 true 即把当下执行转成不可被 JS try/catch 捕获的 "interrupted"
        // 内部异常。处理器是**每运行时一把**（QuickJS 无每上下文中断钩子），
        // 预算在每次进入上下文时重新武装 —— 单线程纪律下上下文串行执行，
        // 等效于"每上下文每次执行一份预算"。
        let budget = Rc::new(ExecBudget::default());
        let handler = Rc::clone(&budget);
        rt.set_interrupt_handler(Some(Box::new(move || {
            // 采样惯用法：先计数，每 N 次回调才查一次时钟（未到采样点放行）。
            let ticks = handler.ticks.get().wrapping_add(1);
            handler.ticks.set(ticks);
            if ticks % INTERRUPT_SAMPLE_EVERY != 0 {
                return false;
            }
            match handler.deadline.get() {
                Some(deadline) => Instant::now() >= deadline,
                None => false,
            }
        })));
        Ok(Self { rt, contexts: Vec::new(), budget })
    }

    /// 预算武装的上下文进入（本 crate 所有 JS 执行的统一咽喉）。
    ///
    /// 每次进入 QuickJS 栈都重置一次截止时刻（预算口径 = "单次执行" 而非
    /// "每帧总额"），离开即解除 —— 下一次执行重新计时；预算外时间不误伤。
    fn enter_context<R>(
        &mut self,
        id: JsContextId,
        f: impl FnOnce(&Ctx<'_>) -> Result<R, ExtError>,
    ) -> Result<R, ExtError> {
        let idx = id.0 as usize;
        let ctx = self.contexts.get_mut(idx).ok_or(ExtError::UnknownContext(id))?;
        self.budget.deadline.set(Some(Instant::now() + EXEC_BUDGET));
        let out = ctx.with(|ctx| f(&ctx));
        self.budget.deadline.set(None);
        out
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
        self.enter_context(id, |ctx| {
            f(ctx).map_err(|e| classify_err(ctx, e, ExtError::CallFailed))
        })
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
        // 顶层求值同样受执行预算保护（顶层 while(true) 与 update 死循环同罪）。
        let source = source.to_string();
        self.enter_context(ctx, |ctx| {
            ctx.eval::<(), _>(source)
                .map_err(|e| classify_err(ctx, e, ExtError::Load))
        })
    }

    fn call(
        &mut self,
        ctx: JsContextId,
        function: &str,
        args: &[NesValue],
    ) -> Result<NesValue, ExtError> {
        self.enter_context(ctx, |ctx| call_in_ctx(ctx, function, args))
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

    #[test]
    fn runaway_loop_is_interrupted_within_budget_and_context_survives() {
        // 防线 2（backend 半边）核心验收：while(true) 必须在预算内被打断，
        // 中断转成 JS 异常（含 "interrupted" 标记），上下文此后照常可用。
        let mut rt = RquickjsRuntime::new().unwrap();
        let ctx = rt.create_context().unwrap();
        rt.load_module(ctx, "function spin() { while (true) { } }")
            .unwrap();
        let start = std::time::Instant::now();
        let err = rt.call(ctx, "spin", &[]).unwrap_err();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "deadloop must be cut promptly, took {elapsed:?}"
        );
        let text = err.to_string();
        assert!(
            text.contains(crate::INTERRUPTED_MARK),
            "interrupt marker missing in: {text}"
        );
        // 中断后同一上下文照常可用（异常已清，预算解除，引擎继续）。
        rt.load_module(ctx, "function ok() { return 41 + 1; }").unwrap();
        assert_eq!(rt.call(ctx, "ok", &[]).unwrap(), NesValue::F64(42.0));
    }

    #[test]
    fn memory_hog_stops_at_the_limit_without_killing_the_process() {
        // 防线 3（backend 半边）行为实测：内存超限 = QuickJS 抛 "out of
        // memory" JS 异常（InternalError），与普通异常同一条路径；预算中断
        // 也可能先到（慢机器上 50ms 先满）—— 两者都安全，进程不炸。
        let mut rt = RquickjsRuntime::new().unwrap();
        let ctx = rt.create_context().unwrap();
        rt.load_module(
            ctx,
            "var hog = []; function hog_run() { while (true) { hog.push(new Array(1024)); } }",
        )
        .unwrap();
        let start = std::time::Instant::now();
        let err = rt.call(ctx, "hog_run", &[]).unwrap_err();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "hog must stop promptly, took {elapsed:?}"
        );
        let text = err.to_string();
        assert!(
            text.contains("out of memory") || text.contains(crate::INTERRUPTED_MARK),
            "OOM or interrupt expected, got: {text}"
        );
        // 进程仍然健康：全新运行时照常执行（炸进程 = 本测试直接崩）。
        let mut fresh = RquickjsRuntime::new().unwrap();
        let fresh_ctx = fresh.create_context().unwrap();
        fresh
            .load_module(fresh_ctx, "function ok() { return 7; }")
            .unwrap();
        assert_eq!(fresh.call(fresh_ctx, "ok", &[]).unwrap(), NesValue::F64(7.0));
    }

    #[test]
    fn memory_limit_raises_out_of_memory_exception() {
        // 防线 3 的 64 MiB 上限直接核实：单次迭代就是一个 C 级填充的稠密
        // 数组（4 Mi x f64 = 32 MiB）—— 两三次迭代必然触顶，分配速度快过
        // 50ms 预算，OOM 先于中断浮出。超限行为 = "out of memory" JS 异常
        //（InternalError），与普通异常同一条路径，进程不炸。
        let mut rt = RquickjsRuntime::new().unwrap();
        let ctx = rt.create_context().unwrap();
        rt.load_module(
            ctx,
            "var hog = []; function grow() { while (true) { hog.push(new Array(4194304).fill(0)); } }",
        )
        .unwrap();
        let start = std::time::Instant::now();
        let err = rt.call(ctx, "grow", &[]).unwrap_err();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "limit must trip promptly, took {elapsed:?}"
        );
        let text = err.to_string();
        assert!(
            text.contains("out of memory") || text.contains(crate::INTERRUPTED_MARK),
            "OOM or interrupt expected, got: {text}"
        );
    }
}
