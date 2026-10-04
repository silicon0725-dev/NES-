//! 运行时装配：动态库定位 → 实例/适配器/设备/队列 → 离屏目标与像素读回 → 精灵图集。
//!
//! # 异步模型：为什么是「轮询 + ProcessEvents」
//!
//! 这份 v29 资产里 `wgpuDevicePoll` **没有导出**（原型实测），而
//! `wgpuInstanceProcessEvents` 有。因此适配器/设备/缓冲映射三条异步路径统一采用
//! `WGPUCallbackMode_AllowProcessEvents` + 主动轮询：回调把结果写进 [`GpuCallbacks`]
//! 的槽位，`pump` 循环推进事件直到槽位被填或超时。副作用是**不需要 `TimedWaitAny`**，
//! 也就不会在没有 surface 的离屏场景里被事件循环卡住。
//!
//! # userdata 的生命周期（裸指针安全性的唯一理由）
//!
//! 所有回调都把 `userdata1` 当作 `*const GpuCallbacks` 解引用。安全性来自
//! [`GpuContext`] 的析构顺序：`callbacks` 是 `Arc`，既被 `GpuContext` 持有、也被
//! 回调引用；`Drop` 先释放 GPU 句柄、最后才让 `callbacks` 随结构体析构 ——
//! 释放句柄期间可能同步触发的 `uncaptured_error` 回调仍然看见一个活着的
//! [`GpuCallbacks`]。
//!
//! # 为什么离屏
//!
//! S4.1 只验证「命令 → 像素」这条链。窗口与 surface 属于下一段，混进来会让
//! 「渲染对不对」和「窗口系统好不好用」两个独立问题互相掩盖。

use core::ffi::c_void;
use core::ptr;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nes_render_api::RenderAssetKey;

use crate::error::BackendError;
use crate::ffi::{self, CallbackInfo, Extent3D, NativeLib, StringView, WgpuApi};

/// 等待异步回调的上限（远超单帧耗时；超时即视为驱动侧卡死并如实报错）。
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(10);

/// 事件轮询间隔（1ms：足够让回调推进，又不至于把单帧拖成忙等）。
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// 纹理 → 缓冲拷贝时每行字节数必须按 256 对齐（WebGPU 规范硬约束，与图有多宽无关：
/// 64 像素宽的 RGBA 行正好 256 字节，128 像素宽就要补到 512）。
const COPY_ROW_ALIGNMENT: u32 = 256;

// ------------------------------------------------------------ 图集常量

/// 图集边长（像素）。64 = 4 格 x 16px：16 格总量、单格与原型精灵同尺寸，
/// 且 `64 * 4 = 256` 字节/行恰好满足纹理上传的 256 字节对齐。
pub const ATLAS_PX: u32 = 64;
/// 图集每边的格数（总计 `ATLAS_CELLS * ATLAS_CELLS = 16` 格）。
pub const ATLAS_CELLS: u32 = 4;
/// 单格边长（像素）—— 与原型 `s41_probe3.log` 的 16x16 精灵同尺寸。
pub const CELL_PX: u32 = ATLAS_PX / ATLAS_CELLS;
/// 单格字节数。
pub const CELL_BYTES: usize = (CELL_PX * CELL_PX * 4) as usize;

/// 精灵底色（红）—— 对应原型日志 `px(10,10) = (255, 0, 0, 255)`。
pub const SPRITE_BODY_COLOR: [u8; 4] = [255, 0, 0, 255];

/// 精灵「眼睛」色（近白）—— 对应原型日志 `px(13,13) = (250, 250, 250, 255)`。
///
/// 用 250 而不是 255：纯白很容易和清屏或未初始化区域混淆，而 `(250, 250, 250)`
/// 是一个只可能来自本图案的值，断言它才有区分度。
pub const SPRITE_EYE_COLOR: [u8; 4] = [250, 250, 250, 255];

/// 图集其余格的填充色（品红）—— 刻意选一个不会出现在别处的颜色。
///
/// 若 UV / 格号算错而采样到了相邻格，画面上会立刻出现品红像素；于是
/// 「精灵画出来了」这件事同时证明了「UV 落点正确」，而不是只证明「有东西被画出来」。
pub const ATLAS_FILLER_COLOR: [u8; 4] = [255, 0, 255, 255];

/// 精灵「眼睛」在单格内的局部像素坐标（列, 行）。
///
/// 取 `(3, 3)` 是为了让本后端的抽样点与原型**完全重合**：精灵以
/// `translation(10, 10)` 落在画面上时，全局 `(10,10)` 即局部 `(0,0)`（红），
/// 全局 `(13,13)` 即局部 `(3,3)`（白）—— 与 `s41_probe3.log` 中
/// `px(10,10)` 红 / `px(13,13)` 白 的两个断言一一对应。
pub const SPRITE_EYE_OFFSET: (u32, u32) = (3, 3);

/// 控件边框色（绿）—— `SetRect` 的 HUD 可视化：1px 边框 + 透明内部
/// （配合片段着色器的 alpha 丢弃，无混合也能"透出"下层像素）。
/// 图案格的中性基底（E-1：颜色一律经实例 tint 相乘进入，图案本身不带色）。
pub const NEUTRAL_WHITE: [u8; 4] = [255, 255, 255, 255];

/// E-1 之前的边框观感（绿色哨兵）—— 已上移为契约缺省
/// `nes_render_api::state::CONTROL_BORDER_LEGACY`，此处保留仅为历史对照。
pub const CONTROL_FRAME_COLOR: [u8; 4] = [0, 255, 0, 255];
/// 控件边框图案所在格号（格 0 = 精灵图案，格 1 = 控件边框，格 2 = 纯色
/// 填充，格 3~15 = 品红哨兵 —— E-1 颜色通道，S12.1）。
pub const CONTROL_CELL: u32 = 1;

/// 纯色填充图案所在格号（中性白 —— 颜色经实例 tint 进入）。
pub const FILL_CELL: u32 = 2;

/// 构造整张精灵图集：第 0 格是「红底 + 一个白眼」，第 1 格是控件边框
///（**中性白**框 + 透明内部），第 2 格是中性白纯色填充，其余格是纯品红
/// 哨兵色。
///
/// E-1 起图案格一律**中性白**：颜色经实例 tint 相乘进入（绿框观感由
/// 契约缺省 `CONTROL_BORDER_LEGACY` 以 tint 复现，逐位同前）。
/// 只让前三格带图案、其余格是哨兵色，「采样到了哪一格」在像素层面
/// 才是可判定的（见 [`ATLAS_FILLER_COLOR`]）。
pub fn build_sprite_sheet() -> Vec<u8> {
    let mut sheet = vec![0u8; (ATLAS_PX * ATLAS_PX * 4) as usize];
    for pixel in sheet.chunks_exact_mut(4) {
        pixel.copy_from_slice(&ATLAS_FILLER_COLOR);
    }
    let cell0 = render_sprite_cell();
    let cell1 = control_frame_cell();
    let cell2 = fill_cell();
    let row_bytes = (CELL_PX * 4) as usize;
    for row in 0..CELL_PX {
        let dst = (row * ATLAS_PX * 4) as usize;
        let src = (row as usize) * row_bytes;
        sheet[dst..dst + row_bytes].copy_from_slice(&cell0[src..src + row_bytes]);
        let dst1 = dst + (CELL_PX * 4) as usize;
        sheet[dst1..dst1 + row_bytes].copy_from_slice(&cell1[src..src + row_bytes]);
        let dst2 = dst1 + row_bytes;
        sheet[dst2..dst2 + row_bytes].copy_from_slice(&cell2[src..src + row_bytes]);
    }
    sheet
}

/// 单格精灵图案：整格红底 + `(3,3)` 处一个白眼（其余格由 [`build_sprite_sheet`] 填哨兵色）。
pub fn render_sprite_cell() -> Vec<u8> {
    let mut cell = Vec::with_capacity(CELL_BYTES);
    for y in 0..CELL_PX {
        for x in 0..CELL_PX {
            let color = if (x, y) == SPRITE_EYE_OFFSET {
                SPRITE_EYE_COLOR
            } else {
                SPRITE_BODY_COLOR
            };
            cell.extend_from_slice(&color);
        }
    }
    cell
}

/// 单格控件边框图案：1px 中性白框 + 透明内部（内部经片段着色器的
/// alpha 丢弃；颜色经实例 tint 进入）。
pub fn control_frame_cell() -> Vec<u8> {
    let mut cell = Vec::with_capacity(CELL_BYTES);
    for y in 0..CELL_PX {
        for x in 0..CELL_PX {
            let border = x == 0 || y == 0 || x == CELL_PX - 1 || y == CELL_PX - 1;
            cell.extend_from_slice(if border {
                &NEUTRAL_WHITE
            } else {
                &[0, 0, 0, 0]
            });
        }
    }
    cell
}

/// 单格纯色填充图案：整格中性白（颜色经实例 tint 进入）。
pub fn fill_cell() -> Vec<u8> {
    let mut cell = Vec::with_capacity(CELL_BYTES);
    for _ in 0..(CELL_PX * CELL_PX) {
        cell.extend_from_slice(&NEUTRAL_WHITE);
    }
    cell
}

// ------------------------------------------------------------ 回调与观测

/// 异步回调的结果槽（回调线程写、轮询线程取；`status` 兼作就绪标志）。
///
/// 就绪标志与结果值分开存：`status` 用 `Release` 最后写、用 `AcqRel` 交换读取，
/// 保证读到非零 `status` 时 `object` / `message` 一定已经写完。`status == 0`
/// 作为「未就绪」哨兵是安全的 —— WebGPU 的 `WGPUStatus` 从 1 起算，永远不会是 0。
#[derive(Default)]
pub(crate) struct CallbackCell {
    status: AtomicI32,
    object: AtomicU64,
    message: Mutex<String>,
}

/// 一次异步调用的结果快照。
pub(crate) struct CallbackOutcome {
    /// 原始状态码（`WGPURequestAdapterStatus` 等）。
    pub(crate) status: i32,
    /// 回调带出的对象指针（adapter / device；缓冲映射时为 0）。
    pub(crate) object: usize,
    /// 驱动回传的诊断串（未取到则为空串）。
    pub(crate) message: String,
}

impl CallbackCell {
    /// 回调侧写入结果（`status` 最后写）。
    fn publish(&self, status: i32, object: usize, message: String) {
        self.object.store(object as u64, Ordering::Release);
        if let Ok(mut slot) = self.message.lock() {
            *slot = message;
        }
        self.status.store(status, Ordering::Release);
    }

    /// 轮询侧取走结果；未就绪返回 `None`。
    ///
    /// 取走即清空，因此**不允许**「探一下再丢掉」的用法：调用方要么立刻用掉，
    /// 要么根本不该调用。
    fn take(&self) -> Option<CallbackOutcome> {
        let status = self.status.swap(0, Ordering::AcqRel);
        if status == 0 {
            return None;
        }
        let object = self.object.swap(0, Ordering::AcqRel) as usize;
        let message = match self.message.lock() {
            Ok(mut slot) => core::mem::take(&mut *slot),
            Err(poisoned) => core::mem::take(&mut *poisoned.into_inner()),
        };
        Some(CallbackOutcome {
            status,
            object,
            message,
        })
    }
}

/// 回调上下文：wgpu-native 把 `userdata1` **原样传回**，于是裸指针在这里变成
/// 三个独立槽位 + 三个观测计数。
///
/// 槽位分开（请求槽 / 映射槽）的理由：适配器请求与缓冲映射的生命周期互不重叠，
/// 但共用一个槽会让「谁把状态写花了」无法从证据上分辨。
#[derive(Default)]
pub(crate) struct GpuCallbacks {
    request: CallbackCell,
    map: CallbackCell,
    errors: Mutex<Vec<String>>,
    error_calls: AtomicU64,
    object_calls: AtomicU64,
    map_calls: AtomicU64,
}

impl GpuCallbacks {
    fn push_error(&self, message: String) {
        if let Ok(mut entries) = self.errors.lock() {
            // 只留前 16 条：驱动一旦开始刷错误，后面的重复没有信息量。
            if entries.len() < 16 {
                entries.push(message);
            }
        }
    }

    fn errors_len(&self) -> usize {
        self.errors.lock().map(|e| e.len()).unwrap_or(0)
    }

    fn errors_snapshot(&self) -> Vec<String> {
        self.errors.lock().map(|e| e.clone()).unwrap_or_default()
    }
}

/// `WGPURequestAdapterCallback` / `WGPURequestDeviceCallback` 的观测实现。
unsafe extern "system" fn on_object(
    status: i32,
    object: *mut c_void,
    message: ffi::StringView,
    userdata1: *mut c_void,
    _userdata2: *mut c_void,
) {
    if userdata1.is_null() {
        return;
    }
    let callbacks = &*(userdata1 as *const GpuCallbacks);
    callbacks.object_calls.fetch_add(1, Ordering::Relaxed);
    callbacks
        .request
        .publish(status, object as usize, message.to_string_lossy());
}

/// `WGPUBufferMapCallback`（注意参数表与对象回调不同：没有对象指针）。
unsafe extern "system" fn on_map(
    status: i32,
    message: ffi::StringView,
    userdata1: *mut c_void,
    _userdata2: *mut c_void,
) {
    if userdata1.is_null() {
        return;
    }
    let callbacks = &*(userdata1 as *const GpuCallbacks);
    callbacks.map_calls.fetch_add(1, Ordering::Relaxed);
    callbacks.map.publish(status, 0, message.to_string_lossy());
}

/// `WGPUUncapturedErrorCallback`（首参是 `device`，共 5 参 —— 见 `ffi.rs` 的实测注释）。
unsafe extern "system" fn on_error(
    _device: *mut c_void,
    _error_type: i32,
    message: ffi::StringView,
    userdata1: *mut c_void,
    _userdata2: *mut c_void,
) {
    if userdata1.is_null() {
        return;
    }
    let callbacks = &*(userdata1 as *const GpuCallbacks);
    callbacks.error_calls.fetch_add(1, Ordering::Relaxed);
    callbacks.push_error(message.to_string_lossy());
}

// ------------------------------------------------------------ 动态库定位

/// 按固定顺序定位 wgpu-native 动态库。
///
/// 候选顺序（前一个不存在就看下一个，**不猜版本**）：
/// 1. 环境变量 `NES_RENDER_WGPU_LIB`（显式覆盖，最高优先级）；
/// 2. `<crate>/../wgpu-win/lib/wgpu_native.dll`（S4.1 资产落点）；
/// 3. `<crate>/../../temp/wgpu-win/lib/wgpu_native.dll`（会话中间产物目录）；
/// 4. 工作目录下的 `wgpu_native.dll`。
///
/// 全部不存在时返回 [`BackendError::NoLibraryCandidates`] 并列出已尝试的路径 ——
/// 这正是「如实报告阻塞点」要求的形态：宁可报告「没找到」，也不静默换一个版本。
pub fn locate_library() -> Result<PathBuf, BackendError> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(explicit) = std::env::var_os("NES_RENDER_WGPU_LIB") {
        candidates.push(PathBuf::from(explicit));
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest.join("../wgpu-win/lib/wgpu_native.dll"));
    candidates.push(manifest.join("../../temp/wgpu-win/lib/wgpu_native.dll"));
    candidates.push(PathBuf::from("wgpu_native.dll"));

    let mut tried: Vec<String> = Vec::new();
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
        tried.push(candidate.display().to_string());
    }
    Err(BackendError::NoLibraryCandidates(tried.join(" ; ")))
}

// ------------------------------------------------------------ 设备身份

/// 适配器 / 后端身份（诊断与证据用：报告里要能指名道姓说清是哪个后端出的帧）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    /// 适配器名称（如显卡型号）。
    pub device: String,
    /// 厂商名。
    pub vendor: String,
    /// 架构描述串。
    pub architecture: String,
    /// 补充描述。
    pub description: String,
    /// `WGPUBackendType` 原始值（4 = D3D12）。
    pub backend_type: i32,
    /// `WGPUAdapterType` 原始值（1 = DiscreteGPU）。
    pub adapter_type: i32,
    /// PCI 厂商 ID。
    pub vendor_id: u32,
    /// PCI 设备 ID。
    pub device_id: u32,
}

impl DeviceInfo {
    /// `WGPUAdapterType` 的可读名（数值取自资产头文件实测）。
    pub fn adapter_type_name(&self) -> &'static str {
        match self.adapter_type {
            1 => "DiscreteGPU",
            2 => "IntegratedGPU",
            3 => "CPU",
            4 => "Unknown",
            _ => "Undefined",
        }
    }

    /// `WGPUBackendType` 的可读名（数值取自资产头文件实测）。
    pub fn backend_type_name(&self) -> &'static str {
        match self.backend_type {
            0 => "Undefined",
            1 => "Null",
            2 => "WebGPU",
            3 => "D3D11",
            4 => "D3D12",
            5 => "Metal",
            6 => "Vulkan",
            7 => "OpenGL",
            8 => "OpenGLES",
            _ => "Force32",
        }
    }
}

// ------------------------------------------------------------ GpuContext

/// 一个已装配好的 wgpu-native 设备上下文（实例 → 适配器 → 设备 → 队列）。
pub struct GpuContext {
    lib: NativeLib,
    api: WgpuApi,
    instance: *mut c_void,
    adapter: *mut c_void,
    device: *mut c_void,
    queue: *mut c_void,
    callbacks: Arc<GpuCallbacks>,
    info: DeviceInfo,
}

impl GpuContext {
    /// 用 [`locate_library`] 找到的库装配上下文。
    pub fn open() -> Result<Self, BackendError> {
        let path = locate_library()?;
        Self::open_at(&path)
    }

    /// 用调用方给定的动态库路径装配上下文。
    pub fn open_at(lib_path: &Path) -> Result<Self, BackendError> {
        let lib = NativeLib::open(lib_path)?;
        let api = WgpuApi::load(&lib)?;
        let callbacks = Arc::new(GpuCallbacks::default());
        let userdata = Arc::as_ptr(&callbacks) as *mut c_void;

        let instance_desc = ffi::InstanceDescriptor::default();
        // SAFETY: `api` 的函数表在 `lib` 存活期间有效，而 `lib` 被 ctx 持有。
        let instance = unsafe { (api.create_instance)(&instance_desc) };
        if instance.is_null() {
            return Err(BackendError::NullHandle("WGPUInstance"));
        }

        let mut ctx = Self {
            lib,
            api,
            instance,
            adapter: ptr::null_mut(),
            device: ptr::null_mut(),
            queue: ptr::null_mut(),
            callbacks,
            info: DeviceInfo::default(),
        };

        // 任一步失败时，已创建的句柄由 `ctx` 的 Drop 释放（顺序与创建相反）。
        let adapter = ctx.request_adapter(userdata)?;
        ctx.adapter = adapter;
        ctx.info = ctx.read_adapter_info();
        let (device, queue) = ctx.request_device(userdata)?;
        ctx.device = device;
        ctx.queue = queue;
        Ok(ctx)
    }

    /// 加载使用的库路径。
    pub fn library_path(&self) -> &str {
        self.lib.path()
    }

    /// 是否离屏（当前恒为 `true`：窗口/表面接入属于 S4 下一段）。
    pub fn headless(&self) -> bool {
        true
    }

    /// 适配器/后端身份。
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// 驱动侧未捕获错误条数（每帧结束都应核对它，别让错误在后台堆积）。
    pub fn errors_len(&self) -> usize {
        self.callbacks.errors_len()
    }

    /// 驱动侧未捕获错误快照。
    pub fn errors_snapshot(&self) -> Vec<String> {
        self.callbacks.errors_snapshot()
    }

    /// 推进一次驱动事件（`wgpuInstanceProcessEvents`），让排队中的异步回调有机会执行。
    pub fn flush_events(&self) {
        // SAFETY: instance 非空且存活。
        unsafe { (self.api.instance_process_events)(self.instance) };
    }

    pub(crate) fn api(&self) -> &WgpuApi {
        &self.api
    }

    pub(crate) fn device(&self) -> *mut c_void {
        self.device
    }

    pub(crate) fn queue(&self) -> *mut c_void {
        self.queue
    }

    /// 实例句柄（surface 创建用，S6.1）。
    pub(crate) fn instance_handle(&self) -> *mut c_void {
        self.instance
    }

    /// 适配器句柄（surface 能力查询用，S6.1）。
    pub(crate) fn adapter_handle(&self) -> *mut c_void {
        self.adapter
    }

    pub(crate) fn callbacks_ptr(&self) -> *mut c_void {
        Arc::as_ptr(&self.callbacks) as *mut c_void
    }

    fn read_adapter_info(&self) -> DeviceInfo {
        let mut raw = ffi::AdapterInfo::default();
        let status = unsafe { (self.api.adapter_get_info)(self.adapter, &mut raw) };
        if status != ffi::WGPU_STATUS_SUCCESS {
            return DeviceInfo::default();
        }
        // 适配器信息里的字符串是**借来的**视图，必须在释放适配器之前拷成 Rust 串。
        DeviceInfo {
            device: raw.device.to_string_lossy(),
            vendor: raw.vendor.to_string_lossy(),
            architecture: raw.architecture.to_string_lossy(),
            description: raw.description.to_string_lossy(),
            backend_type: raw.backend_type,
            adapter_type: raw.adapter_type,
            vendor_id: raw.vendor_id,
            device_id: raw.device_id,
        }
    }

    /// 请求适配器（本机默认：不强制后端、不要求回退适配器 —— 与原型同路径）。
    fn request_adapter(&self, userdata: *mut c_void) -> Result<*mut c_void, BackendError> {
        // feature_level / power_preference / force_fallback_adapter / backend_type
        // 全部保留 Undefined：让驱动自己挑，而不是本后端替它挑。
        let options = ffi::RequestAdapterOptions {
            backend_type: 0,
            ..Default::default()
        };

        let callback: ffi::ObjectCallback = on_object;
        let info = CallbackInfo::process_events(callback as *mut c_void, userdata);
        unsafe { (self.api.instance_request_adapter)(self.instance, &options, info) };

        let outcome = self.pump(&self.callbacks.request, "wgpuInstanceRequestAdapter")?;
        if outcome.status != ffi::WGPU_REQUEST_ADAPTER_STATUS_SUCCESS || outcome.object == 0 {
            return Err(BackendError::AdapterRequestFailed {
                status: outcome.status,
                message: outcome.message,
            });
        }
        Ok(outcome.object as *mut c_void)
    }

    /// 请求设备（顺带挂上未捕获错误回调，让驱动错误进证据链）。
    fn request_device(
        &self,
        userdata: *mut c_void,
    ) -> Result<(*mut c_void, *mut c_void), BackendError> {
        let error_callback: ffi::ErrorCallback = on_error;
        let error_info = ffi::UncapturedErrorCallbackInfo {
            callback: error_callback as *mut c_void,
            userdata1: userdata,
            ..Default::default()
        };

        let desc = ffi::DeviceDescriptor {
            default_queue: ffi::QueueDescriptor::default(),
            device_lost_callback_info: ffi::DeviceLostCallbackInfo::default(),
            uncaptured_error_callback_info: error_info,
            ..Default::default()
        };

        let callback: ffi::ObjectCallback = on_object;
        let info = CallbackInfo::process_events(callback as *mut c_void, userdata);
        unsafe { (self.api.adapter_request_device)(self.adapter, &desc, info) };

        let outcome = self.pump(&self.callbacks.request, "wgpuAdapterRequestDevice")?;
        if outcome.status != ffi::WGPU_REQUEST_DEVICE_STATUS_SUCCESS || outcome.object == 0 {
            return Err(BackendError::DeviceRequestFailed {
                status: outcome.status,
                message: outcome.message,
            });
        }
        let device = outcome.object as *mut c_void;
        let queue = unsafe { (self.api.device_get_queue)(device) };
        if queue.is_null() {
            unsafe { (self.api.device_release)(device) };
            return Err(BackendError::NullHandle("WGPUQueue"));
        }
        Ok((device, queue))
    }

    /// 轮询事件直到 `cell` 就绪或超时。
    fn pump(
        &self,
        cell: &CallbackCell,
        tag: &'static str,
    ) -> Result<CallbackOutcome, BackendError> {
        let deadline = Instant::now() + CALLBACK_TIMEOUT;
        loop {
            if let Some(outcome) = cell.take() {
                return Ok(outcome);
            }
            if Instant::now() >= deadline {
                return Err(BackendError::Timeout(tag));
            }
            self.flush_events();
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// 轮询缓冲映射回调。
    fn pump_map(&self, tag: &'static str) -> Result<CallbackOutcome, BackendError> {
        self.pump(&self.callbacks.map, tag)
    }
}

impl Drop for GpuContext {
    fn drop(&mut self) {
        // SAFETY: 每个句柄只在非空时释放一次，顺序与创建相反
        // （queue → device → adapter → instance）。`callbacks` 必须活过这些释放：
        // 释放过程可能同步触发 uncaptured_error 回调，那里会写进 callbacks。
        unsafe {
            if !self.queue.is_null() {
                (self.api.queue_release)(self.queue);
            }
            if !self.device.is_null() {
                (self.api.device_release)(self.device);
            }
            if !self.adapter.is_null() {
                (self.api.adapter_release)(self.adapter);
            }
            if !self.instance.is_null() {
                (self.api.instance_release)(self.instance);
            }
        }
    }
}

// ------------------------------------------------------------ 帧图像

/// 一帧读回后的 CPU 侧像素。
///
/// 存储即 `wgpuBufferGetMappedRange` 的原始字节：目标是 `RGBA8Unorm` 纹理，
/// 字节序就是 RGBA（S4.1 实机验证：按 BGRA 假设交换 R/B 会让红精灵读成蓝、
/// 深藏青背景读成暗红，与原型日志锚点相反）。语义访问统一走 [`FrameImage::pixel`]，
/// 排版与行跨度信息由 [`FrameImage::bytes_per_row`] 保留。
pub struct FrameImage {
    /// 宽度（像素）。
    pub width: u32,
    /// 高度（像素）。
    pub height: u32,
    /// 紧凑打包的 RGBA8 像素（长度 `width * height * 4`，不含行补齐）。
    pub rgba: Vec<u8>,
    /// 读回缓冲的行跨度（含 256 对齐补齐）。
    pub bytes_per_row: u32,
}

impl FrameImage {
    /// 取像素（`(x, y)` 越界返回 `None`）。返回 `[r, g, b, a]`。
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let base = ((y * self.width + x) * 4) as usize;
        let bytes = self.rgba.get(base..base + 4)?;
        Some([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// 取一行（紧凑 RGBA 切片）。
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.height {
            return None;
        }
        let start = (y * self.width * 4) as usize;
        self.rgba.get(start..start + (self.width * 4) as usize)
    }

    /// 该像素的内存格式名（用于证据里说明像素字节序）。
    pub fn storage_format(&self) -> &'static str {
        "RGBA8Unorm"
    }

    /// 颜色字面量（`rgba(13,13,25,255)` 形态，便于对比原型日志）。
    pub fn rgba_text(&self, x: u32, y: u32) -> Option<String> {
        self.pixel(x, y)
            .map(|[r, g, b, a]| format!("rgba({r},{g},{b},{a})"))
    }

    /// 非黑像素（任一通道非 0）计数。
    pub fn non_black_count(&self) -> u32 {
        self.rgba
            .chunks_exact(4)
            .filter(|p| p[0] | p[1] | p[2] != 0)
            .count() as u32
    }

    /// 不同颜色的去重计数。
    pub fn distinct_colors(&self) -> usize {
        let mut unique: BTreeSet<[u8; 4]> = BTreeSet::new();
        for pixel in self.rgba.chunks_exact(4) {
            unique.insert([pixel[0], pixel[1], pixel[2], pixel[3]]);
        }
        unique.len()
    }
}

impl std::fmt::Debug for FrameImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 摘要式输出：像素本体动辄 16 KiB，整块倾倒没有诊断价值。
        f.debug_struct("FrameImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes_per_row", &self.bytes_per_row)
            .field("rgba_len", &self.rgba.len())
            .finish()
    }
}

/// 释放句柄所需的三支函数指针（从 [`WgpuApi`] 抄一份）。
///
/// 为什么不直接存 `&WgpuApi`：那会让 [`RenderTarget`] 背上生命周期参数，
/// 而本 crate 的其余部分都是裸指针句柄风格。`WgpuApi` 的函数指针是 `Copy`，
/// 抄三支进来即可让 `Drop` 独立成立，不必反过来依赖 `GpuContext` 还活着。
#[derive(Clone, Copy)]
struct TargetOps {
    texture_view_release: unsafe extern "system" fn(*mut c_void),
    texture_release: unsafe extern "system" fn(*mut c_void),
    buffer_release: unsafe extern "system" fn(*mut c_void),
}

/// 离屏渲染目标：一张 RGBA8 纹理 + 视图 + 读回缓冲。
///
/// 尺寸固定为 64x64（`64 * 4 = 256` 字节/行，正好卡在
/// [`COPY_ROW_ALIGNMENT`] 边界上，读回时**无需**插入行补齐 ——
/// 少一段错位的可能，证据更干净）。
pub struct RenderTarget {
    pub(crate) size: (u32, u32),
    pub(crate) texture: *mut c_void,
    pub(crate) view: *mut c_void,
    pub(crate) readback: *mut c_void,
    pub(crate) bytes_per_row: u32,
    pub(crate) buffer_size: u64,
    ops: TargetOps,
}

impl RenderTarget {
    /// 默认 64x64 目标。
    pub const DEFAULT_SIZE: (u32, u32) = (64, 64);

    /// 创建离屏目标。
    pub fn new(ctx: &GpuContext) -> Result<Self, BackendError> {
        Self::with_size(ctx, Self::DEFAULT_SIZE.0, Self::DEFAULT_SIZE.1)
    }

    /// 按指定尺寸创建离屏目标。
    pub fn with_size(ctx: &GpuContext, width: u32, height: u32) -> Result<Self, BackendError> {
        if width == 0 || height == 0 {
            return Err(BackendError::ConfigMismatch(
                "渲染目标尺寸不能为 0".to_string(),
            ));
        }
        let padded_row = width * 4;
        if padded_row % COPY_ROW_ALIGNMENT != 0 {
            return Err(BackendError::ConfigMismatch(format!(
                "读回要求每行 4 字节对齐到 {COPY_ROW_ALIGNMENT}：{width}px 宽为 {padded_row} 字节，不满足"
            )));
        }

        let mut desc = ffi::TextureDescriptor {
            usage: ffi::WGPU_TEXTURE_USAGE_RENDER_ATTACHMENT
                | ffi::WGPU_TEXTURE_USAGE_COPY_SRC
                | ffi::WGPU_TEXTURE_USAGE_TEXTURE_BINDING,
            dimension: ffi::WGPU_TEXTURE_DIMENSION_2D,
            size: Extent3D::rect(width, height),
            format: ffi::WGPU_TEXTURE_FORMAT_RGBA8_UNORM,
            mip_level_count: 1,
            sample_count: 1,
            ..Default::default()
        };
        desc.label = StringView::from_static("nes-render-target");
        let texture = unsafe { (ctx.api().device_create_texture)(ctx.device(), &desc) };
        if texture.is_null() {
            return Err(BackendError::NullHandle("WGPUTexture(render-target)"));
        }

        let view = unsafe { (ctx.api().texture_create_view)(texture, ptr::null()) };
        if view.is_null() {
            unsafe { (ctx.api().texture_release)(texture) };
            return Err(BackendError::NullHandle("WGPUTextureView(render-target)"));
        }

        let buffer_size = (padded_row as u64) * (height as u64);
        let mut buffer_desc = ffi::BufferDescriptor {
            usage: ffi::WGPU_BUFFER_USAGE_MAP_READ | ffi::WGPU_BUFFER_USAGE_COPY_DST,
            size: buffer_size,
            ..Default::default()
        };
        buffer_desc.label = StringView::from_static("nes-render-readback");
        let readback = unsafe { (ctx.api().device_create_buffer)(ctx.device(), &buffer_desc) };
        if readback.is_null() {
            unsafe {
                (ctx.api().texture_view_release)(view);
                (ctx.api().texture_release)(texture);
            }
            return Err(BackendError::NullHandle("WGPUBuffer(readback)"));
        }

        Ok(Self {
            size: (width, height),
            texture,
            view,
            readback,
            bytes_per_row: padded_row,
            buffer_size,
            ops: TargetOps {
                texture_view_release: ctx.api().texture_view_release,
                texture_release: ctx.api().texture_release,
                buffer_release: ctx.api().buffer_release,
            },
        })
    }

    /// 目标尺寸。
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// 读回一行（含尾部补齐）的字节跨度。
    pub fn padded_bytes_per_row(&self) -> u32 {
        self.bytes_per_row
    }

    /// 把纹理拷进读回缓冲、映射、拷成紧凑 RGBA 顺序的 [`FrameImage`]。
    ///
    /// 这里只做**一次**提交；映射回调走 [`GpuContext::flush_events`] 轮询。
    pub fn read_back(&self, ctx: &GpuContext) -> Result<FrameImage, BackendError> {
        let layout = ffi::CopyBufferLayout {
            offset: 0,
            bytes_per_row: self.bytes_per_row,
            rows_per_image: self.size.1,
        };
        let source = ffi::CopyTextureInfo {
            texture: self.texture,
            mip_level: 0,
            origin: ffi::Origin3D { x: 0, y: 0, z: 0 },
            aspect: ffi::WGPU_TEXTURE_ASPECT_ALL,
        };
        let destination = ffi::CopyBufferInfo {
            layout,
            buffer: self.readback,
        };

        let encoder =
            unsafe { (ctx.api().device_create_command_encoder)(ctx.device(), ptr::null()) };
        if encoder.is_null() {
            return Err(BackendError::NullHandle("WGPUCommandEncoder(readback)"));
        }
        // 拷贝范围 = 整张目标纹理（`webgpu.h` 现行 ABI 要求显式传 `WGPUExtent3D`）。
        let extent = Extent3D::rect(self.size.0, self.size.1);
        unsafe {
            (ctx.api().command_encoder_copy_texture_to_buffer)(
                encoder,
                &source,
                &destination,
                &extent,
            );
        }
        // 无可选链式结构：`WGPUCommandEncoderDescriptor.nextInChain` 传空指针。
        let command_buffer = unsafe { (ctx.api().command_encoder_finish)(encoder, ptr::null()) };
        unsafe { (ctx.api().command_encoder_release)(encoder) };
        if command_buffer.is_null() {
            return Err(BackendError::NullHandle("WGPUCommandBuffer(readback)"));
        }
        unsafe { (ctx.api().queue_submit)(ctx.queue(), 1, &command_buffer) };
        unsafe { (ctx.api().command_buffer_release)(command_buffer) };
        ctx.flush_events();

        // 每次读回前必须重新映射：缓冲可能仍处于上一帧的映射状态。
        let callback: ffi::MapCallback = on_map;
        let info = CallbackInfo::process_events(callback as *mut c_void, ctx.callbacks_ptr());
        unsafe {
            (ctx.api().buffer_map_async)(
                self.readback,
                ffi::WGPU_MAP_MODE_READ,
                0,
                self.buffer_size as usize,
                info,
            );
        }

        let outcome = ctx.pump_map("wgpuBufferMapAsync")?;
        if outcome.status != ffi::WGPU_MAP_ASYNC_STATUS_SUCCESS {
            return Err(BackendError::MapFailed {
                status: outcome.status,
                message: outcome.message,
            });
        }

        let mapped = unsafe {
            (ctx.api().buffer_get_mapped_range)(self.readback, 0, self.buffer_size as usize)
        };
        if mapped.is_null() {
            unsafe { (ctx.api().buffer_unmap)(self.readback) };
            return Err(BackendError::NullHandle("mapped range(readback)"));
        }

        let src =
            unsafe { std::slice::from_raw_parts(mapped as *const u8, self.buffer_size as usize) };
        let row_bytes = (self.size.0 * 4) as usize;
        let mut packed = Vec::with_capacity(row_bytes * self.size.1 as usize);
        for row in 0..self.size.1 as usize {
            let start = row * self.bytes_per_row as usize;
            packed.extend_from_slice(&src[start..start + row_bytes]);
        }
        unsafe { (ctx.api().buffer_unmap)(self.readback) };

        Ok(FrameImage {
            width: self.size.0,
            height: self.size.1,
            rgba: packed,
            bytes_per_row: self.bytes_per_row,
        })
    }
}

impl Drop for RenderTarget {
    fn drop(&mut self) {
        // SAFETY: 三个句柄各自非空才释放；`view` 必须先于 `texture` 释放。
        unsafe {
            if !self.view.is_null() {
                (self.ops.texture_view_release)(self.view);
                self.view = ptr::null_mut();
            }
            if !self.texture.is_null() {
                (self.ops.texture_release)(self.texture);
                self.texture = ptr::null_mut();
            }
            if !self.readback.is_null() {
                (self.ops.buffer_release)(self.readback);
                self.readback = ptr::null_mut();
            }
        }
    }
}

// ------------------------------------------------------------ 精灵图集

/// 传给管线层的最小绑定组句柄集合。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindGroupHandles {
    /// 管线布局句柄。
    pub pipeline_layout: *mut c_void,
    /// 绑定组布局句柄。
    pub bind_group_layout: *mut c_void,
    /// 绑定组句柄。
    pub bind_group: *mut c_void,
}

/// 释放图集句柄所需的函数指针（理由同 [`TargetOps`]）。
#[derive(Clone, Copy)]
struct AtlasOps {
    texture_view_release: unsafe extern "system" fn(*mut c_void),
    texture_release: unsafe extern "system" fn(*mut c_void),
    sampler_release: unsafe extern "system" fn(*mut c_void),
    buffer_release: unsafe extern "system" fn(*mut c_void),
    bind_group_release: unsafe extern "system" fn(*mut c_void),
    bind_group_layout_release: unsafe extern "system" fn(*mut c_void),
    pipeline_layout_release: unsafe extern "system" fn(*mut c_void),
}

/// 各绑定资源在着色器里声明的**最小尺寸**。
///
/// 为什么这一点值得单独写：`min_binding_size` 把「着色器里声明的布局」和
/// 「宿主实际绑定的尺寸」钉在一起。若我绑定的纹理/缓冲比声明的小，驱动会在创建
/// 绑定组时判定布局不兼容 —— 而这类错误如果在运行时表现为"画面全黑"，
/// 排查代价极高。所以三处都显式写死、并在创建前自检。
struct Sizes {
    viewport: u64,
}

impl Sizes {
    fn checked() -> Result<Self, BackendError> {
        const F32: u64 = core::mem::size_of::<f32>() as u64;
        let sizes = Self { viewport: 8 * F32 };
        // 图集是单层 2D 纹理：多写一层就是布局不兼容。
        if ATLAS_PX % ATLAS_CELLS != 0 {
            return Err(BackendError::ConfigMismatch(format!(
                "图集 {ATLAS_PX}px 无法被 {ATLAS_CELLS} 等分"
            )));
        }
        Ok(sizes)
    }
}

/// 精灵图集：纹理 + 采样器 + 实例数据缓冲 + 绑定组，一次性装配好交给管线复用。
pub struct SpriteAtlas {
    texture: *mut c_void,
    view: *mut c_void,
    sampler: *mut c_void,
    buffer: *mut c_void,
    bind_group_layout: *mut c_void,
    #[allow(dead_code)]
    pipeline_layout: *mut c_void,
    bind_group: *mut c_void,
    ops: AtlasOps,
}

impl SpriteAtlas {
    /// 装配图集（上传图案、建采样器与绑定组）。
    pub fn new(ctx: &GpuContext) -> Result<Self, BackendError> {
        let api = ctx.api();
        let device = ctx.device();
        let queue = ctx.queue();

        let sheet = build_sprite_sheet();
        if sheet.len() != (ATLAS_PX * ATLAS_PX * 4) as usize {
            return Err(BackendError::ConfigMismatch(
                "图集字节数与声明尺寸不一致".to_string(),
            ));
        }

        // 1) 图集纹理（RGBA8，256x256，COPY_DST 供上传、TEXTURE_BINDING 供采样）。
        let mut texture_desc = ffi::TextureDescriptor {
            usage: ffi::WGPU_TEXTURE_USAGE_COPY_DST | ffi::WGPU_TEXTURE_USAGE_TEXTURE_BINDING,
            dimension: ffi::WGPU_TEXTURE_DIMENSION_2D,
            size: Extent3D::rect(ATLAS_PX, ATLAS_PX),
            format: ffi::WGPU_TEXTURE_FORMAT_RGBA8_UNORM,
            mip_level_count: 1,
            sample_count: 1,
            ..Default::default()
        };
        texture_desc.label = StringView::from_static("nes-sprite-atlas");
        let texture = unsafe { (api.device_create_texture)(device, &texture_desc) };
        if texture.is_null() {
            return Err(BackendError::NullHandle("WGPUTexture(sprite-atlas)"));
        }
        let view = unsafe { (api.texture_create_view)(texture, ptr::null()) };
        if view.is_null() {
            unsafe { (api.texture_release)(texture) };
            return Err(BackendError::NullHandle("WGPUTextureView(sprite-atlas)"));
        }

        // 2) 上传像素：字节数必须等于 width*height*4（漏传/多传都会先在这里失败）。
        let upload_size = (ATLAS_PX * ATLAS_PX * 4) as usize;
        let destination = ffi::CopyTextureInfo {
            texture,
            mip_level: 0,
            origin: ffi::Origin3D { x: 0, y: 0, z: 0 },
            aspect: ffi::WGPU_TEXTURE_ASPECT_ALL,
        };
        let layout = ffi::CopyBufferLayout {
            offset: 0,
            bytes_per_row: ATLAS_PX * 4,
            rows_per_image: ATLAS_PX,
        };
        let extent = Extent3D::rect(ATLAS_PX, ATLAS_PX);
        unsafe {
            (api.queue_write_texture)(
                queue,
                &destination,
                sheet.as_ptr() as *const c_void,
                upload_size,
                &layout,
                &extent,
            );
        }

        // 3) 采样器：最近邻 + 边缘钳位 —— 像素画必须"点对点"，不允许线性插值或越界环绕。
        //    max_anisotropy 最小合法值是 1（1 = 关闭各向异性；0 是校验错误）。
        let mut sampler_desc = ffi::SamplerDescriptor {
            address_mode_u: ffi::WGPU_ADDRESS_MODE_CLAMP_TO_EDGE,
            address_mode_v: ffi::WGPU_ADDRESS_MODE_CLAMP_TO_EDGE,
            address_mode_w: ffi::WGPU_ADDRESS_MODE_CLAMP_TO_EDGE,
            mag_filter: ffi::WGPU_FILTER_MODE_NEAREST,
            min_filter: ffi::WGPU_FILTER_MODE_NEAREST,
            mipmap_filter: ffi::WGPU_MIPMAP_FILTER_MODE_NEAREST,
            max_anisotropy: 1,
            ..Default::default()
        };
        sampler_desc.label = StringView::from_static("nes-sprite-sampler");
        let sampler = unsafe { (api.device_create_sampler)(device, &sampler_desc) };
        if sampler.is_null() {
            unsafe {
                (api.texture_view_release)(view);
                (api.texture_release)(texture);
            }
            return Err(BackendError::NullHandle("WGPUSampler(sprite-atlas)"));
        }

        // 4) 视图参数缓冲（binding 0 的 uniform 数据源，每帧由管线重写）。
        //    必须带 Uniform 用法位才能被绑成 uniform、带 CopyDst 才能被
        //    queueWriteBuffer 重写 —— 缺任一位都是绑定组创建 / 缓冲写入时的
        //    校验错误（以"画面全黑"或未捕获错误的形式出现，排查代价极高）。
        let sizes = Sizes::checked()?;
        let mut view_desc = ffi::BufferDescriptor {
            usage: ffi::WGPU_BUFFER_USAGE_UNIFORM | ffi::WGPU_BUFFER_USAGE_COPY_DST,
            size: sizes.viewport,
            ..Default::default()
        };
        view_desc.label = StringView::from_static("nes-sprite-view-uniform");
        let buffer = unsafe { (api.device_create_buffer)(device, &view_desc) };
        if buffer.is_null() {
            unsafe {
                (api.sampler_release)(sampler);
                (api.texture_view_release)(view);
                (api.texture_release)(texture);
            }
            return Err(BackendError::NullHandle("WGPUBuffer(view-uniform)"));
        }

        // 5) 绑定组布局：binding 0 = 统一缓冲（顶点可见），1 = 纹理，2 = 采样器（均片段可见）。
        let buffer_layout = ffi::BufferBindingLayout {
            binding_type: ffi::WGPU_BUFFER_BINDING_TYPE_UNIFORM,
            has_dynamic_offset: 0,
            min_binding_size: sizes.viewport,
            ..Default::default()
        };
        let texture_layout = ffi::TextureBindingLayout {
            sample_type: ffi::WGPU_TEXTURE_SAMPLE_TYPE_FLOAT,
            view_dimension: ffi::WGPU_TEXTURE_VIEW_DIMENSION_2D,
            multisampled: 0,
            ..Default::default()
        };
        let sampler_layout = ffi::SamplerBindingLayout {
            binding_type: ffi::WGPU_SAMPLER_BINDING_TYPE_FILTERING,
            ..Default::default()
        };
        let entries = [
            ffi::BindGroupLayoutEntry {
                binding: 0,
                visibility: ffi::WGPU_SHADER_STAGE_VERTEX,
                buffer: buffer_layout,
                ..Default::default()
            },
            ffi::BindGroupLayoutEntry {
                binding: 1,
                visibility: ffi::WGPU_SHADER_STAGE_FRAGMENT,
                texture: texture_layout,
                ..Default::default()
            },
            ffi::BindGroupLayoutEntry {
                binding: 2,
                visibility: ffi::WGPU_SHADER_STAGE_FRAGMENT,
                sampler: sampler_layout,
                ..Default::default()
            },
        ];
        let mut bgl_desc = ffi::BindGroupLayoutDescriptor {
            entry_count: entries.len(),
            entries: entries.as_ptr(),
            ..Default::default()
        };
        bgl_desc.label = StringView::from_static("nes-sprite-bgl");
        let bind_group_layout = unsafe { (api.device_create_bind_group_layout)(device, &bgl_desc) };
        if bind_group_layout.is_null() {
            unsafe {
                (api.buffer_release)(buffer);
                (api.sampler_release)(sampler);
                (api.texture_view_release)(view);
                (api.texture_release)(texture);
            }
            return Err(BackendError::NullHandle("WGPUBindGroupLayout(sprite)"));
        }

        // 6) 管线布局。
        let layouts = [bind_group_layout];
        let mut pl_desc = ffi::PipelineLayoutDescriptor {
            bind_group_layout_count: layouts.len(),
            bind_group_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        pl_desc.label = StringView::from_static("nes-sprite-pipeline-layout");
        let pipeline_layout = unsafe { (api.device_create_pipeline_layout)(device, &pl_desc) };
        if pipeline_layout.is_null() {
            unsafe {
                (api.bind_group_layout_release)(bind_group_layout);
                (api.buffer_release)(buffer);
                (api.sampler_release)(sampler);
                (api.texture_view_release)(view);
                (api.texture_release)(texture);
            }
            return Err(BackendError::NullHandle("WGPUPipelineLayout(sprite)"));
        }

        // 7) 绑定组本体。
        let bind_entries = [
            ffi::BindGroupEntry {
                binding: 0,
                buffer,
                offset: 0,
                size: sizes.viewport,
                ..Default::default()
            },
            ffi::BindGroupEntry {
                binding: 1,
                texture_view: view,
                ..Default::default()
            },
            ffi::BindGroupEntry {
                binding: 2,
                sampler,
                ..Default::default()
            },
        ];
        let mut bg_desc = ffi::BindGroupDescriptor {
            layout: bind_group_layout,
            entry_count: bind_entries.len(),
            entries: bind_entries.as_ptr(),
            ..Default::default()
        };
        bg_desc.label = StringView::from_static("nes-sprite-bind-group");
        let bind_group = unsafe { (api.device_create_bind_group)(device, &bg_desc) };
        if bind_group.is_null() {
            unsafe {
                (api.pipeline_layout_release)(pipeline_layout);
                (api.bind_group_layout_release)(bind_group_layout);
                (api.buffer_release)(buffer);
                (api.sampler_release)(sampler);
                (api.texture_view_release)(view);
                (api.texture_release)(texture);
            }
            return Err(BackendError::NullHandle("WGPUBindGroup(sprite)"));
        }

        Ok(Self {
            texture,
            view,
            sampler,
            buffer,
            bind_group_layout,
            pipeline_layout,
            bind_group,
            ops: AtlasOps {
                texture_view_release: api.texture_view_release,
                texture_release: api.texture_release,
                sampler_release: api.sampler_release,
                buffer_release: api.buffer_release,
                bind_group_release: api.bind_group_release,
                bind_group_layout_release: api.bind_group_layout_release,
                pipeline_layout_release: api.pipeline_layout_release,
            },
        })
    }

    /// 供管线复用的句柄集合。
    pub fn handles(&self) -> BindGroupHandles {
        BindGroupHandles {
            pipeline_layout: self.pipeline_layout,
            bind_group_layout: self.bind_group_layout,
            bind_group: self.bind_group,
        }
    }

    /// 图集纹理视图（诊断用）。
    pub fn view(&self) -> *mut c_void {
        self.view
    }

    /// 图集采样器句柄（管线在图集的绑定组布局上自建绑定组时需要引用它）。
    pub fn sampler(&self) -> *mut c_void {
        self.sampler
    }

    /// 视图参数缓冲句柄（绑定组 binding 0 的数据源；管线每帧用
    /// `wgpuQueueWriteBuffer` 重写它）。
    pub fn view_uniform(&self) -> *mut c_void {
        self.buffer
    }
}

impl Drop for SpriteAtlas {
    fn drop(&mut self) {
        // SAFETY: 逐个非空释放；绑定组/视图必须先于其所属布局/纹理释放。
        unsafe {
            if !self.bind_group.is_null() {
                (self.ops.bind_group_release)(self.bind_group);
                self.bind_group = ptr::null_mut();
            }
            if !self.pipeline_layout.is_null() {
                (self.ops.pipeline_layout_release)(self.pipeline_layout);
                self.pipeline_layout = ptr::null_mut();
            }
            if !self.bind_group_layout.is_null() {
                (self.ops.bind_group_layout_release)(self.bind_group_layout);
                self.bind_group_layout = ptr::null_mut();
            }
            if !self.buffer.is_null() {
                (self.ops.buffer_release)(self.buffer);
                self.buffer = ptr::null_mut();
            }
            if !self.sampler.is_null() {
                (self.ops.sampler_release)(self.sampler);
                self.sampler = ptr::null_mut();
            }
            if !self.view.is_null() {
                (self.ops.texture_view_release)(self.view);
                self.view = ptr::null_mut();
            }
            if !self.texture.is_null() {
                (self.ops.texture_release)(self.texture);
                self.texture = ptr::null_mut();
            }
        }
    }
}

// ------------------------------------------------------------ 纹理注册表

/// 释放注册表句柄所需的函数指针（理由同 `AtlasOps`：让 `Drop` 独立成立）。
#[derive(Clone, Copy)]
struct RegistryOps {
    texture_view_release: unsafe extern "system" fn(*mut c_void),
    texture_release: unsafe extern "system" fn(*mut c_void),
    sampler_release: unsafe extern "system" fn(*mut c_void),
    bind_group_release: unsafe extern "system" fn(*mut c_void),
    bind_group_layout_release: unsafe extern "system" fn(*mut c_void),
}

/// 一张已上传图层的 CPU 侧留档（扩容重建时逐层重传）。
#[derive(Clone)]
struct LayerData {
    width: u32,
    height: u32,
    /// 紧凑 RGBA（`width * height * 4` 字节，无行补齐）。
    rgba: Vec<u8>,
}

/// 宿主侧纹理注册表：`RenderAssetKey` -> 平铺大纹理（瓦片图集）里的一个瓦片。
///
/// # 形状（S4.3 口径）
///
/// - 不用 2D 数组纹理：本版 naga 对数组纹理采样内建的重载解析与标准 WGSL
///   不符（实测连续两种合法写法都被拒），而 `texture_2d` 的四参显式 LOD
///   采样已被 S4.1 实证可用。于是注册表是**一张 2D 大纹理**，每张上传占
///   一个 `TILE_PX`×`TILE_PX` 瓦片，容量不足时边长翻倍重建。
/// - 上传走 `wgpuQueueWriteTexture`：源行按 256 字节对齐**在 CPU 侧补齐**
///   （`bytes_per_row` 的本义就是允许源行带补齐），不需要额外的暂存缓冲或
///   `copyBufferToTexture` 符号。
/// - 小于瓦片尺寸的纹理落在瓦片左上角，采样按 `uv = (w/TILE_PX, h/TILE_PX)`
///   裁剪；超过瓦片尺寸的纹理在 [`TextureRegistry::register`] 里以
///   [`BackendError::ConfigMismatch`] 拒绝（NES 级素材上限，如实报错不缩放）。
/// - 同键重复注册 = 原瓦片覆写（热重载语义），不换瓦片号。
/// - 纹理创建即零初始化（WebGPU 保证），未写到的纹素透明（alpha=0，
///   片段着色器按 alpha 丢弃）。
///
/// # 绑定形态
///
/// 独立绑定组（group 1）：binding 0 = 大纹理（片段可见）、binding 1 = 采样器。
/// 管线布局 = [图集绑定组布局, 注册表绑定组布局]，精灵按"键是否已注册"两路采样。
pub struct TextureRegistry {
    texture: *mut c_void,
    view: *mut c_void,
    sampler: *mut c_void,
    bind_group_layout: *mut c_void,
    bind_group: *mut c_void,
    /// 瓦片边长（像素）。
    tile_px: u32,
    /// 纹理每边的瓦片数（纹理边长 = tile_px * tiles_per_side）。
    tiles_per_side: u32,
    next_tile: u32,
    layers: Vec<Option<LayerData>>,
    keys: std::collections::BTreeMap<RenderAssetKey, u32>,
    ops: RegistryOps,
}

impl TextureRegistry {
    /// 瓦片边长（像素）。256 = NES 级素材上限 + `256*4 = 1024` 字节/行天然满足
    /// 纹理上传的 256 字节对齐。
    pub const TILE_PX: u32 = 256;
    /// 初始每边瓦片数（2x2 = 4 瓦片，纹理 512x512；不足时边长翻倍重建）。
    pub const INITIAL_TILES_PER_SIDE: u32 = 2;

    /// 装配注册表（大纹理 + 采样器 + 绑定组布局与绑定组）。
    pub fn new(ctx: &GpuContext) -> Result<Self, BackendError> {
        let mut registry = Self {
            texture: ptr::null_mut(),
            view: ptr::null_mut(),
            sampler: ptr::null_mut(),
            bind_group_layout: ptr::null_mut(),
            bind_group: ptr::null_mut(),
            tile_px: Self::TILE_PX,
            tiles_per_side: Self::INITIAL_TILES_PER_SIDE,
            next_tile: 0,
            layers: Vec::new(),
            keys: std::collections::BTreeMap::new(),
            ops: RegistryOps {
                texture_view_release: ctx.api().texture_view_release,
                texture_release: ctx.api().texture_release,
                sampler_release: ctx.api().sampler_release,
                bind_group_release: ctx.api().bind_group_release,
                bind_group_layout_release: ctx.api().bind_group_layout_release,
            },
        };
        registry.create_handles(ctx, registry.tiles_per_side)?;
        Ok(registry)
    }

    /// 创建（或扩容重建）纹理三件套：纹理 + 视图 + 绑定组。
    fn create_handles(
        &mut self,
        ctx: &GpuContext,
        tiles_per_side: u32,
    ) -> Result<(), BackendError> {
        let api = ctx.api();
        let device = ctx.device();
        let side = self.tile_px * tiles_per_side;

        // 1) 大纹理（RGBA8，COPY_DST 供上传、TEXTURE_BINDING 供采样）。
        let mut texture_desc = ffi::TextureDescriptor {
            usage: ffi::WGPU_TEXTURE_USAGE_COPY_DST | ffi::WGPU_TEXTURE_USAGE_TEXTURE_BINDING,
            dimension: ffi::WGPU_TEXTURE_DIMENSION_2D,
            size: Extent3D::rect(side, side),
            format: ffi::WGPU_TEXTURE_FORMAT_RGBA8_UNORM,
            mip_level_count: 1,
            sample_count: 1,
            ..Default::default()
        };
        texture_desc.label = StringView::from_static("nes-registry-atlas");
        let texture = unsafe { (api.device_create_texture)(device, &texture_desc) };
        if texture.is_null() {
            return Err(BackendError::NullHandle("WGPUTexture(registry)"));
        }
        let view = unsafe { (api.texture_create_view)(texture, ptr::null()) };
        if view.is_null() {
            unsafe { (api.texture_release)(texture) };
            return Err(BackendError::NullHandle("WGPUTextureView(registry)"));
        }

        // 2) 采样器：与图集同口径（最近邻 + 钳位，像素画"点对点"）。
        if self.sampler.is_null() {
            let mut sampler_desc = ffi::SamplerDescriptor {
                address_mode_u: ffi::WGPU_ADDRESS_MODE_CLAMP_TO_EDGE,
                address_mode_v: ffi::WGPU_ADDRESS_MODE_CLAMP_TO_EDGE,
                address_mode_w: ffi::WGPU_ADDRESS_MODE_CLAMP_TO_EDGE,
                mag_filter: ffi::WGPU_FILTER_MODE_NEAREST,
                min_filter: ffi::WGPU_FILTER_MODE_NEAREST,
                mipmap_filter: ffi::WGPU_MIPMAP_FILTER_MODE_NEAREST,
                max_anisotropy: 1,
                ..Default::default()
            };
            sampler_desc.label = StringView::from_static("nes-registry-sampler");
            let sampler = unsafe { (api.device_create_sampler)(device, &sampler_desc) };
            if sampler.is_null() {
                unsafe {
                    (api.texture_view_release)(view);
                    (api.texture_release)(texture);
                }
                return Err(BackendError::NullHandle("WGPUSampler(registry)"));
            }
            self.sampler = sampler;
        }

        // 3) 绑定组布局：binding 0 = 大纹理，binding 1 = 采样器（均片段可见）。
        if self.bind_group_layout.is_null() {
            let texture_layout = ffi::TextureBindingLayout {
                sample_type: ffi::WGPU_TEXTURE_SAMPLE_TYPE_FLOAT,
                view_dimension: ffi::WGPU_TEXTURE_VIEW_DIMENSION_2D,
                multisampled: 0,
                ..Default::default()
            };
            let sampler_layout = ffi::SamplerBindingLayout {
                binding_type: ffi::WGPU_SAMPLER_BINDING_TYPE_FILTERING,
                ..Default::default()
            };
            let entries = [
                ffi::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ffi::WGPU_SHADER_STAGE_FRAGMENT,
                    texture: texture_layout,
                    ..Default::default()
                },
                ffi::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ffi::WGPU_SHADER_STAGE_FRAGMENT,
                    sampler: sampler_layout,
                    ..Default::default()
                },
            ];
            let mut bgl_desc = ffi::BindGroupLayoutDescriptor {
                entry_count: entries.len(),
                entries: entries.as_ptr(),
                ..Default::default()
            };
            bgl_desc.label = StringView::from_static("nes-registry-bgl");
            let bgl = unsafe { (api.device_create_bind_group_layout)(device, &bgl_desc) };
            if bgl.is_null() {
                unsafe {
                    (api.texture_view_release)(view);
                    (api.texture_release)(texture);
                }
                return Err(BackendError::NullHandle("WGPUBindGroupLayout(registry)"));
            }
            self.bind_group_layout = bgl;
        }

        // 4) 绑定组本体（指向新视图；扩容重建时换绑）。
        let bind_entries = [
            ffi::BindGroupEntry {
                binding: 0,
                texture_view: view,
                ..Default::default()
            },
            ffi::BindGroupEntry {
                binding: 1,
                sampler: self.sampler,
                ..Default::default()
            },
        ];
        let mut bg_desc = ffi::BindGroupDescriptor {
            layout: self.bind_group_layout,
            entry_count: bind_entries.len(),
            entries: bind_entries.as_ptr(),
            ..Default::default()
        };
        bg_desc.label = StringView::from_static("nes-registry-bind-group");
        let bind_group = unsafe { (api.device_create_bind_group)(device, &bg_desc) };
        if bind_group.is_null() {
            unsafe {
                (api.texture_view_release)(view);
                (api.texture_release)(texture);
            }
            return Err(BackendError::NullHandle("WGPUBindGroup(registry)"));
        }

        // 换句柄前释放旧三件（绑定组与视图先于纹理；首次创建时均为空，跳过）。
        unsafe {
            if !self.bind_group.is_null() {
                (self.ops.bind_group_release)(self.bind_group);
            }
            if !self.view.is_null() {
                (self.ops.texture_view_release)(self.view);
            }
            if !self.texture.is_null() {
                (self.ops.texture_release)(self.texture);
            }
        }
        self.texture = texture;
        self.view = view;
        self.bind_group = bind_group;
        self.tiles_per_side = tiles_per_side;
        self.layers
            .resize_with((tiles_per_side * tiles_per_side) as usize, || None);
        Ok(())
    }

    /// 注册（或覆写）一张纹理，返回其瓦片号。
    ///
    /// 尺寸超限（> `TILE_PX`）或字节数与尺寸不符时拒绝；同键重复注册覆写
    /// 原瓦片（瓦片号不变，热重载语义）。
    pub fn register(
        &mut self,
        ctx: &GpuContext,
        key: RenderAssetKey,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<u32, BackendError> {
        if key.is_nil() {
            // NIL 保留给"未绑定"语义（契约层口径）：NIL 键的渲染物在绘制过滤前
            // 就被跳过，注册进去是一张永远采不到的死纹理 —— 拒绝并指名道姓。
            return Err(BackendError::ConfigMismatch(
                "注册键不能是 NIL（NIL 保留给未绑定语义）".to_string(),
            ));
        }
        if width == 0 || height == 0 {
            return Err(BackendError::InvalidImageSize { width, height });
        }
        if width > self.tile_px || height > self.tile_px {
            return Err(BackendError::ConfigMismatch(format!(
                "纹理 {width}x{height} 超过注册表瓦片上限 {}x{}（NES 级素材口径，不缩放）",
                self.tile_px, self.tile_px
            )));
        }
        let expected = (width * height * 4) as usize;
        if rgba.len() != expected {
            return Err(BackendError::PixelBufferSize {
                expected,
                actual: rgba.len(),
            });
        }

        // 分配（或复用）瓦片；容量不足先边长翻倍重建。
        let tile = match self.keys.get(&key) {
            Some(&existing) => existing,
            None => {
                if self.next_tile >= self.tiles_per_side * self.tiles_per_side {
                    self.grow(ctx)?;
                }
                let allocated = self.next_tile;
                self.next_tile += 1;
                allocated
            }
        };
        self.upload_tile(ctx, tile, width, height, rgba)?;
        self.layers[tile as usize] = Some(LayerData {
            width,
            height,
            rgba: rgba.to_vec(),
        });
        self.keys.insert(key, tile);
        Ok(tile)
    }

    /// 查询键所在瓦片（未注册返回 `None`）。
    pub fn layer_of(&self, key: RenderAssetKey) -> Option<u32> {
        self.keys.get(&key).copied()
    }

    /// 已注册纹理数（= 已占用的瓦片数）。
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// 是否一张都没有注册。
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// 注册表绑定组（管线的 group 1）。
    pub fn bind_group(&self) -> *mut c_void {
        self.bind_group
    }

    /// 注册表绑定组布局（精灵管线的管线布局第二项）。
    pub fn bind_group_layout(&self) -> *mut c_void {
        self.bind_group_layout
    }

    /// 取键的采样信息：`(瓦片号, UV 矩形)`。UV 矩形在大纹理坐标系里给出
    /// （瓦片左上角 + 按实际尺寸裁剪）；未注册返回 `None`。
    pub fn sample_info(&self, key: RenderAssetKey) -> Option<(u32, [f32; 4])> {
        let tile = self.layer_of(key)?;
        let data = self.layers.get(tile as usize)?.as_ref()?;
        let side = (self.tile_px * self.tiles_per_side) as f32;
        let (tx, ty) = self.tile_origin(tile);
        Some((
            tile,
            [
                tx as f32 / side,
                ty as f32 / side,
                data.width as f32 / side,
                data.height as f32 / side,
            ],
        ))
    }

    /// 取键的注册尺寸（源纹理像素宽高；注册时的 `width`/`height`）。
    ///
    /// S16.6 九宫格的补充读法：`sample_info` 只给归一化 UV 矩形（分数面
    /// 不含像素尺寸），而九宫切割边距是**源纹理像素**口径 —— 子矩形折算
    /// 需要注册尺寸做分母（`uv = px / 注册宽 x 全瓦片宽`）。数据与
    /// `sample_info` 同源（同一份 `LayerData`），未注册返回 `None`。
    pub fn texture_px_size(&self, key: RenderAssetKey) -> Option<(f32, f32)> {
        let tile = self.layer_of(key)?;
        let data = self.layers.get(tile as usize)?.as_ref()?;
        Some((data.width as f32, data.height as f32))
    }

    /// 瓦片号 -> 大纹理内的像素原点。
    fn tile_origin(&self, tile: u32) -> (u32, u32) {
        let tx = tile % self.tiles_per_side;
        let ty = tile / self.tiles_per_side;
        (tx * self.tile_px, ty * self.tile_px)
    }

    /// 把一张紧凑 RGBA 上传到指定瓦片（源行在 CPU 侧补齐到 256 字节倍数）。
    fn upload_tile(
        &self,
        ctx: &GpuContext,
        tile: u32,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), BackendError> {
        let row_bytes = (width * 4) as usize;
        // 源行补齐到 256 的倍数：queueWriteTexture 的 bytes_per_row 只要求
        // 「不小于实际行字节且为 256 倍数」，补齐正是它的设计用途。
        let padded = row_bytes.div_ceil(256) * 256;
        let mut staging = vec![0u8; padded * height as usize];
        for y in 0..height as usize {
            let dst = y * padded;
            let src = y * row_bytes;
            staging[dst..dst + row_bytes].copy_from_slice(&rgba[src..src + row_bytes]);
        }

        let (ox, oy) = self.tile_origin(tile);
        let destination = ffi::CopyTextureInfo {
            texture: self.texture,
            mip_level: 0,
            origin: ffi::Origin3D { x: ox, y: oy, z: 0 },
            aspect: ffi::WGPU_TEXTURE_ASPECT_ALL,
        };
        let layout = ffi::CopyBufferLayout {
            offset: 0,
            bytes_per_row: padded as u32,
            rows_per_image: height,
        };
        let extent = Extent3D::rect(width, height);
        // SAFETY: staging 在本次调用期间存活，长度与布局/范围一致。
        unsafe {
            (ctx.api().queue_write_texture)(
                ctx.queue(),
                &destination,
                staging.as_ptr() as *const c_void,
                staging.len(),
                &layout,
                &extent,
            );
        }
        Ok(())
    }

    /// 容量翻倍：边长 x2 重建大纹理（每边瓦片数翻倍），已有瓦片逐张重传。
    fn grow(&mut self, ctx: &GpuContext) -> Result<(), BackendError> {
        let new_side = self.tiles_per_side.saturating_mul(2);
        let side = self.tile_px * new_side;
        if side > 8192 {
            return Err(BackendError::ConfigMismatch(format!(
                "注册表扩容到 {side}px 超过常见设备上限 8192px（已含 {} 张纹理）",
                self.next_tile
            )));
        }
        self.create_handles(ctx, new_side)?;
        // 既有瓦片重传（留档的紧凑 RGBA 重新走补齐上传）。
        let snapshots: Vec<(u32, u32, u32, Vec<u8>)> = self
            .layers
            .iter()
            .take(self.next_tile as usize)
            .enumerate()
            .filter_map(|(tile, data)| {
                data.as_ref()
                    .map(|d| (tile as u32, d.width, d.height, d.rgba.clone()))
            })
            .collect();
        for (tile, width, height, rgba) in snapshots {
            self.upload_tile(ctx, tile, width, height, &rgba)?;
        }
        Ok(())
    }
}

impl Drop for TextureRegistry {
    fn drop(&mut self) {
        // SAFETY: 逐个非空释放；绑定组/视图必须先于其所属布局/纹理释放。
        unsafe {
            if !self.bind_group.is_null() {
                (self.ops.bind_group_release)(self.bind_group);
                self.bind_group = ptr::null_mut();
            }
            if !self.bind_group_layout.is_null() {
                (self.ops.bind_group_layout_release)(self.bind_group_layout);
                self.bind_group_layout = ptr::null_mut();
            }
            if !self.sampler.is_null() {
                (self.ops.sampler_release)(self.sampler);
                self.sampler = ptr::null_mut();
            }
            if !self.view.is_null() {
                (self.ops.texture_view_release)(self.view);
                self.view = ptr::null_mut();
            }
            if !self.texture.is_null() {
                (self.ops.texture_release)(self.texture);
                self.texture = ptr::null_mut();
            }
        }
    }
}

// ------------------------------------------------------------ 表面目标（S6.1）

/// 表面所需的函数指针（理由同 `TargetOps`：让 `Drop` 与 [`SurfaceTarget::
/// reconfigure`] 独立成立 —— 重配不要求调用方再递 `GpuContext`）。
#[derive(Clone, Copy)]
struct SurfaceOps {
    surface_configure: unsafe extern "system" fn(*mut c_void, *const ffi::SurfaceConfiguration),
    surface_unconfigure: unsafe extern "system" fn(*mut c_void),
    surface_release: unsafe extern "system" fn(*mut c_void),
}

/// 一次成功获取的表面帧（纹理 + 即建视图；present 后释放）。
pub struct SurfaceFrame {
    /// 表面纹理（`GetCurrentTexture` 返回，调用方持有）。
    pub texture: *mut c_void,
    /// 该纹理的即时视图（渲染目标用；先于纹理释放）。
    pub view: *mut c_void,
}

/// `WGPUSurfaceGetCurrentTextureStatus` 的状态名（错误信息用；未知码
/// 如实报"未知"不猜）。纯函数 —— 收束阶段的可测性口径。
pub fn surface_status_name(status: i32) -> &'static str {
    match status {
        ffi::WGPU_SURFACE_STATUS_SUCCESS_OPTIMAL => "SUCCESS_OPTIMAL",
        ffi::WGPU_SURFACE_STATUS_SUCCESS_SUBOPTIMAL => "SUCCESS_SUBOPTIMAL",
        ffi::WGPU_SURFACE_STATUS_TIMEOUT => "TIMEOUT",
        ffi::WGPU_SURFACE_STATUS_OUTDATED => "OUTDATED",
        ffi::WGPU_SURFACE_STATUS_LOST => "LOST",
        ffi::WGPU_SURFACE_STATUS_OUT_OF_MEMORY => "OUT_OF_MEMORY",
        _ => "未知状态",
    }
}

/// 窗口呈现目标：HWND -> wgpu surface（按客户区配置，Fifo 垂直同步）。
///
/// # 兼容性口径（S6.1）
///
/// surface 在 `GpuContext` 装配**之后**创建（不经 `compatibleSurface` 请求
/// 适配器）；随后立即用 `wgpuSurfaceGetCapabilities` 校验适配器与表面的
/// 兼容性，不兼容即如实报错（单 GPU 机器上 Vulkan 适配器与窗口表面同源，
/// 这是实测成立的最小路径；若未来多 GPU 报错，把 `RequestAdapterOptions::
/// compatible_surface` 的装配顺序提前即可，入口已备）。
///
/// 表面格式取 caps 中第一个 `RGBA8Unorm`（与离屏管线同一格式，精灵管线
/// 无需第二份）；caps 里没有则指名报错。
pub struct SurfaceTarget {
    surface: *mut c_void,
    /// 配置用的设备句柄（`new` 时自 ctx 记下；重配沿同一设备）。
    device: *mut c_void,
    width: u32,
    height: u32,
    format: i32,
    ops: SurfaceOps,
}

impl SurfaceTarget {
    /// 为窗口创建并配置表面（客户区尺寸、RGBA8Unorm、Fifo）。
    pub fn new(ctx: &GpuContext, window: &crate::window::Window) -> Result<Self, BackendError> {
        let api = ctx.api();
        let source = ffi::SurfaceSourceWindowsHwnd {
            chain: ffi::ChainedStruct {
                next: ptr::null_mut(),
                s_type: ffi::WGPU_STYPE_SURFACE_SOURCE_WINDOWS_HWND,
            },
            hinstance: window.hinstance(),
            hwnd: window.hwnd(),
        };
        let desc = ffi::SurfaceDescriptor {
            next_in_chain: &source as *const ffi::SurfaceSourceWindowsHwnd as *mut c_void,
            label: StringView::from_static("nes-surface"),
        };
        // SAFETY: source/desc 在本次调用期间存活；句柄来自存活的 Window。
        let surface = unsafe { (api.instance_create_surface)(ctx.instance_handle(), &desc) };
        if surface.is_null() {
            return Err(BackendError::NullHandle("WGPUSurface"));
        }

        // 能力校验 + 格式选择（顺便确认适配器兼容）。
        let mut caps = ffi::SurfaceCapabilities {
            next_in_chain: ptr::null_mut(),
            usages: 0,
            format_count: 0,
            formats: ptr::null(),
            present_mode_count: 0,
            present_modes: ptr::null(),
            alpha_mode_count: 0,
            alpha_modes: ptr::null(),
        };
        let status =
            unsafe { (api.surface_get_capabilities)(surface, ctx.adapter_handle(), &mut caps) };
        if status != ffi::WGPU_STATUS_SUCCESS {
            unsafe { (api.surface_release)(surface) };
            return Err(BackendError::ConfigMismatch(format!(
                "表面能力查询失败（status={status}）：适配器与窗口表面不兼容"
            )));
        }
        let formats = unsafe { std::slice::from_raw_parts(caps.formats, caps.format_count) };
        let format = formats
            .iter()
            .copied()
            .find(|f| *f == ffi::WGPU_TEXTURE_FORMAT_RGBA8_UNORM);
        // SAFETY: caps 的数组成员由该函数释放。
        unsafe { (api.surface_capabilities_free_members)(caps) };
        let Some(format) = format else {
            unsafe { (api.surface_release)(surface) };
            return Err(BackendError::ConfigMismatch(format!(
                "表面格式不含 RGBA8Unorm（实际 {:?}）：精灵管线暂只为该格式装配",
                formats
            )));
        };

        let (width, height) = window.client_size();
        if width == 0 || height == 0 {
            unsafe { (api.surface_release)(surface) };
            return Err(BackendError::ConfigMismatch(
                "窗口客户区尺寸为 0（窗口未显示？）".to_string(),
            ));
        }
        let config = ffi::SurfaceConfiguration {
            next_in_chain: ptr::null_mut(),
            device: ctx.device(),
            format,
            usage: ffi::WGPU_TEXTURE_USAGE_RENDER_ATTACHMENT,
            width,
            height,
            view_format_count: 0,
            view_formats: ptr::null(),
            alpha_mode: ffi::WGPU_COMPOSITE_ALPHA_MODE_AUTO,
            present_mode: ffi::WGPU_PRESENT_MODE_FIFO,
        };
        // SAFETY: config 在本次调用期间存活；设备来自 ctx。
        unsafe { (api.surface_configure)(surface, &config) };

        Ok(Self {
            surface,
            device: ctx.device(),
            width,
            height,
            format,
            ops: SurfaceOps {
                surface_configure: api.surface_configure,
                surface_unconfigure: api.surface_unconfigure,
                surface_release: api.surface_release,
            },
        })
    }

    /// 配置尺寸（客户区像素）。
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 重配表面尺寸（S12-4：窗口客户区变化后的根修 —— 开窗时只配置一次
    /// 的旧口径会把小交换链经合成器拉伸铺满大窗口）。
    ///
    /// 其余配置字段（设备/格式/用法/alpha/呈现模式 = Fifo）与 [`Self::new`]
    /// 逐项同源不变。FFI 口径：`wgpuSurfaceConfigure` 可**重复调用**
    ///（webgpu.h 语义：再次配置即整体替换旧配置，`Unconfigure` 只用于
    /// 销毁前解除）—— 同一 surface 句柄原地换尺寸，无需重建。
    ///
    /// 尺寸为 0（窗口最小化时 `GetClientRect` 归零）拒绝重配：wgpu 校验
    /// 不接受 0 尺寸表面，如实报错；调用方（运行时同步）应跳过该帧。
    pub fn reconfigure(&mut self, width: u32, height: u32) -> Result<(), BackendError> {
        if width == 0 || height == 0 {
            return Err(BackendError::ConfigMismatch(format!(
                "表面重配尺寸为 0（请求 {width}x{height}；窗口最小化？）"
            )));
        }
        let config = ffi::SurfaceConfiguration {
            next_in_chain: ptr::null_mut(),
            device: self.device,
            format: self.format,
            usage: ffi::WGPU_TEXTURE_USAGE_RENDER_ATTACHMENT,
            width,
            height,
            view_format_count: 0,
            view_formats: ptr::null(),
            alpha_mode: ffi::WGPU_COMPOSITE_ALPHA_MODE_AUTO,
            present_mode: ffi::WGPU_PRESENT_MODE_FIFO,
        };
        // SAFETY: config 在本次调用期间存活；device 与 surface 均存活
        //（device 生命周期覆盖整个 GpuContext，surface 由本对象持有）。
        unsafe { (self.ops.surface_configure)(self.surface, &config) };
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// 表面格式（恒为 `RGBA8Unorm`，见 [`Self::new`] 口径）。
    pub fn format(&self) -> i32 {
        self.format
    }

    /// 获取当前帧的表面纹理与视图（渲染目标）。状态非成功即如实报错。
    /// 获取当前帧的表面纹理与视图（渲染目标）。状态非成功即如实报错
    /// （状态名 + 原始码；**Timeout 归瞬态类** —— 呈现队列暂满不是配置
    /// 矛盾，宿主可跳过该帧重试，见 [`BackendError::Timeout`]）。
    pub fn acquire(&self, ctx: &GpuContext) -> Result<SurfaceFrame, BackendError> {
        let mut st = ffi::SurfaceTexture {
            next_in_chain: ptr::null_mut(),
            texture: ptr::null_mut(),
            status: 0,
        };
        // SAFETY: st 是合法出参。
        unsafe { (ctx.api().surface_get_current_texture)(self.surface, &mut st) };
        if st.status != ffi::WGPU_SURFACE_STATUS_SUCCESS_OPTIMAL
            && st.status != ffi::WGPU_SURFACE_STATUS_SUCCESS_SUBOPTIMAL
        {
            if st.status == ffi::WGPU_SURFACE_STATUS_TIMEOUT {
                // 瞬态：窗口被遮挡/合成器停顿时呈现队列暂满 —— 与异步
                // 回调超时同一类（可重试），不误诊成配置矛盾。
                return Err(BackendError::Timeout("surface_get_current_texture"));
            }
            return Err(BackendError::ConfigMismatch(format!(
                "获取表面纹理失败：{}(status={})",
                surface_status_name(st.status),
                st.status
            )));
        }
        if st.texture.is_null() {
            return Err(BackendError::NullHandle("surface texture"));
        }
        // SAFETY: 表面纹理非空存活；空描述符 = 默认视图。
        let view = unsafe { (ctx.api().texture_create_view)(st.texture, ptr::null()) };
        if view.is_null() {
            unsafe { (ctx.api().texture_release)(st.texture) };
            return Err(BackendError::NullHandle("surface texture view"));
        }
        Ok(SurfaceFrame {
            texture: st.texture,
            view,
        })
    }

    /// 呈现当前帧（`acquire` 之后调用；成功返回）。
    pub fn present(&self, ctx: &GpuContext) -> Result<(), BackendError> {
        // SAFETY: surface 存活。
        let status = unsafe { (ctx.api().surface_present)(self.surface) };
        if status != ffi::WGPU_STATUS_SUCCESS {
            return Err(BackendError::ConfigMismatch(format!(
                "呈现失败：status={status}"
            )));
        }
        Ok(())
    }

    /// 释放一帧的表面资源（先视图后纹理；`present` 之后调用）。
    pub fn release_frame(&self, ctx: &GpuContext, frame: SurfaceFrame) {
        // SAFETY: 两个句柄各自非空时释放一次；视图先于纹理。
        unsafe {
            if !frame.view.is_null() {
                (ctx.api().texture_view_release)(frame.view);
            }
            if !frame.texture.is_null() {
                (ctx.api().texture_release)(frame.texture);
            }
        }
    }
}

impl Drop for SurfaceTarget {
    fn drop(&mut self) {
        // SAFETY: 先解除配置再释放表面；各只一次。
        unsafe {
            if !self.surface.is_null() {
                (self.ops.surface_unconfigure)(self.surface);
                (self.ops.surface_release)(self.surface);
                self.surface = ptr::null_mut();
            }
        }
    }
}
