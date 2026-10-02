//! [`RenderExtractor`]：每帧把场景树"提取"成一串属性级渲染命令。
//!
//! # 一帧的全过程
//!
//! ```text
//! tree.refresh_transforms()            世界矩阵冲洗（场景层负责算，本层不重算）
//!        │
//!        ▼
//! scratch = 前序遍历序                 预分配缓冲，语义等同 scene.preorder()
//!        │
//!        ▼  逐节点（确定性顺序）
//!   Camera2D ─▶ set_camera（单槽 last-write-wins；不建渲染物、不进生命周期）
//!        │
//!   Sprite2D 有非空纹理键 / Label 有非空文本 / Control（三类任一准入）
//!        │否 ─▶ 摘条目 + destroy_item（若有）
//!        │是
//!        ├─ 条目在且键没变 ─▶ 复用句柄
//!        ├─ 条目在但键变了 ─▶ destroy_item(旧) + create_item(新)   ← 资源换代
//!        └─ 无条目         ─▶ create_item(新)                      ← 节点新增
//!        │
//!        ▼
//!   set_transform / set_flip / set_z / set_visible          全量属性快照
//!   （Label 追加 set_text、Control 追加 set_rect；Sprite2D 无追加项）
//!        │
//!        ▼
//!   retain_seen()：本帧没见过的条目全部 destroy_item          ← 节点删除 / 子树摘除
//!        │
//!        ▼
//!   server.submit_into(frame, out)                            out 跨帧复用
//! ```
//!
//! # 三条出口准则在本层怎么落地
//!
//! - **变换传播一致**：推的是 `tree.world(node)`（场景层冲洗后的世界矩阵），
//!   本层**不**自己乘父子链。世界矩阵一改，下一帧推送的值跟着改 —— 推出去的值
//!   与场景层的真值逐位相同，一致性没有第二处可漂移。
//! - **z 序稳定**：`z` 取 `Node2D::z_index` 属性，`order` 取前序遍历序号
//!   （即场景层 `NodeData::order`，"第几个被遍历到"）。两者合起来就是契约层的
//!   `DrawKey(z, order, handle)` 全序：**同 z 的先后只由遍历序决定**，
//!   不依赖任何容器迭代顺序。
//! - **节点增删不泄漏 / 不悬垂**：新增节点在下一帧被建条目；被删的节点（或其整棵
//!   子树）不会出现在遍历里，其条目由 `retain_seen` 清扫掉并 `destroy_item`。
//!   句柄由契约层保证不复用，映射表由 `NodeId` 的代际保证不串味。
//!
//! # S3 四项缺口的落点
//!
//! | 缺口 | 本层的动作 | 算式权威在哪 |
//! |---|---|---|
//! | 相机视图矩阵 | [`camera_state_of`] 逐字段解析节点 → `set_camera`（单槽，前序序里最后写入者生效） | 契约 [`Camera2DState::view_matrix`] |
//! | Label 文本布局 | [`label_state_of`] 备齐排版参数（空文本不可渲染）→ `set_text` | 契约 [`LabelState`]；断行与字形度量属 CPU 侧，不下沉后端 |
//! | Control 锚点布局 | [`control_state_of`] 把 `anchor` / `offset` / `size` 摊成四边锚点 + 偏移 → `set_rect` | 契约 [`ControlState::resolve`] |
//! | flip 合成 | 读 `flip_h` / `flip_v` 推 `set_flip`，合成出口是 [`compose_flip`]（`world ∘ scale(±1,±1)`） | 契约 [`Flip::compose`] |
//!
//! 四项的共同纪律：**本层只搬运参数，不重算算式**。相机矩阵、控件矩形、翻转合成、
//! 文本排版四个算式在契约层各自只有一处实现，提取层与后端都调它 ——
//! 这也是 S3 与 `twn-render-stage` 逐帧比对能有唯一参照的前提。
//!
//! # 热路径纪律
//!
//! 遍历缓冲（`order` / `stack`）与输出缓冲都在构造时预分配、跨帧复用：
//! 稳定的树上跑第 N 帧与第 1 帧的容量完全相同（有测试钉住 `ScratchStats`）。
//! 本层额外的内存动作只有映射表的增删，且只在节点/资源发生变化的帧发生。

use std::cell::RefCell;
use std::rc::Rc;
use nes_render_api::{
    Affine2, Camera2DState, ControlState, Flip, FrameInfo, ItemHandle, LabelState, RenderAssetKey,
    RenderCommand, RenderServer, Vec2,
};
use nes_scene::ui::{ThemeColors, UiStates, WidgetState};
use nes_scene::{Affine, NodeId, NodeKindTag, ResId, SceneTree, Value};

use crate::bridge::{affine2_of, flip_of, vec2_of};
use crate::map::NodeItemMap;
use crate::source::RenderKeySource;

/// `Sprite2D` 的纹理属性名（`nes-scene` 的 schema 冻结拼写）。
pub const PROP_TEXTURE: &str = "texture";
/// `Sprite2D` 的水平翻转属性名（已裁决：flip 复用此属性，不建旁路表）。
pub const PROP_FLIP_H: &str = "flip_h";
/// `Sprite2D` 的垂直翻转属性名。
pub const PROP_FLIP_V: &str = "flip_v";
/// `Node2D` 的层号属性名（已裁决：`set_z` 的 `z` 取此属性）。
pub const PROP_Z_INDEX: &str = "z_index";
/// 通用节点的可见性属性名。
pub const PROP_VISIBLE: &str = "visible";
/// `Camera2D` 的缩放属性名（场景层是**标量**，契约层是逐轴 `Vec2`）。
pub const PROP_CAMERA_ZOOM: &str = "zoom";
/// `Camera2D` 的启用属性名（缺失即启用）。
pub const PROP_CAMERA_ACTIVE: &str = "active";
/// `Label` 的文本属性名（空文本不可渲染）。
pub const PROP_LABEL_TEXT: &str = "text";
/// `Label` 的字号属性名（逻辑像素）。
pub const PROP_LABEL_FONT_SIZE: &str = "font_size";
/// `Control` 的锚点属性名（归一化 `Vec2`，`0` = 父左上、`1` = 父右下）。
pub const PROP_CONTROL_ANCHOR: &str = "anchor";
/// `Control` 的偏移属性名（像素）。
pub const PROP_CONTROL_OFFSET: &str = "offset";
/// `Control` 的尺寸属性名（像素；负值照收，不钳制）。
pub const PROP_CONTROL_SIZE: &str = "size";

/// `Label` 字号缺省值（与场景层 schema 的 `font_size` 缺省一致）。
pub const DEFAULT_LABEL_FONT_SIZE: f32 = 16.0;

/// `Control` 尺寸缺省值（与场景层 schema 的 `size` 缺省一致）。
pub const DEFAULT_CONTROL_SIZE: (f32, f32) = (100.0, 100.0);

/// 一帧提取的记账（测试与调优用；不参与渲染决策）。
///
/// 不变式（[`ExtractStats::is_consistent`] 会检查）：
///
/// - `created == fresh + rebound`（每次 `create_item` 都有出处）；
/// - `pushed == fresh + rebound + reused`（每个被提取的渲染物都推了一轮属性）；
/// - `destroyed == rebound + dropped + swept`（每次 `destroy_item` 都有理由）。
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct ExtractStats {
    /// 本帧遍历到的节点数。
    pub nodes_visited: usize,
    /// 本帧推送了属性的渲染物数（= 本帧可渲染的节点数）。
    pub pushed: usize,
    /// `create_item` 次数。
    pub created: usize,
    /// 其中：新节点的首次建条目数。
    pub fresh: usize,
    /// 其中：资源换代导致的 `destroy + create` 数。
    pub rebound: usize,
    /// 复用既有句柄（键未变）的节点数。
    pub reused: usize,
    /// `destroy_item` 次数。
    pub destroyed: usize,
    /// 其中：节点还在树里但**本帧不再可渲染**（资源消失）导致的摘条目数。
    pub dropped: usize,
    /// 其中：本帧未被遍历到（节点被删 / 子树被摘）导致的清扫数。
    pub swept: usize,
    /// 本帧结束时映射表里的存活条目数。
    pub map_len: usize,
}

impl ExtractStats {
    /// 记账是否自洽（见类型文档的三条不变式）。
    pub fn is_consistent(&self) -> bool {
        self.created == self.fresh + self.rebound
            && self.pushed == self.fresh + self.rebound + self.reused
            && self.destroyed == self.rebound + self.dropped + self.swept
    }
}

/// 提取器的预分配缓冲统计（"不每帧分配"的可观测出口）。
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct ScratchStats {
    /// 遍历序缓冲当前容量（`Vec<NodeId>`）。
    pub order_capacity: usize,
    /// 前序遍历辅助栈当前容量（`Vec<NodeId>`）。
    pub stack_capacity: usize,
}

/// 每帧提取器：持有 `NodeId ↔ ItemHandle` 映射表与遍历缓冲。
///
/// 它**不持有** `SceneTree`、也不持有 `RenderServer`：两者每帧作为参数传入
/// （借用短、不存在两套生命周期打结的问题）；提取器只拥有"身份映射"这件
/// 必须跨帧延续的状态。
#[derive(Clone, Debug, Default)]
pub struct RenderExtractor {
    map: NodeItemMap,
    /// 遍历序缓冲（跨帧复用；语义 = 前序遍历）。
    order: Vec<NodeId>,
    /// 前序遍历辅助栈（跨帧复用；不递归，避免深树爆栈）。
    stack: Vec<NodeId>,
    /// 提取帧序号（从 0 起，每帧 `+1`，只用于"本帧是否见过"的标记与诊断）。
    frame_seq: u64,
    /// UI 瞬态状态共享面（S12.1 四态着色的只读来源；None = 无 UI）。
    ui: Option<Rc<RefCell<UiStates>>>,
}

impl RenderExtractor {
    /// 新建（无映射、无缓冲占用）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前身份映射表（只读）。
    pub fn map(&self) -> &NodeItemMap {
        &self.map
    }

    /// 已提取的帧数。
    pub fn frames(&self) -> u64 {
        self.frame_seq
    }

    /// 预分配缓冲统计。
    pub fn scratch(&self) -> ScratchStats {
        ScratchStats {
            order_capacity: self.order.capacity(),
            stack_capacity: self.stack.capacity(),
        }
    }

    /// 某节点当前挂的句柄。
    pub fn handle_of(&self, node: NodeId) -> Option<ItemHandle> {
        self.map.handle_of(node)
    }

    /// 挂接 UI 瞬态状态表（宿主装配时一次；每帧提取读取当前值做四态着色）。
    pub fn attach_ui(&mut self, states: Rc<RefCell<UiStates>>) {
        self.ui = Some(states);
    }

    /// **提取一帧**：遍历 → 属性级推送 → 清扫 → `submit_into`。
    ///
    /// - `tree`：需要 `&mut` 只为了冲洗世界矩阵缓存（`refresh_transforms`）；
    ///   本层不修改树的结构与属性；
    /// - `source`：资源键来源（生产用 `ResourceTable`，测试可用替身）；
    /// - `server`：契约层 [`RenderServer`]（生产用真实后端，测试用
    ///   [`NullRenderServer`](nes_render_api::NullRenderServer)）；
    /// - `out`：输出缓冲，由 `submit_into` 先清空再写满本帧命令（跨帧复用，
    ///   不要在帧间读它的旧内容）。
    ///
    /// # 遍历顺序
    ///
    /// 前序（父在子前），兄弟按 `NodeId` 升序（场景层 `Arena` 的槽位序），
    /// 与 `SceneTree::preorder()` 语义一致 —— 顺序由**结构**决定，与哈希、
    /// 插入时刻、内存布局均无关，因此跨进程、跨帧可复现。
    pub fn extract_into(
        &mut self,
        tree: &mut SceneTree,
        source: &dyn RenderKeySource,
        server: &mut dyn RenderServer,
        frame: &FrameInfo,
        out: &mut Vec<RenderCommand>,
    ) -> ExtractStats {
        // 世界矩阵必须在提取前冲洗：本层推送的是 `world` 缓存，
        // 不做 `local` 的临时换算 —— "传播一致"由场景层的唯一权威算式负责。
        let flushed = tree.refresh_transforms();
        debug_assert!(flushed <= tree.len(), "冲洗计数不应超过节点数");

        self.frame_seq = self.frame_seq.wrapping_add(1);
        let stamp = self.frame_seq;
        let mut stats = ExtractStats::default();

        // 遍历序写进预分配缓冲；取出后本帧不再借用 `self.order`，
        // 循环体内即可自由改映射表（缓冲在末尾归还，全程零分配）。
        self.collect_order(tree);
        let order_nodes = std::mem::take(&mut self.order);

        // 主题解析（S12.1）：前序序里**最后**一个 Theme 节点生效
        //（与相机单槽 last-write-wins 同款仲裁，Q4 裁决）；无主题
        // 节点用缺省深色兜底。槽位引用都对着这一份解析。
        let mut theme = ThemeColors::DEFAULT_DARK;
        for &node in order_nodes.iter() {
            if tree.kind_tag(node) == Some(NodeKindTag::Theme) {
                theme = ThemeColors::from_tree(tree, node);
            }
        }
        let ui = self.ui.as_ref().map(|u| u.borrow());

        for &node in order_nodes.iter() {
            stats.nodes_visited += 1;

            // 相机：**不**建渲染物，只更新契约层的单槽相机（`set_camera`）。
            // 遍历序是确定性的，因此"多台相机谁生效"也是确定的：前序序里
            // 最后写入的那台 —— 已裁决的 last-write-wins，不需要额外仲裁逻辑。
            // 相机状态照实推送（`active=false` 也推），"要不要用"由契约层的
            // `view_matrix()` 决定，提取层不做二次判断。
            if is_camera(tree, node) {
                server.set_camera(&camera_state_of(tree, node, frame.viewport));
            }

            // 准入条件：Sprite2D 有非空纹理键 / Label 有非空文本 / Control 恒准入。
            // 非渲染节点（Node / Node2D 容器 / Camera2D / Script …，以及纹理
            // 未绑定的 Sprite2D、空文本的 Label）一律跳过。
            let Some(admission) = admit(tree, node, source, &theme, ui.as_deref()) else {
                // 本帧不可渲染：若上一帧建过条目，必须销毁，否则留下悬垂渲染物。
                // 判据是"本帧不再可渲染"而不是"资源消失了"：节点被改成非渲染
                // 类型、纹理被解绑、文本被清空、资源被回收，走的是同一条路径。
                if let Some(slot) = self.map.remove(node) {
                    server.destroy_item(slot.handle);
                    stats.destroyed += 1;
                    stats.dropped += 1;
                }
                continue;
            };
            let key = admission.key();

            // 生命周期：复用 / 换代重建 / 首次创建。
            let existing = self.map.get(node).map(|slot| (slot.handle, slot.key));
            let handle = match existing {
                Some((handle, old_key)) if old_key == key => {
                    self.map.mark_seen(node, stamp);
                    stats.reused += 1;
                    handle
                }
                Some((handle, _)) => {
                    // 资源换代（或重新绑到了别的资源）：旧句柄必须显式销毁，
                    // 新资源建新渲染物。旧句柄此后永不分配（契约层保证）。
                    server.destroy_item(handle);
                    stats.destroyed += 1;
                    stats.rebound += 1;
                    let fresh = server.create_item(key);
                    stats.created += 1;
                    self.map.insert(node, fresh, key, stamp);
                    fresh
                }
                None => {
                    let fresh = server.create_item(key);
                    stats.created += 1;
                    stats.fresh += 1;
                    self.map.insert(node, fresh, key, stamp);
                    fresh
                }
            };

            // 属性级推送：先推四类通用项，顺序与契约层 `RenderItem::apply_item`
            // 完全一致（transform → flip → z → visible）—— 这样"逐项 set"出来的
            // 快照就等于"照 `RenderItem` 整项 apply"的结果，逐帧比对才有同一个
            // 参照物。再推类型专属项（Label 的文本 / Control 的布局），它们不在
            // `RenderItem` 里（那是精灵的模型），因此排在通用项之后。
            let world = tree.world(node).unwrap_or(Affine::IDENTITY);
            let flip_h = bool_prop(tree, node, PROP_FLIP_H, false);
            let flip_v = bool_prop(tree, node, PROP_FLIP_V, false);
            let z = z_of(tree, node);
            // `order` = 确定性前序序号（已裁决：不为场景层加字段）。
            let node_order = tree.get(node).map_or(0, |data| data.order);
            let visible = bool_prop(tree, node, PROP_VISIBLE, true);

            server.set_transform(handle, affine2_of(world));
            server.set_flip(handle, flip_of(flip_h, flip_v));
            server.set_z(handle, z, node_order);
            server.set_visible(handle, visible);
            // 类型专属属性：Label 的文本状态 / Control 的锚点状态。
            // 没有这一层，Label 就会以"有渲染物但没文字"的空壳上屏，
            // Control 则连尺寸都没有 —— 这正是 S3 要补的两项缺口。
            match &admission {
                Admission::Sprite(_) => {}
                Admission::Label(_, text) => server.set_text(handle, text),
                Admission::Control(_, layout) => server.set_rect(handle, layout),
                // 按钮摊平（S12.0 §2.1）：单节点单渲染物，rect + text
                // 同句柄双推 —— 消费者侧先画矩形后画字形。
                Admission::Button(_, layout, text) => {
                    server.set_rect(handle, layout);
                    server.set_text(handle, text);
                }
            }
            stats.pushed += 1;
        }

        // 归还遍历缓冲（容量延续到下一帧）。
        self.order = order_nodes;

        // 清扫：本帧未被遍历到的条目 —— 节点被删、或整棵子树被摘下来。
        // 没有这一步，"删节点"就会把渲染物永久留在后端里（泄漏），
        // 而映射表与后端的句柄集合也就此分叉（悬垂）。
        stats.swept = self
            .map
            .retain_seen(stamp, |handle| server.destroy_item(handle));
        stats.destroyed += stats.swept;

        // 输出：契约层负责"先清空 out"与"按 DrawKey 升序输出全量快照"。
        server.submit_into(frame, out);

        stats.map_len = self.map.len();
        debug_assert!(stats.is_consistent(), "提取记账不自洽：{stats:?}");
        stats
    }

    /// 取确定性前序遍历序（父在子前、兄弟按槽位升序），写进预分配缓冲。
    ///
    /// 用**显式栈的迭代**实现而不是递归：树深不受调用栈限制，且压栈顺序
    /// 只由 `NodeId` 升序决定 —— 顺序不依赖任何哈希或插入时刻。
    fn collect_order(&mut self, tree: &SceneTree) {
        self.order.clear();
        self.stack.clear();
        self.stack.push(tree.root());
        while let Some(node) = self.stack.pop() {
            self.order.push(node);
            if let Some(data) = tree.get(node) {
                // 逆序压栈 ⇒ 出栈时即升序，且每个节点都排在它的子节点之前。
                for &child in data.children.iter().rev() {
                    self.stack.push(child);
                }
            }
        }
    }
}

/// 节点是不是 `Camera2D`（相机走独立通道，不参与渲染物生命周期）。
fn is_camera(tree: &SceneTree, node: NodeId) -> bool {
    matches!(tree.kind_tag(node), Some(tag) if tag.is_a(NodeKindTag::Camera2D))
}

// ---------------------------------------------------------------- 准入与状态解析

/// 一个节点这一帧的准入判定，以及与本次准入绑定的类型专属载荷。
///
/// 它同时回答两件事：**上不上屏**（`None` = 本帧不生成渲染物，映射表里若有过
/// 条目就该销毁）与**以什么身份上屏**（`Sprite2D` 取纹理资源键；`Label` /
/// `Control` 的可迭代资源池是空的，身份由"路径 + 节点类型名"确定性导出）。
/// 载荷在这里一次解析完，推送阶段直接原样交给后端，不重复读属性。
enum Admission {
    /// 精灵：键来自纹理资源槽位；无类型专属推送项（变换 / flip 走通用路径）。
    Sprite(RenderAssetKey),
    /// 文字：键由节点身份派生；载荷是备齐的排版参数。
    Label(RenderAssetKey, LabelState),
    /// 控件：键由节点身份派生；载荷是解析好的布局状态。
    Control(RenderAssetKey, ControlState),
    /// 按钮（S12.1）：同句柄摊平 rect + text —— 消费者已支持一物多
    /// 实例（Label 一字形一四边形），按钮是其"矩形 + 文字"组合形态。
    Button(RenderAssetKey, ControlState, LabelState),
}

impl Admission {
    /// 本类渲染物在契约层的身份键。
    fn key(&self) -> RenderAssetKey {
        match self {
            Admission::Sprite(key) => *key,
            Admission::Label(key, _) => *key,
            Admission::Control(key, _) => *key,
            Admission::Button(key, _, _) => *key,
        }
    }
}

/// 非资源类渲染物的**命名空间标记**（占槽位空间的最高位）。
///
/// 资源类渲染物的键来自资源 arena（`AssetKey` 的 `slot` 从 0 起单调增长），
/// 非资源类没有资源可依附，只能由**节点身份**派生（`NodeId::to_bits`，编码与
/// `RenderAssetKey` 同构）。两套 arena 各编各的号，位编码直接透传就会让
/// "节点 #5" 与 "纹理 #5" 压成同一个键 —— 后端按键缓存资源时会把两者当成
/// 同一个东西。留一位当命名空间，两端就永远不会撞上。
const NON_RESOURCE_KEY_TAG: u32 = 0x8000_0000;

/// 非资源类渲染物（`Label` / `Control`）的身份键：节点身份 + 命名空间标记。
///
/// 键必须**稳定**（后端按键缓存字形 / 布局产物）且**非空**（空键在契约层
/// 等同未绑定）。节点身份天然满足稳定：`(slot, gen)` 一经删除永不复用，
/// 因此"删掉文本节点再建一个"不会命中旧缓存。取 `gen` 参与编码还顺带挡住
/// 了槽位复用 —— 与契约层"句柄永不复用"是同一条纪律。
///
/// 一个节点只有一种类型，所以键里不必再区分 Label / Control：节点身份已经
/// 唯一确定了是哪一个渲染物。
fn node_key(node: NodeId) -> RenderAssetKey {
    debug_assert!(
        node.slot() & NON_RESOURCE_KEY_TAG == 0,
        "节点槽位越界，会与命名空间标记冲突"
    );
    RenderAssetKey::from_parts(NON_RESOURCE_KEY_TAG | node.slot(), node.generation())
}

/// 准入判定：一个节点这一帧的渲染身份与类型专属载荷（不可渲染 → `None`）。
///
/// 三类可渲染物各自的准入条件：
///
/// - `Sprite2D`：纹理槽位解析出**非空**资源键（未绑定 / 资源被回收都不算）；
/// - `Label`：文本**非空**（空文本没有可显示内容，不该占一个渲染物）；
/// - `Control`：恒准入（控件是布局容器，本身就有尺寸，不依赖任何资源）。
fn admit(
    tree: &SceneTree,
    node: NodeId,
    source: &dyn RenderKeySource,
    theme: &ThemeColors,
    ui: Option<&UiStates>,
) -> Option<Admission> {
    let tag = tree.kind_tag(node)?;
    if tag.is_a(NodeKindTag::Sprite2D) {
        let key = texture_res(tree, node).and_then(|id| source.renderable_key(id))?;
        Some(Admission::Sprite(key))
    } else if tag.is_a(NodeKindTag::Button) {
        // 按钮恒准入（空文本 = 纯图形按钮）；四态着色在此一次解析。
        let (layout, text) = button_states_of(tree, node, theme, ui);
        Some(Admission::Button(node_key(node), layout, text))
    } else if tag.is_a(NodeKindTag::Label) {
        let state = label_state_of(tree, node)?;
        Some(Admission::Label(
            node_key(node),
            themed_label(tree, node, state, theme),
        ))
    } else if tag.is_a(NodeKindTag::Control) {
        Some(Admission::Control(
            node_key(node),
            themed_control(tree, node, control_state_of(tree, node), theme),
        ))
    } else {
        None
    }
}

/// 槽位名属性读取（缺失/类型错 → `default`）。
fn slot_prop(tree: &SceneTree, node: NodeId, name: &str, default: &str) -> String {
    match tree.prop(node, name) {
        Some(Value::Str(v)) => v.clone(),
        _ => default.to_string(),
    }
}

/// 控件着色（S12.1 E-1）：`fill_slot` / `border_slot` 属性按主题解析
///（fill 缺省空名 = 透明 —— 与 E-1 之前同观感）。
pub fn themed_control(
    tree: &SceneTree,
    node: NodeId,
    mut layout: ControlState,
    theme: &ThemeColors,
) -> ControlState {
    let fill_slot = slot_prop(tree, node, "fill_slot", "");
    let border_slot = slot_prop(tree, node, "border_slot", "border");
    if !fill_slot.is_empty() {
        layout.fill = theme.slot(&fill_slot).unwrap_or(layout.fill);
    }
    layout.border = theme.slot(&border_slot).unwrap_or(layout.border);
    layout
}

/// 文本着色：`color_slot` 属性按主题解析（缺省 text 槽）。
pub fn themed_label(
    tree: &SceneTree,
    node: NodeId,
    mut state: LabelState,
    theme: &ThemeColors,
) -> LabelState {
    let color_slot = slot_prop(tree, node, "color_slot", "text");
    state.color = theme.slot(&color_slot).unwrap_or(state.color);
    state
}

/// 按钮摊平载荷：布局 + 文本一次备齐（含四态换档 —— 悬停边框 accent、
/// 按下填充+边框 accent；S12.0 §3.2）。
fn button_states_of(
    tree: &SceneTree,
    node: NodeId,
    theme: &ThemeColors,
    ui: Option<&UiStates>,
) -> (ControlState, LabelState) {
    let st: WidgetState = ui.and_then(|u| u.get(&node).copied()).unwrap_or_default();
    let mut layout = control_state_of(tree, node);
    // 按钮缺省自带面板填充（槽位可覆写）；hover/pressed 换档。
    let fill_slot = slot_prop(tree, node, "fill_slot", "panel");
    let border_slot = slot_prop(tree, node, "border_slot", "border");
    layout.fill = theme.slot(&fill_slot).unwrap_or(layout.fill);
    layout.border = theme.slot(&border_slot).unwrap_or(layout.border);
    const ACCENT: usize = 6; // THEME_SLOTS 序：bg panel border text text_dim selected accent danger
    if st.hover {
        layout.border = theme.slots[ACCENT];
    }
    if st.pressed {
        layout.fill = theme.slots[ACCENT];
        layout.border = theme.slots[ACCENT];
    }
    let text = match tree.prop(node, "text") {
        Some(Value::Str(v)) => v.clone(),
        _ => String::new(),
    };
    let text_slot = slot_prop(tree, node, "text_slot", "text");
    let mut label = LabelState::new(text, 16.0);
    label.color = theme.slot(&text_slot).unwrap_or(label.color);
    (layout, label)
}

/// 从 `Camera2D` 节点解析契约层的相机状态（[`RenderServer::set_camera`] 的实参）。
///
/// 只做属性搬运与一次形状归一：`zoom` 在场景层是**标量**（schema 冻结），
/// 在契约层是**逐轴** `Vec2`，本层把标量摊到两轴。矩阵、夹紧、可见世界矩形
/// 一律在契约层算（[`Camera2DState::view_matrix`]），本层不重算。
///
/// `viewport` 取当帧 [`FrameInfo`]，`enabled` 取 `active`（缺省启用）；
/// `active=false` 也照实推送 —— "要不要用这台相机"是契约层的判断。
///
/// `zoom` 先挡掉 NaN 与负值（`f32::max` 对 NaN 返回另一侧），剩下的"`0` 与
/// `1` 同义"由契约层的 `effective_zoom` 归一 —— 归一只有一处，别在这里再来一遍。
pub fn camera_state_of(tree: &SceneTree, node: NodeId, viewport: Vec2) -> Camera2DState {
    let world = tree.world(node).unwrap_or(Affine::IDENTITY);
    let zoom = f32_prop(tree, node, PROP_CAMERA_ZOOM, 1.0).max(0.0);
    let enabled = bool_prop(tree, node, PROP_CAMERA_ACTIVE, true);

    Camera2DState {
        transform: affine2_of(world),
        offset: Vec2::ZERO,
        zoom: Vec2::new(zoom, zoom),
        viewport,
        limits: None,
        enabled,
    }
}

/// 从 `Label` 节点备齐排版参数；**空文本返回 `None`**（无可显示内容 ⇒ 不上屏）。
///
/// 断行、字形度量、基线定位属 CPU 侧排版，是后续里程碑的活；本版把排版的
/// **输入**（文本 / 字号）按契约层形状备齐 —— 字号非负且在 `f32` 里有限，
/// 文本为空由上方判空拦下，因此后端拿到的 [`LabelState`] 一定可排版。
pub fn label_state_of(tree: &SceneTree, node: NodeId) -> Option<LabelState> {
    let text = tree
        .prop(node, PROP_LABEL_TEXT)
        .and_then(|value| value.as_str().map(str::to_owned))?;
    if text.is_empty() {
        return None;
    }
    let font_size = f32_prop(tree, node, PROP_LABEL_FONT_SIZE, DEFAULT_LABEL_FONT_SIZE).max(0.0);
    Some(LabelState::new(text.as_str(), font_size))
}

/// 从 `Control` 节点摊出布局状态（[`RenderServer::set_rect`] 的实参）。
///
/// 场景层的 `anchor` / `offset` / `size` 是三个 `Vec2`，契约层要的是
/// "四边锚点 + 四边偏移"。本层做的是**形状转换**：`anchor` 同时落到左上与右下
/// 两个锚点（"锚点定位"），`offset` 是左上偏移，"左上偏移 + 尺寸"是右下偏移
/// （"像素尺寸"）。不重算布局 —— 负宽高**照原样穿过**（已裁决：不钳制），
/// 取舍由契约 [`ControlState::resolve`] 一处决定，保证 LTR / RTL 布局下
/// "尺寸为负"这件事只有一个解释点。
///
/// `min_size` 恒为**无下界**（`None`）：场景层这一版没有最小尺寸属性，本层不凭空
/// 发明默认值，更不得造出零下界 —— 零下界会把负宽高压成 0，既与"负尺寸不钳制"
/// 相悖，也会把场景文件的"未设置"与"显式要求最小尺寸"混为一谈。
pub fn control_state_of(tree: &SceneTree, node: NodeId) -> ControlState {
    let anchor = vec2_prop(tree, node, PROP_CONTROL_ANCHOR, Vec2::ZERO);
    let offset = vec2_prop(tree, node, PROP_CONTROL_OFFSET, Vec2::ZERO);
    let (w, h) = tuple2_prop(tree, node, PROP_CONTROL_SIZE, DEFAULT_CONTROL_SIZE);

    ControlState::new(
        [anchor.x, anchor.y, anchor.x, anchor.y],
        [offset.x, offset.y, offset.x + w, offset.y + h],
    )
}

/// 翻转合成：把 `flip` 作为**子局部**后乘到世界矩阵上（`world ∘ scale(±1,±1)`）。
///
/// 契约层的 [`Flip::compose`] / `RenderItem::world_transform` 用的是同一个
/// 算式，本函数把它提到公开面，供后端与逐帧比对共用 —— 合成只有一处实现，
/// "提取层不把 flip 折进世界矩阵"这条纪律才守得住（折进去后端会再乘一次，
/// 翻两次等于没翻）。
pub fn compose_flip(world: Affine2, flip: Flip) -> Affine2 {
    flip.compose(world)
}

/// 取数值属性（缺失 / 类型不对 → `default`），`F32` 与 `I64` 都认。
///
/// 两种都要认不是宽容，是场景层的实情：`font_size` 在 schema 里声明为 `I64`
/// （编辑器给的是整数步进），`zoom` 声明为 `F32`。只认 `F32` 会让"用户在
/// 场景里把字号改成 32"被静默忽略成缺省 16 —— 属性读不到就是属性丢失。
fn f32_prop(tree: &SceneTree, node: NodeId, name: &str, default: f32) -> f32 {
    tree.prop(node, name)
        .and_then(|value| value.as_f32().or_else(|| value.as_i64().map(|v| v as f32)))
        .unwrap_or(default)
}

/// 取 `Vec2` 属性（缺失 / 类型不对 → `default`），并桥到契约层的 [`Vec2`]。
///
/// 两侧的 `Vec2` 是**不同的类型**（场景层 `nes_scene::Vec2` 是几何缓存，
/// 契约层的是 wire 格式），因此这里必须过一趟 [`vec2_of`]，不能靠类型推断。
fn vec2_prop(tree: &SceneTree, node: NodeId, name: &str, default: Vec2) -> Vec2 {
    tree.prop(node, name)
        .and_then(|value| value.as_vec2())
        .map_or(default, vec2_of)
}

/// 取"两个浮点"属性的逐轴读法（缺失 / 类型不对 → `default`）。
fn tuple2_prop(tree: &SceneTree, node: NodeId, name: &str, default: (f32, f32)) -> (f32, f32) {
    match tree.prop(node, name).and_then(|value| value.as_vec2()) {
        Some(v) => (v.x, v.y),
        None => default,
    }
}

/// 取 `Sprite2D` 的纹理槽位。
///
/// 走 [`ResId::from_value`] 而不是裸 `match`：它顺手把两种"不可渲染"的写法
/// 折成同一个 `None` —— 值不是 `Resource`（类型不对）、以及 `Resource(0)`
/// （未绑定，M2 起的约定）。提取层的准入条件只看 `None`，不看原因。
fn texture_res(tree: &SceneTree, node: NodeId) -> Option<ResId> {
    tree.prop(node, PROP_TEXTURE)
        .and_then(ResId::from_value)
}

/// 取布尔属性（缺失 → `default`）。
fn bool_prop(tree: &SceneTree, node: NodeId, name: &str, default: bool) -> bool {
    tree.prop(node, name)
        .and_then(|value| value.as_bool())
        .unwrap_or(default)
}

/// 取 `z_index`（缺失 / 类型不对 → `0`；schema 已把范围 clamp 到 ±4096）。
fn z_of(tree: &SceneTree, node: NodeId) -> i32 {
    tree.prop(node, PROP_Z_INDEX)
        .and_then(|value| value.as_i64())
        .unwrap_or(0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_consistency_rule() {
        let base = ExtractStats {
            nodes_visited: 4,
            pushed: 2,
            created: 2,
            fresh: 1,
            rebound: 1,
            reused: 0,
            destroyed: 1,
            dropped: 0,
            swept: 0,
            map_len: 2,
        };
        assert!(base.is_consistent());

        // 每次 create 都要有出处：只改 created 就破坏自洽。
        let bad = ExtractStats {
            created: 3,
            ..base
        };
        assert!(!bad.is_consistent());
    }
}
