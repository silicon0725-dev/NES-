//! 四项缺口契约：flip、Camera2D、Label、Control。
//!
//! 调研报告把 Camera2D / Label / Control / flip 认定为 A 方案 fork 里的"缺口"。
//! S1 的立场是：这四项**属于契约层**，不是某个 stage 的内部细节 ——
//! 只有落在契约里，它们才能被单测覆盖、被后端独立实现、被 M5 兼容层复用，
//! 而不是散落在 stage 内部随 fork 漂移。

use std::sync::Arc;

use crate::handle::RenderAssetKey;
use crate::math::{Affine2, Rect, Vec2};

// ---------------------------------------------------------------- flip

/// 水平/垂直翻转。
///
/// # 与变换的关系（契约冻结点）
///
/// flip **不改变**节点的世界变换，只在绘制时作**子局部后乘**：
/// `world ∘ scale(±1, ±1)`。因此翻转绕渲染物自身原点发生，
/// 平移分量逐位不变（见 [`Flip::compose`]）。
/// 把 flip 折进节点变换是错的：那会污染 `nes-scene` 的世界变换缓存，
/// 也让"改 flip 不改位置"这条不变式失去可验证性。
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Flip {
    /// 水平翻转（沿 Y 轴镜像）。
    pub h: bool,
    /// 垂直翻转（沿 X 轴镜像）。
    pub v: bool,
}

impl Flip {
    /// 不翻转。
    pub const IDENTITY: Self = Self { h: false, v: false };

    /// 构造。
    pub const fn new(h: bool, v: bool) -> Self {
        Self { h, v }
    }

    /// 是否不翻转。
    pub const fn is_identity(self) -> bool {
        !self.h && !self.v
    }

    /// 是否有任一方向翻转。
    pub const fn any(self) -> bool {
        self.h || self.v
    }

    /// 转成缩放矩阵（翻转方向为 -1）。
    pub const fn to_affine(self) -> Affine2 {
        Affine2::scale(
            if self.h { -1.0 } else { 1.0 },
            if self.v { -1.0 } else { 1.0 },
        )
    }

    /// 把翻转合成到变换上：`transform ∘ flip`。
    pub fn compose(self, transform: Affine2) -> Affine2 {
        transform.mul(&self.to_affine())
    }
}

// ---------------------------------------------------------------- camera

/// 2D 相机的完整状态（缺口契约之一）。
///
/// 契约只负责"**世界坐标 → 视图坐标**"这一个转换，[`Camera2DState::view_matrix`]
/// 是它的唯一权威出口。后端不得另行推导相机矩阵，否则 S3 与
/// `twn-render-stage` 的逐帧比对会失去参照。
///
/// 语义约定（冻结）：
/// - `viewport` 单位是**设备像素**（与 `FrameInfo::viewport` 同义），缩放由
///   `zoom` 承担，DPI 由 `FrameInfo::dpi_scale` 承担；
/// - 相机节点的缩放/斜切**不参与**视图矩阵（缩放权威在 `zoom`），只取旋转分量；
/// - `offset` 是**相机局部**偏移，先按相机旋转转到世界方向，再加到相机位置上；
/// - `limits` 按**世界轴对齐 AABB** 夹紧相机中心（旋转下的近似，见开放问题）。
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Camera2DState {
    /// 相机节点的世界变换（来自 `nes-scene` 的 `world` 缓存）。
    pub transform: Affine2,
    /// 相机局部偏移（跟随相机旋转）。
    pub offset: Vec2,
    /// 缩放，必须为正；`<= 0` 视为 1。
    pub zoom: Vec2,
    /// 视口尺寸（设备像素）。
    pub viewport: Vec2,
    /// 世界坐标可视范围限制（`None` = 不限制）。
    pub limits: Option<Rect>,
    /// 是否启用。`false` 时 [`Camera2DState::view_matrix`] 返回 `None`，
    /// 后端应退回上一有效相机或不做变换。
    pub enabled: bool,
}

impl Camera2DState {
    /// 以给定视口构造一台单位相机（位于原点、无缩放、无偏移、无限制、已启用）。
    pub fn new(viewport: Vec2) -> Self {
        Self {
            transform: Affine2::IDENTITY,
            offset: Vec2::ZERO,
            zoom: Vec2::ONE,
            viewport,
            limits: None,
            enabled: true,
        }
    }

    /// 归一化后的缩放（非正值退化为 1，避免"零缩放把整场景压成一点"）。
    pub fn effective_zoom(self) -> Vec2 {
        Vec2::new(
            if self.zoom.x > 0.0 { self.zoom.x } else { 1.0 },
            if self.zoom.y > 0.0 { self.zoom.y } else { 1.0 },
        )
    }

    /// 相机朝向（弧度），取变换的旋转分量。
    pub fn rotation(self) -> f32 {
        self.transform.rotation_of()
    }

    /// 相机注视的世界点（未夹紧）：`transform 平移 + 旋转后的 offset`。
    pub fn center(self) -> Vec2 {
        let rotated = Affine2::rotation(self.rotation()).apply(self.offset);
        Vec2::new(self.transform.tx + rotated.x, self.transform.ty + rotated.y)
    }

    /// 可视区在**世界单位**下的轴对齐半尺寸。
    ///
    /// 分两步：先在相机轴上得到 `viewport / 2 / zoom`，再按旋转把该矩形
    /// 投影到世界轴（取绝对值求和），得到包住整块可视区的 AABB。
    pub fn visible_half_extents(self) -> Vec2 {
        let zoom = self.effective_zoom();
        let half_cam = Vec2::new(
            self.viewport.x * 0.5 / zoom.x,
            self.viewport.y * 0.5 / zoom.y,
        );
        let rot = self.rotation();
        let (sin_r, cos_r) = rot.sin_cos();
        Vec2::new(
            cos_r.abs() * half_cam.x + sin_r.abs() * half_cam.y,
            sin_r.abs() * half_cam.x + cos_r.abs() * half_cam.y,
        )
    }

    /// 夹紧后的相机注视点：保证可视 AABB 落在 `limits` 内；
    /// 若某个轴上 `limits` 比可视区还窄，则该轴取 `limits` 中心（不抖动、不缩放）。
    pub fn clamped_center(self) -> Vec2 {
        let center = self.center();
        let Some(limits) = self.limits else {
            return center;
        };
        let half = self.visible_half_extents();
        let min = limits.min();
        let max = limits.max();
        let cx = if limits.w >= half.x * 2.0 {
            center.x.clamp(min.x + half.x, max.x - half.x)
        } else {
            limits.center().x
        };
        let cy = if limits.h >= half.y * 2.0 {
            center.y.clamp(min.y + half.y, max.y - half.y)
        } else {
            limits.center().y
        };
        Vec2::new(cx, cy)
    }

    /// 世界坐标下的可视矩形（夹紧后）。
    pub fn visible_world_rect(self) -> Rect {
        let center = self.clamped_center();
        let half = self.visible_half_extents();
        Rect::new(
            center.x - half.x,
            center.y - half.y,
            half.x * 2.0,
            half.y * 2.0,
        )
    }

    /// **世界坐标 → 视图坐标** 矩阵（本契约的核心算式，冻结）。
    ///
    /// ```text
    /// view = T(viewport/2) ∘ S(zoom) ∘ T(-clamped_center) ∘ R(-rotation)
    /// ```
    ///
    /// 逐条含义：先抵消相机旋转 → 把相机注视点搬到原点 → 施加缩放 →
    /// 把原点搬到视口中心。`enabled == false` 时返回 `None`。
    pub fn view_matrix(self) -> Option<Affine2> {
        if !self.enabled {
            return None;
        }
        let center = self.clamped_center();
        let zoom = self.effective_zoom();
        let to_view_center = Affine2::translation(self.viewport.x * 0.5, self.viewport.y * 0.5);
        let scale = Affine2::scale(zoom.x, zoom.y);
        let to_origin = Affine2::translation(-center.x, -center.y);
        let undo_rotation = Affine2::rotation(-self.rotation());
        Some(
            to_view_center
                .mul(&scale)
                .mul(&to_origin)
                .mul(&undo_rotation),
        )
    }
}

impl Default for Camera2DState {
    fn default() -> Self {
        Self::new(Vec2::ZERO)
    }
}

// ---------------------------------------------------------------- label

/// 水平对齐。
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum HAlign {
    /// 左对齐。
    #[default]
    Left,
    /// 水平居中。
    Center,
    /// 右对齐。
    Right,
}

/// 垂直对齐。
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum VAlign {
    /// 上对齐。
    #[default]
    Top,
    /// 垂直居中。
    Center,
    /// 下对齐。
    Bottom,
}

/// Label 的文本状态（缺口契约之一）。
///
/// # 归属纪律（与调研报告风险点 2 一致）
///
/// 文本的**排版**（断行、字形度量、图集打包）属于 CPU 侧，落在提取层或
/// S3 的文本实现里，**不得下沉进 GPU 后端**。契约层只描述"要显示什么"：
/// 后端拿到 [`LabelState`] 后需要的是"按这些参数排好的字形序列"，
/// 而不是自己去解读字体文件。
///
/// # 为什么是 `Arc<str>`
///
/// `text` 用 `Arc<str>`（与 `nes-scene` 的 `Value::Str` 同一表示）：
/// 克隆 [`LabelState`] 只递增引用计数，**不复制文本字节、不分配堆内存**。
/// 提取层每帧把这些状态投进命令缓冲时，代价是 O(1) 的原子自增。
#[derive(Clone, PartialEq, Debug)]
pub struct LabelState {
    /// 文本内容（共享，克隆不复制字节）。
    pub text: Arc<str>,
    /// 字体资源键；[`RenderAssetKey::NIL`] 表示使用后端默认字体。
    pub font: RenderAssetKey,
    /// 字号（逻辑像素）。
    pub font_size: f32,
    /// 额外行距（逻辑像素，`0` = 由后端按字号决定）。
    pub line_spacing: f32,
    /// 水平对齐。
    pub align_h: HAlign,
    /// 垂直对齐。
    pub align_v: VAlign,
    /// 自动换行宽度（逻辑像素）；`None` = 不换行。
    pub wrap_width: Option<f32>,
    /// 文字着色（RGBA8 直 alpha；E-1 颜色通道，S12.1）。缺省白色
    /// —— 后端"采样色 x 1"与 E-1 之前逐位相同。
    pub color: [u8; 4],
    /// 文本光标（S12-2 TextInput）：`Some(n)` = 在第 `n` 个字符槽位画一根
    /// 1px 宽、字高竖条；`None` = 不画（缺省 —— 既有路径逐位不变）。
    /// 位置是**字符下标**（等宽 16px 冻结口径，后端 `n * 16.0` 推笔）；
    /// 闪隐节拍由提取层裁决（可见半拍才置 `Some`），后端零动画状态。
    pub caret: Option<u16>,
}

impl LabelState {
    /// 以文本与字号构造（其余取默认：默认字体、无额外行距、左上对齐、不换行）。
    pub fn new(text: impl Into<Arc<str>>, font_size: f32) -> Self {
        Self {
            text: text.into(),
            font: RenderAssetKey::NIL,
            font_size,
            line_spacing: 0.0,
            align_h: HAlign::Left,
            align_v: VAlign::Top,
            wrap_width: None,
            color: [255, 255, 255, 255],
            caret: None,
        }
    }
}

// ---------------------------------------------------------------- control

/// 控件边框的缺省色（历史哨兵绿 —— E-1 之前内建图集边框格的观感，
/// 保留为契约缺省使旧观感可经数据显式覆盖而非悄悄变化）。
pub const CONTROL_BORDER_LEGACY: [u8; 4] = [0, 255, 0, 255];

/// Control 的锚点布局状态（缺口契约之一）。
///
/// 与 Godot 的 `anchor_*` / `offset_*` 同构：四边各由
/// `anchor * parent_size + offset` 决定。`resolve` 是唯一权威算式 ——
/// 否则"同一份节点属性在不同 stage 里算出不同矩形"这类分歧会一直存在。
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct ControlState {
    /// 左锚点（0 = 父左，1 = 父右）。
    pub anchor_left: f32,
    /// 上锚点。
    pub anchor_top: f32,
    /// 右锚点。
    pub anchor_right: f32,
    /// 下锚点。
    pub anchor_bottom: f32,
    /// 左边偏移（像素）。
    pub offset_left: f32,
    /// 上边偏移（像素）。
    pub offset_top: f32,
    /// 右边偏移（像素）。
    pub offset_right: f32,
    /// 下边偏移（像素）。
    pub offset_bottom: f32,
    /// 最小尺寸下界；`None` = **无下界**（缺省值，负宽高原样保留）。
    ///
    /// `Some(min)` 时 `resolve` 只**扩张右下边**去满足它，不移动左上角；
    /// 无论取何值，`resolve` 都**不**把负宽高钳制到 0。
    pub min_size: Option<Vec2>,
    /// 填充色（RGBA8 直 alpha；`a == 0` = 无填充四边形）。缺省透明
    /// —— E-1 之前控件只画边框，缺省保持同像素。
    pub fill: [u8; 4],
    /// 边框色（RGBA8；`a == 0` = 无边框四边形）。缺省绿色哨兵
    /// [`CONTROL_BORDER_LEGACY`] —— 与内建图集边框格的历史观感逐位
    /// 一致（着色实现后图集格转中性白，绿色改经本字段进入）。
    pub border: [u8; 4],
    /// 边框线宽（像素；缺省 1 —— S12.0 设计语言：平直 1px 边框）。
    pub border_w: f32,
}

impl ControlState {
    /// 铺满父容器（四锚点 0,0,1,1 + 零偏移）。
    pub const FULL_RECT: Self = Self {
        anchor_left: 0.0,
        anchor_top: 0.0,
        anchor_right: 1.0,
        anchor_bottom: 1.0,
        offset_left: 0.0,
        offset_top: 0.0,
        offset_right: 0.0,
        offset_bottom: 0.0,
        min_size: None,
        fill: [0, 0, 0, 0],
        border: CONTROL_BORDER_LEGACY,
        border_w: 1.0,
    };

    /// 左上角固定尺寸（四锚点 0，偏移里写尺寸）。
    pub const fn new(anchors: [f32; 4], offsets: [f32; 4]) -> Self {
        Self {
            anchor_left: anchors[0],
            anchor_top: anchors[1],
            anchor_right: anchors[2],
            anchor_bottom: anchors[3],
            offset_left: offsets[0],
            offset_top: offsets[1],
            offset_right: offsets[2],
            offset_bottom: offsets[3],
            min_size: None,
            fill: [0, 0, 0, 0],
            border: CONTROL_BORDER_LEGACY,
            border_w: 1.0,
        }
    }

    /// 解析出相对父容器的矩形（`parent_size` 为父容器尺寸）。
    ///
    /// 步骤：四边各自 `anchor * parent + offset` → 得到宽高 →
    /// 若 `min_size` 为 `Some(min)`，宽/高小于 `min` 时只把右下边推出去
    /// （左上角不动，避免布局抖动）。
    /// 本函数**不钳制负尺寸**：负宽高保留原值，语义留给调用方
    /// （S1 开放问题 Q2 裁决 A；v1.1 起 `min_size` 缺省为 `None` = 无下界，
    /// 因此缺省路径下负宽高必然原样透传）。
    pub fn resolve(&self, parent_size: Vec2) -> Rect {
        let left = self.anchor_left * parent_size.x + self.offset_left;
        let top = self.anchor_top * parent_size.y + self.offset_top;
        let right = self.anchor_right * parent_size.x + self.offset_right;
        let bottom = self.anchor_bottom * parent_size.y + self.offset_bottom;
        let mut w = right - left;
        let mut h = bottom - top;
        if let Some(min) = self.min_size {
            if w < min.x {
                w = min.x;
            }
            if h < min.y {
                h = min.y;
            }
        }
        Rect::new(left, top, w, h)
    }
}
