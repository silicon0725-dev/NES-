# NES 2.0 · S8.2b 实体规模化 API · 设计冻结 v1

> 交付日期：2026-10-01　｜　状态：**设计冻结（评审裁决 + 实现陷阱预排）；实现分 b-1/b-2/b-3 三步走，先重构 Mini Dungeon 再谈第三个项目**
> 前置：S8.2 移植压力报告（缺口实证）；评审裁决（本轮用户反馈）。

---

## 0. 裁决汇总（评审定调）

1. **第一优先级 = 实体句柄 + 集合**（运行时对象模型问题），不是动态
   signal/数学（语言表达问题）。
2. **语义层不以整数为实体身份**：`NodeHandle` 是独立类型（内部
   `NodeId{slot,gen}` 位副本）；I64 只允许出现在脚本 ABI 边界，不作
   语义身份。与 identity.rs 既有纪律（"悬垂引用变 None 而不是撞上
   复用槽位的另一个节点"）同源。
3. **不做 ECS**：要的是 `Array<Value>` / `Array<NodeHandle>` /
   `for_each`，不是 Entity/Component/Archetype/Query/System。
4. **输入共享读面**（b-3）：`input` 已是 Frame Snapshot（S7.2），
   脚本理应有稳定的**同帧只读**入口（`key()` 已是此形态 —— 扩到
   鼠标/按钮），而不是"信号 → 某脚本局部 → 别人看不见"。
5. 动态 emit（b-4）、sqrt/abs（b-5）后置；b-1/b-2 完成后动态 emit 的
   实际需求预计下降。
6. **下一个动作不是第三个游戏**：用新 API **重构 Mini Dungeon**，
   量化对比（§5 指标），再拿不同类型的第二项目验证 API 未被过拟合。

## 1. b-1 实体句柄（设计）

### 1.1 值面

```rust
// value.rs
Value::Node(NodeHandle)      // 新变体；ValueType::Node
NodeHandle                   // identity.rs 既有类型升格：
  - 语义：脚本可见的弱句柄（位副本），**解析时验证存活**（gen 防
    悬垂撞名）；文档口径从"帧内有效"升格为"跨帧有效、解析时校验"
  - 局部/数组可持有；**不进属性表**（schema 按 ValueType 校验，
    Node 不在 PropDesc 类型集 -> 脚本写节点值到属性走既有静默丢弃
    路径）；**不序列化**（场景文件里节点引用 = 路径，那是 scene_io
    的既有口径，两者不混）
```

### 1.2 栈语义（复用 N/V 二象性 —— 已踩实的陷阱）

VM 栈已有 `StackVal::N(NodeId)`（NodeByName 的产物，成员读写指令
全部消费它）。句柄进局部只要两条**边界强制转换**规则：

| 边界 | 规则 |
|---|---|
| `Op::Local`（读局部） | 值为 `Value::Node(h)` → 压 `N(h.to_id())`（后续 `.pos`/属性指令直接可用） |
| `Op::SetLocal` / 数组元素写入 | 栈顶为 `N(id)` → 存 `Value::Node(of(id))` |

`Eq` 增 `Node×Node` 按位比较（身份等式）。指纹（determinism.rs
`mix_value`）增臂：位形（gen 参与哈希 —— 句柄即语义状态）。

### 1.3 文法（已踩实的陷阱：`x.pos` 是**编译期名字解析**）

`x.pos` 现文法把 `x` 编译为 `NodeByName`——局部**不能**用成员语法
解引用（编译器无类型推理）。裁决：

- 新内建 `node(e)`：`Str` → 按名解析压 `N`；`Node` → 校验存活后
  压 `N`；其余停机。
- **后缀链限定形态**：`node(...)` 调用之后允许 `.member` 链
  （先例：`.pos` 后的 `.x/.y`，S7.4 的 `pos_component`）——
  `node(h).pos.x` 合法；任意表达式后缀 `(expr).x` **明确不支持**
  （静默不支持不如不支持，与 S7.4 同一条裁决）。
- 赋值目标扩展：`node(...).member = expr`（语句层 lvalue 增这一种
  形态）。裸标识符成员 `x.pos` 语义不变（仍是节点名）。

### 1.4 spawn 的分配陷阱与 v1 口径

`spawn` 返回句柄要求**即时分配** arena 槽位 —— 但回调持有只读树，
`&mut arena` 与 `&self` 借用冲突（tick 的 ctx 构造是 `&*self` 整体
只读）。候选方案（按优先序，b-1b 再裁）：

- **A 预留协议**：`Arena::reserve() -> NodeId`（只递增计数不插入），
  ctx 经命令缓冲侧信道预约；`Cmd::Spawn` 携带预留 id，apply 时按
  预留 id 插入。借用侧信道 = ctx 里的 `&mut Vec<NodeId>` 预约栈
  （与 cmds/signals 并列，不碰 tree）——**最干净，待 arena 改造**。
- **B 名字票据**：spawn 带显式名，句柄下帧经 `node("名")` 取回
  （名字唯一化保证可寻）——零借用改动，但"spawn 当帧不可配"。

**b-1 v1 裁决：先落 B（名字票据）+ `node()`/局部句柄/成员链**——
"按名收集一次、句柄驱动 thereafter" 已解锁实体规模化（数组 +
for_each 在 b-2 把它变成惯用法）；A 作为 b-1b 升级（spawn 即返句柄）
单独立项，不阻塞主线。

## 2. b-2 集合原语（设计）

```text
Array<Value>（元素含 NodeHandle）
内建：array() 空表 / push(a, v) / pop(a) / len(a) / a[i]（既有索引
      语法复用？StrIndex 是字符串 —— 数组索引新臂）
for_each(a) { ... }        // 循环变量 it（局部语义，迭代中禁改表）
children(h) -> Array       // 子节点句柄表（实体枚举的引擎侧入口）
```

陷阱预排：迭代器协议走**快照拷贝**（迭代中 push 的可见性 = 下次
迭代 —— 与 Cmd 微批次同哲学：遍历序确定、当次迭代看不见结构变更）；
数组是值语义（赋值拷贝）还是引用语义 —— **裁决：引用语义经局部
别名太隐晦，v1 取值拷贝 + for_each 内建持有**（数组不作为可变共享
容器；跨脚本共享仍走信号 —— 别把数组变成第二个隐藏状态总线）。

## 3. b-3 输入共享读面（设计）

`ScriptVm` 的探针槽已共享**整份** `InputSnapshot`（S7.2 的
mount_key_probe 接的就是它）—— 把探针从 `Fn(&str)->bool` 升格为
`Fn(&str) -> Option<Value>` 或并列读函数：

```text
key("W")            // 既有
mouse_x() mouse_y() mouse_dx() mouse_dy()
button("left")      text_len()
```

全部**只读、同帧、零信号消费** —— 与 S7.2 架构零新增面（同一份
快照，多几个读口）。裁决：不做 `input.mouse.position` 属性面
（属性表是 schema 封闭的，评审既定）—— 内建函数口径。

## 4. 实现排期（每步带契约测试 + Mini Dungeon 增量重构）

| 步 | 面 | 测试 |
|---|---|---|
| b-1 | Value::Node + 边界转换 + `node()` + 成员链 + 名字票据 spawn（既有 spawn 不变） | T-H-01..04（局部持有/成员读写/悬垂校验/Eq+指纹） |
| b-2 | Array + for_each + children | T-A-01..05 |
| b-3 | 输入读面内建 | T-IR-01..02 |
| 重构 | Mini Dungeon v3：多弹道 + 句柄驱动 + 波次 | §5 指标 + T-GP-03 |

## 5. 重构对比指标（冻模板）

| 指标 | v2（现状） | v3（新 API） |
|---|---|---|
| 脚本数 / 总字符 | 6 / ~2600 | |
| 最大单脚本字符（管理器复杂度） | referee ~1100 | |
| 状态绕行数（几何当状态/信号转传） | 3 | |
| 并发弹丸 | 1 | |
| 实体规模上限 | 3 敌（具名） | |
| emit 样板分支 | — | |

## 6. 记账

- 本文档为设计冻结（零代码）；实现自下一轮起按 §4 排期。
- Mini Dungeon 正式记为 **NES 2.0 第一次"实体规模化"压力测试**
  （分水岭：单对象脚本模型 → 多实体游戏模型）。

*（内容由AI生成，仅供参考）*
