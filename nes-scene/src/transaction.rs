//! 事务与撤销/重做（S9-2，S9-0 Q3..Q6 兑现）。
//!
//! # 核心不变量：身份连续性
//!
//! ```text
//! Create A(X) → Modify → Delete → Undo → A(X)   // X 不变
//! Redo → 删除（X 死亡不被占）→ Undo → A(X)      // 同一身份回归
//! Delete A(X) → 新建 B → B ≠ X                  // 死 uid 不被抢占
//! ```
//!
//! 事务 = **操作级双向记录**：每条记录携带 undo 方向与 redo 方向各自
//! 需要的数据（结构变更记节点快照 —— uid 锚定身份；修改记新旧两份
//! 设计数据；移动记新旧拓扑）。undo = 逆序走 undo 方向；redo = 正序
//! 走 redo 方向（**重放原始写入，身份自然回归**）。

use crate::identity::NodeId;
use crate::props::PropStore;
use crate::node::NodeKind;
use crate::transform::Transform2D;
use crate::tree::{ProcessMode, SceneTree, Uid};

/// 节点设计数据（结构恢复的最小完整集；uid 锚定身份）。
#[derive(Clone)]
pub struct NodeData2 {
    pub uid: Uid,
    pub kind: NodeKind,
    pub name: String,
    pub local: Transform2D,
    pub process_mode: ProcessMode,
    pub props: PropStore,
}

/// 子树快照（含父子关系与兄弟位；恢复时自顶向下原 uid 重建）。
#[derive(Clone)]
pub struct SubtreeSnapshot {
    pub data: NodeData2,
    /// 父的 uid（顶层快照 = 挂载点）。
    pub parent: Uid,
    /// 在父 children 里的位置（越界则追加）。
    pub at: usize,
    pub children: Vec<SubtreeSnapshot>,
}

impl SubtreeSnapshot {
    /// 收集某节点（含子树）的快照；`parent`/`at` 描述该节点在树中的位置。
    pub fn capture(tree: &SceneTree, node: NodeId, parent: Uid, at: usize) -> Option<Self> {
        let kind = tree.kind_tag(node)?.kind();
        let children: Vec<Self> = tree
            .children(node)
            .iter()
            .enumerate()
            .filter_map(|(i, &c)| {
                let p_uid = tree.uid_of(node)?;
                Self::capture(tree, c, p_uid, i)
            })
            .collect();
        Some(Self {
            data: NodeData2 {
                uid: tree.uid_of(node)?,
                kind,
                name: tree.name(node)?.to_string(),
                local: tree.local(node).unwrap_or(Transform2D::IDENTITY),
                process_mode: tree.process_mode(node).unwrap_or_default(),
                props: tree.props(node).cloned().unwrap_or_default(),
            },
            parent,
            at,
            children,
        })
    }

    /// 按快照重建子树（原 uid 复活；返回新 NodeId）。位置尽力恢复
    ///（父 children 不足则追加 —— 撤销序列保证一致性）。
    pub fn restore(&self, tree: &mut SceneTree, parent: NodeId, at: usize) -> Result<NodeId, String> {
        let id = tree.add_node_with_uid(parent, &self.data.name, self.data.kind.clone(), self.data.uid.clone())?;
        tree.set_local(id, self.data.local);
        tree.set_process_mode(id, self.data.process_mode);
        for (k, v) in self.data.props.iter() {
            let _ = tree.set_prop(id, k, v.clone());
        }
        for (i, child) in self.children.iter().enumerate() {
            child.restore(tree, id, i)?;
        }
        tree.apply_pending();
        // 位置：reparent 到指定位（add 追加在尾）。
        if at < tree.children(parent).len() {
            tree.reparent(id, parent, Some(at));
            tree.apply_pending();
        }
        Ok(id)
    }

    /// 应用一份节点数据到已存在节点（修改方向）。
    ///
    /// S12-9 起公开：play-in-editor 的 RESET 与事务的 Modified 方向走
    /// **同一条数据还原路** —— 运行期树结构不变（脚本没有结构指令、
    /// 编辑交互已禁用），按 uid 寻回节点后整体写回设计数据即可。结构
    /// 层的 [`Self::restore`] 会换 NodeId（删了重加），编辑器壳层持有
    /// 的控件手柄经不起 —— 所以还原只走数据面，不动结构。
    pub fn apply_data(tree: &mut SceneTree, id: NodeId, d: &NodeData2) -> Result<(), String> {
        tree.rename(id, &d.name);
        tree.set_local(id, d.local);
        tree.set_process_mode(id, d.process_mode);
        // 属性：先清后写（恢复精确旧值/新值 —— 逆记录自足）。
        for (k, v) in d.props.iter() {
            let _ = tree.set_prop(id, k, v.clone());
        }
        Ok(())
    }
}

/// 一条双向操作记录（undo 与 redo 各携所需）。
#[derive(Clone)]
pub enum TxCapture {
    /// 创建：undo = 删除；redo = 按快照重建（同 uid）。
    Created { snapshot: SubtreeSnapshot },
    /// 删除：undo = 按快照复活；redo = 再删除。
    Removed { snapshot: SubtreeSnapshot },
    /// 修改：undo = 写回旧数据；redo = 写新数据（uid 不变）。
    Modified {
        uid: Uid,
        before: NodeData2,
        after: NodeData2,
    },
    /// 移动/重挂：undo = 回旧拓扑；redo = 到新拓扑。
    Reparented {
        uid: Uid,
        old_parent: Uid,
        old_at: usize,
        new_parent: Uid,
    },
}

/// 一个事务（一批记录 —— undo/redo 的最小单位）。
#[derive(Clone, Default)]
pub struct SceneTransaction {
    entries: Vec<TxCapture>,
}

/// 事务历史（线性栈；undo 后新提交丢弃 redo 尾 —— 标准编辑器行为）。
pub struct TransactionLog {
    undo_stack: Vec<SceneTransaction>,
    redo_stack: Vec<SceneTransaction>,
    current: Option<SceneTransaction>,
}

impl Default for TransactionLog {
    fn default() -> Self {
        Self::new()
    }
}

impl TransactionLog {
    pub fn new() -> Self {
        Self { undo_stack: Vec::new(), redo_stack: Vec::new(), current: None }
    }

    /// 开始一个事务（嵌套报错 —— v1 线性）。
    pub fn begin(&mut self) -> Result<(), String> {
        if self.current.is_some() {
            return Err("事务嵌套（commit 前不可再 begin）".to_string());
        }
        self.current = Some(SceneTransaction::default());
        Ok(())
    }

    /// 提交当前事务进 undo 栈（清 redo 尾）。
    pub fn commit(&mut self) -> Result<(), String> {
        let Some(tx) = self.current.take() else {
            return Err("commit 无进行中的事务".to_string());
        };
        if !tx.entries.is_empty() {
            self.redo_stack.clear();
            self.undo_stack.push(tx);
        }
        Ok(())
    }

    /// 记录一条（须在 begin..commit 内）。
    pub fn record(&mut self, e: TxCapture) -> Result<(), String> {
        let Some(tx) = self.current.as_mut() else {
            return Err("记录须在 begin..commit 之间".to_string());
        };
        tx.entries.push(e);
        Ok(())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.current = None;
    }

    /// 撤销最近一个事务（逆序走 undo 方向）。返回是否执行了动作。
    pub fn undo(&mut self, tree: &mut SceneTree) -> Result<bool, String> {
        let Some(tx) = self.undo_stack.pop() else {
            return Ok(false);
        };
        // 部分应用失败：事务**推回栈顶**（已应用的逆保留效果，未应用的
        // 仍在 —— 身份冲突等如实报错但历史不丢）。
        for e in tx.entries.iter().rev() {
            if let Err(err) = self.apply(tree, e, false) {
                self.undo_stack.push(tx);
                return Err(err);
            }
        }
        self.redo_stack.push(tx);
        Ok(true)
    }

    /// 重做最近撤销的事务（正序走 redo 方向）。
    pub fn redo(&mut self, tree: &mut SceneTree) -> Result<bool, String> {
        let Some(mut tx) = self.redo_stack.pop() else {
            return Ok(false);
        };
        for e in tx.entries.iter_mut() {
            if let Err(err) = self.apply(tree, e, true) {
                self.redo_stack.push(tx);
                return Err(err);
            }
        }
        self.undo_stack.push(tx);
        Ok(true)
    }

    /// 应用一条记录的指定方向。redo 方向可能更新记录（Created 的
    /// redo 重建后快照里 NodeId 无关 —— uid 锚定，无需更新）。
    fn apply(&self, tree: &mut SceneTree, e: &TxCapture, redo: bool) -> Result<(), String> {
        match e {
            TxCapture::Created { snapshot } => {
                if redo {
                    let parent = tree.find_by_uid(&snapshot.parent).ok_or_else(|| {
                        format!("redo 创建失败：父 {} 不在树中", snapshot.parent.to_hex())
                    })?;
                    snapshot.restore(tree, parent, snapshot.at)?;
                } else {
                    let id = tree.find_by_uid(&snapshot.data.uid).ok_or_else(|| {
                        format!("undo 创建失败：{} 不在树中", snapshot.data.uid.to_hex())
                    })?;
                    tree.remove_node(id, false);
                    tree.apply_pending();
                }
                Ok(())
            }
            TxCapture::Removed { snapshot } => {
                if redo {
                    let id = tree.find_by_uid(&snapshot.data.uid).ok_or_else(|| {
                        format!("redo 删除失败：{} 不在树中", snapshot.data.uid.to_hex())
                    })?;
                    tree.remove_node(id, false);
                    tree.apply_pending();
                } else {
                    let parent = tree.find_by_uid(&snapshot.parent).ok_or_else(|| {
                        format!("undo 删除失败：父 {} 不在树中", snapshot.parent.to_hex())
                    })?;
                    snapshot.restore(tree, parent, snapshot.at)?;
                }
                Ok(())
            }
            TxCapture::Modified { uid, before, after } => {
                let id = tree
                    .find_by_uid(uid)
                    .ok_or_else(|| format!("修改方向失败：{} 不在树中", uid.to_hex()))?;
                let d = if redo { after } else { before };
                SubtreeSnapshot::apply_data(tree, id, d)
            }
            TxCapture::Reparented { uid, old_parent, old_at, new_parent } => {
                let id = tree
                    .find_by_uid(uid)
                    .ok_or_else(|| format!("移动方向失败：{} 不在树中", uid.to_hex()))?;
                let (p, at) = if redo {
                    (new_parent.clone(), None)
                } else {
                    (old_parent.clone(), Some(*old_at))
                };
                let pid = tree.find_by_uid(&p).ok_or_else(|| {
                    format!("移动方向失败：目标父 {} 不在树中", p.to_hex())
                })?;
                tree.reparent(id, pid, at);
                tree.apply_pending();
                Ok(())
            }
        }
    }
}
