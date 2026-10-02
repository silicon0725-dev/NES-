//! T-UI 契约回归：S12.1 组件库交互层 —— 主题色板 / UiVm 状态机 /
//! 四态词汇（S12.0 设计冻结 §2.2/§3.2）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-UI-01 | Button/Theme 节点种类：schema 封闭属性（text/槽位；八色板）、字符串往返 |
//! | T-UI-02 | ThemeColors：节点属性解析（I64 打包）、槽位名查找、缺省深色八槽齐全 |
//! | T-UI-03 | UiVm 状态机：视口锚定命中（前序最后者胜）、悬停/按下、抬键命中激活、移出释放不激活 |
//! | T-UI-04 | 瞬态不入指纹：update 前后 scene_fingerprint 逐位相同（§3.3 裁决） |

use std::cell::RefCell;
use std::rc::Rc;

use nes_scene::{
    NodeId, NodeKind, NodeSchema, NodeKindTag, SceneTree, UiVm, Value, Vec2,
    InputView, ThemeColors, THEME_SLOTS, scene_fingerprint,
};

/// 可编程假输入（鼠标位置 + 左键；newtype 绕开孤儿规则）。
#[derive(Clone, Default)]
struct FakeInput(Rc<RefCell<(f32, f32, bool)>>);

impl FakeInput {
    fn set(&self, mouse: (f32, f32), left: bool) {
        *self.0.borrow_mut() = (mouse.0, mouse.1, left);
    }
    fn mouse(&self) -> (f32, f32) {
        let s = self.0.borrow();
        (s.0, s.1)
    }
    fn left(&self) -> bool {
        self.0.borrow().2
    }
}

impl InputView for FakeInput {
    fn key(&self, _name: &str) -> bool {
        false
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
        0
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
    vm.update(&t, (512.0, 288.0));
    assert!(vm.state(a).hover && !vm.state(a).pressed, "悬停 a");
    assert!(!vm.state(b).hover, "b 不悬停");

    // 按下 a：pressed 置位。
    input.set((40.0, 130.0), true);
    vm.update(&t, (512.0, 288.0));
    assert!(vm.state(a).pressed, "按下 a");

    // 移出后抬键：不激活（标准 UI 语义 —— 按下目标 ≠ 抬键命中）。
    input.set((300.0, 130.0), true); // b 区域
    vm.update(&t, (512.0, 288.0));
    input.set((300.0, 130.0), false);
    vm.update(&t, (512.0, 288.0));
    assert!(!vm.state(a).pressed && !vm.state(b).pressed, "抬键清按下");

    // 完整点击 b：按下 → 抬键（同目标） → 激活。
    let fired = Rc::new(RefCell::new(Vec::new()));
    let sink = fired.clone();
    vm.on_activate(move |n| sink.borrow_mut().push(n));
    input.set((240.0, 130.0), true);
    vm.update(&t, (512.0, 288.0));
    assert!(vm.state(b).pressed, "按下 b");
    input.set((240.0, 130.0), false);
    vm.update(&t, (512.0, 288.0));
    assert_eq!(*fired.borrow(), vec![b], "抬键命中 b 激活一次");
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
    vm.update(&t, (512.0, 288.0));
    assert!(vm.state(a).hover, "前置：状态确实变了");
    let after = scene_fingerprint(&t, None);
    assert_eq!(before, after, "UI 瞬态不进语义指纹");
}
