//! T-In 契约回归：窗口输入泵（S7.2：平台消息 → 中性事件队列）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-In-01 | 真实消息路径：PostMessageW 的键/字符/鼠标/尺寸消息经泵映射成中性 `InputEvent` 入队，按到达序；drain 取走清空 |
//! | T-In-02 | 程序化注入与真实消息同队列；容量上限满时丢新保旧；`vk_to_key` 映射表 |
//! | T-In-03 | 滚轮（S12-3）：WM_MOUSEWHEEL 真实消息与注入同队列，增量按 WHEEL_DELTA 归一成格（+y=向上）；WM_GETMINMAXINFO 钳制最小整窗尺寸 |

use nes_render_api::input::{InputCollector, InputEvent, Key, MouseButton};
use nes_render_api::math::Vec2;
use nes_render_wgpu::window::{drain_input, inject_input, vk_to_key, Window};

use std::sync::Mutex;

/// 进程级事件队列在**测试线程间共享**（static）—— cargo 默认并行跑同
/// 二进制的测试，t_in_02 灌 1500 条期间若窗口测试正在 drain（开窗后清
/// 残留），注入会被偷走（封顶断言偶发 <1024，S12-3 实测）。三测试全
/// 程串行：锁覆盖各自整个测试体。
static TEST_LOCK: Mutex<()> = Mutex::new(());

const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;

const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_SIZE: u32 = 0x0005;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_GETMINMAXINFO: u32 = 0x0024;

#[link(name = "user32")]
extern "system" {
    fn PostMessageW(hwnd: *mut core::ffi::c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn SendMessageW(hwnd: *mut core::ffi::c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
}

/// T-In-01：真实按键路径（PostMessageW → 泵 → wnd_proc → 映射 → 队列）。
/// 新窗口首泵会带系统自发消息（WM_SIZE 等）—— 初始断言只排除键/字符，
/// 投递断言按"我们的消息在队尾按序"钉（前缀允许系统噪声）。
#[test]
fn t_in_01_events_round_trip() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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

/// `MINMAXINFO`（测试侧镜像；与引擎 `wnd_proc` 里的布局一致）。
#[repr(C)]
struct ProbePoint {
    x: i32,
    y: i32,
}

#[repr(C)]
struct ProbeMinMaxInfo {
    pt_reserved: ProbePoint,
    pt_max_size: ProbePoint,
    pt_max_position: ProbePoint,
    pt_min_track_size: ProbePoint,
    pt_max_track_size: ProbePoint,
}

/// T-In-03：滚轮路径（S12-3）—— 真实 WM_MOUSEWHEEL 与注入同队列、按
/// WHEEL_DELTA 归一成格；最小窗口钳制（WM_GETMINMAXINFO 的
/// pt_min_track_size 覆盖客户区 384x240 外扩）。
#[test]
fn t_in_03_wheel_notches_and_min_track() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _ = drain_input();
    let window = Window::open("t-in-03-wheel", 512, 288).expect("窗口");
    window.pump();
    let _ = drain_input(); // 清首泵系统噪声

    // 真实滚轮：wparam 高 16 位装原始增量 —— +240 = 上 2 格；-120 经
    // u16 回绕（0xFF88 << 16），映射侧 as i16 还原成负。注入 0.5 格夹在
    // 中间（与真实消息同队列，S7.2 口径）。
    let wp_up = 240usize << 16;
    let wp_down = ((-120i32) as u16 as usize) << 16;
    assert!(unsafe { PostMessageW(window.hwnd(), WM_MOUSEWHEEL, wp_up, 0) } != 0);
    inject_input(InputEvent::Wheel { x: 0.0, y: 0.5 });
    assert!(unsafe { PostMessageW(window.hwnd(), WM_MOUSEWHEEL, wp_down, 0) } != 0);
    window.pump(); // 分发 -> wnd_proc -> 映射入队

    let drained = drain_input();
    let wheels: Vec<InputEvent> = drained
        .iter()
        .copied()
        .filter(|e| matches!(e, InputEvent::Wheel { .. }))
        .collect();
    assert_eq!(
        wheels,
        vec![
            InputEvent::Wheel { x: 0.0, y: 0.5 }, // 注入先在队头
            InputEvent::Wheel { x: 0.0, y: 2.0 }, // +240 原码
            InputEvent::Wheel { x: 0.0, y: -1.0 }, // -120 回绕还原
        ],
        "三路滚轮按到达序、增量归一成格：{drained:?}"
    );

    // 折叠口径：同帧相加（2.0 + 0.5 - 1.0 = +1.5 格）—— 运行时消费侧。
    let mut c = InputCollector::new();
    for ev in drained {
        c.push(ev);
    }
    let snap = c.frame();
    assert_eq!(snap.wheel, Vec2::new(0.0, 1.5), "+y=向上");

    // 最小窗口：直答 WM_GETMINMAXINFO 后 min_track >= 客户区 384x240
    //（外扩自 AdjustWindowRect）；其余字段保持调用方所填 —— 直答不转发
    // （DefWindowProcW 不回填结构体；真实拖拽场景里系统发消息前已预填
    // 默认值，wnd_proc 只覆写 min_track、原样放行其余字段）。
    // 该消息是同步语义（PostMessageW 拒收 < WM_USER 的系统消息），同线程
    // SendMessageW 直达 wnd_proc —— 指针 lparam 无跨线程问题。
    let sentinel = 123_456_789i32;
    let mut mmi = ProbeMinMaxInfo {
        pt_reserved: ProbePoint { x: sentinel, y: sentinel },
        pt_max_size: ProbePoint { x: sentinel, y: sentinel },
        pt_max_position: ProbePoint { x: sentinel, y: sentinel },
        pt_min_track_size: ProbePoint { x: 0, y: 0 },
        pt_max_track_size: ProbePoint { x: sentinel, y: sentinel },
    };
    unsafe {
        SendMessageW(window.hwnd(), WM_GETMINMAXINFO, 0, &mut mmi as *mut ProbeMinMaxInfo as isize);
    }
    assert!(
        mmi.pt_min_track_size.x >= 384 && mmi.pt_min_track_size.y >= 240,
        "min_track 覆盖客户区下限：{:?}",
        (mmi.pt_min_track_size.x, mmi.pt_min_track_size.y)
    );
    assert_eq!(mmi.pt_reserved.x, sentinel, "reserved 不动");
    assert_eq!(
        (mmi.pt_max_size.x, mmi.pt_max_size.y, mmi.pt_max_position.x, mmi.pt_max_position.y),
        (sentinel, sentinel, sentinel, sentinel),
        "最大化尺寸/位置原样放行（系统默认的预填不被破坏）"
    );
    assert_eq!(
        (mmi.pt_max_track_size.x, mmi.pt_max_track_size.y),
        (sentinel, sentinel),
        "最大追踪尺寸原样放行"
    );
    // 先析构大窗（类名取 title 参数的栈地址，两窗共存会撞注册名 ——
    // 析构即注销类）再开小窗；WM_QUIT 由小窗 open 的首泵顺带吞掉。
    drop(window);

    // 登记口径：开窗时不小于 384x240 的窗口才挂钳制。小窗（256x128，
    // T-Surf-01 同参数）不登记 —— 客户区精确如请求，且 GETMINMAXINFO
    // 直答不触发（字段哨兵原样）。
    let small = Window::open("t-in-03-small", 256, 128).expect("小窗");
    assert_eq!(small.client_size(), (256, 128), "小窗不被钳制顶大");
    let mut probe_small = ProbeMinMaxInfo {
        pt_reserved: ProbePoint { x: sentinel, y: sentinel },
        pt_max_size: ProbePoint { x: sentinel, y: sentinel },
        pt_max_position: ProbePoint { x: sentinel, y: sentinel },
        pt_min_track_size: ProbePoint { x: 0, y: 0 },
        pt_max_track_size: ProbePoint { x: sentinel, y: sentinel },
    };
    unsafe {
        SendMessageW(
            small.hwnd(),
            WM_GETMINMAXINFO,
            0,
            &mut probe_small as *mut ProbeMinMaxInfo as isize,
        );
    }
    assert_eq!(
        (probe_small.pt_min_track_size.x, probe_small.pt_min_track_size.y),
        (0, 0),
        "未登记窗口：min_track 不被覆写（沿默认路径）"
    );
    assert_eq!(probe_small.pt_max_size.x, sentinel, "未登记窗口：其余字段也不动");

    let _ = drain_input(); // 收尾清队列（下一测试自会再清，双保险）
}
