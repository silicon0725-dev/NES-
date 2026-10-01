//! T-In-C 契约回归：输入折叠器（S7.2，纯数据层无平台依赖）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-In-C01 | 键边缘：按下/抬起按集合差算；**自动重发幂等**（重复 down 不二次 pressed）；held 含本帧新按 |
//! | T-In-C02 | 鼠标：位置覆盖、delta 相对上一快照（首帧 0）；按钮三态（held/pressed/released） |
//! | T-In-C03 | 文本与窗口尺寸是**一次性**数据（快照取走即清）；键名/按钮名 round-trip；is_down 未列举名 = 未按 |

use nes_render_api::input::{InputCollector, InputEvent, Key, MouseButton};
use nes_render_api::math::Vec2;

/// T-In-C01：键盘边缘与重发幂等。
#[test]
fn t_in_c01_key_edges_and_repeat_idempotent() {
    let mut c = InputCollector::new();
    // 帧 1：W 按下（含自动重发 ×3 —— 集合语义天然幂等）。
    for _ in 0..4 {
        c.push(InputEvent::Key { key: Key::W, down: true });
    }
    c.push(InputEvent::Key { key: Key::LShift, down: true });
    let s1 = c.frame();
    assert!(s1.held.contains(&Key::W) && s1.held.contains(&Key::LShift));
    assert!(s1.pressed.contains(&Key::W) && s1.pressed.contains(&Key::LShift));
    assert!(s1.released.is_empty());

    // 帧 2：只有重发（无新边缘）。
    c.push(InputEvent::Key { key: Key::W, down: true });
    let s2 = c.frame();
    assert!(s2.held.contains(&Key::W), "仍按住");
    assert!(s2.pressed.is_empty(), "重发不产生第二次 pressed");
    assert!(s2.released.is_empty());

    // 帧 3：W 抬起。
    c.push(InputEvent::Key { key: Key::W, down: false });
    let s3 = c.frame();
    assert!(!s3.held.contains(&Key::W));
    assert!(s3.released.contains(&Key::W));
    assert!(s3.held.contains(&Key::LShift), "其他键不受影响");

    // 帧 4：**同帧脉冲** —— D 按下又抬起（快点击/注入节奏 < 一帧）：
    // pressed 如实记录，held=false，不产生 released（干净单帧脉冲）。
    c.push(InputEvent::Key { key: Key::D, down: true });
    c.push(InputEvent::Key { key: Key::D, down: false });
    let s4 = c.frame();
    assert!(s4.pressed.contains(&Key::D), "脉冲可见");
    assert!(!s4.held.contains(&Key::D));
    assert!(!s4.released.contains(&Key::D), "从未按住过 → 无 released 边缘");
}

/// T-In-C02：鼠标位置、增量、按钮三态。
#[test]
fn t_in_c02_mouse_position_delta_buttons() {
    let mut c = InputCollector::new();
    // 帧 1：移动到 (10,20)（首帧 delta = 0）+ 左键按下。
    c.push(InputEvent::MouseMove { x: 10.0, y: 20.0 });
    c.push(InputEvent::MouseButton { button: MouseButton::Left, down: true });
    let s1 = c.frame();
    assert_eq!(s1.mouse, Vec2::new(10.0, 20.0));
    assert_eq!(s1.mouse_delta, Vec2::new(0.0, 0.0), "首帧无增量基准");
    assert!(s1.buttons_held[MouseButton::Left.index()]);
    assert!(s1.buttons_pressed[MouseButton::Left.index()]);

    // 帧 2：移动 (30,50) —— delta = (20,30)；左键保持（无边缘）。
    c.push(InputEvent::MouseMove { x: 30.0, y: 50.0 });
    let s2 = c.frame();
    assert_eq!(s2.mouse_delta, Vec2::new(20.0, 30.0));
    assert!(s2.buttons_held[MouseButton::Left.index()]);
    assert!(!s2.buttons_pressed[MouseButton::Left.index()]);

    // 帧 3：左键抬起 + 右键按下（同帧双向）。
    c.push(InputEvent::MouseButton { button: MouseButton::Left, down: false });
    c.push(InputEvent::MouseButton { button: MouseButton::Right, down: true });
    let s3 = c.frame();
    assert!(!s3.buttons_held[MouseButton::Left.index()]);
    assert!(s3.buttons_released[MouseButton::Left.index()]);
    assert!(s3.buttons_pressed[MouseButton::Right.index()]);
    assert_eq!(s3.mouse_delta, Vec2::new(0.0, 0.0), "无移动事件无增量");
}

/// T-In-C03：一次性数据、名字 round-trip、is_down 口径。
#[test]
fn t_in_c03_transient_data_and_names() {
    let mut c = InputCollector::new();
    c.push(InputEvent::Char(b'h' as u32));
    c.push(InputEvent::Char(0x0D));
    c.push(InputEvent::Resize { w: 768, h: 432 });
    let s1 = c.frame();
    assert_eq!(s1.text, vec![b'h' as u32, 0x0D]);
    assert_eq!(s1.resized, Some((768, 432)));

    // 帧 2 无新事件：text 空、resize None（取走即清）。
    let s2 = c.frame();
    assert!(s2.text.is_empty());
    assert_eq!(s2.resized, None);

    // 名字 round-trip（信号载荷与脚本探针共用）。
    for k in [Key::W, Key::ArrowRight, Key::Num7, Key::Escape, Key::RCtrl] {
        assert_eq!(Key::from_name(&k.name()), Some(k), "{k:?}");
    }
    assert_eq!(Key::from_name("Other(99)"), None, "未列举名不解析");
    assert_eq!(MouseButton::from_name(MouseButton::Middle.name()), Some(MouseButton::Middle));

    // is_down：held 里的键真、未列举名假。
    c.push(InputEvent::Key { key: Key::W, down: true });
    let s3 = c.frame();
    assert!(s3.is_down("W"));
    assert!(!s3.is_down("ArrowLeft"));
    assert!(!s3.is_down("NoSuchKey"), "未列举 = 未按（不猜）");
}

/// T-In-C04：轨迹解析（S7.3）—— 全事件族 round-trip、注释/空行、
/// 同帧合并、未知记法/未知键名如实报错带行号。
#[test]
fn t_in_c04_parse_trace() {
    use nes_render_api::input::parse_trace;
    let text = "# 注释\n\n0 key_down W mouse_move 10 20\n1 char 104 char 13\n2 key_up W mouse_down left resize 800 600\n0 key_down LShift";
    let trace = parse_trace(text).expect("解析");
    assert_eq!(trace.len(), 3, "帧 0 两行合并");
    assert_eq!(trace[0].frame, 0);
    assert_eq!(
        trace[0].events,
        vec![
            InputEvent::Key { key: Key::W, down: true },
            InputEvent::MouseMove { x: 10.0, y: 20.0 },
            InputEvent::Key { key: Key::LShift, down: true },
        ],
        "同帧合并按出现序"
    );
    assert_eq!(trace[1].events, vec![InputEvent::Char(104), InputEvent::Char(13)]);
    assert_eq!(trace[2].events.len(), 3);

    // 错误口径：未知键名 / 未知事件 / 缺参数 —— 指名行。
    assert!(parse_trace("0 key_down NOPE").unwrap_err().contains("未知键名"));
    assert!(parse_trace("0 teleport").unwrap_err().contains("未知事件"));
    assert!(parse_trace("0 key_down").unwrap_err().contains("缺名"));
    assert!(parse_trace("later key_down W").unwrap_err().contains("帧号"));
    assert!(parse_trace("").unwrap().is_empty(), "空文本 = 空轨迹");
}
