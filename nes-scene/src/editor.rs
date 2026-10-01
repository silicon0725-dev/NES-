//! 编辑器核心状态层（S9-3a）：Selection + Inspector/Hierarchy 变更
//! 适配器 + 事务绑定。
//!
//! # 职责边界（S9-3 契约）
//!
//! - **Selection = uid 有序集**（编辑器会话态，不进事务、不落盘）；
//!   悬空条目（uid 无活节点）保留不剔除 —— undo/redo 往返时选择自动
//!   恢复有效（身份连续性，S9-0 Q6）。
//! - **Inspector/Hierarchy 适配器**：一切设计数据修改经 `TransactionLog`
//!   记录（Inspector 是事务的普通客户，无特权通道）；会话态
//!   （hover/高亮/gizmo 中间帧/展开）不经此 —— Document State ≠
//!   Editor Session State。
//! - Hierarchy 不另造 EditorTree —— `SceneTree` + uid/父 uid/兄弟序
//!   即完整表达（与 S9-1 指纹同一三元组）。

use crate::identity::NodeId;
use crate::transaction::{NodeData2, SubtreeSnapshot, TransactionLog, TxCapture};
use crate::node::NodeKind;
use crate::transform::Transform2D;
use crate::tree::{SceneTree, Uid};
use crate::value::Value;

// ------------------------------------------------------------ Selection

/// 选择模型（S9-3）：**uid 有序集**，主选 = 首元素。
///
/// 悬空条目（uid 无活节点）**保留不剔除**：redo 删除后条目悬空，
/// undo 复活即自动重新有效 —— 不会因 UI 清理而丢编辑器状态。
/// 会话态：不进事务、不落盘（D1 编辑态口径）。
#[derive(Clone, Default)]
pub struct Selection {
    uids: Vec<Uid>,
}

impl Selection {
    pub fn new() -> Self {
        Self { uids: Vec::new() }
    }

    /// 选中（已在集则移到首位 = 成为主选）。
    pub fn select(&mut self, uid: Uid) {
        self.uids.retain(|u| u != &uid);
        self.uids.insert(0, uid);
    }

    /// 切换（在则移除、不在则加入并成为主选）。
    pub fn toggle(&mut self, uid: Uid) {
        if self.uids.iter().any(|u| u == &uid) {
            self.uids.retain(|u| u != &uid);
        } else {
            self.uids.insert(0, uid);
        }
    }

    pub fn clear(&mut self) {
        self.uids.clear();
    }

    pub fn len(&self) -> usize {
        self.uids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.uids.is_empty()
    }

    pub fn contains(&self, uid: &Uid) -> bool {
        self.uids.iter().any(|u| u == uid)
    }

    /// 全部条目（**含悬空**）。
    pub fn uids(&self) -> &[Uid] {
        &self.uids
    }

    /// 仅命中活节点的部分（uid -> Handle 解析经 find_by_uid）。
    pub fn live(&self, tree: &SceneTree) -> Vec<NodeId> {
        self.uids.iter().filter_map(|u| tree.find_by_uid(u)).collect()
    }

    /// 主选（首元素的活节点；悬空则 None）。
    pub fn primary(&self, tree: &SceneTree) -> Option<NodeId> {
        self.uids.first().and_then(|u| tree.find_by_uid(u))
    }
}

// ------------------------------------------------------------ Inspector 适配器

/// Inspector 变更适配器（S9-3a）：设计数据修改的**事务化惯用法**。
///
/// 一个适配器实例绑定一个进行中的"面板编辑会话"：`begin` 开事务，
/// 各字段修改方法自动记录 Modified（before/after 快照），`commit`
/// 落账。**gizmo 拖拽合并提交**：拖拽中间帧直接写树（preview），
/// 只在 drop 后经 `modify_local` 记一次 + commit —— 一次拖拽一条
/// 事务（T-INS-02 口径）。
pub struct Inspector<'a> {
    tree: &'a mut SceneTree,
    log: &'a mut TransactionLog,
}

impl<'a> Inspector<'a> {
    pub fn new(tree: &'a mut SceneTree, log: &'a mut TransactionLog) -> Self {
        Self { tree, log }
    }

    /// 开始一次编辑事务（须配对 commit）。
    pub fn begin(&mut self) -> Result<(), String> {
        self.log.begin()
    }

    pub fn commit(&mut self) -> Result<(), String> {
        self.log.commit()
    }

    fn node_data(&self, id: NodeId) -> Result<NodeData2, String> {
        Ok(NodeData2 {
            uid: self.tree.uid_of(id).ok_or("节点无 uid")?,
            kind: self.tree.kind_tag(id).ok_or("节点无类型")?.kind().clone(),
            name: self.tree.name(id).ok_or("节点无名")?.to_string(),
            local: self.tree.local(id).unwrap_or(Transform2D::IDENTITY),
            process_mode: self.tree.process_mode(id).unwrap_or_default(),
            props: self.tree.props(id).cloned().unwrap_or_default(),
        })
    }

    /// 修改名字（一条 Modified）。
    pub fn modify_name(&mut self, uid: &Uid, new_name: &str) -> Result<(), String> {
        let id = self.tree.find_by_uid(uid).ok_or("节点不在树中")?;
        let before = self.node_data(id)?;
        self.tree.rename(id, new_name);
        let after = self.node_data(id)?;
        self.log.record(TxCapture::Modified { uid: uid.clone(), before, after })
    }

    /// 修改变换（gizmo 落点一次调用 = 一条 Modified）。
    pub fn modify_local(&mut self, uid: &Uid, new_local: Transform2D) -> Result<(), String> {
        let id = self.tree.find_by_uid(uid).ok_or("节点不在树中")?;
        let before = self.node_data(id)?;
        self.tree.set_local(id, new_local);
        let after = self.node_data(id)?;
        self.log.record(TxCapture::Modified { uid: uid.clone(), before, after })
    }

    /// 修改属性（一条 Modified；多次属性编辑 = 多条 —— 面板逐字段提交）。
    pub fn modify_prop(&mut self, uid: &Uid, key: &str, value: Value) -> Result<(), String> {
        let id = self.tree.find_by_uid(uid).ok_or("节点不在树中")?;
        let before = self.node_data(id)?;
        self.tree.set_prop(id, key, value).map_err(|e| e.to_string())?;
        let after = self.node_data(id)?;
        self.log.record(TxCapture::Modified { uid: uid.clone(), before, after })
    }
}

// ------------------------------------------------------------ Hierarchy 适配器

/// 层级变更适配器（S9-3a）：拖拽重排/移父的**事务化惯用法**。
///
/// 结构即 `SceneTree`（不另造 EditorTree）；一切经既有 Reparent
/// 事务（S9-2 T-TX-05）。
pub struct Hierarchy<'a> {
    tree: &'a mut SceneTree,
    log: &'a mut TransactionLog,
}

impl<'a> Hierarchy<'a> {
    pub fn new(tree: &'a mut SceneTree, log: &'a mut TransactionLog) -> Self {
        Self { tree, log }
    }

    /// 开始一次结构编辑事务（须配对 commit）。
    pub fn begin(&mut self) -> Result<(), String> {
        self.log.begin()
    }

    pub fn commit(&mut self) -> Result<(), String> {
        self.log.commit()
    }

    /// 拖放：把 `uid` 挂到 `new_parent` 的 `at` 位（一条 Reparented）。
    /// 同父不同位 = 重排；异父 = 移动 —— 同一事务形态。
    pub fn drag_to(&mut self, uid: &Uid, new_parent: &Uid, at: Option<usize>) -> Result<(), String> {
        let id = self.tree.find_by_uid(uid).ok_or("拖动节点不在树中")?;
        let old_parent_id = self
            .tree
            .parent(id)
            .ok_or("拖动节点无父（根不可拖）")?;
        let old_parent = self.tree.uid_of(old_parent_id).ok_or("原父无 uid")?;
        let old_at = self
            .tree
            .children(old_parent_id)
            .iter()
            .position(|&c| c == id)
            .unwrap_or(0);
        let new_parent_id = self.tree.find_by_uid(new_parent).ok_or("目标父不在树中")?;
        self.tree.reparent(id, new_parent_id, at);
        self.tree.apply_pending();
        self.log.record(TxCapture::Reparented {
            uid: uid.clone(),
            old_parent,
            old_at,
            new_parent: new_parent.clone(),
        })
    }

    /// 在 `parent` 下新建节点（一条 Created；子树快照含子节点）。
    pub fn create_child(
        &mut self,
        parent: &Uid,
        name: &str,
        kind: NodeKind,
    ) -> Result<Uid, String> {
        let pid = self.tree.find_by_uid(parent).ok_or("父不在树中")?;
        let id = self.tree.add_node(pid, name, kind);
        self.tree.apply_pending();
        let uid = self.tree.uid_of(id).ok_or("新节点无 uid")?;
        let at = self.tree.children(pid).iter().position(|&c| c == id).unwrap_or(0);
        let snapshot = SubtreeSnapshot::capture(self.tree, id, parent.clone(), at)
            .ok_or("快照失败")?;
        self.log.record(TxCapture::Created { snapshot })?;
        Ok(uid)
    }

    /// 删除子树（一条 Removed；子树快照含全部后代）。
    pub fn delete_subtree(&mut self, uid: &Uid) -> Result<(), String> {
        let id = self.tree.find_by_uid(uid).ok_or("删除节点不在树中")?;
        let parent_id = self.tree.parent(id).ok_or("根不可删（编辑器口径）")?;
        let parent = self.tree.uid_of(parent_id).ok_or("父无 uid")?;
        let at = self.tree.children(parent_id).iter().position(|&c| c == id).unwrap_or(0);
        let snapshot = SubtreeSnapshot::capture(self.tree, id, parent.clone(), at)
            .ok_or("快照失败")?;
        self.tree.remove_node(id, false);
        self.tree.apply_pending();
        self.log.record(TxCapture::Removed { snapshot })
    }
}
