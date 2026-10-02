# NES 2.0 · S7.0 → S8.2b 全链架构复盘 v1

> 交付日期：2026-10-01　｜　状态：**九个里程碑的全链审计：不变量清单 + 三条升格的架构原则 + 两条待裁 + S8.3 前置条件**
> 范围：S7.0 收束 / S7.1 语义冻结 / S7.2 输入 / S7.3 headless 确定性 / S7.4 首项目 / S8.0 生命周期 / S8.1 节拍 / S8.2 移植压力 / S8.2b 实体规模化（含 v3 验收）。

---

## 0. 一句话结论

全链审计结论：**没有发现需要回退任何已冻结语义的问题**；v3 验收的三条
过程发现升格为正式架构记录（§2）；两条遗留升格为待裁项（§3）；全链
不变量清单成文（§1）；集合 API 与 VM 语言面**封口令**下达（§4）；
S8.3 的前置条件与候选排序（§5）。当前基线：**450 测试 / 11 守卫 /
clippy 零**，游戏级 ABI（Dodge 基线）+ 双实测修复回归网在档。

---

## 1. 全链不变量清单（每条已由契约测试钉死）

| # | 不变量 | 来源 | 钉死测试 |
|---|---|---|---|
| I1 | 帧序：结构落地→桥→enter→ready→process（五模式表）→信号泵（宿主预发+内建 tick 最先、桥次之）→变换冲洗→提取→消费 | S7.1 | T-RS-01 |
| I2 | Cmd 微批次：回调返回即落地；级联读新值、自读旧值、末写胜；结构（Spawn/Tree）跨帧 | S7.1 | T-RS-04 |
| I3 | 信号：BFS 级联禁递归、上限 1024、注册序路由、路由处理器按目标生效模式门控（Disabled 永不、Pausable 暂停中跳过）；广播不受暂停影响 | S7.1/S6.x | T-RS-02、T-Sig 系 |
| I4 | 输入：平台→中性事件→折叠器（闩锁边缘/首帧基准）→快照；四路分立；WM_CHAR 非引擎 API | S7.2 | T-In-C/In/In-R 系 |
| I5 | 确定性：语义状态白名单指纹（**当前实现以 SceneTree preorder position 作为临时 canonical semantic identity —— 它不是 Persistent NodeId，后者建立后由其替换**；变换位形/属性/局部含句柄 resolve 结果）；句柄/数组/gen/布局**不进**指纹（T-H-04 实证 allocator history 免疫）；逐帧→轨迹哈希 | S7.3/S8.2b | T-HR、T-H-04、T-A-05 |
| I6 | headless = 同一运行时（GPU 装配缺席非语义分支）；CLI 与测试共用 run | S7.3 | T-HR 系 |
| I7 | init：首派发前执行一次；重挂载重跑；哨兵可观测 | S8.0 | T-LC 系 |
| I8 | 节拍：`every` 按固定模拟步执行（蓄步器/钳制）；内建 tick 每帧一次载荷帧号；**同一节点的第二脚本不得因 `every` 产生独立或额外的调度频率**（组件化 guardrail：一个实体挂 N 个脚本组件，`every` 调度次数仍 = 每模拟步一次/脚本，不多 tick） | S8.1 | T-LP 系 |
| I9 | 实体：NodeHandle 弱引用**可持有不保证存活**，两条 handle→N 通道（node()/Local 物化）统一过 resolve gen 校验；Name=查找/Handle=运行引用/NodeId=未来语义身份三层分离 | S8.2b | T-H 系 |
| I10 | 集合：Array 纯拥有（赋值深拷贝）；push/pop 变异局部绑定；for_each 快照迭代（禁改表允许改 it 实体）；children=结构序 | S8.2b | T-A 系 |
| I11 | 输入读面：同帧只读快照经共享视图到内建，零信号中转 | S8.2b-3 | T-IR 系 |
| I12 | ABI：Dodge 基线（场景+轨迹+期望哈希）压**合取语义**；漂移需评审 | S8.0 | T-ABI-01 |

## 2. 升格的架构原则（v3 验收三条过程发现 → 正式记录）

### P1 语言作用域 ≠ VM 槽位模型

```text
语言层：单名 it（内层遮蔽外层 —— lexical scope）
VM 层：it0/it1/...（编译器按深度映射）
```

> **编译器负责 lexical scope → VM slot 映射；脚本作者永远不见 itN。**

任何深度的嵌套由编译器处理。T-A-06 由 bug regression 升格为
**语言语义测试**（嵌套遮蔽 + 外层可见性是规范行为不是实现细节）。

### P2 Script Owner ≠ Script Host ≠ Semantic Entity

v3 再次实证：子脚本挂在精灵下，`this` 指脚本节点（执行上下文），
不是用户以为的精灵（语义实体）。当前解：自治脚本顶层具名。**裁决：
不把 this 扩展成"猜用户想要的那个 Sprite"**——那是把三层混回去。
四层身份各安其位：

```text
Name        = 查找便利
NodeHandle  = 运行时引用
NodeId      = 未来持久语义身份
Script Node = 执行上下文（this 的所指）
```

组件化/脚本资产复用时此契约为准。

### P2.1 `this` 的防猜测价值（第三次实证，S8.3-1）

旧 fly 挂弹丸子节点、`this.pos` 动的是隐形脚本节点 —— P2 阻止的正是
"自动猜用户想要的实体"这类隐蔽 runtime magic。修法不碰 `this`：
Controller + for_each + `it.pos` 即可表达。`this` = 执行上下文、
`node(h)` = 实体引用，两通道永不合流。

### P5 表达层不得创造运行时真相（S8.3-2 升格）

> 组件、快捷语法、编辑器便利结构，只能**展开为已有 Runtime
> Primitive**；任何新表达能力必须证明不能由现有 Instance/Handle/
> Collection 模型表达。

来源：S8.3-2 Component Expansion / D1 / I8 / P2。这是防止"为了方便
再造 ECS"的核心约束。

### G-COMP-01 组件调度守卫（I8 的组件版）

> 组件展开不得产生额外调度层；组件数量只增加 Script Instance 数，
> 不改变单实例调度语义。**Host 数 = Instance 数 = 调度数**。

T-C-02 钉死（3 组件 → 全树 process +3；删一 Host 恰 −1）。

### P4 复用的双形态裁决（S8.3-1 升格）

> **"脚本复用"与"实体行为复用"不是同一个问题。**

```text
行为完全相同         → Controller + Collection（for_each children）
入口/参数/生命周期不同 → 共享 .nes 资产 + 独立实例（产物共享状态独立）
```

看到"5 实体 × 5 份相同行为"默认先考虑 Controller，而非机械产生
Entity × ScriptInstance。选择条件是**状态所有权与生命周期差异**，
不是"代码像不像"。

### P3 "几何即状态"的正当形态（与 hack 的区分）

```text
hack：     状态所有权不存在 → 偷偷编码进另一个可观察字段
正当：     实体拥有 pos + 运动逻辑 + 生命周期判据，
          自治脚本与裁判都读同一几何 —— 坐标就是实体状态
```

同样的 `pos.x > -50`，架构意义由**状态归属**决定，不由写法决定。
禁的是前者，不是后者。

## 3. 待裁项（S8.3 前必须裁决，当前明确不动）

| # | 问题 | 当前口径 |
|---|---|---|
| D1 | Array 是否永远运行时值，还是未来进可持久化 Schema | 升级为强约束：**Array 是运行时值，不是 Project Model 类型**（即使未来跨脚本传递也仍是 VM runtime value）；Project Model 侧只有持久标量/属性与持久 NodeId，两域经显式转换边界；**不 为 NodeHandle 提前设计路径序列化** —— Persistent NodeId 先成立，才谈 Handle→持久身份 |
| D2 | Script Instance / Script Owner / Script Host / Semantic Entity / `this` 五术语的一次性定义（各是什么、生命周期、删除谁影响谁、`this` 可否转 NodeHandle） | P2 已立原则；D2 裁决时五术语边界先冻结再谈实现（不提前设计，但术语不得漂移） |

## 4. 封口令（明确不做）

1. **集合 API 封口**：`array/push/pop/len/index/for_each/children`
   即全部 —— 不加 map/filter/find/sort/remove/contains/reverse。
   加之前必须证明"实体模型不够"而不是"现代语言都有"。
2. **VM 语言面封口**：动态 emit 名、sqrt/abs 仍后置；渲染率脚本
   入口不引入（S8.1 已否决）。
3. **不因 for_each 成功而扩 API** —— 从"验证实体模型"滑向"建设
   通用脚本语言"会直接削弱本轮实验价值。

## 5. S8.3 前置条件与候选排序

**前置**：本复盘通过评审。

**候选按性质分三类（不并列排序，按先后）**：

1. **第二个不同类型项目（最重要）**——验证 Entity Handle + Array +
   children + for_each 是**通用运行时抽象**而非 Mini Dungeon 专用
   语法糖。**刻意选不同实体关系**：平台/机关/触发器、NPC+对话对象、
   背包/物品、RTS 编组、粒子/视觉实体、地图编辑器——**避开**
   Player/Enemy/Bullet/Collision/HP 同构。不必是完整游戏。
2. **同源多实例装载**（解决 v3 实证的 ×5 复用成本；S6.33 外置
   .nes 路径已支持，缺的是场景侧引用同槽位的惯用法/示例）。
3. **脚本组件化**（组织/复用模型 —— 最大的一步，须待 1 的结果）。

## 5.5 S8 收官补记（S8.3-2..S8.4 评审后）

S8 正式定名：**Runtime Entity Model & Behavior Organization
Validation**（非"实体系统"）—— 验证了四件事：

```text
Entity Identity/Handle → Array/children/for_each → Script
Instance/Asset → Component Expansion
```

三象限证据齐备（战斗 / 拓扑触发 / 编辑资源），**均零新 VM 原语**。
`this`=Host 的**第四次实证**（编辑器组件）与 Tab 重按需先 key_up
（闩锁边缘）作为边界证据保留（非 bug）。S8 = **FROZEN / CLOSED**
（454 测试 / 11 守卫 / clippy 零 / wgpu 89 逐二进制 / D2·P5·
G-COMP-01 冻结 / VM 语言封口）。

**S9 裁决（评审定调）**：不做第二个完整游戏，先进 **S9-0
Persistent Identity & Editor Mutation Contract**（编辑器拐点：
preorder 身份在 delete/rename/undo 面前不稳定 —— Persistent
NodeId 从"未来设计"变为"实际工程需求"）；路线 S9-0 契约 →
S9-1 对象模型 → S9-2 变更/事务 → S9-3 Inspector/选择/层级。

## 5.6 S9 收官补记（S9-3a..S9-3b 评审后）

S9 正式收官：**Persistent Identity → Transaction → Editor Object
Model** 全链闭环。核心架构验证：**Editor Shell 是投影层** —— UI
只消费 `uid + TransactionLog + Selection/Inspector/Hierarchy`，
零自有状态零自有语义。S9 **没有给 VM 增加任何语言能力**：
S8 解决"Runtime Entity Model 能否支撑真实项目"，S9 解决"这个
Runtime/Project Model 能否成为真正编辑器的数据基础"—— 都已
经实现 + 回归网验证。

```text
Persistent UID ──┬── Runtime Handle（arena resolve）
                 ├── Transaction History（原 uid 复活）
                 └── Editor State（Selection/Inspector/Hierarchy）
                          ↓
                    Editor Shell（Projection）
```

后续编辑器扩展（多选/框选/Gizmo/Inspector 控件/子场景跨文件事务/
通知信号化/自动拦截/VM locals undo/协作 merge）全部后置 ——
都不属于 S9 完成条件。扩展纪律：**UI 是投影，Editor Core 是操作层，
Transaction 是历史，SceneTree 是结构，uid 是身份**。

## 5.7 S10 收官补记（S10-0..S10-2 评审后）

S10 正式关闭：**Cross-Project Calibration**——Seed & Harvest
（非战斗完整游戏）校准 API 摩擦五项 → hit + timer 两项 ★★★ API
落地 → 编辑器五项（点击/多选/框选/Gizmo/保存）全部建在 S9 冻结
语义上零引擎改动。**跨项目交叉验证成立**：Dodge（战斗）+ Seed &
Harvest（生产/计时）+ 编辑器（对象操作）三种状态组织方式均可在
不增加 VM 语义的情况下表达。S10 = CLOSED（474 tests / 11 guards
/ clippy 0）。

**S11-0 裁决**：先做 API 摩擦重评（零代码——统计全部项目中
F-1/F-4 的真实出现频次），再决定后续方向。纪律不变：M1。

## 6. 复盘结论

- 九里程碑无回退项；三条新原则、两条待裁、两道封口令入档；
- **元不变量（M1，长期工程原则）**：运行时语义必须经至少一个
  非平凡真实项目验证；真实项目暴露的语义缺口必须**先进入回归网**，
  再继续扩展下游能力。本链完整跑过一轮：Dodge→缺口→S7.x 修复/
  冻结→Mini Dungeon v2→S8.2b→v3→发现 nested for_each/this→回归
  测试→架构原则升格。这个闭环比 450 测试本身更值得保留；
- 风险登记：450 测试的增长曲线本身不是目标 —— 下阶段的验收
  语言是"第二个项目里 API 是否自然"，不是测试数量。

*（内容由AI生成，仅供参考）*
