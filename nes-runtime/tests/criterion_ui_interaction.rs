//! T-UI-R 契约回归：UI 交互全链接入运行时（S12.1）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-UI-R1 | 全链悬停/按下/激活：`inject_input` 注入 MouseMove/MouseButton（与真实消息同队列）-> `collect_input` 折叠快照 -> UiVm 共享读面命中 -> `frame_with` 内 `ui_vm.update` + 提取层四态路径 -> 悬停/按下状态与激活钩子如实反映 |
//! | T-UI-R2 | 命中仲裁负例：按钮矩形外按下-抬起不激活、不悬停（标准 UI 语义） |
//! | T-UI-R3 | 全链文本输入（S12-2 真实运行时路径）：点击 TextInput 夺焦 -> `Char` 注入经 `SnapshotView::text` 读面泵进草稿 -> Enter 提交经 `on_commit` 回调整体值 -> 属性表零直写（UiVm 零写权） |
//! | T-UI-R4 | 鼠标点击重命名全链回归：MouseMove+左键按下/抬键 -> 焦点建立 -> 字符落草稿 -> Enter 提交进 rename_sink -> `Inspector::modify_name` 一条 Modified 事务落账 + undo 还原 |

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

use nes_render_api::input::{InputEvent, Key, MouseButton};
use nes_render_api::{FrameInfo, Vec2};
use nes_render_wgpu::window::inject_input;
use nes_runtime::NesRuntime;
use nes_scene::{NodeKind, NoObserver, NodeId, Value, Vec2 as SVec2};

/// 输入事件队列是进程级静态 —— 同文件的用例串行（与 criterion_input 同款锁）。
static INPUT_LOCK: Mutex<()> = Mutex::new(());

fn lock_input() -> MutexGuard<'static, ()> {
    INPUT_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(64.0, 64.0))
}

/// 视口 64x64，按钮矩形 = offset(10,10) + size(20,20) = (10,10,20,20)。
const INSIDE: (f32, f32) = (15.0, 15.0);
const OUTSIDE: (f32, f32) = (50.0, 50.0);

/// 搭一个单按钮场景，返回按钮 NodeId。
fn build_button(rt: &mut NesRuntime) -> NodeId {
    let tree = rt.tree_mut();
    let b = tree.add_node(tree.root(), "btn", NodeKind::Button);
    tree.set_prop(b, "offset", Value::Vec2(SVec2::new(10.0, 10.0)))
        .unwrap();
    tree.set_prop(b, "size", Value::Vec2(SVec2::new(20.0, 20.0)))
        .unwrap();
    tree.set_prop(b, "text", Value::Str("OK".into())).unwrap();
    tree.apply_pending();
    b
}

/// 一帧完整宿主序：collect -> emit 信号 -> frame_with（内含 ui_vm.update
/// + 提取 + GPU 消费 —— 真实帧路径）。
fn step(rt: &mut NesRuntime, index: u64) {
    let snap = rt.collect_input();
    let _ = rt.emit_input_signals(&snap);
    let out = rt.frame_with(&frame(index), &mut NoObserver).expect("frame");
    assert_eq!(out.stats.driver_errors, 0, "extraction path clean");
}

/// T-UI-R1：移入 -> 悬停；左键按下 -> pressed；抬起命中 -> 激活钩子。
#[test]
fn t_ui_r1_hover_press_activate_full_chain() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_ui")
        .join("r1");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _guard = lock_input();
    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[skip GPU case] wgpu-native library not found");
        return;
    };

    let btn = build_button(&mut rt);

    // 激活钩子：NodeId 记入共享缓冲（s12_widgets 示例同款接线）。
    let activated: Rc<RefCell<Vec<NodeId>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = activated.clone();
    rt.ui_vm_mut().on_activate(move |node| {
        sink.borrow_mut().push(node);
    });

    // 帧 0：无事件（鼠标未观测 = (0,0)，在按钮矩形外）—— 全 false。
    step(&mut rt, 0);
    let st = rt.ui_vm_mut().state(btn);
    assert!(!st.hover && !st.pressed, "no input, no hover");

    // 帧 1：鼠标移入按钮矩形（真实消息同队列注入）—— hover 亮、pressed 灭。
    inject_input(InputEvent::MouseMove { x: INSIDE.0, y: INSIDE.1 });
    step(&mut rt, 1);
    let st = rt.ui_vm_mut().state(btn);
    assert!(st.hover, "mouse inside button rect -> hover");
    assert!(!st.pressed, "button not held yet");

    // 帧 2：左键按下（按住态）—— pressed 亮（按下沿当帧生效）。
    inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true });
    step(&mut rt, 2);
    let st = rt.ui_vm_mut().state(btn);
    assert!(st.hover && st.pressed, "left held inside -> hover + pressed");
    assert!(activated.borrow().is_empty(), "no activation while held");

    // 帧 3：左键抬起且仍命中 —— 完整一次按下-抬起，激活钩子触发。
    inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false });
    step(&mut rt, 3);
    let st = rt.ui_vm_mut().state(btn);
    assert!(st.hover, "still inside -> hover stays");
    assert!(!st.pressed, "released -> pressed clears");
    assert_eq!(
        activated.borrow().as_slice(),
        [btn],
        "full press-release inside -> on_activate fires once with button id"
    );

    // 帧 4：鼠标移出 —— 悬停熄灭（快照按住态跨帧保持的负证）。
    inject_input(InputEvent::MouseMove { x: OUTSIDE.0, y: OUTSIDE.1 });
    step(&mut rt, 4);
    let st = rt.ui_vm_mut().state(btn);
    assert!(!st.hover && !st.pressed, "mouse left rect -> no hover");

    // 清空事件队列，避免跨测试残留。
    let _ = rt.collect_input();
}

/// T-UI-R2：矩形外完整一次按下-抬起 —— 不激活、不悬停。
#[test]
fn t_ui_r2_press_outside_rect_never_activates() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_ui")
        .join("r2");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _guard = lock_input();
    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[skip GPU case] wgpu-native library not found");
        return;
    };

    let btn = build_button(&mut rt);
    let activated: Rc<RefCell<Vec<NodeId>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = activated.clone();
    rt.ui_vm_mut().on_activate(move |node| {
        sink.borrow_mut().push(node);
    });

    // 移到矩形外按下再抬起（同帧脉冲口径：held 帧末=false，但 down/up
    // 分两帧注入 —— 走标准按下目标/抬键命中两条路径）。
    inject_input(InputEvent::MouseMove { x: OUTSIDE.0, y: OUTSIDE.1 });
    inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true });
    step(&mut rt, 0);
    let st = rt.ui_vm_mut().state(btn);
    assert!(!st.hover && !st.pressed, "outside rect: no hover no press");

    inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false });
    step(&mut rt, 1);
    let st = rt.ui_vm_mut().state(btn);
    assert!(!st.hover && !st.pressed);
    assert!(
        activated.borrow().is_empty(),
        "press-release outside rect -> no activation"
    );

    // 清空事件队列，避免跨测试残留。
    let _ = rt.collect_input();
}

/// T-UI-R3：全链文本输入 —— 点击夺焦 -> 字符注入进草稿 -> Enter 提交
/// 经 `on_commit` 回调；属性表零直写。输入框矩形 = offset(10,10) +
/// size(30,10)，命中点 (15,12)。
#[test]
fn t_ui_r3_text_input_focus_type_enter_commit() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_ui")
        .join("r3");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _guard = lock_input();
    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[skip GPU case] wgpu-native library not found");
        return;
    };

    let input_box = {
        let tree = rt.tree_mut();
        let t = tree.add_node(tree.root(), "rename", NodeKind::TextInput);
        tree.set_prop(t, "offset", Value::Vec2(SVec2::new(10.0, 10.0)))
            .unwrap();
        tree.set_prop(t, "size", Value::Vec2(SVec2::new(30.0, 10.0)))
            .unwrap();
        tree.set_prop(t, "text", Value::Str(String::new())).unwrap();
        tree.apply_pending();
        t
    };

    // 提交钩子：(NodeId, Value::Str) 记入共享缓冲（宿主帧后落账的
    // editor_shell 同款接线 —— UiVm 零写权）。
    let committed: Rc<RefCell<Vec<(NodeId, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = committed.clone();
    rt.ui_vm_mut().on_commit(move |node, value| {
        if let Value::Str(s) = value {
            sink.borrow_mut().push((node, s));
        }
    });

    // 帧 0：无事件 —— 无焦点、无提交。
    step(&mut rt, 0);
    assert_eq!(rt.ui_vm_mut().focus(), None, "no click, no focus");
    assert!(committed.borrow().is_empty());

    // 帧 1：鼠标移入输入框矩形。
    inject_input(InputEvent::MouseMove { x: 15.0, y: 12.0 });
    step(&mut rt, 1);

    // 帧 2：左键按下 —— 命中 TextInput 夺焦，编辑会话开启（草稿 =
    // 已提交值 ""）。
    inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true });
    step(&mut rt, 2);
    assert_eq!(
        rt.ui_vm_mut().focus(),
        Some(input_box),
        "click on TextInput -> focus"
    );
    assert_eq!(
        rt.ui_vm_mut().text_state(input_box).map(|ts| ts.draft),
        Some(String::new()),
        "session starts with committed value as draft"
    );

    // 帧 3：左键抬起（仍命中 —— 不失焦）。
    inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false });
    step(&mut rt, 3);
    assert_eq!(rt.ui_vm_mut().focus(), Some(input_box), "release inside -> focus stays");

    // 帧 4：注入字符 'h' 'i'（真实 UTF-16 消息序）—— 经快照 ->
    // SnapshotView::text 读面 -> 草稿光标处插入。
    inject_input(InputEvent::Char('h' as u32));
    inject_input(InputEvent::Char('i' as u32));
    step(&mut rt, 4);
    assert_eq!(
        rt.ui_vm_mut().text_state(input_box).map(|ts| ts.draft),
        Some("hi".into()),
        "typed chars land in draft"
    );
    assert!(
        committed.borrow().is_empty(),
        "typing alone commits nothing"
    );

    // 帧 5：Enter 按下沿 —— 草稿整体值经 on_commit 回调。
    inject_input(InputEvent::Key { key: Key::Enter, down: true });
    step(&mut rt, 5);
    assert_eq!(
        committed.borrow().as_slice(),
        [(input_box, "hi".to_string())],
        "Enter -> on_commit fires once with full draft"
    );

    // 帧 6：Enter 抬起（清理按住态；不再触发第二次提交）。
    inject_input(InputEvent::Key { key: Key::Enter, down: false });
    step(&mut rt, 6);
    assert_eq!(
        committed.borrow().as_slice(),
        [(input_box, "hi".to_string())],
        "Enter release -> no duplicate commit"
    );

    // UiVm 零写权：属性表的 text 仍是已提交旧值 —— 落账由宿主决定。
    assert_eq!(
        rt.tree_mut().prop(input_box, "text").cloned(),
        Some(Value::Str(String::new())),
        "UiVm never writes the property table"
    );

    // 清空事件队列，避免跨测试残留。
    let _ = rt.collect_input();
}

/// T-UI-R4：鼠标点击重命名全链（S12-2 真实运行时路径 + 事务落账）——
/// MouseMove 到输入框矩形 + 左键按下/抬键 -> 焦点建立 -> 字符注入落草稿
/// -> Enter 提交经 `on_commit` 进 rename_sink -> 宿主帧后落一条
/// `Inspector::modify_name` Modified 事务（uid 绑定，editor_shell 同款
/// 接线）-> 节点名变更 + undo 还原。
#[test]
fn t_ui_r4_click_focus_rename_via_modify_name_transaction() {
    use nes_scene::editor::Inspector;
    use nes_scene::transaction::TransactionLog;
    use nes_scene::Uid;

    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_ui")
        .join("r4");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _guard = lock_input();
    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[skip GPU case] wgpu-native library not found");
        return;
    };

    // 被重命名目标 + 绑定它的重命名输入框（矩形 (10,10)+(30,10)）。
    let (target, input_box, target_uid) = {
        let tree = rt.tree_mut();
        let target = tree.add_node(tree.root(), "sprite1", NodeKind::Button);
        let input_box = tree.add_node(tree.root(), "rename", NodeKind::TextInput);
        tree.set_prop(input_box, "offset", Value::Vec2(SVec2::new(10.0, 10.0)))
            .unwrap();
        tree.set_prop(input_box, "size", Value::Vec2(SVec2::new(30.0, 10.0)))
            .unwrap();
        tree.set_prop(input_box, "text", Value::Str("sprite1".into())).unwrap();
        tree.apply_pending();
        let uid = tree.uid_of(target).expect("target uid");
        (target, input_box, uid)
    };

    // rename_sink（editor_shell 同款）：on_commit 只传值，宿主帧后落账。
    let rename_sink: Rc<RefCell<Vec<(Uid, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = rename_sink.clone();
    let hook_uid = target_uid.clone();
    rt.ui_vm_mut().on_commit(move |_node, value| {
        if let Value::Str(name) = value {
            sink.borrow_mut().push((hook_uid.clone(), name));
        }
    });

    // 帧 0：MouseMove 到输入框 + 左键按下 -> 命中 TextInput 夺焦。
    inject_input(InputEvent::MouseMove { x: 15.0, y: 12.0 });
    inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true });
    step(&mut rt, 0);
    assert_eq!(rt.ui_vm_mut().focus(), Some(input_box), "click -> focus");
    assert_eq!(
        rt.ui_vm_mut().text_state(input_box).map(|ts| ts.draft),
        Some("sprite1".into()),
        "session draft starts from committed value"
    );

    // 帧 1：左键抬起（仍命中不失焦）。
    inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false });
    step(&mut rt, 1);
    assert_eq!(rt.ui_vm_mut().focus(), Some(input_box), "release inside -> focus stays");

    // 帧 2：注入字符 "2x"（真实 UTF-16 消息序）-> 草稿 "sprite12x"。
    inject_input(InputEvent::Char('2' as u32));
    inject_input(InputEvent::Char('x' as u32));
    step(&mut rt, 2);
    assert_eq!(
        rt.ui_vm_mut().text_state(input_box).map(|ts| ts.draft),
        Some("sprite12x".into()),
        "typed chars land in draft"
    );

    // 帧 3：Enter 按下沿 -> 提交经 on_commit 进 rename_sink。
    inject_input(InputEvent::Key { key: Key::Enter, down: true });
    step(&mut rt, 3);
    assert_eq!(
        rename_sink.borrow().as_slice(),
        [(target_uid.clone(), "sprite12x".to_string())],
        "Enter -> rename_sink gets (uid, new name)"
    );

    // 帧 4：Enter 抬起（无重复提交）。
    inject_input(InputEvent::Key { key: Key::Enter, down: false });
    step(&mut rt, 4);
    assert_eq!(rename_sink.borrow().len(), 1, "no duplicate commit on release");

    // 帧后落账：一条 Inspector::modify_name Modified 事务（UiVm 零写权）。
    {
        let mut log = TransactionLog::new();
        let new_name = rename_sink.borrow_mut().drain(..).next().unwrap().1;
        let tree = rt.tree_mut();
        log.begin().unwrap();
        Inspector::new(tree, &mut log)
            .modify_name(&target_uid, &new_name)
            .unwrap();
        log.commit().unwrap();
        tree.apply_pending(); // rename 是延迟 TreeOp，落定后名字可见

        assert_eq!(tree.name(target), Some("sprite12x"), "transaction renamed the node");
        assert_eq!(
            tree.prop(input_box, "text").cloned(),
            Some(Value::Str("sprite1".into())),
            "UiVm never writes the property table"
        );

        // undo 还原：事务可回滚旧名（S9-2 契约）。
        assert!(log.undo(tree).unwrap(), "undo restores previous name");
        tree.apply_pending();
        assert_eq!(tree.name(target), Some("sprite1"), "undo -> old name back");
    }

    // 清空事件队列，避免跨测试残留。
    let _ = rt.collect_input();
}
