//! 帧信息与渲染命令。

use crate::handle::{ItemHandle, RenderAssetKey};
use crate::math::{Affine2, Rect, Vec2};
use crate::state::{Camera2DState, ControlState, Flip, LabelState, ListState};

/// 一帧的上下文。
///
/// `frame_index` 是引擎侧的单调帧号（不是墙钟），后端可用它做缓冲轮转与
/// 幂等校验；`delta`/`time` 只作参考，**渲染真源不依赖它们**
/// （确定性纪律：同样输入必须出同样帧，时间不该进入绘制决策）。
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct FrameInfo {
    /// 单调帧号（从 0 或 1 起由引擎决定，契约只要求不倒退）。
    pub frame_index: u64,
    /// 距上一帧的秒数。
    pub delta: f32,
    /// 引擎运行时间（秒）。
    pub time: f64,
    /// 视口尺寸（设备像素）。
    pub viewport: Vec2,
    /// DPI 缩放系数（逻辑像素 → 设备像素）。
    pub dpi_scale: f32,
}

impl FrameInfo {
    /// 构造（`dpi_scale` 默认为 1）。
    pub const fn new(frame_index: u64, delta: f32, time: f64, viewport: Vec2) -> Self {
        Self {
            frame_index,
            delta,
            time,
            viewport,
            dpi_scale: 1.0,
        }
    }

    /// 追加 DPI 缩放（链式构造用）。
    pub fn with_dpi_scale(mut self, dpi_scale: f32) -> Self {
        self.dpi_scale = dpi_scale;
        self
    }
}

impl Default for FrameInfo {
    fn default() -> Self {
        Self::new(0, 0.0, 0.0, Vec2::ZERO)
    }
}

/// 渲染命令 —— **后端需要执行的动作**（线性缓冲，S4 的自研后端直接消费它）。
///
/// # 生命周期动作 vs 属性动作（契约冻结点）
///
/// - [`RenderCommand::CreateItem`] / [`RenderCommand::DestroyItem`] 是**生命周期**
///   动作：由 [`create_item`](crate::RenderServer::create_item) /
///   [`destroy_item`](crate::RenderServer::destroy_item) 触发，**即时**进入待发队列
///   （顺序 = 调用顺序），下次 `submit` 时落到缓冲里；
/// - `Set*` 是**属性**动作：`set_*` 只更新服务端内部记录，`submit` 时按绘制次序
///   统一输出当前**全量**属性快照（见 [`RenderServer::submit_into`](crate::RenderServer::submit_into)）。
///
/// 这样切分的好处：Create/Destroy 与 GPU 资源的创建/释放一一对应（谁都不能省），
/// 而属性流是全量幂等的，后端不必维护跨帧 diff，漏推一帧也不会状态漂移。
#[derive(Clone, PartialEq, Debug)]
pub enum RenderCommand {
    /// 新建渲染物（后端在此按 `key` 准备 GPU 侧资源）。
    CreateItem {
        /// 句柄。
        handle: ItemHandle,
        /// 稳定资源键。
        key: RenderAssetKey,
    },
    /// 销毁渲染物（后端在此释放 GPU 侧资源）。
    DestroyItem {
        /// 句柄。
        handle: ItemHandle,
    },
    /// 可见性。
    SetVisible {
        /// 句柄。
        handle: ItemHandle,
        /// 是否可见。
        visible: bool,
    },
    /// 世界变换。
    SetTransform {
        /// 句柄。
        handle: ItemHandle,
        /// 世界变换。
        transform: Affine2,
    },
    /// 层号与同层次序。
    SetZ {
        /// 句柄。
        handle: ItemHandle,
        /// 层号。
        z: i32,
        /// 同层次序。
        order: u64,
    },
    /// 翻转。
    SetFlip {
        /// 句柄。
        handle: ItemHandle,
        /// 翻转。
        flip: Flip,
    },
    /// 相机（每帧最多一条，位于属性流之前）。
    SetCamera {
        /// 相机状态。
        camera: Camera2DState,
    },
    /// 文本（仅 Label 类渲染物）。
    SetText {
        /// 句柄。
        handle: ItemHandle,
        /// 文本状态（`Arc` 克隆，不复制字节）。
        text: LabelState,
    },
    /// 列表/页签（S12-3 任务 4；ListView / Tabs 摊平后的专属载荷）。
    ///
    /// 与 [`RenderCommand::SetText`] 同一性质：属性动作、全量快照、按序重放。
    /// 输出序冻结在 `SetText` 之后、`SetRect` 之前（同一渲染物的属性流序：
    /// SetText → SetList → SetRect → SetClip —— null 与 wgpu 两处 submit
    /// 严格同序）。
    SetList {
        /// 句柄。
        handle: ItemHandle,
        /// 列表状态（`Arc` 克隆，不复制行文本字节）。
        rows: ListState,
    },
    /// 控件布局（仅 Control 类渲染物）。
    SetRect {
        /// 句柄。
        handle: ItemHandle,
        /// 控件状态。
        rect: ControlState,
    },
    /// 裁剪矩形（E-2 裁剪契约，S12-3，语义裁决 D1）。
    ///
    /// 裁剪是**渲染物属性**，不是流式栈：这与"属性流 = 全量快照、按序重放、
    /// 跨帧幂等"的契约一致 —— 栈式流序状态会破坏"漏推一帧不漂移"的不变式。
    /// 嵌套裁剪由提取层沿祖先链求交集后以单条 `SetClip` 下发（本层不做栈语义）。
    ///
    /// - `rect = Some(r)`：`r` 是**已解析的视口空间**矩形，后端按目标尺寸折算成
    ///   帧缓冲像素 scissor（半开区间）；
    /// - `rect = None`：清除该条目的裁剪；
    /// - 条目销毁（`DestroyItem`）时裁剪随条目消亡；
    /// - 本命令恒出现在对应条目的 `SetRect` 之后。
    SetClip {
        /// 句柄。
        handle: ItemHandle,
        /// 视口空间裁剪矩形；`None` = 清除裁剪。
        rect: Option<Rect>,
    },
    /// 着色（S16.1 alpha 通道的契约扩展）：RGBA8 直 alpha 相乘色。
    ///
    /// 与 E-1 的 ControlState/LabelState 颜色字段同一相乘语义（采样色 x tint，
    /// 中性 `[255,255,255,255]` = 恒等），但作为**独立属性命令**存在：
    /// 精灵（Sprite）没有自己的颜色字段，alpha 通道经本命令进入。
    ///
    /// - 属性动作、全量快照、同键覆写：`submit` 时对有 tint 簿记的条目按序
    ///   重发当前值（漏推一帧不漂移）；条目销毁时随条目消亡；
    /// - 输出序冻结在对应条目的 `SetRect` / `SetClip` 之后（同一渲染物的
    ///   属性流序：… → SetRect → SetClip → SetTint —— null 与 wgpu 两处
    ///   submit 严格同序）；
    /// - 空句柄 / 未知句柄静默忽略（契约 I1 口径）。
    SetTint {
        /// 句柄。
        handle: ItemHandle,
        /// 相乘色（RGBA8 直 alpha；RGB 与 alpha 都参与相乘 —— 提取层
        /// 的 alpha 通道只动 A：`[255, 255, 255, a]`）。
        rgba: [u8; 4],
    },
    /// 子矩形采样（S16.2 图集帧动画的契约扩展）：注册表纹理的**归一化
    /// UV 矩形** `[u0, v0, us, vs]`（0..1，相对整张注册纹理）。
    ///
    /// 与 [`RenderCommand::SetTint`] 同一性质与同一条纪律：
    ///
    /// - 属性动作、全量快照、同键覆写：`submit` 时对有 uv 簿记的条目按序
    ///   重发当前值（漏推一帧不漂移）；条目销毁时随条目消亡；
    /// - 输出序冻结在对应条目的 `SetTint` 之后（同一渲染物的属性流序：
    ///   … → SetRect → SetClip → SetTint → SetUv —— null 与 wgpu 两处
    ///   submit 严格同序）；
    /// - 空句柄 / 未知句柄静默忽略（契约 I1 口径）；
    /// - **无记录 = 整瓦片采样**（既有行为逐位不变）：后端只在注册表
    ///   精灵分支消费本命令 —— `[0, 0, 1, 1]`（恒等矩形）折算后与
    ///   sample_info 的全瓦片矩形逐位相同，因此"清除"恒等矩形即可表达；
    /// - 提取层把 Sprite2D 的 `sheet_cols` / `sheet_rows` / `frame` 三属性
    ///   折成网格分数（col/cols 等纯分数运算 —— **不需要**纹理像素尺寸，
    ///   提取层也不可见），后端拿注册表实际尺寸一折即得采样矩形。
    SetUv {
        /// 句柄。
        handle: ItemHandle,
        /// 归一化 UV 矩形 `[u0, v0, us, vs]`（相对整张注册纹理；负值/
        /// 越界不在此钳制 —— 采样取舍的权威在后端单处折算）。
        rect: [f32; 4],
    },
    /// 精灵锚点（S16.3 精灵锚点的契约扩展）：**归一化锚点** `[px, py]`
    /// （0..1 相对精灵矩形；越界照实接受 = 锚点落在精灵外，合法用途）。
    ///
    /// 与 [`RenderCommand::SetTint`] / [`RenderCommand::SetUv`] 同一性质
    /// 与同一条纪律：
    ///
    /// - 属性动作、全量快照、同键覆写：`submit` 时对有 pivot 簿记的条目
    ///   按序重发当前值（漏推一帧不漂移）；条目销毁时随条目消亡；
    /// - 输出序冻结在对应条目的 `SetUv` 之后（同一渲染物的属性流序：
    ///   … → SetRect → SetClip → SetTint → SetUv → SetPivot —— null 与
    ///   wgpu 两处 submit 严格同序）；
    /// - 空句柄 / 未知句柄静默忽略（契约 I1 口径）；
    /// - **无记录 = 无平移**（既有行为逐位不变）：后端只在注册表精灵
    ///   分支消费本命令 —— 语义是精灵四边形在**变换前的局部空间**平移
    ///   `-pivot × 16px 基准格`（`world ∘ translation`，平移在 world 之后
    ///   乘 = 先平移后过世界变换），旋转/缩放因此绕锚点发生；`[0, 0]`
    ///   记录 = 零平移 = 恒等（"清除"零向量即可表达）；
    /// - **与图集帧的关系**：pivot 归一化相对**当前帧矩形**（帧动画换帧
    ///   不换 pivot 语义）—— 帧采样只影响 uv 不影响几何，天然成立；
    /// - 提取层把 Sprite2D 的 `pivot` 属性（Vec2）直读下发；`(0,0)` 缺省
    ///   不推（命令流与既有路径逐条相同）。
    SetPivot {
        /// 句柄。
        handle: ItemHandle,
        /// 归一化锚点 `[px, py]`（0..1 相对精灵矩形；负值/越界照实接受
        /// —— 超出精灵外锚定是合法创作用途，取舍权威在后端单处折算）。
        pivot: [f32; 2],
    },
    /// 帧结束标记（**每条命令流都必须以它结尾**）。
    Submit {
        /// 本帧上下文。
        frame: FrameInfo,
    },
}

impl RenderCommand {
    /// 该命令指向的渲染物句柄（`SetCamera` / `Submit` 返回 `None`）。
    pub fn handle(&self) -> Option<ItemHandle> {
        match self {
            Self::CreateItem { handle, .. }
            | Self::DestroyItem { handle }
            | Self::SetVisible { handle, .. }
            | Self::SetTransform { handle, .. }
            | Self::SetZ { handle, .. }
            | Self::SetFlip { handle, .. }
            | Self::SetText { handle, .. }
            | Self::SetList { handle, .. }
            | Self::SetRect { handle, .. }
            | Self::SetClip { handle, .. }
            | Self::SetTint { handle, .. }
            | Self::SetUv { handle, .. }
            | Self::SetPivot { handle, .. } => Some(*handle),
            Self::SetCamera { .. } | Self::Submit { .. } => None,
        }
    }

    /// 是否是生命周期动作（Create / Destroy）。
    pub fn is_lifecycle(&self) -> bool {
        matches!(self, Self::CreateItem { .. } | Self::DestroyItem { .. })
    }
}
