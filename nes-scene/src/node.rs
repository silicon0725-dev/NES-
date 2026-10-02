//! 节点类型（enum dispatch）。
//!
//! 为什么是 enum 而不是 `dyn Node` trait object（草案第 18 节第 1 点，已拍板采用 enum）：
//!
//! 1. `NodeData` 可以保持 `Copy` 的邻居、`Clone` 的整体、无 vtable、无堆分配；
//! 2. 遍历时不会因为 trait object 的间接跳转破坏缓存局部性 —— 确定性遍历是 M1 的核心出口；
//! 3. 序列化（M2）可以用一个直接的 `match` 写全，不用处理"未知实现类型"；
//! 4. 类型集合是**封闭**的，正是渲染路径想要的（可在编译期穷尽检查）。
//!
//! 代价：第三方无法在本 crate 外新增节点类型。这在引擎内核里是可接受的 ——
//! 扩展节点应由 `nes-scene-compat`（M5）以注册表方式承接，而不是开放 trait。

/// 节点种类标签。用于继承判定与编辑器分类，与 [`NodeKind`] 一一对应。
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum NodeKindTag {
    /// 基础节点。
    Node,
    /// 2D 变换节点。
    Node2D,
    /// 2D 精灵。
    Sprite2D,
    /// 2D 相机。
    Camera2D,
    /// UI 容器基类。
    Control,
    /// 文本。
    Label,
    /// 脚本宿主（M5 兼容层预留：`registry_key` 是唯一挂载点）。
    Script,
    /// 按钮（S12.1 组件库）：Control 之上加文本与主题槽位引用；
    /// 提取层摊平为同句柄 rect + text。
    Button,
    /// 主题（S12.1 组件库）：八槽位语义色板（纯数据节点，不渲染）。
    Theme,
    /// 单行文本输入框（S12-2 组件库）：Control 之上加提交值/主题槽位；
    /// 草稿与光标是 UiVm 瞬态，不进属性表。
    TextInput,
    /// 滚动容器（S12-3 组件库）：可见后代控件超出自身视口的部分按滚动
    /// 偏移平移/裁剪；滚动偏移是 UiVm 瞬态（`UiStates::scrolls`），
    /// 不进属性表。
    ScrollView,
    /// 列表（S12-3 组件库）：`rows` 属性按 '\n' 分隔行文本，UiVm 据
    /// 行点击回调宿主（选中落账由宿主做，UiVm 零写权）。
    ListView,
    /// 页签（S12-3 组件库）：`tabs` 属性按 '\n' 分隔页签文本，
    /// 活动页下标是宿主属性 `active`（UiVm 不写）。
    Tabs,
}

impl NodeKindTag {
    /// 全部标签，按继承链自上而下排列（属性面板顺序直接用它）。
    pub const ALL: [NodeKindTag; 13] = [
        NodeKindTag::Node,
        NodeKindTag::Node2D,
        NodeKindTag::Sprite2D,
        NodeKindTag::Camera2D,
        NodeKindTag::Control,
        NodeKindTag::Label,
        NodeKindTag::Script,
        NodeKindTag::Button,
        NodeKindTag::Theme,
        NodeKindTag::TextInput,
        NodeKindTag::ScrollView,
        NodeKindTag::ListView,
        NodeKindTag::Tabs,
    ];

    /// 在 [`Self::ALL`] 里的下标。位集与表格化存储要用。
    pub const fn index(self) -> usize {
        match self {
            Self::Node => 0,
            Self::Node2D => 1,
            Self::Sprite2D => 2,
            Self::Camera2D => 3,
            Self::Control => 4,
            Self::Label => 5,
            Self::Script => 6,
            Self::Button => 7,
            Self::Theme => 8,
            Self::TextInput => 9,
            Self::ScrollView => 10,
            Self::ListView => 11,
            Self::Tabs => 12,
        }
    }

    /// 该标签对应的空数据（专有字段取默认值）。
    ///
    /// 反序列化与"新建节点"都从它起步 —— 它是"标签 → 数据"的唯一构造口，
    /// 保证不会出现"标签说 Sprite2D、数据却是 Label"的错配。
    pub const fn kind(self) -> NodeKind {
        match self {
            Self::Node => NodeKind::Node,
            Self::Node2D => NodeKind::Node2D,
            Self::Sprite2D => NodeKind::Sprite2D,
            Self::Camera2D => NodeKind::Camera2D,
            Self::Control => NodeKind::Control,
            Self::Label => NodeKind::Label,
            Self::Script => NodeKind::Script,
            Self::Button => NodeKind::Button,
            Self::Theme => NodeKind::Theme,
            Self::TextInput => NodeKind::TextInput,
            Self::ScrollView => NodeKind::ScrollView,
            Self::ListView => NodeKind::ListView,
            Self::Tabs => NodeKind::Tabs,
        }
    }

    /// 稳定字符串名。用于序列化与日志，**不得**随重构改名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Node => "Node",
            Self::Node2D => "Node2D",
            Self::Sprite2D => "Sprite2D",
            Self::Camera2D => "Camera2D",
            Self::Control => "Control",
            Self::Label => "Label",
            Self::Script => "Script",
            Self::Button => "Button",
            Self::Theme => "Theme",
            Self::TextInput => "TextInput",
            Self::ScrollView => "ScrollView",
            Self::ListView => "ListView",
            Self::Tabs => "Tabs",
        }
    }

    /// 从稳定字符串名还原。
    pub fn from_str_exact(s: &str) -> Option<Self> {
        Some(match s {
            "Node" => Self::Node,
            "Node2D" => Self::Node2D,
            "Sprite2D" => Self::Sprite2D,
            "Camera2D" => Self::Camera2D,
            "Control" => Self::Control,
            "Label" => Self::Label,
            "Script" => Self::Script,
            "Button" => Self::Button,
            "Theme" => Self::Theme,
            "TextInput" => Self::TextInput,
            "ScrollView" => Self::ScrollView,
            "ListView" => Self::ListView,
            "Tabs" => Self::Tabs,
            _ => return None,
        })
    }

    /// 直接基类。`None` 表示继承链顶端。
    pub fn base(self) -> Option<Self> {
        Some(match self {
            Self::Node | Self::Script | Self::Theme => return None,
            Self::Node2D | Self::Control => Self::Node,
            Self::Sprite2D | Self::Camera2D => Self::Node2D,
            Self::Label | Self::Button | Self::TextInput => Self::Control,
            Self::ScrollView | Self::ListView | Self::Tabs => Self::Control,
        })
    }

    /// 继承判定，含自身（`Sprite2D is_a Node2D == true`）。
    pub fn is_a(self, ancestor: Self) -> bool {
        let mut cur = Some(self);
        while let Some(t) = cur {
            if t == ancestor {
                return true;
            }
            cur = t.base();
        }
        false
    }

    /// 从自身到基类的完整链，含自身。
    pub fn chain(self) -> Vec<Self> {
        let mut out = Vec::new();
        let mut cur = Some(self);
        while let Some(t) = cur {
            out.push(t);
            cur = t.base();
        }
        out
    }
}

/// 节点类型：**只表达类型，不承载数据**。
///
/// # 为什么把专有字段搬走（M2 实现层修订第 3 条）
///
/// M1 时 `Sprite2D { texture }` / `Label { text }` 这类"变体带字段"的写法看着很省事，
/// 到 M2 就撞墙了：
///
/// 1. **专有字段没有统一的读写口**。编辑面板、脚本、序列化三条路径要分别 `match`
///    变体去读写，加一个属性就要改三处，漏一处就是静默失效。
/// 2. **属性与 schema 冲突**。一旦 `texture` 同时是变体字段又是 schema 里的属性，
///    就有两份事实来源，热重载与撤销栈必然对不上。
/// 3. **和前向兼容打架**。序列化要能保留引擎不认识的扩展属性，而变体字段是编译期固定的，
///    根本没地方放。
///
/// 所以 M2 起：**一切设计时数据都在 [`crate::props::PropStore`]**，
/// `NodeKind` 只留类型标签 +（将来真正只有运行时才有的）瞬时状态。
/// 这条也顺带把 M5 的兼容层简化了：Scratch 的变量、JS 扩展的字段，
/// 一律是属性表里的键，挂载点仍然只有 `Script` 一个。
///
/// 派生 `Clone` 是给场景复制与编辑器撤销用的；`PartialEq` 是给测试与快照比对用的。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum NodeKind {
    /// 基础节点。
    #[default]
    Node,
    /// 2D 变换节点。
    Node2D,
    /// 2D 精灵。纹理键是属性 `texture`（资源注册表稳定键，M3）。
    Sprite2D,
    /// 2D 相机。缩放是属性 `zoom`。
    Camera2D,
    /// UI 容器。锚点是属性 `anchor`。
    Control,
    /// 文本节点。内容是属性 `text`。
    Label,
    /// 脚本宿主。
    ///
    /// 脚本注册表键是属性 `registry_key`，它是草案第 16 节留下的**唯一挂载点**：
    /// M5 的 JS 扩展、可视化脚本、Scratch 广播监听器都从这里进入，
    /// 不允许在别处另开挂载点，否则兼容层会碎成多处特判。
    Script,
    /// 按钮（S12.1）：专有字段全在属性表（text / 槽位引用），无专有数据。
    Button,
    /// 主题（S12.1）：八槽位色板全在属性表，纯数据节点。
    Theme,
    /// 单行文本输入框（S12-2）：提交值与槽位引用全在属性表；
    /// 草稿/光标是 UiVm 瞬态，不落属性表。
    TextInput,
    /// 滚动容器（S12-3）：滚动步进等全在属性表；滚动偏移是 UiVm 瞬态。
    ScrollView,
    /// 列表（S12-3）：行文本/行高/选中下标全在属性表；行点击经 UiVm
    /// 回调宿主（选中落账由宿主做）。
    ListView,
    /// 页签（S12-3）：页签文本/页签宽/活动页全在属性表。
    Tabs,
}

impl NodeKind {
    /// 种类标签。
    pub fn tag(&self) -> NodeKindTag {
        match self {
            Self::Node => NodeKindTag::Node,
            Self::Node2D => NodeKindTag::Node2D,
            Self::Sprite2D => NodeKindTag::Sprite2D,
            Self::Camera2D => NodeKindTag::Camera2D,
            Self::Control => NodeKindTag::Control,
            Self::Label => NodeKindTag::Label,
            Self::Script => NodeKindTag::Script,
            Self::Button => NodeKindTag::Button,
            Self::Theme => NodeKindTag::Theme,
            Self::TextInput => NodeKindTag::TextInput,
            Self::ScrollView => NodeKindTag::ScrollView,
            Self::ListView => NodeKindTag::ListView,
            Self::Tabs => NodeKindTag::Tabs,
        }
    }

    /// 稳定类型名。
    pub fn type_name(&self) -> &'static str {
        self.tag().as_str()
    }

    /// 是否为该类型的实例（含继承）。
    pub fn is_a(&self, tag: NodeKindTag) -> bool {
        self.tag().is_a(tag)
    }

    /// 该类型是否需要参与 2D 变换计算。
    pub fn uses_transform(&self) -> bool {
        self.tag().is_a(NodeKindTag::Node2D)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inheritance_chain_is_correct() {
        assert!(NodeKindTag::Sprite2D.is_a(NodeKindTag::Node2D));
        assert!(NodeKindTag::Sprite2D.is_a(NodeKindTag::Node));
        assert!(NodeKindTag::Sprite2D.is_a(NodeKindTag::Sprite2D));
        assert!(NodeKindTag::Label.is_a(NodeKindTag::Control));
        assert!(!NodeKindTag::Node2D.is_a(NodeKindTag::Control));
        assert!(!NodeKindTag::Node.is_a(NodeKindTag::Node2D));
    }

    #[test]
    fn chain_is_self_to_base() {
        assert_eq!(
            NodeKindTag::Sprite2D.chain(),
            vec![NodeKindTag::Sprite2D, NodeKindTag::Node2D, NodeKindTag::Node]
        );
        assert_eq!(NodeKindTag::Node.chain(), vec![NodeKindTag::Node]);
    }

    #[test]
    fn name_roundtrip_is_stable() {
        for tag in [
            NodeKindTag::Node,
            NodeKindTag::Node2D,
            NodeKindTag::Sprite2D,
            NodeKindTag::Camera2D,
            NodeKindTag::Control,
            NodeKindTag::Label,
            NodeKindTag::Script,
        ] {
            assert_eq!(NodeKindTag::from_str_exact(tag.as_str()), Some(tag));
        }
        assert_eq!(NodeKindTag::from_str_exact("Barrel"), None);
    }

    #[test]
    fn transform_participation_matches_hierarchy() {
        assert!(NodeKind::Sprite2D.uses_transform());
        assert!(NodeKind::Camera2D.uses_transform());
        assert!(!NodeKind::Label.uses_transform());
        assert!(!NodeKind::Node.uses_transform());
    }
}
