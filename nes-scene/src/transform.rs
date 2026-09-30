//! 2D 变换与仿射缓存。
//!
//! 两种表示的职责划分（见 crate 根注释的修订说明）：
//!
//! - [`Transform2D`]：人类可读的本地变换（pos / rot / scale / skew）。
//!   面向编辑器属性面板与脚本。**只用于 local。**
//! - [`Affine`]：精确的 2x3 仿射矩阵。面向父子复合、世界变换缓存、渲染与命中测试。
//!   复合是纯矩阵乘法，无分解歧义、无精度损失。


/// 二维向量。
#[derive(Copy, Clone, PartialEq, Debug, Default)]
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
}

/// 本地变换（编辑器/脚本视角）。
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Transform2D {
    /// 平移。
    pub pos: Vec2,
    /// 旋转（弧度）。
    pub rot: f32,
    /// 缩放。
    pub scale: Vec2,
    /// 斜切（弧度）。
    pub skew: f32,
}

impl Transform2D {
    /// 单位变换。缓存的初值。
    pub const IDENTITY: Self = Self {
        pos: Vec2::ZERO,
        rot: 0.0,
        scale: Vec2::ONE,
        skew: 0.0,
    };

    /// 仅平移。
    pub const fn from_pos(x: f32, y: f32) -> Self {
        Self {
            pos: Vec2::new(x, y),
            rot: 0.0,
            scale: Vec2::ONE,
            skew: 0.0,
        }
    }

    /// 仅旋转。
    pub const fn from_rot(radians: f32) -> Self {
        Self {
            pos: Vec2::ZERO,
            rot: radians,
            scale: Vec2::ONE,
            skew: 0.0,
        }
    }

    /// 仅缩放。
    pub const fn from_scale(x: f32, y: f32) -> Self {
        Self {
            pos: Vec2::ZERO,
            rot: 0.0,
            scale: Vec2::new(x, y),
            skew: 0.0,
        }
    }

    /// 转为精确仿射表示。世界变换计算一律走这里。
    pub fn to_affine(self) -> Affine {
        let (sin_r, cos_r) = self.rot.sin_cos();
        let (sin_k, cos_k) = (self.rot + self.skew).sin_cos();
        // 列向量约定：x 轴 = scale.x * (cos r, sin r)
        //               y 轴 = scale.y * (-sin(r+skew), cos(r+skew))
        Affine {
            a: self.scale.x * cos_r,
            b: self.scale.x * sin_r,
            c: -self.scale.y * sin_k,
            d: self.scale.y * cos_k,
            tx: self.pos.x,
            ty: self.pos.y,
        }
    }

    /// 对点做变换（单节点语义，不涉及父子）。
    pub fn transform_point(self, p: Vec2) -> Vec2 {
        self.to_affine().apply(p)
    }
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// 2x3 仿射矩阵：
/// ```text
/// | a  c  tx |
/// | b  d  ty |
/// | 0  0   1 |
/// ```
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Affine {
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

impl Affine {
    /// 单位矩阵。
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// 平移矩阵。
    pub const fn translation(x: f32, y: f32) -> Self {
        Self {
            tx: x,
            ty: y,
            ..Self::IDENTITY
        }
    }

    /// 先应用 `rhs`，再应用 `self`（即 `self ∘ rhs`）。
    pub fn mul(&self, rhs: &Affine) -> Affine {
        Affine {
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
    pub fn inverse(&self) -> Option<Affine> {
        let det = self.determinant();
        if det.abs() <= f32::EPSILON {
            return None;
        }
        let inv = 1.0 / det;
        Some(Affine {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            tx: (self.c * self.ty - self.d * self.tx) * inv,
            ty: (self.b * self.tx - self.a * self.ty) * inv,
        })
    }
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

// 为什么**不**实现 `Mul` 运算符重载（实现期踩坑，记录以免后人重犯）：
//
// `impl Mul for Affine` 会定义 `Mul::mul(self: Affine, rhs: Affine)`，其 receiver 是
// **by-value** 的 `Affine`。而 [`Affine::mul`] 固有方法的 receiver 是 `&Affine`。
// Rust 的方法查找沿"receiver 步进序列"逐级尝试，并在**每一步内先看 by-value 候选**：
// 于是 `a.mul(&b)` 会在第一步就命中 trait 的 by-value 候选，随后再报"参数类型不匹配"，
// 而不是回退到固有方法。结果就是所有 `a.mul(&b)` 调用点集体编译失败，且错误信息
// 指向 `core::ops::Mul` 而非本文件，排查成本很高。
//
// 结论：矩阵复合保留为唯一入口 [`Affine::mul`]（显式取引用），不引入运算符重载。

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn identity_is_neutral() {
        let t = Transform2D::from_pos(3.0, -4.0).to_affine();
        let out = Affine::IDENTITY.mul(&t);
        assert!(approx(out.tx, 3.0) && approx(out.ty, -4.0));
    }

    #[test]
    fn translation_composes_additively() {
        let parent = Transform2D::from_pos(10.0, 0.0).to_affine();
        let child = Transform2D::from_pos(0.0, 5.0).to_affine();
        let world = parent.mul(&child);
        assert!(approx(world.tx, 10.0));
        assert!(approx(world.ty, 5.0));
    }

    #[test]
    fn nonuniform_parent_scale_is_exact() {
        // 草案修订的动因：父节点非均匀缩放时，矩阵复合必须精确，
        // 走 (pos,rot,scale,skew) 分解会漂。
        let parent = Transform2D::from_scale(2.0, 3.0).to_affine();
        let child = Transform2D::from_pos(1.0, 1.0).to_affine();
        let world = parent.mul(&child);
        assert!(approx(world.tx, 2.0));
        assert!(approx(world.ty, 3.0));
    }

    #[test]
    fn rotation_of_ninety_degrees_maps_axes() {
        let r = Transform2D::from_rot(std::f32::consts::FRAC_PI_2).to_affine();
        let p = r.apply(Vec2::new(1.0, 0.0));
        assert!(approx(p.x, 0.0));
        assert!(approx(p.y, 1.0));
    }

    #[test]
    fn inverse_roundtrips_point() {
        let t = Transform2D {
            pos: Vec2::new(5.0, -2.0),
            rot: 0.7,
            scale: Vec2::new(1.5, 0.5),
            skew: 0.1,
        }
        .to_affine();
        let p = Vec2::new(3.0, 4.0);
        let back = t.inverse().expect("non-degenerate").apply(t.apply(p));
        assert!(approx(back.x, p.x) && approx(back.y, p.y));
    }

    #[test]
    fn degenerate_matrix_has_no_inverse() {
        let t = Transform2D::from_scale(0.0, 1.0).to_affine();
        assert!(t.inverse().is_none());
    }
}
