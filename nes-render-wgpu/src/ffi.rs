//! wgpu-native（v29 资产）的 C ABI 绑定：手写 `#[repr(C)]` 结构体 + 运行时符号解析。
//!
//! # 为什么手写而不是用 bindgen / wgpu-rs
//!
//! 1. **本机工具链约束**：本机没有 MSVC 链接器 / 无 gcc / 无 cmake，`wgpu-rs` 需要
//!    构建脚本 + 原生工具链；bindgen 同样需要 `libclang`。手写绑定把"能不能编译"
//!    从不确定事件变回确定事件（这也是 `nes-scene` / `nes-render-api` 坚持零依赖的同一条理由）。
//! 2. **单向依赖**：方案 D 要求后端是叶子。自己解析符号意味着**不引入任何第三方 crate**，
//!    依赖图上只有一个 `nes-render-api`（见 `Cargo.toml` 的注释与依赖守卫 G8）。
//! 3. **ABI 是本层的真实风险点，因此必须可视化**：所有结构体尺寸/对齐在
//!    `tests/criterion_ffi_layout.rs` 里对着"由 ctypes 原型实测导出"的尺寸表逐条钉死；
//!    函数签名逐条对应原型脚本已验证的 `argtypes`（S4.1 原型已在本机实跑通过：
//!    清屏 + 绘精灵 + 读回像素全绿）。
//!
//! # 与 webgpu.h 的对应关系
//!
//! 字段名、字段顺序、字段类型与 `temp\wgpu-win\include\webgpu\webgpu.h`（资产内，
//! **只读**）逐项对应；本模块不做任何字段合并或重排，也不做"顺手加点糖"的封装 ——
//! 糖放在 [`crate::gpu`] 与 [`crate::renderer`]。
//!
//! 本模块豁免字段级文档（`#[allow(missing_docs)]` 在 `lib.rs` 的模块声明上）：
//! 每个字段都是 webgpu.h 的同名字段，重复注释没有信息量；有语义决策的地方
//! （用哪个枚举值、为什么用 ProcessEvents 而不是 WaitAny、Success 为什么是 1）
//! 都写在类型级与函数级注释里。

use core::ffi::{c_char, c_void};
use core::mem;
use std::path::Path;

use crate::error::BackendError;

// ------------------------------------------------------------ 枚举常量
//
// 全部取自 webgpu.h 的实测值（`dump_abi.py` 口径），与原型脚本 `ev()` 解析结果一致。
// 命名统一加 `WGPU_` 前缀，避免与 Rust 侧命名冲突。

/// `WGPUSType_ShaderSourceWGSL`
pub const WGPU_STYPE_SHADER_SOURCE_WGSL: i32 = 0x0000_0002;
/// `WGPUInstanceFeatureName_TimedWaitAny`
pub const WGPU_INSTANCE_FEATURE_TIMED_WAIT_ANY: i32 = 0x0000_0001;
/// `WGPUTextureFormat_RGBA8Unorm`
pub const WGPU_TEXTURE_FORMAT_RGBA8_UNORM: i32 = 22;
/// `WGPUTextureDimension_2D`
pub const WGPU_TEXTURE_DIMENSION_2D: i32 = 2;
/// `WGPUTextureAspect_All`
pub const WGPU_TEXTURE_ASPECT_ALL: i32 = 1;
/// `WGPUTextureUsage_RenderAttachment`
pub const WGPU_TEXTURE_USAGE_RENDER_ATTACHMENT: u64 = 16;
/// `WGPUTextureUsage_CopySrc`
pub const WGPU_TEXTURE_USAGE_COPY_SRC: u64 = 1;
/// `WGPUTextureUsage_CopyDst`
pub const WGPU_TEXTURE_USAGE_COPY_DST: u64 = 2;
/// `WGPUTextureUsage_TextureBinding`
pub const WGPU_TEXTURE_USAGE_TEXTURE_BINDING: u64 = 4;
/// `WGPUBufferUsage_MapRead`
pub const WGPU_BUFFER_USAGE_MAP_READ: u64 = 1;
/// `WGPUBufferUsage_CopyDst`
pub const WGPU_BUFFER_USAGE_COPY_DST: u64 = 8;
/// `WGPUBufferUsage_Vertex`
pub const WGPU_BUFFER_USAGE_VERTEX: u64 = 32;
/// `WGPUBufferUsage_Uniform`（视图参数缓冲需要 `Uniform | CopyDst` 两个用法位）。
pub const WGPU_BUFFER_USAGE_UNIFORM: u64 = 64;

// ------------------------------------------------------------ 表面（S6.1）

/// `WGPUSType_SurfaceSourceWindowsHWND`（HWND 表面源的链式标记）。
pub const WGPU_STYPE_SURFACE_SOURCE_WINDOWS_HWND: i32 = 0x0000_0005;
/// `WGPUPresentMode_Fifo`（垂直同步呈现；caps 保证可用）。
pub const WGPU_PRESENT_MODE_FIFO: i32 = 1;
/// `WGPUCompositeAlphaMode_Auto`（不透明合成，驱动自选）。
pub const WGPU_COMPOSITE_ALPHA_MODE_AUTO: i32 = 0;
/// `WGPUSurfaceGetCurrentTextureStatus_SuccessOptimal`。
pub const WGPU_SURFACE_STATUS_SUCCESS_OPTIMAL: i32 = 1;
/// `WGPUSurfaceGetCurrentTextureStatus_SuccessSuboptimal`（仍可呈现）。
pub const WGPU_SURFACE_STATUS_SUCCESS_SUBOPTIMAL: i32 = 2;
/// `WGPUSurfaceGetCurrentTextureStatus_Timeout`（瞬态：呈现队列暂满，
/// 常见于窗口被遮挡/合成器停顿 —— 下一帧重试通常即恢复）。
pub const WGPU_SURFACE_STATUS_TIMEOUT: i32 = 3;
/// `WGPUSurfaceGetCurrentTextureStatus_Outdated`（表面已过期，需重配置）。
pub const WGPU_SURFACE_STATUS_OUTDATED: i32 = 4;
/// `WGPUSurfaceGetCurrentTextureStatus_Lost`（表面已丢失，需重建）。
pub const WGPU_SURFACE_STATUS_LOST: i32 = 5;
/// `WGPUSurfaceGetCurrentTextureStatus_OutOfMemory`。
pub const WGPU_SURFACE_STATUS_OUT_OF_MEMORY: i32 = 6;
/// `WGPUMapMode_Read`
pub const WGPU_MAP_MODE_READ: u64 = 1;
/// `WGPUAddressMode_ClampToEdge`
pub const WGPU_ADDRESS_MODE_CLAMP_TO_EDGE: i32 = 1;
/// `WGPUFilterMode_Nearest`
pub const WGPU_FILTER_MODE_NEAREST: i32 = 1;
/// `WGPUMipmapFilterMode_Nearest`
pub const WGPU_MIPMAP_FILTER_MODE_NEAREST: i32 = 1;
/// `WGPUCompareFunction_Undefined`
pub const WGPU_COMPARE_FUNCTION_UNDEFINED: i32 = 0;
/// `WGPUShaderStage_Vertex`
pub const WGPU_SHADER_STAGE_VERTEX: u64 = 1;
/// `WGPUShaderStage_Fragment`
pub const WGPU_SHADER_STAGE_FRAGMENT: u64 = 2;
/// `WGPUTextureSampleType_Float`
pub const WGPU_TEXTURE_SAMPLE_TYPE_FLOAT: i32 = 2;
/// `WGPUTextureViewDimension_2D`
pub const WGPU_TEXTURE_VIEW_DIMENSION_2D: i32 = 2;
/// `WGPUBufferBindingType_Uniform`（视口参数走统一缓冲）。
pub const WGPU_BUFFER_BINDING_TYPE_UNIFORM: i32 = 2;
/// `WGPUSamplerBindingType_Filtering`
pub const WGPU_SAMPLER_BINDING_TYPE_FILTERING: i32 = 2;
/// `WGPUVertexFormat_Float32x2`
pub const WGPU_VERTEX_FORMAT_FLOAT32X2: i32 = 29;
/// `WGPUVertexFormat_Float32x4`（UV 矩形四分量属性）。
pub const WGPU_VERTEX_FORMAT_FLOAT32X4: i32 = 31;
/// `WGPUVertexStepMode_Vertex`
pub const WGPU_VERTEX_STEP_MODE_VERTEX: i32 = 1;
/// `WGPUVertexStepMode_Instance`（精灵按实例步进：一条实例记录一个精灵）。
pub const WGPU_VERTEX_STEP_MODE_INSTANCE: i32 = 2;
/// `WGPUPrimitiveTopology_TriangleList`
pub const WGPU_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST: i32 = 4;
/// `WGPUIndexFormat_Undefined`
pub const WGPU_INDEX_FORMAT_UNDEFINED: i32 = 0;
/// `WGPUFrontFace_Undefined`
pub const WGPU_FRONT_FACE_UNDEFINED: i32 = 0;
/// `WGPUFrontFace_CCW`（cull 关闭时本无方向语义，但给合法值可避开严格校验；
/// 注意本枚举没有 `None` 成员，次序是 Undefined=0 / CCW=1 / CW=2）。
pub const WGPU_FRONT_FACE_CCW: i32 = 1;
/// `WGPUCullMode_None`
pub const WGPU_CULL_MODE_NONE: i32 = 1;
/// `WGPULoadOp_Clear`
pub const WGPU_LOAD_OP_CLEAR: i32 = 2;
/// `WGPU_DEPTH_SLICE_UNDEFINED`（`UINT32_MAX`；2D 纹理视图的颜色附件必须用它，
/// 填 0 会被判成"给非 3D 视图提供了深度切片"）。
pub const WGPU_DEPTH_SLICE_UNDEFINED: u32 = u32::MAX;
/// `WGPUStoreOp_Store`
pub const WGPU_STORE_OP_STORE: i32 = 1;
/// `WGPUColorWriteMask_All`
pub const WGPU_COLOR_WRITE_MASK_ALL: u64 = 15;
/// `WGPUCallbackMode_AllowProcessEvents`
///
/// 本后端用"轮询模型"（`AllowProcessEvents` + `wgpuInstanceProcessEvents`）而**不是**
/// `WaitAnyOnly`：原型实测 `wgpuDevicePoll` 在这份资产里没有导出，而
/// `wgpuInstanceProcessEvents` 有；轮询模型不需要 `TimedWaitAny` 之外的任何设施，
/// 也不会在缺少 surface 时阻塞事件循环。
pub const WGPU_CALLBACK_MODE_ALLOW_PROCESS_EVENTS: i32 = 2;
/// `WGPUStatus_Success`
///
/// 注意：这里的 `Success` 是 **1** 而不是 0（webgpu.h 的枚举从 1 起算），
/// `wgpuAdapterGetInfo` 的返回值必须按 1 判定；而"回调尚未触发"的哨兵值用 0 —— 
/// 两个都用了同一个 `i32`，弄混就会出现"明明成功却报失败"的假故障。
pub const WGPU_STATUS_SUCCESS: i32 = 1;
/// `WGPURequestAdapterStatus_Success`
pub const WGPU_REQUEST_ADAPTER_STATUS_SUCCESS: i32 = 1;
/// `WGPURequestDeviceStatus_Success`
pub const WGPU_REQUEST_DEVICE_STATUS_SUCCESS: i32 = 1;
/// `WGPUMapAsyncStatus_Success`
pub const WGPU_MAP_ASYNC_STATUS_SUCCESS: i32 = 1;

// ------------------------------------------------------------ 基础结构体

/// `WGPUStringView`：非拥有字符串视图（`data` + `length`，**不保证**以 NUL 结尾）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct StringView {
    /// 指向 UTF-8 字节的指针（可为 null）。
    pub data: *const c_char,
    /// 字节长度（不含 NUL）。
    pub length: usize,
}

impl StringView {
    /// 空视图（`data == null, length == 0`）—— 即 WebGPU 里的"无标签"。
    pub const EMPTY: Self = Self {
        data: core::ptr::null(),
        length: 0,
    };

    /// 由 `'static` 字符串借用构造。
    ///
    /// 只接受 `'static`：视图本身不带生命周期，若允许借用临时字符串，
    /// 调用方很容易在 wgpu 读取前就把它释放掉（这类悬垂在 C ABI 侧不会报错，
    /// 只会表现为随机的乱码标签）。限制为字面量即可根除此类风险。
    pub const fn from_static(s: &'static str) -> Self {
        Self {
            data: s.as_ptr() as *const c_char,
            length: s.len(),
        }
    }

    /// 是否为空视图。
    pub fn is_empty(&self) -> bool {
        self.data.is_null() || self.length == 0
    }

    /// 拷贝为 `String`（按 UTF-8 宽松解码，用于诊断输出）。
    pub fn to_string_lossy(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        // SAFETY: 非空视图的 data/length 由 wgpu-native 给出，指向其内部
        // 在回调期内有效的 UTF-8 缓冲；这里只读取 length 个字节并立即拷贝。
        let bytes = unsafe { core::slice::from_raw_parts(self.data as *const u8, self.length) };
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// `WGPUChainedStruct`：链式扩展头。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct ChainedStruct {
    /// 下一个扩展（本后端一律为 null）。
    pub next: *mut c_void,
    /// 扩展类型标记（`WGPUSType`）。
    pub s_type: i32,
}

/// `WGPUInstanceDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct InstanceDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 必需实例特性个数。
    pub required_feature_count: usize,
    /// 必需实例特性数组指针。
    pub required_features: *const i32,
    /// 必需实例限制（null = 默认）。
    pub required_limits: *const c_void,
}

/// `WGPURequestAdapterOptions`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct RequestAdapterOptions {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// `WGPUFeatureLevel`（0 = Undefined）。
    pub feature_level: i32,
    /// `WGPUPowerPreference`（0 = Undefined）。
    pub power_preference: i32,
    /// 是否强制软件适配器。
    pub force_fallback_adapter: u32,
    /// `WGPUBackendType`（0 = Undefined，即"驱动自选"）。
    pub backend_type: i32,
    /// 兼容的表面（本后端离屏渲染，恒为 null）。
    pub compatible_surface: *mut c_void,
}

/// `WGPUAdapterInfo`（`wgpuAdapterGetInfo` 出参）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct AdapterInfo {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 厂商名。
    pub vendor: StringView,
    /// 架构名。
    pub architecture: StringView,
    /// 设备名。
    pub device: StringView,
    /// 描述串。
    pub description: StringView,
    /// `WGPUBackendType`。
    pub backend_type: i32,
    /// `WGPUAdapterType`。
    pub adapter_type: i32,
    /// PCI 厂商 ID。
    pub vendor_id: u32,
    /// PCI 设备 ID。
    pub device_id: u32,
    /// subgroup 最小宽度。
    pub subgroup_min_size: u32,
    /// subgroup 最大宽度。
    pub subgroup_max_size: u32,
}

/// `WGPUQueueDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct QueueDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
}

/// 通用回调信息（`WGPURequestAdapterCallbackInfo` / `WGPURequestDeviceCallbackInfo` /
/// `WGPUBufferMapCallbackInfo` 三者布局一致：`next + mode + callback + userdata1 + userdata2`）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct CallbackInfo {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// `WGPUCallbackMode`。
    pub mode: i32,
    /// 回调函数指针（签名为各 API 的 `WGPU*Callback`）。
    pub callback: *mut c_void,
    /// 用户数据 1（本后端固定传 `GpuCallbacks` 裸指针）。
    pub userdata1: *mut c_void,
    /// 用户数据 2。
    pub userdata2: *mut c_void,
}

impl CallbackInfo {
    /// 构造"允许在处理事件时触发"的回调信息。
    pub fn process_events(callback: *mut c_void, userdata1: *mut c_void) -> Self {
        Self {
            next_in_chain: core::ptr::null_mut(),
            mode: WGPU_CALLBACK_MODE_ALLOW_PROCESS_EVENTS,
            callback,
            userdata1,
            userdata2: core::ptr::null_mut(),
        }
    }
}

/// `WGPUDeviceLostCallbackInfo`。
///
/// 本后端**不注册** device-lost 回调（`callback = null`）：S4.1 是单帧离屏闭环，
/// 设备丢失会让 `wgpuDeviceCreate*` 返回 null，从而被 [`BackendError::NullHandle`] 抓住，
/// 比异步回调更早、更确定。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct DeviceLostCallbackInfo {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// `WGPUCallbackMode`。
    pub mode: i32,
    /// 回调指针（null = 不注册）。
    pub callback: *mut c_void,
    /// 用户数据 1。
    pub userdata1: *mut c_void,
    /// 用户数据 2。
    pub userdata2: *mut c_void,
}

/// `WGPUUncapturedErrorCallbackInfo`（**注册**：所有驱动侧错误都进 [`crate::gpu::GpuCallbacks`]）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct UncapturedErrorCallbackInfo {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 回调指针。
    pub callback: *mut c_void,
    /// 用户数据 1。
    pub userdata1: *mut c_void,
    /// 用户数据 2。
    pub userdata2: *mut c_void,
}

/// `WGPUDeviceDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct DeviceDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
    /// 必需设备特性个数。
    pub required_feature_count: usize,
    /// 必需设备特性数组。
    pub required_features: *const c_void,
    /// 必需限制（null = 默认）。
    pub required_limits: *const c_void,
    /// 默认队列描述。
    pub default_queue: QueueDescriptor,
    /// device-lost 回调信息。
    pub device_lost_callback_info: DeviceLostCallbackInfo,
    /// 未捕获错误回调信息。
    pub uncaptured_error_callback_info: UncapturedErrorCallbackInfo,
}

/// `WGPUShaderModuleDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct ShaderModuleDescriptor {
    /// 链式扩展（本后端挂 [`ShaderSourceWgsl`]）。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
}

/// `WGPUShaderSourceWGSL`（链在 [`ShaderModuleDescriptor`] 上的 WGSL 源码）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct ShaderSourceWgsl {
    /// 链头（`s_type = WGPU_STYPE_SHADER_SOURCE_WGSL`）。
    pub chain: ChainedStruct,
    /// WGSL 源码。
    pub code: StringView,
}

/// `WGPUVertexAttribute`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct VertexAttribute {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// `WGPUVertexFormat`。
    pub format: i32,
    /// 字节偏移。
    pub offset: u64,
    /// 着色器位置（`@location(n)`）。
    pub shader_location: u32,
}

/// `WGPUVertexBufferLayout`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct VertexBufferLayout {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// `WGPUVertexStepMode`。
    pub step_mode: i32,
    /// 顶点步长（字节）。
    pub array_stride: u64,
    /// 属性个数。
    pub attribute_count: usize,
    /// 属性数组。
    pub attributes: *const VertexAttribute,
}

/// `WGPUVertexState`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct VertexState {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 着色器模块。
    pub module: *mut c_void,
    /// 顶点入口函数名。
    pub entry_point: StringView,
    /// 常量个数。
    pub constant_count: usize,
    /// 常量数组。
    pub constants: *const c_void,
    /// 顶点缓冲布局个数。
    pub buffer_count: usize,
    /// 顶点缓冲布局数组。
    pub buffers: *const VertexBufferLayout,
}

/// `WGPUColorTargetState`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct ColorTargetState {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 纹理格式。
    pub format: i32,
    /// 混合状态（null = 不混合）。
    pub blend: *const c_void,
    /// `WGPUColorWriteMask`。
    pub write_mask: u64,
}

/// `WGPUFragmentState`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct FragmentState {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 着色器模块。
    pub module: *mut c_void,
    /// 片元入口函数名。
    pub entry_point: StringView,
    /// 常量个数。
    pub constant_count: usize,
    /// 常量数组。
    pub constants: *const c_void,
    /// 颜色目标个数。
    pub target_count: usize,
    /// 颜色目标数组。
    pub targets: *const ColorTargetState,
}

/// `WGPUPrimitiveState`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct PrimitiveState {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// `WGPUPrimitiveTopology`。
    pub topology: i32,
    /// strip 索引格式（非 strip 拓扑下为 Undefined）。
    pub strip_index_format: i32,
    /// `WGPUFrontFace`。
    pub front_face: i32,
    /// `WGPUCullMode`。
    pub cull_mode: i32,
    /// 是否禁用深度裁剪。
    pub unclipped_depth: u32,
}

/// `WGPUMultisampleState`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct MultisampleState {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 采样数（1 = 关闭 MSAA）。
    pub count: u32,
    /// 采样掩码。
    pub mask: u32,
    /// 是否开启 alpha-to-coverage。
    pub alpha_to_coverage_enabled: u32,
}

/// `WGPURenderPipelineDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct RenderPipelineDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
    /// 管线布局。
    pub layout: *mut c_void,
    /// 顶点阶段。
    pub vertex: VertexState,
    /// 图元状态。
    pub primitive: PrimitiveState,
    /// 深度模板状态（null = 无）。
    pub depth_stencil: *const c_void,
    /// 多重采样状态。
    pub multisample: MultisampleState,
    /// 片元阶段（null = 无片元阶段）。
    pub fragment: *const FragmentState,
}

/// `WGPUBufferDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct BufferDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
    /// `WGPUBufferUsage`。
    pub usage: u64,
    /// 字节大小。
    pub size: u64,
    /// 是否创建即映射。
    pub mapped_at_creation: u32,
}

/// `WGPUExtent3D`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct Extent3D {
    /// 宽（像素）。
    pub width: u32,
    /// 高（像素）。
    pub height: u32,
    /// 深度或数组层数（2D 恒为 1）。
    pub depth_or_array_layers: u32,
}

impl Extent3D {
    /// 2D 尺寸构造。
    pub const fn rect(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            depth_or_array_layers: 1,
        }
    }
}

/// `WGPUTextureDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct TextureDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
    /// `WGPUTextureUsage`。
    pub usage: u64,
    /// `WGPUTextureDimension`。
    pub dimension: i32,
    /// 尺寸。
    pub size: Extent3D,
    /// 格式。
    pub format: i32,
    /// mip 层数。
    pub mip_level_count: u32,
    /// 采样数。
    pub sample_count: u32,
    /// 视图格式个数。
    pub view_format_count: usize,
    /// 视图格式数组。
    pub view_formats: *const c_void,
}

/// `WGPUOrigin3D`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct Origin3D {
    /// x（像素）。
    pub x: u32,
    /// y（像素）。
    pub y: u32,
    /// z（2D 恒为 0）。
    pub z: u32,
}

/// `WGPUColor`（清屏色等，f64 通道）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct ClearColor {
    /// 红。
    pub r: f64,
    /// 绿。
    pub g: f64,
    /// 蓝。
    pub b: f64,
    /// 透明度。
    pub a: f64,
}

/// `WGPUTexelCopyBufferLayout`（本资产头文件旧名 `WGPUTextureDataLayout`）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct CopyBufferLayout {
    /// 起始字节偏移。
    pub offset: u64,
    /// 每行字节数（256 的倍数约束只在 buffer↔texture 拷贝的 buffer 侧成立）。
    pub bytes_per_row: u32,
    /// 每张图像的层数。
    pub rows_per_image: u32,
}

/// `WGPUTexelCopyBufferInfo`（buffer ↔ texture 拷贝的 buffer 侧）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct CopyBufferInfo {
    /// 缓冲区布局。
    pub layout: CopyBufferLayout,
    /// 缓冲区句柄。
    pub buffer: *mut c_void,
}

/// `WGPUTexelCopyTextureInfo`（buffer ↔ texture 拷贝的 texture 侧）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct CopyTextureInfo {
    /// 纹理句柄。
    pub texture: *mut c_void,
    /// mip 层。
    pub mip_level: u32,
    /// 起始原点。
    pub origin: Origin3D,
    /// `WGPUTextureAspect`。
    pub aspect: i32,
}

/// `WGPURenderPassColorAttachment`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct ColorAttachment {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 目标视图。
    pub view: *mut c_void,
    /// 深度切片。
    pub depth_slice: u32,
    /// resolve 目标（null = 无）。
    pub resolve_target: *mut c_void,
    /// `WGPULoadOp`。
    pub load_op: i32,
    /// `WGPUStoreOp`。
    pub store_op: i32,
    /// 清屏色（`load_op = Clear` 时使用）。
    pub clear_value: ClearColor,
}

/// `WGPURenderPassDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct RenderPassDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
    /// 颜色附件个数。
    pub color_attachment_count: usize,
    /// 颜色附件数组。
    pub color_attachments: *const ColorAttachment,
    /// 深度模板附件（null = 无）。
    pub depth_stencil_attachment: *const c_void,
    /// occlusion query 集合（null = 无）。
    pub occlusion_query_set: *mut c_void,
    /// timestamp 写入配置（null = 无）。
    pub timestamp_writes: *const c_void,
}

/// `WGPUSurfaceSourceWindowsHWND`（链在 [`SurfaceDescriptor`] 上的 HWND 表面源）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SurfaceSourceWindowsHwnd {
    /// 链头（`s_type = SurfaceSourceWindowsHWND`）。
    pub chain: ChainedStruct,
    /// 进程实例句柄（`GetModuleHandleW(null)`）。
    pub hinstance: *mut c_void,
    /// 被包裹的窗口句柄。
    pub hwnd: *mut c_void,
}

/// `WGPUSurfaceDescriptor`（`nextInChain` 挂 HWND 源）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SurfaceDescriptor {
    /// 链式扩展（本后端挂 [`SurfaceSourceWindowsHwnd`]）。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
}

/// `WGPUSurfaceConfiguration`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SurfaceConfiguration {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 渲染到表面的设备。
    pub device: *mut c_void,
    /// 表面纹理格式（须取自 caps）。
    pub format: i32,
    /// 表面纹理用法（本后端：RenderAttachment）。
    pub usage: u64,
    /// 宽（客户区像素）。
    pub width: u32,
    /// 高（客户区像素）。
    pub height: u32,
    /// 视图格式再解释个数（0）。
    pub view_format_count: usize,
    /// 视图格式数组（null）。
    pub view_formats: *const c_void,
    /// 合成 alpha 模式。
    pub alpha_mode: i32,
    /// 呈现模式（Fifo = 垂直同步）。
    pub present_mode: i32,
}

/// `WGPUSurfaceTexture`（`GetCurrentTexture` 的返回）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SurfaceTexture {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 本帧表面纹理（调用方持有，present 后释放）。
    pub texture: *mut c_void,
    /// 获取状态（1/2 = 成功）。
    pub status: i32,
}

/// `WGPUSurfaceCapabilities`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SurfaceCapabilities {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 支持的用法位集。
    pub usages: u64,
    /// 格式数。
    pub format_count: usize,
    /// 格式数组（按驱动偏好排序）。
    pub formats: *const i32,
    /// 呈现模式数。
    pub present_mode_count: usize,
    /// 呈现模式数组。
    pub present_modes: *const i32,
    /// alpha 模式数。
    pub alpha_mode_count: usize,
    /// alpha 模式数组。
    pub alpha_modes: *const i32,
}

/// `WGPUSamplerDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SamplerDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
    /// U 方向寻址模式。
    pub address_mode_u: i32,
    /// V 方向寻址模式。
    pub address_mode_v: i32,
    /// W 方向寻址模式。
    pub address_mode_w: i32,
    /// 放大过滤。
    pub mag_filter: i32,
    /// 缩小过滤。
    pub min_filter: i32,
    /// mip 过滤。
    pub mipmap_filter: i32,
    /// mip LOD 下限。
    pub lod_min_clamp: f32,
    /// mip LOD 上限。
    pub lod_max_clamp: f32,
    /// 比较函数（`Undefined` = 非比较采样器）。
    pub compare: i32,
    /// 各向异性上限。
    pub max_anisotropy: u16,
}

/// `WGPUBufferBindingLayout`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct BufferBindingLayout {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 绑定类型。
    pub binding_type: i32,
    /// 是否有动态偏移。
    pub has_dynamic_offset: u32,
    /// 最小绑定字节数。
    pub min_binding_size: u64,
}

/// `WGPUSamplerBindingLayout`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SamplerBindingLayout {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// `WGPUSamplerBindingType`。
    pub binding_type: i32,
}

/// `WGPUTextureBindingLayout`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct TextureBindingLayout {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// `WGPUTextureSampleType`。
    pub sample_type: i32,
    /// `WGPUTextureViewDimension`。
    pub view_dimension: i32,
    /// 是否多重采样。
    pub multisampled: u32,
}

/// `WGPUStorageTextureBindingLayout`（本后端不使用，保留以对齐尺寸）。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct StorageTextureBindingLayout {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 访问模式。
    pub access: i32,
    /// 格式。
    pub format: i32,
    /// 视图维度。
    pub view_dimension: i32,
}

/// `WGPUBindGroupLayoutEntry`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct BindGroupLayoutEntry {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 绑定号。
    pub binding: u32,
    /// `WGPUShaderStage`。
    pub visibility: u64,
    /// 绑定数组长度。
    pub binding_array_size: u32,
    /// 缓冲区绑定布局。
    pub buffer: BufferBindingLayout,
    /// 采样器绑定布局。
    pub sampler: SamplerBindingLayout,
    /// 纹理绑定布局。
    pub texture: TextureBindingLayout,
    /// 存储纹理绑定布局。
    pub storage_texture: StorageTextureBindingLayout,
}

/// `WGPUBindGroupLayoutDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct BindGroupLayoutDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
    /// 条目个数。
    pub entry_count: usize,
    /// 条目数组。
    pub entries: *const BindGroupLayoutEntry,
}

/// `WGPUBindGroupEntry`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct BindGroupEntry {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 绑定号。
    pub binding: u32,
    /// 缓冲区句柄。
    pub buffer: *mut c_void,
    /// 缓冲区偏移。
    pub offset: u64,
    /// 绑定字节数。
    pub size: u64,
    /// 采样器句柄。
    pub sampler: *mut c_void,
    /// 纹理视图句柄。
    pub texture_view: *mut c_void,
}

/// `WGPUBindGroupDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct BindGroupDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
    /// 绑定的布局。
    pub layout: *mut c_void,
    /// 条目个数。
    pub entry_count: usize,
    /// 条目数组。
    pub entries: *const BindGroupEntry,
}

/// `WGPUPipelineLayoutDescriptor`。
#[derive(Copy, Clone)]
#[repr(C)]
pub struct PipelineLayoutDescriptor {
    /// 链式扩展。
    pub next_in_chain: *mut c_void,
    /// 标签。
    pub label: StringView,
    /// 绑定组布局个数。
    pub bind_group_layout_count: usize,
    /// 绑定组布局数组。
    pub bind_group_layouts: *const *mut c_void,
    /// immediate 数据字节数（0 = 不使用）。
    pub immediate_size: u32,
}

// ------------------------------------------------------------ 各结构体的零值默认
//
// 这些结构体全部是 POD：字段的"未设置"状态在 webgpu.h 里就是 0 / null。
// 用 `mem::zeroed()` 统一生成默认值，避免 40 个结构体各写一份逐字段的 Default
// （逐字段写反而更容易漏字段——漏掉的那个字段会静默变成垃圾值）。

macro_rules! zeroed_defaults {
    ($($t:ty),* $(,)?) => {
        $(
            impl Default for $t {
                fn default() -> Self {
                    // SAFETY: 全部字段都是整数 / 浮点 / 裸指针 / 上述 POD 结构体，
                    // 全零位模式对它们都是合法值（裸指针的 0 即 null）。
                    unsafe { mem::zeroed() }
                }
            }
        )*
    };
}

zeroed_defaults!(
    ChainedStruct,
    InstanceDescriptor,
    RequestAdapterOptions,
    AdapterInfo,
    QueueDescriptor,
    CallbackInfo,
    DeviceLostCallbackInfo,
    UncapturedErrorCallbackInfo,
    DeviceDescriptor,
    ShaderModuleDescriptor,
    ShaderSourceWgsl,
    VertexAttribute,
    VertexBufferLayout,
    VertexState,
    ColorTargetState,
    FragmentState,
    PrimitiveState,
    MultisampleState,
    RenderPipelineDescriptor,
    BufferDescriptor,
    Extent3D,
    TextureDescriptor,
    Origin3D,
    ClearColor,
    CopyBufferLayout,
    CopyBufferInfo,
    CopyTextureInfo,
    ColorAttachment,
    RenderPassDescriptor,
    SamplerDescriptor,
    BufferBindingLayout,
    SamplerBindingLayout,
    TextureBindingLayout,
    StorageTextureBindingLayout,
    BindGroupLayoutEntry,
    BindGroupLayoutDescriptor,
    BindGroupEntry,
    BindGroupDescriptor,
    PipelineLayoutDescriptor,
);

impl Default for StringView {
    fn default() -> Self {
        Self::EMPTY
    }
}

// ------------------------------------------------------------ 回调函数类型

/// `WGPURequestAdapterCallback` / `WGPURequestDeviceCallback` 的函数指针类型
/// （`status`, `对象`, `message`, `userdata1`, `userdata2`）。
pub type ObjectCallback =
    unsafe extern "system" fn(i32, *mut c_void, StringView, *mut c_void, *mut c_void);

/// `WGPUUncapturedErrorCallback`（`device`, `type`, `message`, `userdata1`, `userdata2`）。
///
/// **5 个参数，不是 4 个**：这份 v29 资产的 `webgpu.h` 把 `device` 作为回调首参传入。
/// 该结论由本机实测钉死（`temp\tmp_s41_preflight.py`：故意构造 `sampleCount = 3`
/// 触发驱动错误，按 5 参解析时 message 文本可读；若按 4 参解析，`type` 会被当成
/// `message.data` 去解引用 —— 那是"读到垃圾指针然后崩溃"，而不是"打印错误信息"）。
/// 这类 ABI 偏差不会在编译期暴露，因此本类型在 `criterion_ffi_layout` 之外
/// 还有一条运行时自证：任何一次 `GpuContext::errors_len()` 从 0 变正数，
/// 都说明这条回调真的被驱动调用了。
pub type ErrorCallback =
    unsafe extern "system" fn(*mut c_void, i32, StringView, *mut c_void, *mut c_void);

/// `WGPUBufferMapCallback`（`status`, `message`, `userdata1`, `userdata2`）。
pub type MapCallback = unsafe extern "system" fn(i32, StringView, *mut c_void, *mut c_void);

// ------------------------------------------------------------ 动态库加载

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn GetLastError() -> u32;
    // 不声明 FreeLibrary：动态库按进程生命周期持有，见 NativeLib 的 Drop 说明。
}

/// 已加载的 wgpu-native 动态库。
///
/// 用 `LoadLibraryW` 显式加载（而不是 `#[link]` 隐式链接）的理由：
/// wgpu-native 资产不在系统搜索路径上，路径由调用方给出（`temp\wgpu-win\lib`），
/// 且"库不存在 / 加载失败 / 符号缺失"三种情况都要能被 [`BackendError`] 区分报告。
pub struct NativeLib {
    handle: *mut c_void,
    path: String,
}

impl NativeLib {
    /// 打开动态库。路径不存在时**不**尝试系统搜索（避免"静默用了别的版本"）。
    pub fn open(path: &Path) -> Result<Self, BackendError> {
        if !path.is_file() {
            return Err(BackendError::LibraryNotFound(path.to_path_buf()));
        }
        let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
        wide.push(0);
        // SAFETY: `wide` 是 NUL 结尾的宽字符串，生命周期覆盖本次调用。
        let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            // SAFETY: GetLastError 无前置条件；必须在 LoadLibraryW 之后立刻读，
            // 中间不能再插入任何可能覆盖 last-error 的调用。
            let code = unsafe { GetLastError() };
            return Err(BackendError::LibraryLoad {
                path: path.to_path_buf(),
                code,
            });
        }
        Ok(Self {
            handle,
            path: path.display().to_string(),
        })
    }

    /// 已加载的库路径。
    pub fn path(&self) -> &str {
        &self.path
    }

    /// 解析一个导出符号。
    pub fn symbol(&self, name: &str) -> Result<*mut c_void, BackendError> {
        let cname = std::ffi::CString::new(name)
            .map_err(|_| BackendError::MissingSymbol(format!("{name}（名称含 NUL）")))?;
        // SAFETY: handle 来自 LoadLibraryW 且未释放；cname 是 NUL 结尾字符串。
        let proc = unsafe { GetProcAddress(self.handle, cname.as_ptr()) };
        if proc.is_null() {
            Err(BackendError::MissingSymbol(name.to_string()))
        } else {
            Ok(proc)
        }
    }
}

impl Drop for NativeLib {
    fn drop(&mut self) {
        // 刻意**不** FreeLibrary：wgpu-native 与 Vulkan 加载器会创建自己的线程
        // 和 TLS 状态，把 DLL 完全卸载后再次加载，实机上会以 0xC000041D
        // （致命回调异常）或访问违例崩溃 —— S4.1 测试序列里"逐用例开/关上下文"
        // 稳定复现，并行时序下偶发。渲染后端按**进程生命周期**持有动态库是
        // 引擎侧惯例（驱动 DLL 同理）：句柄与线程由操作系统在进程退出时回收，
        // "重复装配后端"因此从危险操作变回普通操作。
        let _handle = self.handle;
    }
}

impl std::fmt::Debug for NativeLib {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeLib").field("path", &self.path).finish()
    }
}

/// 把导出符号转换为具体函数指针类型。
///
/// `F` 由调用点（[`WgpuApi`] 的字段类型）反推，因此这里不需要逐个符号写参数表 ——
/// 参数表只写一遍，就在 [`WgpuApi`] 的声明里。
fn resolve<F: Copy>(lib: &NativeLib, name: &str) -> Result<F, BackendError> {
    let proc = lib.symbol(name)?;
    debug_assert_eq!(mem::size_of::<F>(), mem::size_of::<*mut c_void>());
    // SAFETY: 函数指针与数据指针在本机（Windows x64）同为 8 字节且调用约定一致；
    // `F` 的每个类型都在下方宏里显式写出（`extern "system"` 是 Windows ABI 的正确约定），
    // 与原型脚本 `argtypes` 表逐条对应。
    Ok(unsafe { mem::transmute_copy::<*mut c_void, F>(&proc) })
}

/// 声明整个 wgpu-native 函数表：一次写出"字段名 + 符号名 + 参数表"，
/// 同时生成结构体与加载逻辑，避免声明与解析两处漂移。
macro_rules! wgpu_api {
    ($( $field:ident : $symbol:literal , ( $($arg:ty),* $(,)? ) $(-> $ret:ty)? ; )*) => {
        /// 已解析的 wgpu-native 函数表。
        ///
        /// 字段类型即 `extern "system"` 函数指针；参数表与原型脚本的 `argtypes` 一致。
        /// 任何符号缺失都会在 [`WgpuApi::load`] 阶段以 [`BackendError::MissingSymbol`] 指名报错。
        pub struct WgpuApi {
            $(
                #[allow(missing_docs)]
                pub $field: unsafe extern "system" fn($($arg),*) $(-> $ret)?,
            )*
        }

        impl WgpuApi {
            /// 从已加载的动态库解析全部符号。
            pub fn load(lib: &NativeLib) -> Result<Self, BackendError> {
                Ok(Self {
                    $( $field: resolve(lib, $symbol)?, )*
                })
            }

            /// 本后端依赖的符号名清单（诊断用：能一眼看出"资产缺了哪个符号"）。
            pub fn required_symbols() -> &'static [&'static str] {
                &[ $( $symbol, )* ]
            }
        }
    };
}

wgpu_api! {
    create_instance: "wgpuCreateInstance", (*const InstanceDescriptor) -> *mut c_void;
    instance_request_adapter: "wgpuInstanceRequestAdapter",
        (*mut c_void, *const RequestAdapterOptions, CallbackInfo) -> u64;
    instance_process_events: "wgpuInstanceProcessEvents", (*mut c_void);
    adapter_get_info: "wgpuAdapterGetInfo", (*mut c_void, *mut AdapterInfo) -> i32;
    adapter_request_device: "wgpuAdapterRequestDevice",
        (*mut c_void, *const DeviceDescriptor, CallbackInfo) -> u64;
    device_get_queue: "wgpuDeviceGetQueue", (*mut c_void) -> *mut c_void;
    device_create_shader_module: "wgpuDeviceCreateShaderModule",
        (*mut c_void, *const ShaderModuleDescriptor) -> *mut c_void;
    device_create_buffer: "wgpuDeviceCreateBuffer",
        (*mut c_void, *const BufferDescriptor) -> *mut c_void;
    device_create_texture: "wgpuDeviceCreateTexture",
        (*mut c_void, *const TextureDescriptor) -> *mut c_void;
    device_create_sampler: "wgpuDeviceCreateSampler",
        (*mut c_void, *const SamplerDescriptor) -> *mut c_void;
    device_create_bind_group_layout: "wgpuDeviceCreateBindGroupLayout",
        (*mut c_void, *const BindGroupLayoutDescriptor) -> *mut c_void;
    device_create_pipeline_layout: "wgpuDeviceCreatePipelineLayout",
        (*mut c_void, *const PipelineLayoutDescriptor) -> *mut c_void;
    device_create_render_pipeline: "wgpuDeviceCreateRenderPipeline",
        (*mut c_void, *const RenderPipelineDescriptor) -> *mut c_void;
    device_create_bind_group: "wgpuDeviceCreateBindGroup",
        (*mut c_void, *const BindGroupDescriptor) -> *mut c_void;
    device_create_command_encoder: "wgpuDeviceCreateCommandEncoder",
        (*mut c_void, *const c_void) -> *mut c_void;
    texture_create_view: "wgpuTextureCreateView", (*mut c_void, *const c_void) -> *mut c_void;
    command_encoder_begin_render_pass: "wgpuCommandEncoderBeginRenderPass",
        (*mut c_void, *const RenderPassDescriptor) -> *mut c_void;
    command_encoder_finish: "wgpuCommandEncoderFinish",
        (*mut c_void, *const c_void) -> *mut c_void;
    command_encoder_copy_texture_to_buffer: "wgpuCommandEncoderCopyTextureToBuffer",
        (*mut c_void, *const CopyTextureInfo, *const CopyBufferInfo, *const Extent3D);
    render_pass_encoder_set_pipeline: "wgpuRenderPassEncoderSetPipeline",
        (*mut c_void, *mut c_void);
    render_pass_encoder_set_bind_group: "wgpuRenderPassEncoderSetBindGroup",
        (*mut c_void, u32, *mut c_void, usize, *const c_void);
    render_pass_encoder_set_vertex_buffer: "wgpuRenderPassEncoderSetVertexBuffer",
        (*mut c_void, u32, *mut c_void, u64, u64);
    render_pass_encoder_draw: "wgpuRenderPassEncoderDraw",
        (*mut c_void, u32, u32, u32, u32);
    render_pass_encoder_end: "wgpuRenderPassEncoderEnd", (*mut c_void);
    queue_submit: "wgpuQueueSubmit", (*mut c_void, usize, *const *mut c_void);
    queue_write_buffer: "wgpuQueueWriteBuffer",
        (*mut c_void, *mut c_void, u64, *const c_void, usize);
    queue_write_texture: "wgpuQueueWriteTexture",
        (*mut c_void, *const CopyTextureInfo, *const c_void, usize, *const CopyBufferLayout, *const Extent3D);
    buffer_map_async: "wgpuBufferMapAsync",
        (*mut c_void, u64, usize, usize, CallbackInfo) -> u64;
    buffer_get_mapped_range: "wgpuBufferGetMappedRange", (*mut c_void, usize, usize) -> *mut c_void;
    buffer_unmap: "wgpuBufferUnmap", (*mut c_void);
    buffer_release: "wgpuBufferRelease", (*mut c_void);
    instance_create_surface: "wgpuInstanceCreateSurface",
        (*mut c_void, *const SurfaceDescriptor) -> *mut c_void;
    surface_configure: "wgpuSurfaceConfigure",
        (*mut c_void, *const SurfaceConfiguration);
    surface_get_capabilities: "wgpuSurfaceGetCapabilities",
        (*mut c_void, *mut c_void, *mut SurfaceCapabilities) -> i32;
    surface_capabilities_free_members: "wgpuSurfaceCapabilitiesFreeMembers",
        (SurfaceCapabilities);
    surface_get_current_texture: "wgpuSurfaceGetCurrentTexture",
        (*mut c_void, *mut SurfaceTexture);
    surface_present: "wgpuSurfacePresent", (*mut c_void) -> i32;
    surface_unconfigure: "wgpuSurfaceUnconfigure", (*mut c_void);
    surface_release: "wgpuSurfaceRelease", (*mut c_void);
    texture_release: "wgpuTextureRelease", (*mut c_void);
    texture_view_release: "wgpuTextureViewRelease", (*mut c_void);
    sampler_release: "wgpuSamplerRelease", (*mut c_void);
    shader_module_release: "wgpuShaderModuleRelease", (*mut c_void);
    bind_group_layout_release: "wgpuBindGroupLayoutRelease", (*mut c_void);
    bind_group_release: "wgpuBindGroupRelease", (*mut c_void);
    pipeline_layout_release: "wgpuPipelineLayoutRelease", (*mut c_void);
    render_pipeline_release: "wgpuRenderPipelineRelease", (*mut c_void);
    command_encoder_release: "wgpuCommandEncoderRelease", (*mut c_void);
    render_pass_encoder_release: "wgpuRenderPassEncoderRelease", (*mut c_void);
    command_buffer_release: "wgpuCommandBufferRelease", (*mut c_void);
    queue_release: "wgpuQueueRelease", (*mut c_void);
    device_release: "wgpuDeviceRelease", (*mut c_void);
    adapter_release: "wgpuAdapterRelease", (*mut c_void);
    instance_release: "wgpuInstanceRelease", (*mut c_void);
}
