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
//!   提交不直写属性表，只发 [`UiVm::on_commit`] 钩子。草稿生命周期的
//!   两个端点（S12-4）：**获焦沿**（无论点击或 Tab，单点
//!   [`UiVm::focus_node`])一律 `draft = text 属性值, caret = len`；
//!   **文档侧换绑**（宿主检测到选中变化 / undo-redo 落账）走
//!   [`UiVm::reset_text`]（置草稿、不动焦点、不触发提交）。
//! - **滚动与行点击（S12-3）**：ScrollView/ListView/Tabs 的垂直滚动
//!   偏移是 UiVm 瞬态（[`UiStates::scrolls`]，与悬停/按下同款生灭
//!   纪律）；滚轮（[`InputView::wheel`]，一次性）路由给前序序最后
//!   命中的滚动控件，步进夹紧在 `[0, scroll_max]`。ListView 行点击
//!   复用 Button 同款按下/抬键边沿机，抬键仍命中才回调
//!   [`UiVm::on_row_activate`]（只回报告 (节点, 行下标)，选中落账
//!   由宿主做 —— UiVm 零写权延续）。
//!
//! 命中口径：控件锚定**视口**（ControlState 单级锚定，S3 契约），
//! UiVm 以 `anchor * viewport + offset`、尺寸 `size` 直算视口矩形，
//! 无嵌套布局递归。命中优先级 = 前序序最后者（与提取层 last-write-wins
//! 的相机/主题仲裁同款确定性规则）。
//!
//! 滚动坐标约定（S12-3，单处实现）：祖先 ScrollView 把内容**向上**
//! 平移其滚动偏移渲染，命中矩形随之**减去**同一偏移
//! （[`scroll_context_of`] 返回沿祖先链的偏移之和），再做祖先
//! ScrollView 视口矩形交集裁剪（滚出容器的内容不可命中）。无滚动
//! 祖先时上下文为 `(0.0, None)`，既有 Button/TextInput 命中零变化。

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
/// 三张子表：`widgets` 是悬停/按下等小旗标（`Copy`），`texts` 是
/// TextInput 的编辑会话（草稿/光标，非 `Copy`），`scrolls` 是
/// ScrollView/ListView/Tabs 的垂直滚动偏移（S12-3）。
#[derive(Default, Debug)]
pub struct UiStates {
    /// 四态词汇（悬停/按下/选中/焦点）。
    pub widgets: HashMap<NodeId, WidgetState>,
    /// TextInput 编辑会话（仅持有焦点期间存在）。
    pub texts: HashMap<NodeId, TextState>,
    /// 垂直滚动偏移（S12-3；仅滚轮实际改动过的滚动控件有记录）。
    /// 与 widgets/texts 同款生灭纪律：死节点清扫、无输入视图全清、
    /// 不序列化、不进语义指纹。
    pub scrolls: HashMap<NodeId, f32>,
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
    /// 行点击钩子（S12-3：ListView 抬键仍命中时回调 (节点, 行下标)；
    /// UiVm 零写权 —— 选中落账（写 `selected` 属性）由宿主做）。
    on_row_activate: Option<Box<dyn FnMut(NodeId, u16)>>,
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
            on_row_activate: None,
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

    /// 注册行点击钩子（S12-3：ListView 抬键仍命中时回调
    /// `(列表节点, 行下标 u16，0 起)`；后注册者覆盖）。
    pub fn on_row_activate(&mut self, f: impl FnMut(NodeId, u16) + 'static) {
        self.on_row_activate = Some(Box::new(f));
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

    /// 宿主换绑专用（S12-4）：把节点的编辑草稿整体替换为 `value`，
    /// 光标移到末尾。**不动焦点槽、不触发 [`UiVm::on_commit`]** ——
    /// 与获焦初始化（[`Self::focus_node`]）是同一份赋值，但方向相反：
    /// 获焦是"草稿 := 文档"，这里是文档侧变化（换选中 / undo/redo 落账）
    /// 之后宿主主动把草稿拉回文档真相 —— 持焦中的旧草稿即刻作废，
    /// 输入框显示跟手刷新（提取层有会话即显示草稿）。
    ///
    /// 无条件 upsert（含未聚焦节点）：reset 后草稿与宿主刚写入的
    /// `text` 属性同值，即便之后失焦提交也是同值回写 —— 落账面的
    /// unchanged 检查自然跳过，零副作用。
    pub fn reset_text(&mut self, node: NodeId, value: &str) {
        self.states.borrow_mut().texts.insert(
            node,
            TextState {
                draft: value.to_string(),
                caret: value.chars().count(),
            },
        );
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
            states.scrolls.retain(|n, _| tree.contains(*n));
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
                states.scrolls.clear();
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
        // S12-3：滚动控件（ScrollView/ListView/Tabs，可见者）同轮收集，
        // 命中矩形统一经祖先滚动上下文平移/裁剪（[`contextual_rect`]，
        // 坐标约定见模块注释 —— 单处实现）；滚轮路由取前序序最后命中的
        // 滚动控件，与 `hit` 同一条 last-write-wins 仲裁。
        let mut focusable: Vec<NodeId> = Vec::new();
        let mut hit: Option<NodeId> = None;
        let mut scroll_hit: Option<(NodeId, (f32, f32, f32, f32))> = None;
        let mut wheel_apply: Option<(NodeId, f32, f32)> = None;
        {
            let states = self.states.borrow();
            let mut stack = vec![tree.root()];
            while let Some(node) = stack.pop() {
                let tag = tree.kind_tag(node);
                let is_widget =
                    tag == Some(NodeKindTag::Button) || tag == Some(NodeKindTag::TextInput);
                let is_scroller = tag == Some(NodeKindTag::ScrollView)
                    || tag == Some(NodeKindTag::ListView)
                    || tag == Some(NodeKindTag::Tabs);
                let visible = tree
                    .prop(node, "visible")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                if (is_widget || is_scroller) && visible {
                    let rect = contextual_rect(
                        widget_rect(tree, node, viewport),
                        scroll_context_of(tree, &states, node, viewport),
                    );
                    let inside = mx >= rect.0
                        && my >= rect.1
                        && mx < rect.0 + rect.2
                        && my < rect.1 + rect.3;
                    if inside {
                        hit = Some(node);
                        if is_scroller {
                            scroll_hit = Some((node, rect));
                        }
                    }
                    if is_widget {
                        focusable.push(node);
                    }
                }
                if let Some(data) = tree.get(node) {
                    for &child in data.children.iter().rev() {
                        stack.push(child);
                    }
                }
            }

            // 滚轮路由（wheel 一次性字段：当帧有效，无命中滚动控件即丢弃）。
            // scroll = clamp(scroll − wheel.y * step, 0, scroll_max)：向上
            // 滚（+y）内容回落向顶。夹紧上限是内容高度 − 视口高（下限 0）。
            let wheel = input.wheel();
            if wheel.1 != 0.0 {
                if let Some((n, _)) = scroll_hit {
                    let step = wheel_step(tree, n);
                    let max = scroll_max_of(tree, &states, n, viewport);
                    let old = states.scrolls.get(&n).copied().unwrap_or(0.0);
                    let new = (old - wheel.1 * step).clamp(0.0, max);
                    // 值无变化不落表：未滚动的控件不在 scrolls 里留 0.0 壳。
                    if new != old {
                        wheel_apply = Some((n, new, old));
                    }
                }
            }
        }
        if let Some((n, new, _old)) = wheel_apply {
            self.states.borrow_mut().scrolls.insert(n, new);
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
        // 激活回调**仅 Button**（TextInput 点击是夺焦，不是激活 —— S12-2）；
        // ListView 走同款边沿机发行点击（S12-3）—— 抬键仍命中才结算。
        if release_edge {
            if hit.is_none() {
                self.blur_commit(tree);
            }
            if let (Some(t), true) = (self.press_target, hit == self.press_target && hit.is_some())
            {
                match tree.kind_tag(t) {
                    Some(NodeKindTag::Button) => {
                        if let Some(cb) = self.on_activate.as_mut() {
                            cb(t);
                        }
                    }
                    Some(NodeKindTag::ListView) => {
                        self.row_activate(tree, t, my, scroll_hit);
                    }
                    _ => {}
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
    /// - 字符：[`InputView::text`] 的 UTF-16 码元流逐单元解码，**全部
    ///   非控制 Unicode 标量**插入光标处（IME 第 1 期 —— 中文/全角/
    ///   假名可入草稿）：控制码 `0x00..=0x1F` 与 `0x7F` 丢弃；代理对
    ///   （高 `0xD800..=0xDBFF` + 紧随低 `0xDC00..=0xDFFF`）合成一个
    ///   标量、光标只 +1；孤立低代理与流尾悬挂高代理丢弃（`text` 是
    ///   一次性当帧批 —— `InputCollector::frame()` 用 `mem::take` 取走
    ///   即清、本 VM 无跨帧文本缓存，低代理落在下一帧时高代理已不可
    ///   寻；真实窗口里 WM_CHAR 代理对背靠背投递，同帧到达是常态）。
    /// - Backspace（按下沿）：删光标前一**字符**（非字节 —— 草稿是
    ///   String 按字节索引，经 [`char_bound`] 换算字符边界）。
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
                // 字符插入（IME 第 1 期：UTF-16 码元流 -> 全部非控制标量；
                // 契约细节见本方法 doc）。
                let stream = input.text();
                let mut i = 0usize;
                while i < stream.len() {
                    let cp = stream[i];
                    let scalar = if (0xD800..=0xDBFF).contains(&cp) {
                        match stream
                            .get(i + 1)
                            .copied()
                            .filter(|lo| (0xDC00..=0xDFFF).contains(lo))
                        {
                            Some(lo) => {
                                i += 2;
                                // 合成公式恒落 0x10000..=0x10FFFF ——
                                // from_u32 必 Some（unwrap 口径见下）。
                                char::from_u32(
                                    0x1_0000 + ((cp - 0xD800) << 10) + (lo - 0xDC00),
                                )
                            }
                            None => {
                                i += 1; // 流尾悬挂高代理：丢弃（见 doc）。
                                None
                            }
                        }
                    } else if (0xDC00..=0xDFFF).contains(&cp) {
                        i += 1; // 孤立低代理：非法序，丢弃。
                        None
                    } else {
                        i += 1;
                        char::from_u32(cp)
                    };
                    let Some(ch) = scalar else { continue };
                    // 控制码丢弃（C0 与 DEL；代理合成 ≥0x10000 恒不中）。
                    if (ch as u32) <= 0x1F || ch as u32 == 0x7F {
                        continue;
                    }
                    let byte_idx = char_bound(&ts.draft, ts.caret);
                    ts.draft.insert(byte_idx, ch);
                    ts.caret += 1;
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

    /// ListView 行点击结算（S12-3）：抬键仍命中同一列表时，把点击点
    /// 换算回内容空间求行下标 —— 行 i 的屏上 y = 列表顶 + [`ROW_INSET`]
    /// 内衬 + i × row_h − 自身滚动，反解 i = (my − 顶 − 4 + scroll) / row_h。
    /// 顶内衬区（local < 0）与超出实际行数的下标不回调。
    /// UiVm 零写权：只经 [`Self::on_row_activate`] 报告 (节点, 行下标)，
    /// 选中落账（写 `selected` 属性）由宿主做。
    fn row_activate(
        &mut self,
        tree: &SceneTree,
        node: NodeId,
        my: f32,
        scroll_hit: Option<(NodeId, (f32, f32, f32, f32))>,
    ) {
        // 本帧命中矩形（已按祖先滚动上下文平移/裁剪 —— 与按下沿同源）。
        let Some((_, rect)) = scroll_hit.filter(|(n, _)| *n == node) else {
            return;
        };
        let scroll = self
            .states
            .borrow()
            .scrolls
            .get(&node)
            .copied()
            .unwrap_or(0.0);
        let local = my - rect.1 - ROW_INSET + scroll;
        if local < 0.0 {
            return; // 顶内衬区：无行。
        }
        // local >= 0 时 `as u16` 即向下取整（Rust 浮点转整饱和截断）。
        let row = (local / list_row_h(tree, node)) as u16;
        if (row as usize) < rows_count(tree, node, "rows") {
            if let Some(cb) = self.on_row_activate.as_mut() {
                cb(node, row);
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

// —— S12-3 滚动/行命中测算（纯助手，提取层复用；单实现纪律）——

/// 列表/页签行区顶内衬（像素）。行 i 的屏上 y = 列表视口顶 + 4 +
/// i × row_h − 自身滚动（[`UiVm::row_activate`] 按此反解行下标）。
const ROW_INSET: f32 = 4.0;
/// 滚动内容总内衬（像素；上下各 4px，与 [`ROW_INSET`] 对应）。
const SCROLL_PAD: f32 = 8.0;
/// ListView 行高缺省（schema `row_h` 缺省同值；属性缺失/类型错回退到它）。
const DEFAULT_ROW_H: f32 = 18.0;
/// ScrollView 滚轮步进缺省（schema `step` 缺省同值）。
const DEFAULT_STEP: f32 = 48.0;

/// 节点的滚动上下文：`(沿祖先链的滚动偏移之和, 祖先 ScrollView 视口
/// 矩形交集)`（S12-3）。命中测算、extent 测算与提取层烘焙三处共用，
/// 坐标约定单处实现（矩形减偏移，见 [`contextual_rect`] 与模块注释）。
///
/// - 偏移和：祖先链上每个 ScrollView 在 [`UiStates::scrolls`] 里的
///   偏移相加（无记录 = 0；不查可见性 —— 场景层无可见性继承，与命中
///   循环逐节点判 visible 的口径一致）；
/// - 矩形：祖先 ScrollView 视口矩形（`anchor * viewport + offset`）
///   逐级求交；无 ScrollView 祖先 = `None`（此时偏移和必为 0 ——
///   既有 Button/TextInput 命中零变化，additive 保证）。
///
/// 纯函数：只读树属性与瞬态表，不写任何状态。
pub fn scroll_context_of(
    tree: &SceneTree,
    ui: &UiStates,
    node: NodeId,
    viewport: (f32, f32),
) -> (f32, Option<(f32, f32, f32, f32)>) {
    let mut sum = 0.0f32;
    let mut clip: Option<(f32, f32, f32, f32)> = None;
    let mut cur = tree.parent(node);
    while let Some(p) = cur {
        if tree.kind_tag(p) == Some(NodeKindTag::ScrollView) {
            sum += ui.scrolls.get(&p).copied().unwrap_or(0.0);
            let r = widget_rect(tree, p, viewport);
            clip = Some(match clip {
                None => r,
                Some(c) => rect_intersect(c, r),
            });
        }
        cur = tree.parent(p);
    }
    (sum, clip)
}

/// 把命中矩形放入滚动上下文（坐标约定**全 crate 仅此一处**）：祖先
/// ScrollView 把内容向上平移 scroll 像素渲染，命中矩形 y 随之减去同一
/// 偏移和，再做祖先视口矩形交集裁剪（滚出容器的内容不可命中）。
/// 无滚动祖先（上下文 `(0.0, None)`）时原样返回 —— 既有行为零变化。
pub fn contextual_rect(
    rect: (f32, f32, f32, f32),
    context: (f32, Option<(f32, f32, f32, f32)>),
) -> (f32, f32, f32, f32) {
    let (sum, clip) = context;
    let shifted = (rect.0, rect.1 - sum, rect.2, rect.3);
    match clip {
        Some(c) => rect_intersect(shifted, c),
        None => shifted,
    }
}

/// 矩形求交（空交 = 零尺寸矩形，永不命中）。
fn rect_intersect(
    a: (f32, f32, f32, f32),
    b: (f32, f32, f32, f32),
) -> (f32, f32, f32, f32) {
    let x0 = a.0.max(b.0);
    let y0 = a.1.max(b.1);
    let x1 = (a.0 + a.2).min(b.0 + b.2);
    let y1 = (a.1 + a.3).min(b.1 + b.3);
    if x1 <= x0 || y1 <= y0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    (x0, y0, x1 - x0, y1 - y0)
}

/// 滚动上限（内容超出视口的高度，下限 0；纯函数）。
///
/// - ListView：行数 × row_h + 8 内衬 − 视口高（下限 0）；
/// - Tabs：页签数 × 行高 + 8 内衬 − 视口高（Tabs 无 row_h 属性，按
///   ListView 缺省行高 [`DEFAULT_ROW_H`] 口径）；
/// - ScrollView：可见后代控件底缘最大值 − 自身底（下限 0）。按**固有
///   坐标**测算（不含任何滚动偏移 —— 内容范围不随当前滚动变化）；
///   嵌套滚动容器的子树被其裁剪，以容器自身矩形计入、不再下探；
/// - 其他类型 = 0.0。
///
/// `ui` 参数与 [`scroll_context_of`] 保持同形（提取层同一调用点两种
/// 测算），当前测算不读瞬态表。
pub fn scroll_max_of(tree: &SceneTree, ui: &UiStates, node: NodeId, viewport: (f32, f32)) -> f32 {
    let _ = ui;
    let Some(tag) = tree.kind_tag(node) else {
        return 0.0;
    };
    let rect = widget_rect(tree, node, viewport);
    match tag {
        NodeKindTag::ListView => {
            let rows = rows_count(tree, node, "rows") as f32;
            (rows * list_row_h(tree, node) + SCROLL_PAD - rect.3).max(0.0)
        }
        NodeKindTag::Tabs => {
            let tabs = rows_count(tree, node, "tabs") as f32;
            (tabs * DEFAULT_ROW_H + SCROLL_PAD - rect.3).max(0.0)
        }
        NodeKindTag::ScrollView => {
            let own_bottom = rect.1 + rect.3;
            let mut max_bottom = own_bottom;
            let mut stack: Vec<NodeId> = tree.children(node).to_vec();
            while let Some(n) = stack.pop() {
                let Some(t) = tree.kind_tag(n) else { continue };
                if t == NodeKindTag::ScrollView
                    || t == NodeKindTag::ListView
                    || t == NodeKindTag::Tabs
                {
                    // 嵌套滚动容器：自身矩形计入，其子树被裁剪不再下探。
                    // 隐藏容器不计（S12-3 评审 [low]：与下方非容器后代
                    // 的 visible 口径一致，否则隐藏容器虚增 scroll_max）。
                    let visible = tree
                        .prop(n, "visible")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    if visible {
                        let r = widget_rect(tree, n, viewport);
                        max_bottom = max_bottom.max(r.1 + r.3);
                    }
                    continue;
                }
                let visible = tree
                    .prop(n, "visible")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                if visible && t.is_a(NodeKindTag::Control) {
                    let r = widget_rect(tree, n, viewport);
                    max_bottom = max_bottom.max(r.1 + r.3);
                }
                if let Some(data) = tree.get(n) {
                    for &child in data.children.iter() {
                        stack.push(child);
                    }
                }
            }
            (max_bottom - own_bottom).max(0.0)
        }
        _ => 0.0,
    }
}

/// 滚轮步进（像素/格）：ScrollView 取 `step` 属性，ListView 取 `row_h`
/// 属性（行进列 —— 一次一格），Tabs 无行高属性按缺省行高。缺失/类型
/// 错回缺省；下限 1.0 防除零/原地踏步。
fn wheel_step(tree: &SceneTree, node: NodeId) -> f32 {
    match tree.kind_tag(node) {
        Some(NodeKindTag::ScrollView) => i64_prop(tree, node, "step")
                .map(|v| (v as f32).max(1.0))
                .unwrap_or(DEFAULT_STEP),
        Some(NodeKindTag::ListView) => list_row_h(tree, node),
        _ => DEFAULT_ROW_H,
    }
}

/// ListView 行高（`row_h` I64；缺失/类型错回缺省 [`DEFAULT_ROW_H`]）。
fn list_row_h(tree: &SceneTree, node: NodeId) -> f32 {
    i64_prop(tree, node, "row_h")
        .map(|v| (v as f32).max(1.0))
        .unwrap_or(DEFAULT_ROW_H)
}

/// 取节点 I64 属性（缺失/类型错 = `None`）。
fn i64_prop(tree: &SceneTree, node: NodeId, name: &str) -> Option<i64> {
    match tree.prop(node, name) {
        Some(Value::I64(v)) => Some(*v),
        _ => None,
    }
}

/// 属性串按 '\n' 分隔的行数（空串/缺失/类型错 = 0 行；尾随分隔符按
/// `split` 口径计一个空行）。
fn rows_count(tree: &SceneTree, node: NodeId, prop: &str) -> usize {
    match tree.prop(node, prop) {
        Some(Value::Str(s)) if !s.is_empty() => s.split('\n').count(),
        _ => 0,
    }
}
