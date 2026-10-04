//! 场景树：存储、结构变更、确定性遍历、变换传播。
//!
//! # 树不变式（任何时刻都必须成立）
//!
//! 1. **父指针与子列表双向一致**：`nodes[c].parent == Some(p)` ⟺ `p.children` 含 `c`。
//! 2. **子列表按 `(order, slot)` 全序排列**。`order` 单调分配，`slot` 是兜底
//!    决断项 —— 因此**不存在"顺序未定义"的兄弟对**，遍历必然可复现。
//! 3. **无环**：`Reparent` 会拒绝把节点挂到自己或自己的后代下。
//! 4. **根唯一**：`root` 的 `parent` 恒为 `None`，且永远 `IN_TREE`。
//! 5. **不在树中的节点不出现于任何 `children` 列表中**。
//!
//! 这些不变式由 [`SceneTree`] 私有方法集中维护。外部只能通过 [`NodeCtx`] 接触树，
//! 而 `NodeCtx` 在类型层面只提供只读树 + 命令缓冲，**无法直接改结构** —— 这是
//! 与 Scratch 扩展生态打交道时最重要的一道防线：脚本作者再怎么写，也写不坏树。
//!
//! # 帧模型
//!
//! ```text
//! tick(delta)
//!   ├─ 1.    apply_pending     结构变更统一落地（上一帧累积的全部 TreeOp）
//!   ├─ 1.5  timer 递减        每节点倒计时（S10-1）
//!   ├─ 1.75 tween 推进        位置补间直写 local（S16；先于一切脚本）
//!   ├─ 2.    enter_tree        自顶向下，仅新入树节点
//!   ├─ 3.    ready             自底向上（逆前序），仅新就绪节点
//!   ├─ 4.    process           自顶向下，全树
//!   └─ 5.    flush_transforms  脏传播 → 世界矩阵
//! ```
//!
//! 回调里发起的结构变更一律进入**下一帧**的 `pending`，因此本帧的遍历序列
//! 在回调开始前就已固定，回调无法把遍历搅乱。属性写入（`SetLocal`）是例外，
//! 它立即生效 —— 因为帧内可见的属性写入是脚本的普遍预期，而它不改变树的形状。

use std::collections::HashMap;

use crate::identity::{Arena, NodeHandle, NodeId};
use crate::node::{NodeKind, NodeKindTag};
use crate::path::{NodePath, PathSeg};
use crate::props::{PropError, PropStore};
use crate::scene_io::InstanceOverride;
use crate::schema::NodeSchema;
use crate::transform::{Affine, Transform2D, Vec2};
use crate::value::Value;

/// 节点状态位。用 `u32` 位集而非多个 `bool`：`NodeData` 要保持紧凑。
pub struct NodeFlags;

impl NodeFlags {
    /// 无标志。
    pub const NONE: u32 = 0;
    /// 已挂到树上（可被遍历）。
    pub const IN_TREE: u32 = 1 << 0;
    /// 已派发过 `enter_tree`。
    pub const ENTERED: u32 = 1 << 1;
    /// 已派发过 `ready`。
    pub const READY: u32 = 1 << 2;
    /// 本地变换已变，世界矩阵待重算。
    pub const DIRTY_XFORM: u32 = 1 << 3;
    /// 该节点的**子树**中存在变换脏节点。
    ///
    /// 这是 `refresh_transforms` 能安全剪枝的前提：没有这个标记的分支
    /// 完全不会被遍历，因此无关子树（如"改了 A、不影响 B"里的 B）不会被重算。
    pub const DIRTY_SUBTREE: u32 = 1 << 4;
}

/// 单个节点的全部数据。
#[derive(Clone, Debug, PartialEq)]
pub struct NodeData {
    /// 类型与专有字段。
    pub kind: NodeKind,
    /// 名字。同一父节点下唯一（重名会自动加数字后缀）。
    pub name: String,
    /// 父节点。
    pub parent: Option<NodeId>,
    /// 子节点，按 `(order, slot)` 排序。
    pub children: Vec<NodeId>,
    /// 本地变换（相对父节点）。
    pub local: Transform2D,
    /// 世界变换缓存。由 [`SceneTree::refresh_transforms`] 维护。
    pub world: Affine,
    /// 兄弟间排序键，全局单调分配。
    pub order: u64,
    /// 设计时属性表。
    ///
    /// **一切设计时数据都在这里**（M2 实现层修订第 3 条）：`kind` 只表类型，
    /// M1 时挂在变体上的 `texture` / `text` / `zoom` 等已迁入本表。
    /// 属性表与树结构是**两条独立的变更通道**：属性写入立即生效，
    /// 结构变更延迟落地 —— 这个区分是撤销栈与热重载都依赖的。
    pub props: PropStore,
    /// 处理模式（草案 §9）。**调度数据**，与 `local` 同级的一等字段
    /// （空间数据不入属性表的同一裁决）；序列化口径见 S6.4 文档遗留。
    pub process_mode: ProcessMode,
    /// 实例级覆盖记录（仅当本节点是绑定了 `sub_scene` 的包装节点时有意义）。
    ///
    /// 记录本身是文件事实（父场景文件拥有它），实例化时应用到展开子树；
    /// 运行时对实例内部节点的直接编辑**不回写**这里（见 S6.8 文档口径）。
    pub overrides: Vec<InstanceOverride>,
    /// [`NodeFlags`] 位集。
    pub flags: u32,
    /// **持久语义身份**（S9-0 契约，S9-1 实现）：128 位 UUID v4（十六进制
    /// 32 字符）。新对象随机生成；旧文件迁移确定性派生（scene_io）。
    /// **内容无关**（name/path/parent/pos/component/asset 不参与生成）；
    /// 正常编辑不变；clone/duplicate 重新生成；delete 后禁止新对象复用
    ///（undo 经 [`SceneTree::add_node_with_uid`] 恢复原身份 —— S9-2）。
    /// 与 runtime NodeId(slot,gen) 严格分层：uid 是语义身份，不参与
    /// arena 执行安全。
    pub uid: Uid,
    /// **每帧倒计时**（S10-1/F-2）：引擎每 tick -1，到 0 停。
    /// 调度数据（同 process_mode 层级，不进属性表）。
    pub timer: u32,
}

/// 持久语义身份（S9-0）：128 位 UUID v4 的十六进制 32 字符形态。
/// 零依赖 —— 随机源用 `std::collections::hash_map::RandomState`
///（进程级随机种子，非加密强度：编辑器会话内唯一性足够；跨会话
/// 冲突由装载期检测兜底）。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Uid(pub [u8; 16]);

impl Uid {
    /// 随机生成（新对象）。
    pub fn new_v4() -> Self {
        // 进程内唯一熵链：计数器 + 双时间源（纳秒 + 性能计数器）+ 代码地址，
        // FNV-1a 混合两条独立链（前后 8 字节）。跨进程/跨机器唯一性由
        // 128 位空间 + 时间项保证（编辑器会话口径足够；装载期冲突检测兜底）。
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let c = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let perf = (c.wrapping_mul(0x9E37_79B9_7F4A_7C15)) ^ (nanos.rotate_left(13));
        let mix = |salt: u64, mut vals: Vec<u64>| -> u64 {
            let mut h = nes_asset::fnv1a64(&salt.to_le_bytes());
            for v in vals.drain(..) {
                h = nes_asset::fnv1a64(&[h.to_le_bytes(), v.to_le_bytes()].concat());
            }
            h
        };
        let a = mix(1, vec![c, nanos, perf]);
        let b = mix(2, vec![perf.rotate_left(29), c.rotate_left(47), nanos.rotate_left(7)]);
        let mut u = [0u8; 16];
        u[..8].copy_from_slice(&a.to_le_bytes());
        u[8..].copy_from_slice(&b.to_le_bytes());
        u[6] = (u[6] & 0x0F) | 0x40;
        u[8] = (u[8] & 0x3F) | 0x80;
        Self(u)
    }

    /// 确定性派生（旧文件迁移专用；内容 = 派生种子，不是身份来源 ——
    /// 与 new_v4 严格双机制，S9-0 v1.1）。
    pub fn derive_legacy(seed: &str) -> Self {
        let h1 = nes_asset::fnv1a64(format!("{seed}|1").as_bytes());
        let h2 = nes_asset::fnv1a64(format!("{seed}|2").as_bytes());
        let mut u = [0u8; 16];
        u[..8].copy_from_slice(&h1.to_le_bytes());
        u[8..].copy_from_slice(&h2.to_le_bytes());
        u[6] = (u[6] & 0x0F) | 0x40;
        u[8] = (u[8] & 0x3F) | 0x80;
        Self(u)
    }

    /// 十六进制 32 字符（序列化形态）。
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 解析十六进制（32 字符）。非法如实报错。
    pub fn from_hex(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("uid 非法（期望 32 位十六进制）：{s}"));
        }
        let mut u = [0u8; 16];
        for i in 0..16 {
            u[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
        }
        Ok(Self(u))
    }

    /// 位形（指纹用）。
    pub fn bits(&self) -> [u8; 16] {
        self.0
    }
}

/// 节点的处理模式（草案 §9）：决定树处于 `paused` 时 `process` 的派发。
///
/// 生效规则：`Inherit` 沿父链取最近的非 `Inherit` 祖先；整条链都 `Inherit`
/// （含根）解析为 `Pausable`（与 Godot 缺省一致）。
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ProcessMode {
    /// 继承父链（缺省）。
    #[default]
    Inherit,
    /// 受暂停影响（缺省行为）。
    Pausable,
    /// 仅在暂停期间派发，且 `delta = 0`（时间冻结，逻辑/结构仍可做）。
    WhenPaused,
    /// 永远派发，`delta` 不受暂停影响（UI、存档点必需）。
    Always,
    /// 从不派发（无论暂停与否）。
    Disabled,
}

impl ProcessMode {
    /// 稳定字符串名（序列化用，**不得**随重构改名）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "Inherit",
            Self::Pausable => "Pausable",
            Self::WhenPaused => "WhenPaused",
            Self::Always => "Always",
            Self::Disabled => "Disabled",
        }
    }

    /// 从稳定字符串名还原。未知值返回 `None`（由解析层如实报语义错误，
    /// 不静默回落 —— 调度语义不是可容忍的前向兼容数据）。
    pub fn from_str_exact(s: &str) -> Option<Self> {
        match s {
            "Inherit" => Some(Self::Inherit),
            "Pausable" => Some(Self::Pausable),
            "WhenPaused" => Some(Self::WhenPaused),
            "Always" => Some(Self::Always),
            "Disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

impl NodeData {
    fn placeholder(name: &str, kind: NodeKind) -> Self {
        let props = NodeSchema::of(kind.tag()).default_store();
        Self {
            kind,
            name: name.to_string(),
            parent: None,
            children: Vec::new(),
            local: Transform2D::IDENTITY,
            world: Affine::IDENTITY,
            order: 0,
            props,
            process_mode: ProcessMode::default(),
            overrides: Vec::new(),
            flags: NodeFlags::DIRTY_XFORM,
            uid: Uid::new_v4(),
            timer: 0,
        }
    }

    /// 是否已挂在树上。
    pub fn is_in_tree(&self) -> bool {
        self.flags & NodeFlags::IN_TREE != 0
    }

    /// 是否已派发 `enter_tree`。
    pub fn is_entered(&self) -> bool {
        self.flags & NodeFlags::ENTERED != 0
    }

    /// 是否已派发 `ready`。
    pub fn is_ready(&self) -> bool {
        self.flags & NodeFlags::READY != 0
    }

    /// 世界矩阵是否待重算。
    pub fn is_transform_dirty(&self) -> bool {
        self.flags & NodeFlags::DIRTY_XFORM != 0
    }
}

/// 结构变更操作。
///
/// `Add` 里的 `node` 必须已由 [`SceneTree::add_node`] 或 [`NodeCtx::spawn_child`]
/// 在 arena 中占好槽位 —— 这样调用方在 `queue` 的当下就能拿到可用的 [`NodeId`]，
/// 而不必等一帧。占位期间该节点的 `IN_TREE` 为假，遍历看不到它。
#[derive(Clone, Debug, PartialEq)]
pub enum TreeOp {
    /// 把已占位的节点挂到父节点下。`at` 为 `None` 时追加到末尾。
    Add {
        /// 待挂载节点。
        node: NodeId,
        /// 目标父节点。
        parent: NodeId,
        /// 插入位置。越界则追加。
        at: Option<usize>,
    },
    /// 移除节点。`keep_children` 为真时把子节点重新挂到被删节点的父上。
    Remove {
        /// 目标节点。
        node: NodeId,
        /// 是否保留子节点。
        keep_children: bool,
    },
    /// 改挂父节点。
    Reparent {
        /// 目标节点。
        node: NodeId,
        /// 新父节点。
        new_parent: NodeId,
        /// 插入位置。
        at: Option<usize>,
    },
    /// 改名。重名时自动加后缀。
    Rename {
        /// 目标节点。
        node: NodeId,
        /// 期望名字。
        name: String,
    },
    /// 在同父下换位。
    Move {
        /// 目标节点。
        node: NodeId,
        /// 新下标。越界则夹到末尾。
        new_index: usize,
    },
}

/// 结构变更结果事件。用于日志、编辑器刷新、以及将来 SignalBus 的上游。
#[derive(Clone, Debug, PartialEq)]
pub enum TreeEvent {
    /// 节点已入树。
    Added {
        /// 节点。
        node: NodeId,
        /// 父节点。
        parent: NodeId,
    },
    /// 节点已出树。
    Removed {
        /// 节点。
        node: NodeId,
        /// 原父节点。
        parent: NodeId,
    },
    /// 节点已改挂。
    Reparented {
        /// 节点。
        node: NodeId,
        /// 原父节点。
        old_parent: NodeId,
        /// 新父节点。
        new_parent: NodeId,
    },
    /// 节点已改名。
    Renamed {
        /// 节点。
        node: NodeId,
        /// 原名。
        old: String,
        /// 新名。
        new: String,
    },
    /// 节点已换位。
    Moved {
        /// 节点。
        node: NodeId,
        /// 原下标。
        from: usize,
        /// 新下标。
        to: usize,
    },
    /// 请求的名字被占用，已自动调整。
    NameAdjusted {
        /// 节点。
        node: NodeId,
        /// 请求名。
        requested: String,
        /// 实际生效名。
        actual: String,
    },
    /// 操作被拒绝（不变式保护）。
    Rejected {
        /// 操作名。
        op: &'static str,
        /// 原因。
        reason: String,
    },
}

impl TreeEvent {
    /// 桥信号的稳定名（`tree/*`，S6.15：TreeEvent 是 SignalBus 的上游 ——
    /// 草案 TreeEvent 文档"用于日志、编辑器刷新、以及将来 SignalBus 的上游"）。
    /// **不得**随重构改名。
    pub const fn signal_name(&self) -> &'static str {
        match self {
            Self::Added { .. } => "tree/added",
            Self::Removed { .. } => "tree/removed",
            Self::Reparented { .. } => "tree/reparented",
            Self::Renamed { .. } => "tree/renamed",
            Self::Moved { .. } => "tree/moved",
            Self::NameAdjusted { .. } => "tree/name_adjusted",
            Self::Rejected { .. } => "tree/rejected",
        }
    }
}

/// 行为代码发起的命令。
///
/// 这是 [`NodeCtx`] 唯一能产生副作用的出口。分成三类：结构变更、属性写入、衍生。
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    /// 结构变更，延迟到下一帧帧首落地。
    Tree(TreeOp),
    /// 本地变换写入，立即生效（帧内可见）。
    SetLocal {
        /// 目标节点。
        node: NodeId,
        /// 新本地变换。
        t: Transform2D,
    },
    /// 请求在自身下新建子节点。
    ///
    /// 草案第 6 节的 `queue(TreeOp)` 对 `Add` 要求调用方先持有 `NodeId`，
    /// 但回调里只有只读树，无法分配 arena 槽位。补这条命令解决。
    Spawn {
        /// 父节点。
        parent: NodeId,
        /// 新节点名。
        name: String,
        /// 新节点类型。
        kind: NodeKind,
    },
    /// 属性写入，立即生效。
    ///
    /// 与 `SetLocal` 同理：属性不改变树形状，帧内可见是脚本的普遍预期。
    SetProp {
        /// 目标节点。
        node: NodeId,
        /// 属性名。
        name: String,
        /// 新值。
        value: Value,
    },
    /// 请求播放一个声音（S13 第 2 期；`play "key"` 语句的编译产物）。
    ///
    /// 通道裁决：树**不认识音频** —— 不解码、不混音、不碰设备；本命令
    /// 只是脚本"出声"意图进入既有 Cmd 流的唯一形态（照 `Emit` 的思路：
    /// 脚本面单一出口，不另开第二通道）。落地见 [`SceneTree::apply_cmd`]：
    /// 键名收进 [`Self::take_played_sounds`] 的取走缓冲，由宿主
    ///（nes-runtime）在 tick 后转交混音器；无人取走即自然蒸发（headless
    /// 不接音频时零成本丢弃，确定性不受影响 —— 缓冲不进语义指纹）。
    PlaySound {
        /// 声音键（资产装载链注册进混音器的键，约定 = 资源路径去扩展名）。
        key: String,
    },
    /// 请求开始播放一个视频（S15；`video_play "key"` 语句的编译产物）。
    ///
    /// 与 [`Cmd::PlaySound`] 同一条纪律：树**不认识视频** —— 不解码、
    /// 不计时、不碰渲染注册表；本命令只是脚本"播视频"意图进入既有
    /// Cmd 流的形态。落地收进 [`Self::take_video_cmds`] 的取走缓冲，
    /// 由宿主（nes-runtime）在 tick 后消费：起播计时 + 有音轨则转交
    /// 混音器。播放状态全在渲染侧 —— **不进树、不进语义指纹**
    ///（headless 消费即弃，确定性不受影响）。
    VideoPlay {
        /// 视频键（资产装载链解析出的键，约定 = 资源路径去扩展名）。
        key: String,
    },
    /// 请求停止播放一个视频（S15；`video_stop "key"` 的编译产物）。
    /// 语义同 [`Cmd::VideoPlay`]：树只收键名，宿主消费（停计时 + 停音轨）。
    VideoStop {
        /// 视频键。
        key: String,
    },
    /// 请求对目标节点发起一次位置补间（S16 第 1 期；`tween_pos "name" x y ms`
    /// 的编译产物）。
    ///
    /// 与 PlaySound/VideoPlay 的"树无法解释才外送"不同：补间登记表本来
    /// 就是树状态 —— 本命令**直接操作登记表**（不走单帧取走缓冲）。
    /// `from` 不随命令携带：落地时（apply 阶段）采样该节点**当前实际
    /// local pos** 作起点 —— 同帧先推进后落命令时，起点含本帧推进，
    /// last-wins 换程不跳变（S16 §1 冻结的采样时机）。
    TweenPos {
        /// 目标节点（命令发射时已按名解析；落地时查无即静默丢弃 ——
        /// 与 `SetLocal` 对死节点同口径）。
        node: NodeId,
        /// 终点（local pos）。
        to: Vec2,
        /// 时长毫秒（<= 0 的请求落地处拒收：非法请求不落地，不编造
        /// "瞬时移动"语义 —— 解析期字面量已报错，这里是运行时兜底）。
        duration_ms: f64,
    },
    /// 请求移除目标节点的位置补间（S16；`tween_stop "name"` 的编译产物）。
    /// 位置停在当前值（登记丢弃，local 不动）。
    TweenStop {
        /// 目标节点。
        node: NodeId,
    },
}

/// 行为代码看到的树句柄：**只读树 + 命令缓冲**。
///
/// 它刻意不提供任何 `&mut SceneTree`：脚本能表达的全部意图都必须经过命令，
/// 命令再统一走帧首落地。因此不会出现"遍历到一半树变了"这类问题。
pub struct NodeCtx<'a> {
    this: NodeId,
    tree: &'a SceneTree,
    cmds: &'a mut Vec<Cmd>,
    signals: &'a mut Vec<Signal>,
}

impl<'a> NodeCtx<'a> {
    /// 当前节点。
    pub fn this(&self) -> NodeId {
        self.this
    }

    /// 当前节点的属性表（只读）。
    pub fn props(&self) -> &'a PropStore {
        &self.node().props
    }

    /// 读属性。
    pub fn prop(&self, name: &str) -> Option<&'a Value> {
        self.node().props.get(name)
    }

    /// 写属性，立即生效。
    ///
    /// 类型不符 / 名字不在 schema 里时**静默丢弃**：回调里没有事件通道，
    /// 而脚本写错属性名是常态，不该让整帧崩掉。编辑器要走
    /// [`SceneTree::set_prop`]，那里会返回 `Result`。
    pub fn set_prop(&mut self, name: &str, value: Value) {
        self.cmds.push(Cmd::SetProp {
            node: self.this,
            name: name.to_string(),
            value,
        });
    }

    /// 只读树。它是 `&'a`，因此可以在回调内自由传给别的只读函数。
    pub fn tree(&self) -> &'a SceneTree {
        self.tree
    }

    /// 当前节点数据。回调期间该节点必然存活（`tick` 按前序快照派发，不在回调中删节点）。
    pub fn node(&self) -> &'a NodeData {
        self.tree
            .get(self.this)
            .expect("NodeCtx 持有的节点在回调期间必须存活")
    }

    /// 当前节点名。
    pub fn name(&self) -> &'a str {
        self.node().name.as_str()
    }

    /// 当前节点类型。
    pub fn kind(&self) -> &'a NodeKind {
        &self.node().kind
    }

    /// 本地变换。
    pub fn local(&self) -> Transform2D {
        self.node().local
    }

    /// 世界变换（上一帧 `flush` 的结果）。
    pub fn world(&self) -> Affine {
        self.node().world
    }

    /// 世界坐标原点。
    pub fn world_position(&self) -> Vec2 {
        let w = self.node().world;
        Vec2::new(w.tx, w.ty)
    }

    /// 父节点。
    pub fn parent(&self) -> Option<NodeId> {
        self.node().parent
    }

    /// 子节点切片。
    pub fn children(&self) -> &'a [NodeId] {
        &self.node().children
    }

    /// 排队一项结构变更，下一帧帧首落地。
    pub fn queue(&mut self, op: TreeOp) {
        self.cmds.push(Cmd::Tree(op));
    }

    /// 请求在自身下新建子节点。返回的 [`NodeId`] 在本帧内即可用于后续命令。
    pub fn spawn_child(&mut self, name: &str, kind: NodeKind) {
        self.cmds.push(Cmd::Spawn {
            parent: self.this,
            name: name.to_string(),
            kind,
        });
    }

    /// 写本地变换，立即生效。
    pub fn set_local(&mut self, t: Transform2D) {
        self.cmds.push(Cmd::SetLocal {
            node: self.this,
            t,
        });
    }

    /// 本地平移增量。
    pub fn translate(&mut self, dx: f32, dy: f32) {
        let mut t = self.local();
        t.pos.x += dx;
        t.pos.y += dy;
        self.set_local(t);
    }

    /// 当前节点的**生效**处理模式（继承解析后）。配合 `tree().paused()`
    /// 供行为代码自省调度态。
    pub fn process_mode(&self) -> ProcessMode {
        self.tree.effective_process_mode(self.this)
    }

    /// 发射一条信号（源自动填当前节点）。入队，帧末泵统一交付。
    pub fn emit(&mut self, name: &str, payload: Value) {
        self.signals.push(Signal {
            src: Some(self.this),
            name: name.to_string(),
            payload,
            event: None,
        });
    }

    /// 请求播放一个声音（S13 第 2 期）：入既有 Cmd 流（[`Cmd::PlaySound`]），
    /// 键名落地到 [`SceneTree::take_played_sounds`] 的取走缓冲，宿主 tick 后
    /// 转交混音器。**不写树**（音频不是树状态）—— 与 `emit` 同一条纪律：
    /// process 入口与信号入口都可发。
    pub fn play_sound(&mut self, key: &str) {
        self.cmds.push(Cmd::PlaySound { key: key.to_string() });
    }

    /// 请求开始播放一个视频（S15）：入既有 Cmd 流（[`Cmd::VideoPlay`]），
    /// 与 [`Self::play_sound`] 同一条纪律 —— 不写树、两入口同权。
    pub fn video_play(&mut self, key: &str) {
        self.cmds.push(Cmd::VideoPlay { key: key.to_string() });
    }

    /// 请求停止播放一个视频（S15）：入既有 Cmd 流（[`Cmd::VideoStop`]）。
    pub fn video_stop(&mut self, key: &str) {
        self.cmds.push(Cmd::VideoStop { key: key.to_string() });
    }

    /// 对任意节点发起一次位置补间（S16 第 1 期）：入既有 Cmd 流
    ///（[`Cmd::TweenPos`]）。补间不写树形状、只推 local —— 与"process
    /// 入口只写自身"的 SetT 纪律不同权：本命令走登记表（树状态），
    /// 两入口同权（照 play/emit 口径）。`from` 在 Cmd 落地时采样。
    pub fn tween_pos(&mut self, node: NodeId, to: Vec2, duration_ms: f64) {
        self.cmds.push(Cmd::TweenPos {
            node,
            to,
            duration_ms,
        });
    }

    /// 移除任意节点的位置补间（S16）：入既有 Cmd 流（[`Cmd::TweenStop`]），
    /// 位置停在当前值。两入口同权。
    pub fn tween_stop(&mut self, node: NodeId) {
        self.cmds.push(Cmd::TweenStop { node });
    }
}

/// 遍历钩子。引擎与测试都通过它观察树。
///
/// 全部方法都有空实现 —— 使用方只覆写关心的那几个。
pub trait SceneObserver {
    /// 结构变更已落地。
    fn on_tree_event(&mut self, _tree: &SceneTree, _ev: &TreeEvent) {}
    /// 节点首次入树（自顶向下）。
    fn on_enter_tree(&mut self, _ctx: &mut NodeCtx<'_>) {}
    /// 节点就绪（自底向上，子先于父）。
    fn on_ready(&mut self, _ctx: &mut NodeCtx<'_>) {}
    /// 每帧处理（自顶向下）。
    fn on_process(&mut self, _ctx: &mut NodeCtx<'_>, _delta: f32) {}
    /// 节点出树。
    fn on_exit_tree(&mut self, _tree: &SceneTree, _node: NodeId) {}
    /// 信号交付（帧末泵，按发射序）。行为代码据此解耦通信：发射方不认识
    /// 接收方，接收方按名字过滤（订阅册属脚本 VM 里程碑，见 S6.14 文档）。
    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, _sig: &Signal) {}
    /// 声明订阅（S6.16）：泵只把命中的信号送进 [`Self::on_signal`]。
    /// 缺省全收；每帧取一次（不是每信号一次）。
    fn signal_filter(&self) -> SignalFilter {
        SignalFilter::All
    }
}

/// 观察者组合（S7.1.3 冻结）：**注册序稳定 Vec**，派发按注册序转发
/// 给全部成员 —— 引擎侧永不引入优先级数值 / HashMap 排序。
///
/// 裁决口径：
/// - **一个节点可以挂多少观察者？** 观察者是**宿主级**不是节点级：
///   每个生命周期/帧回调对全部注册观察者各调一次（注册序）；
/// - **同一节点回调内**，后注册成员看到先注册成员**落地前**的状态
///   （Cmd 批次在整组回调返回后才落地 —— 与单观察者自身的批语义
///   同一条屏障）；
/// - **订阅过滤取并集**：任一成员订阅的广播都会送达组合（送达后
///   各成员在 `on_signal` 里自行忽略不关心的名字）—— 一个成员的
///   过滤器不能静默掐掉另一个成员的邮件；
/// - 泵的统计口径不变：组合对引擎是**一个**观察者（`signals_delivered`
///   按泵交付计，不按成员数放大）。
pub struct Observers {
    inner: Vec<Box<dyn SceneObserver>>,
}

impl Observers {
    /// 空组合（零成员 —— 所有回调空转）。
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    /// 追加一个成员，返回其注册序号（从 0 起）。注册序即派发序，
    /// 运行期不可重排（要换序就重建组合）。
    pub fn push(&mut self, obs: Box<dyn SceneObserver>) -> usize {
        self.inner.push(obs);
        self.inner.len() - 1
    }

    /// 成员数。
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 是否没有成员。
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for Observers {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneObserver for Observers {
    fn on_tree_event(&mut self, tree: &SceneTree, ev: &TreeEvent) {
        for o in &mut self.inner {
            o.on_tree_event(tree, ev);
        }
    }

    fn on_enter_tree(&mut self, ctx: &mut NodeCtx<'_>) {
        for o in &mut self.inner {
            o.on_enter_tree(ctx);
        }
    }

    fn on_ready(&mut self, ctx: &mut NodeCtx<'_>) {
        for o in &mut self.inner {
            o.on_ready(ctx);
        }
    }

    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, delta: f32) {
        for o in &mut self.inner {
            o.on_process(ctx, delta);
        }
    }

    fn on_exit_tree(&mut self, tree: &SceneTree, node: NodeId) {
        for o in &mut self.inner {
            o.on_exit_tree(tree, node);
        }
    }

    fn on_signal(&mut self, ctx: &mut SignalCtx<'_>, sig: &Signal) {
        for o in &mut self.inner {
            o.on_signal(ctx, sig);
        }
    }

    fn signal_filter(&self) -> SignalFilter {
        // 并集：任一成员 All 即 All；否则合并名/前缀集。
        let mut names = Vec::new();
        let mut prefixes = Vec::new();
        for o in &self.inner {
            match o.signal_filter() {
                SignalFilter::All => return SignalFilter::All,
                SignalFilter::Select {
                    names: mut n,
                    prefixes: mut p,
                } => {
                    names.append(&mut n);
                    prefixes.append(&mut p);
                }
            }
        }
        SignalFilter::Select { names, prefixes }
    }
}

/// 双观察者组合（S17.2，**借用形态**）：把同一事件流转发给两个观察者
/// —— 宿主需要同时驱动"游戏观察者（VM 等）"与"扩展信号观察者"时用。
///
/// 与 [`Observers`]（拥有式 Vec 组合）的分工：`Observers` 收
/// `Box<dyn SceneObserver>`（'static），适合装配期固定的成员；本类型收
/// **借用**（`&mut dyn`），适合帧路径里"外部传入的宿主观察者 + 运行时
/// 内部的扩展观察者"这类生命周期不齐的组合 —— 两者并存，能拥有成员的
/// 场合用 `Observers`，借用场合用本类型（功能上是同一裁决的两副面孔）。
///
/// 口径与 [`Observers`] 一致：
/// - **派发序 = 构造序**（先 `first` 后 `second`），无优先级数值；
/// - **订阅过滤取并集**：任一侧 `All` 即 `All`，否则合并名/前缀集 ——
///   一侧的过滤器不能静默掐掉另一侧的邮件；
/// - 组合对引擎是**一个**观察者（`TickStats` 按泵交付计，不按成员数放大）。
pub struct TeeObserver<'a> {
    first: &'a mut dyn SceneObserver,
    second: &'a mut dyn SceneObserver,
}

impl<'a> TeeObserver<'a> {
    /// 由两个观察者引用组装（派发序即参数序）。
    pub fn new(first: &'a mut dyn SceneObserver, second: &'a mut dyn SceneObserver) -> Self {
        Self { first, second }
    }
}

impl SceneObserver for TeeObserver<'_> {
    fn on_tree_event(&mut self, tree: &SceneTree, ev: &TreeEvent) {
        self.first.on_tree_event(tree, ev);
        self.second.on_tree_event(tree, ev);
    }

    fn on_enter_tree(&mut self, ctx: &mut NodeCtx<'_>) {
        self.first.on_enter_tree(ctx);
        self.second.on_enter_tree(ctx);
    }

    fn on_ready(&mut self, ctx: &mut NodeCtx<'_>) {
        self.first.on_ready(ctx);
        self.second.on_ready(ctx);
    }

    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, delta: f32) {
        self.first.on_process(ctx, delta);
        self.second.on_process(ctx, delta);
    }

    fn on_exit_tree(&mut self, tree: &SceneTree, node: NodeId) {
        self.first.on_exit_tree(tree, node);
        self.second.on_exit_tree(tree, node);
    }

    fn on_signal(&mut self, ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.first.on_signal(ctx, sig);
        self.second.on_signal(ctx, sig);
    }

    fn signal_filter(&self) -> SignalFilter {
        // 并集（与 Observers 同一条逻辑）：任一侧 All 即 All，否则合并集合。
        let (mut names, mut prefixes) = match self.first.signal_filter() {
            SignalFilter::All => return SignalFilter::All,
            SignalFilter::Select { names, prefixes } => (names, prefixes),
        };
        match self.second.signal_filter() {
            SignalFilter::All => return SignalFilter::All,
            SignalFilter::Select {
                names: n,
                prefixes: p,
            } => {
                names.extend(n);
                prefixes.extend(p);
            }
        }
        SignalFilter::Select { names, prefixes }
    }
}

/// 一条信号：名字键 + 值载荷 + 发射源（`None` = 宿主/无名源）。
///
/// 载荷是 [`Value`]（值语义，交付即拷贝）。草案 §12：入队、帧末统一 flush、
/// 禁止 emit 中同步递归 —— 泵以工作队列迭代级联（带上限），不违反。
#[derive(Clone, Debug, PartialEq)]
pub struct Signal {
    /// 发射源节点（`NodeCtx::emit` 自动填当前节点；桥信号与宿主预发为 `None`）。
    pub src: Option<NodeId>,
    /// 信号名（接收方按名过滤；桥信号用 `tree/*` 稳定名）。
    pub name: String,
    /// 载荷（用户信号；桥信号为 `Bool(true)` 占位，事实在 `event`）。
    pub payload: Value,
    /// 结构事件原文（**仅 `tree/*` 桥信号**非空 —— NodeId 无法编进 `Value`，
    /// 硬编码槽位/代际是身份谎言；用户信号恒 `None`）。
    pub event: Option<TreeEvent>,
}

/// 信号处理器看到的句柄：**只读树 + 命令缓冲 + 再发射**（与 [`NodeCtx`]
/// 同一形状）。广播交付没有目标节点（`dst = None`）；订阅册路由交付以
/// 连接的目标节点为上下文（`dst = Some`）。
pub struct SignalCtx<'a> {
    dst: Option<NodeId>,
    tree: &'a SceneTree,
    cmds: &'a mut Vec<Cmd>,
    signals: &'a mut Vec<Signal>,
}

impl<'a> SignalCtx<'a> {
    /// 路由交付的目标节点（订阅册连接命中时 `Some`；广播交付 `None`）。
    pub fn dst(&self) -> Option<NodeId> {
        self.dst
    }

    /// 只读树。
    pub fn tree(&self) -> &'a SceneTree {
        self.tree
    }

    /// 再发射一条信号（入队，本帧泵内继续交付 —— 迭代级联，非同步递归）。
    pub fn emit(&mut self, name: &str, payload: Value) {
        self.signals.push(Signal {
            src: None,
            name: name.to_string(),
            payload,
            event: None,
        });
    }

    /// 排队一项结构变更（延迟落地，与 [`NodeCtx::queue`] 同口径）。
    pub fn queue(&mut self, op: TreeOp) {
        self.cmds.push(Cmd::Tree(op));
    }

    /// 写任意节点的本地变换（立即生效）。
    pub fn set_local(&mut self, node: NodeId, t: Transform2D) {
        self.cmds.push(Cmd::SetLocal { node, t });
    }

    /// 写任意节点的属性（立即生效；类型不符/未知键静默忽略 —— 与
    /// [`NodeCtx::set_prop`] 同口径：回调路径没有事件通道，写错不该崩帧）。
    pub fn set_prop(&mut self, node: NodeId, name: &str, value: Value) {
        self.cmds.push(Cmd::SetProp {
            node,
            name: name.to_string(),
            value,
        });
    }

    /// 请求播放一个声音（S13 第 2 期；与 [`NodeCtx::play_sound`] 同一条
    /// Cmd 通道 —— 信号入口照发不误）。
    pub fn play_sound(&mut self, key: &str) {
        self.cmds.push(Cmd::PlaySound { key: key.to_string() });
    }

    /// 请求开始播放一个视频（S15；与 [`NodeCtx::video_play`] 同一条
    /// Cmd 通道 —— 信号入口照发不误）。
    pub fn video_play(&mut self, key: &str) {
        self.cmds.push(Cmd::VideoPlay { key: key.to_string() });
    }

    /// 请求停止播放一个视频（S15；与 [`NodeCtx::video_stop`] 同通道）。
    pub fn video_stop(&mut self, key: &str) {
        self.cmds.push(Cmd::VideoStop { key: key.to_string() });
    }

    /// 对任意节点发起一次位置补间（S16 第 1 期；与 [`NodeCtx::tween_pos`]
    /// 同一条 Cmd 通道 —— 信号入口照发不误）。
    pub fn tween_pos(&mut self, node: NodeId, to: Vec2, duration_ms: f64) {
        self.cmds.push(Cmd::TweenPos {
            node,
            to,
            duration_ms,
        });
    }

    /// 移除任意节点的位置补间（S16；与 [`NodeCtx::tween_stop`] 同通道）。
    pub fn tween_stop(&mut self, node: NodeId) {
        self.cmds.push(Cmd::TweenStop { node });
    }
}

/// 空观察者：宿主没有行为代码时的缺省。
///
/// 生命周期仍照常推进（标志位照置、命令缓冲照走），只是没人监听 ——
/// 信号订阅过滤（S6.16）：观察者声明感兴趣的名字，泵只交付命中项。
///
/// 这是"订阅"在本引擎的诚实形态 —— 声明式、随观察者走、无注册表状态可
/// 悬挂（S6.14"无连接表"的延续）；节点-方法级的 `connect/disconnect`
/// 订阅册仍归脚本 VM 里程碑。未命中的信号**不进处理器**：不消耗交付上限、
/// 不触发级联，计入 [`TickStats::signals_filtered`]。
#[derive(Clone, Debug, PartialEq)]
pub enum SignalFilter {
    /// 全收（[`SceneObserver`] 缺省 —— 既有行为不变）。
    All,
    /// 只收列出的名字（精确）与前缀（如 `tree/`）。
    Select {
        /// 精确名集。
        names: Vec<String>,
        /// 前缀集。
        prefixes: Vec<String>,
    },
}

impl SignalFilter {
    /// 全不收（[`crate::NoObserver`] 用：泵只记账不进回调）。
    pub const NONE: SignalFilter = SignalFilter::Select {
        names: Vec::new(),
        prefixes: Vec::new(),
    };

    /// 精确名订阅。
    pub fn names(names: &[&str]) -> Self {
        SignalFilter::Select {
            names: names.iter().map(|s| s.to_string()).collect(),
            prefixes: Vec::new(),
        }
    }

    /// 前缀订阅（如 `["tree/"]` 收全部桥信号）。
    pub fn prefixes(prefixes: &[&str]) -> Self {
        SignalFilter::Select {
            names: Vec::new(),
            prefixes: prefixes.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// 名字是否命中订阅。
    pub fn matches(&self, name: &str) -> bool {
        match self {
            SignalFilter::All => true,
            SignalFilter::Select { names, prefixes } => {
                names.iter().any(|n| n == name) || prefixes.iter().any(|p| name.starts_with(p.as_str()))
            }
        }
    }
}

/// 运行时的无观察者帧循环用它，保证"不接行为"与"接了行为"走同一条 tick 路径。
#[derive(Copy, Clone, Debug, Default)]
pub struct NoObserver;

/// 订阅册连接句柄（`connect_signal` 返回；计数器分配，不复用）。
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct SignalConnectionId(pub u64);

/// 一条订阅册连接（草案 §12 `connect` 的路由层形态）：名字（精确）+
/// 可选源节点（只订阅来自它的发射）-> 目标节点。
///
/// 命中时观察者收到一次**路由交付**（`SignalCtx::dst() == Some(dst)`，
/// 在广播交付之后按注册序）；方法级分发仍归脚本 VM（`Script` 节点挂载点）。
/// 源或目标节点销毁时连接**自动清理**（tick 阶段 1 修剪 —— 草案
/// "连接表只存 NodeId，节点销毁时自动清理悬挂连接"）。
#[derive(Clone, Debug, PartialEq)]
pub struct SignalConnection {
    /// 连接句柄。
    pub id: SignalConnectionId,
    /// 订阅的信号名（精确匹配）。
    pub name: String,
    /// 只订阅来自该节点的发射（`None` = 任意源，含桥信号的引擎源）。
    pub src: Option<NodeId>,
    /// 路由交付的目标节点（交付上下文）。
    pub dst: NodeId,
    /// 方法级分发（S6.18）：`Some(m)` = 命中时引擎直接调用目标节点的
    /// 处理器表 `m`（[`SceneTree::set_signal_handler`] 注册），**不经观察
    /// 者**；`None` = 观察者交付（S6.17 语义）。草案 connect 的 method 位。
    pub method: Option<String>,
}

/// 节点信号处理器：与观察者回调同一形状的可装箱闭包。
///
/// 这是**方法级分发的落点**：行为代码把处理逻辑挂到具体节点上，连接命中
/// 时引擎直接调用（观察者无需按 dst 分支）。脚本 VM 将来在这里注册解释器
/// 闭包 —— `Script` 节点的 `registry_key` 语义由此承接，无需第二套机制。
pub type SignalHandler = Box<dyn FnMut(&mut SignalCtx<'_>, &Signal)>;

impl SceneObserver for NoObserver {
    /// 无行为代码 = 对任何信号都不感兴趣（泵跳过全部回调，只记账）。
    fn signal_filter(&self) -> SignalFilter {
        SignalFilter::NONE
    }
}

/// 信号泵单帧交付上限（含级联）。超出即丢弃并计入
/// [`TickStats::signals_dropped`] —— runaway 级联是编程错误，引擎的
/// 责任是不挂起帧循环并如实计数。
pub const SIGNAL_DELIVERY_CAP: usize = 1024;

/// 一次 `tick` 的统计。用于性能观测与测试断言。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickStats {
    /// 本次 tick 的帧号。
    pub frame: u64,
    /// 落地的结构变更事件数。
    pub events: usize,
    /// 派发 `enter_tree` 的节点数。
    pub entered: usize,
    /// 派发 `ready` 的节点数。
    pub readied: usize,
    /// 派发 `process` 的节点数。
    pub processed: usize,
    /// **未**派发 `process` 的节点数（暂停跳过 / Disabled）——
    /// 与 `processed` 相加恒等于遍历到的节点数。
    pub process_skipped: usize,
    /// 重算世界矩阵的节点数。
    pub dirty_flushed: usize,
    /// 信号泵交付的信号数（含级联）。
    pub signals_delivered: usize,
    /// 未命中订阅被过滤的信号数（不进处理器、不耗上限）。
    pub signals_filtered: usize,
    /// 其中经订阅册**路由交付**的次数（一条信号可路由多次：每条命中
    /// 连接一次；计入 `signals_delivered` 并单独在此可观测）。
    pub signals_routed: usize,
    /// 因目标节点**生效模式被跳过的处理器调用**数（S7.1 冻结：路由
    /// 交付与 process 同表门控 —— Disabled 永不调用、Pausable 暂停中
    /// 跳过；跳过的调用不进 `signals_routed`）。
    pub handlers_skipped: usize,
    /// 超出交付上限被丢弃的信号数（ runaway 级联的如实计数）。
    pub signals_dropped: usize,
}

/// 场景树。
pub struct SceneTree {
    nodes: Arena<NodeData>,
    root: NodeId,
    pending: Vec<TreeOp>,
    order_seq: u64,
    frame: u64,
    groups: HashMap<String, Vec<NodeId>>,
    /// 待交付信号队列（宿主经 [`SceneTree::emit_signal`] 预发；回调经
    /// [`NodeCtx::emit`]/[`SignalCtx::emit`] 收集到 tick 本地缓冲）。
    /// 帧末泵清空 —— 信号生命周期 = 单帧，跨帧留存请宿主自行存状态。
    signal_queue: Vec<Signal>,
    /// 订阅册（S6.17）：连接按注册序；节点销毁自动清理（阶段 1 修剪）。
    signal_connections: Vec<SignalConnection>,
    /// 连接句柄计数器（只增不减，不复用）。
    next_connection_id: u64,
    /// 节点处理器表（S6.18 方法级分发）：NodeId -> 方法名 -> 闭包。
    /// 与订阅册同一修剪（节点销毁 -> 表项随之清理）。
    signal_handlers: HashMap<NodeId, HashMap<String, SignalHandler>>,
    /// 全局暂停位（草案 §9）。影响 [`ProcessMode::Pausable`]（含 `Inherit`
    /// 解析结果）的 `process` 派发；生命周期与结构变更**不受影响**。
    paused: bool,
    /// 全局时间缩放（草案 §9）。只乘 `delta`，不改遍历次数（确定性优先）。
    /// 写入时钳到 `[0, +∞)`——负时间没有可解释的语义，宁可夹住不放行。
    time_scale: f32,
    /// [`Cmd::PlaySound`] 的落地缓冲（S13 第 2 期）：树不解释音频，只把
    /// 脚本请求的声音键按发射序收在这里，宿主经 [`Self::take_played_sounds`]
    /// 取走转交混音器。与 `pending`/`signal_queue` 同一家法 —— 单帧内
    /// 聚积的副作用缓冲，不进语义指纹（音频不是树状态，取走与否不影响
    /// 结构/属性/局部）；无人取走时仅占内存、不影响任何语义输出。
    played_sounds: Vec<String>,
    /// [`Cmd::VideoPlay`] / [`Cmd::VideoStop`] 的落地缓冲（S15）：与
    /// `played_sounds` 同构 —— 树不解释视频，只把脚本请求的视频键按
    /// **发射序**收在这里（play/stop 混排时序如实保留），宿主经
    /// [`Self::take_video_cmds`] 取走转交渲染侧播放状态机。不进语义
    /// 指纹；无人取走即自然蒸发。
    video_cmds: Vec<VideoCmd>,
    /// 位置补间登记表（S16 第 1 期）：按登记序的 [`Tween`] 列表。
    ///
    /// 与 `played_sounds`/`video_cmds` 的取走缓冲**不同家**：补间是
    /// **游戏可见状态**（每 tick 直写节点 local），登记表本身是树状态
    /// —— 进语义指纹（条件混入：有补间才摺进哈希）、不进序列化
    ///（会话态：保存时进行中的补间丢弃，位置字段已是最新，无损）。
    /// last-wins：同一目标的重复登记前者被替换；目标死亡自动清。
    tweens: Vec<Tween>,
}

/// 视频控制命令（S15）：[`SceneTree::take_video_cmds`] 取走缓冲的元素。
/// 树侧只是键的搬运工，播放语义（计时/换页/音轨）全在宿主渲染侧。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoCmd {
    /// 起播（[`Cmd::VideoPlay`] 的落地形态）。
    Play {
        /// 视频键。
        key: String,
    },
    /// 停播（[`Cmd::VideoStop`] 的落地形态）。
    Stop {
        /// 视频键。
        key: String,
    },
}

/// 一次进行中的**位置补间**（S16 第 1 期）：把 `target` 的本地平移从
/// `from` 线性推向 `to`，时长 `duration_ms`。
///
/// # 语义冻结（S16 §1）
///
/// - **游戏可见状态**：补间登记表是树状态（与音频/视频的"渲染侧、
///   不进指纹"不同）—— 推进发生在 [`SceneTree::tick`] 的专属阶段
///   （结构落地后、`enter` 前），每 tick 直写节点 local（经
///   [`SceneTree::set_local`] 脏标记路径，世界矩阵照常冲洗），并
///   **全程进语义指纹**（同 tick 同轨迹必同结果）；
/// - **last-wins**：同一目标节点的已有位置补间被新补间替换，新起点
///   = 落地时该节点的**当前实际位置**（不跳变）；
/// - `target` 是 [`NodeHandle`]（临时句柄）：结构变更后每 tick resolve，
///   失败即移除 —— 死节点的补间自动清，不悬挂；
/// - 时满（`t >= 1`）落位 `to` 并移除登记 —— 同帧脚本可读到终值
///   （推进阶段在 process 之前）；
/// - **会话态**：不进 RON 往返（保存时进行中的补间丢弃；位置字段
///   已是最新值，无损）。
#[derive(Clone, Debug, PartialEq)]
pub struct Tween {
    /// 目标节点（句柄形态：每 tick 经 arena resolve，失败自动清）。
    pub target: NodeHandle,
    /// 起点（登记落地时该节点的当前 local pos）。
    pub from: Vec2,
    /// 终点。
    pub to: Vec2,
    /// 已推进毫秒数。
    pub elapsed_ms: f64,
    /// 总时长毫秒数（<= 0 的请求在落地处拒收，不会出现在登记表里）。
    pub duration_ms: f64,
}

impl SceneTree {
    /// 建树。根节点固定为 [`NodeKind::Node`]，名字可指定（影响路径首段）。
    pub fn new(root_name: &str) -> Self {
        Self::new_with_kind(root_name, NodeKind::Node)
    }

    /// 建树并指定根节点类型。
    ///
    /// 场景实例化需要它 —— 场景文件的根可以是 `Node2D`、`Control` 等。
    pub fn new_with_kind(root_name: &str, kind: NodeKind) -> Self {
        let props = NodeSchema::of(kind.tag()).default_store();
        let mut nodes = Arena::new();
        let root = nodes.insert(NodeData {
            kind,
            name: root_name.to_string(),
            parent: None,
            children: Vec::new(),
            local: Transform2D::IDENTITY,
            world: Affine::IDENTITY,
            order: 0,
            props,
            process_mode: ProcessMode::default(),
            overrides: Vec::new(),
            uid: Uid::new_v4(),
            timer: 0,
            flags: NodeFlags::IN_TREE | NodeFlags::DIRTY_XFORM,
        });
        Self {
            nodes,
            root,
            pending: Vec::new(),
            order_seq: 0,
            frame: 0,
            groups: HashMap::new(),
            signal_queue: Vec::new(),
            signal_connections: Vec::new(),
            next_connection_id: 0,
            signal_handlers: HashMap::new(),
            paused: false,
            time_scale: 1.0,
            played_sounds: Vec::new(),
            video_cmds: Vec::new(),
            tweens: Vec::new(),
        }
    }

    // ---------- 只读访问 ----------

    /// 根节点。
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// 已推进的帧数。
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// 存活节点数（含未挂树的占位节点）。
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// 是否无节点（恒为假：根节点始终存在）。
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// 身份是否仍有效。
    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains(id)
    }

    /// 节点数据。
    pub fn get(&self, id: NodeId) -> Option<&NodeData> {
        self.nodes.get(id)
    }

    /// 节点名。
    pub fn name(&self, id: NodeId) -> Option<&str> {
        self.nodes.get(id).map(|n| n.name.as_str())
    }

    /// 节点类型。
    pub fn kind(&self, id: NodeId) -> Option<&NodeKind> {
        self.nodes.get(id).map(|n| &n.kind)
    }

    /// 节点类型的标签（不用先解引用 `NodeKind`）。
    pub fn kind_tag(&self, id: NodeId) -> Option<NodeKindTag> {
        self.nodes.get(id).map(|n| n.kind.tag())
    }

    /// 父节点。
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(id).and_then(|n| n.parent)
    }

    /// 子节点。不存在的节点返回空切片。
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.nodes
            .get(id)
            .map(|n| n.children.as_slice())
            .unwrap_or(&[])
    }

    /// 本地变换。
    pub fn local(&self, id: NodeId) -> Option<Transform2D> {
        self.nodes.get(id).map(|n| n.local)
    }

    /// 世界变换缓存。
    pub fn world(&self, id: NodeId) -> Option<Affine> {
        self.nodes.get(id).map(|n| n.world)
    }

    /// 世界坐标原点。
    pub fn world_position(&self, id: NodeId) -> Option<Vec2> {
        self.nodes.get(id).map(|n| {
            let w = n.world;
            Vec2::new(w.tx, w.ty)
        })
    }

    /// 是否已挂树。
    pub fn is_in_tree(&self, id: NodeId) -> bool {
        self.nodes.get(id).map(|n| n.is_in_tree()).unwrap_or(false)
    }

    /// 是否已派发 `enter_tree`。
    pub fn is_entered(&self, id: NodeId) -> bool {
        self.nodes.get(id).map(|n| n.is_entered()).unwrap_or(false)
    }

    /// 是否已派发 `ready`。
    pub fn is_ready(&self, id: NodeId) -> bool {
        self.nodes.get(id).map(|n| n.is_ready()).unwrap_or(false)
    }

    /// 待落地的结构变更数。
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 位置补间登记表（只读视图，登记序）。语义指纹按此采样；宿主/
    /// 编辑器检视同入口。会话态：不进序列化（见 [`Tween`] 文档）。
    pub fn tweens(&self) -> &[Tween] {
        &self.tweens
    }

    // ---------- 遍历 ----------

    /// 前序遍历（自顶向下，兄弟按序）。这是引擎的**确定性遍历序**，全项目以此为准。
    pub fn preorder(&self) -> Vec<NodeId> {
        self.subtree_preorder(self.root)
    }

    /// 某子树的前序遍历。
    pub fn subtree_preorder(&self, node: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        if !self.is_in_tree(node) {
            return out;
        }
        let mut stack = vec![node];
        while let Some(id) = stack.pop() {
            out.push(id);
            if let Some(nd) = self.nodes.get(id) {
                // 逆序压栈 → 出栈即正向顺序
                for &c in nd.children.iter().rev() {
                    stack.push(c);
                }
            }
        }
        out
    }

    /// 某节点的祖先链，自父向上。
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = self.parent(id);
        while let Some(p) = cur {
            out.push(p);
            cur = self.parent(p);
        }
        out
    }

    // ---------- 信号（草案 §12，S6.14） ----------

    /// 宿主预发一条信号（`src = None`）：本帧 tick 的信号泵统一交付。
    pub fn emit_signal(&mut self, name: &str, payload: Value) {
        self.signal_queue.push(Signal {
            src: None,
            name: name.to_string(),
            payload,
            event: None,
        });
    }

    /// 尚未交付的宿主预发信号（泵在每次 tick 帧末清空队列）。
    pub fn pending_signals(&self) -> &[Signal] {
        &self.signal_queue
    }

    // ---------- 订阅册（草案 §12 connect/disconnect，S6.17） ----------

    /// 注册一条订阅连接。`src = None` 订阅任意源（含桥信号的引擎源）。
    /// 名字为空或目标节点不存在时返回 `None`（如实拒绝，不注册哑连接）。
    pub fn connect_signal(
        &mut self,
        name: &str,
        src: Option<NodeId>,
        dst: NodeId,
    ) -> Option<SignalConnectionId> {
        if name.is_empty() || self.nodes.get(dst).is_none() {
            return None;
        }
        let id = SignalConnectionId(self.next_connection_id);
        self.next_connection_id += 1;
        self.signal_connections.push(SignalConnection {
            id,
            name: name.to_string(),
            src,
            dst,
            method: None,
        });
        Some(id)
    }

    /// 注销一条连接：存在并移除返回 `true`，未知句柄返回 `false`。
    pub fn disconnect_signal(&mut self, id: SignalConnectionId) -> bool {
        let before = self.signal_connections.len();
        self.signal_connections.retain(|c| c.id != id);
        self.signal_connections.len() != before
    }

    /// 方法级连接（草案 connect 的 method 位，S6.18）：命中时引擎直接调用
    /// `dst` 节点处理器表里的 `method`（[`Self::set_signal_handler`] 注册），
    /// 不经观察者。目标节点上未注册该方法则该连接静默跳过（接线期缺口
    /// 不崩帧，与 `NodeCtx::set_prop` 同口径 —— 想可见就在处理器表侧对账）。
    pub fn connect_signal_to(
        &mut self,
        name: &str,
        src: Option<NodeId>,
        dst: NodeId,
        method: &str,
    ) -> Option<SignalConnectionId> {
        if name.is_empty() || method.is_empty() || self.nodes.get(dst).is_none() {
            return None;
        }
        let id = SignalConnectionId(self.next_connection_id);
        self.next_connection_id += 1;
        self.signal_connections.push(SignalConnection {
            id,
            name: name.to_string(),
            src,
            dst,
            method: Some(method.to_string()),
        });
        Some(id)
    }

    /// 在节点上注册/替换一个信号处理器（方法级分发）。节点不存在返回
    /// `false`。同名替换（后注册者生效）。
    pub fn set_signal_handler(
        &mut self,
        node: NodeId,
        method: &str,
        handler: SignalHandler,
    ) -> bool {
        if self.nodes.get(node).is_none() || method.is_empty() {
            return false;
        }
        self.signal_handlers
            .entry(node)
            .or_default()
            .insert(method.to_string(), handler);
        true
    }

    /// 移除节点上的一个处理器：存在并移除返回 `true`。
    pub fn remove_signal_handler(&mut self, node: NodeId, method: &str) -> bool {
        match self.signal_handlers.get_mut(&node) {
            Some(map) => map.remove(method).is_some(),
            None => false,
        }
    }

    /// 取出处理器（调用期暂离处理器表，避免与只读树借用冲突）。
    fn take_signal_handler(&mut self, node: NodeId, method: &str) -> Option<SignalHandler> {
        self.signal_handlers.get_mut(&node)?.remove(method)
    }

    /// 归还处理器（take 的逆）。
    fn put_signal_handler(&mut self, node: NodeId, method: &str, handler: SignalHandler) {
        self.signal_handlers
            .entry(node)
            .or_default()
            .insert(method.to_string(), handler);
    }

    /// 订阅册只读视图（注册序；宿主/编辑器检视用）。
    pub fn signal_connections(&self) -> &[SignalConnection] {
        &self.signal_connections
    }

    /// 修剪死连接与死处理器表：源或目标节点已销毁（arena 查无，代际即
    /// 身份）的移除。tick 阶段 1 调用 —— 节点销毁自动清理（草案 §12）。
    fn prune_dead_connections(&mut self) {
        self.signal_connections.retain(|c| {
            let src_alive = c.src.map_or(true, |n| self.nodes.get(n).is_some());
            let dst_alive = self.nodes.get(c.dst).is_some();
            src_alive && dst_alive
        });
        self.signal_handlers
            .retain(|node, _| self.nodes.get(*node).is_some());
    }

    // ---------- 暂停与时间缩放（草案 §9） ----------

    /// 全局暂停位。
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// 设置全局暂停。暂停**不是停止遍历**：结构变更与生命周期照常落地，
    /// 只是 `Pausable`（含 `Inherit` 解析）节点的 `process` 不再派发。
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// 全局时间缩放（缺省 1.0）。
    pub fn time_scale(&self) -> f32 {
        self.time_scale
    }

    /// 设置全局时间缩放（钳到 `[0, +∞)`）。只乘 `delta`，不改遍历次数。
    pub fn set_time_scale(&mut self, scale: f32) {
        self.time_scale = if scale.is_finite() { scale.max(0.0) } else { 1.0 };
    }

    /// 节点自身设置的处理模式（未经继承解析）。
    pub fn process_mode(&self, id: NodeId) -> Option<ProcessMode> {
        self.nodes.get(id).map(|n| n.process_mode)
    }

    /// 写节点的处理模式。节点不存在时静默忽略（与 `set_local` 同口径）。
    pub fn set_process_mode(&mut self, id: NodeId, mode: ProcessMode) {
        if let Some(nd) = self.nodes.get_mut(id) {
            nd.process_mode = mode;
        }
    }

    /// **生效**处理模式：`Inherit` 沿父链取最近的非 `Inherit` 祖先；
    /// 整条链都 `Inherit`（含根、或节点不在树上）解析为 `Pausable`。
    pub fn effective_process_mode(&self, id: NodeId) -> ProcessMode {
        let mut cur = Some(id);
        while let Some(n) = cur {
            let Some(nd) = self.nodes.get(n) else {
                break;
            };
            if nd.process_mode != ProcessMode::Inherit {
                return nd.process_mode;
            }
            cur = nd.parent;
        }
        ProcessMode::Pausable
    }

    // ---------- 实例级覆盖（S6.8） ----------

    /// 节点携带的实例级覆盖记录（通常只有 `sub_scene` 包装节点非空）。
    pub fn instance_overrides(&self, id: NodeId) -> Option<&[InstanceOverride]> {
        self.nodes.get(id).map(|n| n.overrides.as_slice())
    }

    /// 写实例级覆盖记录（序列化回写用；节点不存在静默忽略，与 `set_local`
    /// 同口径）。
    pub fn set_instance_overrides(&mut self, id: NodeId, overrides: Vec<InstanceOverride>) {
        if let Some(nd) = self.nodes.get_mut(id) {
            nd.overrides = overrides;
        }
    }

    /// `anc` 是否为 `node` 的祖先（含自身）。
    pub fn is_ancestor_of(&self, anc: NodeId, node: NodeId) -> bool {
        if anc == node {
            return true;
        }
        let mut cur = self.parent(node);
        while let Some(id) = cur {
            if id == anc {
                return true;
            }
            cur = self.parent(id);
        }
        false
    }

    /// 某个节点到根的深度（根为 0）。
    pub fn depth(&self, id: NodeId) -> usize {
        self.ancestors(id).len()
    }

    /// 前序第一个匹配名字的节点。
    pub fn find_by_name(&self, name: &str) -> Option<NodeId> {
        self.preorder()
            .into_iter()
            .find(|&id| self.nodes.get(id).map(|n| n.name == name).unwrap_or(false))
    }

    /// 某父节点下所有同名子节点。
    pub fn children_named(&self, parent: NodeId, name: &str) -> Vec<NodeId> {
        self.children(parent)
            .iter()
            .copied()
            .filter(|&c| self.nodes.get(c).map(|n| n.name == name).unwrap_or(false))
            .collect()
    }

    // ---------- 路径 ----------

    /// 生成节点路径。与 [`Self::find`] 严格互逆。
    pub fn path_of(&self, id: NodeId) -> Option<NodePath> {
        self.nodes.get(id)?;
        let mut segs = Vec::new();
        let mut cur = id;
        loop {
            let nd = self.nodes.get(cur)?;
            let name = nd.name.clone();
            let parent = nd.parent;
            let mut seg = PathSeg::Named(name.clone());
            if let Some(p) = parent {
                let siblings = self.children_named(p, &name);
                if siblings.len() > 1 {
                    let idx = siblings.iter().position(|&c| c == cur).unwrap_or(0);
                    seg = PathSeg::Indexed(name, idx);
                }
            }
            segs.push(seg);
            match parent {
                Some(p) => cur = p,
                None => break,
            }
        }
        segs.reverse();
        Some(NodePath {
            absolute: false,
            segs,
        })
    }

    /// 按路径查找。首段必须匹配根节点名。
    ///
    /// 该接口是 M5 兼容层（Scratch 的 `getSpriteTargetByName` 之类）的主要入口，
    /// 因此失败时返回 `None` 而不是 panic，也不做模糊匹配。
    pub fn find(&self, path: &NodePath) -> Option<NodeId> {
        let mut iter = path.segs.iter();
        let first = iter.next()?;
        let root_name = self.nodes.get(self.root)?.name.as_str();
        if first.name() != root_name {
            return None;
        }
        let mut cur = self.root;
        for seg in iter {
            let want = seg.name();
            let idx = match seg {
                PathSeg::Named(_) => None,
                PathSeg::Indexed(_, i) => Some(*i),
            };
            let matches: Vec<NodeId> = self
                .children(cur)
                .iter()
                .copied()
                .filter(|&c| self.nodes.get(c).map(|n| n.name == want).unwrap_or(false))
                .collect();
            cur = match idx {
                Some(i) => *matches.get(i)?,
                None => *matches.first()?,
            };
        }
        Some(cur)
    }

    /// 按路径字符串查找的便捷形式。
    pub fn find_str(&self, path: &str) -> Option<NodeId> {
        NodePath::parse(path).ok().and_then(|p| self.find(&p))
    }

    // ---------- 结构变更（排队） ----------

    /// 在 `parent` 下追加一个新节点，立即返回可用的 [`NodeId`]。
    ///
    /// 挂载本身在**下一次** `tick` 帧首完成；在那之前该节点不可遍历。
    /// 如果你需要它立刻可见（例如初始化代码里连续建树），连续调用后统一 `tick` 一次即可。
    pub fn add_node(&mut self, parent: NodeId, name: &str, kind: NodeKind) -> NodeId {
        self.add_node_at(parent, name, kind, None)
    }

    /// 指定 uid 建节点（S9-1）：装载（迁移派生）/ 粘贴 undo 恢复等
    /// 显式身份路径。**同 uid 冲突如实报错**（S9-0 Q1：一个身份一个
    /// 活对象）。
    pub fn add_node_with_uid(
        &mut self,
        parent: NodeId,
        name: &str,
        kind: NodeKind,
        uid: Uid,
    ) -> Result<NodeId, String> {
        if self.find_by_uid(&uid).is_some() {
            return Err(format!("uid 冲突：{} 已有活节点", uid.to_hex()));
        }
        let id = self.add_node(parent, name, kind);
        if let Some(nd) = self.nodes.get_mut(id) {
            nd.uid = uid;
        }
        Ok(id)
    }

    /// 按持久身份查找（S9-0 Q7：uid -> Handle 的查找通道；与
    /// NodeHandle 的 arena resolve 分层，互不替代）。
    pub fn find_by_uid(&self, uid: &Uid) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|(_, nd)| &nd.uid == uid)
            .map(|(id, _)| id)
    }

    /// 显式改写节点 uid（装载/根节点迁移路径；冲突报错）。运行期
    /// 编辑器改名等**不得**走此（身份不可变，S9-0）。
    pub fn set_uid(&mut self, node: NodeId, uid: Uid) -> Result<(), String> {
        let occupied = self
            .nodes
            .iter()
            .any(|(id, nd)| id != node && nd.uid == uid);
        if occupied {
            return Err(format!("uid 冲突：{}", uid.to_hex()));
        }
        if let Some(nd) = self.nodes.get_mut(node) {
            nd.uid = uid;
            Ok(())
        } else {
            Err("节点不存在".to_string())
        }
    }

    /// 读节点持久身份（uid_of(handle) 通道）。
    /// 设置节点倒计时（S10-1/F-2；调度数据，不走属性/Cmd）。
    pub fn set_timer(&mut self, node: NodeId, ticks: u32) {
        if let Some(nd) = self.nodes.get_mut(node) {
            nd.timer = ticks;
        }
    }

    /// 读节点倒计时。
    pub fn timer(&self, node: NodeId) -> Option<u32> {
        self.nodes.get(node).map(|nd| nd.timer)
    }

    pub fn uid_of(&self, node: NodeId) -> Option<Uid> {
        self.nodes.get(node).map(|nd| nd.uid.clone())
    }

    /// 同 [`Self::add_node`]，但指定插入位置。
    pub fn add_node_at(
        &mut self,
        parent: NodeId,
        name: &str,
        kind: NodeKind,
        at: Option<usize>,
    ) -> NodeId {
        let node = self.nodes.insert(NodeData::placeholder(name, kind));
        self.pending.push(TreeOp::Add { node, parent, at });
        node
    }

    /// 排队任意结构变更。
    pub fn queue(&mut self, op: TreeOp) {
        self.pending.push(op);
    }

    /// 移除节点。
    pub fn remove_node(&mut self, node: NodeId, keep_children: bool) {
        self.pending.push(TreeOp::Remove {
            node,
            keep_children,
        });
    }

    /// 改挂父节点。
    pub fn reparent(&mut self, node: NodeId, new_parent: NodeId, at: Option<usize>) {
        self.pending.push(TreeOp::Reparent {
            node,
            new_parent,
            at,
        });
    }

    /// 改名。
    pub fn rename(&mut self, node: NodeId, name: &str) {
        self.pending.push(TreeOp::Rename {
            node,
            name: name.to_string(),
        });
    }

    /// 在同父下换位。
    pub fn move_child(&mut self, node: NodeId, new_index: usize) {
        self.pending.push(TreeOp::Move { node, new_index });
    }

    // ---------- 属性（反射） ----------

    /// 属性表（只读）。
    pub fn props(&self, id: NodeId) -> Option<&PropStore> {
        self.nodes.get(id).map(|n| &n.props)
    }

    /// 读单个属性。
    pub fn prop(&self, id: NodeId, name: &str) -> Option<&Value> {
        self.nodes.get(id).and_then(|n| n.props.get(name))
    }

    /// 该节点的 schema。
    pub fn schema_of(&self, id: NodeId) -> Option<&'static NodeSchema> {
        self.kind_tag(id).map(NodeSchema::of)
    }

    /// 写属性。schema 认识的按 schema 校验（类型不符会按 hint 夹取或拒绝），
    /// schema 不认识的**拒绝**。
    ///
    /// 拒绝是有意的：编辑器面板上拼错属性名，必须当场报错，
    /// 而不是静默写进表里变成永远读不到的垃圾。加载器要保留未知属性时走
    /// [`Self::set_prop_raw`]。
    pub fn set_prop(&mut self, id: NodeId, name: &str, value: Value) -> Result<(), PropError> {
        let schema = self.schema_of(id).ok_or(PropError::NoSuchNode)?;
        // 先校验再落盘：`validate` 可能把值夹取到 hint 范围内，
        // 所以写进去的是**校验后的值**，不是原始输入。
        let checked = schema.validate(name, &value)?;
        let nd = self.nodes.get_mut(id).ok_or(PropError::NoSuchNode)?;
        nd.props.set(name, checked);
        Ok(())
    }

    /// 直接写属性表，不做 schema 校验（反序列化的前向兼容通道）。
    /// 返回属性表的新版本号；节点不存在时返回 0。
    pub fn set_prop_raw(&mut self, id: NodeId, name: &str, value: Value) -> u64 {
        match self.nodes.get_mut(id) {
            Some(nd) => {
                nd.props.set(name, value);
                nd.props.version()
            }
            None => 0,
        }
    }

    /// 移除单个属性（schema 不参与 —— 数据还原的逆向通道）。返回被移
    /// 除的旧值；节点或键不存在返回 `None`。
    ///
    /// S12-9 play-in-editor 的 RESET 用：schema 键出生即满配
    ///（`default_store`），校验通道写不进新键 —— 运行期新增键只可能
    /// 来自 [`Self::set_prop_raw`] 前向兼容通道；快照数据还原只覆盖
    /// 快照里有的键，这种键必须显式摘掉才是诚实的"回到运行前"
    ///（`SubtreeSnapshot::apply_data` 的写回不含删除语义）。
    pub fn remove_prop(&mut self, id: NodeId, name: &str) -> Option<Value> {
        let nd = self.nodes.get_mut(id)?;
        nd.props.remove(name)
    }

    /// 立即写本地变换并标记脏。世界矩阵在下一次 `refresh_transforms` / `tick` 时重算。
    ///
    /// 脏标记分两份：自身 `DIRTY_XFORM`（自己的世界矩阵要重算），
    /// 祖先链 `DIRTY_SUBTREE`（子树里有人要重算）。只标自身的话，冲洗阶段
    /// 无法判断哪条分支需要下探，就只能全树重算。
    pub fn set_local(&mut self, id: NodeId, t: Transform2D) {
        match self.nodes.get_mut(id) {
            Some(nd) => {
                nd.local = t;
                nd.flags |= NodeFlags::DIRTY_XFORM;
            }
            None => return,
        }
        self.mark_subtree_dirty_upwards(id);
    }

    /// 列出节点加入的组。
    pub fn groups_of(&self, id: NodeId) -> Vec<&str> {
        self.groups
            .iter()
            .filter(|(_, members)| members.contains(&id))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// 加组 / 移组。M5 的 Scratch 广播寻址依赖它。
    pub fn set_group(&mut self, id: NodeId, group: &str, member: bool) {
        let entry = self.groups.entry(group.to_string()).or_default();
        let has = entry.contains(&id);
        if member && !has {
            entry.push(id);
        } else if !member && has {
            entry.retain(|&x| x != id);
        }
    }

    /// 组内成员（前序序）。
    pub fn group_members(&self, group: &str) -> Vec<NodeId> {
        let mut ids = self.groups.get(group).cloned().unwrap_or_default();
        // 统一按前序序输出，避免 HashSet/Vec 插入序泄漏成遍历序
        let order = self.preorder();
        ids.sort_by_key(|id| order.iter().position(|x| x == id).unwrap_or(usize::MAX));
        ids
    }

    // ---------- 帧推进 ----------

    /// 落地全部待处理结构变更，返回产生的事件。
    pub fn apply_pending(&mut self) -> Vec<TreeEvent> {
        let ops = std::mem::take(&mut self.pending);
        let mut events = Vec::with_capacity(ops.len());
        for op in ops {
            self.apply_op(op, &mut events);
        }
        events
    }

    /// 重算世界矩阵，返回**实际被重算**的节点数。
    ///
    /// 剪枝依据两条不变式：
    /// 1. 任何 `DIRTY_XFORM` 节点的祖先链上都亮着 `DIRTY_SUBTREE`
    ///    （由 [`Self::mark_subtree_dirty_upwards`] 维护）；
    /// 2. 父的世界矩阵若在本轮被重算，则其全部子节点必然需要重算。
    ///
    /// 于是"子树无脏标记、父也没变"的分支**完全不进入遍历** ——
    /// 与脏节点无关的兄弟子树既不会被访问，也不会被计入返回值。
    ///
    /// 用显式栈而非递归：深场景不应撞上调用栈上限。
    pub fn refresh_transforms(&mut self) -> usize {
        let mut flushed = 0usize;
        let mut stack: Vec<(NodeId, bool)> = vec![(self.root, false)];
        while let Some((id, parent_updated)) = stack.pop() {
            let (self_dirty, subtree_dirty) = match self.nodes.get(id) {
                Some(n) => (
                    n.flags & NodeFlags::DIRTY_XFORM != 0,
                    n.flags & NodeFlags::DIRTY_SUBTREE != 0,
                ),
                None => continue,
            };
            // 只有"自身脏"或"父刚被重算"才真正重算。仅仅被访问到不算数 ——
            // root 是每个非空树的必经节点，若无条件计数，静止树的冲洗数就恒 >= 1。
            let needs = self_dirty || parent_updated;
            if needs {
                let world = match self.parent(id) {
                    None => self
                        .nodes
                        .get(id)
                        .map(|n| n.local.to_affine())
                        .unwrap_or(Affine::IDENTITY),
                    Some(p) => {
                        let pw = self.nodes.get(p).map(|n| n.world).unwrap_or(Affine::IDENTITY);
                        let l = self
                            .nodes
                            .get(id)
                            .map(|n| n.local.to_affine())
                            .unwrap_or(Affine::IDENTITY);
                        pw.mul(&l)
                    }
                };
                if let Some(nd) = self.nodes.get_mut(id) {
                    nd.world = world;
                    nd.flags &= !NodeFlags::DIRTY_XFORM;
                }
                flushed += 1;
            }

            // 下探条件：子树里还有脏，或父刚被重算（子必须跟着重算）。
            if !(subtree_dirty || needs) {
                continue;
            }
            if let Some(nd) = self.nodes.get_mut(id) {
                nd.flags &= !NodeFlags::DIRTY_SUBTREE;
            }
            let kids: Vec<NodeId> = self
                .nodes
                .get(id)
                .map(|n| n.children.clone())
                .unwrap_or_default();
            // 逆序压栈 → 出栈即自左向右（只影响可读性，不影响结果）
            for k in kids.into_iter().rev() {
                stack.push((k, needs));
            }
        }
        flushed
    }

    /// 推进一帧。阶段顺序见模块文档。
    pub fn tick(&mut self, delta: f32, obs: &mut dyn SceneObserver) -> TickStats {
        let mut stats = TickStats {
            frame: self.frame,
            ..TickStats::default()
        };
        // 本帧各回调阶段发射的信号（帧末泵统一交付）。
        let mut emitted: Vec<Signal> = Vec::new();

        // 1. 结构变更落地 + 信号桥（S6.15）
        let events = self.apply_pending();
        stats.events = events.len();
        // 订阅册修剪：本帧结构落地销毁的节点，其连接随之清理（草案 §12）。
        self.prune_dead_connections();
        for ev in &events {
            obs.on_tree_event(&*self, ev);
            // 双通道不互斥：`on_tree_event` 即时回调照旧；同一事件以
            // `tree/*` 桥信号入泵（帧末交付，携带事件原文）。桥信号在泵序
            // 最前（结构落地是帧内最早阶段），src = None（引擎源）。
            emitted.push(Signal {
                src: None,
                name: ev.signal_name().to_string(),
                payload: Value::Bool(true),
                event: Some(ev.clone()),
            });
        }

        // 1.5 timer 递减（S10-1/F-2）：所有 timer > 0 的节点每 tick -1
        //     （到 0 停住）。结构落地后、生命周期/过程前 —— 同帧设 N
        //     即从此帧开始倒数；脚本在 process/信号里读 timer == 0
        //     即"刚到时"。per-entity 定时的引擎侧支撑，消除管理器
        //     平行计时局部（farm 的 g0..g5 形态）。
        let ids: Vec<NodeId> = self.nodes.iter().map(|(id, _)| id).collect();
        for id in ids {
            if let Some(nd) = self.nodes.get_mut(id) {
                if nd.timer > 0 {
                    nd.timer -= 1;
                    // 同步属性表（脚本经 GetProp 可见引擎递减值）。
                    let _ = nd.props.set("timer", Value::I64(nd.timer as i64));
                }
            }
        }

        // 生效 delta（time_scale 只乘 delta，不改遍历次数 —— 与 process
        // 同一口径；补间的"游戏时间"也走这条缩放，确定性与语义都一致）。
        let scaled_delta = delta * self.time_scale;

        // 1.75 位置补间推进（S16 第 1 期，专属阶段：结构落地后、enter/process
        //     之前）。补间是游戏可见状态：每 tick 直写目标节点 local（经
        //     [`Self::set_local`] 脏标记路径，世界矩阵照常在阶段 6 冲洗），
        //     并全程进语义指纹 —— 同 tick 同轨迹必同结果。
        //     - 死目标（arena 查无/代际失效）：resolve 失败即移除（自动清）；
        //     - t >= 1：落位 `to` 并移除登记 —— 本帧 process/信号读到的
        //       就是终值（推进先于脚本）；
        //     - 时间口径：`elapsed += delta * time_scale * 1000`（毫秒）；
        //       v1 冻结面不受暂停门控（补间不是 process 派发，是引擎推进
        //       阶段；暂停交互归后续里程碑 —— S16 文档 §5）。
        if !self.tweens.is_empty() {
            let dt_ms = scaled_delta as f64 * 1000.0;
            let mut i = 0usize;
            while i < self.tweens.len() {
                let (target, from, to, elapsed_ms, duration_ms) = {
                    let tw = &self.tweens[i];
                    (tw.target, tw.from, tw.to, tw.elapsed_ms, tw.duration_ms)
                };
                let id = target.to_id();
                // 死节点补间自动清（NodeHandle resolve 失败）。
                if self.nodes.get(id).is_none() {
                    self.tweens.remove(i);
                    continue;
                }
                let elapsed_ms = elapsed_ms + dt_ms;
                let t = (elapsed_ms / duration_ms).clamp(0.0, 1.0);
                let mut local = self.nodes.get(id).map(|n| n.local).unwrap_or_default();
                if t >= 1.0 {
                    // 时满落位终值并移除（同帧脚本可读终值）。
                    local.pos = to;
                    self.set_local(id, local);
                    self.tweens.remove(i);
                    continue;
                }
                // 位置 = lerp(from, to, t)（f32 域，与 local 同精度）。
                let tf = t as f32;
                local.pos = Vec2::new(
                    from.x + (to.x - from.x) * tf,
                    from.y + (to.y - from.y) * tf,
                );
                self.set_local(id, local);
                self.tweens[i].elapsed_ms = elapsed_ms;
                i += 1;
            }
        }

        // 2. enter_tree（自顶向下）
        for id in self.preorder() {
            let need = self
                .nodes
                .get(id)
                .map(|n| !n.is_entered())
                .unwrap_or(false);
            if !need {
                continue;
            }
            if let Some(nd) = self.nodes.get_mut(id) {
                nd.flags |= NodeFlags::ENTERED;
            }
            let mut cmds: Vec<Cmd> = Vec::new();
            {
                let mut ctx = NodeCtx {
                    this: id,
                    tree: &*self,
                    cmds: &mut cmds,
                    signals: &mut emitted,
                };
                obs.on_enter_tree(&mut ctx);
            }
            for c in cmds {
                self.apply_cmd(c);
            }
            stats.entered += 1;
        }

        // 3. ready（逆前序 ≈ 自底向上：保证子先于父）
        let mut reversed = self.preorder();
        reversed.reverse();
        for id in reversed {
            let need = self
                .nodes
                .get(id)
                .map(|n| n.is_entered() && !n.is_ready())
                .unwrap_or(false);
            if !need {
                continue;
            }
            if let Some(nd) = self.nodes.get_mut(id) {
                nd.flags |= NodeFlags::READY;
            }
            let mut cmds: Vec<Cmd> = Vec::new();
            {
                let mut ctx = NodeCtx {
                    this: id,
                    tree: &*self,
                    cmds: &mut cmds,
                    signals: &mut emitted,
                };
                obs.on_ready(&mut ctx);
            }
            for c in cmds {
                self.apply_cmd(c);
            }
            stats.readied += 1;
        }

        // 4. process（自顶向下）。
        //    遍历序列在这里一次性快照，回调里发起的一切结构变更都进入下一帧 pending，
        //    所以回调不可能把本次遍历搅乱。
        //
        //    暂停/时间缩放（草案 §9）的派发口径，在此冻结：
        //    - `time_scale` 只乘 delta，遍历次数不变（确定性优先）；
        //    - 暂停时 Pausable（含 Inherit 解析）**不派发**；Always 照常派发且
        //      delta 不受暂停影响；WhenPaused 仅暂停时派发、delta = 0（时间冻结，
        //      逻辑与结构变更仍可做）；Disabled 永不派发；
        //    - 生命周期（enter/ready）与结构变更不受暂停影响 —— 暂停期间 UI
        //      不能僵死，结构照常落地。
        let paused = self.paused;
        for id in self.preorder() {
            let dispatch_delta = match self.effective_process_mode(id) {
                ProcessMode::Inherit => Some(scaled_delta), // effective 解析后不会出现；防御口径
                ProcessMode::Pausable => {
                    if paused {
                        None
                    } else {
                        Some(scaled_delta)
                    }
                }
                ProcessMode::Always => Some(scaled_delta),
                ProcessMode::WhenPaused => {
                    if paused {
                        Some(0.0)
                    } else {
                        None
                    }
                }
                ProcessMode::Disabled => None,
            };
            let Some(delta) = dispatch_delta else {
                stats.process_skipped += 1;
                continue;
            };
            let mut cmds: Vec<Cmd> = Vec::new();
            {
                let mut ctx = NodeCtx {
                    this: id,
                    tree: &*self,
                    cmds: &mut cmds,
                    signals: &mut emitted,
                };
                obs.on_process(&mut ctx, delta);
            }
            for c in cmds {
                self.apply_cmd(c);
            }
            stats.processed += 1;
        }

        // 5. 信号泵（草案 §12：入队、帧末统一 flush、禁同步递归）。
        //    交付集 = 宿主预发 + 本帧各回调阶段发射；处理器可再发射（入队，
        //    同泵继续交付 —— 迭代级联，非同步递归）；处理器的 Cmd 立即落地，
        //    紧随其后的变换冲洗看得见 —— **信号触发的变更同帧生效**。
        //    订阅过滤（S6.16）：未命中 [`SceneObserver::signal_filter`] 的信号
        //    不进处理器 —— 不耗上限、不触发级联，计入 signals_filtered。
        //    级联上限 [`SIGNAL_DELIVERY_CAP`]：runaway 时丢弃并如实计数，
        //    不挂起帧循环。
        let filter = obs.signal_filter();
        let mut inflight: Vec<Signal> = std::mem::take(&mut self.signal_queue);
        inflight.append(&mut emitted);
        while !inflight.is_empty() {
            if stats.signals_delivered >= SIGNAL_DELIVERY_CAP {
                stats.signals_dropped += inflight.len();
                inflight.clear();
                break;
            }
            let sig = inflight.remove(0);
            // 订阅过滤（S6.16）只管**广播**：连接是显式接线（S6.17/S6.18），
            // 与观察者的订阅声明无关 —— NoObserver 宿主（如脚本 VM 挂载的
            // 处理器）不能让显式连接静默失效（S6.19 实证修正）。
            let broadcast = filter.matches(&sig.name);
            if !broadcast {
                stats.signals_filtered += 1;
            }
            if broadcast {
                // 广播交付（无目标上下文）。
                let mut cmds: Vec<Cmd> = Vec::new();
                let mut re_emitted: Vec<Signal> = Vec::new();
                {
                    let mut ctx = SignalCtx {
                        dst: None,
                        tree: &*self,
                        cmds: &mut cmds,
                        signals: &mut re_emitted,
                    };
                    obs.on_signal(&mut ctx, &sig);
                }
                for c in cmds {
                    self.apply_cmd(c);
                }
                inflight.append(&mut re_emitted);
                stats.signals_delivered += 1;
            }

            // 路由交付（订阅册，S6.17/S6.18）：按注册序，每条命中连接一次，
            // 目标节点作交付上下文；与广播同守 CAP（每次调用都是真实
            // 处理器）。双路：method=None -> 观察者交付；method=Some(m) ->
            // 引擎直接调用目标节点处理器表的 m（不经观察者；未注册则该连接
            // 静默跳过 —— 接线期缺口不崩帧）。路由不受观察者订阅过滤影响
            //（显式接线，见上方广播处的修正注释）。
            let routed: Vec<(NodeId, Option<String>)> = self
                .signal_connections
                .iter()
                .filter(|c| c.name == sig.name && c.src.map_or(true, |s| Some(s) == sig.src))
                .map(|c| (c.dst, c.method.clone()))
                .collect();
            for (dst, method) in routed {
                if stats.signals_delivered >= SIGNAL_DELIVERY_CAP {
                    stats.signals_dropped += 1; // 该信号剩余路由被截断
                    continue;
                }
                // 生效模式门控（S7.1 冻结）：路由交付与 process 同表 ——
                // **Disabled 永不调用**（行为完全惰性）、**Pausable 暂停中
                // 跳过**（暂停冻结 Pausable 族的时间与事件两者）、
                // Always / WhenPaused 照常。与 S6.4"信号仍然工作"不冲突：
                // 那条口径覆盖的是**宿主广播路径**（obs.on_signal，下方
                // 不受门控）—— 订阅册/处理器表是 S6.17/18 才有的面。
                // 事件不排队：跳过即丢弃，如实计数。
                match self.effective_process_mode(dst) {
                    ProcessMode::Disabled => {
                        stats.handlers_skipped += 1;
                        continue;
                    }
                    ProcessMode::Pausable | ProcessMode::Inherit => {
                        if paused {
                            stats.handlers_skipped += 1;
                            continue;
                        }
                    }
                    ProcessMode::Always | ProcessMode::WhenPaused => {}
                }
                let mut cmds: Vec<Cmd> = Vec::new();
                let mut re_emitted: Vec<Signal> = Vec::new();
                match &method {
                    None => {
                        let mut ctx = SignalCtx {
                            dst: Some(dst),
                            tree: &*self,
                            cmds: &mut cmds,
                            signals: &mut re_emitted,
                        };
                        obs.on_signal(&mut ctx, &sig);
                    }
                    Some(m) => {
                        // take/put：处理器暂离表，避开与只读树借用的别名冲突。
                        if let Some(mut handler) = self.take_signal_handler(dst, m) {
                            let mut ctx = SignalCtx {
                                dst: Some(dst),
                                tree: &*self,
                                cmds: &mut cmds,
                                signals: &mut re_emitted,
                            };
                            handler(&mut ctx, &sig);
                            // ctx 在下一行前离开作用域：树借用结束，处理器归还。
                            let SignalCtx { dst: _, tree: _, cmds: _, signals: _ } = ctx;
                            self.put_signal_handler(dst, m, handler);
                        } else {
                            // 未注册处理器：静默跳过（不计数 —— 没有发生调用）。
                            continue;
                        }
                    }
                }
                for c in cmds {
                    self.apply_cmd(c);
                }
                inflight.append(&mut re_emitted);
                stats.signals_delivered += 1;
                stats.signals_routed += 1;
            }
        }

        // 6. 变换冲洗
        stats.dirty_flushed = self.refresh_transforms();

        self.frame += 1;
        stats
    }

    /// 取走自上次取走以来脚本请求播放的声音键（S13 第 2 期；**发射序**，
    /// 每条 `play` 一个元素，可重复）。宿主（nes-runtime）在 tick 后调它，
    /// 把键转交混音器；headless 不接音频时不调即静默丢弃。
    ///
    /// 缓冲是副作用通道不是状态：不进语义指纹、不进序列化，取走与否
    /// 不影响树的结构/属性/局部 —— 同一轨迹跑两遍指纹逐位相同。
    pub fn take_played_sounds(&mut self) -> Vec<String> {
        std::mem::take(&mut self.played_sounds)
    }

    /// 取走自上次取走以来脚本请求的视频控制命令（S15；**发射序**，
    /// play/stop 混排时序如实保留）。宿主（nes-runtime）在 tick 后调它，
    /// 把命令转交渲染侧播放状态机；headless 不接渲染时不调即静默丢弃。
    ///
    /// 缓冲是副作用通道不是状态：不进语义指纹、不进序列化（与
    /// [`Self::take_played_sounds`] 同一条纪律 —— 同一轨迹跑两遍指纹
    /// 逐位相同）。
    pub fn take_video_cmds(&mut self) -> Vec<VideoCmd> {
        std::mem::take(&mut self.video_cmds)
    }

    // ---------- 内部：不变式维护 ----------

    fn next_order(&mut self) -> u64 {
        self.order_seq = self.order_seq.wrapping_add(1);
        self.order_seq
    }

    fn apply_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Tree(op) => self.pending.push(op),
            Cmd::SetLocal { node, t } => self.set_local(node, t),
            Cmd::SetProp { node, name, value } => {
                let _ = self.set_prop(node, &name, value.clone());
                if name == "timer" {
                    if let Value::I64(t) = value {
                        if let Some(nd) = self.nodes.get_mut(node) {
                            nd.timer = t.max(0) as u32;
                        }
                    }
                }
            }
            Cmd::Spawn { parent, name, kind } => {
                // 只占 arena 槽位，结构变更依旧走 pending —— 保持"帧首统一落地"的纪律。
                let node = self.nodes.insert(NodeData::placeholder(&name, kind));
                self.pending.push(TreeOp::Add {
                    node,
                    parent,
                    at: None,
                });
            }
            Cmd::PlaySound { key } => {
                // 树不认识音频：只收下键名（发射序），宿主 tick 后经
                // take_played_sounds 取走转交混音器；无人取走即自然蒸发
                //（headless 零成本丢弃，确定性不受影响 —— 缓冲不进指纹）。
                self.played_sounds.push(key);
            }
            Cmd::VideoPlay { key } => {
                // 树不认识视频（S15）：与 PlaySound 同一条通道纪律 ——
                // 只收键名，宿主 tick 后经 take_video_cmds 取走转交渲染侧。
                self.video_cmds.push(VideoCmd::Play { key });
            }
            Cmd::VideoStop { key } => {
                self.video_cmds.push(VideoCmd::Stop { key });
            }
            Cmd::TweenPos {
                node,
                to,
                duration_ms,
            } => {
                // 补间登记（S16 第 1 期）：登记表是树状态，Cmd 直接操作 ——
                // 不走单帧取走缓冲（与 PlaySound/VideoPlay 的"树无法解释才
                // 外送"不同）。
                // - `from` 在**落地时**采样（本命令的冻结选择）：apply 发生在
                //   命令发射后的当下（回调 Cmd 即刻落地），此时该节点的 local
                //   含本帧补间推进（推进阶段 1.75 先于 process/信号泵）——
                //   last-wins 换程从当前实际位置起算，不跳变；
                // - 死节点：静默丢弃（与 SetLocal 同口径）；
                // - duration <= 0：拒收（解析期字面量已报错，这里兜底运行时
                //   非法值 —— 非法请求不落地，不编造"瞬时移动"）。
                if duration_ms <= 0.0 || !duration_ms.is_finite() {
                    return;
                }
                let Some(nd) = self.nodes.get(node) else {
                    return;
                };
                let from = Vec2::new(nd.local.pos.x, nd.local.pos.y);
                // last-wins：同目标已有补间先移除，再按登记序追加。
                self.tweens.retain(|tw| tw.target != NodeHandle::of(node));
                self.tweens.push(Tween {
                    target: NodeHandle::of(node),
                    from,
                    to,
                    elapsed_ms: 0.0,
                    duration_ms,
                });
            }
            Cmd::TweenStop { node } => {
                // 停补间：登记丢弃，位置停在当前值（local 不动）。
                self.tweens.retain(|tw| tw.target != NodeHandle::of(node));
            }
        }
    }

    fn apply_op(&mut self, op: TreeOp, events: &mut Vec<TreeEvent>) {
        match op {
            TreeOp::Add { node, parent, at } => {
                if !self.is_in_tree(parent) {
                    self.nodes.remove(node);
                    events.push(TreeEvent::Rejected {
                        op: "Add",
                        reason: format!("父节点 {:?} 不在树中", parent),
                    });
                    return;
                }
                let requested = self
                    .nodes
                    .get(node)
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                let actual = self.unique_name(parent, &requested, Some(node));
                if actual != requested {
                    events.push(TreeEvent::NameAdjusted {
                        node,
                        requested,
                        actual: actual.clone(),
                    });
                }
                if let Some(nd) = self.nodes.get_mut(node) {
                    nd.name = actual;
                }
                self.attach(parent, node, at);
                events.push(TreeEvent::Added { node, parent });
            }

            TreeOp::Remove { node, keep_children } => {
                self.apply_remove(node, keep_children, events);
            }

            TreeOp::Reparent {
                node,
                new_parent,
                at,
            } => {
                let old_parent = match self.nodes.get(node).and_then(|n| n.parent) {
                    Some(p) => p,
                    None => {
                        events.push(TreeEvent::Rejected {
                            op: "Reparent",
                            reason: format!("节点 {:?} 不在树中", node),
                        });
                        return;
                    }
                };
                if node == new_parent || self.is_ancestor_of(node, new_parent) {
                    events.push(TreeEvent::Rejected {
                        op: "Reparent",
                        reason: format!("把 {:?} 挂到自身/后代 {:?} 下会形成环", node, new_parent),
                    });
                    return;
                }
                if !self.is_in_tree(new_parent) {
                    events.push(TreeEvent::Rejected {
                        op: "Reparent",
                        reason: format!("目标父节点 {:?} 不在树中", new_parent),
                    });
                    return;
                }
                self.detach_from_parent(node);
                self.attach(new_parent, node, at);
                events.push(TreeEvent::Reparented {
                    node,
                    old_parent,
                    new_parent,
                });
            }

            TreeOp::Rename { node, name } => {
                let parent = match self.nodes.get(node).and_then(|n| n.parent) {
                    Some(p) => p,
                    None => {
                        events.push(TreeEvent::Rejected {
                            op: "Rename",
                            reason: format!("节点 {:?} 不在树中", node),
                        });
                        return;
                    }
                };
                let actual = self.unique_name(parent, &name, Some(node));
                if actual != name {
                    events.push(TreeEvent::NameAdjusted {
                        node,
                        requested: name,
                        actual: actual.clone(),
                    });
                }
                let old = self
                    .nodes
                    .get(node)
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                if let Some(nd) = self.nodes.get_mut(node) {
                    nd.name = actual.clone();
                }
                events.push(TreeEvent::Renamed {
                    node,
                    old,
                    new: actual,
                });
            }

            TreeOp::Move { node, new_index } => {
                self.apply_move(node, new_index, events);
            }
        }
    }

    fn apply_remove(&mut self, node: NodeId, keep_children: bool, events: &mut Vec<TreeEvent>) {
        let parent = match self.nodes.get(node).and_then(|n| n.parent) {
            Some(p) => p,
            None => {
                events.push(TreeEvent::Rejected {
                    op: "Remove",
                    reason: format!("节点 {:?} 不在树中", node),
                });
                return;
            }
        };

        if keep_children {
            let kids: Vec<NodeId> = self
                .nodes
                .get(node)
                .map(|n| n.children.clone())
                .unwrap_or_default();
            for k in kids {
                self.detach_from_parent(k);
                self.attach(parent, k, None);
                events.push(TreeEvent::Reparented {
                    node: k,
                    old_parent: node,
                    new_parent: parent,
                });
            }
            self.detach_from_parent(node);
            events.push(TreeEvent::Removed { node, parent });
            self.nodes.remove(node);
        } else {
            // 整棵子树移除。自顶向下逐个出树，保证 `Removed` 事件顺序即层级顺序。
            let subtree = self.subtree_preorder(node);
            if subtree.is_empty() {
                events.push(TreeEvent::Rejected {
                    op: "Remove",
                    reason: format!("节点 {:?} 不在树中", node),
                });
                return;
            }
            for n in subtree {
                if let Some(p) = self.nodes.get(n).and_then(|x| x.parent) {
                    self.detach_from_parent(n);
                    events.push(TreeEvent::Removed { node: n, parent: p });
                }
                self.nodes.remove(n);
            }
        }
    }

    fn apply_move(&mut self, node: NodeId, new_index: usize, events: &mut Vec<TreeEvent>) {
        let parent = match self.nodes.get(node).and_then(|n| n.parent) {
            Some(p) => p,
            None => {
                events.push(TreeEvent::Rejected {
                    op: "Move",
                    reason: format!("节点 {:?} 不在树中", node),
                });
                return;
            }
        };
        let from = match self
            .nodes
            .get(parent)
            .and_then(|p| p.children.iter().position(|&c| c == node))
        {
            Some(i) => i,
            None => {
                events.push(TreeEvent::Rejected {
                    op: "Move",
                    reason: format!("节点 {:?} 不在其父的子列表中", node),
                });
                return;
            }
        };
        let len = self.nodes.get(parent).map(|p| p.children.len()).unwrap_or(0);
        if len == 0 {
            return;
        }
        let to = new_index.min(len - 1);
        if to == from {
            return;
        }

        // 1) 落地新顺序
        let new_children: Vec<NodeId> = {
            let mut kids = self
                .nodes
                .get(parent)
                .map(|p| p.children.clone())
                .unwrap_or_default();
            let item = kids.remove(from);
            kids.insert(to, item);
            kids
        };
        // 2) 重写 order 键，使数组顺序即排序结果
        let mut next = self.order_seq;
        for &c in &new_children {
            next = next.wrapping_add(1);
            if let Some(nd) = self.nodes.get_mut(c) {
                nd.order = next;
            }
        }
        self.order_seq = next;
        // 3) 写回。换位只改兄弟顺序、不改父链，因此**不需要**动变换脏标记。
        if let Some(pd) = self.nodes.get_mut(parent) {
            pd.children = new_children;
        }
        events.push(TreeEvent::Moved { node, from, to });
    }

    fn attach(&mut self, parent: NodeId, node: NodeId, at: Option<usize>) {
        let order = self.next_order();
        match self.nodes.get_mut(node) {
            Some(nd) => {
                nd.parent = Some(parent);
                nd.order = order;
                nd.flags |= NodeFlags::IN_TREE | NodeFlags::DIRTY_XFORM;
            }
            None => return,
        }
        if let Some(pd) = self.nodes.get_mut(parent) {
            let len = pd.children.len();
            let idx = match at {
                Some(i) if i <= len => i,
                _ => len,
            };
            pd.children.insert(idx, node);
        }
        self.sort_children(parent);
        // 新挂载节点及其**整棵子树**的世界矩阵全部失效：它的父变了。
        // （`attach` 也被 `keep_children` 的移除路径复用，因此这一条覆盖了
        //  "删父保留子"时把孙节点提升到祖父下导致的世界矩阵变化。）
        self.mark_subtree_transform_dirty(node);
        self.mark_subtree_dirty_upwards(node);
    }

    fn detach_from_parent(&mut self, node: NodeId) {
        let parent = self.nodes.get(node).and_then(|n| n.parent);
        if let Some(p) = parent {
            if let Some(pd) = self.nodes.get_mut(p) {
                if let Some(pos) = pd.children.iter().position(|&c| c == node) {
                    pd.children.remove(pos);
                }
            }
            if let Some(nd) = self.nodes.get_mut(node) {
                nd.parent = None;
            }
        }
    }

    /// 子列表按 `(order, slot)` 排序。`slot` 是兜底决断项，保证全序无并列。
    fn sort_children(&mut self, parent: NodeId) {
        let mut items: Vec<(u64, u32, NodeId)> = Vec::new();
        if let Some(pd) = self.nodes.get(parent) {
            for &c in &pd.children {
                let key = self
                    .nodes
                    .get(c)
                    .map(|n| (n.order, c.slot()))
                    .unwrap_or((u64::MAX, c.slot()));
                items.push((key.0, key.1, c));
            }
        }
        items.sort_unstable();
        if let Some(pd) = self.nodes.get_mut(parent) {
            pd.children = items.into_iter().map(|(_, _, c)| c).collect();
        }
    }

    /// 标记 `start` 的**全部祖先**为 `DIRTY_SUBTREE`：告诉它们"子树里有人要重算"。
    ///
    /// `start` 自身不在标记范围内 —— 它该不该重算由调用方决定
    /// （`set_local` 会自己置 `DIRTY_XFORM`）。这条区分是剪枝能成立的关键。
    fn mark_subtree_dirty_upwards(&mut self, start: NodeId) {
        let mut cur = self.nodes.get(start).and_then(|n| n.parent);
        while let Some(id) = cur {
            match self.nodes.get_mut(id) {
                Some(nd) => {
                    nd.flags |= NodeFlags::DIRTY_SUBTREE;
                    cur = nd.parent;
                }
                None => break,
            }
        }
    }

    /// 把 `node` 及其整棵子树标为 `DIRTY_XFORM`。
    /// 用于子树被挂到新父下：父变换变了，全部后代的世界矩阵都失效。
    fn mark_subtree_transform_dirty(&mut self, node: NodeId) {
        for n in self.subtree_preorder(node) {
            if let Some(nd) = self.nodes.get_mut(n) {
                nd.flags |= NodeFlags::DIRTY_XFORM;
            }
        }
    }

    /// 在 `parent` 下求一个不撞车的名字。
    fn unique_name(&self, parent: NodeId, requested: &str, exclude: Option<NodeId>) -> String {
        let taken = |candidate: &str, this: &Self| -> bool {
            this.nodes
                .get(parent)
                .map(|p| {
                    p.children.iter().any(|&c| {
                        Some(c) != exclude
                            && this.nodes.get(c).map(|n| n.name == candidate).unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        };
        if !taken(requested, self) {
            return requested.to_string();
        }
        let mut i = 2u32;
        loop {
            let cand = format!("{}{}", requested, i);
            if !taken(&cand, self) {
                return cand;
            }
            i += 1;
        }
    }
}

impl Default for SceneTree {
    fn default() -> Self {
        Self::new("root")
    }
}
