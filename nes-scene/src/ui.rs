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
    /// 焦点（S12-2 TextInput 用；Button 不用）。
    pub focused: bool,
}

/// 全部控件节点的瞬态状态表（UiVm 与提取层共享的只读面）。
pub type UiStates = HashMap<NodeId, WidgetState>;

type InputSlot = Rc<RefCell<Option<Rc<dyn crate::script::InputView>>>>;

/// UI 交互状态机（ScriptVm 同构；S12.0 §2.2）。
///
/// 宿主每帧在 tick 与提取之间调 [`UiVm::update`]：读输入快照 →
/// 命中测算 → 更新悬停/按下 → 抬键命中即激活（回调）。提取层经
/// [`UiVm::states_rc`] 读状态做四态着色 —— UI 是投影，状态不回流
/// 属性表。
pub struct UiVm {
    states: Rc<RefCell<UiStates>>,
    input: InputSlot,
    /// 上一帧左键是否按下（边沿检测）。
    prev_left: bool,
    /// 按下时命中的节点（抬键时仍命中才激活 —— 标准 UI 语义）。
    press_target: Option<NodeId>,
    /// 激活钩子（按钮抬键命中；UiVm 零写权，动作由宿主定义）。
    on_activate: Option<Box<dyn FnMut(NodeId)>>,
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
            states: Rc::new(RefCell::new(HashMap::new())),
            input: Rc::new(RefCell::new(None)),
            prev_left: false,
            press_target: None,
            on_activate: None,
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

    /// 状态表共享引用（提取层四态着色的只读面）。
    pub fn states_rc(&self) -> Rc<RefCell<UiStates>> {
        self.states.clone()
    }

    /// 查单节点状态（无记录 = 全 false）。
    pub fn state(&self, node: NodeId) -> WidgetState {
        self.states.borrow().get(&node).copied().unwrap_or_default()
    }

    /// 每帧更新：命中测算 + 悬停/按下状态机 + 激活回调。
    ///
    /// 在 tick 之后、提取之前调用（看到的是当帧终值 —— 与 F-1 信号
    /// 泵同款时序裁决）。无输入视图时为空转（状态全清）。
    pub fn update(&mut self, tree: &SceneTree, viewport: (f32, f32)) {
        // 清死节点状态（悬垂不留 —— 与 ScriptVm 状态清扫同款纪律）。
        self.states
            .borrow_mut()
            .retain(|n, _| tree.contains(*n));

        let Some(input) = self.input.borrow().clone() else {
            return;
        };
        let (mx, my) = input.mouse();
        let left = input.button("left");

        // 前序遍历收集 Button 视口矩形（前序 = 提取层同款确定性序，
        // 后者命中 —— 同一仲裁规则）。
        let mut hit: Option<NodeId> = None;
        let mut stack = vec![tree.root()];
        while let Some(node) = stack.pop() {
            let tag_ok = tree.kind_tag(node) == Some(NodeKindTag::Button);
            let visible = tree
                .prop(node, "visible")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if tag_ok && visible {
                let rect = button_rect(tree, node, viewport);
                let inside = mx >= rect.0
                    && my >= rect.1
                    && mx < rect.0 + rect.2
                    && my < rect.1 + rect.3;
                if inside {
                    hit = Some(node);
                }
            }
            if let Some(data) = tree.get(node) {
                for &child in data.children.iter().rev() {
                    stack.push(child);
                }
            }
        }

        // 边沿检测**先于**状态计算：按下沿记录目标（本帧 pressed 即生效），
        // 抬键沿命中才激活（标准 UI 语义 —— 按下目标 = 抬键命中目标）。
        if left && !self.prev_left {
            self.press_target = hit;
        }
        if !left && self.prev_left {
            if let (Some(t), true) = (self.press_target, hit == self.press_target && hit.is_some())
            {
                if let Some(cb) = self.on_activate.as_mut() {
                    cb(t);
                }
            }
            self.press_target = None;
        }
        self.prev_left = left;

        // 状态机：悬停 = 当前命中；按下 = 按住且目标是自己。
        let mut states = self.states.borrow_mut();
        for st in states.values_mut() {
            st.hover = false;
            st.pressed = false;
        }
        if let Some(n) = hit {
            let st = states.entry(n).or_default();
            st.hover = true;
            st.pressed = left && self.press_target == Some(n);
        }
        drop(states);
    }
}

/// Button 的视口矩形 `(x, y, w, h)`（锚定视口单级解析，S3 契约）。
fn button_rect(tree: &SceneTree, node: NodeId, viewport: (f32, f32)) -> (f32, f32, f32, f32) {
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
