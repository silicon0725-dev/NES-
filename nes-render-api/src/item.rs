//! [`RenderItem`]：一个渲染物的**属性集合**（通用部分）。

use crate::handle::{ItemHandle, RenderAssetKey};
use crate::math::Affine2;
use crate::state::Flip;

/// 绘制次序键 —— 渲染侧排序的**唯一许可入口**。
///
/// 顺序为 `z` 升序 → `order` 升序 → `handle` 升序，全序、无并列。
/// 这条纪律源自 NES 2.0 的确定性要求：任何用哈希表迭代顺序、
/// 也不允许用"插入先后"决定绘制次序 —— 同一棵树在两台机器上必须出同样的帧。
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct DrawKey {
    /// 层号（越小越先画）。
    pub z: i32,
    /// 同层内的稳定次序（提取层填 `nes-scene` 的兄弟 `order`）。
    pub order: u64,
    /// 句柄原始位（仅作全序兜底，不承载语义）。
    pub handle: u64,
}

/// 渲染物的通用属性集合。
///
/// # 为什么是 `Copy`
///
/// 它只含句柄、键、可见性、层号、变换与 flip，全部是可平凡复制的值，
/// 且**故意不含** Label / Control 的专用状态。这样：
///
/// - 提取层可以把它整块放进预分配缓冲，不产生每帧堆分配；
/// - 后端可以把它当作"待绘条目"直接排序、批量上传；
/// - 专用状态（文本、控件布局、相机）走
///   [`RenderServer::set_text`](crate::RenderServer::set_text) /
///   [`set_rect`](crate::RenderServer::set_rect) /
///   [`set_camera`](crate::RenderServer::set_camera)，各自独立演进。
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct RenderItem {
    /// 后端侧易变句柄。
    pub handle: ItemHandle,
    /// 稳定资源身份（纹理）。
    pub key: RenderAssetKey,
    /// 可见性。`false` 时后端**仍保留**该渲染物与其属性（只跳过绘制），
    /// 这与"销毁"是两件事。
    pub visible: bool,
    /// 层号。
    pub z: i32,
    /// 同层内的稳定次序。
    pub order: u64,
    /// 世界变换（已含父链复合，来自 `nes-scene` 的 `world` 缓存）。
    pub transform: Affine2,
    /// 翻转。**不参与世界变换缓存**，是渲染期附加的子局部后乘，
    /// 见 [`RenderItem::world_transform`]。
    pub flip: Flip,
}

impl RenderItem {
    /// 构造：新建的渲染物默认可见、层号 0、次序 0、单位变换、不翻转。
    pub fn new(handle: ItemHandle, key: RenderAssetKey, transform: Affine2) -> Self {
        Self {
            handle,
            key,
            visible: true,
            z: 0,
            order: 0,
            transform,
            flip: Flip::IDENTITY,
        }
    }

    /// 绘制次序键。
    pub fn draw_key(&self) -> DrawKey {
        DrawKey {
            z: self.z,
            order: self.order,
            handle: self.handle.raw(),
        }
    }

    /// 后端实际用于绘制的矩阵：`transform ∘ flip`。
    ///
    /// flip 必须是**后乘**（子局部缩放），这样翻转绕渲染物自身原点发生，
    /// 不会把节点的世界位置搬走 —— 这正是 `nes-scene` 选择往
    /// `Affine` 上加一层"渲染期翻转"而不是改节点变换的原因。
    pub fn world_transform(&self) -> Affine2 {
        self.flip.compose(self.transform)
    }
}

impl Default for RenderItem {
    fn default() -> Self {
        Self::new(ItemHandle::NIL, RenderAssetKey::NIL, Affine2::IDENTITY)
    }
}
