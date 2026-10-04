//! NES 2.0 渲染契约层 —— M4 / S1「契约冻结」产物（零依赖）。
//!
//! - 依据调研报告：`m4_render_borrow_research.md`（方案 D：渲染服务端化 + 单向依赖 + 每帧提取）
//! - 冻结文档：`NES2.0_M4渲染接入_S1契约冻结_v1.md`（冻结签名、出口准则对照、开放问题）
//! - 方向守卫：`check_dependency_direction.py`（基于 `cargo metadata`，位于 output 根目录）
//!
//! # 这个 crate 是什么
//!
//! 「节点/场景树 → 渲染物」的**属性级推送契约**。它只描述*说什么*，不描述*怎么做*：
//! 不含 GPU、窗口、表面概念，不认识 `wgpu` / `wgpu-native` / `winit`，
//! 也不认识 `nes-scene` 的节点树 —— 后端与场景层都只依赖本 crate 的类型，互不依赖。
//!
//! 三条来自调研报告的硬事实决定了这个形状：
//!
//! 1. 渲染层的原生输入是**属性集合**，不是上层语义快照
//!    （Godot RID / scratch-render Drawable 的属性级 update 接口）；
//! 2. 依赖方向必须**单向**：`renderer → scene`，scene 不得知道 renderer
//!    （Fyrox ARCHITECTURE.md 原文）；
//! 3. 语义 → 渲染物的转换**只允许发生一次**，发生在提取/推送那一步
//!    （Bevy `ExtractSchedule`、Realism `Scene::extract`）。
//!
//! # 硬约束（冻结，不得协商）
//!
//! 1. **零依赖**：`[dependencies]` 为空，连同项目 crate 也不行（理由见 `Cargo.toml`）；
//! 2. **方向单向**：`nes-scene` 的依赖树中不得出现 `nes-render-*`（含 dev-dependencies），
//!    由 `tools/check_dep_direction.ps1` 自动判定；
//! 3. **零 unsafe**：`#![forbid(unsafe_code)]` —— 契约层没有指针可玩；
//! 4. **不含语义**：本 crate 不认识 Sprite / Label 之外的积木语义，
//!    Scratch 语义只允许出现在 M5 兼容层，且位于渲染路径**之外**；
//! 5. **不改上游**：本 crate 不依赖 `nes-scene` / `nes-asset`，也不要求它们改动一行。
//!
//! # 分层中的位置
//!
//! ```text
//! nes-scene ──✗──▶ nes-render-*          （场景层不得知道渲染层；脚本可自动判定）
//! nes-render-extract ──▶ nes-scene        （提取层依赖场景，合法）
//! nes-render-extract ──▶ nes-render-api   （S2）
//! nes-render-backend ──▶ nes-render-api   （后端依赖契约，不依赖场景；S4）
//! ```
//!
//! # 契约表面（S1 冻结的部分）
//!
//! | 类型 | 职责 |
//! |---|---|
//! | [`ItemHandle`] | 后端侧**易变**句柄（等价 Godot RID），不得序列化 |
//! | [`RenderAssetKey`] | 稳定资源身份在渲染侧的投影，与 M3 位编码一致 |
//! | [`RenderItem`] | 渲染物通用属性集合（`Copy`，可零分配入缓冲） |
//! | [`DrawKey`] | 绘制次序全序键，后端排序的唯一许可入口 |
//! | [`RenderCommand`] | 后端要执行的动作（线性缓冲，S4 后端直接消费） |
//! | [`FrameInfo`] | 帧上下文 |
//! | [`RenderServer`] | 属性级推送 trait（对象安全） |
//! | [`Flip`] | 缺口 1：翻转合成（子局部后乘） |
//! | [`Camera2DState`] | 缺口 2：视图矩阵（唯一权威算式） |
//! | [`LabelState`] | 缺口 3：文本状态（`Arc<str>`，克隆不复制字节） |
//! | [`ControlState`] | 缺口 4：锚点布局解析 |
//! | [`NullRenderServer`] | headless 空实现 + 行为计数器 |

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]
#![deny(missing_docs)]

pub mod command;
pub mod handle;
pub mod input;
pub mod item;
pub mod math;
pub mod null;
pub mod server;
pub mod state;

pub use command::{FrameInfo, RenderCommand};
pub use handle::{ItemHandle, RenderAssetKey};
pub use item::{DrawKey, RenderItem};
pub use math::{Affine2, Rect, Vec2};
pub use null::{NullRenderServer, ServerCounters};
pub use server::RenderServer;
pub use state::{
    Camera2DState, ControlState, Flip, HAlign, LabelState, ListAxis, ListState, NineSliceState,
    ScrollBar, VAlign,
};
