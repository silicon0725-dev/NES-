//! NES 2.0 JS 扩展运行时（S17 第 1 期）：QuickJS-NG backend（经 rquickjs）。
//!
//! # 边界
//!
//! QuickJS **不是 NES 的"第二 Runtime"**——它只是 Extension Execution
//! Runtime：JS 扩展只能通过注入的 `nes` 能力对象操作引擎（场景查询 /
//! 节点读写 / 输入快照 / 音频触发 / 扩展注册），永远不直接碰
//! SceneTree / Runtime / Renderer / WGPU。引擎类型不进 JS，JS 值不进
//! 引擎 —— 双向都走 [`nes_extension_api::NesValue`] 冻结边界。
//!
//! # 组成
//!
//! * [`RquickjsRuntime`] —— [`nes_extension_api::JsRuntime`] 的 QuickJS-NG
//!   实现（create_context / load_module / call / collect）；
//! * [`CapabilityBinding`] —— 把宿主实现的能力 traits 绑进全局对象
//!   `nes`（`nes.scene.find` / `nes.node.getPos|setPos|setVisible|getName`
//!   / `nes.input.isPressed` / `nes.audio.play` / `nes.registerExtension`
//!   / `nes.onUpdate`）；**不需要引擎在场 —— mock traits 即可驱动**；
//!   S17.3 起 bootstrap 内含两件纯 JS 机制：**生成器协程调度器**
//!   （`onUpdate`/`onSignal` 处理器可为 `function*`，帧计数驱动，见
//!   [`COROUTINE_CAP`]）与**权限守卫**（`registerExtension` 第二参
//!   声明权限数组，能力调用逐调用裁决，未授予 = JS 异常走 fault 隔离）；
//!   S17.5 再收三件纯 JS 小件：**工具函数集** `nes.util.*`（数学三件套 +
//!   `dist` + 非确定性分区的 `rand`/`randInt`）、**扩展级存储**
//!   `nes.storage`（声明期按 id 自动命名空间，值域 = JSON 面，生命周期 =
//!   运行时会话）与 **default-deny 选入**（`registerExtension` 第三参
//!   `{ strict: true }` —— strict 下 perms 缺省 = 全拒）；
//! * [`JsExtension`] —— [`nes_extension_api::ExtensionLifecycle`] 的 JS
//!   实现（update 钩子经全局蹦床转发到 JS 注册的回调）。
//!
//! # 失控脚本的闸门（S17.1）
//!
//! * [`EXEC_BUDGET`] —— 单次 JS 执行的墙钟预算（50ms；每次 load/call 入口
//!   重新计时），超时经 QuickJS 中断处理器转成 JS 异常（[`INTERRUPTED_MARK`]，
//!   不可被 JS try/catch 捕获）；引擎侧据此计数/停用（nes-runtime）。
//! * 内存上限 64 MiB + 栈上限 1 MiB —— 超限是同一形态的 JS 异常
//!   （"out of memory"），同一条隔离路径。
//!
//! # 与 backend 冻结面的关系
//!
//! 本 crate 的 Rust 面只有 nes-extension-api 的类型（+自身类型）；任何
//! rquickjs/QuickJS 类型都不得出现在本 crate 之外 —— 换 backend（Boa /
//! quickjs-rs）时只动本 crate。

#![deny(missing_docs)]

mod binding;
mod runtime;
mod value;

pub use binding::{CapabilityBinding, COROUTINE_CAP, JsExtension, NES_BOOTSTRAP_JS};
pub use runtime::{EXEC_BUDGET, INTERRUPTED_MARK, RquickjsRuntime};
pub use value::{js_to_nes, nes_to_js};
