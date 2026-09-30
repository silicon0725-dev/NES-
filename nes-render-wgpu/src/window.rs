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
//! - 不处理 DPI / 重绘 / 输入：S6.1 只需要"有一个能被 surface 呈现的客户区"；
//! - **固定尺寸**：surface 按创建时的客户区配置，窗口 resize 的重配置
//!   （`WM_SIZE` -> `wgpuSurfaceConfigure`）属后续里程碑，见 S6 文档遗留。

use core::ffi::c_void;
use core::ptr;

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const SW_SHOW: i32 = 5;
const WM_DESTROY: u32 = 0x0002;
const WM_QUIT: u32 = 0x0012;
const PM_REMOVE: u32 = 0x0001;

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

/// 窗口过程：只处理销毁（`WM_DESTROY` -> 投递退出消息），其余走默认过程。
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
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
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
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
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
