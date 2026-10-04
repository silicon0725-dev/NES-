//! NES 2.0 扩展 ABI 层（S17 第 1 期）：**要冻结的是 Extension API，不是某个 JS 引擎**。
//!
//! # 这一层是什么
//!
//! S17 用户裁决把 JS 扩展定位为 **Extension Execution Runtime**，而不是 NES 的
//! "第二 Runtime"：JS 扩展只能通过 NES 给它的能力对象操作引擎
//! （`nes.registerExtension(...)` / `nes.scene.find(...)` / `nes.input.isPressed(...)`
//! / `nes.audio.play(...)` / `nes.node.get(...)`），**永远不直接碰
//! SceneTree / Runtime / Renderer / WGPU**。本 crate 就是那条边界上的**冻结面**：
//!
//! ```text
//! nes-extension-api（本 crate：纯 ABI，零依赖）
//!   ├─ NesValue            引擎 <-> 脚本的值边界（自有类型，不暴露任何第三方类型）
//!   ├─ JsRuntime           可换 backend 的 JS 执行面（create_context / load_module /
//!   │                      call / collect）—— QuickJS-NG 只是它的第一个实现方
//!   ├─ 能力 traits         Scene / Node / Input / Audio（宿主实现、JS 侧绑定）
//!   └─ ExtensionLifecycle  扩展生命周期（注册 + 每帧 update 钩子）
//!
//! nes-extension-api ──▶ nes-extension-js（QuickJS-NG 绑定实现，G15 白名单）
//!            └───────▶ nes-runtime（能力宿主：把 traits 桥到真引擎）
//! ```
//!
//! # 依赖纪律（G14）
//!
//! **零第三方、零仓库内依赖**（normal / dev / build 三类都不得有）：任何第三方
//! 类型出现在这里都会泄进冻结面，换 backend 时就成了破绽。JS 引擎类型
//! 只允许出现在 `nes-extension-js`（第三方白名单家族 = rquickjs，G15）。
//!
//! # 确定性口径
//!
//! 扩展写树 = 游戏状态（进指纹）。headless 下 JS 执行本身是确定性的
//! （同输入同字节码同求值序），但脚本若依赖宿主注入的不确定源（`Date`、
//! `Math.random` —— P0 不禁用但文档警告），确定性即由脚本作者自证。

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod capability;
mod error;
mod runtime;
mod value;

pub use capability::{
    AudioCapability, ExtensionLifecycle, InputCapability, NodeCapability, NodeRef, SceneCapability,
};
pub use error::ExtError;
pub use runtime::{JsContextId, JsRuntime};
pub use value::NesValue;
