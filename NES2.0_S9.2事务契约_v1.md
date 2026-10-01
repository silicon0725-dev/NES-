# NES 2.0 · S9-2 事务与撤销/重做（Transaction / Undo / Redo）v1

> 交付日期：2026-10-02　｜　状态：**事务原语 + T-TX-01..07 全过 —— 身份连续性经事务保持（原 uid 复活，不被新对象抢占）**
> 前置：S9-1（uid 一等字段 + `add_node_with_uid` 恢复路径）；S9-0 Q3..Q6 四象限。

---

## 0. 一句话结论

`SceneTransaction`：**操作级逆记录**（非全量快照 —— 每个原语操作的
逆足够重建；子树删除记**节点序列化快照**因其不可逆推）。`undo` =
逆序应用逆记录（**原 uid 复活**经 `add_node_with_uid`）；`redo` =
正序重放（同一身份回归）。事务栈 = 线性历史（v1 不做分支/合并）。
**身份连续性七问（T-TX-01..07）全过**：create/modify/delete/subtree/
reparent/多级历史/历史不占新身份。全仓测试 **457+7=464** 全绿，
守卫 11/11，clippy 零。

---

## 1. 设计

### 1.1 操作记录（Op-level inverse log）

```rust
pub enum UndoEntry {
    Created { node: NodeId, uid: Uid },                  // 逆 = 删除
    Removed { snapshot: NodeSnapshot, parent: NodeId,
              at: usize },                               // 逆 = add_node_with_uid + 恢复
    Modified { node: NodeId, kind: NodeKind, name: String,
               local: Transform2D, process_mode: ProcessMode,
               props: PropStore },                       // 逆 = 写回旧值
    Reparented { node: NodeId, old_parent: NodeId, at: usize },  // 逆 = 挂回
}
```

- **Modified 记改动前的整份节点设计数据**（kind/name/local/mode/
  props —— 结构性字段不变，逆即完整）；结构变更（Created/Removed/
  Reparented）**只记拓扑**（uid 锚定身份）。
- **子树删除**：递归收集子树全部节点的 `NodeSnapshot`（uid/kind/
  name/local/mode/props + 父子关系 + 兄弟位置），恢复时自顶向下
  `add_node_with_uid` 原身份重建。
- 事务边界：宿主 `begin()` .. `commit()` 之间的一切树写入聚合为一条
  `SceneTransaction`（undo 的最小单位）；嵌套 begin 报错（v1 线性）。

### 1.2 身份连续性（S9-0 Q3..Q6 的实现兑现）

```text
Create A(X) → Modify → Delete → Undo → A(X)   // X 不变
                                  Redo → 删除（X 死亡但不被占）
                                  Undo → A(X)  // 同一身份回归
Delete A(X) → 新建 B            // B ≠ X（死 uid 不被抢占）
Undo（A 复活 X）                 // X 与 B 共存 —— 不冲突（B 本就不是 X）
```

undo 恢复走 `add_node_with_uid(原 uid)`；redo 重放 = 再次执行原
操作（删除/修改/移动）。**redo 不缓存快照** —— 重放原始写入，
行为与首次一致（身份自然回归）。

### 1.3 API 面（nes-scene）

| 成员 | 职责 |
|---|---|
| `TransactionLog::new()` | 空历史 |
| `log.begin()` / `log.commit()` | 事务边界（聚合为一条） |
| `log.undo(&mut tree)` / `log.redo(&mut tree)` | 栈式撤销/重做（返回是否动作） |
| `log.can_undo()` / `can_redo()` | 边界查询 |
| `log.clear()` | 清历史（宿主换场景） |

记录收集：事务开启期间，宿主的树写入经 `log.record_*` 辅助（或
`TransactionCtx` 包装）进入记录 —— v1 不做写入拦截（无 Drop 钩子
侵入 SceneTree），编辑器宿主经包装方法写入即可保证完整记录。

## 2. T-TX-01..07（身份连续性七问）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-TX-01 | Create → Undo（节点消失）→ Redo（回来且 **uid 同**） | ✅ |
| T-TX-02 | Modify（local/props/name）→ Undo → Redo：**uid 全程不变**，值回到各版本 | ✅ |
| T-TX-03 | Delete → Undo（**原 uid 复活**）；期间新建对象不得抢占；Redo 再删（uid 死亡不被占）→ Undo 再活 | ✅ |
| T-TX-04 | 子树删除（3 层 5 节点）→ Undo：**整个子树 uid 集合完全恢复**（含父子关系与兄弟位） | ✅ |
| T-TX-05 | Reparent + 位置 → Undo：uid 不变、回原父原位 | ✅ |
| T-TX-06 | 连续事务 A→B→C，逐级 Undo 到初态再逐级 Redo 到末态：**每级 uid 与状态对应** | ✅ |
| T-TX-07 | Undo 后创建新对象：与历史中的（现死亡的）uid **不冲突**——两规则（死 uid 不被新对象占 / undo 恢复原 uid）兼容 | ✅ |

## 3. 边界与遗留

| 事项 | 口径 |
|---|---|
| 写入拦截 | v1 宿主经包装方法记录；自动拦截（Drop/代理）属后续 |
| 事务分支 | 线性历史；undo 后新写入丢弃 redo 尾（标准编辑器行为） |
| 子场景边界 | 事务只管本树；子场景文件的事务属子场景编辑器会话 |
| 脚本 VM 状态 | undo 恢复节点不自动恢复 VM locals（脚本状态非设计数据；S9-3 裁决口径） |
| uid 泄漏防线 | 事务记录里的 uid 仅用于恢复，不进指纹/不进序列化 |

## 4. 记账

- 测试：**nes-scene 195→202**（+T-TX-01..07），全仓合计 **464**；
- 改动面：`nes-scene/src/transaction.rs`（新）+ lib 导出；其余零改动；
- S9-3（Inspector/选择/层级）解除 BLOCKED。

*（内容由AI生成，仅供参考）*
