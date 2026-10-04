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
//! * [`JsExtension`] —— [`nes_extension_api::ExtensionLifecycle`] 的 JS
//!   实现（update 钩子经全局蹦床转发到 JS 注册的回调）。
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

pub use binding::{CapabilityBinding, JsExtension, NES_BOOTSTRAP_JS};
pub use runtime::RquickjsRuntime;
pub use value::{js_to_nes, nes_to_js};
