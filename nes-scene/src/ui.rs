//! S12.1 组件库交互层：主题色板 + UiVm 交互状态机。
//!
//! 三件套之二/三（S12.0 设计冻结）：
//!
//! - **主题即场景节点**：`Theme` 节点带八槽位语义色板（I64
//!   `0xRRGGBBAA` 打包）。控件引用**槽位名**而非写死颜色 —— 换主题
//!   节点即整体换肤。无 Theme 节点时用 [`ThemeColors::DEFAULT_DARK`]
//!   兜底（像素工程师风深色，S12.0 §6）。
//! - **UiVm 交互状态机**：[`ScriptVm`](crate::script::ScriptVm) 同构
//!   —— 悬停/按下等瞬态住本 VM 的状态表（**不进属性表**：写属性 =
//!   弄脏文档 + 进事务 + 进语义指纹，三重错误）；输入经
//!   [`InputView`](crate::InputView) 注入（S7.2 形态复用）；提交/
//!   激活走钩子回调，UiVm 自身**零写权**（五层纪律延续）。
//! - **焦点路由与文本输入（S12-2）**：单一焦点槽（`Option<NodeId>`），
//!   点击 TextInput 夺焦、Tab 在可焦点控件（Button/TextInput）间按
//!   场景序轮转、点击空白 = 失焦 + 提交。输入框的**草稿与光标是
//!   UiVm 瞬态**（[`TextState`]），回车提交 / Esc 回滚 / 失焦提交；
//!   提交不直写属性表，只发 [`UiVm::on_commit`] 钩子。
//!
//! 命中口径：控件锚定**视口**（ControlState 单级锚定，S3 契约），
//! UiVm 以 `anchor * viewport + offset`、尺寸 `size` 直算视口矩形，
//! 无嵌套布局递归。命中优先级 = 前序序最后者（与提取层 last-write-wins
//! 的相机/主题仲裁同款确定性规则）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::identity::NodeId;
use crate::node::NodeKindTag;
use crate::tree::SceneTree;
use crate::value::Value;

/// 八槽位语义色板（名字 + 缺省深色值，RGBA8 直 alpha）。
///
/// 缺省即 [`ThemeColors::DEFAULT_DARK`]；schema 的 Theme 属性缺省、
/// 无主题节点的提取兜底，都从这一张表出 —— 缺省只有一份。
pub const THEME_SLOTS: [(&str, [u8; 4]); 8] = [
    ("bg", [0x14, 0x16, 0x1A, 0xFF]),        // 窗口底
    ("panel", [0x1E, 0x22, 0x28, 0xFF]),      // 面板
    ("border", [0x3A, 0x40, 0x48, 0xFF]),     // 边框
    ("text", [0xD8, 0xDC, 0xE2, 0xFF]),       // 正文
    ("text_dim", [0x7A, 0x82, 0x8C, 0xFF]),   // 次级文字
    ("selected", [0x2E, 0x4A, 0x6B, 0xFF]),   // 选中
    ("accent", [0x4A, 0x9E, 0xFF, 0xFF]),     // 强调
    ("danger", [0xD2, 0x4B, 0x4B, 0xFF]),     // 危险
];

/// 一份解析好的主题色板。
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct ThemeColors {
    /// 槽位颜色，序与 [`THEME_SLOTS`] 一致。
    pub slots: [[u8; 4]; 8],
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self::DEFAULT_DARK
    }
}

impl ThemeColors {
    /// 缺省深色主题（像素工程师风，S12.0 §6）。
    pub const DEFAULT_DARK: Self = Self {
        slots: [
            [0x14, 0x16, 0x1A, 0xFF],
            [0x1E, 0x22, 0x28, 0xFF],
            [0x3A, 0x40, 0x48, 0xFF],
            [0xD8, 0xDC, 0xE2, 0xFF],
            [0x7A, 0x82, 0x8C, 0xFF],
            [0x2E, 0x4A, 0x6B, 0xFF],
            [0x4A, 0x9E, 0xFF, 0xFF],
            [0xD2, 0x4B, 0x4B, 0xFF],
        ],
    };

    /// 从 Theme 节点属性解析（I64 `0xRRGGBBAA`；缺名/类型错回缺省）。
    pub fn from_tree(tree: &SceneTree, node: NodeId) -> Self {
        let mut out = Self::DEFAULT_DARK;
        for (i, (name, _)) in THEME_SLOTS.iter().enumerate() {
            if let Some(Value::I64(packed)) = tree.prop(node, name) {
                let v = *packed as u64;
                out.slots[i] = [
                    (v >> 24) as u8,
                    (v >> 16) as u8,
                    (v >> 8) as u8,
                    v as u8,
                ];
            }
        }
        out
    }

    /// 按槽位名取色（未知名 → `None`，调用方决定回退）。
    pub fn slot(&self, name: &str) -> Option<[u8; 4]> {
        THEME_SLOTS
            .iter()
            .position(|(n, _)| *n == name)
            .map(|i| self.slots[i])
    }
}

/// 单个控件节点的瞬态交互状态（S12.0 §3.2 四态词汇）。
///
/// 内存态：不序列化、不进语义指纹（与 script locals 同口径）。
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct WidgetState {
    /// 悬停。
    pub hover: bool,
    /// 按下（按下且未释放）。
    pub pressed: bool,
    /// 选中（S12-2 列表/多选用；Button 不用）。
    pub selected: bool,
    /// 焦点（Button/TextInput 可聚焦；提取层据此做 accent 边框换档）。
    pub focused: bool,
}

/// 单个文本输入框的编辑瞬态（S12-2）：草稿与光标。
///
/// **不是 `Copy`**：草稿是编辑态，随焦点会话生灭。内存态：不序列化、
/// 不进属性表、不进语义指纹（与悬停/按下同口径）。
#[derive(Clone, Default, PartialEq, Debug)]
pub struct TextState {
    /// 编辑草稿（回车/失焦提交的整体值；Esc 回滚到节点已提交值）。
    pub draft: String,
    /// 光标位置（草稿的字符下标，按 `chars().count()` 口径）。
    pub caret: usize,
}

/// 全部控件节点的瞬态状态表（UiVm 与提取层共享的只读面）。
///
/// 两张子表：`widgets` 是悬停/按下等小旗标（`Copy`），`texts` 是
/// TextInput 的编辑会话（草稿/光标，非 `Copy`）。
#[derive(Default, Debug)]
pub struct UiStates {
    /// 四态词汇（悬停/按下/选中/焦点）。
    pub widgets: HashMap<NodeId, WidgetState>,
    /// TextInput 编辑会话（仅持有焦点期间存在）。
    pub texts: HashMap<NodeId, TextState>,
}

impl UiStates {
    /// 空状态表。
    pub fn new() -> Self {
        Self::default()
    }
}

type InputSlot = Rc<RefCell<Option<Rc<dyn crate::script::InputView>>>>;

/// UI 交互状态机（ScriptVm 同构；S12.0 §2.2 + S12-2 焦点路由）。
///
/// 宿主每帧在 tick 与提取之间调 [`UiVm::update`]：读输入快照 →
/// 命中测算 → 更新悬停/按下 → 抬键命中即激活（回调）→ 焦点路由与
/// 文本输入状态机。提取层经 [`UiVm::states_rc`] 读状态做四态着色
/// —— UI 是投影，状态不回流属性表。
pub struct UiVm {
    states: Rc<RefCell<UiStates>>,
    input: InputSlot,
    /// 上一帧左键是否按下（边沿检测）。
    prev_left: bool,
    /// 按下时命中的节点（抬键时仍命中才激活 —— 标准 UI 语义）。
    press_target: Option<NodeId>,
    /// 单一焦点槽（Button/TextInput 可聚焦；TextInput 持有编辑会话）。
    focus: Option<NodeId>,
    /// 上一帧 Tab 是否按住（焦点轮转的边沿检测）。
    prev_tab: bool,
    /// 上一帧 Backspace 是否按住（删除边沿检测）。
    prev_backspace: bool,
    /// 上一帧 Enter 是否按住（提交边沿检测）。
    prev_enter: bool,
    /// 上一帧 Escape 是否按住（回滚边沿检测）。
    prev_escape: bool,
    /// 激活钩子（按钮抬键命中；UiVm 零写权，动作由宿主定义）。
    on_activate: Option<Box<dyn FnMut(NodeId)>>,
    /// 提交钩子（S12-2：TextInput 回车/失焦时回调整体草稿值；
    /// UiVm 零写权 —— 落不落属性表由宿主决定）。
    on_commit: Option<Box<dyn FnMut(NodeId, Value)>>,
}

impl Default for UiVm {
    fn default() -> Self {
        Self::new()
    }
}

impl UiVm {
    /// 空状态机（无输入视图时 `update` 为空转，测试/无窗宿主安全）。
    pub fn new() -> Self {
        Self {
            states: Rc::new(RefCell::new(UiStates::new())),
            input: Rc::new(RefCell::new(None)),
            prev_left: false,
            press_target: None,
            focus: None,
            prev_tab: false,
            prev_backspace: false,
            prev_enter: false,
            prev_escape: false,
            on_activate: None,
            on_commit: None,
        }
    }

    /// 注入输入视图（与 `ScriptVm::set_input_view` 同款）。
    pub fn set_input_view(&mut self, view: Rc<dyn crate::script::InputView>) {
        *self.input.borrow_mut() = Some(view);
    }

    /// 注册激活钩子（按钮抬键命中时回调；后注册者覆盖）。
    pub fn on_activate(&mut self, f: impl FnMut(NodeId) + 'static) {
        self.on_activate = Some(Box::new(f));
    }

    /// 注册提交钩子（S12-2：TextInput 回车/失焦时回调
    /// `(输入框节点, 提交值 Value::Str)`；后注册者覆盖）。
    pub fn on_commit(&mut self, f: impl FnMut(NodeId, Value) + 'static) {
        self.on_commit = Some(Box::new(f));
    }

    /// 状态表共享引用（提取层四态着色的只读面）。
    pub fn states_rc(&self) -> Rc<RefCell<UiStates>> {
        self.states.clone()
    }

    /// 查单节点状态（无记录 = 全 false）。
    pub fn state(&self, node: NodeId) -> WidgetState {
        self.states
            .borrow()
            .widgets
            .get(&node)
            .copied()
            .unwrap_or_default()
    }

    /// 查单节点编辑会话（无会话 = `None`；`Clone` 出去，不持借用）。
    pub fn text_state(&self, node: NodeId) -> Option<TextState> {
        self.states.borrow().texts.get(&node).cloned()
    }

    /// 当前焦点节点（无焦点 = `None`）。
    pub fn focus(&self) -> Option<NodeId> {
        self.focus
    }

    /// 每帧更新：命中测算 + 悬停/按下状态机 + 激活回调 + 焦点/文本输入。
    ///
    /// `mouse_scale` = 视图空间 / 客户区像素（窗口帧路径由运行时按
    /// `frame.viewport / 客户区尺寸` 折算；渲染把视图空间经 NDC 拉伸
    /// 铺满表面，鼠标必须同比例映射回视图空间命中才准 —— 窗口缩放
    /// 后错位的根修）。离屏/1:1 路径传 `(1.0, 1.0)`。
    ///
    /// 在 tick 之后、提取之前调用（看到的是当帧终值 —— 与 F-1 信号
    /// 泵同款时序裁决）。无输入视图时为空转（状态全清）。
    pub fn update(&mut self, tree: &SceneTree, viewport: (f32, f32), mouse_scale: (f32, f32)) {
        // 清死节点状态（悬垂不留 —— 与 ScriptVm 状态清扫同款纪律）。
        {
            let mut states = self.states.borrow_mut();
            states.widgets.retain(|n, _| tree.contains(*n));
            states.texts.retain(|n, _| tree.contains(*n));
            if let Some(f) = self.focus {
                if !tree.contains(f) {
                    self.focus = None;
                }
            }
        }

        let Some(input) = self.input.borrow().clone() else {
            // 无输入视图：状态全清（文档口径兑现 —— 悬停/按下/焦点是
            // 输入的投影，输入面缺席时不残留旧帧状态去驱动提取层着色）。
            {
                let mut states = self.states.borrow_mut();
                states.widgets.clear();
                states.texts.clear();
            }
            self.focus = None;
            self.press_target = None;
            self.prev_left = false;
            self.prev_tab = false;
            self.prev_backspace = false;
            self.prev_enter = false;
            self.prev_escape = false;
            return;
        };
        let (mx, my) = {
            let (x, y) = input.mouse();
            (x * mouse_scale.0, y * mouse_scale.1)
        };
        let left = input.button("left");

        // 前序遍历收集可焦点控件（Button/TextInput，可见者）的命中与
        // 轮转序（前序 = 提取层同款确定性序，后者命中 —— 同一仲裁规则）。
        let mut focusable: Vec<NodeId> = Vec::new();
        let mut hit: Option<NodeId> = None;
        let mut stack = vec![tree.root()];
        while let Some(node) = stack.pop() {
            let tag = tree.kind_tag(node);
            let is_widget = tag == Some(NodeKindTag::Button) || tag == Some(NodeKindTag::TextInput);
            let visible = tree
                .prop(node, "visible")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if is_widget && visible {
                let rect = widget_rect(tree, node, viewport);
                let inside = mx >= rect.0
                    && my >= rect.1
                    && mx < rect.0 + rect.2
                    && my < rect.1 + rect.3;
                if inside {
                    hit = Some(node);
                }
                focusable.push(node);
            }
            if let Some(data) = tree.get(node) {
                for &child in data.children.iter().rev() {
                    stack.push(child);
                }
            }
        }

        // —— 焦点路由（点击）——
        // 按下沿：点到的东西不是当前焦点输入框 → 先失焦（=提交）；
        // 点到别的输入框则随后夺焦。点空白（hit=None）在抬键沿失焦提交。
        let left_edge = left && !self.prev_left;
        let release_edge = !left && self.prev_left;
        self.prev_left = left;
        if left_edge {
            self.press_target = hit;
            let clicked_away = match (hit, self.focus) {
                (Some(n), Some(f)) => n != f,
                _ => false,
            };
            if clicked_away {
                self.blur_commit(tree);
            }
            // 按下沿命中输入框 → 夺焦（开启编辑会话，草稿 = 已提交值）。
            if let Some(n) = hit {
                if tree.kind_tag(n) == Some(NodeKindTag::TextInput) && self.focus != Some(n) {
                    self.focus_node(tree, n);
                }
            }
        }
        // 抬键沿：命中空白 = 失焦 + 提交（标准"点外面收起"语义）；
        // 激活回调**仅 Button**（TextInput 点击是夺焦，不是激活 —— S12-2）。
        if release_edge {
            if hit.is_none() {
                self.blur_commit(tree);
            }
            if let (Some(t), true) = (self.press_target, hit == self.press_target && hit.is_some())
            {
                if tree.kind_tag(t) == Some(NodeKindTag::Button) {
                    if let Some(cb) = self.on_activate.as_mut() {
                        cb(t);
                    }
                }
            }
            self.press_target = None;
        }

        // —— 焦点路由（Tab 轮转）——按下沿在可焦点序内循环（确定性）。
        let tab = key_down(&*input, "Tab", "tab");
        if tab && !self.prev_tab && !focusable.is_empty() {
            let next = match self.focus {
                Some(f) => {
                    let idx = focusable.iter().position(|&n| n == f);
                    match idx {
                        Some(i) => focusable[(i + 1) % focusable.len()],
                        None => focusable[0],
                    }
                }
                None => focusable[0],
            };
            if Some(next) != self.focus {
                self.blur_commit(tree);
                self.focus_node(tree, next);
            }
        }
        self.prev_tab = tab;

        // —— 文本输入状态机（仅路由到持焦点的 TextInput）——
        self.pump_text_input(tree, &*input);

        // 状态机：悬停 = 当前命中；按下 = 按住且目标是自己；焦点 = 焦点槽。
        let mut states = self.states.borrow_mut();
        for st in states.widgets.values_mut() {
            st.hover = false;
            st.pressed = false;
            st.focused = false;
        }
        if let Some(n) = hit {
            let st = states.widgets.entry(n).or_default();
            st.hover = true;
            st.pressed = left && self.press_target == Some(n);
        }
        if let Some(f) = self.focus {
            states.widgets.entry(f).or_default().focused = true;
        }
        drop(states);
    }

    /// 夺焦：旧焦点失焦（= 提交），新节点开编辑会话。
    ///
    /// TextInput 会初始化草稿（= 节点 `text` 已提交值、光标到末尾）；
    /// Button 只占焦点槽（无编辑会话，回车激活留待后续里程碑）。
    fn focus_node(&mut self, tree: &SceneTree, node: NodeId) {
        self.blur_commit(tree);
        self.focus = Some(node);
        if tree.kind_tag(node) == Some(NodeKindTag::TextInput) {
            let draft = node_text(tree, node);
            let caret = draft.chars().count();
            self.states
                .borrow_mut()
                .texts
                .insert(node, TextState { draft, caret });
        }
    }

    /// 失焦 = 提交：把草稿整体经 [`UiVm::on_commit`] 钩子交给宿主
    /// （UiVm 零写权 —— 不直写属性表），并关闭编辑会话。
    ///
    /// **仅 TextInput 失焦才提交**：Button 等纯占焦控件没有编辑会话，
    /// 失焦不发 `on_commit`（宿主的提交落账面只对输入框有意义 ——
    /// 无会话控件回读 `text` 属性伪造载荷是错的）。
    fn blur_commit(&mut self, tree: &SceneTree) {
        if let Some(f) = self.focus.take() {
            if tree.kind_tag(f) != Some(NodeKindTag::TextInput) {
                return;
            }
            // TextInput：提交草稿（无会话则提交节点现值 —— 会话从未开始）。
            let value = match self.states.borrow_mut().texts.remove(&f) {
                Some(ts) => Value::Str(ts.draft),
                None => Value::Str(node_text(tree, f)),
            };
            if let Some(cb) = self.on_commit.as_mut() {
                cb(f, value);
            }
        }
    }

    /// 文本输入泵：字符插入 / Backspace 删除 / Enter 提交 / Esc 回滚。
    ///
    /// - 字符：[`InputView::text`] 的 Unicode 标量值逐个插入光标处；
    ///   P0 只收 ASCII 可打印（`0x20..=0x7E`），非 ASCII 忽略。
    /// - Backspace（按下沿）：删光标前一字符。
    /// - Enter（按下沿）：提交草稿整体值（经 [`UiVm::on_commit`]），
    ///   焦点保留、会话继续。
    /// - Escape（按下沿）：回滚 —— 草稿 := 节点 `text` 已提交值。
    /// - 左右光标移动：P0 未做（可选缺口，后续里程碑补）。
    fn pump_text_input(&mut self, tree: &SceneTree, input: &dyn crate::script::InputView) {
        let Some(node) = self.focus else {
            // 无焦点也要推进键边沿，避免旧沿残留导致恢复焦点时误触发。
            self.prev_backspace = key_down(input, "Backspace", "backspace");
            self.prev_enter = key_down(input, "Enter", "enter");
            self.prev_escape = key_down(input, "Escape", "escape");
            return;
        };
        if tree.kind_tag(node) != Some(NodeKindTag::TextInput) {
            self.prev_backspace = key_down(input, "Backspace", "backspace");
            self.prev_enter = key_down(input, "Enter", "enter");
            self.prev_escape = key_down(input, "Escape", "escape");
            return;
        }

        let mut commit: Option<String> = None;
        {
            let mut states = self.states.borrow_mut();
            if let Some(ts) = states.texts.get_mut(&node) {
                // 字符插入（P0：ASCII 可打印，非 ASCII 忽略）。
                for cp in input.text() {
                    if (0x20..=0x7E).contains(&cp) {
                        let ch = char::from_u32(cp).unwrap_or('?');
                        let byte_idx = char_bound(&ts.draft, ts.caret);
                        ts.draft.insert(byte_idx, ch);
                        ts.caret += 1;
                    }
                }
                // Backspace：删除光标前一字符（按下沿）。
                let bs = key_down(input, "Backspace", "backspace");
                if bs && !self.prev_backspace && ts.caret > 0 {
                    let byte_idx = char_bound(&ts.draft, ts.caret - 1);
                    ts.draft.remove(byte_idx);
                    ts.caret -= 1;
                }
                self.prev_backspace = bs;
                // Enter：提交草稿整体值（按下沿）。
                let enter = key_down(input, "Enter", "enter");
                if enter && !self.prev_enter {
                    commit = Some(ts.draft.clone());
                }
                self.prev_enter = enter;
                // Escape：回滚到节点已提交值（按下沿）。
                let esc = key_down(input, "Escape", "escape");
                if esc && !self.prev_escape {
                    ts.draft = node_text(tree, node);
                    ts.caret = ts.draft.chars().count();
                }
                self.prev_escape = esc;
            }
        }

        if let Some(value) = commit {
            if let Some(cb) = self.on_commit.as_mut() {
                cb(node, Value::Str(value));
            }
        }
    }
}

/// 取节点 `text` 属性的字符串值（缺失/类型错 = 空串）。
fn node_text(tree: &SceneTree, node: NodeId) -> String {
    match tree.prop(node, "text") {
        Some(Value::Str(v)) => v.clone(),
        _ => String::new(),
    }
}

/// 键探针（双名兼容）：真实快照桥的键名是冻结大小写口径
///（`Key::from_name`：`"Enter"`/`"Tab"`…），S12 单测假读面用的是小写。
/// 两个名字都探，任一按住即按住 —— 真实运行时路径与单测路径同语义。
fn key_down(input: &dyn crate::script::InputView, canonical: &str, lower: &str) -> bool {
    input.key(canonical) || input.key(lower)
}

/// 草稿第 `char_idx` 个字符（字符下标）的字节下标（越界 = 末尾）。
fn char_bound(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

/// 控件的视口矩形 `(x, y, w, h)`（锚定视口单级解析，S3 契约）。
fn widget_rect(tree: &SceneTree, node: NodeId, viewport: (f32, f32)) -> (f32, f32, f32, f32) {
    let anchor = vec2_prop(tree, node, "anchor");
    let offset = vec2_prop(tree, node, "offset");
    let size = vec2_prop(tree, node, "size");
    (
        anchor.x * viewport.0 + offset.x,
        anchor.y * viewport.1 + offset.y,
        size.x,
        size.y,
    )
}

fn vec2_prop(tree: &SceneTree, node: NodeId, name: &str) -> crate::transform::Vec2 {
    match tree.prop(node, name) {
        Some(Value::Vec2(v)) => *v,
        _ => crate::transform::Vec2::ZERO,
    }
}
