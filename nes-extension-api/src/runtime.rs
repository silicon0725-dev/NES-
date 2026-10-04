//! 可换 backend 的 JS 执行面。
//!
//! S17 用户裁决：QuickJS-NG 只是 Extension Execution Runtime 的**第一个**
//! backend（Boa / quickjs-rs 是未来可选实现方）。上层只依赖本 trait ——
//! **真正要冻结的是 Extension API，不是某个 JS 引擎**。
//!
//! 与用户草图的一致性：四个方法原样落地；签名仅按 ergonomics 做了
//! 一处微调 —— `ctx` 以 `JsContextId`（Copy 句柄）**按值**传入而非引用
//!（句柄由各 backend 自行解释，本 crate 不假设其结构）。

/// JS 执行上下文句柄（backend 本地有效；跨 backend 无意义）。
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct JsContextId(pub u64);

/// JS 执行运行时（backend 无关的最小面）。
///
/// P0 语义约定：
/// * `load_module` 把源码作为**全局脚本**求值（顶层 `function`/`var` 落成
///   全局符号）—— 扩展不是 ES 模块（P0 不做 import/export）；
/// * `call` 按名调用**全局函数**（值经 [`NesValue`] 边界双向转换）；
/// * `collect` 触发一次 GC（backend 自选实现深度，宿主每帧或按需调用）。
pub trait JsRuntime {
    /// 创建一个新上下文，返回其句柄。
    fn create_context(&mut self) -> Result<JsContextId, crate::ExtError>;

    /// 把源码装入指定上下文（全局脚本求值；顶层符号落成全局）。
    fn load_module(
        &mut self,
        ctx: JsContextId,
        source: &str,
    ) -> Result<(), crate::ExtError>;

    /// 按名调用上下文里的全局函数，入参/返回值都走 [`NesValue`] 边界。
    fn call(
        &mut self,
        ctx: JsContextId,
        function: &str,
        args: &[crate::NesValue],
    ) -> Result<crate::NesValue, crate::ExtError>;

    /// 触发一次垃圾回收。
    fn collect(&mut self);
}
