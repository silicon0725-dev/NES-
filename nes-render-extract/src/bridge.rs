//! 场景层类型 → 契约层 wire 类型的**逐字段桥接**。
//!
//! 契约层是零依赖的，因此它不能认识 `nes_scene::Affine` 或 `nes_asset::AssetKey`；
//! 把两侧接起来的地方**只能**是提取层（本 crate）。三个桥都是纯搬字节，
//! 不做任何几何推导、不做任何语义加工：
//!
//! - [`affine2_of`]：`Affine → Affine2`（字段顺序一致，逐字段搬）；
//! - [`vec2_of`]：`nes_scene::Vec2 → Vec2`（同名不同型，逐字段搬）；
//! - [`render_key_of_bits`]：资源位编码 `u64 → RenderAssetKey`（编码规则一致，位拷贝）；
//! - [`flip_of`]：`Sprite2D::flip_h/flip_v → Flip`（已裁决：复用属性，不建旁路表）。
//!
//! 两侧的 `Vec2` 是**两个不同的类型**（各自 crate 的几何表示），因此 `anchor` /
//! `offset` 这类 `Vec2` 属性必须过 [`vec2_of`]，不能指望类型推断或 `From` 兜底。

use nes_render_api::{Affine2, Flip, RenderAssetKey, Vec2};
use nes_scene::{Affine, Vec2 as SceneVec2};

/// `nes_scene::Affine`（世界变换缓存）→ `Affine2`（契约层 wire 格式）。
///
/// 两侧字段顺序完全一致（`a, b, c, d, tx, ty`，列向量约定，见契约层 `math.rs`），
/// 所以这里是**逐字段搬运**而不是换算。任何"顺手归一化一下"的加工都会让推送出去的
/// 世界变换与场景层缓存不再逐位一致，"变换传播一致"这条出口准则就失去参照物。
pub fn affine2_of(affine: Affine) -> Affine2 {
    Affine2 {
        a: affine.a,
        b: affine.b,
        c: affine.c,
        d: affine.d,
        tx: affine.tx,
        ty: affine.ty,
    }
}

/// 资源位编码 → 契约层资源键。
///
/// `nes_asset::RenderAssetKeyView` 与契约层 `RenderAssetKey` 都只认一个 `u64`，
/// 且编码规则相同（高 32 位代际、低 32 位槽位），因此这里是纯位拷贝。
/// 之所以经"位"而不是直接搬类型，是因为契约层不允许依赖 `nes-asset`（零依赖）。
pub fn render_key_of_bits(bits: u64) -> RenderAssetKey {
    RenderAssetKey::from_bits(bits)
}

/// `Sprite2D` 的翻转属性 → 契约层 `Flip`。
///
/// 已裁决（S2）：flip 直接复用 `Sprite2D` 的 `flip_h` / `flip_v`，**不建旁路表**。
/// 翻转是绘制期子局部后乘（`world ∘ scale(±1, ±1)`），不进入节点的世界变换 ——
/// 所以提取层也不得把它折进 `set_transform` 的值里。
pub fn flip_of(flip_h: bool, flip_v: bool) -> Flip {
    Flip::new(flip_h, flip_v)
}

/// `nes_scene::Vec2`（场景几何缓存）→ `Vec2`（契约层 wire 格式）。
///
/// 两个类型同名同构（`x, y` 两个 `f32`），但没有共享定义 —— 契约层不能依赖
/// 场景层，场景层也不该依赖渲染契约。于是这个转换只能发生在提取层，且必须是
/// 逐字段搬运：`anchor` / `offset` / `size` / `viewport` 的语义都在数值里，
/// 任何"顺手取整"或"顺手归一"都会让推送出去的布局与用户所见不一致。
pub fn vec2_of(v: SceneVec2) -> Vec2 {
    Vec2::new(v.x, v.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affine_bridge_copies_every_field() {
        let src = Affine {
            a: 2.0,
            b: 0.5,
            c: -0.25,
            d: 3.0,
            tx: 11.0,
            ty: -7.5,
        };
        let dst = affine2_of(src);
        assert_eq!(dst.to_array(), [2.0, 0.5, -0.25, 3.0, 11.0, -7.5]);
    }

    #[test]
    fn affine_bridge_keeps_identity() {
        assert_eq!(affine2_of(Affine::IDENTITY), Affine2::IDENTITY);
    }

    #[test]
    fn render_key_bridge_is_bit_copy() {
        let bits = (3u64 << 32) | 7u64;
        let key = render_key_of_bits(bits);
        assert_eq!(key.to_bits(), bits);
        assert!(!key.is_nil());
        assert!(render_key_of_bits(0).is_nil());
    }

    #[test]
    fn flip_bridge_maps_props() {
        assert_eq!(flip_of(false, false), Flip::IDENTITY);
        assert_eq!(flip_of(true, false), Flip::new(true, false));
        assert_eq!(flip_of(false, true), Flip::new(false, true));
        assert_eq!(flip_of(true, true), Flip::new(true, true));
    }

    #[test]
    fn vec2_bridge_copies_every_axis() {
        let dst = vec2_of(SceneVec2::new(-1.5, 2.25));
        assert_eq!(dst, Vec2::new(-1.5, 2.25));
        // 负值与零都要原样过：`size` 允许为负（不钳制是已裁决语义）。
        assert_eq!(vec2_of(SceneVec2::ZERO), Vec2::ZERO);
        assert_eq!(vec2_of(SceneVec2::new(-100.0, 0.0)), Vec2::new(-100.0, 0.0));
    }
}
