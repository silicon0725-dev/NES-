//! 命令消费层：[`RenderServer`] 实现 + 线性命令流消费 + 精灵管线。
//!
//! # 分工（为什么 `WgpuRenderServer` 不直接碰 GPU）
//!
//! [`RenderServer`] 的方法签名**不返回错误**（契约层面向引擎热路径，推属性不允许
//! 中途失败），而 GPU 侧的任何一步都可能失败。把两者塞进同一个类型会让
//! "属性推送"与"驱动装配"的生命周期互相绑架。因此本模块切成两半：
//!
//! - [`WgpuRenderServer`]：契约侧簿记（与 `NullRenderServer` 同构的纯 CPU 实现），
//!   只负责"把属性记下来、按 [`DrawKey`](nes_render_api::DrawKey) 排好、产出命令流"；
//! - [`CommandConsumer`]：GPU 侧执行器，逐条消费命令流，**每一步失败都能指名道姓**
//!   （返回 [`BackendError`]），跑完一帧给出 [`FrameOutcome`]（像素 + 统计）。
//!
//! # 条目表的生命周期（与契约 I4 对齐）
//!
//! 命令流是"一次性生命周期事件 + 每帧全量属性快照"的复合体：`CreateItem` /
//! `DestroyItem` 只在发生后的**那次** submit 落缓冲，属性流则每帧重复。
//! 因此 [`CommandConsumer`] 的条目表**跨帧持有**（`Create` 建、`Destroy` 删），
//! 属性命令按句柄更新已有条目 —— "每帧提取"（方案 D 第三件套）约束的是场景侧
//! 每帧重提取属性推成全量快照，不是后端遗忘自己的条目登记。
//!
//! # S4.1 的范围声明（如实报告）
//!
//! - 只光栅化**清屏 + 精灵**。`SetText` / `SetRect` 被确认并记账
//!   （`FrameStats::updates`），但不产生像素 —— 排版归属 CPU 侧，本阶段无字形光栅；
//! - 读回像素的通道语义：存储即 `RGBA8Unorm` 的原始 RGBA 字节
//!   （S4.1 实机验证过：BGRA 假设会让红/蓝互换），语义访问走 [`FrameImage::pixel`]；
//! - 精灵统一采样图集：`key.slot % (ATLAS_CELLS²)` 决定采样格，格 0 是真实图案、
//!   格 1 是控件边框、其余格是品红哨兵色（采错格会立刻显形，见
//!   [`gpu::ATLAS_FILLER_COLOR`]）；
//! - 控件（有 `SetRect` 状态的渲染物）按 **HUD 口径**画成 1px 边框的视口空间
//!   矩形：锚点以相机视口为父尺寸解析 —— 后端没有场景侧父节点信息，这是
//!   文档化的降级口径（上游补齐场景属性入口后可换精确父尺寸）；矩形经视图
//!   矩阵的逆折回世界空间，与精灵共用同一次绘制，纹理键对控件不再是必要条件；
//! - `SetText`：**S4.4 起产生像素** —— 消费器把文本按字形表展开成"每字形一个
//!   四边形"的实例（采样注册表里的字形表纹理）。字形栅格化在外部完成
//!   （烘焙工具产出位图字形表，见 `examples/assets`），本 crate 零依赖；
//!   未设置默认字体且 `LabelState.font` 未注册时不画（静默，记账不变）。
//!   S4.4 布局口径：`\n` 多行、行高 = 基准行高 + `line_spacing`、左上锚点、
//!   字距恒定（等宽口径）；`font_size` 缩放与 `align_*`/`wrap_width` 记账
//!   不参与布局（需要对齐/换行需先有排版框语义，属后续）。
//! - **绘制锚点 = 渲染物局部原点（左上角）**：四边形从局部 `(0,0)` 铺到
//!   `(CELL_PX, CELL_PX)`。推论：flip 按契约 I8 的 `transform ∘ flip` 绕**原点**
//!   镜像（水平翻转让精灵出现在锚点左侧），平移分量逐位不变。M5 Scratch
//!   兼容层若需"绕中心翻转"，由兼容层在翻转时补偿半个 extents，不改本层。
//!
//! # 相机
//!
//! 契约 I9：`enabled == false` 时 `view_matrix()` 为 `None`，后端应"退回上一有效
//! 相机或不做变换"。本消费器是**帧本地**的（无跨帧状态可退），故退回**单位视图**；
//! 相机视口尺寸非法（非正）时同样退回，避免着色器里除零产生 NaN 坐标。

use core::ffi::c_void;
use core::ptr;
use std::collections::BTreeMap;
use std::path::Path;

use nes_render_api::command::{FrameInfo, RenderCommand};
use nes_render_api::handle::{ItemHandle, RenderAssetKey};
use nes_render_api::item::RenderItem;
use nes_render_api::math::Affine2;
use nes_render_api::server::RenderServer;
use nes_render_api::state::{Camera2DState, ControlState, Flip, LabelState};

use crate::error::BackendError;
use crate::ffi;
use crate::gpu::{self, FrameImage, GpuContext, RenderTarget, SpriteAtlas};
use crate::png::write_rgba8_png;

// ------------------------------------------------------------ 常量

/// 清屏色（深藏青 `rgba(13,13,25,255)`）。
///
/// 与精灵红 [`gpu::SPRITE_BODY_COLOR`]、眼白 [`gpu::SPRITE_EYE_COLOR`]、
/// 哨兵品红 [`gpu::ATLAS_FILLER_COLOR`] 四者互不相同：断言"这个像素是背景"
/// 才有区分度，而不是"某个恰好很暗的颜色"。出口准则测试与示例共用它做锚点。
pub const CLEAR_COLOR: ffi::ClearColor = ffi::ClearColor {
    r: 13.0 / 255.0,
    g: 13.0 / 255.0,
    b: 25.0 / 255.0,
    a: 1.0,
};

/// 视图统一缓冲字节数（8 个 `f32`：2x3 视图矩阵 + 视口尺寸）。
///
/// 与 `gpu::Sizes::viewport`（图集绑定组布局声明的最小绑定尺寸）一致：
/// 绑定得比声明小，驱动会在创建绑定组时判定布局不兼容。
const VIEW_UNIFORM_BYTES: u64 = 8 * core::mem::size_of::<f32>() as u64;

/// 单个精灵实例的字节跨度（12 个 `f32`：2x3 世界矩阵 + UV 矩形 + 采样来源）。
///
/// 与 `gpu::Sizes` 系（图集侧声明的视图缓冲尺寸）同一纪律：布局声明与
/// CPU 打包必须同步，扩字段时两处一起改。
const INSTANCE_STRIDE: u64 = 12 * core::mem::size_of::<f32>() as u64;

/// 初始实例容量（64 个精灵 = 2 KiB；不足时按需倍增重建缓冲）。
const INITIAL_INSTANCE_CAPACITY: u32 = 64;

/// 精灵着色器（WGSL）。
///
/// 几何由 `vertex_index` 生成（两个三角形拼一个四边形，**不需要顶点缓冲存几何**），
/// 每精灵数据走实例步进的 8 个 `f32`。着色器里的字面量 `4.0` 是图集每边格数
/// （[`gpu::ATLAS_CELLS`]），构建期有 `debug_assert` 钉住同步。
const SPRITE_WGSL: &str = r#"
// 单个精灵的绘制边长（像素）= gpu::CELL_PX（构建期 debug_assert 钉住同步）。
const SPRITE_PX: f32 = 16.0;

// 视图参数（uniform，binding 0）：世界 -> 视图矩阵按三列 vec2 存 + 视口像素尺寸。
// uniform 地址空间要求矩阵列跨度 16 字节对齐（mat3x2 的 8 字节跨度不合法），
// 故矩阵拆列存储、在着色器内拼回。
struct ViewParams {
    col0: vec2<f32>,
    col1: vec2<f32>,
    col2: vec2<f32>,
    viewport: vec2<f32>,
};
// 每精灵实例数据（实例步进顶点缓冲，12 个 f32 = 48 字节）：
//   loc0..2 = 世界矩阵三列（已含 flip 的子局部后乘）；
//   loc3    = UV 矩形 (u0, v0, us, vs)；
//   loc4    = 采样来源 (瓦片号, 类型)：类型 0 = 内建图集、1 = 注册表瓦片
//             （注册表是单张平铺大纹理，位置全在 UV 矩形里；瓦片号仅作
//             实例侧留档，着色器当前不读它）。
struct SpriteData {
    @location(0) col0: vec2<f32>,
    @location(1) col1: vec2<f32>,
    @location(2) col2: vec2<f32>,
    @location(3) uv_rect: vec4<f32>,
    @location(4) source: vec2<f32>,
};
struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) corner: vec2<f32>,
    @location(1) uv_rect: vec4<f32>,
    @location(2) source: vec2<f32>,
};

@group(0) @binding(0) var<uniform> VIEW: ViewParams;
@group(0) @binding(1) var ATLAS: texture_2d<f32>;
@group(0) @binding(2) var ATLAS_SAMPLER: sampler;
@group(1) @binding(0) var USER_TEX: texture_2d<f32>;
@group(1) @binding(1) var USER_SAMPLER: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, data: SpriteData) -> VSOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vi];
    // 世界矩阵（已含 flip 的子局部后乘）把"单位格"摆进世界，视图矩阵再搬到视口像素。
    let world = mat3x2<f32>(data.col0, data.col1, data.col2)
        * vec3<f32>(corner * vec2<f32>(SPRITE_PX), 1.0);
    let view = mat3x2<f32>(VIEW.col0, VIEW.col1, VIEW.col2) * vec3<f32>(world, 1.0);
    // 视口像素 -> NDC：x 拉伸平移，y 还要上下翻转（像素 Y 向下、NDC Y 向上）。
    var out: VSOut;
    out.pos = vec4<f32>(
        view.x / VIEW.viewport.x * 2.0 - 1.0,
        1.0 - view.y / VIEW.viewport.y * 2.0,
        0.0,
        1.0,
    );
    out.corner = corner;
    out.uv_rect = data.uv_rect;
    out.source = data.source;
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let uv = in.uv_rect.xy + in.corner * in.uv_rect.zw;
    // 两路采样同构（texture_2d 的四参显式 LOD 形式，S4.1 实证可用）：
    // 隐式 LOD 的 textureSample 要求均匀控制流，而来源选择是逐实例数据；
    // 本管线无 mip、像素画 1:1 映射，level 0 即精确值。
    var color: vec4<f32>;
    if (in.source.y < 0.5) {
        color = textureSampleLevel(ATLAS, ATLAS_SAMPLER, uv, 0.0);
    } else {
        color = textureSampleLevel(USER_TEX, USER_SAMPLER, uv, 0.0);
    }
    // 透明像素丢弃（控件边框格的内部与注册表图层的未写区域）：
    // 管线不带混合，靠丢弃"透出"下层。
    if (color.a < 0.5) {
        discard;
    }
    return color;
}
"#;

// ------------------------------------------------------------ WgpuRenderServer

/// 契约侧渲染服务端（纯 CPU 簿记）。
///
/// 与 `NullRenderServer` 同构：同样的忽略规则、同样的命令顺序、同样的快照语义
/// （它就是契约不变式 I1~I7 在真实后端里的第一半）。GPU 侧执行由
/// [`CommandConsumer`] 承担 —— 两者经命令流衔接，这正是"属性推送"与
/// "驱动装配"解耦后的组装点。
#[derive(Debug, Default)]
pub struct WgpuRenderServer {
    next_handle: u64,
    items: BTreeMap<ItemHandle, RenderItem>,
    labels: BTreeMap<ItemHandle, LabelState>,
    rects: BTreeMap<ItemHandle, ControlState>,
    camera: Option<Camera2DState>,
    lifecycle: Vec<RenderCommand>,
}

impl WgpuRenderServer {
    /// 新建（无渲染物、无相机）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 存活渲染物数量。
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否没有任何渲染物。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 取单个渲染物。
    pub fn item(&self, handle: ItemHandle) -> Option<&RenderItem> {
        self.items.get(&handle)
    }

    /// 当前相机。
    pub fn camera(&self) -> Option<&Camera2DState> {
        self.camera.as_ref()
    }

    /// 按 [`DrawKey`](nes_render_api::DrawKey) 升序的句柄序列 —— 即本帧应采纳的绘制次序。
    pub fn draw_order(&self) -> Vec<ItemHandle> {
        let mut items: Vec<&RenderItem> = self.items.values().collect();
        items.sort_by_key(|item| item.draw_key());
        items.into_iter().map(|item| item.handle).collect()
    }
}

impl RenderServer for WgpuRenderServer {
    fn create_item(&mut self, key: RenderAssetKey) -> ItemHandle {
        // 单调计数器分配，永不复用（契约 I2）；
        // 真实 arena + 代际分配是后续优化，行为契约与此等价。
        self.next_handle += 1;
        let handle = ItemHandle::from_raw(self.next_handle);
        self.items
            .insert(handle, RenderItem::new(handle, key, Affine2::IDENTITY));
        self.lifecycle.push(RenderCommand::CreateItem { handle, key });
        handle
    }

    fn destroy_item(&mut self, handle: ItemHandle) {
        if handle.is_nil() || self.items.remove(&handle).is_none() {
            return;
        }
        self.labels.remove(&handle);
        self.rects.remove(&handle);
        self.lifecycle.push(RenderCommand::DestroyItem { handle });
    }

    fn set_visible(&mut self, handle: ItemHandle, visible: bool) {
        if let Some(item) = self.items.get_mut(&handle) {
            item.visible = visible;
        }
    }

    fn set_transform(&mut self, handle: ItemHandle, transform: Affine2) {
        if let Some(item) = self.items.get_mut(&handle) {
            item.transform = transform;
        }
    }

    fn set_z(&mut self, handle: ItemHandle, z: i32, order: u64) {
        if let Some(item) = self.items.get_mut(&handle) {
            item.z = z;
            item.order = order;
        }
    }

    fn set_flip(&mut self, handle: ItemHandle, flip: Flip) {
        if let Some(item) = self.items.get_mut(&handle) {
            item.flip = flip;
        }
    }

    fn set_camera(&mut self, camera: &Camera2DState) {
        self.camera = Some(*camera);
    }

    fn set_text(&mut self, handle: ItemHandle, text: &LabelState) {
        if self.items.contains_key(&handle) {
            self.labels.insert(handle, text.clone());
        }
    }

    fn set_rect(&mut self, handle: ItemHandle, rect: &ControlState) {
        if self.items.contains_key(&handle) {
            self.rects.insert(handle, *rect);
        }
    }

    fn submit_into(&mut self, frame: &FrameInfo, out: &mut Vec<RenderCommand>) {
        // 1) 先清空（契约 I3：缓冲跨帧复用，不留上一帧残留）。
        out.clear();

        // 2) 生命周期动作：即时入队、submit 时按发生顺序落缓冲（契约 I4/I6）。
        out.append(&mut self.lifecycle);

        // 3) 相机（每帧最多一条，位于属性流之前）。
        if let Some(camera) = self.camera {
            out.push(RenderCommand::SetCamera { camera });
        }

        // 4) 各渲染物：按 DrawKey 升序输出全量属性快照（契约 I5）。
        let mut ordered: Vec<&RenderItem> = self.items.values().collect();
        ordered.sort_by_key(|item| item.draw_key());
        for item in ordered {
            out.push(RenderCommand::SetTransform {
                handle: item.handle,
                transform: item.transform,
            });
            out.push(RenderCommand::SetFlip {
                handle: item.handle,
                flip: item.flip,
            });
            out.push(RenderCommand::SetZ {
                handle: item.handle,
                z: item.z,
                order: item.order,
            });
            out.push(RenderCommand::SetVisible {
                handle: item.handle,
                visible: item.visible,
            });
            if let Some(text) = self.labels.get(&item.handle) {
                out.push(RenderCommand::SetText {
                    handle: item.handle,
                    text: text.clone(),
                });
            }
            if let Some(rect) = self.rects.get(&item.handle) {
                out.push(RenderCommand::SetRect {
                    handle: item.handle,
                    rect: *rect,
                });
            }
        }

        // 5) 帧结束标记（契约 I3：末条必为 Submit）。
        out.push(RenderCommand::Submit { frame: *frame });
    }
}

// ------------------------------------------------------------ 帧统计与结果

/// 一帧的消费统计（把静默行为显式化 —— 与 `ServerCounters` 同一理由）。
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct FrameStats {
    /// 消费的命令总数（含 `Submit` 终止标记）。
    pub commands: u64,
    /// `CreateItem` 命中数。
    pub creates: u64,
    /// `DestroyItem` 命中数。
    pub destroys: u64,
    /// 命中已知句柄的属性命令数（`SetTransform` / `SetFlip` / `SetZ` /
    /// `SetVisible` / `SetText` / `SetRect`）。
    pub updates: u64,
    /// 因空句柄 / 未知句柄被静默忽略的命令数（契约 I1 的可观测计数）。
    pub ignored: u64,
    /// 因不可见或资源键未绑定而跳过绘制的渲染物数。
    pub skipped: u64,
    /// 实际提交绘制的四边形数（精灵 + 控件 + 字形）。
    pub drawn: u64,
    /// 其中控件边框四边形数（有 `SetRect` 状态、按 HUD 口径绘制的渲染物）。
    pub controls: u64,
    /// 其中从纹理注册表采样真实纹理的精灵数（不含字形；字形单列）。
    pub from_registry: u64,
    /// 其中字形四边形数（`SetText` 展开的文本像素）。
    pub glyphs: u64,
    /// 是否应用了相机（相机存在、启用且视口合法）。
    pub camera_applied: bool,
    /// 本帧帧号（来自 `Submit` 携带的 `FrameInfo`）。
    pub frame_index: u64,
    /// 帧内新增的驱动侧未捕获错误条数（>0 时像素证据应存疑）。
    pub driver_errors: u64,
}

/// 一帧的结果：读回的像素 + 统计。
#[derive(Debug)]
pub struct FrameOutcome {
    /// 离屏目标读回的像素（RGBA 存储，语义访问走 [`FrameImage::pixel`]）。
    pub image: FrameImage,
    /// 本帧消费统计。
    pub stats: FrameStats,
}

impl FrameOutcome {
    /// 把本帧像素编码为 RGBA8 并落盘 PNG（返回写入字节数）。
    ///
    /// 存储即 RGBA（[`FrameImage`]），编码直接透传；与原型日志的逐像素
    /// 比对已在 `s41_visual_closure` 示例内完成。
    pub fn write_png(&self, path: &Path) -> Result<usize, BackendError> {
        write_rgba8_png(path, self.image.width, self.image.height, &self.image.rgba)
    }
}

// ------------------------------------------------------------ SpritePipeline

/// 一条精灵绘制记录（实例缓冲的单条数据，12 个 `f32`）。
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct SpriteInstance {
    /// 渲染物句柄（不进 GPU，仅供帧对账与诊断）。
    pub handle: ItemHandle,
    /// 世界矩阵（`transform ∘ flip`，契约 I8），展平为 `[a, b, c, d, tx, ty]`。
    pub world: [f32; 6],
    /// UV 矩形 `[u0, v0, us, vs]`：内建图集 = 采样格子矩形；
    /// 注册表纹理 = 大纹理瓦片坐标系里的子矩形（瓦片左上角 + 实际尺寸裁剪）。
    pub uv_rect: [f32; 4],
    /// 采样来源 `[瓦片号, 类型]`：类型 0 = 内建图集、1 = 注册表瓦片
    ///（瓦片号仅供帧对账留档，注册表位置信息已并入 UV 矩形）。
    pub source: [f32; 2],
}

/// 资源键 → 图集采样格的 UV 矩形。
///
/// 映射规则：`key.slot % (ATLAS_CELLS²)` 确定性选格。格 0 是真实图案、
/// 格 1 是控件边框、其余格是品红哨兵 —— 采样错格在像素层面立刻显形。
/// 键已注册进纹理注册表时不走本函数（消费器按注册表采样）。
fn cell_uv_rect(key: RenderAssetKey) -> [f32; 4] {
    let cells = gpu::ATLAS_CELLS;
    let index = key.slot() % (cells * cells);
    let cell = gpu::CELL_PX as f32 / gpu::ATLAS_PX as f32;
    [
        (index % cells) as f32 * cell,
        (index / cells) as f32 * cell,
        cell,
        cell,
    ]
}

/// 释放管线句柄所需的函数指针（理由同 `gpu::TargetOps`：让 `Drop` 独立成立）。
#[derive(Clone, Copy)]
struct PipelineOps {
    pipeline_layout_release: unsafe extern "system" fn(*mut c_void),
    render_pipeline_release: unsafe extern "system" fn(*mut c_void),
    shader_module_release: unsafe extern "system" fn(*mut c_void),
    buffer_release: unsafe extern "system" fn(*mut c_void),
}

/// 精灵管线：着色器 + 渲染管线 + 实例缓冲，外加从图集借来的绑定组。
///
/// 一次装配、逐帧复用；逐帧变动的只有两个缓冲的内容（写入即用，无跨帧状态）。
/// 绑定组与视图 uniform 缓冲**沿用 [`SpriteAtlas`] 的装配**（group 0：
/// binding 0 = 视图 uniform、1 = 图集纹理、2 = 采样器）：管线每帧把视图参数
/// 写进图集的缓冲、渲染时绑定图集的绑定组。这两个句柄是**借用的** ——
/// 本管线的 `Drop` 不释放它们，生命周期由同住一个 [`CommandConsumer`] 的
/// 图集保证（先装配、后析构）。
///
/// 管线布局由本管线自持：`[图集绑定组布局, 纹理注册表绑定组布局]`
///（group 0 / group 1），采样来源按实例数据在内建图集与注册表之间二选一。
pub struct SpritePipeline {
    module: *mut c_void,
    pipeline: *mut c_void,
    pipeline_layout: *mut c_void,
    instance_buffer: *mut c_void,
    instance_capacity: u32,
    /// 图集的视图 uniform 缓冲（借用，不释放）。
    view_uniform: *mut c_void,
    /// 图集的绑定组（借用，不释放）。
    bind_group: *mut c_void,
    staging: Vec<u8>,
    ops: PipelineOps,
}

impl SpritePipeline {
    /// 装配管线（创建着色器、管线布局、实例缓冲与渲染管线；group 0 沿用图集装配）。
    pub fn new(
        ctx: &GpuContext,
        atlas: &SpriteAtlas,
        registry: &gpu::TextureRegistry,
    ) -> Result<Self, BackendError> {
        debug_assert_eq!(gpu::ATLAS_CELLS, 4, "WGSL 内嵌的图集格数字面量需要同步更新");
        debug_assert_eq!(gpu::CELL_PX, 16, "WGSL 内嵌的精灵边长字面量需要同步更新");
        let api = ctx.api();
        let device = ctx.device();

        // 1) 着色器模块（WGSL 源经链式结构挂载）。
        let source = ffi::ShaderSourceWgsl {
            chain: ffi::ChainedStruct {
                next: ptr::null_mut(),
                s_type: ffi::WGPU_STYPE_SHADER_SOURCE_WGSL,
            },
            code: ffi::StringView::from_static(SPRITE_WGSL),
        };
        let module_desc = ffi::ShaderModuleDescriptor {
            next_in_chain: &source as *const ffi::ShaderSourceWgsl as *mut c_void,
            label: ffi::StringView::from_static("nes-sprite-shader"),
        };
        // SAFETY: `source` 在本次调用期间存活；wgpu 在创建时同步消费 WGSL 视图。
        let module = unsafe { (api.device_create_shader_module)(device, &module_desc) };
        if module.is_null() {
            return Err(BackendError::NullHandle("WGPUShaderModule(sprite)"));
        }

        // 2) 实例缓冲（Vertex | CopyDst，容量不足时由 ensure_capacity 重建）。
        let instance_buffer = create_buffer(
            ctx,
            "nes-sprite-instances",
            ffi::WGPU_BUFFER_USAGE_VERTEX | ffi::WGPU_BUFFER_USAGE_COPY_DST,
            INSTANCE_STRIDE * INITIAL_INSTANCE_CAPACITY as u64,
        )?;

        // 3) 管线布局：group 0 = 图集，group 1 = 纹理注册表。
        let layouts = [atlas.handles().bind_group_layout, registry.bind_group_layout()];
        let mut pl_desc = ffi::PipelineLayoutDescriptor {
            bind_group_layout_count: layouts.len(),
            bind_group_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        pl_desc.label = ffi::StringView::from_static("nes-sprite-pipeline-layout");
        // SAFETY: 描述符与 layouts 数组在本次调用期间存活。
        let pipeline_layout = unsafe { (api.device_create_pipeline_layout)(device, &pl_desc) };
        if pipeline_layout.is_null() {
            unsafe {
                (api.buffer_release)(instance_buffer);
                (api.shader_module_release)(module);
            }
            return Err(BackendError::NullHandle("WGPUPipelineLayout(sprite)"));
        }

        // 4) 渲染管线的顶点属性表：三列矩阵 + UV 矩形 + 采样来源（步长 48 字节）。
        let attributes = [
            ffi::VertexAttribute {
                next_in_chain: ptr::null_mut(),
                format: ffi::WGPU_VERTEX_FORMAT_FLOAT32X2,
                offset: 0,
                shader_location: 0,
            },
            ffi::VertexAttribute {
                next_in_chain: ptr::null_mut(),
                format: ffi::WGPU_VERTEX_FORMAT_FLOAT32X2,
                offset: 8,
                shader_location: 1,
            },
            ffi::VertexAttribute {
                next_in_chain: ptr::null_mut(),
                format: ffi::WGPU_VERTEX_FORMAT_FLOAT32X2,
                offset: 16,
                shader_location: 2,
            },
            ffi::VertexAttribute {
                next_in_chain: ptr::null_mut(),
                format: ffi::WGPU_VERTEX_FORMAT_FLOAT32X4,
                offset: 24,
                shader_location: 3,
            },
            ffi::VertexAttribute {
                next_in_chain: ptr::null_mut(),
                format: ffi::WGPU_VERTEX_FORMAT_FLOAT32X2,
                offset: 40,
                shader_location: 4,
            },
        ];
        let buffers = [ffi::VertexBufferLayout {
            next_in_chain: ptr::null_mut(),
            step_mode: ffi::WGPU_VERTEX_STEP_MODE_INSTANCE,
            array_stride: INSTANCE_STRIDE,
            attribute_count: attributes.len(),
            attributes: attributes.as_ptr(),
        }];
        let vertex = ffi::VertexState {
            next_in_chain: ptr::null_mut(),
            module,
            entry_point: ffi::StringView::from_static("vs_main"),
            constant_count: 0,
            constants: ptr::null(),
            buffer_count: buffers.len(),
            buffers: buffers.as_ptr(),
        };
        let targets = [ffi::ColorTargetState {
            next_in_chain: ptr::null_mut(),
            // 与 RenderTarget 的纹理格式一致，否则管线与附件不兼容。
            format: ffi::WGPU_TEXTURE_FORMAT_RGBA8_UNORM,
            blend: ptr::null(),
            write_mask: ffi::WGPU_COLOR_WRITE_MASK_ALL,
        }];
        let fragment_state = ffi::FragmentState {
            next_in_chain: ptr::null_mut(),
            module,
            entry_point: ffi::StringView::from_static("fs_main"),
            constant_count: 0,
            constants: ptr::null(),
            target_count: targets.len(),
            targets: targets.as_ptr(),
        };
        let desc = ffi::RenderPipelineDescriptor {
            next_in_chain: ptr::null_mut(),
            label: ffi::StringView::from_static("nes-sprite-pipeline"),
            layout: pipeline_layout,
            vertex,
            primitive: ffi::PrimitiveState {
                next_in_chain: ptr::null_mut(),
                topology: ffi::WGPU_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST,
                strip_index_format: ffi::WGPU_INDEX_FORMAT_UNDEFINED,
                front_face: ffi::WGPU_FRONT_FACE_CCW,
                cull_mode: ffi::WGPU_CULL_MODE_NONE,
                unclipped_depth: 0,
            },
            depth_stencil: ptr::null(),
            multisample: ffi::MultisampleState {
                next_in_chain: ptr::null_mut(),
                count: 1,
                mask: u32::MAX,
                alpha_to_coverage_enabled: 0,
            },
            fragment: &fragment_state,
        };
        // SAFETY: 各描述符与数组在本次调用期间存活。
        let pipeline = unsafe { (api.device_create_render_pipeline)(device, &desc) };
        if pipeline.is_null() {
            unsafe {
                (api.pipeline_layout_release)(pipeline_layout);
                (api.buffer_release)(instance_buffer);
                (api.shader_module_release)(module);
            }
            return Err(BackendError::NullHandle("WGPURenderPipeline(sprite)"));
        }

        Ok(Self {
            module,
            pipeline,
            pipeline_layout,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            view_uniform: atlas.view_uniform(),
            bind_group: atlas.handles().bind_group,
            staging: Vec::new(),
            ops: PipelineOps {
                pipeline_layout_release: api.pipeline_layout_release,
                render_pipeline_release: api.render_pipeline_release,
                shader_module_release: api.shader_module_release,
                buffer_release: api.buffer_release,
            },
        })
    }

    /// 实例缓冲容量（精灵数）。
    pub fn capacity(&self) -> u32 {
        self.instance_capacity
    }

    /// 容量不足时倍增重建实例缓冲（旧缓冲立即释放，不存在双活窗口）。
    fn ensure_capacity(&mut self, ctx: &GpuContext, wanted: u32) -> Result<(), BackendError> {
        if wanted <= self.instance_capacity {
            return Ok(());
        }
        let new_capacity = wanted.max(self.instance_capacity.saturating_mul(2));
        let grown = create_buffer(
            ctx,
            "nes-sprite-instances",
            ffi::WGPU_BUFFER_USAGE_VERTEX | ffi::WGPU_BUFFER_USAGE_COPY_DST,
            INSTANCE_STRIDE * new_capacity as u64,
        )?;
        // SAFETY: 旧缓冲仅本管线持有，替换前释放一次。
        unsafe { (self.ops.buffer_release)(self.instance_buffer) };
        self.instance_buffer = grown;
        self.instance_capacity = new_capacity;
        Ok(())
    }

    /// 渲染一帧：清屏 + 按传入顺序绘制精灵列表。
    ///
    /// `view` 是 8 个 `f32`：`[a, b, c, d, tx, ty, viewport_w, viewport_h]`
    /// （视图矩阵来自契约的 `Camera2DState::view_matrix`，本层不另行推导）。
    /// `registry` 提供第二绑定组（group 1），按实例数据在内建图集与注册表
    /// 之间选择采样来源。无精灵时仍执行清屏与存储 —— "空帧也要有合法像素"
    /// 是读回的前提。
    pub fn render(
        &mut self,
        ctx: &GpuContext,
        target_view: *mut c_void,
        registry: &gpu::TextureRegistry,
        view: &[f32; 8],
        sprites: &[SpriteInstance],
    ) -> Result<(), BackendError> {
        self.ensure_capacity(ctx, sprites.len() as u32)?;
        let api = ctx.api();

        // 1) 上传统一缓冲（视图矩阵 + 视口；缓冲是图集装配的那只，binding 0 直连）。
        // SAFETY: `view` 在本次调用期间存活，长度恰为 VIEW_UNIFORM_BYTES。
        unsafe {
            (api.queue_write_buffer)(
                ctx.queue(),
                self.view_uniform,
                0,
                view.as_ptr().cast::<c_void>(),
                VIEW_UNIFORM_BYTES as usize,
            );
        }

        // 2) 打包并上传实例数据（staging 跨帧复用，稳态零分配）。
        self.staging.clear();
        for sprite in sprites {
            for f in sprite
                .world
                .iter()
                .chain(sprite.uv_rect.iter())
                .chain(sprite.source.iter())
            {
                self.staging.extend_from_slice(&f.to_ne_bytes());
            }
        }
        if !self.staging.is_empty() {
            // SAFETY: staging 在本次调用期间存活且长度为实例数 * 跨度。
            unsafe {
                (api.queue_write_buffer)(
                    ctx.queue(),
                    self.instance_buffer,
                    0,
                    self.staging.as_ptr().cast::<c_void>(),
                    self.staging.len(),
                );
            }
        }

        // 3) 渲染通道：清屏 -> 管线 -> 绑定组 -> 实例缓冲 -> 绘制 -> 结束。
        let attachment = ffi::ColorAttachment {
            next_in_chain: ptr::null_mut(),
            view: target_view,
            depth_slice: ffi::WGPU_DEPTH_SLICE_UNDEFINED,
            resolve_target: ptr::null_mut(),
            load_op: ffi::WGPU_LOAD_OP_CLEAR,
            store_op: ffi::WGPU_STORE_OP_STORE,
            clear_value: CLEAR_COLOR,
        };
        let attachments = [attachment];
        let pass_desc = ffi::RenderPassDescriptor {
            next_in_chain: ptr::null_mut(),
            label: ffi::StringView::from_static("nes-sprite-pass"),
            color_attachment_count: attachments.len(),
            color_attachments: attachments.as_ptr(),
            depth_stencil_attachment: ptr::null(),
            occlusion_query_set: ptr::null_mut(),
            timestamp_writes: ptr::null(),
        };
        // SAFETY: 描述符在本次调用期间存活；encoder 归本函数独有。
        let encoder = unsafe { (api.device_create_command_encoder)(ctx.device(), ptr::null()) };
        if encoder.is_null() {
            return Err(BackendError::NullHandle("WGPUCommandEncoder(sprite)"));
        }
        // SAFETY: pass_desc 与 attachment 在本次调用期间存活。
        let pass =
            unsafe { (api.command_encoder_begin_render_pass)(encoder, &pass_desc) };
        if pass.is_null() {
            unsafe { (api.command_encoder_release)(encoder) };
            return Err(BackendError::NullHandle("WGPURenderPassEncoder(sprite)"));
        }
        // SAFETY: pass 由上一行返回且非空；本块内不再并发使用 encoder。
        unsafe {
            (api.render_pass_encoder_set_pipeline)(pass, self.pipeline);
            (api.render_pass_encoder_set_bind_group)(pass, 0, self.bind_group, 0, ptr::null());
            (api.render_pass_encoder_set_bind_group)(
                pass,
                1,
                registry.bind_group(),
                0,
                ptr::null(),
            );
            if !sprites.is_empty() {
                (api.render_pass_encoder_set_vertex_buffer)(
                    pass,
                    0,
                    self.instance_buffer,
                    0,
                    self.staging.len() as u64,
                );
                (api.render_pass_encoder_draw)(pass, 6, sprites.len() as u32, 0, 0);
            }
            (api.render_pass_encoder_end)(pass);
            (api.render_pass_encoder_release)(pass);
        }
        // SAFETY: 编码已完成，finish 消费 encoder。
        let command_buffer = unsafe { (api.command_encoder_finish)(encoder, ptr::null()) };
        unsafe { (api.command_encoder_release)(encoder) };
        if command_buffer.is_null() {
            return Err(BackendError::NullHandle("WGPUCommandBuffer(sprite)"));
        }
        // SAFETY: 单条命令缓冲，提交后立即释放（队列已持有引用）。
        unsafe {
            (api.queue_submit)(ctx.queue(), 1, &command_buffer);
            (api.command_buffer_release)(command_buffer);
        }
        Ok(())
    }
}

impl Drop for SpritePipeline {
    fn drop(&mut self) {
        // SAFETY: 每个句柄只在非空时释放一次，顺序与创建相反。
        // 图集的视图缓冲与绑定组是借用句柄，不在此释放（由 SpriteAtlas 的 Drop 负责）。
        unsafe {
            if !self.pipeline.is_null() {
                (self.ops.render_pipeline_release)(self.pipeline);
                self.pipeline = ptr::null_mut();
            }
            if !self.pipeline_layout.is_null() {
                (self.ops.pipeline_layout_release)(self.pipeline_layout);
                self.pipeline_layout = ptr::null_mut();
            }
            if !self.instance_buffer.is_null() {
                (self.ops.buffer_release)(self.instance_buffer);
                self.instance_buffer = ptr::null_mut();
            }
            if !self.module.is_null() {
                (self.ops.shader_module_release)(self.module);
                self.module = ptr::null_mut();
            }
        }
    }
}

/// 建一个缓冲（标签 + 用法 + 尺寸，失败指名道姓）。
fn create_buffer(
    ctx: &GpuContext,
    label: &'static str,
    usage: u64,
    size: u64,
) -> Result<*mut c_void, BackendError> {
    let mut desc = ffi::BufferDescriptor {
        usage,
        size,
        ..Default::default()
    };
    desc.label = ffi::StringView::from_static(label);
    // SAFETY: 描述符在本次调用期间存活。
    let buffer = unsafe { (ctx.api().device_create_buffer)(ctx.device(), &desc) };
    if buffer.is_null() {
        return Err(BackendError::NullHandle("WGPUBuffer(sprite-pipeline)"));
    }
    Ok(buffer)
}

// ------------------------------------------------------------ CommandConsumer

/// 命令流消费器：GPU 部件的组装点，逐条执行 [`RenderCommand`] 并产出一帧。
///
/// 用法（S4.1 离屏闭环）：
///
/// ```text
/// let mut server = WgpuRenderServer::new();          // 场景侧推属性
/// let mut consumer = CommandConsumer::open()?;        // GPU 装配（或 new(ctx, target, atlas)）
/// let mut buffer = Vec::new();
/// server.submit_into(&frame, &mut buffer);
/// let outcome = consumer.consume(&buffer)?;           // 清屏 + 精灵 -> 读回
/// outcome.write_png(path)?;                           // 可视证据落盘
/// ```
pub struct CommandConsumer {
    /// 跨帧条目登记表（`CreateItem` 建、`DestroyItem` 删；属性命令按句柄更新）。
    /// 契约 I4：生命周期事件一次性落缓冲，没有这张表，第二帧起属性就无人认领。
    items: BTreeMap<ItemHandle, RenderItem>,
    /// 跨帧控件布局登记表（`SetRect` 建、`DestroyItem` 删）。有 `SetRect` 状态的
    /// 渲染物按 HUD 口径画成控件边框（S4.2，见模块文档）。
    rects: BTreeMap<ItemHandle, ControlState>,
    /// 跨帧文本状态登记表（`SetText` 建、`DestroyItem` 删）。有 `SetText` 状态的
    /// 渲染物按字形表展开成文本（S4.4，见模块文档）。
    texts: BTreeMap<ItemHandle, LabelState>,
    /// 字体登记表：资源键 -> 排版参数（字形表本体作为纹理住在注册表里）。
    /// 默认字体住在保留键 [`DEFAULT_FONT_KEY`] 下；`LabelState.font` 按键解析，
    /// 未登记的键与 `NIL` 一样退回默认字体（S4.5 契约口径，T-Text-07/08 钉住）。
    fonts: BTreeMap<RenderAssetKey, FontEntry>,
    // 字段声明序 = 析构序（Rust 逐字段按声明序 drop）：管线 -> 图集 -> 注册表 ->
    // 目标 -> 上下文，与创建序相反。动态库按进程生命周期持有（见
    // ffi::NativeLib 的 Drop 说明），顺序不再是"函数指针失效"意义上的硬约束，
    // 但保持"子件先于上下文释放"仍与资源所属关系一致。
    pipeline: SpritePipeline,
    atlas: SpriteAtlas,
    registry: gpu::TextureRegistry,
    target: RenderTarget,
    ctx: GpuContext,
}

/// 字体登记项（默认字体与自定义字体同构：字形表纹理 + 排版参数）。
#[derive(Copy, Clone)]
struct FontEntry {
    /// 单字格尺寸（像素）。
    cell: (f32, f32),
    /// 字形表每行列数。
    cols: u32,
    /// 首字符码点（通常 32）。
    first_char: u32,
    /// 覆盖的字符数。
    count: u32,
    /// 字距（像素，等宽口径）。
    advance: f32,
    /// 基准行高（像素；实际行高 = 基准 + `LabelState::line_spacing`）。
    line_height: f32,
}

/// 默认字体表占用的保留资源键（slot = `u32::MAX`，位编码刻意远离场景侧资产槽位；
/// 该键在注册表里指向字形表纹理）。
const DEFAULT_FONT_KEY: RenderAssetKey = RenderAssetKey::from_parts(u32::MAX, 1);

/// 默认字体的登记参数（[`CommandConsumer::set_default_font`] 用）。
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FontParams {
    /// 字形表位图宽（像素）。
    pub width: u32,
    /// 字形表位图高（像素）。
    pub height: u32,
    /// 单字格宽（像素）。
    pub cell_w: u32,
    /// 单字格高（像素）。
    pub cell_h: u32,
    /// 字形表每行列数。
    pub cols: u32,
    /// 首字符码点（通常 32）。
    pub first_char: u32,
    /// 覆盖的字符数。
    pub count: u32,
    /// 字距（像素，等宽口径，须为正有限值）。
    pub advance: f32,
    /// 基准行高（像素，须为正有限值；实际行高 = 基准 + `line_spacing`）。
    pub line_height: f32,
}

impl CommandConsumer {
    /// 装配默认消费器（默认 64x64 离屏目标 + 内建精灵图集）。
    pub fn open() -> Result<Self, BackendError> {
        let ctx = GpuContext::open()?;
        let target = RenderTarget::new(&ctx)?;
        let atlas = SpriteAtlas::new(&ctx)?;
        Self::new(ctx, target, atlas)
    }

    /// 用已装配的 GPU 部件构造消费器（纹理注册表与精灵管线在此创建）。
    pub fn new(
        ctx: GpuContext,
        target: RenderTarget,
        atlas: SpriteAtlas,
    ) -> Result<Self, BackendError> {
        let registry = gpu::TextureRegistry::new(&ctx)?;
        let pipeline = SpritePipeline::new(&ctx, &atlas, &registry)?;
        Ok(Self {
            items: BTreeMap::new(),
            rects: BTreeMap::new(),
            texts: BTreeMap::new(),
            fonts: BTreeMap::new(),
            pipeline,
            atlas,
            registry,
            target,
            ctx,
        })
    }

    /// GPU 上下文（适配器身份、库路径等诊断信息从这里取）。
    pub fn ctx(&self) -> &GpuContext {
        &self.ctx
    }

    /// 离屏渲染目标。
    pub fn target(&self) -> &RenderTarget {
        &self.target
    }

    /// 精灵图集。
    pub fn atlas(&self) -> &SpriteAtlas {
        &self.atlas
    }

    /// 纹理注册表（图层查询、容量诊断）。
    pub fn registry(&self) -> &gpu::TextureRegistry {
        &self.registry
    }

    /// 注册（或覆写）一张纹理到注册表，返回分到的图层号。
    ///
    /// 之后任何以 `key` 为资源键的精灵在绘制时采样该纹理（注册表的 2D 数组
    /// 图层），而不是内建图集格。同键重复注册覆写原图层（图层号不变，热重载
    /// 语义）。尺寸超限或字节数不符时如实报错（见 [`gpu::TextureRegistry::register`]）。
    pub fn register_texture(
        &mut self,
        key: RenderAssetKey,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<u32, BackendError> {
        self.registry
            .register(&self.ctx, key, width, height, rgba)
    }

    /// 设置默认字体（`LabelState.font == NIL` 或指向未登记键时使用），返回瓦片号。
    ///
    /// `rgba` 是字形表位图（每字符一格，按行主序从 `first_char` 排到
    /// `first_char + count - 1`；空格等留空白即可）。字形栅格化由外部烘焙工具
    /// 完成（本 crate 零依赖），排版参数在 [`FontParams`] 里登记。
    /// 返回后 `SetText` 的文本开始产生像素。
    pub fn set_default_font(
        &mut self,
        params: FontParams,
        rgba: &[u8],
    ) -> Result<u32, BackendError> {
        self.register_font(DEFAULT_FONT_KEY, params, rgba)
    }

    /// 登记自定义字体（`LabelState.font` 指向该键时使用），返回瓦片号。
    ///
    /// `key` 必须非 `NIL`（NIL 保留给"用默认字体"语义）；同键重复登记 = 覆写
    /// （热重载语义）。字形表与排版参数同 [`FontParams`] 口径。
    pub fn set_custom_font(
        &mut self,
        key: RenderAssetKey,
        params: FontParams,
        rgba: &[u8],
    ) -> Result<u32, BackendError> {
        if key.is_nil() {
            return Err(BackendError::ConfigMismatch(
                "自定义字体键不能是 NIL（NIL 保留给默认字体语义）".to_string(),
            ));
        }
        self.register_font(key, params, rgba)
    }

    /// 字体登记的共用实现：参数自检 -> 字形表注册进注册表 -> 排版参数入表。
    fn register_font(
        &mut self,
        key: RenderAssetKey,
        params: FontParams,
        rgba: &[u8],
    ) -> Result<u32, BackendError> {
        let FontParams {
            width,
            height,
            cell_w,
            cell_h,
            cols,
            first_char,
            count,
            advance,
            line_height,
        } = params;
        if cell_w == 0 || cell_h == 0 || cols == 0 || count == 0 {
            return Err(BackendError::ConfigMismatch(
                "字形表参数自相矛盾：字格/列数/字符数不能为 0".to_string(),
            ));
        }
        if (cell_w * cols) > width || (cell_h * count.div_ceil(cols)) > height {
            return Err(BackendError::ConfigMismatch(format!(
                "字形表 {width}x{height} 容不下 {count} 个 {cell_w}x{cell_h} 字格（每行 {cols} 格）"
            )));
        }
        if !(advance.is_finite() && advance > 0.0 && line_height.is_finite() && line_height > 0.0)
        {
            return Err(BackendError::ConfigMismatch(
                "字距与行高必须是正的有限值".to_string(),
            ));
        }
        let tile = self
            .registry
            .register(&self.ctx, key, width, height, rgba)?;
        self.fonts.insert(
            key,
            FontEntry {
                cell: (cell_w as f32, cell_h as f32),
                cols,
                first_char,
                count,
                advance,
                line_height,
            },
        );
        Ok(tile)
    }

    /// 精灵管线。
    pub fn pipeline(&self) -> &SpritePipeline {
        &self.pipeline
    }

    /// 驱动侧未捕获错误快照（`FrameStats::driver_errors` 的原文出处）。
    pub fn errors_snapshot(&self) -> Vec<String> {
        self.ctx.errors_snapshot()
    }

    /// 消费一整条命令流并产出一帧（渲染 + 读回）。
    ///
    /// 命令流必须以 [`RenderCommand::Submit`] 结尾（契约 I3），否则
    /// [`BackendError::MalformedCommandStream`]。空句柄 / 未知句柄的命令按
    /// 契约 I1 静默忽略（计入 `stats.ignored`，不中断本帧）。条目表跨帧持有：
    /// `CreateItem` / `DestroyItem` 一次性登记/注销，属性命令更新已有条目。
    pub fn consume(&mut self, commands: &[RenderCommand]) -> Result<FrameOutcome, BackendError> {
        let frame = stream_terminator(commands)?;
        let (w, h) = self.target.size();
        let mut stats = self.draw_into(commands, self.target.view, (w, h))?;
        stats.frame_index = frame.frame_index;

        let errors_before = self.ctx.errors_len();
        let image = self.target.read_back(&self.ctx)?;
        stats.driver_errors = self.ctx.errors_len().saturating_sub(errors_before) as u64;
        Ok(FrameOutcome { image, stats })
    }

    /// 把一整条命令流绘制到一个渲染目标视图上（离屏与表面路径共用）。
    ///
    /// `viewport_size` 是相机缺位/禁用时的回退视口（像素）。条目表跨帧持有，
    /// 空格/表外字符、控件 HUD 口径、注册表选源等语义与 [`Self::consume`] 完全一致。
    fn draw_into(
        &mut self,
        commands: &[RenderCommand],
        target_view: *mut c_void,
        viewport_size: (u32, u32),
    ) -> Result<FrameStats, BackendError> {
        let _ = stream_terminator(commands)?;
        let mut stats = FrameStats::default();
        let mut camera: Option<Camera2DState> = None;
        let errors_before = self.ctx.errors_len();

        for command in commands {
            stats.commands += 1;
            match command {
                RenderCommand::CreateItem { handle, key } => {
                    self.items
                        .insert(*handle, RenderItem::new(*handle, *key, Affine2::IDENTITY));
                    stats.creates += 1;
                }
                RenderCommand::DestroyItem { handle } => {
                    if self.items.remove(handle).is_some() {
                        self.rects.remove(handle);
                        self.texts.remove(handle);
                        stats.destroys += 1;
                    } else {
                        stats.ignored += 1;
                    }
                }
                RenderCommand::SetCamera { camera: c } => camera = Some(*c),
                RenderCommand::SetVisible { handle, visible } => {
                    match self.items.get_mut(handle) {
                        Some(item) => {
                            item.visible = *visible;
                            stats.updates += 1;
                        }
                        None => stats.ignored += 1,
                    }
                }
                RenderCommand::SetTransform { handle, transform } => {
                    match self.items.get_mut(handle) {
                        Some(item) => {
                            item.transform = *transform;
                            stats.updates += 1;
                        }
                        None => stats.ignored += 1,
                    }
                }
                RenderCommand::SetZ { handle, z, order } => match self.items.get_mut(handle) {
                    Some(item) => {
                        item.z = *z;
                        item.order = *order;
                        stats.updates += 1;
                    }
                    None => stats.ignored += 1,
                },
                RenderCommand::SetFlip { handle, flip } => match self.items.get_mut(handle) {
                    Some(item) => {
                        item.flip = *flip;
                        stats.updates += 1;
                    }
                    None => stats.ignored += 1,
                },
                // SetRect：登记布局状态，绘制阶段按 HUD 口径画成控件边框。
                // SetText：登记文本状态，绘制阶段按字形表展开成文本（S4.4）。
                RenderCommand::SetRect { handle, rect } => {
                    if self.items.contains_key(handle) {
                        // 有 SetRect 状态的渲染物按控件对待：纹理键不再必要
                        //（控件画固定的边框图案格）。
                        self.rects.insert(*handle, *rect);
                        stats.updates += 1;
                    } else {
                        stats.ignored += 1;
                    }
                }
                RenderCommand::SetText { handle, text } => {
                    if self.items.contains_key(handle) {
                        // 有 SetText 状态的渲染物按文本对待（同理不依赖纹理键）。
                        self.texts.insert(*handle, text.clone());
                        stats.updates += 1;
                    } else {
                        stats.ignored += 1;
                    }
                }
                RenderCommand::Submit { .. } => {} // 终止标记，已在入口校验。
            }
        }

        // 绘制列表：可见，且（精灵：资源键已绑定 / 控件：有 SetRect / 文本：有
        // SetText），按 DrawKey 升序（契约 I5）。
        let mut draw_list: Vec<&RenderItem> = self
            .items
            .values()
            .filter(|item| {
                item.visible
                    && (!item.key.is_nil()
                        || self.rects.contains_key(&item.handle)
                        || self.texts.contains_key(&item.handle))
            })
            .collect();
        draw_list.sort_by_key(|item| item.draw_key());
        stats.skipped = (self.items.len() - draw_list.len()) as u64;

        // 相机 -> 视图矩阵（契约 I9：视图矩阵的唯一权威出口，本层不另行推导）。
        // 相机缺位 / 禁用 / 视口非法时退回单位视图 + 目标尺寸（帧本地，无历史可退）。
        let target_size = viewport_size;
        let (view, viewport) = match camera {
            Some(c) if c.enabled && c.viewport.x > 0.0 && c.viewport.y > 0.0 => {
                stats.camera_applied = true;
                (c.view_matrix().unwrap_or_default(), c.viewport)
            }
            _ => (
                Affine2::IDENTITY,
                nes_render_api::math::Vec2::new(target_size.0 as f32, target_size.1 as f32),
            ),
        };
        let view_params = [
            view.a, view.b, view.c, view.d, view.tx, view.ty, viewport.x, viewport.y,
        ];

        // 世界矩阵 = transform ∘ flip（契约 I8），采样格由资源键确定性决定。
        // 控件（有 SetRect 状态）另走 HUD 口径：锚点以视口为父尺寸解析出矩形，
        // 再经视图矩阵的**逆**折回世界空间 —— 期望的视口空间变换 V∘M 恰等于
        // 目标矩形变换，于是控件与精灵共用同一次绘制、同一个 uniform 视图。
        // 视图退化（不可逆，理论上线性部分行列式为 zoom²，正常不会发生）时跳过控件。
        let inv_view = view.inverse();
        let cell_uv = gpu::CELL_PX as f32 / gpu::ATLAS_PX as f32;
        let control_uv = [
            cell_uv * (gpu::CONTROL_CELL % gpu::ATLAS_CELLS) as f32,
            cell_uv * (gpu::CONTROL_CELL / gpu::ATLAS_CELLS) as f32,
            cell_uv,
            cell_uv,
        ];
        let mut sprites: Vec<SpriteInstance> = Vec::with_capacity(draw_list.len());
        for item in draw_list {
            if let (Some(rect_state), Some(inv)) = (self.rects.get(&item.handle), inv_view) {
                let rect = rect_state.resolve(viewport);
                let quad = Affine2::translation(rect.x, rect.y).mul(&Affine2::scale(
                    rect.w / gpu::CELL_PX as f32,
                    rect.h / gpu::CELL_PX as f32,
                ));
                sprites.push(SpriteInstance {
                    handle: item.handle,
                    world: inv.mul(&quad).to_array(),
                    uv_rect: control_uv,
                    source: [0.0, 0.0],
                });
                stats.controls += 1;
            } else if let Some(label) = self.texts.get(&item.handle) {
                // 文本（S4.4/S4.5）：世界变换 = 笔起点（首行首字格左上角），每字形
                // 一个四边形，采样字形表对应字格。字距恒定（等宽口径）、
                // 行高 = 基准 + line_spacing；空格与表外字符只推进笔位不画。
                // 字体解析（T-Text-07/08 口径）：`font == NIL` 或指向未登记键
                // -> 默认字体；指向已登记键 -> 该字体。两种都拿不到时不画。
                let resolved_font = if label.font.is_nil() {
                    self.fonts.get(&DEFAULT_FONT_KEY).map(|e| (DEFAULT_FONT_KEY, *e))
                } else {
                    self.fonts
                        .get(&label.font)
                        .map(|e| (label.font, *e))
                        .or_else(|| self.fonts.get(&DEFAULT_FONT_KEY).map(|e| (DEFAULT_FONT_KEY, *e)))
                };
                if let Some((font_key, font)) = resolved_font {
                    if let Some((tile, sheet_uv)) = self.registry.sample_info(font_key) {
                        let world = item.world_transform();
                        let rows = font.count.div_ceil(font.cols);
                        let cell_us = sheet_uv[2] / font.cols as f32;
                        let cell_vs = sheet_uv[3] / rows as f32;
                        for (line_index, line) in label.text.split('\n').enumerate() {
                            let line_y =
                                line_index as f32 * (font.line_height + label.line_spacing);
                            for (char_index, ch) in line.chars().enumerate() {
                                // 空格无墨，只推进笔位（笔位由 char_index 决定，跳过不影响后续落字）。
                                let code = ch as u32;
                                if ch == ' '
                                    || !(font.first_char..font.first_char + font.count)
                                        .contains(&code)
                                {
                                    continue;
                                }
                                let index = code - font.first_char;
                                let col = index % font.cols;
                                let row = index / font.cols;
                                let pen_x = char_index as f32 * font.advance;
                                let quad = world
                                    .mul(&Affine2::translation(pen_x, line_y))
                                    .mul(&Affine2::scale(
                                        font.cell.0 / gpu::CELL_PX as f32,
                                        font.cell.1 / gpu::CELL_PX as f32,
                                    ));
                                sprites.push(SpriteInstance {
                                    handle: item.handle,
                                    world: quad.to_array(),
                                    uv_rect: [
                                        sheet_uv[0] + col as f32 * cell_us,
                                        sheet_uv[1] + row as f32 * cell_vs,
                                        cell_us,
                                        cell_vs,
                                    ],
                                    source: [tile as f32, 1.0],
                                });
                                stats.glyphs += 1;
                            }
                        }
                    }
                }
            } else if let Some((layer, uv_rect)) = self.registry.sample_info(item.key) {
                // 键已注册：采样注册表图层（纹理落在图层左上角，UV 按实际尺寸裁剪）。
                sprites.push(SpriteInstance {
                    handle: item.handle,
                    world: item.world_transform().to_array(),
                    uv_rect,
                    source: [layer as f32, 1.0],
                });
                stats.from_registry += 1;
            } else if !self.rects.contains_key(&item.handle) {
                sprites.push(SpriteInstance {
                    handle: item.handle,
                    world: item.world_transform().to_array(),
                    uv_rect: cell_uv_rect(item.key),
                    source: [0.0, 0.0],
                });
            }
        }

        self.pipeline
            .render(&self.ctx, target_view, &self.registry, &view_params, &sprites)?;
        stats.drawn = sprites.len() as u64;
        stats.driver_errors = self.ctx.errors_len().saturating_sub(errors_before) as u64;
        Ok(stats)
    }

    /// 把一整条命令流绘制到**窗口表面**并呈现（S6.1）。
    ///
    /// 与 [`Self::consume`] 的分工：离屏路径渲染 + 读回（供断言/PNG）；
    /// 表面路径渲染 + present（供真窗口显示，不读回）。命令语义、条目表、
    /// 统计口径完全一致 —— "画到哪"是目标差异，"画什么"是同一套。
    pub fn consume_to_surface(
        &mut self,
        commands: &[RenderCommand],
        surface: &gpu::SurfaceTarget,
    ) -> Result<FrameStats, BackendError> {
        let frame = stream_terminator(commands)?;
        let (w, h) = surface.size();
        let surface_frame = surface.acquire(&self.ctx)?;
        let stats = self.draw_into(commands, surface_frame.view, (w, h))?;
        surface.present(&self.ctx)?;
        surface.release_frame(&self.ctx, surface_frame);
        let mut stats = stats;
        stats.frame_index = frame.frame_index;
        Ok(stats)
    }
}

/// 校验命令流以 `Submit` 结尾，并取出它携带的帧上下文。
fn stream_terminator(commands: &[RenderCommand]) -> Result<&FrameInfo, BackendError> {
    match commands.last() {
        Some(RenderCommand::Submit { frame }) => Ok(frame),
        _ => Err(BackendError::MalformedCommandStream(
            "命令流必须以 Submit 结尾（契约 I3）",
        )),
    }
}

// ------------------------------------------------------------ 测试（无 GPU 依赖）

#[cfg(test)]
mod tests {
    use super::*;
    use nes_render_api::math::Vec2;

    fn frame(index: u64) -> FrameInfo {
        FrameInfo::new(index, 0.0, 0.0, Vec2::new(64.0, 64.0))
    }

    #[test]
    fn server_stream_is_deterministic_terminated_and_ordered() {
        let mut server = WgpuRenderServer::new();
        let h1 = server.create_item(RenderAssetKey::from_parts(1, 1));
        let h2 = server.create_item(RenderAssetKey::from_parts(2, 1));
        server.set_z(h2, -1, 0); // h2 的 DrawKey 排到 h1 前面
        server.set_z(h1, 0, 0);
        server.set_flip(h1, Flip::new(true, false));
        server.set_visible(ItemHandle::NIL, false); // 空句柄：静默忽略
        server.set_transform(ItemHandle::from_raw(999), Affine2::IDENTITY); // 未知句柄：同上
        server.destroy_item(h2);

        let mut a = Vec::new();
        server.submit_into(&frame(7), &mut a);
        let mut b = Vec::new();
        server.submit_into(&frame(7), &mut b);
        let mut c = Vec::new();
        server.submit_into(&frame(7), &mut c);
        // 生命周期动作是一次性的（契约 I4）：只在事件后的第一次 submit 落缓冲。
        assert!(matches!(a[0], RenderCommand::CreateItem { .. }));
        assert!(matches!(a[1], RenderCommand::CreateItem { .. }));
        assert!(matches!(a[2], RenderCommand::DestroyItem { .. }));
        // 状态不变时，快照部分（含 Submit）逐条可重现。
        let lifecycle_len = a.len() - b.len();
        assert_eq!(&a[lifecycle_len..], &b[..], "第二帧起只剩全量快照");
        assert_eq!(b, c, "稳态下连续 submit 逐条相同（契约确定性）");

        assert!(
            matches!(a.last(), Some(RenderCommand::Submit { .. })),
            "末条必为 Submit（契约 I3）"
        );
        // 已销毁的渲染物不再出现在属性流里（快照段里不应有任何指向 h2 的命令）。
        assert!(b.iter().all(|c| c.handle() != Some(h2)));
        // 空句柄与未知句柄的操作没有产生任何命令。
        assert!(a.iter().all(|c| c.handle() != Some(ItemHandle::NIL)));
        assert!(a.iter().all(|c| c.handle() != Some(ItemHandle::from_raw(999))));
        assert_eq!(server.len(), 1);
        assert!(!server.is_empty());
        assert_eq!(server.draw_order(), vec![h1]);
    }

    #[test]
    fn server_handles_are_never_reused() {
        let mut server = WgpuRenderServer::new();
        let h1 = server.create_item(RenderAssetKey::NIL);
        server.destroy_item(h1);
        let h2 = server.create_item(RenderAssetKey::NIL);
        assert_ne!(h1, h2, "销毁后的句柄值不得再分配（契约 I2）");
    }

    #[test]
    fn malformed_stream_without_submit_is_rejected() {
        let mut server = WgpuRenderServer::new();
        let handle = server.create_item(RenderAssetKey::from_parts(1, 1));
        assert!(matches!(
            stream_terminator(&[]),
            Err(BackendError::MalformedCommandStream(_))
        ));
        let no_submit = vec![RenderCommand::SetVisible {
            handle,
            visible: true,
        }];
        assert!(matches!(
            stream_terminator(&no_submit),
            Err(BackendError::MalformedCommandStream(_))
        ));
        let ok = server.submit(&frame(3));
        assert_eq!(stream_terminator(&ok).unwrap().frame_index, 3);
    }

    #[test]
    fn frame_outcome_writes_png() {
        // 2x2 RGBA 存储的四个像素；write_png 直接透传给编码器。
        let rgba: [[u8; 4]; 4] = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
        ];
        let mut flat = Vec::new();
        for px in rgba {
            flat.extend_from_slice(&px);
        }
        let outcome = FrameOutcome {
            image: FrameImage {
                width: 2,
                height: 2,
                rgba: flat,
                bytes_per_row: 8,
            },
            stats: FrameStats::default(),
        };
        let path = std::env::temp_dir().join("nes_render_wgpu_frame_outcome_test.png");
        outcome.write_png(&path).expect("PNG 落盘");
        let bytes = std::fs::read(&path).expect("PNG 回读");
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "必须是合法 PNG 签名"
        );
        let _ = std::fs::remove_file(&path);
    }
}
