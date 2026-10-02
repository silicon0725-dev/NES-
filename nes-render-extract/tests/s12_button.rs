//! T-WID 契约回归：S12.1 组件库提取层 —— Button 摊平（同句柄
//! rect+text）/ 主题解析（last-wins + 缺省兜底）/ 四态着色。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-WID-01 | Button 单节点单渲染物：SetRect + SetText 同句柄；panel 填充 / border 边框 / text 文字三槽齐达 |
//! | T-WID-02 | 主题 last-wins：前序序最后 Theme 生效（Q4 裁决）；槽位属性覆写（border_slot=danger） |
//! | T-WID-03 | 四态着色：hover -> accent 边框；pressed -> accent 填充+边框（UiVm 状态共享面） |
//! | T-WID-04 | 无 Theme 节点 -> DEFAULT_DARK 兜底；Control/Label 也走槽位（面板/文字着色） |
//! | T-WID-05 | TextInput 摊平（S12-2）：同句柄 rect+text 三槽齐达；focused -> 边框 accent；草稿文本到达；光标 Some + 30 帧节拍闪 |
//! | T-WID-06 | 点击夺焦进提取层回归：真实 UiVm（FakeInput 点击输入框矩形）驱动 states_rc 共享面 -> focused 边框 accent / 草稿 / caret，失焦回落 |

use std::cell::RefCell;
use std::rc::Rc;

use nes_render_api::{FrameInfo, NullRenderServer, Vec2};
use nes_render_extract::{RenderExtractor, RenderKeySource};
use nes_render_api::RenderAssetKey;
use nes_scene::ui::{TextState, UiStates, WidgetState, ThemeColors};
use nes_scene::{NodeKind, NodeId, SceneTree, Value, Vec2 as SVec2};

/// 空资源键源（按钮/控件不需要资源）。
struct NoAssets;
impl RenderKeySource for NoAssets {
    fn render_key(&self, _id: nes_scene::ResId) -> Option<RenderAssetKey> {
        None
    }
}

fn frame(vw: f32, vh: f32) -> FrameInfo {
    FrameInfo::new(0, 1.0 / 60.0, 0.0, Vec2::new(vw, vh))
}

/// 搭一按钮场景（视口 512x288，按钮 offset(32,120) size(140,28)）。
fn one_button() -> (SceneTree, NodeId) {
    let mut t = SceneTree::new("root");
    let b = t.add_node(t.root(), "ok", NodeKind::Button);
    t.set_prop(b, "offset", Value::Vec2(SVec2::new(32.0, 120.0))).unwrap();
    t.set_prop(b, "size", Value::Vec2(SVec2::new(140.0, 28.0))).unwrap();
    t.set_prop(b, "text", Value::Str("OK".into())).unwrap();
    t.apply_pending();
    (t, b)
}

fn extract(
    t: &mut SceneTree,
    ui: Option<Rc<RefCell<UiStates>>>,
) -> (NullRenderServer, RenderExtractor) {
    let mut ex = RenderExtractor::new();
    if let Some(states) = ui {
        ex.attach_ui(states);
    }
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();
    let _ = ex.extract_into(t, &NoAssets, &mut srv, &frame(512.0, 288.0), &mut out);
    (srv, ex)
}

/// T-WID-01：摊平 —— 同句柄 rect+text，三槽颜色齐达（缺省深色）。
#[test]
fn t_wid_01_button_flattens_rect_and_text() {
    let (mut t, b) = one_button();
    let (srv, ex) = extract(&mut t, None);
    let h = ex.handle_of(b).expect("按钮必须有渲染物");
    let rect = *srv.rect_of(h).expect("同句柄收到 SetRect");
    assert_eq!(rect.fill, ThemeColors::DEFAULT_DARK.slot("panel").unwrap(), "panel 填充");
    assert_eq!(rect.border, ThemeColors::DEFAULT_DARK.slot("border").unwrap(), "border 边框");
    assert_eq!(rect.border_w, 1.0, "1px 平直边框");
    let text = srv.label_of(h).expect("同句柄收到 SetText").clone();
    assert_eq!(&*text.text, "OK");
    assert_eq!(text.color, ThemeColors::DEFAULT_DARK.slot("text").unwrap(), "text 文字色");
}

/// T-WID-02：主题 last-wins + 槽位覆写。
#[test]
fn t_wid_02_theme_last_wins_and_slot_override() {
    let (mut t, b) = one_button();
    // 第一个主题：accent 红。
    let t1 = t.add_node(t.root(), "theme1", NodeKind::Theme);
    t.set_prop(t1, "accent", Value::I64(0xFF0000FF)).unwrap();
    // 第二个主题（前序更后）：accent 蓝 —— 生效者。
    let t2 = t.add_node(t.root(), "theme2", NodeKind::Theme);
    t.set_prop(t2, "accent", Value::I64(0x0000FFFF)).unwrap();
    // 按钮边框槽覆写为 danger。
    t.set_prop(b, "border_slot", Value::Str("danger".into())).unwrap();
    t.apply_pending();

    let (srv, ex) = extract(&mut t, None);
    let h = ex.handle_of(b).expect("渲染物");
    let rect = srv.rect_of(h).expect("rect");
    assert_eq!(
        rect.border,
        ThemeColors::DEFAULT_DARK.slot("danger").unwrap(),
        "border_slot 覆写生效（danger 槽）"
    );
    // 悬停时换 accent —— 用 hover 态验证 last-wins（第二个主题的蓝）。
    let mut ui = UiStates::new();
    ui.widgets.insert(b, WidgetState { hover: true, ..Default::default() });
    let (srv, ex) = extract(&mut t, Some(Rc::new(RefCell::new(ui))));
    let h = ex.handle_of(b).expect("渲染物");
    let rect = srv.rect_of(h).expect("rect");
    assert_eq!(rect.border, [0x00, 0x00, 0xFF, 0xFF], "hover 换 accent（theme2 的蓝 = last-wins）");
}

/// T-WID-03：四态着色 —— pressed = accent 填充 + accent 边框。
#[test]
fn t_wid_03_pressed_tint() {
    let (mut t, b) = one_button();
    let mut ui = UiStates::new();
    ui.widgets.insert(b, WidgetState { pressed: true, ..Default::default() });
    let (srv, ex) = extract(&mut t, Some(Rc::new(RefCell::new(ui))));
    let h = ex.handle_of(b).expect("渲染物");
    let rect = srv.rect_of(h).expect("rect");
    let accent = ThemeColors::DEFAULT_DARK.slot("accent").unwrap();
    assert_eq!(rect.fill, accent, "按下填充 accent");
    assert_eq!(rect.border, accent, "按下边框 accent");
}

/// T-WID-04：Control/Label 也走槽位（面板填充 + 文字色，无主题兜底）。
#[test]
fn t_wid_04_control_label_themed() {
    let mut t = SceneTree::new("root");
    let panel = t.add_node(t.root(), "panel", NodeKind::Control);
    t.set_prop(panel, "size", Value::Vec2(SVec2::new(480.0, 256.0))).unwrap();
    t.set_prop(panel, "fill_slot", Value::Str("panel".into())).unwrap();
    let lbl = t.add_node(t.root(), "lbl", NodeKind::Label);
    t.set_prop(lbl, "text", Value::Str("hi".into())).unwrap();
    t.set_prop(lbl, "color_slot", Value::Str("text_dim".into())).unwrap();
    t.apply_pending();

    let (srv, ex) = extract(&mut t, None);
    let ph = ex.handle_of(panel).expect("面板渲染物");
    let rect = srv.rect_of(ph).expect("rect");
    assert_eq!(rect.fill, ThemeColors::DEFAULT_DARK.slot("panel").unwrap(), "面板填充槽");
    let lh = ex.handle_of(lbl).expect("文本渲染物");
    let text = srv.label_of(lh).expect("text");
    assert_eq!(text.color, ThemeColors::DEFAULT_DARK.slot("text_dim").unwrap(), "文字槽位色");
    // 无主题节点：DEFAULT_DARK 兜底已在 T-WID-01 钉过，这里钉 Control
    // 默认填充 = 透明（fill_slot 空名）。
    let bare = t.add_node(t.root(), "bare", NodeKind::Control);
    t.set_prop(bare, "size", Value::Vec2(SVec2::new(10.0, 10.0))).unwrap();
    t.apply_pending();
    let (srv, ex) = extract(&mut t, None);
    let bh = ex.handle_of(bare).expect("裸控件渲染物");
    assert_eq!(srv.rect_of(bh).expect("rect").fill, [0, 0, 0, 0], "空槽名 = 透明填充（E-1 前同观感）");
}

/// T-WID-05：TextInput 摊平（S12-2）—— 同句柄 rect+text、三主题槽、
/// focused 边框 accent、草稿文本到达、光标 Some 与 30 帧节拍闪。
#[test]
fn t_wid_05_text_input_flattens_with_draft_and_caret() {
    let mut t = SceneTree::new("root");
    let input = t.add_node(t.root(), "name", NodeKind::TextInput);
    t.set_prop(input, "offset", Value::Vec2(SVec2::new(32.0, 40.0))).unwrap();
    t.set_prop(input, "size", Value::Vec2(SVec2::new(200.0, 24.0))).unwrap();
    t.set_prop(input, "text", Value::Str("committed".into())).unwrap();
    t.apply_pending();

    // 未聚焦（无 UI 共享面）：显示已提交值，光标不画。
    let (srv, ex) = extract(&mut t, None);
    let h = ex.handle_of(input).expect("输入框恒准入（空框也有框可点）");
    let text = srv.label_of(h).expect("同句柄收到 SetText").clone();
    assert_eq!(&*text.text, "committed", "无编辑会话显示已提交值");
    assert_eq!(text.caret, None, "未聚焦不画光标");
    let dark = ThemeColors::DEFAULT_DARK;
    let rect = srv.rect_of(h).expect("同句柄收到 SetRect");
    assert_eq!(rect.fill, dark.slot("panel").unwrap(), "panel 填充槽");
    assert_eq!(rect.border, dark.slot("border").unwrap(), "border 边框槽");

    // 聚焦 + 编辑会话：草稿 "AB"、光标 2（ASCII 字面量口径）。
    let mut ui = UiStates::new();
    ui.widgets
        .insert(input, WidgetState { focused: true, ..Default::default() });
    ui.texts
        .insert(input, TextState { draft: "AB".into(), caret: 2 });
    let ui = Rc::new(RefCell::new(ui));
    let mut ex = RenderExtractor::new();
    ex.attach_ui(ui);
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    // 第 1 帧（可见半拍）：focused 边框 accent + 草稿 + 光标 Some(2)。
    let _ = ex.extract_into(&mut t, &NoAssets, &mut srv, &frame(512.0, 288.0), &mut out);
    let h = ex.handle_of(input).expect("渲染物");
    let rect = srv.rect_of(h).expect("rect");
    assert_eq!(rect.fill, dark.slot("panel").unwrap(), "focused 仍取 panel 填充");
    assert_eq!(rect.border, dark.slot("accent").unwrap(), "focused 边框换 accent");
    let text = srv.label_of(h).expect("SetText").clone();
    assert_eq!(&*text.text, "AB", "草稿优先于已提交值");
    assert_eq!(text.color, dark.slot("text").unwrap(), "text 文字色");
    assert_eq!(text.caret, Some(2), "光标 Some(字符下标 2)");

    // 推进 30 帧到第 31 帧（隐半拍）：光标 None；文本/矩形照常推送。
    for _ in 0..30 {
        let _ = ex.extract_into(&mut t, &NoAssets, &mut srv, &frame(512.0, 288.0), &mut out);
    }
    let text = srv.label_of(h).expect("SetText").clone();
    assert_eq!(text.caret, None, "第 31 帧 = 隐半拍，光标不画");
    assert_eq!(&*text.text, "AB", "隐半拍文本不变");

    // 推进到第 61 帧（下一个可见半拍，30 帧周期）：光标复现。
    for _ in 0..30 {
        let _ = ex.extract_into(&mut t, &NoAssets, &mut srv, &frame(512.0, 288.0), &mut out);
    }
    assert_eq!(
        srv.label_of(h).expect("SetText").caret,
        Some(2),
        "回到可见半拍光标复现"
    );
}

/// 可编程假输入（T-UI 单元测试同款缩版：鼠标位置 + 左键 + 键盘 + 文本）。
#[derive(Clone, Default)]
struct ClickInput(Rc<RefCell<ClickState>>);

#[derive(Default)]
struct ClickState {
    mouse: (f32, f32),
    left: bool,
    keys: std::collections::BTreeMap<String, bool>,
    text: Vec<u32>,
}

impl nes_scene::InputView for ClickInput {
    fn key(&self, name: &str) -> bool {
        self.0.borrow().keys.get(name).copied().unwrap_or(false)
    }
    fn mouse(&self) -> (f32, f32) {
        self.0.borrow().mouse
    }
    fn mouse_delta(&self) -> (f32, f32) {
        (0.0, 0.0)
    }
    fn button(&self, name: &str) -> bool {
        name == "left" && self.0.borrow().left
    }
    fn text_len(&self) -> usize {
        self.0.borrow().text.len()
    }
    fn text(&self) -> Vec<u32> {
        self.0.borrow().text.clone()
    }
}

/// T-WID-06：点击夺焦进提取层（真实 UiVm 驱动，非手工插状态）——
/// FakeInput 点击输入框矩形 -> UiVm 夺焦 + 打字，`states_rc()` 共享面
/// attach 给提取器：focused 边框 accent、草稿文本、caret Some；失焦后
/// 边框回落 border 槽、草稿让位于已提交值、光标不画。
#[test]
fn t_wid_06_click_focus_drives_extractor_accent_draft_caret() {
    use nes_scene::UiVm;

    let mut t = SceneTree::new("root");
    let input = t.add_node(t.root(), "name", NodeKind::TextInput);
    t.set_prop(input, "offset", Value::Vec2(SVec2::new(200.0, 120.0))).unwrap();
    t.set_prop(input, "size", Value::Vec2(SVec2::new(140.0, 28.0))).unwrap();
    t.set_prop(input, "text", Value::Str("committed".into())).unwrap();
    t.apply_pending();

    let dark = ThemeColors::DEFAULT_DARK;
    let mouse = (240.0_f32, 130.0_f32); // 输入框矩形内一点
    let vp = (512.0_f32, 288.0_f32);

    let fake = ClickInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(fake.clone()));

    // 前置：未点击 —— 提取层无 focused，显示已提交值、无光标、边框 border。
    vm.update(&t, vp, (1.0, 1.0));
    let mut ex = RenderExtractor::new();
    ex.attach_ui(vm.states_rc());
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();
    let _ = ex.extract_into(&mut t, &NoAssets, &mut srv, &frame(vp.0, vp.1), &mut out);
    let h = ex.handle_of(input).expect("input handle");
    assert_eq!(srv.rect_of(h).expect("rect").border, dark.slot("border").unwrap());
    assert_eq!(&*srv.label_of(h).expect("SetText").text, "committed");
    assert_eq!(srv.label_of(h).expect("SetText").caret, None, "no focus, no caret");

    // 点击夺焦（按下 -> 抬键）。
    fake.0.borrow_mut().mouse = mouse;
    fake.0.borrow_mut().left = true;
    vm.update(&t, vp, (1.0, 1.0));
    fake.0.borrow_mut().left = false;
    vm.update(&t, vp, (1.0, 1.0));
    assert_eq!(vm.focus(), Some(input), "click on input rect -> focus");

    // 打字 "AB" -> 草稿进共享面。
    fake.0.borrow_mut().text = vec!['A' as u32, 'B' as u32];
    vm.update(&t, vp, (1.0, 1.0));
    fake.0.borrow_mut().text.clear();
    assert_eq!(
        vm.text_state(input),
        Some(TextState { draft: "committedAB".into(), caret: 11 }),
        "session draft = committed value + typed chars",
    );

    // focused 换档进提取层：边框 accent + 草稿优先 + 光标 Some(2)。
    let _ = ex.extract_into(&mut t, &NoAssets, &mut srv, &frame(vp.0, vp.1), &mut out);
    let rect = srv.rect_of(h).expect("rect");
    assert_eq!(rect.fill, dark.slot("panel").unwrap(), "focused keeps panel fill");
    assert_eq!(rect.border, dark.slot("accent").unwrap(), "focused border -> accent");
    let text = srv.label_of(h).expect("SetText").clone();
    assert_eq!(&*text.text, "committedAB", "draft (committed + typed) wins");
    assert_eq!(text.caret, Some(11), "caret lands in label payload");

    // 点空白失焦：边框回落 border、草稿让位于已提交值、光标不画。
    fake.0.borrow_mut().mouse = (400.0, 40.0);
    fake.0.borrow_mut().left = true;
    vm.update(&t, vp, (1.0, 1.0));
    fake.0.borrow_mut().left = false;
    vm.update(&t, vp, (1.0, 1.0));
    assert_eq!(vm.focus(), None, "click on blank -> blur");
    let _ = ex.extract_into(&mut t, &NoAssets, &mut srv, &frame(vp.0, vp.1), &mut out);
    assert_eq!(srv.rect_of(h).expect("rect").border, dark.slot("border").unwrap());
    let text = srv.label_of(h).expect("SetText").clone();
    assert_eq!(&*text.text, "committed", "blur -> committed value shows");
    assert_eq!(text.caret, None, "blur -> no caret");
}
