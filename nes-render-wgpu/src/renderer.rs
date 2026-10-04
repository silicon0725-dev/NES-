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
//! - `SetText` 的 **TTF 默认字体路径（S12-11 第 2 期）**：[`CommandConsumer::
//!   set_ttf_default`] 装载真字体后，`font == NIL` 的文本改走动态字形图集
//!   排版 —— 按比例字宽逐字符步进、字号 clamp 8..128 任意缩放、CJK 可上屏、
//!   光标位随比例字宽（见 `push_ttf_label` 与 `glyph` 模块）。TTF 未装载时
//!   一切与位图路径逐位相同；显式 `font` 键（位图字形表）完全不受影响。
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
use nes_render_api::math::{Affine2, Rect};
use nes_render_api::server::RenderServer;
use nes_render_api::state::{Camera2DState, ControlState, Flip, LabelState, ListAxis, ListState};

use crate::error::BackendError;
use crate::ffi;
use crate::glyph::{page_key, GlyphAtlas, GlyphSlot, GLYPH_PAGE_PX};
use crate::gpu::{self, FrameImage, GpuContext, RenderTarget, SpriteAtlas};
use crate::png::write_rgba8_png;
use crate::ttf::TtfFont;

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

/// 单个精灵实例的字节跨度（20 个 `f32`：2x3 世界矩阵 + UV 矩形 + 采样来源 +
/// 着色 RGBA（E-1 颜色通道，S12.1）+ 视口空间裁剪矩形（E-2 裁剪契约，S12-3））。
///
/// 与 `gpu::Sizes` 系（图集侧声明的视图缓冲尺寸）同一纪律：布局声明与
/// CPU 打包必须同步，扩字段时两处一起改。
const INSTANCE_STRIDE: u64 = 20 * core::mem::size_of::<f32>() as u64;

/// 初始实例容量（64 个精灵 = 2 KiB；不足时按需倍增重建缓冲）。
const INITIAL_INSTANCE_CAPACITY: u32 = 64;

/// 精灵着色器（WGSL）。
///
/// 几何由 `vertex_index` 生成（两个三角形拼一个四边形，**不需要顶点缓冲存几何**），
/// 每精灵数据走实例步进的 20 个 `f32`。着色器里的字面量 `4.0` 是图集每边格数
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
// 每精灵实例数据（实例步进顶点缓冲，20 个 f32 = 80 字节）：
//   loc0..2 = 世界矩阵三列（已含 flip 的子局部后乘）；
//   loc3    = UV 矩形 (u0, v0, us, vs)；
//   loc4    = 采样来源 (瓦片号, 类型)：类型 0 = 内建图集、1 = 注册表瓦片
//             （注册表是单张平铺大纹理，位置全在 UV 矩形里；瓦片号仅作
//             实例侧留档，着色器当前不读它）；
//   loc5    = 着色 RGBA（直 alpha，归一化 0..1；E-1 —— 采样色 x tint，
//             中性 [1,1,1,1] 与 E-1 之前逐位相同）；
//   loc6    = 视口空间裁剪矩形 (x, y, w, h)（E-2 / S12-3）：裁剪经 encoder
//             侧 scissor 执行，着色器**不读**它 —— 实例布局保持自洽，
//             便于实例侧对账与后续扩展。
struct SpriteData {
    @location(0) col0: vec2<f32>,
    @location(1) col1: vec2<f32>,
    @location(2) col2: vec2<f32>,
    @location(3) uv_rect: vec4<f32>,
    @location(4) source: vec2<f32>,
    @location(5) tint: vec4<f32>,
    @location(6) clip: vec4<f32>,
};
struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) corner: vec2<f32>,
    @location(1) uv_rect: vec4<f32>,
    @location(2) source: vec2<f32>,
    @location(3) tint: vec4<f32>,
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
    out.tint = data.tint;
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
    // E-1 着色：采样色 x tint（中性 tint = 恒等）。图集图案格是中性白，
    // 颜色一律经 tint 进入 —— 契约层 ControlState/LabelState 的颜色
    // 字段在 draw_into 侧折算成本属性。
    return color * in.tint;
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
    /// 列表/页签簿记（S12-3 任务 4，与 `NullRenderServer` 同构）。
    lists: BTreeMap<ItemHandle, ListState>,
    rects: BTreeMap<ItemHandle, ControlState>,
    /// 裁剪簿记（E-2 / D1，与 `NullRenderServer` 同构）：`Some(rect)` 存、
    /// `None`/销毁移除；`submit_into` 在对应条目的 `SetRect` 之后追加 `SetClip`。
    clips: BTreeMap<ItemHandle, Rect>,
    /// 相乘色簿记（S16.1 alpha 通道，与 `NullRenderServer` 同构）：同键覆写、
    /// 销毁移除；`submit_into` 在对应条目的 `SetClip` 之后追加 `SetTint`。
    tints: BTreeMap<ItemHandle, [u8; 4]>,
    /// 子矩形采样簿记（S16.2 图集帧动画，与 `NullRenderServer` 同构）：
    /// 同键覆写、销毁移除；`submit_into` 在对应条目的 `SetTint` 之后追加
    /// `SetUv`。无记录 = 整瓦片采样（既有行为逐位不变）。
    uvs: BTreeMap<ItemHandle, [f32; 4]>,
    /// 精灵锚点簿记（S16.3，与 `NullRenderServer` 同构）：同键覆写、
    /// 销毁移除；`submit_into` 在对应条目的 `SetUv` 之后追加 `SetPivot`。
    /// 无记录 = 无平移（既有行为逐位不变）。
    pivots: BTreeMap<ItemHandle, [f32; 2]>,
    /// 九宫格簿记（S16.6，与 `NullRenderServer` 同构）：同键覆写（NIL 键
    /// = 恒等记录照存照发）、销毁移除；`submit_into` 在对应条目的
    /// `SetPivot` 之后追加 `SetNineSlice`。无记录 = fill/border 照旧
    ///（既有行为逐位不变）。载荷 = `(源纹理键, [l, t, r, b])`（源纹理
    /// 像素边距）。
    nines: BTreeMap<ItemHandle, (RenderAssetKey, [f32; 4])>,
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
        self.lifecycle
            .push(RenderCommand::CreateItem { handle, key });
        handle
    }

    fn destroy_item(&mut self, handle: ItemHandle) {
        if handle.is_nil() || self.items.remove(&handle).is_none() {
            return;
        }
        self.labels.remove(&handle);
        self.lists.remove(&handle);
        self.rects.remove(&handle);
        self.clips.remove(&handle);
        self.tints.remove(&handle);
        self.uvs.remove(&handle);
        self.pivots.remove(&handle);
        self.nines.remove(&handle);
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

    fn set_list(&mut self, handle: ItemHandle, rows: &ListState) {
        if self.items.contains_key(&handle) {
            self.lists.insert(handle, rows.clone());
        }
    }

    fn set_rect(&mut self, handle: ItemHandle, rect: &ControlState) {
        if self.items.contains_key(&handle) {
            self.rects.insert(handle, *rect);
        }
    }

    fn set_clip(&mut self, handle: ItemHandle, rect: Option<Rect>) {
        if !self.items.contains_key(&handle) {
            // 空句柄 / 未知句柄：静默忽略（契约 I1 口径）。
            return;
        }
        match rect {
            Some(rect) => {
                self.clips.insert(handle, rect);
            }
            None => {
                // `None` = 清除裁剪（本来就没有也是合法的清除）。
                self.clips.remove(&handle);
            }
        }
    }

    fn set_tint(&mut self, handle: ItemHandle, rgba: [u8; 4]) {
        if !self.items.contains_key(&handle) {
            // 空句柄 / 未知句柄：静默忽略（契约 I1 口径）。
            return;
        }
        // 同键覆写（全量快照语义，S16.1 alpha 通道）。
        self.tints.insert(handle, rgba);
    }

    fn set_uv(&mut self, handle: ItemHandle, rect: [f32; 4]) {
        if !self.items.contains_key(&handle) {
            // 空句柄 / 未知句柄：静默忽略（契约 I1 口径）。
            return;
        }
        // 同键覆写（全量快照语义，S16.2 图集帧动画）。
        self.uvs.insert(handle, rect);
    }

    fn set_pivot(&mut self, handle: ItemHandle, pivot: [f32; 2]) {
        if !self.items.contains_key(&handle) {
            // 空句柄 / 未知句柄：静默忽略（契约 I1 口径）。
            return;
        }
        // 同键覆写（全量快照语义，S16.3 精灵锚点；`[0,0]` 也照存 ——
        // 零平移 = 恒等，迁移帧"补推清除"走的就是它）。
        self.pivots.insert(handle, pivot);
    }

    fn set_nine_slice(
        &mut self,
        handle: ItemHandle,
        texture: RenderAssetKey,
        l: f32,
        t: f32,
        r: f32,
        b: f32,
    ) {
        if !self.items.contains_key(&handle) {
            // 空句柄 / 未知句柄：静默忽略（契约 I1 口径）。
            return;
        }
        // 同键覆写（全量快照语义，S16.6 九宫格）。NIL 键**照存**（恒等
        // 记录 = fill/border 照旧，随每帧快照重发）—— 与 `NullRenderServer`
        // 同构：清除必须可在命令流里承载，跨帧簿记的消费端才收得到"清掉"
        // （照 pivot `[0,0]` 零向量先例）。
        self.nines.insert(handle, (texture, [l, t, r, b]));
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
            // 列表/页签（S12-3 任务 4）：输出序冻结 SetText → SetList →
            // SetRect → SetClip（与 `NullRenderServer` 严格同序）。
            if let Some(rows) = self.lists.get(&item.handle) {
                out.push(RenderCommand::SetList {
                    handle: item.handle,
                    rows: rows.clone(),
                });
            }
            if let Some(rect) = self.rects.get(&item.handle) {
                out.push(RenderCommand::SetRect {
                    handle: item.handle,
                    rect: *rect,
                });
            }
            // 裁剪恒在 SetRect 之后（E-2 / D1 推送序）；仅当该条目存在裁剪时追加。
            if let Some(clip) = self.clips.get(&item.handle) {
                out.push(RenderCommand::SetClip {
                    handle: item.handle,
                    rect: Some(*clip),
                });
            }
            // 相乘色（S16.1 alpha 通道）：恒在 SetClip 之后（契约 I5 顺序
            // 冻结；与 `NullRenderServer` 严格同序）。仅当该条目存在 tint
            // 簿记时追加。
            if let Some(rgba) = self.tints.get(&item.handle) {
                out.push(RenderCommand::SetTint {
                    handle: item.handle,
                    rgba: *rgba,
                });
            }
            // 子矩形采样（S16.2 图集帧动画）：恒在 SetTint 之后（契约 I5
            // 顺序冻结；与 `NullRenderServer` 严格同序）。仅当该条目存在
            // uv 簿记时追加 —— 无记录 = 整瓦片采样，命令流与既有路径
            // 逐条相同。
            if let Some(rect) = self.uvs.get(&item.handle) {
                out.push(RenderCommand::SetUv {
                    handle: item.handle,
                    rect: *rect,
                });
            }
            // 精灵锚点（S16.3）：恒在 SetUv 之后（契约 I5 顺序冻结；与
            // `NullRenderServer` 严格同序）。仅当该条目存在 pivot 簿记时
            // 追加 —— 无记录 = 无平移，命令流与既有路径逐条相同。
            if let Some(pivot) = self.pivots.get(&item.handle) {
                out.push(RenderCommand::SetPivot {
                    handle: item.handle,
                    pivot: *pivot,
                });
            }
            // 九宫格（S16.6）：恒在 SetPivot 之后（契约 I5 顺序冻结；与
            // `NullRenderServer` 严格同序）。仅当该条目存在九宫格簿记时
            // 追加 —— 无记录 = fill/border 照旧，命令流与既有路径逐条
            // 相同。
            if let Some((texture, margins)) = self.nines.get(&item.handle) {
                out.push(RenderCommand::SetNineSlice {
                    handle: item.handle,
                    texture: *texture,
                    l: margins[0],
                    t: margins[1],
                    r: margins[2],
                    b: margins[3],
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
    /// `SetVisible` / `SetText` / `SetList` / `SetRect` / `SetClip` /
    /// `SetTint` / `SetUv` / `SetPivot` / `SetNineSlice`）。
    pub updates: u64,
    /// 因空句柄 / 未知句柄被静默忽略的命令数（契约 I1 的可观测计数）。
    pub ignored: u64,
    /// 因不可见或资源键未绑定而跳过绘制的渲染物数。
    pub skipped: u64,
    /// 实际提交绘制的四边形数（精灵 + 控件 + 字形）。
    pub drawn: u64,
    /// 其中控件条目数（有 `SetRect` 状态、按 HUD 口径绘制的渲染物；
    /// S16.6 起含九宫格展开的面板 —— 每条目计 1，不按实例数计）。
    pub controls: u64,
    /// 其中从纹理注册表采样真实纹理的精灵数（不含字形；字形单列）。
    pub from_registry: u64,
    /// 其中字形四边形数（`SetText` / `SetList` 展开的文本像素）。
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

/// 一条精灵绘制记录（实例缓冲的单条数据，20 个 `f32`）。
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct SpriteInstance {
    /// 渲染物句柄（不进 GPU，仅供帧对账与诊断）。
    pub handle: ItemHandle,
    /// 世界矩阵（`transform ∘ flip`，契约 I8；有 SetPivot 簿记时再后乘
    /// 局部平移 `-pivot × 16px`，S16.3），展平为 `[a, b, c, d, tx, ty]`。
    pub world: [f32; 6],
    /// UV 矩形 `[u0, v0, us, vs]`：内建图集 = 采样格子矩形；
    /// 注册表纹理 = 大纹理瓦片坐标系里的子矩形（瓦片左上角 + 实际尺寸裁剪）。
    pub uv_rect: [f32; 4],
    /// 采样来源 `[瓦片号, 类型]`：类型 0 = 内建图集、1 = 注册表瓦片
    ///（瓦片号仅供帧对账留档，注册表位置信息已并入 UV 矩形）。
    pub source: [f32; 2],
    /// 着色 RGBA（直 alpha 归一化 0..1；E-1 颜色通道，S12.1）。
    /// 缺省 `[1,1,1,1]` 中性 —— 与 E-1 之前逐位相同。
    pub tint: [f32; 4],
    /// 视口空间裁剪矩形 `[x, y, w, h]`（E-2 裁剪契约，S12-3）。
    /// 缺省 [`Self::NO_CLIP`] 哨兵 = 无裁剪（输出与裁剪机制之前逐位相同）；
    /// 裁剪由 `render` 按"连续相同 clip 值分段 + encoder 侧 scissor"执行，
    /// 该值进实例缓冲是为保持实例布局自洽（着色器声明与对账可读）。
    pub clip: [f32; 4],
}

impl SpriteInstance {
    /// 无裁剪哨兵：视口空间 `[0,0,0,0]`，折算为"全目标 scissor"。
    pub const NO_CLIP: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

    /// RGBA8（直 alpha）-> 实例着色（归一化）。
    pub fn tint_of(rgba: [u8; 4]) -> [f32; 4] {
        [
            rgba[0] as f32 / 255.0,
            rgba[1] as f32 / 255.0,
            rgba[2] as f32 / 255.0,
            rgba[3] as f32 / 255.0,
        ]
    }
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

/// 发一行/一页签的实例（S12-3 任务 4；垂直轴与水平轴共用的发射子程序）：
///
/// 1. 选中填充条（`selected` 为真时）：铺在 `band` 处，tint = `sel_fill`
///    （选中行的观感差异来自这条填充条；字形若再用 sel_fill 会与条同色
///    隐形，故字形 tint 恒取 `text_color`）；
/// 2. 字形序列：等宽推进、空格与表外字符只推进笔位；`max_chars` 截断
///    （水平轴页签用 `(tab_w - 8) / 16`，垂直轴不截断传 `usize::MAX`）；
///    字格 UV 按纹理实际尺寸折算（与文本分支同一算式，S8.2 实证修复）。
#[allow(clippy::too_many_arguments)]
fn push_list_row(
    sprites: &mut Vec<SpriteInstance>,
    handle: ItemHandle,
    item_clip: [f32; 4],
    inv: &Affine2,
    fill_uv: [f32; 4],
    font: FontEntry,
    sheet_uv: [f32; 4],
    tile: u32,
    sel_fill: [u8; 4],
    text_color: [u8; 4],
    pen: (f32, f32),
    band: (f32, f32, f32, f32),
    selected: bool,
    line: &str,
    max_chars: usize,
    glyphs: &mut u64,
) {
    // ① 选中填充条（填充格 + sel_fill 着色；退化条不画）。
    if selected {
        let (bx, by, bw, bh) = band;
        if bw > 0.0 && bh > 0.0 {
            let quad = Affine2::translation(bx, by).mul(&Affine2::scale(
                bw / gpu::CELL_PX as f32,
                bh / gpu::CELL_PX as f32,
            ));
            sprites.push(SpriteInstance {
                handle,
                world: inv.mul(&quad).to_array(),
                uv_rect: fill_uv,
                source: [0.0, 0.0],
                tint: SpriteInstance::tint_of(sel_fill),
                clip: item_clip,
            });
        }
    }
    // ② 字形序列。
    let cell_us = sheet_uv[2] * font.cell.0 / font.tex.0;
    let cell_vs = sheet_uv[3] * font.cell.1 / font.tex.1;
    let text_tint = SpriteInstance::tint_of(text_color);
    let glyph_scale = Affine2::scale(
        font.cell.0 / gpu::CELL_PX as f32,
        font.cell.1 / gpu::CELL_PX as f32,
    );
    for (char_index, ch) in line.chars().take(max_chars).enumerate() {
        // 空格无墨，只推进笔位（笔位由 char_index 决定，跳过不影响后续落字）。
        let code = ch as u32;
        if ch == ' ' || !(font.first_char..font.first_char + font.count).contains(&code) {
            continue;
        }
        let index = code - font.first_char;
        let col = index % font.cols;
        let row = index / font.cols;
        let quad = inv
            .mul(&Affine2::translation(
                pen.0 + char_index as f32 * font.advance,
                pen.1,
            ))
            .mul(&glyph_scale);
        sprites.push(SpriteInstance {
            handle,
            world: quad.to_array(),
            uv_rect: [
                sheet_uv[0] + col as f32 * cell_us,
                sheet_uv[1] + row as f32 * cell_vs,
                cell_us,
                cell_vs,
            ],
            source: [tile as f32, 1.0],
            tint: text_tint,
            clip: item_clip,
        });
        *glyphs += 1;
    }
}

/// 九宫格展开（S16.6）：把源纹理按 3x3 切割铺进控件矩形 —— 每片一个
/// 实例（照 Label"一字形一实例"的展开先例）。
///
/// # 几何算式（冻结，单处实现）
///
/// - **边距钳制**（防负 / 防角重叠）：`实际边距 = min(声明边距, 控件边长 / 2)`
///   （声明负值按 0 处理）。推论：`中段宽 = w - l' - r' >= 0`、
///   `中段高 = h - t' - b' >= 0` 恒成立（`x/2` 与 `x/2 + x/2 = x` 在
///   IEEE 754 下精确）；零中段 = 合法退化（中带片 w/h <= 0，整片跳过）；
/// - 四角 1:1：目标尺寸 = 钳制后源边距（`l' x t'` 等）—— 任意缩放角
///   不变形；源子矩形**锚在纹理角上**（右/下角从纹理右/下缘回退 `r'/b'`
///   切割）：未钳制时与"左上顺序切"逐位同值，钳制退化（控件小于边距和）
///   时四角仍采到纹理真角（标准九宫格"角永远属于纹理角"口径）；
/// - 四边单向拉伸：上/下条 = 中段宽 x `t'/b'`（水平拉伸、垂直 1:1）；
///   左/右条 = `l'/r'` x 中段高（垂直拉伸、水平 1:1）；
/// - 中心双向拉伸：中段宽 x 中段高；
/// - 源子矩形 = `sample_info` 全瓦片 uv 的**分数内插**
///   `uv = 全瓦片.xy + 源px / 注册尺寸 x 全瓦片.wh`（注册尺寸经
///   [`gpu::TextureRegistry::texture_px_size`] 另取 —— sample_info 的
///   返回面只有分数，像素口径的分母在此单处折算）；
/// - `fill` / `border` 条带在九宫格模式**不画**（纹理自带边）—— 由调用
///   方分臂，本函数只发九片；tint 恒中性（面板色即纹理色，不经着色通道）。
#[allow(clippy::too_many_arguments)]
fn push_nine_slice(
    sprites: &mut Vec<SpriteInstance>,
    handle: ItemHandle,
    item_clip: [f32; 4],
    inv: &Affine2,
    rect: Rect,
    tile: u32,
    sheet: [f32; 4],
    tex_px: (f32, f32),
    margins: [f32; 4],
) {
    // 边距钳制：负值按 0、超过半边按半边（角不重叠、中段非负恒成立）。
    let l = margins[0].max(0.0).min(rect.w * 0.5);
    let t = margins[1].max(0.0).min(rect.h * 0.5);
    let r = margins[2].max(0.0).min(rect.w * 0.5);
    let b = margins[3].max(0.0).min(rect.h * 0.5);
    let cx = rect.w - l - r; // 中段宽（>= 0 由钳制保证）
    let cy = rect.h - t - b; // 中段高（同上）
                             // 源 uv 折算：全瓦片矩形是唯一参照，像素 -> 分数内插只有这一处。
    let (tw, th) = tex_px;
    let ux = |px: f32| sheet[0] + px / tw * sheet[2];
    let uy = |px: f32| sheet[1] + px / th * sheet[3];
    let uw = |px: f32| px / tw * sheet[2];
    let uh = |px: f32| px / th * sheet[3];
    // 源切割线**锚在纹理角上**：右/下切割线从纹理右/下缘回退钳制边距
    //（`tw - r` / `th - b`）。未钳制时与 `l + 中段` 重合（逐位同值）；
    // 钳制退化（30px 控件 < 32px 边距和）时四角仍采到纹理真角 —— 标准
    // 九宫格口径"角永远属于纹理角"。
    let (sx_mid, sy_mid) = (tw - r, th - b);
    let sw_edge = sx_mid - l; // 源上/下边条宽（未钳制时 = 中段宽）
    let sh_edge = sy_mid - t; // 源左/右边条高（未钳制时 = 中段高）
    // 九片 = (目标 x, y, w, h；源 x, y, w, h)（相对矩形左上角 / 纹理左上
    // 角）。宽或高 <= 0 的片跳过 —— 零中段的合法退化，半开区间无像素可画。
    let pieces = [
        // 上带：左角（1:1）/ 上边（水平拉伸）/ 右角（1:1，锚纹理右缘）。
        (0.0, 0.0, l, t, 0.0, 0.0, l, t),
        (l, 0.0, cx, t, l, 0.0, sw_edge, t),
        (l + cx, 0.0, r, t, sx_mid, 0.0, r, t),
        // 中带：左边（垂直拉伸）/ 中心（双向拉伸）/ 右边（垂直拉伸）。
        (0.0, t, l, cy, 0.0, t, l, sh_edge),
        (l, t, cx, cy, l, t, sw_edge, sh_edge),
        (l + cx, t, r, cy, sx_mid, t, r, sh_edge),
        // 下带：左角（1:1，锚纹理下缘）/ 下边（水平拉伸）/ 右角（锚双缘）。
        (0.0, t + cy, l, b, 0.0, sy_mid, l, b),
        (l, t + cy, cx, b, l, sy_mid, sw_edge, b),
        (l + cx, t + cy, r, b, sx_mid, sy_mid, r, b),
    ];
    for (dx, dy, dw, dh, sx, sy, sw, sh) in pieces {
        if dw <= 0.0 || dh <= 0.0 {
            continue;
        }
        let quad = Affine2::translation(rect.x + dx, rect.y + dy).mul(&Affine2::scale(
            dw / gpu::CELL_PX as f32,
            dh / gpu::CELL_PX as f32,
        ));
        sprites.push(SpriteInstance {
            handle,
            world: inv.mul(&quad).to_array(),
            uv_rect: [ux(sx), uy(sy), uw(sw), uh(sh)],
            source: [tile as f32, 1.0],
            tint: [1.0, 1.0, 1.0, 1.0],
            clip: item_clip,
        });
    }
}

/// 视口空间裁剪矩形 -> 帧缓冲像素 scissor（E-2 裁剪契约，S12-3）。
///
/// 折算规则（冻结）：
/// - 哨兵 [`SpriteInstance::NO_CLIP`]（`[0,0,0,0]`）= 全目标 scissor ——
///   每段显式设置，不依赖跨段状态；
/// - 其余按 `target_size / viewport` 比例缩放、`floor` 取整成**半开区间**
///   `[x0, x1) x [y0, y1)`，再与目标边界求交（保守包含亚像素覆盖的边界像素，
///   像素内部由光栅器精确裁剪）；
/// - 交集为空（或视口退化 / 裁剪值非有限）返回 `None`，调用方整段跳过。
fn clip_to_scissor(
    clip: [f32; 4],
    viewport: (f32, f32),
    target: (u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    if clip == SpriteInstance::NO_CLIP {
        return Some((0, 0, target.0, target.1));
    }
    if viewport.0 <= 0.0 || viewport.1 <= 0.0 || clip.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let scale_x = target.0 as f32 / viewport.0;
    let scale_y = target.1 as f32 / viewport.1;
    let fx = clip[0] * scale_x;
    let fy = clip[1] * scale_y;
    let x0 = fx.floor().max(0.0);
    let y0 = fy.floor().max(0.0);
    let x1 = (fx + clip[2] * scale_x).floor().min(target.0 as f32);
    let y1 = (fy + clip[3] * scale_y).floor().min(target.1 as f32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
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
        let layouts = [
            atlas.handles().bind_group_layout,
            registry.bind_group_layout(),
        ];
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

        // 4) 渲染管线的顶点属性表：三列矩阵 + UV 矩形 + 采样来源 + 着色 RGBA +
        //    裁剪矩形（跨度 INSTANCE_STRIDE = 80 字节；loc6 与 WGSL 的
        //    SpriteData::clip 对应 —— 布局声明与 CPU 打包同步扩展）。
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
            ffi::VertexAttribute {
                next_in_chain: ptr::null_mut(),
                format: ffi::WGPU_VERTEX_FORMAT_FLOAT32X4,
                offset: 48,
                shader_location: 5,
            },
            ffi::VertexAttribute {
                next_in_chain: ptr::null_mut(),
                format: ffi::WGPU_VERTEX_FORMAT_FLOAT32X4,
                offset: 64,
                shader_location: 6,
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
    /// 之间选择采样来源。`target_size` 是帧缓冲的像素尺寸（scissor 折算
    /// 比例的分母口径用 `view` 里的视口、分子用本参数）。无精灵时仍执行
    /// 清屏与存储 —— "空帧也要有合法像素"是读回的前提。
    ///
    /// # E-2 裁剪（S12-3）
    ///
    /// 精灵列表按**连续相同 `clip` 值**划段，每段先显式设 scissor 再
    /// `draw`：哨兵 [`SpriteInstance::NO_CLIP`] = 全目标 scissor（每段都
    /// 显式设置，状态不跨段继承，无"上一段裁剪泄漏到下一段"的时序隐患）；
    /// 真实裁剪值按 `target_size / viewport` 比例折算成帧缓冲像素、`floor`
    /// 取整为半开区间后与目标边界求交，交集为空的段整段跳过（一次 draw
    /// 都不发）。缺省路径（全部实例无裁剪）与单次整批 draw 逐位相同。
    ///
    /// 返回**实际经 `draw` 提交**的实例数（被空交集跳过的段不计入 ——
    /// `FrameStats::drawn` 的"实际提交"口径由此保证）。
    pub fn render(
        &mut self,
        ctx: &GpuContext,
        target_view: *mut c_void,
        registry: &gpu::TextureRegistry,
        view: &[f32; 8],
        sprites: &[SpriteInstance],
        target_size: (u32, u32),
    ) -> Result<u64, BackendError> {
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
        //    链序 = 顶点属性表序：world -> uv_rect -> source -> tint -> clip。
        self.staging.clear();
        for sprite in sprites {
            for f in sprite
                .world
                .iter()
                .chain(sprite.uv_rect.iter())
                .chain(sprite.source.iter())
                .chain(sprite.tint.iter())
                .chain(sprite.clip.iter())
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
        let pass = unsafe { (api.command_encoder_begin_render_pass)(encoder, &pass_desc) };
        if pass.is_null() {
            unsafe { (api.command_encoder_release)(encoder) };
            return Err(BackendError::NullHandle("WGPURenderPassEncoder(sprite)"));
        }
        // 实际提交的实例数（空交集段不计）。
        let mut submitted: u64 = 0;
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
                // E-2 裁剪（S12-3）：按连续相同 clip 值把实例划成段，每段先
                // 显式设 scissor 再画（哨兵段也显式设回全目标 —— 状态不跨段
                // 继承，无泄漏）。缺省路径（全部哨兵）只有一段，等价于整批 draw。
                let mut start = 0usize;
                while start < sprites.len() {
                    let clip = sprites[start].clip;
                    let mut end = start + 1;
                    while end < sprites.len() && sprites[end].clip == clip {
                        end += 1;
                    }
                    if let Some((x, y, w, h)) =
                        clip_to_scissor(clip, (view[6], view[7]), target_size)
                    {
                        (api.render_pass_encoder_set_scissor_rect)(pass, x, y, w, h);
                        (api.render_pass_encoder_draw)(
                            pass,
                            6,
                            (end - start) as u32,
                            0,
                            start as u32,
                        );
                        submitted += (end - start) as u64;
                    }
                    // 交集为空的段整段跳过：不可见内容一次 draw 都不发。
                    start = end;
                }
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
        Ok(submitted)
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
    /// 跨帧列表/页签登记表（`SetList` 建、`DestroyItem` 删；S12-3 任务 4）。
    /// 有 `SetList` 状态的渲染物按行/页签展开成字形序列（笔起点 = 矩形
    /// 左上 + 4 内衬，行 y 随 `ListState::scroll` 平移）。
    lists: BTreeMap<ItemHandle, ListState>,
    /// 跨帧相乘色登记表（`SetTint` 建/覆写、`DestroyItem` 删；S16.1 alpha
    /// 通道）。精灵实例的 tint 从中性改查此表 —— 无记录 = 中性恒等
    ///（与 E-1 之前的像素逐位相同）。
    tints: BTreeMap<ItemHandle, [u8; 4]>,
    /// 跨帧子矩形采样登记表（`SetUv` 建/覆写、`DestroyItem` 删；S16.2 图集
    /// 帧动画）。注册表精灵分支的 uv_rect 从全瓦片改查此表折算 —— 无记录
    /// = 整瓦片采样（与既有路径逐位相同）。归一化矩形 `[u0, v0, us, vs]`
    /// （相对整张注册纹理），消费点单处折算（见 draw_into 精灵分支）。
    uvs: BTreeMap<ItemHandle, [f32; 4]>,
    /// 跨帧精灵锚点登记表（`SetPivot` 建/覆写、`DestroyItem` 删；S16.3）。
    /// 注册表精灵分支的 world 从 `transform ∘ flip` 改查此表多乘一截
    /// **局部空间**平移 `-pivot × 16px 基准格` —— 无记录 = 无平移（与既有
    /// 路径逐位相同）。归一化锚点 `[px, py]`，消费点单处折算。
    pivots: BTreeMap<ItemHandle, [f32; 2]>,
    /// 跨帧九宫格登记表（`SetNineSlice` 建/覆写、NIL 清除、`DestroyItem`
    /// 删；S16.6）。Control 分支查此表决定走九宫格展开还是 fill/border
    /// 条带 —— 无记录 = fill/border 照旧（与既有路径逐位相同）。载荷 =
    /// `(源纹理键, [l, t, r, b])`（源纹理像素边距），消费点单处折算
    /// （见 draw_into Control 分支的九宫展开）。
    nines: BTreeMap<ItemHandle, (RenderAssetKey, [f32; 4])>,
    /// 字体登记表：资源键 -> 排版参数（字形表本体作为纹理住在注册表里）。
    /// 默认字体住在保留键 [`DEFAULT_FONT_KEY`] 下；`LabelState.font` 按键解析，
    /// 未登记的键与 `NIL` 一样退回默认字体（S4.5 契约口径，T-Text-07/08 钉住）。
    fonts: BTreeMap<RenderAssetKey, FontEntry>,
    /// TTF 默认字体（[`CommandConsumer::set_ttf_default`] 装载；`None` = 未装载，
    /// 文本一律走位图字形表路径 —— 与基线逐位相同）。
    ttf: Option<TtfFont>,
    /// TTF 动态字形图集（shelf 装箱 + (char, 字号) 缓存，见 [`crate::glyph`]）。
    /// 未装载 TTF 时恒为空集，零行为影响。
    glyph_atlas: GlyphAtlas,
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
    /// 字形表纹理尺寸（像素，S8.2 补存 —— 字格 UV 按实际纹理折算，
    /// 紧排表与留白表都成立）。
    tex: (f32, f32),
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

/// 文本光标的笔位步长（像素；等宽 16px 冻结设计语言 —— 与字距同口径）。
/// 契约层 [`LabelState::caret`] 的字符下标乘它得到笔位偏移；宽恒 1px、
/// 高取字形格高。是否本帧画由提取层的 `caret` 位裁决（`None` = 不画），
/// 渲染侧零动画状态。
const CARET_ADVANCE_PX: f32 = 16.0;

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
            lists: BTreeMap::new(),
            tints: BTreeMap::new(),
            uvs: BTreeMap::new(),
            pivots: BTreeMap::new(),
            nines: BTreeMap::new(),
            fonts: BTreeMap::new(),
            ttf: None,
            glyph_atlas: GlyphAtlas::default(),
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
        self.registry.register(&self.ctx, key, width, height, rgba)
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

    /// 装载 TTF 默认字体（S12-11 第 2 期）：之后 `LabelState.font == NIL` 的
    /// 文本改走**真字体动态字形图集**排版 —— 比例字宽、任意字号（clamp
    /// 8..128）、CJK 可上屏、光标位随比例字宽。
    ///
    /// `data` 是一份 `.ttf` 或 `.ttc` 字体文件字节（TTC 取第一个字体，解析
    /// 由 [`TtfFont::parse`] 完成）。解析失败如实报错（[`BackendError::
    /// ConfigMismatch`] 携带 [`crate::ttf::TtfError`] 原文），**失败不改动
    /// 现状**：已装载的旧 TTF 保留、未装载仍是位图路径。重复装载 = 覆写
    /// （字形缓存与已注册字形页保留不动 —— 页内容只增不改，新字体未命中的
    /// 字形会继续装箱进既有页序列，混合页面对编辑器场景无观察意义；如需
    /// 干净状态请新建消费器）。
    ///
    /// 显式 `font` 资源键（位图字形表路径）完全不受影响：装载后走 TTF 的
    /// 只有 `font == NIL` 的条目；未登记键仍退回位图默认字体（基线口径）。
    pub fn set_ttf_default(&mut self, data: &[u8]) -> Result<(), BackendError> {
        let font = TtfFont::parse(data)
            .map_err(|err| BackendError::ConfigMismatch(format!("TTF 默认字体解析失败：{err}")))?;
        self.ttf = Some(font);
        Ok(())
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
        if !(advance.is_finite() && advance > 0.0 && line_height.is_finite() && line_height > 0.0) {
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
                tex: (width as f32, height as f32),
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

    /// 字体解析（T-Text-07/08 口径）：`font == NIL` 或指向未登记键 ->
    /// 默认字体；指向已登记键 -> 该字体。两种都拿不到时返回 `None`。
    /// 文本（SetText）与列表/页签（SetList）两条展开路径共用 —— 解析
    /// 只有这一处实现。
    fn resolve_font(&self, font: RenderAssetKey) -> Option<(RenderAssetKey, FontEntry)> {
        if font.is_nil() {
            self.fonts
                .get(&DEFAULT_FONT_KEY)
                .map(|e| (DEFAULT_FONT_KEY, *e))
        } else {
            self.fonts.get(&font).map(|e| (font, *e)).or_else(|| {
                self.fonts
                    .get(&DEFAULT_FONT_KEY)
                    .map(|e| (DEFAULT_FONT_KEY, *e))
            })
        }
    }

    /// 驱动侧未捕获错误快照（`FrameStats::driver_errors` 的原文出处）。
    pub fn errors_snapshot(&self) -> Vec<String> {
        self.ctx.errors_snapshot()
    }

    /// 取（或装箱）一个 TTF 字形槽位：缓存键 `(char, size_px)`（见
    /// [`crate::glyph`] 的缓存语义 —— 进程内不淘汰）。
    ///
    /// 未命中路径：cmap 查码点 -> [`TtfFont::rasterize`] -> shelf 装箱 ->
    /// 字形页整张重注册（[`gpu::TextureRegistry::register`] 的语义是"同键
    /// 覆写 + 立即 `queueWriteTexture` 上传"—— 本 crate 没有 pending 上传
    /// 队列，runtime 层的 `upload_pending_textures` 是另一条路径；所以装箱
    /// 当帧即可采样，无需 flush）。
    ///
    /// 缺字形（cmap 查不到）：**跳过不画，只推笔位**（笔位步进取 `.notdef`
    /// 的 advance）。选跳过而非 notdef 方块兜底：编辑器文本混入未覆盖码点时
    /// 一排 .notdef 方块比安静留白更吵，且"未知字符占位推进"的排版节奏仍在。
    fn glyph_slot(&mut self, ch: char, size_px: i32) -> Result<GlyphSlot, BackendError> {
        if let Some(hit) = self.glyph_atlas.cached(ch, size_px) {
            return Ok(hit);
        }
        let font = self
            .ttf
            .as_ref()
            .expect("glyph_slot 只在 TTF 已装载时被调用");
        let px = size_px as f32;
        let (bitmap, advance) = match font.glyph_index(ch) {
            Some(gid) => {
                let bm = font.rasterize(gid, px).map_err(|err| {
                    BackendError::ConfigMismatch(format!(
                        "字形 {ch:?}（gid {gid}，{size_px}px）光栅化失败：{err}"
                    ))
                })?;
                let adv = bm.advance;
                (Some(bm), adv)
            }
            None => (None, font.advance(TtfFont::NOTDEF, px).unwrap_or(0.0)),
        };
        let placement = bitmap
            .as_ref()
            .and_then(|bm| self.glyph_atlas.place(bm.width, bm.height));
        let slot = match placement {
            Some((page, x, y)) => {
                let bm = bitmap.as_ref().expect("有落位必有位图");
                GlyphSlot {
                    page,
                    x,
                    y,
                    w: bm.width,
                    h: bm.height,
                    bearing_x: bm.bearing_x as f32,
                    bearing_y: bm.bearing_y as f32,
                    advance,
                }
            }
            // 空字形（空格等 0x0 位图）或单边超页：不占页，只记账 advance。
            None => GlyphSlot {
                page: 0,
                x: 0,
                y: 0,
                w: 0,
                h: 0,
                bearing_x: 0.0,
                bearing_y: 0.0,
                advance,
            },
        };
        if let Some((page, x, y)) = placement {
            let bm = bitmap.as_ref().expect("有落位必有位图");
            self.glyph_atlas
                .blit(page, x, y, bm.width, bm.height, &bm.coverage);
            // 字段拆借（blit 已结束）：页缓冲、注册表、上下文三处互不相交，
            // 免去整页 256 KiB 的克隆。
            let this = &mut *self;
            let rgba = this.glyph_atlas.page_rgba(page);
            this.registry.register(
                &this.ctx,
                page_key(page),
                GLYPH_PAGE_PX,
                GLYPH_PAGE_PX,
                rgba,
            )?;
        }
        self.glyph_atlas.cache_insert(ch, size_px, slot);
        Ok(slot)
    }

    /// TTF 文本排版（S12-11 第 2 期）：把一条 [`LabelState`] 展开成字形四边形
    /// 实例（笔基点 `world` 与位图路径同口径：按钮 = 矩形左上 + 4px 内衬，
    /// 纯 Label = 自身世界变换；调用方算好传入）。
    ///
    /// # 布局口径
    ///
    /// - 字号：`font_size` clamp 8..128 后取整（f32 -> i32）作缓存键；度量
    ///   （ascent / line_height）用 clamp 后的 f32 值；
    /// - 行基线：第 i 行基线 y = `i * (line_height + line_spacing) + ascent`
    ///   （line_spacing 与位图路径同口径逐行叠加 —— T-Text-13 契约在 TTF
    ///   路径同样成立）；
    /// - 字形四边形：左 = 笔位 + `bearing_x`，顶 = 基线 - `bearing_y`，宽高 =
    ///   位图像素尺寸直出；着色器常量是"世界单位 x 16 = 1 格"，故 scale 取
    ///   `(w/16, h/16)` —— 与位图路径的 `cell/16` 同一折算口径（像素不再除
    ///   `CELL_PX` 归一字格，只折算进世界矩阵）；
    /// - UV：字形页 256x256 整张注册，页内矩形按注册表纹理实际尺寸折算
    ///   （S8.2 修复口径）；
    /// - 空格 / 缺字形 / 空位图：只推笔位不画；
    /// - 光标（[`LabelState::caret`] = `Some(n)`）：竖条宽 1px、高 =
    ///   `line_height`，x = 第 n 个字符槽位前所有字符的 advance 之和
    ///   （'\n' 归零换行；单行文本退化为 `笔位起始 + sum(advance of
    ///   chars[..n])`），颜色同 `label.color`，与条目共用裁剪与 DrawKey 序。
    #[allow(clippy::too_many_arguments)]
    fn push_ttf_label(
        &mut self,
        sprites: &mut Vec<SpriteInstance>,
        handle: ItemHandle,
        clip: [f32; 4],
        world: &Affine2,
        label: &LabelState,
        fill_uv: [f32; 4],
        glyphs: &mut u64,
    ) -> Result<(), BackendError> {
        let size_f = label.font_size.clamp(8.0, 128.0);
        let size_px = size_f.round() as i32;
        let metrics = self
            .ttf
            .as_ref()
            .expect("push_ttf_label 只在 TTF 已装载时被调用")
            .metrics(size_f)
            .expect("字号 clamp 8..128 后度量不可能失败");
        let tint = SpriteInstance::tint_of(label.color);
        let line_h = metrics.line_height + label.line_spacing;
        for (line_index, line) in label.text.split('\n').enumerate() {
            let baseline = line_index as f32 * line_h + metrics.ascent;
            let mut pen_x = 0.0f32;
            for ch in line.chars() {
                let slot = self.glyph_slot(ch, size_px)?;
                if slot.w > 0 && slot.h > 0 {
                    let quad = world
                        .mul(&Affine2::translation(
                            pen_x + slot.bearing_x,
                            baseline - slot.bearing_y,
                        ))
                        .mul(&Affine2::scale(
                            slot.w as f32 / gpu::CELL_PX as f32,
                            slot.h as f32 / gpu::CELL_PX as f32,
                        ));
                    let (tile, sheet) = self
                        .registry
                        .sample_info(page_key(slot.page))
                        .expect("字形页在装箱时已注册");
                    // 页内矩形 -> 大纹理 UV：页恒为 256x256 整张注册，
                    // sample_info 返回注册尺寸的瓦片矩形（S8.2 口径）。
                    let page = GLYPH_PAGE_PX as f32;
                    let uv_rect = [
                        sheet[0] + slot.x as f32 / page * sheet[2],
                        sheet[1] + slot.y as f32 / page * sheet[3],
                        slot.w as f32 / page * sheet[2],
                        slot.h as f32 / page * sheet[3],
                    ];
                    sprites.push(SpriteInstance {
                        handle,
                        world: quad.to_array(),
                        uv_rect,
                        source: [tile as f32, 1.0],
                        tint,
                        clip,
                    });
                    *glyphs += 1;
                }
                pen_x += slot.advance;
            }
        }
        // 光标：n = caret，前 n 个字符的 advance 累加（'\n' 归零换行）。
        // 缺字形同样按 .notdef advance 推进，与正文的笔位轨迹严格一致。
        if let Some(caret) = label.caret {
            let mut caret_line = 0.0f32;
            let mut caret_x = 0.0f32;
            for ch in label.text.chars().take(usize::from(caret)) {
                if ch == '\n' {
                    caret_line += line_h;
                    caret_x = 0.0;
                    continue;
                }
                caret_x += self.glyph_slot(ch, size_px)?.advance;
            }
            let quad = world
                .mul(&Affine2::translation(caret_x, caret_line))
                .mul(&Affine2::scale(
                    1.0 / gpu::CELL_PX as f32,
                    metrics.line_height / gpu::CELL_PX as f32,
                ));
            sprites.push(SpriteInstance {
                handle,
                world: quad.to_array(),
                uv_rect: fill_uv,
                source: [0.0, 0.0],
                tint,
                clip,
            });
        }
        Ok(())
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

    /// 精灵实例的相乘色（S16.1 alpha 通道）：查跨帧 tint 簿记，无记录 =
    /// 中性恒等 —— 与 E-1 之前的像素逐位相同。字形/控件实例不查此表
    ///（颜色各走契约字段 `LabelState::color` / `ControlState::fill` 等）。
    fn sprite_tint(&self, handle: ItemHandle) -> [f32; 4] {
        match self.tints.get(&handle) {
            Some(rgba) => SpriteInstance::tint_of(*rgba),
            None => [1.0, 1.0, 1.0, 1.0],
        }
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
        // 裁剪表是**帧本地**的（E-2 / D1）：每帧的裁剪状态只来自本帧命令流。
        // 与 rects/texts 的跨帧登记表刻意不同 —— 契约侧的 `set_clip(h, None)`
        // 清除后，后续帧的全量快照里不再出现该条目的 `SetClip`，帧本地表让
        // "未被重申的裁剪 = 本帧不裁"自然成立（缺省路径因此与裁剪机制之前
        // 逐位相同）；`SetClip { rect: None }` 在帧内同样按"清除"处理。
        let mut clips: BTreeMap<ItemHandle, Rect> = BTreeMap::new();

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
                        self.lists.remove(handle);
                        self.tints.remove(handle);
                        self.uvs.remove(handle);
                        self.pivots.remove(handle);
                        self.nines.remove(handle);
                        stats.destroys += 1;
                    } else {
                        stats.ignored += 1;
                    }
                }
                RenderCommand::SetCamera { camera: c } => camera = Some(*c),
                RenderCommand::SetVisible { handle, visible } => match self.items.get_mut(handle) {
                    Some(item) => {
                        item.visible = *visible;
                        stats.updates += 1;
                    }
                    None => stats.ignored += 1,
                },
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
                // SetClip：登记/清除裁剪矩形（E-2 / D1）。入帧本地表（见其声明
                // 处的说明）；空句柄 / 未知句柄静默忽略（契约 I1 口径）。
                RenderCommand::SetClip { handle, rect } => {
                    if self.items.contains_key(handle) {
                        match rect {
                            Some(rect) => {
                                clips.insert(*handle, *rect);
                            }
                            None => {
                                clips.remove(handle);
                            }
                        }
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
                // SetList：登记列表/页签状态（S12-3 任务 4），绘制阶段按
                // 行/页签展开成字形序列（与 SetText 同一字形机制）。
                RenderCommand::SetList { handle, rows } => {
                    if self.items.contains_key(handle) {
                        self.lists.insert(*handle, rows.clone());
                        stats.updates += 1;
                    } else {
                        stats.ignored += 1;
                    }
                }
                // SetTint：登记相乘色（S16.1 alpha 通道）。精灵实例的 tint
                // 按此覆写（无记录 = 中性恒等）；已知句柄同键覆写，未知句柄
                // 静默忽略（契约 I1 口径）。
                RenderCommand::SetTint { handle, rgba } => {
                    if self.items.contains_key(handle) {
                        self.tints.insert(*handle, *rgba);
                        stats.updates += 1;
                    } else {
                        stats.ignored += 1;
                    }
                }
                // SetUv：登记子矩形采样（S16.2 图集帧动画）。注册表精灵分支
                // 按此折算采样矩形（无记录 = 整瓦片，逐位不变）；已知句柄
                // 同键覆写，未知句柄静默忽略（契约 I1 口径）。
                RenderCommand::SetUv { handle, rect } => {
                    if self.items.contains_key(handle) {
                        self.uvs.insert(*handle, *rect);
                        stats.updates += 1;
                    } else {
                        stats.ignored += 1;
                    }
                }
                // SetPivot：登记精灵锚点（S16.3）。注册表精灵分支按此在
                // world 之后多乘一截局部空间平移（无记录 = 无平移，逐位
                // 不变）；已知句柄同键覆写，未知句柄静默忽略（契约 I1）。
                RenderCommand::SetPivot { handle, pivot } => {
                    if self.items.contains_key(handle) {
                        self.pivots.insert(*handle, *pivot);
                        stats.updates += 1;
                    } else {
                        stats.ignored += 1;
                    }
                }
                // SetNineSlice：登记九宫格配置（S16.6）。Control 分支按此
                // 改走九宫格展开（无记录 = fill/border 照旧，逐位不变）；
                // NIL 键 = 清除；已知句柄同键覆写，未知句柄静默忽略（契约
                // I1 口径）。
                RenderCommand::SetNineSlice {
                    handle,
                    texture,
                    l,
                    t,
                    r,
                    b,
                } => {
                    if self.items.contains_key(handle) {
                        if texture.is_nil() {
                            self.nines.remove(handle);
                        } else {
                            self.nines.insert(*handle, (*texture, [*l, *t, *r, *b]));
                        }
                        stats.updates += 1;
                    } else {
                        stats.ignored += 1;
                    }
                }
                RenderCommand::Submit { .. } => {} // 终止标记，已在入口校验。
            }
        }

        // 绘制列表：可见，且（精灵：资源键已绑定 / 控件：有 SetRect / 文本：
        // 有 SetText / 列表：有 SetList），按 DrawKey 升序（契约 I5）。
        // 拷贝而非借用（RenderItem 是 Copy 的小结构）：条目循环体内的 TTF
        // 路径需要 &mut self（字形缓存未命中会装箱新页并注册上传），持有
        // 借用会把整个消费器锁死。
        let mut draw_list: Vec<RenderItem> = self
            .items
            .values()
            .copied()
            .filter(|item| {
                item.visible
                    && (!item.key.is_nil()
                        || self.rects.contains_key(&item.handle)
                        || self.texts.contains_key(&item.handle)
                        || self.lists.contains_key(&item.handle))
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
        let mut sprites: Vec<SpriteInstance> = Vec::with_capacity(draw_list.len());
        // E-1 颜色通道（S12.1）：填充/边框条按 ControlState 的颜色发实例
        //（填充格 + 四条 border_w 宽的边条 —— 像素精确的平直边框，不再
        // 随矩形尺寸缩放图案边）。按钮（同句柄 rect+text）的文字锚定
        // 矩形左上 + 4px 内衬；纯 Label 仍锚定自身世界变换（既有行为）。
        let fill_uv = [
            cell_uv * (gpu::FILL_CELL % gpu::ATLAS_CELLS) as f32,
            cell_uv * (gpu::FILL_CELL / gpu::ATLAS_CELLS) as f32,
            cell_uv,
            cell_uv,
        ];
        for item in draw_list {
            // E-2 裁剪：该条目本帧的视口空间裁剪矩形（帧本地表，无则哨兵），
            // 条目的全部实例共享同一裁剪 —— render 侧按连续相同 clip 值分段设 scissor。
            let clip_entry = clips.get(&item.handle);
            // D6 全裁（S12-3 任务 4）：零尺寸/负尺寸裁剪矩形 = 本帧什么都不画
            //（空交集条目）。实例侧的零矩形与 NO_CLIP 哨兵（[0,0,0,0] = 全目标
            // scissor）在表示上不可区分，"空交 = 全裁"必须在这里提前拦下 ——
            // 该条目的实例整条省略，效果等同空 scissor。
            if let Some(rect) = clip_entry {
                if rect.w <= 0.0 || rect.h <= 0.0 {
                    continue;
                }
            }
            let item_clip = clip_entry
                .map(|rect| rect.to_array())
                .unwrap_or(SpriteInstance::NO_CLIP);
            if let (Some(rect_state), Some(inv)) = (self.rects.get(&item.handle), inv_view) {
                let rect = rect_state.resolve(viewport);
                // S16.6 九宫格分臂：有九宫格簿记（非 NIL 键）时面板改走
                // 九宫格展开，fill/border 条带**不画**（纹理自带边）。
                // 纹理未注册 / 矩形退化（非正宽高）= 本帧不画面板 —— 不
                // 回退 fill/border，防"有纹理画九宫、没纹理画边框"的模式
                // 间闪烁；滚动条两种模式共用（面板内容 chrome，非面板本体）。
                let nine = match self.nines.get(&item.handle) {
                    Some((texture, margins)) if !texture.is_nil() => Some((*texture, *margins)),
                    _ => None,
                };
                if let Some((ns_tex, ns_margins)) = nine {
                    if rect.w > 0.0 && rect.h > 0.0 {
                        // sample_info（全瓦片 uv）与注册尺寸（像素分母）
                        // 同查一表；任一缺席 = 纹理未注册，本帧不画
                        //（不回退 fill/border —— 见上方分臂注释）。
                        if let Some(((tile, sheet), tex_px)) = self
                            .registry
                            .sample_info(ns_tex)
                            .zip(self.registry.texture_px_size(ns_tex))
                        {
                            push_nine_slice(
                                &mut sprites,
                                item.handle,
                                item_clip,
                                &inv,
                                rect,
                                tile,
                                sheet,
                                tex_px,
                                ns_margins,
                            );
                        }
                    }
                }
                if nine.is_none() {
                    // 填充（alpha == 0 不发 —— 缺省透明，与 E-1 之前同像素）。
                    if rect_state.fill[3] > 0 {
                        let quad = Affine2::translation(rect.x, rect.y).mul(&Affine2::scale(
                            rect.w / gpu::CELL_PX as f32,
                            rect.h / gpu::CELL_PX as f32,
                        ));
                        sprites.push(SpriteInstance {
                            handle: item.handle,
                            world: inv.mul(&quad).to_array(),
                            uv_rect: fill_uv,
                            source: [0.0, 0.0],
                            tint: SpriteInstance::tint_of(rect_state.fill),
                            clip: item_clip,
                        });
                    }
                    // 边框：四条 border_w 宽的填充条（上/下/左/右）。
                    if rect_state.border[3] > 0 {
                        let t = rect_state
                            .border_w
                            .clamp(0.0, (rect.h * 0.5).max(0.0))
                            .clamp(0.0, (rect.w * 0.5).max(0.0));
                        let border_tint = SpriteInstance::tint_of(rect_state.border);
                        let strips = [
                            (rect.x, rect.y, rect.w, t),
                            (rect.x, rect.y + rect.h - t, rect.w, t),
                            (rect.x, rect.y + t, t, rect.h - 2.0 * t),
                            (rect.x + rect.w - t, rect.y + t, t, rect.h - 2.0 * t),
                        ];
                        for (sx, sy, sw, sh) in strips {
                            let quad = Affine2::translation(sx, sy).mul(&Affine2::scale(
                                sw / gpu::CELL_PX as f32,
                                sh / gpu::CELL_PX as f32,
                            ));
                            sprites.push(SpriteInstance {
                                handle: item.handle,
                                world: inv.mul(&quad).to_array(),
                                uv_rect: fill_uv,
                                source: [0.0, 0.0],
                                tint: border_tint,
                                clip: item_clip,
                            });
                        }
                    }
                }
                // 滚动条（S12-3 任务 4）：`ControlState::scroll_bar` 为 Some
                // 时在矩形**右缘内侧**画 4px 宽竖向滑块（x = rect.x+rect.w-5），
                // 滑块长 max(8, frac*(h-2))，行程 = h-2-滑块长、按 pos 取位，
                // y 基点 = rect.y+1（上下各让 1px 内衬）。填充格 + tint =
                // bar.color（提取层算好的视觉状态，本层零再计算）。
                // 九宫格模式下同样成立（滑块是内容 chrome，不随面板纹理走）。
                if let Some(bar) = rect_state.scroll_bar {
                    let thumb_h = (bar.frac * (rect.h - 2.0)).max(8.0);
                    let bar_x = rect.x + rect.w - 5.0;
                    let bar_y = rect.y + 1.0 + bar.pos * (rect.h - 2.0 - thumb_h);
                    let quad = Affine2::translation(bar_x, bar_y).mul(&Affine2::scale(
                        4.0 / gpu::CELL_PX as f32,
                        thumb_h / gpu::CELL_PX as f32,
                    ));
                    sprites.push(SpriteInstance {
                        handle: item.handle,
                        world: inv.mul(&quad).to_array(),
                        uv_rect: fill_uv,
                        source: [0.0, 0.0],
                        tint: SpriteInstance::tint_of(bar.color),
                        clip: item_clip,
                    });
                }
                stats.controls += 1;
            }
            if let Some(rows) = self.lists.get(&item.handle) {
                // 列表/页签展开（S12-3 任务 4）：与文本分支同构 —— 复用
                // 同一套字体解析与字格 UV 机制，每行/页签一段字形序列。
                // 笔起点 = 解析矩形左上 + 4 内衬（滚动烘焙已在 ControlState
                // 的 offset 里，直接用解析矩形）。矩形缺席或视图退化时
                // 不画（行进几何与裁剪都要矩形；摊平路径恒有 SetRect）。
                if let (Some(rect_state), Some(inv)) = (self.rects.get(&item.handle), inv_view) {
                    let rect = rect_state.resolve(viewport);
                    if let Some((font_key, font)) = self.resolve_font(rows.font) {
                        if let Some((tile, sheet_uv)) = self.registry.sample_info(font_key) {
                            match rows.axis {
                                ListAxis::Vertical => {
                                    // 垂直轴（ListView）：行 i 的笔 y =
                                    // 4 + i*row_h - scroll（scroll 已折进
                                    // ListState —— 提取层当帧值；冻结算式）。
                                    // 行完全超出矩形顶/底不画（clip 会裁，
                                    // 这里省实例；continue 跳过但不影响
                                    // 后续行的下标计数）。
                                    for (i, line) in rows.text.split('\n').enumerate() {
                                        let top =
                                            rect.y + 4.0 + i as f32 * rows.row_h - rows.scroll;
                                        if top >= rect.y + rect.h {
                                            break; // 行超矩形底：其后各行更靠下。
                                        }
                                        // 判据用 row_h 而非字形格高（S12-3 评审
                                        // [low]：选中条带底缘 top+row_h-1 比格高
                                        // 低 1px，临界滚动位会漏画 1px 条带）。
                                        if top + rows.row_h <= rect.y {
                                            continue; // 整行（含条带）已在矩形顶之上。
                                        }
                                        let selected = rows.selected == Some(i as u16);
                                        // 选中条：列表矩形内衬边框 1px
                                        //（x+1..w-2），行带内衬 1px 高 row_h-2。
                                        let band = (
                                            rect.x + 1.0,
                                            top + 1.0,
                                            rect.w - 2.0,
                                            rows.row_h - 2.0,
                                        );
                                        push_list_row(
                                            &mut sprites,
                                            item.handle,
                                            item_clip,
                                            &inv,
                                            fill_uv,
                                            font,
                                            sheet_uv,
                                            tile,
                                            rows.sel_fill,
                                            rows.text_color,
                                            (rect.x + 4.0, top),
                                            band,
                                            selected,
                                            line,
                                            usize::MAX,
                                            &mut stats.glyphs,
                                        );
                                    }
                                }
                                ListAxis::Horizontal => {
                                    // 水平轴（Tabs）：页签 i 的笔 x =
                                    // 4 + i*tab_w（忽略 scroll —— 冻结口径）；
                                    // 页签文本按 (tab_w - 8) / 16 个字符截断
                                    //（16px 等宽冻结口径）。
                                    let max_chars =
                                        (((rows.tab_w - 8.0) / 16.0).floor().max(0.0)) as usize;
                                    for (i, line) in rows.text.split('\n').enumerate() {
                                        let left = rect.x + 4.0 + i as f32 * rows.tab_w;
                                        if left >= rect.x + rect.w {
                                            break; // 页签超矩形右缘：其后更靠右。
                                        }
                                        let selected = rows.selected == Some(i as u16);
                                        // 选中条：页签格内衬边框 1px，高同行带。
                                        let band = (
                                            left + 1.0,
                                            rect.y + 5.0,
                                            rows.tab_w - 2.0,
                                            rows.row_h - 2.0,
                                        );
                                        push_list_row(
                                            &mut sprites,
                                            item.handle,
                                            item_clip,
                                            &inv,
                                            fill_uv,
                                            font,
                                            sheet_uv,
                                            tile,
                                            rows.sel_fill,
                                            rows.text_color,
                                            (left, rect.y + 4.0),
                                            band,
                                            selected,
                                            line,
                                            max_chars,
                                            &mut stats.glyphs,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            } else if let Some(label) = self.texts.get(&item.handle).cloned() {
                // 文本（S4.4/S4.5）：世界变换 = 笔起点（首行首字格左上角），每字形
                // 一个四边形，采样字形表对应字格。字距恒定（等宽口径）、
                // 行高 = 基准 + line_spacing；空格与表外字符只推进笔位不画。
                // 字体解析（T-Text-07/08 口径）：`font == NIL` 或指向未登记键
                // -> 默认字体；指向已登记键 -> 该字体。两种都拿不到时不画。
                // （`.cloned()` 只递增一个 Arc —— push_ttf_label 需要 &mut self，
                // 不能与这里的只读借用共存；位图分支不受影响。）
                if label.font.is_nil() && self.ttf.is_some() {
                    // TTF 默认字体路径（S12-11 第 2 期）：真字体动态字形图集
                    // 排版 —— 比例字宽、字号 clamp 8..128、CJK 可上屏；笔基点
                    // 与位图路径同口径（按钮 = 矩形左上 + 4px 内衬，纯 Label =
                    // 自身世界变换）。显式 font 键与未登记键不走这里（基线不变）。
                    let world = match (self.rects.get(&item.handle), inv_view) {
                        (Some(rect_state), Some(inv)) => {
                            let rect = rect_state.resolve(viewport);
                            inv.mul(&Affine2::translation(rect.x + 4.0, rect.y + 4.0))
                        }
                        _ => item.world_transform(),
                    };
                    self.push_ttf_label(
                        &mut sprites,
                        item.handle,
                        item_clip,
                        &world,
                        &label,
                        fill_uv,
                        &mut stats.glyphs,
                    )?;
                } else if let Some((font_key, font)) = self.resolve_font(label.font) {
                    if let Some((tile, sheet_uv)) = self.registry.sample_info(font_key) {
                        // 笔基点：纯 Label = 自身世界变换（既有行为）；
                        // 按钮（同句柄带 rect）= 矩形左上 + 4px 内衬
                        //（S12.0 设计语言 4px 栅格）。
                        let world = match (self.rects.get(&item.handle), inv_view) {
                            (Some(rect_state), Some(inv)) => {
                                let rect = rect_state.resolve(viewport);
                                inv.mul(&Affine2::translation(rect.x + 4.0, rect.y + 4.0))
                            }
                            _ => item.world_transform(),
                        };
                        let text_tint = SpriteInstance::tint_of(label.color);
                        // 字格 UV 按**纹理实际尺寸**折算（S8.2 实证修复）：
                        // 按列数/行数除只在"紧排表"（tex == cols*cell ×
                        // rows*cell）成立 —— 真实烘焙图集 256x256 只占顶部
                        // 96px，按 rows 除会每格采样 2.67 倍高度再压进 16px
                        // 四边形，字形竖向压扁成 ~3px（面板/HUD 文字一直
                        // 过小的根因）。紧排表两式等价，行为不变。
                        let cell_us = sheet_uv[2] * font.cell.0 / font.tex.0;
                        let cell_vs = sheet_uv[3] * font.cell.1 / font.tex.1;
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
                                let quad = world.mul(&Affine2::translation(pen_x, line_y)).mul(
                                    &Affine2::scale(
                                        font.cell.0 / gpu::CELL_PX as f32,
                                        font.cell.1 / gpu::CELL_PX as f32,
                                    ),
                                );
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
                                    tint: text_tint,
                                    clip: item_clip,
                                });
                                stats.glyphs += 1;
                            }
                        }
                        // 文本光标（S12-2）：1px 宽、字格高的实心竖条，画在
                        // 笔起点 + `caret * 16px`（等宽 16px 冻结口径）；颜色
                        // 取 [`LabelState::color`]。`None` = 本帧不画（提取层
                        // 30 帧节拍的"隐"半拍），后端零动画状态。
                        if let Some(caret) = label.caret {
                            let quad = world
                                .mul(&Affine2::translation(caret as f32 * CARET_ADVANCE_PX, 0.0))
                                .mul(&Affine2::scale(
                                    1.0 / gpu::CELL_PX as f32,
                                    font.cell.1 / gpu::CELL_PX as f32,
                                ));
                            sprites.push(SpriteInstance {
                                handle: item.handle,
                                world: quad.to_array(),
                                uv_rect: fill_uv,
                                source: [0.0, 0.0],
                                tint: text_tint,
                                clip: item_clip,
                            });
                        }
                    }
                }
            } else if let Some((layer, uv_rect)) = self.registry.sample_info(item.key) {
                // 键已注册：采样注册表图层（纹理落在图层左上角，UV 按实际尺寸裁剪）。
                // S16.2 图集帧动画：有 SetUv 簿记时按归一化子矩形折算 ——
                // 最终采样坐标 = 瓦片左上 + 归一化偏移 x 瓦片宽高（单处折算，
                // sample_info 的全瓦片矩形是唯一参照）。恒等矩形 `[0,0,1,1]`
                // 折算结果与全瓦片逐位相同（0.0 偏移 + 1.0 比例都是精确浮点）；
                // 无记录走原矩形 —— 既有路径逐位不变。
                let uv_rect = match self.uvs.get(&item.handle) {
                    Some(r) => [
                        uv_rect[0] + r[0] * uv_rect[2],
                        uv_rect[1] + r[1] * uv_rect[3],
                        r[2] * uv_rect[2],
                        r[3] * uv_rect[3],
                    ],
                    None => uv_rect,
                };
                // S16.3 精灵锚点：有 SetPivot 簿记时在 world **之后**乘一截
                // 平移 —— `Affine2::mul(self, rhs)` 的序是"先 rhs 后 self"
                // （self ∘ rhs，见 nes-render-api::math 的乘法文档），因此
                // `world ∘ translation(-pivot × 16px)` 把平移落在了**变换前
                // 的局部空间**：世界变换的旋转/缩放先作用于平移过的四边形，
                // 位置/旋转/缩放遂全部以锚点为基准（(0.5,0.5) = 中心锚定）。
                // 反序（translation ∘ world）会把平移抬到世界空间，旋转轴
                // 跟着错位 —— 序错则锚点语义整体作废，这里是唯一折算点。
                // 无记录走原矩阵 —— 既有路径逐位不变（`[0,0]` 记录 = 零
                // 平移，乘上去的 ±0.0 加法也逐位精确，见 T-P-01）。
                let world = match self.pivots.get(&item.handle) {
                    Some(p) => item.world_transform().mul(&Affine2::translation(
                        -p[0] * gpu::CELL_PX as f32,
                        -p[1] * gpu::CELL_PX as f32,
                    )),
                    None => item.world_transform(),
                };
                sprites.push(SpriteInstance {
                    handle: item.handle,
                    world: world.to_array(),
                    uv_rect,
                    source: [layer as f32, 1.0],
                    // S16.1：tint 查跨帧簿记（无记录 = 中性恒等，逐位不变）。
                    tint: self.sprite_tint(item.handle),
                    clip: item_clip,
                });
                stats.from_registry += 1;
            } else if !self.rects.contains_key(&item.handle) {
                sprites.push(SpriteInstance {
                    handle: item.handle,
                    world: item.world_transform().to_array(),
                    uv_rect: cell_uv_rect(item.key),
                    source: [0.0, 0.0],
                    // S16.1：同上 —— 图集格路径同样查 tint 簿记。
                    tint: self.sprite_tint(item.handle),
                    clip: item_clip,
                });
            }
        }

        stats.drawn = self.pipeline.render(
            &self.ctx,
            target_view,
            &self.registry,
            &view_params,
            &sprites,
            target_size,
        )?;
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
        assert!(a
            .iter()
            .all(|c| c.handle() != Some(ItemHandle::from_raw(999))));
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
