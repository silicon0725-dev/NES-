//! T-UI 契约回归：S12.1 组件库交互层 —— 主题色板 / UiVm 状态机 /
//! 四态词汇（S12.0 设计冻结 §2.2/§3.2）/ 焦点路由与文本输入（S12-2）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-UI-01 | Button/Theme 节点种类：schema 封闭属性（text/槽位；八色板）、字符串往返 |
//! | T-UI-02 | ThemeColors：节点属性解析（I64 打包）、槽位名查找、缺省深色八槽齐全 |
//! | T-UI-03 | UiVm 状态机：视口锚定命中（前序最后者胜）、悬停/按下、抬键命中激活、移出释放不激活 |
//! | T-UI-04 | 瞬态不入指纹：update 前后 scene_fingerprint 逐位相同（§3.3 裁决） |
//! | T-UI-05 | 焦点路由（S12-2）：点击 TextInput 夺焦、Tab 按场景序轮转、点空白失焦；Button 失焦零提交（无编辑会话不伪造载荷） |
//! | T-UI-06 | 文本输入状态机：字符插入光标处（P0 仅 ASCII 可打印）、Backspace 删前一字符 |
//! | T-UI-07 | 草稿语义：Enter 提交整体值（经回调、不直写属性）、Esc 回滚、失焦提交、空/非法处理 |
//! | T-UI-08 | 文本瞬态（草稿/光标）不入语义指纹 |
//! | T-UI-09 | 鼠标点击 TextInput 获焦回归：FakeInput 点击输入框矩形 -> focus()+focused 态 -> focused/草稿/光标经 states_rc 共享面换档进提取层（下游 T-WID-06 消费） |
//! | T-UI-10 | 获焦初始化（S12-4）：点击与 Tab 两路径草稿一律 = text 属性值、光标 = 末尾（focus_node 单点实现；宿主换绑后的新值也跟） |
//! | T-UI-11 | reset_text（S12-4）：置草稿=值、光标=尾、不动焦点、不触发 on_commit；持焦会话中原地替换，后续编辑落在新草稿上 |

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use nes_scene::{
    NodeId, NodeKind, NodeSchema, NodeKindTag, SceneTree, UiVm, Value, Vec2,
    InputView, ThemeColors, THEME_SLOTS, scene_fingerprint,
};
use nes_scene::ui::TextState;

/// 可编程假输入（鼠标位置 + 左键 + 键盘 + 提交文本；newtype 绕孤儿规则）。
#[derive(Clone, Default)]
struct FakeInput(Rc<RefCell<FakeState>>);

#[derive(Default)]
struct FakeState {
    mouse: (f32, f32),
    left: bool,
    keys: BTreeMap<String, bool>,
    text: Vec<u32>,
}

impl FakeInput {
    fn set(&self, mouse: (f32, f32), left: bool) {
        let mut s = self.0.borrow_mut();
        s.mouse = mouse;
        s.left = left;
    }
    fn mouse(&self) -> (f32, f32) {
        self.0.borrow().mouse
    }
    fn left(&self) -> bool {
        self.0.borrow().left
    }
    /// 置键（held 口径）。
    fn set_key(&self, name: &str, down: bool) {
        self.0.borrow_mut().keys.insert(name.to_string(), down);
    }
    /// 本帧提交文本（Unicode 标量值；帧末由测试清空模拟单帧快照）。
    fn set_text(&self, cps: Vec<u32>) {
        self.0.borrow_mut().text = cps;
    }
    fn clear_text(&self) {
        self.0.borrow_mut().text.clear();
    }
}

impl InputView for FakeInput {
    fn key(&self, name: &str) -> bool {
        self.0.borrow().keys.get(name).copied().unwrap_or(false)
    }
    fn mouse(&self) -> (f32, f32) {
        self.mouse()
    }
    fn mouse_delta(&self) -> (f32, f32) {
        (0.0, 0.0)
    }
    fn button(&self, name: &str) -> bool {
        name == "left" && self.left()
    }
    fn text_len(&self) -> usize {
        self.0.borrow().text.len()
    }
    fn text(&self) -> Vec<u32> {
        self.0.borrow().text.clone()
    }
}

/// 搭两按钮场景（视口 512x288）：a=(32,120,140,28)、b=(200,120,140,28)。
fn two_buttons() -> (SceneTree, NodeId, NodeId) {
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "a", NodeKind::Button);
    t.set_prop(a, "offset", Value::Vec2(Vec2::new(32.0, 120.0))).unwrap();
    t.set_prop(a, "size", Value::Vec2(Vec2::new(140.0, 28.0))).unwrap();
    let b = t.add_node(t.root(), "b", NodeKind::Button);
    t.set_prop(b, "offset", Value::Vec2(Vec2::new(200.0, 120.0))).unwrap();
    t.set_prop(b, "size", Value::Vec2(Vec2::new(140.0, 28.0))).unwrap();
    t.apply_pending();
    (t, a, b)
}

/// T-UI-01：新种类 schema —— 属性封闭、链上继承、字符串往返。
#[test]
fn t_ui_01_button_theme_schema() {
    let btn = NodeSchema::of(NodeKindTag::Button);
    let names: Vec<&str> = btn.props().iter().map(|p| p.name()).collect();
    for want in ["anchor", "offset", "size", "text", "fill_slot", "border_slot", "text_slot"] {
        assert!(names.contains(&want), "Button 链上属性缺 {want}: {names:?}");
    }
    // 槽位缺省：panel/border/text（S12.0 §2.1）。
    assert_eq!(btn.validate("fill_slot", &Value::Str("panel".into())), Ok(Value::Str("panel".into())));
    assert!(btn.validate("nope", &Value::I64(1)).is_err(), "封闭属性：未知键拒绝");

    let theme = NodeSchema::of(NodeKindTag::Theme);
    let theme_names: Vec<&str> = theme.own_props().iter().map(|p| p.name()).collect();
    assert_eq!(theme_names.len(), 8, "八槽位：{theme_names:?}");
    for (name, _) in THEME_SLOTS.iter() {
        assert!(theme_names.contains(name), "缺槽位 {name}");
    }

    assert_eq!(NodeKindTag::from_str_exact("Button"), Some(NodeKindTag::Button));
    assert_eq!(NodeKindTag::from_str_exact("Theme"), Some(NodeKindTag::Theme));
    assert_eq!(NodeKindTag::Button.as_str(), "Button");
}

/// T-UI-02：主题色板 —— 属性解析（I64 0xRRGGBBAA）+ 槽位查找 + 缺省齐全。
#[test]
fn t_ui_02_theme_colors() {
    let mut t = SceneTree::new("root");
    let th = t.add_node(t.root(), "theme", NodeKind::Theme);
    t.apply_pending();
    // 只覆写 accent（其余落缺省深色）。
    t.set_prop(th, "accent", Value::I64(0xFF0000FF)).unwrap();
    let c = ThemeColors::from_tree(&t, th);
    assert_eq!(c.slot("accent"), Some([0xFF, 0x00, 0x00, 0xFF]), "覆写生效");
    assert_eq!(
        c.slot("bg"),
        ThemeColors::DEFAULT_DARK.slot("bg"),
        "未覆写槽位落缺省"
    );
    assert_eq!(c.slot("nope"), None, "未知名无槽");
    for (name, rgba) in THEME_SLOTS.iter() {
        assert_eq!(ThemeColors::DEFAULT_DARK.slot(name), Some(*rgba));
    }
}

/// T-UI-03：UiVm 状态机 —— 命中/悬停/按下/激活（前序最后者胜）。
#[test]
fn t_ui_03_uivm_hover_press_activate() {
    let (t, a, b) = two_buttons();
    let input = FakeInput::default();
    input.set((40.0, 130.0), false);
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));

    // 悬停 a：无键。
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert!(vm.state(a).hover && !vm.state(a).pressed, "悬停 a");
    assert!(!vm.state(b).hover, "b 不悬停");

    // 按下 a：pressed 置位。
    input.set((40.0, 130.0), true);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert!(vm.state(a).pressed, "按下 a");

    // 移出后抬键：不激活（标准 UI 语义 —— 按下目标 ≠ 抬键命中）。
    input.set((300.0, 130.0), true); // b 区域
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set((300.0, 130.0), false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert!(!vm.state(a).pressed && !vm.state(b).pressed, "抬键清按下");

    // 完整点击 b：按下 → 抬键（同目标） → 激活。
    let fired = Rc::new(RefCell::new(Vec::new()));
    let sink = fired.clone();
    vm.on_activate(move |n| sink.borrow_mut().push(n));
    input.set((240.0, 130.0), true);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert!(vm.state(b).pressed, "按下 b");
    input.set((240.0, 130.0), false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert_eq!(*fired.borrow(), vec![b], "抬键命中 b 激活一次");
}

/// T-UI-03b：窗口缩放折算 —— 鼠标按 视图/客户区 比例映射回视图空间
/// 后命中（客户区 1024x576 = 视图 512x288 的 2 倍拉伸）。
#[test]
fn t_ui_03b_scaled_window_mouse_hit() {
    let (t, a, _b) = two_buttons();
    let input = FakeInput::default();
    // 客户区 (80, 240) -> 视图 (40, 120)（按钮 a 顶边内侧）。
    input.set((80.0, 240.0), false);
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input));
    vm.update(&t, (512.0, 288.0), (0.5, 0.5));
    assert!(vm.state(a).hover, "折算后命中 a（未折算则 (80,240) 在 a 外）");
}

/// T-UI-04：瞬态不入指纹（§3.3 裁决 —— UI 摇动不影响游戏确定性）。
#[test]
fn t_ui_04_transient_states_not_in_fingerprint() {
    let (t, a, _b) = two_buttons();
    let mut vm = UiVm::new();
    let input = FakeInput::default();
    input.set((40.0, 130.0), true);
    vm.set_input_view(Rc::new(input));
    let before = scene_fingerprint(&t, None);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert!(vm.state(a).hover, "前置：状态确实变了");
    let after = scene_fingerprint(&t, None);
    assert_eq!(before, after, "UI 瞬态不进语义指纹");
}

/// 搭"按钮 + 两个输入框"场景（视口 512x288，前序序 = btn, in1, in2）：
/// btn=(32,120,140,28)、in1=(200,120,140,28)、in2=(200,160,140,28)。
fn widgets_scene() -> (SceneTree, NodeId, NodeId, NodeId) {
    let mut t = SceneTree::new("root");
    let btn = t.add_node(t.root(), "btn", NodeKind::Button);
    t.set_prop(btn, "offset", Value::Vec2(Vec2::new(32.0, 120.0))).unwrap();
    t.set_prop(btn, "size", Value::Vec2(Vec2::new(140.0, 28.0))).unwrap();
    let in1 = t.add_node(t.root(), "in1", NodeKind::TextInput);
    t.set_prop(in1, "offset", Value::Vec2(Vec2::new(200.0, 120.0))).unwrap();
    t.set_prop(in1, "size", Value::Vec2(Vec2::new(140.0, 28.0))).unwrap();
    let in2 = t.add_node(t.root(), "in2", NodeKind::TextInput);
    t.set_prop(in2, "offset", Value::Vec2(Vec2::new(200.0, 160.0))).unwrap();
    t.set_prop(in2, "size", Value::Vec2(Vec2::new(140.0, 28.0))).unwrap();
    t.apply_pending();
    (t, btn, in1, in2)
}

/// T-UI-05：焦点路由 —— 点击夺焦、Tab 按场景序轮转、点空白失焦。
#[test]
fn t_ui_05_focus_routing() {
    let (t, _btn, in1, in2) = widgets_scene();
    let input = FakeInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));
    let frame = |vm: &mut UiVm, input: &FakeInput, pos: (f32, f32), down: bool| {
        input.set(pos, down);
        vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    };

    assert_eq!(vm.focus(), None, "初始无焦点");

    // 点击 in1：夺焦（focused 旗标进共享状态面）。
    frame(&mut vm, &input, (240.0, 130.0), true);
    frame(&mut vm, &input, (240.0, 130.0), false);
    assert_eq!(vm.focus(), Some(in1), "点击 TextInput 夺焦");
    assert!(vm.state(in1).focused, "focused 旗标置位");
    assert!(!vm.state(in2).focused, "未聚焦者旗标为假");

    // Tab 一次：in1 -> in2（场景序）。
    input.set_key("tab", true);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set_key("tab", false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert_eq!(vm.focus(), Some(in2), "Tab 顺场景序轮转");

    // Tab 两次：in2 -> btn（回绕到序首）-> in1。
    for _ in 0..2 {
        input.set_key("tab", true);
        vm.update(&t, (512.0, 288.0), (1.0, 1.0));
        input.set_key("tab", false);
        vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    }
    assert_eq!(vm.focus(), Some(in1), "Tab 回绕");

    // 点击空白：失焦。
    frame(&mut vm, &input, (400.0, 40.0), true);
    frame(&mut vm, &input, (400.0, 40.0), false);
    assert_eq!(vm.focus(), None, "点空白失焦");
    assert!(!vm.state(in1).focused, "失焦清旗标");

    // 点击 Button 不发激活给 TextInput：激活回调只认 Button。
    let fired = Rc::new(RefCell::new(Vec::new()));
    let sink = fired.clone();
    vm.on_activate(move |n| sink.borrow_mut().push(n));
    frame(&mut vm, &input, (240.0, 130.0), true); // 按 in1
    frame(&mut vm, &input, (240.0, 130.0), false);
    assert!(fired.borrow().is_empty(), "TextInput 点击不触发激活钩子");

    // Button 失焦不发 on_commit：Tab 把焦点轮到 btn（纯占焦控件）后
    // 点空白失焦 —— 提交钩子零载荷（提交语义只属于 TextInput；无
    // 编辑会话的控件不得回读 text 属性伪造提交）。
    let commits = Rc::new(RefCell::new(Vec::new()));
    let csink = commits.clone();
    vm.on_commit(move |n, v| csink.borrow_mut().push((n, v)));
    input.set_key("tab", true); // in1 -> in2
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set_key("tab", false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set_key("tab", true); // in2 -> btn
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set_key("tab", false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert_eq!(vm.focus(), Some(_btn), "前置：Tab 轮到 Button");
    assert_eq!(
        *commits.borrow(),
        vec![(in1, Value::Str(String::new())), (in2, Value::Str(String::new()))],
        "途中 TextInput 失焦照常提交（对照组）"
    );
    commits.borrow_mut().clear();
    frame(&mut vm, &input, (400.0, 40.0), true); // 点空白
    frame(&mut vm, &input, (400.0, 40.0), false);
    assert_eq!(vm.focus(), None, "Button 失焦同样清焦点槽");
    assert!(commits.borrow().is_empty(), "Button 失焦不发 on_commit");
}

/// T-UI-06：文本输入状态机 —— 字符插入光标处（P0 仅 ASCII 可打印，
/// 非 ASCII 忽略）、Backspace 删前一字符。
/// 左/右光标移动：P0 可选缺口，本版未实现（见 ui.rs 泵注释）。
#[test]
fn t_ui_06_typing_backspace_caret() {
    let (t, _btn, in1, _in2) = widgets_scene();
    let input = FakeInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));
    let frame = |vm: &mut UiVm| vm.update(&t, (512.0, 288.0), (1.0, 1.0));

    // 点击夺焦（草稿 = 已提交值 ""，光标 0）。
    input.set((240.0, 130.0), true);
    frame(&mut vm);
    input.set((240.0, 130.0), false);
    frame(&mut vm);
    assert_eq!(vm.text_state(in1), Some(TextState { draft: String::new(), caret: 0 }));

    // 输入 "abc"：逐字符落在光标处。
    input.set_text(vec!['a' as u32, 'b' as u32, 'c' as u32]);
    frame(&mut vm);
    input.clear_text();
    assert_eq!(vm.text_state(in1), Some(TextState { draft: "abc".into(), caret: 3 }));

    // 非 ASCII 忽略，ASCII 照收：得到 "abcx"。
    input.set_text(vec![0xE9, 'x' as u32]); // 0xE9 = é，非法（P0）
    frame(&mut vm);
    input.clear_text();
    assert_eq!(vm.text_state(in1), Some(TextState { draft: "abcx".into(), caret: 4 }));

    // Backspace：删光标前一字符（按下沿一次）。
    input.set_key("backspace", true);
    frame(&mut vm);
    input.set_key("backspace", false);
    frame(&mut vm);
    assert_eq!(vm.text_state(in1), Some(TextState { draft: "abc".into(), caret: 3 }));
}

/// T-UI-07：草稿语义 —— Enter 提交整体值（经 on_commit 回调、不直写
/// 属性表）、Esc 回滚到已提交值、失焦 = 提交、空/非法值处理。
#[test]
fn t_ui_07_draft_input_semantics() {
    let (mut t, _btn, in1, _in2) = widgets_scene();
    t.set_prop(in1, "text", Value::Str("hi".into())).unwrap();
    let input = FakeInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));
    let frame = |vm: &mut UiVm| vm.update(&t, (512.0, 288.0), (1.0, 1.0));

    let commits = Rc::new(RefCell::new(Vec::new()));
    let sink = commits.clone();
    vm.on_commit(move |n, v| sink.borrow_mut().push((n, v)));

    // 夺焦：草稿取已提交值 "hi"。
    input.set((240.0, 130.0), true);
    frame(&mut vm);
    input.set((240.0, 130.0), false);
    frame(&mut vm);
    assert_eq!(vm.text_state(in1).unwrap().draft, "hi");

    // 续打 "abc" -> Enter：整体值 "hiabc" 经回调提交；属性表不动。
    input.set_text(vec!['a' as u32, 'b' as u32, 'c' as u32]);
    frame(&mut vm);
    input.clear_text();
    input.set_key("enter", true);
    frame(&mut vm);
    input.set_key("enter", false);
    frame(&mut vm);
    assert_eq!(*commits.borrow(), vec![(in1, Value::Str("hiabc".into()))], "Enter 提交整体值");
    assert_eq!(t.prop(in1, "text"), Some(&Value::Str("hi".into())), "UiVm 零写权");
    commits.borrow_mut().clear();

    // Esc 回滚：草稿 := 节点已提交值（属性表 "hi"），会话继续。
    input.set_text(vec!['x' as u32; 3]);
    frame(&mut vm);
    input.clear_text();
    assert_eq!(vm.text_state(in1).unwrap().draft, "hiabcxxx");
    input.set_key("escape", true);
    frame(&mut vm);
    input.set_key("escape", false);
    frame(&mut vm);
    assert_eq!(vm.text_state(in1).unwrap().draft, "hi", "Esc 回滚");

    // 失焦 = 提交（回滚后的值）。
    input.set((400.0, 40.0), true);
    frame(&mut vm);
    input.set((400.0, 40.0), false);
    frame(&mut vm);
    assert_eq!(*commits.borrow(), vec![(in1, Value::Str("hi".into()))], "失焦提交");
    commits.borrow_mut().clear();

    // 空值处理：聚焦空输入框直接回车 -> 提交空串（如实暴露，不静默拦）。
    input.set((240.0, 170.0), true); // in2
    frame(&mut vm);
    input.set((240.0, 170.0), false);
    frame(&mut vm);
    input.set_key("enter", true);
    frame(&mut vm);
    input.set_key("enter", false);
    frame(&mut vm);
    assert_eq!(*commits.borrow(), vec![(_in2, Value::Str(String::new()))], "空值如实提交");
}

/// T-UI-08：文本瞬态（草稿/光标）不入语义指纹 —— 聚焦 + 打字前后
/// scene_fingerprint 逐位相同。
#[test]
fn t_ui_08_text_transients_not_in_fingerprint() {
    let (t, _btn, in1, _in2) = widgets_scene();
    let input = FakeInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));
    let before = scene_fingerprint(&t, None);
    // 聚焦 + 输入，瞬态全部动起来。
    input.set((240.0, 130.0), true);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set((240.0, 130.0), false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set_text(vec!['q' as u32; 3]);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert_eq!(vm.text_state(in1).unwrap().draft, "qqq", "前置：草稿确实变了");
    let after = scene_fingerprint(&t, None);
    assert_eq!(before, after, "文本瞬态不进语义指纹");
}

/// T-UI-09：鼠标点击 TextInput 获焦（UiVm 单元回归）—— FakeInput 点击
/// 输入框矩形 -> focus()==该节点 + focused 态置位 + focused/草稿/光标
/// 经 [`UiVm::states_rc`] 共享面换档进提取层（提取器 `attach_ui` 消费的
/// 正是这个句柄：边框 accent 取 widgets[in].focused，草稿/caret 取
/// texts[in]；G7 禁场景层依赖提取层，故本侧钉住共享面这一契约交接点，
/// 提取器摊平断言见下游提取层 T-WID-05/06）。
#[test]
fn t_ui_09_click_focus_feeds_extraction_shared_states() {
    let (t, _btn, in1, _in2) = widgets_scene();
    let input = FakeInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));

    // 前置：未点击 —— 无焦点，共享面无 focused。
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert_eq!(vm.focus(), None, "初始无焦点");
    let shared = vm.states_rc();
    assert!(
        !shared.borrow().widgets.get(&in1).is_some_and(|s| s.focused),
        "未聚焦：共享面无 focused 旗标"
    );

    // 点击 in1 矩形 (200,120,140,28) 内一点 -> 夺焦。
    input.set((240.0, 130.0), true);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set((240.0, 130.0), false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert_eq!(vm.focus(), Some(in1), "点击 TextInput 夺焦");
    assert!(vm.state(in1).focused, "focused 态置位");

    // focused 换档进提取层共享面（states_rc == attach_ui 句柄）。
    assert!(
        shared.borrow().widgets.get(&in1).is_some_and(|s| s.focused),
        "focused 旗标进共享面（提取器边框 accent 数据源）"
    );

    // 打字：草稿/光标同样经共享面进提取层（草稿文本 + caret 数据源）。
    input.set_text(vec!['A' as u32, 'B' as u32]);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.clear_text();
    assert_eq!(
        shared.borrow().texts.get(&in1).cloned(),
        Some(TextState { draft: "AB".into(), caret: 2 }),
        "草稿与光标进共享面"
    );

    // 点空白失焦：共享面 focused 清位（提取器边框回落 border 槽）。
    input.set((400.0, 40.0), true);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set((400.0, 40.0), false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert_eq!(vm.focus(), None, "点空白失焦");
    assert!(
        !shared.borrow().widgets.get(&in1).is_some_and(|s| s.focused),
        "失焦清共享面 focused 旗标"
    );
}

/// T-UI-10：获焦初始化（S12-4 ②——草稿生命周期的一个端点）——
/// **无论点击还是 Tab**，获焦沿一律 `draft = text 属性值、caret = 末尾`
///（focus_node 单点实现）。宿主换绑（text 属性被投影成新选中节点名）
/// 后的获焦也跟新值 —— 草稿永远从当前文档真相出发。
#[test]
fn t_ui_10_focus_initializes_draft_from_text_prop() {
    let (mut t, _btn, in1, _in2) = widgets_scene();
    t.set_prop(in1, "text", Value::Str("hello".into())).unwrap();
    let input = FakeInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));

    // 路径 A：点击获焦 —— 草稿 = text 属性值，光标 = 末尾。
    input.set((240.0, 130.0), true);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set((240.0, 130.0), false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert_eq!(vm.focus(), Some(in1), "前置：点击夺焦");
    assert_eq!(
        vm.text_state(in1),
        Some(TextState { draft: "hello".into(), caret: 5 }),
        "点击获焦：草稿从 text 属性初始化"
    );

    // 点空白失焦（失焦提交零钩子 —— 未注册 on_commit）。
    input.set((400.0, 40.0), true);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set((400.0, 40.0), false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    assert_eq!(vm.focus(), None, "前置：点空白失焦");

    // 宿主换绑投影：text 属性换成新值（编辑器换选中 / undo 落账口径）。
    t.set_prop(in1, "text", Value::Str("world".into())).unwrap();

    // 路径 B：Tab 获焦（可焦点序 btn -> in1，按两次）—— 草稿跟**换绑
    // 后的新值**，不是旧会话残值。
    for _ in 0..2 {
        input.set_key("tab", true);
        vm.update(&t, (512.0, 288.0), (1.0, 1.0));
        input.set_key("tab", false);
        vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    }
    assert_eq!(vm.focus(), Some(in1), "Tab 轮转到 in1");
    assert_eq!(
        vm.text_state(in1),
        Some(TextState { draft: "world".into(), caret: 5 }),
        "Tab 获焦：草稿从当前 text 属性初始化"
    );
}

/// T-UI-11：reset_text（S12-4 宿主换绑专用）—— 置草稿 = 值、光标 = 末
/// 尾、**不动焦点槽、不触发 on_commit**。未聚焦节点也可 reset（宿主在
/// 文档侧变化后统一调用）；持焦会话中原地替换，后续编辑落在新草稿上。
#[test]
fn t_ui_11_reset_text_rebinds_draft_without_commit() {
    let (t, _btn, in1, _in2) = widgets_scene();
    let input = FakeInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));
    let commits: Rc<RefCell<Vec<(NodeId, Value)>>> = Rc::new(RefCell::new(Vec::new()));
    let csink = commits.clone();
    vm.on_commit(move |n, v| csink.borrow_mut().push((n, v)));

    // 未聚焦 reset：置草稿/光标；焦点不动（None）、零提交。
    vm.reset_text(in1, "renamed");
    assert_eq!(vm.focus(), None, "不动焦点（本就无焦点）");
    assert_eq!(
        vm.text_state(in1),
        Some(TextState { draft: "renamed".into(), caret: 7 }),
        "置草稿 = 值、光标 = 末尾"
    );
    assert!(commits.borrow().is_empty(), "reset_text 不触发 on_commit");

    // 点击获焦并打字：会话草稿 "x"。
    input.set((240.0, 130.0), true);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set((240.0, 130.0), false);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.set_text(vec!['x' as u32]);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.clear_text();
    assert_eq!(
        vm.text_state(in1),
        Some(TextState { draft: "x".into(), caret: 1 }),
        "前置：持焦编辑中草稿 = x"
    );

    // 持焦 reset（宿主换绑 / undo 落账口径）：草稿整体替换为文档真相，
    // 焦点保留、零提交 —— Tab 循环选中 / 撤销后输入框跟手的机制点。
    vm.reset_text(in1, "doc-truth");
    assert_eq!(vm.focus(), Some(in1), "不动焦点（持焦保持）");
    assert_eq!(
        vm.text_state(in1),
        Some(TextState { draft: "doc-truth".into(), caret: 9 }),
        "持焦中：草稿原地替换、光标到尾"
    );
    assert!(commits.borrow().is_empty(), "换绑不是提交：零 on_commit");

    // reset 后继续编辑：落在新草稿上（会话延续，不是重建）。
    input.set_text(vec!['!' as u32]);
    vm.update(&t, (512.0, 288.0), (1.0, 1.0));
    input.clear_text();
    assert_eq!(
        vm.text_state(in1),
        Some(TextState { draft: "doc-truth!".into(), caret: 10 }),
        "后续编辑落在新草稿上"
    );
    assert!(commits.borrow().is_empty(), "全程零提交");
}
