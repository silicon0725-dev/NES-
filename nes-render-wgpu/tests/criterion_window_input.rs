//! T-In 契约回归：窗口输入泵（S7.2：平台消息 → 中性事件队列）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-In-01 | 真实消息路径：PostMessageW 的键/字符/鼠标/尺寸消息经泵映射成中性 `InputEvent` 入队，按到达序；drain 取走清空 |
//! | T-In-02 | 程序化注入与真实消息同队列；容量上限满时丢新保旧；`vk_to_key` 映射表 |

use nes_render_api::input::{InputEvent, Key, MouseButton};
use nes_render_wgpu::window::{drain_input, inject_input, vk_to_key, Window};

const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;

const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_SIZE: u32 = 0x0005;

#[link(name = "user32")]
extern "system" {
    fn PostMessageW(hwnd: *mut core::ffi::c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
}

/// T-In-01：真实按键路径（PostMessageW → 泵 → wnd_proc → 映射 → 队列）。
/// 新窗口首泵会带系统自发消息（WM_SIZE 等）—— 初始断言只排除键/字符，
/// 投递断言按"我们的消息在队尾按序"钉（前缀允许系统噪声）。
#[test]
fn t_in_01_events_round_trip() {
    let _ = drain_input(); // 清残留（进程级队列）
    let window = Window::open("t-in-01-events", 256, 128).expect("窗口");
    window.pump();
    let initial = drain_input();
    assert!(
        initial.iter().all(|e| !matches!(e, InputEvent::Key { .. } | InputEvent::Char(_))),
        "无人按键：无键/字符事件（系统消息可有）：{initial:?}"
    );

    // W 按下（VK 0x57，泵的 TranslateMessage 会合成 WM_CHAR 'w' —— 真实
    // 用户的字符到达路径）→ 鼠标移动 (10,20)（lparam 打包）→ 左键按下
    // → 尺寸 (100,50)。
    let lparam_xy = ((20usize << 16) | 10) as isize;
    let lparam_size = ((50usize << 16) | 100) as isize;
    let posts = [
        (WM_KEYDOWN, 0x57, 0isize),
        (WM_MOUSEMOVE, 0, lparam_xy),
        (WM_LBUTTONDOWN, 0, 0),
        (WM_SIZE, 0, lparam_size),
    ];
    for (msg, wp, lp) in posts {
        assert!(unsafe { PostMessageW(window.hwnd(), msg, wp, lp) } != 0);
    }
    window.pump(); // 分发 -> wnd_proc -> 映射入队（含合成字符）
    let drained = drain_input();
    let expected = [
        InputEvent::Key { key: Key::W, down: true },
        InputEvent::MouseMove { x: 10.0, y: 20.0 },
        InputEvent::MouseButton { button: MouseButton::Left, down: true },
        InputEvent::Resize { w: 100, h: 50 },
        InputEvent::Char('w' as u32),
    ];
    // 合成 WM_CHAR 经第二趟队列遍历才派发（TranslateMessage 投递、
    // 下轮 Peek 取走）—— 与其他消息的相对序由 OS 决定，钉**保序子序列**。
    let mut it = drained.iter();
    assert!(
        expected.iter().all(|e| it.any(|d| d == e)),
        "五个事件按序都在（允许系统噪声/合成字符交叠）：{drained:?}"
    );
    assert!(drain_input().is_empty(), "drain 后清空");

    // 抬起 + 控制字符（注入先入队、泵后投递 —— 序如实）。
    assert!(unsafe { PostMessageW(window.hwnd(), WM_KEYUP, 0x57, 0) } != 0);
    inject_input(InputEvent::Char(0x0D));
    window.pump();
    let drained = drain_input();
    let tail = drained[drained.len().saturating_sub(2)..].to_vec();
    assert_eq!(
        tail,
        vec![
            InputEvent::Char(0x0D),
            InputEvent::Key { key: Key::W, down: false },
        ],
        "KEYUP 不再合成字符（泵只翻译按下族）"
    );
}

/// T-In-02：注入/上限/映射表（无窗口依赖的纯部分）。
#[test]
fn t_in_02_inject_cap_and_vk_map() {
    let _ = drain_input();
    for i in 0..1500u32 {
        inject_input(InputEvent::Char(32 + i % 90));
    }
    let drained = drain_input();
    assert_eq!(drained.len(), 1024, "封顶 1024");
    assert_eq!(drained[0], InputEvent::Char(32), "FIFO 头保留");
    assert!(drain_input().is_empty());

    // 映射表：字母/数字/编辑键/方向/修饰（左右不分 → 左变体）/未列举。
    assert_eq!(vk_to_key(0x41), Key::A);
    assert_eq!(vk_to_key(0x5A), Key::Z);
    assert_eq!(vk_to_key(0x30), Key::Num0);
    assert_eq!(vk_to_key(0x39), Key::Num9);
    assert_eq!(vk_to_key(0x27), Key::ArrowRight);
    assert_eq!(vk_to_key(0x0D), Key::Enter);
    assert_eq!(vk_to_key(0x1B), Key::Escape);
    assert_eq!(vk_to_key(0x10), Key::LShift, "Win32 不分左右 → 统一记左");
    assert_eq!(vk_to_key(0x11), Key::LCtrl);
    assert_eq!(vk_to_key(0xBA), Key::Other(0xBA), "未列举保留原码");
}
