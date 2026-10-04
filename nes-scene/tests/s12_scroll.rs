//! T-SC 契约回归：S12-3 滚动族组件 —— ScrollView/ListView/Tabs 节点、
//! UiVm 滚动瞬态（scrolls 表）、滚轮路由、ListView 行点击回调。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-SC-01 | 滚轮只在命中滚动控件时改其 scroll 瞬态，夹紧 [0, scroll_max]；scroll_max 算式（ListView 行数×row_h+8−视口高 / ScrollView 后代底缘−自身底）；前序序最后命中者胜 |
//! | T-SC-02 | ListView 行点击：回调载荷 (节点, 行下标) 确定、按下移出抬键不回调（边沿语义）、顶内衬/越界行不回调、UiVm 不写属性表 |
//! | T-SC-03 | scrolls 生灭纪律：死节点清扫、无输入视图全清、瞬态不入语义指纹 |
//! | T-SC-04 | 三个新 kind：schema 封闭属性 + 缺省值 + ALL/index 一致性 + RON 往返 |

use std::cell::RefCell;
use std::rc::Rc;

use nes_scene::scene_io::{self, PackOptions};
use nes_scene::{
    instantiate, scene_fingerprint, InputView, NodeId, NodeKind, NodeKindTag, NodeSchema,
    SceneTree, UiStates, UiVm, Value, Vec2,
};

/// 视口（与 S12.1 既有回归同尺寸）。
const VP: (f32, f32) = (512.0, 288.0);
/// 1:1 鼠标折算。
const SCALE: (f32, f32) = (1.0, 1.0);

/// 可编程假输入（鼠标 + 左键 + 键盘 + 文本 + 滚轮；newtype 绕孤儿规则）。
#[derive(Clone, Default)]
struct FakeInput(Rc<RefCell<FakeState>>);

#[derive(Default)]
struct FakeState {
    mouse: (f32, f32),
    left: bool,
    wheel: (f32, f32),
}

impl FakeInput {
    fn set(&self, mouse: (f32, f32), left: bool) {
        let mut s = self.0.borrow_mut();
        s.mouse = mouse;
        s.left = left;
    }
    /// 置本帧滚轮增量（+y=向上；一次性 —— 帧末由测试清零模拟单帧快照）。
    fn set_wheel(&self, wheel: (f32, f32)) {
        self.0.borrow_mut().wheel = wheel;
    }
    fn left(&self) -> bool {
        self.0.borrow().left
    }
}

impl InputView for FakeInput {
    fn key(&self, _name: &str) -> bool {
        false
    }
    fn mouse(&self) -> (f32, f32) {
        self.0.borrow().mouse
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
    fn wheel(&self) -> (f32, f32) {
        self.0.borrow().wheel
    }
}

/// 推进一帧（含滚轮，帧末清零模拟 wheel 一次性快照口径）。
fn frame(vm: &mut UiVm, t: &SceneTree, input: &FakeInput, pos: (f32, f32), wheel: (f32, f32)) {
    input.set(pos, false);
    input.set_wheel(wheel);
    vm.update(t, VP, SCALE);
    input.set_wheel((0.0, 0.0));
}

/// 完整点击（按下沿 + 抬键沿，同点位）。
fn click(vm: &mut UiVm, t: &SceneTree, input: &FakeInput, pos: (f32, f32)) {
    input.set(pos, true);
    input.set_wheel((0.0, 0.0));
    vm.update(t, VP, SCALE);
    input.set(pos, false);
    vm.update(t, VP, SCALE);
}

/// 读某节点滚动瞬态（共享面直读 —— 与提取层同一数据源）。
fn scroll_of(vm: &UiVm, node: NodeId) -> Option<f32> {
    vm.states_rc().borrow().scrolls.get(&node).copied()
}

/// 滚动场景：sv=(0,0,100,100) 内含按钮 btn=(10,150,50,20)（底缘 170 →
/// scroll_max = 170−100 = 70），tabs=(50,50,40,40) 三页签（与 sv 重叠、
/// 前序序在后 —— 验证最后命中者胜）。
fn scroll_scene() -> (SceneTree, NodeId, NodeId, NodeId) {
    let mut t = SceneTree::new("root");
    let sv = t.add_node(t.root(), "sv", NodeKind::ScrollView);
    t.set_prop(sv, "size", Value::Vec2(Vec2::new(100.0, 100.0))).unwrap();
    let btn = t.add_node(sv, "btn", NodeKind::Button);
    t.set_prop(btn, "offset", Value::Vec2(Vec2::new(10.0, 150.0))).unwrap();
    t.set_prop(btn, "size", Value::Vec2(Vec2::new(50.0, 20.0))).unwrap();
    let tabs = t.add_node(t.root(), "tabs", NodeKind::Tabs);
    t.set_prop(tabs, "offset", Value::Vec2(Vec2::new(50.0, 50.0))).unwrap();
    t.set_prop(tabs, "size", Value::Vec2(Vec2::new(40.0, 40.0))).unwrap();
    t.set_prop(tabs, "tabs", Value::Str("t0\nt1\nt2".into())).unwrap();
    t.apply_pending();
    (t, sv, btn, tabs)
}

/// T-SC-01：滚轮路由 —— 只命中滚动控件才改、夹紧 [0,max]、算式与
/// 前序最后命中仲裁。
#[test]
fn t_sc_01_wheel_routing_and_clamp() {
    let (t, sv, _btn, tabs) = scroll_scene();
    let input = FakeInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));

    // scroll_max 算式：ScrollView = 可见后代底缘最大值 − 自身底（下限 0）。
    // btn 底缘 = 150+20 = 170，自身底 = 0+100 = 100 → 70。
    assert_eq!(
        nes_scene::scroll_max_of(&t, &UiStates::new(), sv, VP),
        70.0,
        "ScrollView extent = 后代底缘 - 自身底"
    );
    // Tabs = 页签数 × 缺省行高 18 + 8 内衬 − 视口高 40 = 22。
    assert_eq!(
        nes_scene::scroll_max_of(&t, &UiStates::new(), tabs, VP),
        22.0,
        "Tabs extent = 页签数*row_h + 8 - 视口高"
    );

    // 命中 sv（tabs 不在指针下）：向下滚一格（wheel.y=-1），步进 = 缺省
    // step 48 → scroll = clamp(0+48, 0, 70) = 48。
    frame(&mut vm, &t, &input, (30.0, 30.0), (0.0, -1.0));
    assert_eq!(scroll_of(&vm, sv), Some(48.0), "向下滚一格 = +step");
    assert_eq!(scroll_of(&vm, tabs), None, "未命中者零改动");

    // 再滚一格：96 超上限 → 夹到 70。
    frame(&mut vm, &t, &input, (30.0, 30.0), (0.0, -1.0));
    assert_eq!(scroll_of(&vm, sv), Some(70.0), "夹紧上限 scroll_max");
    // 上限处继续下滚：仍 70。
    frame(&mut vm, &t, &input, (30.0, 30.0), (0.0, -1.0));
    assert_eq!(scroll_of(&vm, sv), Some(70.0), "上限饱和");

    // 向上滚：70 − 48 = 22。
    frame(&mut vm, &t, &input, (30.0, 30.0), (0.0, 1.0));
    assert_eq!(scroll_of(&vm, sv), Some(22.0), "向上滚 = -step");
    // 大步长向上：夹到 0。
    frame(&mut vm, &t, &input, (30.0, 30.0), (0.0, 5.0));
    assert_eq!(scroll_of(&vm, sv), Some(0.0), "夹紧下限 0");

    // 指针不在任何滚动控件上：滚轮丢弃，瞬态零改动。
    frame(&mut vm, &t, &input, (400.0, 40.0), (0.0, -2.0));
    assert_eq!(scroll_of(&vm, sv), Some(0.0), "未命中不路由");

    // 前序序最后命中者胜：指针落在 sv 与 tabs 的重叠区 —— tabs 在前序
    // 序更后，路由给 tabs（步进 = 缺省行高 18 → 0+18=18），sv 不动。
    frame(&mut vm, &t, &input, (60.0, 60.0), (0.0, -1.0));
    assert_eq!(scroll_of(&vm, tabs), Some(18.0), "重叠区最后命中者胜");
    assert_eq!(scroll_of(&vm, sv), Some(0.0), "败者零改动");
}

/// 列表场景：lv=(100,100,80,60)，四行 row_h=18（行 i 屏上 y =
/// 104 + 18i，scroll=0 时第 3 行大半滚出视口），scroll_max = 4*18+8−60 = 20。
fn list_scene() -> (SceneTree, NodeId) {
    let mut t = SceneTree::new("root");
    let lv = t.add_node(t.root(), "lv", NodeKind::ListView);
    t.set_prop(lv, "offset", Value::Vec2(Vec2::new(100.0, 100.0))).unwrap();
    t.set_prop(lv, "size", Value::Vec2(Vec2::new(80.0, 60.0))).unwrap();
    t.set_prop(lv, "row_h", Value::I64(18)).unwrap();
    t.set_prop(lv, "rows", Value::Str("r0\nr1\nr2\nr3".into())).unwrap();
    t.apply_pending();
    (t, lv)
}

/// T-SC-02：行点击 —— 载荷 (节点, 行下标)、边沿语义、无效区不回调、
/// UiVm 零写权。
#[test]
fn t_sc_02_list_row_activation() {
    let (t, lv) = list_scene();
    let input = FakeInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));
    let fired = Rc::new(RefCell::new(Vec::new()));
    let sink = fired.clone();
    vm.on_row_activate(move |n, row| sink.borrow_mut().push((n, row)));

    // scroll_max 算式（断言算式本身）：4 行 × 18 + 8 − 60 = 20。
    assert_eq!(
        nes_scene::scroll_max_of(&t, &UiStates::new(), lv, VP),
        20.0,
        "ListView extent = 行数*row_h + 8 - 视口高"
    );

    // 完整点击第 1 行（屏上 y 122..140，取 130）：回调 (lv, 1)。
    click(&mut vm, &t, &input, (140.0, 130.0));
    assert_eq!(*fired.borrow(), vec![(lv, 1u16)], "载荷 = (节点, 行下标)");
    fired.borrow_mut().clear();

    // 按下移出抬键：不回调（与 Button 同款边沿语义 —— 按下目标 ≠ 抬键命中）。
    input.set((140.0, 110.0), true); // 第 0 行按下
    input.set_wheel((0.0, 0.0));
    vm.update(&t, VP, SCALE);
    input.set((400.0, 40.0), false); // 移出空白抬键
    vm.update(&t, VP, SCALE);
    assert!(
        fired.borrow().is_empty(),
        "按下移出抬键不回调：{:?}",
        fired.borrow()
    );

    // 顶内衬区（y 100..104，local < 0）：不回调。
    click(&mut vm, &t, &input, (140.0, 102.0));
    assert!(fired.borrow().is_empty(), "内衬区无行");

    // 滚到底（步进 = row_h 18：18 → 夹到 20）后，行区整体上移。
    frame(&mut vm, &t, &input, (140.0, 130.0), (0.0, -1.0));
    frame(&mut vm, &t, &input, (140.0, 130.0), (0.0, -1.0));
    assert_eq!(scroll_of(&vm, lv), Some(20.0), "滚轮步进 = row_h，夹紧 20");
    // y=159：local = 159−100−4+20 = 75 → 行 4 超出行数（4 行 0..3）→ 不回调。
    click(&mut vm, &t, &input, (140.0, 159.0));
    assert!(fired.borrow().is_empty(), "越界行不回调");
    // y=145：local = 61 → 行 3 → 回调 (lv, 3)（滚到底后末行落在视口内）。
    click(&mut vm, &t, &input, (140.0, 145.0));
    assert_eq!(*fired.borrow(), vec![(lv, 3u16)], "滚动后行下标随 scroll 反解");

    // UiVm 零写权：属性表 selected 仍是缺省 -1（选中落账由宿主做）。
    assert_eq!(
        t.prop(lv, "selected"),
        Some(&Value::I64(-1)),
        "UiVm 不写属性表"
    );
}

/// T-SC-03：scrolls 生灭纪律 —— 死节点清扫、无输入视图全清、瞬态
/// 不入语义指纹。
#[test]
fn t_sc_03_scroll_state_lifecycle() {
    let (mut t, sv, _btn, _tabs) = scroll_scene();
    let input = FakeInput::default();
    let mut vm = UiVm::new();
    vm.set_input_view(Rc::new(input.clone()));

    // 滚出非零偏移；断言瞬态不进语义指纹（与悬停/按下同口径）。
    frame(&mut vm, &t, &input, (30.0, 30.0), (0.0, -1.0));
    assert_eq!(scroll_of(&vm, sv), Some(48.0), "前置：滚动瞬态已产生");
    let before = scene_fingerprint(&t, None);
    frame(&mut vm, &t, &input, (30.0, 30.0), (0.0, -1.0));
    assert_eq!(scroll_of(&vm, sv), Some(70.0), "前置：瞬态继续变化");
    assert_eq!(scene_fingerprint(&t, None), before, "scrolls 不入语义指纹");

    // 死节点清扫：删掉滚动控件后，下一帧 update 清掉它的偏移。
    t.remove_node(sv, false);
    t.apply_pending();
    vm.update(&t, VP, SCALE);
    assert!(
        vm.states_rc().borrow().scrolls.is_empty(),
        "死节点清扫：scrolls 悬垂不留"
    );

    // 无输入视图全清：无输入面的 UiVm update 后 scrolls 保持空。
    let t2 = SceneTree::new("root");
    let mut vm2 = UiVm::new();
    vm2.update(&t2, VP, SCALE);
    assert!(
        vm2.states_rc().borrow().scrolls.is_empty(),
        "无输入视图：状态全清"
    );
}

/// T-SC-04：三个新 kind —— schema 封闭属性/缺省值、ALL/index 一致性、
/// 稳定名往返、RON 场景往返（既有 ALL 遍历测试
/// `every_tag_has_a_schema_with_consistent_types` 自动覆盖新成员）。
#[test]
fn t_sc_04_kinds_schema_and_ron_roundtrip() {
    // ALL 与 index 逐位一致；稳定名可逆。
    for (i, tag) in NodeKindTag::ALL.iter().enumerate() {
        assert_eq!(tag.index(), i, "{tag:?} index 与 ALL 位置一致");
        assert_eq!(NodeKindTag::from_str_exact(tag.as_str()), Some(*tag));
    }
    assert_eq!(NodeKindTag::from_str_exact("ScrollView"), Some(NodeKindTag::ScrollView));
    assert_eq!(NodeKindTag::from_str_exact("ListView"), Some(NodeKindTag::ListView));
    assert_eq!(NodeKindTag::from_str_exact("Tabs"), Some(NodeKindTag::Tabs));
    assert_eq!(NodeKindTag::from_str_exact("ScrollViewX"), None, "封闭集合外拒绝");
    // 继承：三者均直接挂 Control。
    for tag in [NodeKindTag::ScrollView, NodeKindTag::ListView, NodeKindTag::Tabs] {
        assert_eq!(tag.base(), Some(NodeKindTag::Control));
        assert!(tag.is_a(NodeKindTag::Control));
        assert_eq!(tag.kind().tag(), tag, "kind/tag 可逆");
    }

    // schema 封闭属性与缺省值（照 TextInput 模式，专属键不与链上冲突）。
    let sv = NodeSchema::of(NodeKindTag::ScrollView);
    let names: Vec<&str> = sv.own_props().iter().map(|p| p.name()).collect();
    assert_eq!(names, vec!["step"]);
    assert_eq!(sv.default_value("step"), Some(&Value::I64(48)));
    assert_eq!(
        sv.validate("step", &Value::I64(99999)),
        Ok(Value::I64(512)),
        "数值提示夹紧"
    );
    assert!(sv.validate("scroll", &Value::I64(1)).is_err(), "封闭属性：未知键拒绝");

    let lv = NodeSchema::of(NodeKindTag::ListView);
    let names: Vec<&str> = lv.own_props().iter().map(|p| p.name()).collect();
    assert_eq!(names, vec!["rows", "row_h", "selected", "text_slot", "sel_fill_slot"]);
    assert_eq!(lv.default_value("row_h"), Some(&Value::I64(18)));
    assert_eq!(lv.default_value("selected"), Some(&Value::I64(-1)));
    assert_eq!(lv.default_value("text_slot"), Some(&Value::Str("text".into())));
    assert_eq!(lv.default_value("sel_fill_slot"), Some(&Value::Str("selected".into())));
    // 链上聚合：Node + Control + ListView 自身 = 2 + 12 + 5（Control 含
    // S16.6 九宫格五键 + S16.7 modulate/tiling 两开关；S16.6 漏改的
    // 计数断言在 S16.7 补齐 —— 该断言在 fea4083 即红）。
    assert_eq!(lv.len(), 19);

    let tabs = NodeSchema::of(NodeKindTag::Tabs);
    let names: Vec<&str> = tabs.own_props().iter().map(|p| p.name()).collect();
    assert_eq!(names, vec!["tabs", "tab_w", "active", "text_slot", "sel_fill_slot"]);
    assert_eq!(tabs.default_value("tab_w"), Some(&Value::I64(64)));
    assert_eq!(tabs.default_value("active"), Some(&Value::I64(-1)));

    // RON 往返：非缺省专有属性 + kind 名随场景文件 round-trip。
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "sv", NodeKind::ScrollView);
    t.set_prop(a, "size", Value::Vec2(Vec2::new(64.0, 32.0))).unwrap();
    t.set_prop(a, "step", Value::I64(24)).unwrap();
    let b = t.add_node(t.root(), "lv", NodeKind::ListView);
    t.set_prop(b, "rows", Value::Str("alpha\nbeta".into())).unwrap();
    t.set_prop(b, "row_h", Value::I64(20)).unwrap();
    t.set_prop(b, "selected", Value::I64(1)).unwrap();
    let c = t.add_node(t.root(), "tabs", NodeKind::Tabs);
    t.set_prop(c, "tabs", Value::Str("x\ny\nz".into())).unwrap();
    t.set_prop(c, "tab_w", Value::I64(40)).unwrap();
    t.set_prop(c, "active", Value::I64(2)).unwrap();
    t.apply_pending();

    let ron = scene_io::write_ron(&t, &PackOptions::verbose());
    for name in ["ScrollView", "ListView", "Tabs"] {
        assert!(ron.contains(name), "RON 应含稳定名 {name}");
    }
    let back = instantiate(&ron).unwrap();
    let kids = back.children(back.root());
    assert_eq!(kids.len(), 3);
    assert_eq!(back.kind_tag(kids[0]), Some(NodeKindTag::ScrollView));
    assert_eq!(back.kind_tag(kids[1]), Some(NodeKindTag::ListView));
    assert_eq!(back.kind_tag(kids[2]), Some(NodeKindTag::Tabs));
    assert_eq!(back.prop(kids[0], "step"), Some(&Value::I64(24)));
    assert_eq!(back.prop(kids[1], "rows"), Some(&Value::Str("alpha\nbeta".into())));
    assert_eq!(back.prop(kids[1], "row_h"), Some(&Value::I64(20)));
    assert_eq!(back.prop(kids[1], "selected"), Some(&Value::I64(1)));
    assert_eq!(back.prop(kids[2], "tabs"), Some(&Value::Str("x\ny\nz".into())));
    assert_eq!(back.prop(kids[2], "tab_w"), Some(&Value::I64(40)));
    assert_eq!(back.prop(kids[2], "active"), Some(&Value::I64(2)));
    // 未覆写的链上属性落缺省（往返不丢继承缺省）。
    assert!(back.prop(kids[0], "step").is_some());
    assert_eq!(back.prop(kids[1], "sel_fill_slot"), Some(&Value::Str("selected".into())));
}
