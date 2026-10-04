//! `NodeId` ↔ `ItemHandle` 映射表：渲染侧的"主实体表"。
//!
//! 等价物：Bevy 的 `RenderEntity` / `MainEntity` 映射，Godot 的 RID 表。
//! 纪律与两端各自的身份设计保持同构：
//!
//! - key 是**稳定身份**（`NodeId`，槽位 + 代际，删除后永不复用）；
//! - value 里的句柄是**易变后端值**（`ItemHandle`，销毁后不得再分配）；
//! - 因此"节点被删 → 句柄释放 → 新节点复用同一槽位"不会串味：
//!   新节点的 `NodeId` 代际不同，在映射表里天然是两个不同的 key。
//!
//! 表用 `BTreeMap` 而不是 `HashMap`：本层全线要求**确定性**，迭代顺序
//! 必须只由 key 决定，不能受哈希随机种子的影响。

use std::collections::BTreeMap;

use nes_render_api::{ItemHandle, RenderAssetKey};
use nes_scene::NodeId;

/// 一个存活渲染物的映射条目：句柄 + 资源键 + 建条目帧号 + 本帧标记。
///
/// `key` 是**资源换代检测**的依据：同一个节点若某帧解析出的资源键与建条目时不同，
/// 说明它换了资源（或资源槽位被回收后重新绑定），此时必须
/// `destroy_item(旧)` + `create_item(新)`，而不是继续复用旧句柄。
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct ItemSlot {
    /// 该渲染物对应的稳定节点身份。
    pub node: NodeId,
    /// 后端侧句柄（易变，进程内有效）。
    pub handle: ItemHandle,
    /// 建条目时推送的资源键。
    pub key: RenderAssetKey,
    /// 建条目时的提取帧序号（从 `1` 起；仅用于诊断"这个渲染物活了多久"）。
    pub created_frame: u64,
    /// 最近一次被遍历到的提取帧序号（清扫依据；`0` 表示从未被遍历）。
    seen_frame: u64,
    /// 上一提取帧是否对该条目推送过 `SetClip(Some)`（S12-3 评审修复：
    /// Some→None 迁移帧据此补推一次 `set_clip(None)` —— 跨帧簿记的
    /// 陈旧裁剪必须显式清除，否则 undo 移出 ScrollView 后旧裁剪残留）。
    pub clipped: bool,
    /// 上一提取帧是否对该条目推送过 `SetUv`（S16.2：图集帧动画的
    /// 激活→整图迁移帧据此补推一次恒等矩形 `[0,0,1,1]` —— uv 簿记跨帧
    /// 持久且"无记录 = 整瓦片"，停用图集后陈旧子矩形同样必须显式清除；
    /// 与 clipped 同一条"全量快照的生产者侧义务"）。
    pub uv_active: bool,
    /// 上一提取帧是否对该条目推送过 `SetPivot`（S16.3：精灵锚点的
    /// 非(0,0)→(0,0) 迁移帧据此补推一次零向量 `[0,0]` —— pivot 簿记
    /// 跨帧持久且"无记录 = 无平移"，从设过非零锚点回调缺省后陈旧锚点
    /// 同样必须显式清除；与 clipped / uv_active 同一条义务）。
    pub pivot_active: bool,
    /// 上一提取帧是否对该条目推送过 `SetNineSlice`（S16.6：九宫格的
    /// 有效→清空迁移帧据此补推一次 NIL **恒等记录**（照 pivot 零向量
    /// 先例：照存照发，消费端据此摘跨帧簿记）—— nines 簿记跨帧持久且
    /// "无记录 / 恒等记录 = fill/border 照旧"，清掉 ns_tex 或边距归零后
    /// 陈旧九宫格同样必须显式清除；与 clipped / uv_active / pivot_active
    /// 同一条"全量快照的生产者侧义务"）。
    pub nines_active: bool,
}

impl ItemSlot {
    /// 最近一次被遍历到的提取帧序号（`0` = 从未）。
    pub fn seen_frame(&self) -> u64 {
        self.seen_frame
    }
}

/// `NodeId` → [`ItemSlot`] 的映射表（含"本帧是否见过"的清扫标记）。
///
/// 它是提取器的内部状态，因此**只在 crate 内写入**，对外只读：
/// 外部（S3 的子阶段、调试工具）可以查"某节点现在挂在哪个句柄上"，
/// 但不能绕过 [`RenderExtractor::extract_into`](crate::RenderExtractor::extract_into)
/// 伪造映射 —— 那正是悬垂句柄的来源。
#[derive(Clone, Debug, Default)]
pub struct NodeItemMap {
    slots: BTreeMap<NodeId, ItemSlot>,
}

impl NodeItemMap {
    /// 空表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 存活渲染物数量。
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// 是否没有任何存活渲染物。
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 取某节点的条目。
    pub fn get(&self, node: NodeId) -> Option<&ItemSlot> {
        self.slots.get(&node)
    }

    /// 某节点当前挂的句柄（`None` = 本节点当前不可渲染）。
    pub fn handle_of(&self, node: NodeId) -> Option<ItemHandle> {
        self.slots.get(&node).map(|slot| slot.handle)
    }

    /// 某节点当前是否有存活条目。
    pub fn contains(&self, node: NodeId) -> bool {
        self.slots.contains_key(&node)
    }

    /// 遍历全部条目（按 `NodeId` 升序，确定性）。
    pub fn iter(&self) -> impl Iterator<Item = &ItemSlot> {
        self.slots.values()
    }
}

// ---- 以下为 crate 内写入口（外部不可见，防伪造映射）----

impl NodeItemMap {
    /// 建立条目（新节点 / 换代重建）。
    pub(crate) fn insert(
        &mut self,
        node: NodeId,
        handle: ItemHandle,
        key: RenderAssetKey,
        frame: u64,
    ) {
        self.slots.insert(
            node,
            ItemSlot {
                node,
                handle,
                key,
                created_frame: frame,
                seen_frame: frame,
                clipped: false,
                uv_active: false,
                pivot_active: false,
                nines_active: false,
            },
        );
    }

    /// 摘掉条目并返回它（节点删除 / 本帧不再可渲染）。
    pub(crate) fn remove(&mut self, node: NodeId) -> Option<ItemSlot> {
        self.slots.remove(&node)
    }

    /// 读写条目的 clip 推送标记（Some→None 迁移检测用；`None` = 无条目）。
    pub fn take_clipped(&mut self, node: NodeId, now: bool) -> Option<bool> {
        self.slots.get_mut(&node).map(|slot| {
            let was = slot.clipped;
            slot.clipped = now;
            was
        })
    }

    /// 读写条目的 uv 推送标记（S16.2 激活→整图迁移检测用；`None` = 无条目）。
    pub fn take_uv_active(&mut self, node: NodeId, now: bool) -> Option<bool> {
        self.slots.get_mut(&node).map(|slot| {
            let was = slot.uv_active;
            slot.uv_active = now;
            was
        })
    }

    /// 读写条目的 pivot 推送标记（S16.3 非(0,0)→(0,0) 迁移检测用；
    /// `None` = 无条目）。
    pub fn take_pivot_active(&mut self, node: NodeId, now: bool) -> Option<bool> {
        self.slots.get_mut(&node).map(|slot| {
            let was = slot.pivot_active;
            slot.pivot_active = now;
            was
        })
    }

    /// 读写条目的九宫格推送标记（S16.6 有效→清空迁移检测用；`None` = 无条目）。
    pub fn take_nines_active(&mut self, node: NodeId, now: bool) -> Option<bool> {
        self.slots.get_mut(&node).map(|slot| {
            let was = slot.nines_active;
            slot.nines_active = now;
            was
        })
    }

    /// 标记"本帧见过"（清扫的反面）。
    pub(crate) fn mark_seen(&mut self, node: NodeId, frame: u64) {
        if let Some(slot) = self.slots.get_mut(&node) {
            slot.seen_frame = frame;
        }
    }

    /// 清扫：把本帧**未见过**的条目全部摘掉，逐个交给 `on_drop`（用于 `destroy_item`）。
    ///
    /// 返回摘掉的条目数。节点被删除、或整棵子树被摘下来时，其下的所有节点
    /// 都不会出现在本帧遍历里 —— 这一步就是"删节点不泄漏"的兜底。
    pub(crate) fn retain_seen(&mut self, frame: u64, mut on_drop: impl FnMut(ItemHandle)) -> usize {
        let before = self.slots.len();
        self.slots.retain(|_, slot| {
            if slot.seen_frame == frame {
                true
            } else {
                on_drop(slot.handle);
                false
            }
        });
        before - self.slots.len()
    }
}