//! 后端错误类型。
//!
//! 设计原则：**每一个失败点都必须能指名道姓**。S4.1 的验收要求里有一条
//! "若 wgpu-native 加载或驱动环境不可用，必须如实报告阻塞点与已排除的原因"，
//! 因此本 crate 不用 `anyhow` 式字符串吞掉上下文，而是把"哪一步、哪个符号、
//! 哪个状态码"逐项展开，测试与封口文档都直接引用这些字段。

use std::fmt;
use std::path::PathBuf;

/// 后端可报告的失败点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// 指定的 wgpu-native 动态库路径不存在。
    LibraryNotFound(PathBuf),
    /// 动态库存在但 `LoadLibraryW` 失败，`code` 为 Win32 `GetLastError()`。
    LibraryLoad {
        /// 尝试加载的库路径。
        path: PathBuf,
        /// Win32 错误码（`GetLastError()`，在 `LoadLibraryW` 之后立刻读取）。
        code: u32,
    },
    /// 动态库加载成功，但缺少本后端所需的某个导出符号。
    ///
    /// 这条错误是**资产版本探针**：只要 wgpu-native 资产换代导致符号改名/删除，
    /// 这里会立刻指名，而不是运行到一半才崩。
    MissingSymbol(String),
    /// 某个 wgpu 句柄创建后为 `NULL`（驱动侧静默失败）。
    NullHandle(&'static str),
    /// 适配器请求失败（异步回调返回非 Success）。
    AdapterRequestFailed {
        /// `WGPURequestAdapterStatus` 原始值。
        status: i32,
        /// 驱动回传的诊断串（未取到则为空串）。
        message: String,
    },
    /// 设备请求失败（异步回调返回非 Success）。
    DeviceRequestFailed {
        /// `WGPURequestDeviceStatus` 原始值。
        status: i32,
        /// 驱动回传的诊断串。
        message: String,
    },
    /// 异步回调在超时前没有返回。
    Timeout(&'static str),
    /// 缓冲区映射失败（`WGPUMapAsyncStatus` 非 Success）。
    MapFailed {
        /// `WGPUMapAsyncStatus` 原始值。
        status: i32,
        /// 驱动回传的诊断串（未取到则为空串）。
        message: String,
    },
    /// 装配参数自相矛盾：尺寸 / 用量 / 布局约束在 WebGPU 校验规则下不可能成立。
    ///
    /// 这类失败是**调用方配置错误**（例如渲染目标宽度不满足读回所需的 4 字节行对齐），
    /// 与"驱动故障"分开报，免得把配置问题误诊成环境问题。
    ConfigMismatch(String),
    /// 命令流不合法（例如结尾缺少 `RenderCommand::Submit`）。
    MalformedCommandStream(&'static str),
    /// 像素缓冲尺寸与目标区域不符（写 PNG 前的自检）。
    PixelBufferSize {
        /// 期望字节数。
        expected: usize,
        /// 实际字节数。
        actual: usize,
    },
    /// PNG 尺寸非法（0 或超出 PNG 规范上限）。
    InvalidImageSize {
        /// 宽度。
        width: u32,
        /// 高度。
        height: u32,
    },
    /// 文件系统错误（PNG 落盘）。
    Io(String),
    /// 本机未找到 wgpu-native 动态库（给出所有已排除的候选路径）。
    NoLibraryCandidates(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LibraryNotFound(p) => write!(f, "wgpu-native 动态库不存在：{}", p.display()),
            Self::LibraryLoad { path, code } => write!(
                f,
                "LoadLibraryW 失败：{}（Win32 错误码 {code}）",
                path.display()
            ),
            Self::MissingSymbol(name) => {
                write!(f, "wgpu-native 缺少导出符号：{name}（资产版本与本绑定不匹配）")
            }
            Self::NullHandle(what) => write!(f, "wgpu 句柄创建后为 NULL：{what}"),
            Self::AdapterRequestFailed { status, message } => write!(
                f,
                "wgpuRequestAdapter 失败：status={status} message=\"{message}\""
            ),
            Self::DeviceRequestFailed { status, message } => write!(
                f,
                "wgpuAdapterRequestDevice 失败：status={status} message=\"{message}\""
            ),
            Self::Timeout(tag) => write!(f, "等待异步回调超时：{tag}"),
            Self::MapFailed { status, message } => write!(
                f,
                "wgpuBufferMapAsync 失败：status={status} message=\"{message}\""
            ),
            Self::ConfigMismatch(why) => write!(f, "装配参数自相矛盾：{why}"),
            Self::MalformedCommandStream(why) => write!(f, "命令流不合法：{why}"),
            Self::PixelBufferSize { expected, actual } => write!(
                f,
                "像素缓冲尺寸不符：期望 {expected} 字节，实际 {actual} 字节"
            ),
            Self::InvalidImageSize { width, height } => {
                write!(f, "非法图像尺寸：{width}x{height}")
            }
            Self::Io(msg) => write!(f, "IO 错误：{msg}"),
            Self::NoLibraryCandidates(tried) => {
                write!(f, "未找到 wgpu-native 动态库，已尝试：{tried}")
            }
        }
    }
}

impl std::error::Error for BackendError {}

impl From<std::io::Error> for BackendError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}
