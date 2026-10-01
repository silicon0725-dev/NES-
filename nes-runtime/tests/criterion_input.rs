//! T-In-R 契约回归：输入系统接入运行时帧循环（S7.2）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-In-R1 | `collect_input`：注入事件折叠成帧快照（边缘/按住/text/resize）；`emit_input_signals` 把边缘发成标准 `input/*` 信号，脚本 `on "input/key_down"` 当帧收到并写节点 |
//! | T-In-R2 | `mount_input_view` + `key("名")`：按住期间每帧驱动、松开后停 —— 全链（事件队列 -> 折叠 -> 共享快照 -> 探针 -> VM） |

use std::sync::{Mutex, MutexGuard};

use nes_render_api::input::{InputEvent, Key, MouseButton};
use nes_render_api::{FrameInfo, Vec2};
use nes_render_wgpu::window::inject_input;
use nes_runtime::NesRuntime;
use nes_scene::{NodeKind, ScriptVm, Transform2D, Value};

/// 输入事件队列是进程级静态 —— 同文件的用例串行（GPU 串行锁同款纪律）。
static INPUT_LOCK: Mutex<()> = Mutex::new(());

fn lock_input() -> MutexGuard<'static, ()> {
    INPUT_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(64.0, 64.0))
}

/// T-In-R1：快照 + 标准信号。
#[test]
fn t_in_r1_snapshot_and_input_signals() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_input")
        .join("r1");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _guard = lock_input();
    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };

    // 脚本：input/key_down(W) -> 精灵右移（信号式消费者）。
    let sp = {
        let tree = rt.tree_mut();
        let sp = tree.add_node(tree.root(), "sp", NodeKind::Node2D);
        tree.set_local(sp, Transform2D::from_pos(0.0, 0.0));
        let brain = tree.add_node(tree.root(), "brain", NodeKind::Script);
        tree.set_prop(
            brain,
            "source",
            Value::Str("on \"input/key_down\" { if arg == \"W\" { sp.pos += (3.0, 0.0) } }".into()),
        )
        .unwrap();
        tree.apply_pending();
        sp
    };
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(rt.tree_mut()).is_empty());

    // 注入一帧的输入：W 按下、D 按下+抬起、鼠标移动两跳+左键、文本、resize。
    inject_input(InputEvent::Key { key: Key::W, down: true });
    inject_input(InputEvent::Key { key: Key::D, down: true });
    inject_input(InputEvent::Key { key: Key::D, down: false });
    inject_input(InputEvent::MouseMove { x: 10.0, y: 10.0 });
    inject_input(InputEvent::MouseMove { x: 14.0, y: 18.0 });
    inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true });
    inject_input(InputEvent::Char(b'x' as u32));
    inject_input(InputEvent::Resize { w: 32, h: 16 });

    let snap = rt.collect_input();
    assert!(snap.held.contains(&Key::W));
    assert!(snap.pressed.contains(&Key::W) && snap.pressed.contains(&Key::D));
    // D 同帧按下又抬起 = 干净单帧脉冲：pressed 有、held/released 无。
    assert!(!snap.released.contains(&Key::D) && !snap.released.contains(&Key::W));
    assert_eq!(snap.mouse, Vec2::new(14.0, 18.0));
    assert_eq!(snap.mouse_delta, Vec2::new(0.0, 0.0), "首次观测只建基准");
    assert!(snap.buttons_held[MouseButton::Left.index()]);
    assert_eq!(snap.text, vec![b'x' as u32]);
    assert_eq!(snap.resized, Some((32, 16)));

    // 第二帧：鼠标移动（真增量）+ 快照一次性数据已清。
    inject_input(InputEvent::MouseMove { x: 20.0, y: 26.0 });
    let snap2 = rt.collect_input();
    assert_eq!(snap2.mouse_delta, Vec2::new(6.0, 8.0));
    assert!(snap2.text.is_empty() && snap2.resized.is_none());
    assert!(!snap2.pressed.contains(&Key::W), "持续按住不再计边缘");

    // 边缘 -> 标准 input/* 信号 -> 当帧泵交付（信号式消费者）。
    let n = rt.emit_input_signals(&snap) + rt.emit_input_signals(&snap2);
    assert_eq!(
        n,
        6,
        "key_down W/D + mouse_move + mouse_down + text + resized（D 同帧脉冲无 key_up）"
    );
    let out = rt.frame_with(&frame(0), &mut vm).expect("帧");
    assert_eq!(out.stats.driver_errors, 0);
    assert_eq!(
        rt.tree_mut().local(sp).unwrap().pos.x,
        3.0,
        "脚本收到 input/key_down(W)（载荷键名口径）"
    );

    // 清空事件队列，避免跨测试残留。
    let _ = rt.collect_input();
}

/// T-In-R2：探针全链（按住驱动、松开即停）。
#[test]
fn t_in_r2_key_probe_full_chain() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_input")
        .join("r2");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _guard = lock_input();
    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };

    let sp = {
        let tree = rt.tree_mut();
        let sp = tree.add_node(tree.root(), "sp", NodeKind::Node2D);
        tree.set_local(sp, Transform2D::from_pos(0.0, 0.0));
        let brain = tree.add_node(tree.root(), "brain", NodeKind::Script);
        tree.set_prop(
            brain,
            "source",
            Value::Str(
                "on \"go\" { if key(\"ArrowRight\") { sp.pos += (2.0, 0.0) } }".into(),
            ),
        )
        .unwrap();
        tree.apply_pending();
        sp
    };
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(rt.tree_mut()).is_empty());
    rt.mount_input_view(&mut vm); // 装一次（共享槽）

    // 帧 1：按住 ArrowRight + 宿主节拍 go -> 移动。
    inject_input(InputEvent::Key { key: Key::ArrowRight, down: true });
    let _ = rt.collect_input();
    rt.tree_mut().emit_signal("go", Value::I64(0));
    let _ = rt.frame_with(&frame(0), &mut vm).expect("帧 1");
    assert_eq!(rt.tree_mut().local(sp).unwrap().pos.x, 2.0, "按住驱动");

    // 帧 2：仍按住（无新事件）-> 继续移动（探针读按住态，不是边缘）。
    let _ = rt.collect_input();
    rt.tree_mut().emit_signal("go", Value::I64(0));
    let _ = rt.frame_with(&frame(1), &mut vm).expect("帧 2");
    assert_eq!(rt.tree_mut().local(sp).unwrap().pos.x, 4.0, "持续按住持续驱动");

    // 帧 3：松开 -> 停。
    inject_input(InputEvent::Key { key: Key::ArrowRight, down: false });
    let _ = rt.collect_input();
    rt.tree_mut().emit_signal("go", Value::I64(0));
    let _ = rt.frame_with(&frame(2), &mut vm).expect("帧 3");
    assert_eq!(rt.tree_mut().local(sp).unwrap().pos.x, 4.0, "松开即停");
}
