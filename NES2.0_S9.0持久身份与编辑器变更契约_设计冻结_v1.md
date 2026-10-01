# NES 2.0 · S9-0 持久身份与编辑器变更契约 · 设计冻结 v1

> 交付日期：2026-10-01　｜　状态：**十问裁决的契约文本（S9-1 编辑器对象模型的实现前置）；零代码**
> 前置：S8 CLOSED（三象限实体模型验证）；S7.3 指纹的"前序 = 临时 canonical identity"；D1/D2 冻结。

---

## 0. 一句话结论

回答评审十问，核心裁决：**Persistent NodeId = 128 位内容无关 UUID**
（生成即永久，跨保存/加载/clone/duplicate 均不复用）；**undo/redo
恢复原 NodeId**（事务日志记录对象全量，redo 重放 = 同一身份回归）；
**三身份严格分层**（runtime NodeId = 执行安全 / Persistent NodeId =
语义身份 / NodeHandle = 弱引用）；**指纹切换时机 = S9-1 首个编辑器
对象落地的同一次里程碑**（一次性切换，不留双轨）；**序列化稳定 =
NodeDoc 一等字段 `uid`，缺省回退前序派生**（存量文件兼容）。

---

## 1. 十问裁决

### Q1 Persistent NodeId 的生成规则？
**128 位 UUID（v4 随机）**，存 `NodeData.uid: Uuid`（一等字段，与
`local`/`process_mode` 同层 —— 调度/空间/身份都是固有数据，不进
属性表）。生成时机 = 节点创建（`add_node`/`spawn` 落地帧）；宿主
可显式指定（`add_node_with_uid`—— 加载/粘贴/undo 用）。冲突
（同 uid 两活节点）= 装载时如实报错，不做静默重编。

### Q2 跨保存/加载稳定？
**稳定**。序列化为 NodeDoc 一等字段 `uid: "..."`（十六进制）。
**缺省回退**：旧文件无 uid → 加载时按**前序位置确定性派生**
（v1 内容哈希 + 前序路径）并**仅写入内存 NodeData**——当前运行期
稳定；**用户/系统正常保存时 uid 才正式落盘**（加载阶段不写磁盘，
不触 mtime —— 与 S9-D3 统一）。派生规则保证"同一旧文件两次加载
得到同一 uid"（幂等，指纹不漂移）。

**uid 内容无关性（评审钉死）**：name/path/parent/position/
component/asset **任一都不得参与 uid 生成**。两个生成机制严格分开：

```text
新对象   → 随机 UUID v4
旧对象迁移 → 确定性兼容派生（仅迁移用）
```

而非"所有 NodeId = hash(节点内容)"——否则改名/移动/改组件都会
污染身份。

### Q3 clone 的 NodeId？
**新生成**。clone = 复制语义（"另一个对象"）。深拷贝子树的所有
uid 全部替换为新 UUID；克隆关系若需保留（编辑器"克隆自"），走
**显式属性/元数据**，不藏在身份里。

### Q4 duplicate（场景内复制粘贴）？
同 Q3：**新身份**。粘贴板持有 NodeDoc（含 uid），落地时全部重编。
"原位复制"（同 uid 两实例）被 Q1 的冲突检测禁止。

### Q5 delete 后 NodeId 永久死亡？
**语义身份规则的精确表述**（评审澄清，与 Q6 闭合）：

```text
delete 正常提交：该 uid 进入"不可被任何新对象生成/复用"的死亡状态
undo 删除事务：原对象以原 uid 恢复（身份随事务回滚回归，非新分配）
```

即：**"永久死亡" ≠ 数据不可恢复，而是"禁止新身份占用"**——
一旦 uid 分配给某对象身份，不允许被另一个新对象重新占用（Q1 的
冲突检测兜底）；删除事务本身可被 undo（Q6），恢复的是**原身份**。
回收不做（uuid 空间 2^128 无需）。

### Q6 undo/redo 恢复原 NodeId 还是重新生成？
**恢复原 NodeId**。裁决依据：编辑器数据模型 = **事务日志**
（transaction = 一组对象的全量快照变更）；undo = 逆事务（被删对象
按原 uid 复活——`add_node_with_uid` 路径），redo = 重放（同一身份
回归）。**身份是跨事务连续的**——否则"选中 C → 删 B → undo → C
仍被选中"这类编辑器基本体验会碎。NodeHandle（弱引用）在 undo 后
自然恢复指向（resolve 按 uid 而非 arena gen —— 见 Q8 边界）。

### Q7 NodeHandle 与 Persistent NodeId 的转换边界？
**显式、单向、运行时侧**：`NodeHandle`（弱引用，resolve 校验）可
升级为 Persistent NodeId（`uid_of(h)`—— 读 Host/Entity 的 uid）；
反向 = 按 uid 查找（`node_by_uid(u)` → Handle）。**D1 边界不动**：
uid 是 Project Model 的持久标量；Handle 是运行时值。脚本层 v1
不暴露 uid（先给编辑器宿主；脚本仍走 Name/Handle）。

### Q8 三身份严格区分？

```text
runtime NodeId (slot, gen)   = 执行安全（arena 代际防悬垂）
Persistent NodeId (uuid)     = 语义身份（跨会话/跨事务稳定）
NodeHandle                   = 弱引用（可持有，解引用校验）
```

resolve 边界扩展：**Handle 的 gen 校验照旧**（S8.2b-1 不变）；
uid 查找是**另一条通道**（`node_by_uid`），不经 Handle 也不替代它。
指纹用 uid（Q9）；执行安全仍用 gen。**两者永不混用**。

### Q9 指纹何时从前序切到 Persistent NodeId？
**S9-1 里程碑内一次性切换**（首个编辑器对象落地时）——`mix_value`/
`scene_fingerprint` 的节点身份臂从 `前序 index` 改为 `uid 位形`
（128 位全哈希）。**不留双轨、不加兼容开关**：切换即全量重生成
Dodge 基线哈希（ABI 按协议评审更新——这是"有意变更"的标准流程）。
别名折叠语义不变（同 uid 两 Handle 同指纹）。

### Q10 序列化如何保证 NodeId 稳定？
- `NodeDoc.uid: Option<String>` 一等字段；`compact` 模式**写出**
  （身份不省略——与 `local` 可省不同：省略即回退派生，代价是身份
  依赖树形，恰是我们要摆脱的）；
- 回写恒定：加载（派生或读取）→ 内存 uid → 保存（原样写出）；
- 组件展开子节点（`comp{n}`）的 uid 同规则生成，回写恒空不变
  （P5：展开是单向语法糖）。

## 2. S9-1..3 路线（预告）

| 阶段 | 面 | 依赖本契约 |
|---|---|---|
| S9-1 | uid 字段 + 派生回写 + `node_by_uid` + **指纹切换** + Dodge 基线重生成 | Q1/Q2/Q7/Q8/Q9/Q10 |
| S9-2 | 事务日志（undo/redo 全量快照变更、原 uid 复活） | Q3/Q4/Q5/Q6 |
| S9-3 | Inspector/选择/层级（选择 = uid 集，不落盘口径照 D1） | 全部 |

### 2.1 S9-1 核心测试（评审指定，实现时落 T-ID-01/02）

**T-ID-01 身份稳定性矩阵**——Identity 对所有**非身份操作**稳定：

```text
创建 A → 保存 → 重加载            → uid 相同
改名 / 移动父节点 / 改属性 / 增删兄弟 → A uid 相同
删除 A → 新建 B                   → B 不得获得 A uid
clone A                           → clone uid != A uid
clone 保存 → 重加载                → clone uid 不变
```

**T-ID-02 旧文件迁移幂等**：

```text
旧无 uid 文件 → 第一次加载得 U（仅内存）→ 不保存再加载 → 仍 U
保存 → uid 落盘 → 再加载 → 仍 U
```

## 3. 待裁项

| # | 问题 | 建议口径 |
|---|---|---|
| S9-D1 | uid 是否进资源引用（纹理/脚本按 uid 而非槽位）？ | 不进（槽位是场景内相对引用，uid 是节点身份——不同物） |
| S9-D2 | 多人协作的 uid 冲突合并策略 | S9 后半（单机编辑器先行） |
| S9-D3 | uid 派生回写是否改文件 mtime 触发热重载循环 | 加载侧内存派生不回写盘（保存时才落）——避免环 |

## 4. 记账

零代码；本契约 = S9-1 实现的验收条款。既有钉死测试映射：
T-H-04（指纹对 allocator 历史免疫 —— uid 切换后此测试语义升级为
"对 uid 无关的内部布局免疫"）、T-ABI-01（基线重生成流程）、
T-SMI/T-C 系（Instance 独立性不受身份层影响）。

*（内容由AI生成，仅供参考）*
