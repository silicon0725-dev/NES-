//! 契约层的几何原语：`Vec2` / `Affine2` / `Rect`。
//!
//! # 为什么契约层自带一份数学类型
//!
//! 本 crate 的硬约束是**零依赖**，因此不能复用 `nes_scene::transform::{Vec2, Affine}`。
//! 做法是把这三个类型定义为契约层的 **wire 格式**：
//!
//! - 字段顺序与 `nes_scene::transform` 完全一致（`Vec2 { x, y }`、
//!   `Affine2 { a, b, c, d, tx, ty }`），转换在提取层（S2）是逐字段搬运；
//! - 均标注 `#[repr(C)]`，将来经 C ABI 递给 wgpu-native 一类后端时布局可预期
//!   （本 crate 依然 `forbid(unsafe_code)`，不做任何指针转换）。
//!
//! 这份"重复"是刻意的：契约层一旦依赖场景层，后端就被拖进场景层，
//! 方案 D 的单向依赖立刻作废（见 `Cargo.toml` 的依赖纪律注释）。

/// 二维向量（契约层 wire 格式）。
#[derive(Copy, Clone, PartialEq, Debug, Default)]
#[repr(C)]
pub struct Vec2 {
    /// X 分量。
    pub x: f32,
    /// Y 分量。
    pub y: f32,
}

impl Vec2 {
    /// 零向量。
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
    /// 单位向量。
    pub const ONE: Self = Self { x: 1.0, y: 1.0 };

    /// 构造。
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// 展平为数组（便于 FFI / 日志 / 序列化桥）。
    pub const fn to_array(self) -> [f32; 2] {
        [self.x, self.y]
    }

    /// 由数组还原。
    pub const fn from_array(a: [f32; 2]) -> Self {
        Self { x: a[0], y: a[1] }
    }

    /// 两个分量是否都是有限值（后端入队前自检用）。
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// 欧氏长度。
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

impl From<[f32; 2]> for Vec2 {
    fn from(a: [f32; 2]) -> Self {
        Self::from_array(a)
    }
}

/// 2x3 仿射矩阵（列向量约定）：
///
/// ```text
/// | a  c  tx |
/// | b  d  ty |
/// | 0  0   1 |
/// ```
#[derive(Copy, Clone, PartialEq, Debug)]
#[repr(C)]
pub struct Affine2 {
    /// 列 0 的 x 分量。
    pub a: f32,
    /// 列 0 的 y 分量。
    pub b: f32,
    /// 列 1 的 x 分量。
    pub c: f32,
    /// 列 1 的 y 分量。
    pub d: f32,
    /// 平移 x。
    pub tx: f32,
    /// 平移 y。
    pub ty: f32,
}

impl Affine2 {
    /// 单位矩阵。
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// 纯平移。
    pub const fn translation(x: f32, y: f32) -> Self {
        Self {
            tx: x,
            ty: y,
            ..Self::IDENTITY
        }
    }

    /// 纯缩放（可为负，用于 flip 合成）。
    pub const fn scale(x: f32, y: f32) -> Self {
        Self {
            a: x,
            d: y,
            ..Self::IDENTITY
        }
    }

    /// 纯旋转（弧度，逆时针为正）。
    pub fn rotation(radians: f32) -> Self {
        let (sin_r, cos_r) = radians.sin_cos();
        Self {
            a: cos_r,
            b: sin_r,
            c: -sin_r,
            d: cos_r,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// 先应用 `rhs`，再应用 `self`（即 `self ∘ rhs`）。
    ///
    /// 为什么不实现 `core::ops::Mul`：`Mul::mul` 的 receiver 是 by-value，
    /// 会与这里的 `&self` 固有方法在方法查找中打架，最终表现为所有
    /// `a.mul(&b)` 调用点集体编译失败且错误信息指向 `core::ops`。
    /// 该坑在 `nes-scene` 的 `transform.rs` 已记录，本契约层沿用同一结论：
    /// 复合只留 [`Affine2::mul`] 一个入口（显式取引用）。
    pub fn mul(&self, rhs: &Affine2) -> Affine2 {
        Affine2 {
            a: self.a * rhs.a + self.c * rhs.b,
            b: self.b * rhs.a + self.d * rhs.b,
            c: self.a * rhs.c + self.c * rhs.d,
            d: self.b * rhs.c + self.d * rhs.d,
            tx: self.a * rhs.tx + self.c * rhs.ty + self.tx,
            ty: self.b * rhs.tx + self.d * rhs.ty + self.ty,
        }
    }

    /// 对点做变换。
    pub fn apply(&self, p: Vec2) -> Vec2 {
        Vec2 {
            x: self.a * p.x + self.c * p.y + self.tx,
            y: self.b * p.x + self.d * p.y + self.ty,
        }
    }

    /// 行列式。为零表示退化（不可逆）。
    pub fn determinant(&self) -> f32 {
        self.a * self.d - self.b * self.c
    }

    /// 逆矩阵。退化时返回 `None`（调用方不得假定可逆）。
    pub fn inverse(&self) -> Option<Affine2> {
        let det = self.determinant();
        if det.abs() <= f32::EPSILON {
            return None;
        }
        let inv = 1.0 / det;
        Some(Affine2 {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            tx: (self.c * self.ty - self.d * self.tx) * inv,
            ty: (self.b * self.tx - self.a * self.ty) * inv,
        })
    }

    /// 只保留线性（旋转/缩放/斜切）部分，丢掉平移。
    pub const fn linear_of(&self) -> Self {
        Self {
            a: self.a,
            b: self.b,
            c: self.c,
            d: self.d,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// 旋转分量（弧度）。取 `atan2(b, a)`，与节点缩放无关。
    pub fn rotation_of(&self) -> f32 {
        self.b.atan2(self.a)
    }

    /// 六个分量是否都是有限值（后端入队前自检用）。
    pub fn is_finite(&self) -> bool {
        self.a.is_finite()
            && self.b.is_finite()
            && self.c.is_finite()
            && self.d.is_finite()
            && self.tx.is_finite()
            && self.ty.is_finite()
    }

    /// 展平为 `[a, b, c, d, tx, ty]`（与 `nes_scene::Affine` 字段顺序一致）。
    pub const fn to_array(self) -> [f32; 6] {
        [self.a, self.b, self.c, self.d, self.tx, self.ty]
    }

    /// 由 `[a, b, c, d, tx, ty]` 还原。
    pub const fn from_array(v: [f32; 6]) -> Self {
        Self {
            a: v[0],
            b: v[1],
            c: v[2],
            d: v[3],
            tx: v[4],
            ty: v[5],
        }
    }
}

impl Default for Affine2 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl From<[f32; 6]> for Affine2 {
    fn from(v: [f32; 6]) -> Self {
        Self::from_array(v)
    }
}

/// 轴对齐矩形（左上角 + 尺寸）。
#[derive(Copy, Clone, PartialEq, Debug, Default)]
#[repr(C)]
pub struct Rect {
    /// 左上角 x。
    pub x: f32,
    /// 左上角 y。
    pub y: f32,
    /// 宽度（可为负，语义由调用方决定；契约层不做钳制）。
    pub w: f32,
    /// 高度（可为负，语义由调用方决定；契约层不做钳制）。
    pub h: f32,
}

impl Rect {
    /// 构造。
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// 由最小/最大角构造。
    pub fn from_min_max(min: Vec2, max: Vec2) -> Self {
        Self::new(min.x, min.y, max.x - min.x, max.y - min.y)
    }

    /// 左上角。
    pub fn min(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    /// 右下角。
    pub fn max(self) -> Vec2 {
        Vec2::new(self.x + self.w, self.y + self.h)
    }

    /// 中心点。
    pub fn center(self) -> Vec2 {
        Vec2::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    /// 尺寸。
    pub fn size(self) -> Vec2 {
        Vec2::new(self.w, self.h)
    }

    /// 点是否落在矩形内（闭区间：四条边都算在内）。
    pub fn contains(self, p: Vec2) -> bool {
        let max = self.max();
        p.x >= self.x && p.x <= max.x && p.y >= self.y && p.y <= max.y
    }

    /// 展平为 `[x, y, w, h]`。
    pub const fn to_array(self) -> [f32; 4] {
        [self.x, self.y, self.w, self.h]
    }

    /// 由 `[x, y, w, h]` 还原。
    pub const fn from_array(v: [f32; 4]) -> Self {
        Self {
            x: v[0],
            y: v[1],
            w: v[2],
            h: v[3],
        }
    }
}

impl From<[f32; 4]> for Rect {
    fn from(v: [f32; 4]) -> Self {
        Self::from_array(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn identity_is_neutral() {
        let t = Affine2::translation(3.0, -4.0);
        assert_eq!(Affine2::IDENTITY.mul(&t), t);
        assert_eq!(t.mul(&Affine2::IDENTITY), t);
    }

    #[test]
    fn composition_order_is_self_after_rhs() {
        let parent = Affine2::scale(2.0, 3.0);
        let child = Affine2::translation(1.0, 1.0);
        let world = parent.mul(&child);
        assert!(approx(world.tx, 2.0) && approx(world.ty, 3.0));
    }

    #[test]
    fn inverse_roundtrips_point() {
        let t = Affine2::rotation(0.7).mul(&Affine2::scale(1.5, 0.5));
        let p = Vec2::new(3.0, 4.0);
        let back = t.inverse().expect("non-degenerate").apply(t.apply(p));
        assert!(approx(back.x, p.x) && approx(back.y, p.y));
    }

    #[test]
    fn degenerate_matrix_has_no_inverse() {
        assert!(Affine2::scale(0.0, 1.0).inverse().is_none());
    }

    #[test]
    fn rotation_of_ignores_scale() {
        let t = Affine2::scale(4.0, 9.0).mul(&Affine2::rotation(std::f32::consts::FRAC_PI_2));
        assert!(approx(t.rotation_of(), std::f32::consts::FRAC_PI_2));
    }

    #[test]
    fn array_roundtrip_keeps_layout_order() {
        let t = Affine2::translation(5.0, 6.0).mul(&Affine2::scale(2.0, 3.0));
        assert_eq!(Affine2::from_array(t.to_array()), t);
        assert_eq!(t.to_array(), [2.0, 0.0, 0.0, 3.0, 5.0, 6.0]);
    }

    #[test]
    fn rect_contains_is_inclusive_on_all_edges() {
        let r = Rect::new(0.0, 0.0, 10.0, 5.0);
        assert!(r.contains(Vec2::new(0.0, 0.0)));
        assert!(r.contains(Vec2::new(10.0, 5.0)));
        assert!(!r.contains(Vec2::new(10.1, 5.0)));
        assert_eq!(r.center(), Vec2::new(5.0, 2.5));
    }
}
