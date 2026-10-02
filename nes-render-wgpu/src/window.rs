//! Win32 窗口（S6.1）：手写 FFI 的最小窗口 + 消息泵。
//!
//! # 为什么手写
//!
//! 本 crate 的依赖纪律禁第三方 crate（`winit` 越界）。窗口是 surface 的前置
//! （`WGPUSurfaceSourceWindowsHWND` 要 HWND），所需 Win32 面极小：
//! 注册类、建窗、`PeekMessage` 泵、销毁。全部 `extern "system"` 声明 +
//! `user32`/`kernel32` 隐式链接（Rust 标准库已链），无 build script。
//!
//! # 语义（刻意最小）
//!
//! - 窗口关闭（点 X）=> [`Window::pump`] 返回 `false`，宿主随之退出帧循环；
//! - **输入（S7.2）**：键/字符/鼠标/滚轮/尺寸消息映射成中性 `InputEvent`
//!   入进程级队列（[`drain_input`]），折叠与消费在契约层/运行时 ——
//!   平台层只投递事实，**WM_CHAR 不是引擎 API**；
//! - 不处理 DPI / 重绘；**固定尺寸**：surface 按创建时的客户区配置，
//!   `WM_SIZE` 只入事件队列（表面重配置属后续里程碑，见 S6 文档遗留）；
//! - **最小窗口（S12-3）**：`WM_GETMINMAXINFO` 钳制用户拖拽下限 ——
//!   客户区 384x240 经 `AdjustWindowRect` 外扩的整窗尺寸（防拖窄裁字）。
//!   只对开窗时不小于该下限的窗口生效（小窗/测试替身保精确开窗）。

use core::ffi::c_void;
use core::ptr;
use std::sync::Mutex;

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const SW_SHOW: i32 = 5;
const WM_DESTROY: u32 = 0x0002;
const WM_QUIT: u32 = 0x0012;
const PM_REMOVE: u32 = 0x0001;
// ---- 输入面（S7.2：平台消息 → 中性事件）----
const WM_SIZE: u32 = 0x0005;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
/// Alt 路径的按键按下（字符合成同覆盖）。
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0205;
const WM_RBUTTONUP: u32 = 0x0206;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
/// 滚轮滚动（垂直；wparam 高 16 位是原始增量，一格 = `WHEEL_DELTA` 120）。
const WM_MOUSEWHEEL: u32 = 0x020A;
/// 尺寸极限询问（拖拽/最大化前系统询问窗口的最小/最大尺寸）。
const WM_GETMINMAXINFO: u32 = 0x0024;
/// Win32 滚轮一格的原始增量（映射到"格"的归一分母）。
const WHEEL_DELTA: f32 = 120.0;
/// 最小客户区基准（S12-3：拖拽下限 —— 用户实测把窗口拖窄会裁掉
/// 文本面板的字，钳在 384x240 客户区上）。
const MIN_CLIENT_W: i32 = 384;
const MIN_CLIENT_H: i32 = 240;
// 虚拟键（Win32）。
const VK_BACK: u32 = 0x08;
const VK_TAB: u32 = 0x09;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_RETURN: u32 = 0x0D;
const VK_ESCAPE: u32 = 0x1B;
const VK_SPACE: u32 = 0x20;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;

use nes_render_api::input::{InputEvent, Key, MouseButton};

/// 进程级中性输入事件队列（`wnd_proc` 里平台消息映射后入队；宿主每帧
/// `drain_input` 取走交给 [`nes_render_api::input::InputCollector`]）。
///
/// **单窗口口径**：队列不区分来源窗口（一进程一窗口）；**容量上限**
/// （S7.0 纪律）：宿主不排空也不无界增长，满时丢新。
/// [`inject_input`] 是同队列的程序化入口（自动化测试 / headless 合成）。
/// **WM_CHAR 不是引擎 API**：字符码在这层折成 `InputEvent::Char`，
/// 引擎与脚本消费的是快照的 `text` 字段。
static EVENTS: Mutex<Vec<InputEvent>> = Mutex::new(Vec::new());

/// 施加最小窗口钳制的窗口表（S12-3）：只登记**开窗时客户区不小于**
/// [`MIN_CLIENT_W`]x[`MIN_CLIENT_H`] 的窗口 —— 用户拖拽下限只对真实
/// 产品窗口生效。小窗（测试/离屏替身）不登记：实证 Windows 在创建/
/// 排列阶段就经 `WM_WINDOWPOSCHANGING` 查询 `WM_GETMINMAXINFO`，小窗
/// 一旦挂钳制会被直接顶到 384x240，破坏 T-Surf-01 的"客户区精确等于
/// 请求尺寸"契约（256x128 开窗实测变 384x240）。析构即除名（防句柄
/// 复用串钳制）。
static CLAMPED_WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// 队列容量上限（事件数）。
const EVENTS_CAP: usize = 1024;

/// 取走全部已入队的输入事件（按到达序）。
pub fn drain_input() -> Vec<InputEvent> {
    let mut guard = EVENTS.lock().unwrap_or_else(|p| p.into_inner());
    std::mem::take(&mut *guard)
}

/// 程序化注入一个输入事件（与真实消息同队列；自动化测试用）。
/// 队列满时丢弃（见 [`EVENTS_CAP`]）。
pub fn inject_input(ev: InputEvent) {
    let mut guard = EVENTS.lock().unwrap_or_else(|p| p.into_inner());
    if guard.len() < EVENTS_CAP {
        guard.push(ev);
    }
}

/// Win32 虚拟键 → 中性 [`Key`]。
///
/// Win32 的 `WM_KEYDOWN` 缺省**不分左右修饰**（`VK_SHIFT` 一个码）——
/// 统一记到左侧变体（`LShift` 等），需要区分左右的宿主走增强路径
///（raw input / scancode），属后续。未列举键保留原码（`Other`）。
pub fn vk_to_key(vk: u32) -> Key {
    const LETTERS: [Key; 26] = [
        Key::A, Key::B, Key::C, Key::D, Key::E, Key::F, Key::G, Key::H, //
        Key::I, Key::J, Key::K, Key::L, Key::M, Key::N, Key::O, Key::P, //
        Key::Q, Key::R, Key::S, Key::T, Key::U, Key::V, Key::W, Key::X, //
        Key::Y, Key::Z,
    ];
    const DIGITS: [Key; 10] = [
        Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, //
        Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9,
    ];
    match vk {
        0x41..=0x5A => LETTERS[(vk - 0x41) as usize],
        0x30..=0x39 => DIGITS[(vk - 0x30) as usize],
        VK_BACK => Key::Backspace,
        VK_TAB => Key::Tab,
        VK_SHIFT => Key::LShift,
        VK_CONTROL => Key::LCtrl,
        VK_MENU => Key::LAlt,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_SPACE => Key::Space,
        VK_LEFT => Key::ArrowLeft,
        VK_UP => Key::ArrowUp,
        VK_RIGHT => Key::ArrowRight,
        VK_DOWN => Key::ArrowDown,
        other => Key::Other(other),
    }
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

/// `MSG`（x64 布局：句柄 8 + 消息 4 + 填充 4 + wparam 8 + lparam 8 + time 4 + pt 8 + 填充 4）。
#[repr(C)]
struct Msg {
    hwnd: *mut c_void,
    message: u32,
    _pad0: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt: Point,
    _pad1: u32,
}

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// `MINMAXINFO`（`WM_GETMINMAXINFO` 的出参；5 组 POINT，复用 [`Point`]）。
#[repr(C)]
struct MinMaxInfo {
    pt_reserved: Point,
    pt_max_size: Point,
    pt_max_position: Point,
    /// 用户拖拽的最小追踪尺寸（整窗口径，含边框）—— 本引擎唯一覆写的字段。
    pt_min_track_size: Point,
    pt_max_track_size: Point,
}

#[repr(C)]
struct WndClassW {
    style: u32,
    lpfn_wnd_proc: Option<unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize>,
    cls_extra: i32,
    wnd_extra: i32,
    hinstance: *mut c_void,
    icon: *mut c_void,
    cursor: *mut c_void,
    background: *mut c_void,
    menu_name: *const u16,
    class_name: *const u16,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: *mut c_void,
        menu: *mut c_void,
        instance: *mut c_void,
        param: *mut c_void,
    ) -> *mut c_void;
    fn DefWindowProcW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn PeekMessageW(msg: *mut Msg, hwnd: *mut c_void, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
    fn DestroyWindow(hwnd: *mut c_void) -> i32;
    fn UnregisterClassW(name: *const u16, instance: *mut c_void) -> i32;
    fn PostQuitMessage(code: i32);
    fn GetClientRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
    fn AdjustWindowRect(rect: *mut Rect, style: u32, menu: i32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

/// 窗口过程：销毁 → 投递退出消息；输入消息 → 中性事件入队（S7.2，
/// 映射见 [`vk_to_key`]）；最小尺寸询问 → 钳制拖拽下限（S12-3）；其余
/// 走默认过程。
unsafe extern "system" fn wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return 0;
    }
    if msg == WM_GETMINMAXINFO {
        // 最小窗口钳制（S12-3）：只对登记过的窗口（开窗时不小于下限的
        // 产品窗口）直答 —— 系统发送该消息前已把 MINMAXINFO 填好默认值
        //（最大化尺寸/位置、追踪上下限），这里只覆写最小追踪尺寸、其余
        // 字段原样放行，按文档返回 0。（不转发 DefWindowProcW：实测它
        // 不回填结构体，主动 SendMessage 时转发拿到的是全零。）
        if lparam != 0 && window_is_clamped(hwnd) {
            let mmi = unsafe { &mut *(lparam as *mut MinMaxInfo) };
            mmi.pt_min_track_size = min_window_outer_size();
            return 0;
        }
        // 未登记窗口沿默认路径（系统下限 ~132x38，小窗精确开窗不受扰）。
    }
    if let Some(ev) = input_event_of(msg, wparam, lparam) {
        inject_input(ev);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// 该窗口是否登记了最小钳制（[`CLAMPED_WINDOWS`] 成员查询；`wnd_proc`
/// 是静态 extern，无 per-window 状态可挂，用进程级表映射句柄）。
fn window_is_clamped(hwnd: *mut c_void) -> bool {
    let guard = CLAMPED_WINDOWS.lock().unwrap_or_else(|p| p.into_inner());
    guard.contains(&(hwnd as isize))
}

/// 最小整窗尺寸（`pt_min_track_size` 口径）：客户区 [`MIN_CLIENT_W`]x
/// [`MIN_CLIENT_H`] 经 `AdjustWindowRect(WS_OVERLAPPEDWINDOW)` 按当前
/// 系统度量外扩 —— 与 [`Window::open`] 同一换算（硬编码边框补偿在不同
/// 主题/DPI 下会偏，S6.1 实证）。纯计算、无窗口状态依赖，静态
/// `wnd_proc` 现场调用即可，不需要 OnceLock。
fn min_window_outer_size() -> Point {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: MIN_CLIENT_W,
        bottom: MIN_CLIENT_H,
    };
    // SAFETY: rect 是合法出参；失败（返回 0）时保持客户区原值兜底。
    unsafe { AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, 0) };
    Point {
        x: rect.right - rect.left,
        y: rect.bottom - rect.top,
    }
}

/// 平台消息 → 中性输入事件（非输入消息返回 `None`）。
///
/// lparam 打包口径：鼠标 x/y 各 16 位有符号（客户区像素，多显示器可
/// 为负）；WM_SIZE 宽高各 16 位无符号。WM_CHAR 的 wparam 是 UTF-16
/// 单元原码（代理对重组属后续，见 S7.2 文档遗留）。
fn input_event_of(msg: u32, wparam: usize, lparam: isize) -> Option<InputEvent> {
    let lo = (lparam & 0xFFFF) as u16;
    let hi = ((lparam >> 16) & 0xFFFF) as u16;
    match msg {
        WM_KEYDOWN => Some(InputEvent::Key { key: vk_to_key(wparam as u32), down: true }),
        WM_KEYUP => Some(InputEvent::Key { key: vk_to_key(wparam as u32), down: false }),
        WM_CHAR => Some(InputEvent::Char(wparam as u32)),
        WM_MOUSEMOVE => Some(InputEvent::MouseMove {
            x: lo as i16 as f32,
            y: hi as i16 as f32,
        }),
        WM_LBUTTONDOWN => Some(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
        WM_LBUTTONUP => Some(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
        WM_RBUTTONDOWN => Some(InputEvent::MouseButton { button: MouseButton::Right, down: true }),
        WM_RBUTTONUP => Some(InputEvent::MouseButton { button: MouseButton::Right, down: false }),
        WM_MBUTTONDOWN => Some(InputEvent::MouseButton { button: MouseButton::Middle, down: true }),
        WM_MBUTTONUP => Some(InputEvent::MouseButton { button: MouseButton::Middle, down: false }),
        WM_SIZE => Some(InputEvent::Resize {
            w: lo as u32,
            h: hi as u32,
        }),
        WM_MOUSEWHEEL => Some(InputEvent::Wheel {
            // 垂直增量归一到格（+WHEEL_DELTA = +1 格，向上）；水平滚轮
            // 源不存在，x 恒 0（字段保留）。鼠标位置不入事件 —— 路由用
            // 快照既有 mouse（消息 lparam 的屏幕坐标口径不同，且同帧
            // WM_MOUSEMOVE 已经在维护它）。
            x: 0.0,
            y: ((wparam >> 16) as u16 as i16) as f32 / WHEEL_DELTA,
        }),
        _ => None,
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 一个已创建并显示的 Win32 窗口。
///
/// 析构时销毁窗口并注销窗口类；句柄从此失效（宿主不得缓存）。
pub struct Window {
    hwnd: *mut c_void,
    hinstance: *mut c_void,
    class_name: Vec<u16>,
}

impl Window {
    /// 创建并显示窗口（`width`/`height` 是**客户区**期望尺寸； decorations 会
    /// 按默认边框外扩）。
    pub fn open(title: &str, width: u32, height: u32) -> Result<Self, crate::error::BackendError> {
        let hinstance = unsafe { GetModuleHandleW(ptr::null()) };
        if hinstance.is_null() {
            return Err(crate::error::BackendError::NullHandle("HINSTANCE"));
        }
        // 类名带地址后缀避免同进程多次注册冲突（示例/测试会开多个窗口）。
        let class_name = wide(&format!("nes_wgpu_window_{:p}", &title));
        let class = WndClassW {
            style: 0,
            lpfn_wnd_proc: Some(wnd_proc),
            cls_extra: 0,
            wnd_extra: 0,
            hinstance,
            icon: ptr::null_mut(),
            cursor: ptr::null_mut(),
            background: ptr::null_mut(),
            menu_name: ptr::null(),
            class_name: class_name.as_ptr(),
        };
        let atom = unsafe { RegisterClassW(&class) };
        if atom == 0 {
            return Err(crate::error::BackendError::NullHandle("RegisterClassW"));
        }
        let title_w = wide(title);
        // 按当前系统度量把客户区期望尺寸外扩成整窗尺寸（AdjustWindowRect 原地
        // 扩张 RECT）。不用硬编码边框补偿 —— 那在不同主题/DPI 下会偏（S6.1
        // 实测过 +16/+39 得到 518x294 客户区而非 512x288）。
        let mut rect = Rect {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        if unsafe { AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, 0) } == 0 {
            unsafe { UnregisterClassW(class_name.as_ptr(), hinstance) };
            return Err(crate::error::BackendError::NullHandle("AdjustWindowRect"));
        }
        let outer_w = rect.right - rect.left;
        let outer_h = rect.bottom - rect.top;
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                title_w.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0x8000_0000u32 as i32, // CW_USEDEFAULT
                0x8000_0000u32 as i32,
                outer_w,
                outer_h,
                ptr::null_mut(),
                ptr::null_mut(),
                hinstance,
                ptr::null_mut(),
            )
        };
        if hwnd.is_null() {
            unsafe { UnregisterClassW(class_name.as_ptr(), hinstance) };
            return Err(crate::error::BackendError::NullHandle("CreateWindowExW"));
        }
        // 客户区请求不小于下限的窗口登记拖拽钳制（S12-3）；小窗不登记
        //（测试/离屏替身保"客户区精确等于请求尺寸"，见 CLAMPED_WINDOWS）。
        if width >= MIN_CLIENT_W as u32 && height >= MIN_CLIENT_H as u32 {
            let mut guard = CLAMPED_WINDOWS.lock().unwrap_or_else(|p| p.into_inner());
            guard.push(hwnd as isize);
        }
        unsafe { ShowWindow(hwnd, SW_SHOW) };
        // 泵一遍待处理消息（含首帧绘制），让窗口真正上屏。
        let window = Self {
            hwnd,
            hinstance,
            class_name,
        };
        let _ = window.pump();
        Ok(window)
    }

    /// 窗口句柄（surface 源用）。
    pub fn hwnd(&self) -> *mut c_void {
        self.hwnd
    }

    /// 进程实例句柄（surface 源用）。
    pub fn hinstance(&self) -> *mut c_void {
        self.hinstance
    }

    /// 客户区尺寸（像素；surface 配置以此为准）。
    pub fn client_size(&self) -> (u32, u32) {
        let mut rect = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: hwnd 存活且 rect 是合法出参。
        if unsafe { GetClientRect(self.hwnd, &mut rect) } == 0 {
            return (0, 0);
        }
        ((rect.right - rect.left).max(0) as u32, (rect.bottom - rect.top).max(0) as u32)
    }

    /// 泵一轮消息。返回 `false` 表示窗口已关闭（收到退出消息），宿主应停止帧循环。
    pub fn pump(&self) -> bool {
        let mut msg = Msg {
            hwnd: ptr::null_mut(),
            message: 0,
            _pad0: 0,
            w_param: 0,
            l_param: 0,
            time: 0,
            pt: Point { x: 0, y: 0 },
            _pad1: 0,
        };
        loop {
            // SAFETY: msg 是合法出参；null hwnd = 取本线程所有窗口的消息。
            let has = unsafe { PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) };
            if has == 0 {
                return true;
            }
            if msg.message == WM_QUIT {
                return false;
            }
            // 只对按键按下族做 Translate（字符合成只应来自按下 —— 带异常
            // lparam 的 KEYUP / 注入消息不再产生幻影字符；S7.2 实证）。
            if msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN {
                unsafe { TranslateMessage(&msg) };
            }
            unsafe { DispatchMessageW(&msg) };
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: 句柄存活且只在此处销毁一次；先销毁窗口再注销类。
        unsafe {
            // 除名钳制登记（先于销毁 —— 防句柄复用把钳制串给新窗口）。
            let mut guard = CLAMPED_WINDOWS.lock().unwrap_or_else(|p| p.into_inner());
            guard.retain(|h| *h != self.hwnd as isize);
            drop(guard);
            if !self.hwnd.is_null() {
                DestroyWindow(self.hwnd);
            }
            UnregisterClassW(self.class_name.as_ptr(), self.hinstance);
        }
    }
}
