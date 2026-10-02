//! NES 2.0 · M4 / S2 —— 渲染**提取层**（方案 D「渲染服务端化 + 每帧提取」的落点）。
//!
//! # 这一层在做什么
//!
//! `nes-scene` 说"树里有什么节点"，`nes-render-api` 说"渲染后端能被喂什么"。
//! 两者的词汇表不同：前者是 `NodeId` + 节点类型 + 属性表，后者是 `ItemHandle` +
//! 资源键 + 世界变换。**把前者翻译成后者的地方只有这一处**，这就是提取层。
//!
//! 每帧一次（[`RenderExtractor::extract_into`]）：
//!
//! 1. 冲洗世界变换，取确定性**前序**遍历序（用预分配缓冲实现，语义等同场景层的
//!    `preorder`）；
//! 2. 逐节点解析"渲染物身份"：`Sprite2D` 解析出非空纹理键 / `Label` 文本非空 /
//!    `Control`（恒准入）→ 可渲染；`Camera2D` 走独立通道（`set_camera`，不建渲染物）；
//! 3. 生命周期：命中既有条目则**复用**句柄；资源换代则 `destroy_item` + `create_item`；
//!    新节点则 `create_item`；本帧不再可渲染的条目则 `destroy_item`；
//! 4. 属性级推送：`set_transform` / `set_flip` / `set_z` / `set_visible`（全量快照），
//!    `Label` 追加 `set_text`、`Control` 追加 `set_rect`；
//! 5. 清扫：本帧未被遍历到的条目（节点被删 / 整棵子树被摘）统一 `destroy_item`；
//! 6. `submit_into` 输出本帧命令流 —— 输出缓冲与遍历缓冲跨帧复用，走热路径不分配。
//!
//! # 依赖方向（不可逆）
//!
//! ```text
//! nes-asset ──▶ nes-scene ──▶ nes-render-extract ──▶ nes-render-api
//! ```
//!
//! - 场景层与资源层**永远不知道**渲染的存在（本 crate 是它们下游的下游）；
//! - 契约层保持零依赖，提取层与后端在它的两侧互不依赖；
//! - 本 crate 除上述两条 path 依赖外不引入任何第三方 crate。
//!
//! # 与 S1 契约的关系
//!
//! 契约层**一个签名都没动**（S1 冻结面）：提取层只做三件契约之外的事 ——
//! 类型桥接（[`bridge`]）、身份映射（[`map`]）、资源键解析（[`source`]）。
//! 不变式 I1~I10 的落点：空句柄静默忽略由契约层保证（本层从不产生空句柄）；
//! "句柄永不复用"由 `ItemHandle` 与 [`NodeItemMap`] 双重保证；
//! "submit 先清空 out"由契约层保证（本层不碰 `out` 的旧内容）。
//!
//! # 已裁决事项的落点
//!
//! - **flip**：直接读 `Sprite2D` 的 `flip_h` / `flip_v` 属性，不建旁路表
//!   （[`PROP_FLIP_H`] / [`PROP_FLIP_V`]），推给契约层的 `set_flip`；
//! - **set_z**：`z` 取 `Node2D` 的 `z_index` 属性，`order` 取确定性前序遍历的
//!   序号（即场景层 `NodeData::order`），不为场景层加字段。
//!
//! # S3 四项渲染缺口（本层补全）
//!
//! | 缺口 | 准入 | 出口 |
//! |---|---|---|
//! | 相机视图矩阵 | 遍历到 `Camera2D` | [`camera_state_of`] → `set_camera`（单槽 last-write-wins） |
//! | Label 文本布局 | `Label` 且文本非空 | [`label_state_of`] → `set_text`（空文本不上屏） |
//! | Control 锚点布局 | `Control` 恒准入 | [`control_state_of`] → `set_rect`（负宽高不钳制） |
//! | flip 合成 | `flip_h` / `flip_v` 属性 | [`compose_flip`]（`world ∘ scale(±1,±1)`，flip 后乘） |
//!
//! 四项都**没有改动契约签名**：S1 冻结的 `set_camera` / `set_text` / `set_rect` /
//! `Flip::compose` 在 S2 期间只有 null 后端在调，S3 起多了提取层这一侧的生产调用方。
//! 相机不建渲染物（不参与生命周期），Label / Control 与精灵共用同一条生命周期与
//! z 序通道，只是身份键与追加推送项不同。

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rust_2018_idioms)]

pub mod bridge;
pub mod extractor;
pub mod map;
pub mod source;

pub use bridge::{affine2_of, flip_of, render_key_of_bits, vec2_of};
pub use extractor::{
    camera_state_of, compose_flip, control_state_of, label_state_of, themed_control,
    themed_label, ExtractStats, RenderExtractor,
    ScratchStats, DEFAULT_CONTROL_SIZE, DEFAULT_LABEL_FONT_SIZE, PROP_CAMERA_ACTIVE,
    PROP_CAMERA_ZOOM, PROP_CONTROL_ANCHOR, PROP_CONTROL_OFFSET, PROP_CONTROL_SIZE, PROP_FLIP_H,
    PROP_FLIP_V, PROP_LABEL_FONT_SIZE, PROP_LABEL_TEXT, PROP_TEXTURE, PROP_VISIBLE, PROP_Z_INDEX,
};
pub use map::{ItemSlot, NodeItemMap};
pub use source::RenderKeySource;
