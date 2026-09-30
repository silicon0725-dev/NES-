//! 渲染侧的两种身份：易变的 [`ItemHandle`] 与稳定的 [`RenderAssetKey`]。
//!
//! 这套「稳定身份 / 易变句柄分离」纪律与 `nes-scene` 的 `NodeId`、`nes-asset`
//! 的 `AssetKey` 完全同构，是 NES 2.0 内核自研部分的核心资产之一：
//! **稳定身份可以进场景文件、可以跨会话 diff；易变句柄只能活在进程内。**

use core::fmt;

/// 渲染物句柄 —— 后端侧的**易变**句柄（等价 Godot 的 RID / Bevy 的 RenderEntity）。
///
/// # 纪律
///
/// - **不得序列化**、不得写进场景文件、不得跨帧假设仍然有效；
/// - `0` 保留为「空句柄」（[`ItemHandle::NIL`]）；任何以空句柄发起的操作
///   都必须被服务端**静默忽略**（不 panic、不中断本帧提交）；
/// - 句柄由服务端 [`crate::RenderServer::create_item`] 分配；销毁后其值
///   **不得**重新分配给别的渲染物，否则悬垂引用会命中"恰好复用同一槽位的
///   另一个物体"，这正是「永不复用」纪律要消灭的错误类；
/// - 低 32 位是槽位、高 32 位是代际（`(slot, gen)`）。契约层只冻结这个编码，
///   分配策略留给后端；[`NullRenderServer`](crate::NullRenderServer) 用单调计数器，
///   真实后端可换成 arena + 代际。
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemHandle(u64);

impl ItemHandle {
    /// 空句柄。所有以它发起的操作都必须被忽略。
    pub const NIL: Self = Self(0);

    /// 由原始位构造。
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// 原始位。
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// 由 `(slot, gen)` 构造。
    pub const fn from_parts(slot: u32, gen: u32) -> Self {
        Self(((gen as u64) << 32) | (slot as u64))
    }

    /// 槽位。
    pub const fn slot(self) -> u32 {
        (self.0 & 0xFFFF_FFFF) as u32
    }

    /// 代际号。
    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// 是否为空句柄。
    pub const fn is_nil(self) -> bool {
        self.0 == 0
    }
}

impl Default for ItemHandle {
    fn default() -> Self {
        Self::NIL
    }
}

impl fmt::Debug for ItemHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_nil() {
            return f.write_str("Item#nil");
        }
        write!(f, "Item#{}v{}", self.slot(), self.generation())
    }
}

/// 稳定资源身份在渲染侧的投影（`gen << 32 | slot`，`0` 表示未绑定）。
///
/// # 与 M3 的位编码统一
///
/// 它与 `nes_asset::RenderAssetKeyView`（M3 建立的镜像视图）以及
/// `twn-render-resources::RenderAssetKey` **位编码完全一致**：三处都只认一个
/// `u64`，都遵守「稳定身份 vs 易变后端句柄」的双身份纪律。因此契约层不需要
/// 依赖任何一方，转换由提取层（S2）在 `AssetKey → RenderAssetKey` 时完成，
/// 是纯位传递（`to_bits` / `from_bits` 互逆）。
///
/// `slot == 0` 视为**未绑定**（与 `nes-scene` 的 `Value::Resource(0)`、
/// `nes_asset::AssetKey::NIL` 同义）。渲染侧拿到未绑定键时的行为由后端决定，
/// 但**不得 panic**：契约层的立场是"未绑定不渲染"，不是"未绑定即错误"。
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RenderAssetKey(u64);

impl RenderAssetKey {
    /// 未绑定键（位全零）。
    pub const NIL: Self = Self(0);

    /// 由位编码构造。
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// 由 `(slot, gen)` 构造。
    pub const fn from_parts(slot: u32, gen: u32) -> Self {
        Self(((gen as u64) << 32) | (slot as u64))
    }

    /// 位编码（可直接与 `nes_asset::RenderAssetKeyView::to_bits` 比对）。
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    /// 槽位。
    pub const fn slot(self) -> u32 {
        (self.0 & 0xFFFF_FFFF) as u32
    }

    /// 代际号。
    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// 是否未绑定。
    pub const fn is_nil(self) -> bool {
        self.0 == 0
    }
}

impl Default for RenderAssetKey {
    fn default() -> Self {
        Self::NIL
    }
}

impl fmt::Debug for RenderAssetKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_nil() {
            return f.write_str("Asset#nil");
        }
        write!(f, "Asset#{}v{}", self.slot(), self.generation())
    }
}
