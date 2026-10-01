# NES 2.0 · S9-3 编辑器对象模型契约（Selection / Inspector / Hierarchy）v1

> 交付日期：2026-10-02　｜　状态：**三模型契约文本（实现前置裁决）；零代码**
> 前置：S9-1 uid（CLOSED）/ S9-2 事务（CLOSED）/ D1（运行时值 ≠ Project Model）。

---

## 0. 一句话结论

三模型裁决：**Selection = uid 集**（跨 undo/reload 稳定；与 Handle 分层）；
**Inspector：一切设计数据修改入事务，编辑器会话态（hover/高亮/gizmo/
展开）不入**（Document State ≠ Editor Session State 的操作化）；
**Hierarchy：uid + 父 uid + 兄弟序 = 拖拽/移父/undo/保存的完整表达**
（Reparent 事务已备）。三模型全部建在 `uid + transaction + handle +
component` 之上，零新 VM 能力。

---

## 1. Selection Model

### 裁决

```text
Selection = Persistent uid 的有序集合
```

**不是** NodeHandle 集 —— Handle 因 reload / undo / tree rebuild 而
失效或漂移；uid 是编辑器语义身份（S9-0 Q6 身份跨事务连续的直接红利）。

### 性质

| 操作 | 行为 |
|---|---|
| undo 删除恢复 | 选择自动"回来"（uid 复活 → 集合里的 uid 再次命中）—— 无需重选 |
| redo 再删 | 选择条目变"悬空"（uid 在集合但无活节点）—— 保留不剔除（用户 redo 回来时选择恢复）；**悬空条目不参与 Inspector/命令** |
| reload | 集合里 uid 与新树的派生/落盘 uid 比对命中 —— 旧文件迁移派生的幂等性（T-ID-02）保证同文件同 uid |
| 多选 | 有序集（首元素 = 主选 —— Inspector 显示目标） |

### API 面（nes-scene，v1）

```rust
pub struct Selection { /* 有序 uid 集 */ }
Selection::new() / select(uid) / toggle(uid) / clear() / len()
Selection::uids() -> &[Uid]                      // 含悬空
Selection::live(&SceneTree) -> Vec<NodeId>       // 仅活节点（解析经 find_by_uid）
Selection::primary(&SceneTree) -> Option<NodeId> // 主选（活）
```

**不落盘**（D1：编辑态是运行时值 —— S8.4 编辑器演示的回写无痕口径延伸）。

## 2. Inspector Model

### 裁决

**入事务**（每次修改 = 一条 Modified 记录，可 undo）：

```text
name / local / process_mode / props / components（挂载变更）
```

**不入事务**（编辑器会话态，不产生历史）：

```text
hover / 选中高亮 / gizmo 拖拽中间态 / 层级面板展开 / 滚动位置 / 面板焦点
```

这是 **Document State ≠ Editor Session State** 的操作化：会话态
改变不进 undo 栈（否则历史被 UI 噪声淹没）；gizmo 拖拽的**落点**
（最终 local）入事务、中间帧不入（拖拽结束 commit 一次）。

### 修改路径

```text
Inspector 字段编辑
  → log.begin()
  → 记 Modified（before 快照）→ 写树 → after 快照 → record
  → log.commit()
```

（即 S9-2 的既有惯用法 —— Inspector 是事务的普通客户，无特权通道。）

## 3. Hierarchy Model

### 裁决

**uid + 父 uid + 兄弟序 = 结构的完整可逆表达**（S9-1 指纹已用同三
元组 —— 编辑器与确定性共用同一结构观）。

| 操作 | 表达 |
|---|---|
| 拖拽重排 | Reparent（同父新位）→ 一条 TxCapture::Reparented |
| 移动父节点 | Reparent（异父）→ 同上 |
| undo/redo | 事务既有语义（T-TX-05 已钉） |
| 保存/加载 | NodeDoc 树序即兄弟序（既有）；uid 恒写 |

**层级展示** = 前序遍历（树序），与 children() 的结构序一致
（S8.2b-2 冻结）—— 编辑器面板不另造排序。

## 4. 边界与遗留

| 事项 | 口径 |
|---|---|
| 选择悬空条目的可视化 | S9-3 实现层（灰显/隐藏 —— 会话态裁决） |
| 多选的框选/修饰键语义 | 编辑器 UI 层（模型只提供有序集 + toggle） |
| gizmo 中间态的事务合并 | 拖拽期 defer、drop 时一次 commit（实现层惯用法） |
| 选择变更的通知 | v1 轮询（宿主每帧查）；信号化（selection_changed）属后续 |
| 子场景内节点的选择 | 跨文件选择属子场景编辑会话（S9 后半） |

## 5. 记账

零代码；本契约 = S9-3 实现的验收条款。既有钉死测试映射：
T-TX-01..07（身份连续性 = 选择稳定性的基础）、T-ID-01/02（保存
重载/迁移幂等 = reload 后选择命中）、T-A-02（children 结构序 =
层级展示序）。

*（内容由AI生成，仅供参考）*
