//! NES 2.0 渲染后端：wgpu-native（v29 资产）实机后端 —— M4 / S4「最小可视闭环」。
//!
//! # 这个 crate 是什么
//!
//! 方案 D 的 **叶子**：只依赖契约层 [`nes_render_api`]，实现
//! [`RenderServer`](nes_render_api::RenderServer)（属性级推送）并消费
//! [`RenderCommand`](nes_render_api::RenderCommand)（线性命令流），
//! 把"要画什么"翻译成 GPU 侧动作。
//!
//! 依赖方向（`Cargo.toml` 的 G8/G9/G10 注释 + `check_dependency_direction.py`）：
//!
//! ```text
//! nes-asset ──▶ nes-scene ──▶ nes-render-extract ──▶ nes-render-api ──▶ 本 crate
//! ```
//!
//! 本 crate 是这条链的终点：**不得**反向被依赖，也**不得**看见场景层与资源层。
//!
//! # 本层不做什么（边界纪律）
//!
//! - 不认识 `nes-scene` 的节点树、不认识 `nes-asset` 的资产库：句柄与资源键
//!   都只是契约层的 `u64` 位模式；
//! - 不做文本排版：`SetText` 只登记，字形度量留在 CPU 侧（契约层已把
//!   `LabelState` 的排版归属写死在文档里）；
//! - 不碰窗口 / 表面：S4.1 是**离屏**闭环（纹理 → 读回 → PNG），
//!   这样"渲染是否正确"与"窗口系统是否可用"可以分开验证。
//!
//! # 为什么手写 FFI 而不引入 `wgpu` / `wgpu-native` crate
//!
//! 不引入 `wgpu-rs`（需要 build script + 原生工具链）与 `bindgen`（需要
//! `libclang`）：手写 `#[repr(C)]` + 运行时 `LoadLibraryW` 把"能不能编译"与
//! "装没装工具链"解耦 —— 与契约层坚持零依赖是同一条理由。S4.1 已在本机
//! （MSVC 工具链 + wgpu-native v29.0.1.1 资产）实机跑通最小可视闭环；
//! 更早的"本机没有链接器"口径已过期，随 S4.1 封口更正（归档说明 §4.11）。
//! 绑定与 ABI 结论见 [`ffi`]，运行时装配见 [`gpu`]，命令消费见 [`renderer`]。
//!
//! # 分层
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`ffi`] | `#[repr(C)]` 结构体 + 枚举常量 + 运行时符号解析（无第三方依赖） |
//! | [`gpu`] | 动态库定位、实例/适配器/设备、离屏目标与像素读回、精灵图集、纹理注册表 |
//! | [`renderer`] | [`RenderServer`](nes_render_api::RenderServer) 实现 + 命令消费器 + 精灵管线 |
//! | [`ttf`] | 手写 TrueType 子集解析器 + 标量灰度光栅化器（S12-10 第 1 期） |
//! | `glyph` | TTF 动态字形图集：shelf 装箱 + (char, 字号) 缓存（S12-11 第 2 期，内部模块） |
//! | [`bmp`] | 极简 BMP 装载器（外部预处理产物 -> RGBA，供纹理注册表上传） |
//! | [`png`] | 手写 PNG 编码器（导出可视证据） |
//! | [`error`] | 每一个失败点都能指名道姓的 [`BackendError`] |
//!
//! # 怎么跑
//!
//! ```text
//! cargo run --example s41_visual_closure      # 清屏 + 精灵 → PNG（output 目录）
//! cargo test                                  # 单元 + 出口准则测试
//! ```
//!
//! 动态库路径按 [`gpu::locate_library`] 的候选顺序解析（可用环境变量
//! `NES_RENDER_WGPU_LIB` 覆盖）。找不到资产时**不会**静默失败：返回
//! [`BackendError::NoLibraryCandidates`] 并列出所有已尝试的路径。
//!
//! # S4.1 的"如实报告"纪律
//!
//! 如果 wgpu-native 加载失败、适配器/设备请求失败或驱动拒绝命令，
//! 本 crate 一律返回带上下文的 [`BackendError`]，**不伪造截图、不谎报跑通**：
//! 读回的像素只在真正提交并映射成功后才会变成 PNG。

#![deny(missing_docs)]
#![deny(rust_2018_idioms)]

pub mod bmp;
pub mod error;
/// wgpu-native 的 C ABI 绑定。
///
/// 模块内所有 `repr(C)` 结构体都是 `webgpu.h` 同名字段的逐字段直译，
/// 字段级文档在此豁免（重复头文件注释没有信息量），语义决策写在类型级注释里。
#[allow(missing_docs)]
pub mod ffi;
mod glyph;
pub mod gpu;
pub mod png;
pub mod renderer;
pub mod ttf;
pub mod window;

pub use crate::error::BackendError;
pub use crate::gpu::{
    DeviceInfo, FrameImage, GpuContext, RenderTarget, SpriteAtlas, SurfaceFrame, SurfaceTarget,
    TextureRegistry,
};
pub use crate::png::{encode_rgba8, write_rgba8_png};
pub use crate::renderer::{
    CommandConsumer, FontParams, FrameOutcome, FrameStats, SpritePipeline, WgpuRenderServer,
};

/// 本 crate 版本（封口文档、示例日志与产出物清单用它对齐）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
