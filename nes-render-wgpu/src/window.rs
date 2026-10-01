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
//! - **输入（S7.2）**：键/字符/鼠标/尺寸消息映射成中性 `InputEvent`
//!   入进程级队列（[`drain_input`]），折叠与消费在契约层/运行时 ——
//!   平台层只投递事实，**WM_CHAR 不是引擎 API**；
//! - 不处理 DPI / 重绘；**固定尺寸**：surface 按创建时的客户区配置，
//!   `WM_SIZE` 只入事件队列（表面重配置属后续里程碑，见 S6 文档遗留）。

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
/// 映射见 [`vk_to_key`]）；其余走默认过程。
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
    if let Some(ev) = input_event_of(msg, wparam, lparam) {
        inject_input(ev);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
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
            if !self.hwnd.is_null() {
                DestroyWindow(self.hwnd);
            }
            UnregisterClassW(self.class_name.as_ptr(), self.hinstance);
        }
    }
}
