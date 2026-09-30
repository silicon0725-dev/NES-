//! 帧信息与渲染命令。

use crate::handle::{ItemHandle, RenderAssetKey};
use crate::math::{Affine2, Vec2};
use crate::state::{Camera2DState, ControlState, Flip, LabelState};

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
    /// 控件布局（仅 Control 类渲染物）。
    SetRect {
        /// 句柄。
        handle: ItemHandle,
        /// 控件状态。
        rect: ControlState,
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
            | Self::SetRect { handle, .. } => Some(*handle),
            Self::SetCamera { .. } | Self::Submit { .. } => None,
        }
    }

    /// 是否是生命周期动作（Create / Destroy）。
    pub fn is_lifecycle(&self) -> bool {
        matches!(self, Self::CreateItem { .. } | Self::DestroyItem { .. })
    }
}
