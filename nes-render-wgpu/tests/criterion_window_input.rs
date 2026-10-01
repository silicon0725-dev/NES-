//! T-In 契约回归：窗口字符输入泵（S6.34）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-In-01 | WM_CHAR 经消息泵入队；drain 取走全部且清空；inject 与真实按键同队列同序 |

use nes_render_wgpu::window::{drain_chars, inject_char, Window};

const WM_CHAR: u32 = 0x0102;

#[link(name = "user32")]
extern "system" {
    fn PostMessageW(hwnd: *mut core::ffi::c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
}

#[test]
fn t_in_01_chars_round_trip() {
    // 清空残留（进程级队列，测试串行）。
    let _ = drain_chars();
    let window = Window::open("t-in-01", 256, 128).expect("窗口");
    window.pump();
    assert!(drain_chars().is_empty(), "初始无字符");

    // 注入（程序化路径）+ PostMessage（真实按键路径）—— 同队列按到达序。
    inject_char(b'a' as u32);
    let ok1 = unsafe { PostMessageW(window.hwnd(), WM_CHAR, b'b' as usize, 0) };
    let ok2 = unsafe { PostMessageW(window.hwnd(), WM_CHAR, b'c' as usize, 0) };
    assert!(ok1 != 0 && ok2 != 0, "PostMessageW 应成功");
    window.pump(); // 分发 -> wnd_proc -> 入队
    assert_eq!(drain_chars(), vec![b'a' as u32, b'b' as u32, b'c' as u32]);
    assert!(drain_chars().is_empty(), "drain 后清空");

    // 控制字符也在队列里（编辑语义由宿主解释）。
    inject_char(0x08); // 退格
    inject_char(0x0D); // 回车
    assert_eq!(drain_chars(), vec![0x08, 0x0D]);
}
