# NES 2.0 · S6.17 订阅册 v1

> 交付日期：2026-10-01　｜　状态：**connect/disconnect 路由层落地（含节点销毁自动清理）**
> 前置：S6.14 信号总线、S6.16 订阅过滤。草案 §12 原初形态的诚实子集。

---

## 0. 一句话结论

`SceneTree` 新增订阅册：`connect_signal(名字, 可选源节点, 目标节点) ->
SignalConnectionId` / `disconnect_signal(id)` / `signal_connections()`。
信号交付 = **广播一次**（既有）+ **每条命中连接路由一次**（目标节点作
`SignalCtx::dst()` 上下文，注册序，广播之后）；名字精确匹配、源兼容匹配
（`src=None` 连接匹配任意源含桥信号的引擎源）。源/目标节点销毁时连接
**自动清理**（tick 阶段 1 修剪 —— 草案"连接表只存 NodeId"条款的落地）。
出口准则 T-Sig-13..16 全过。全仓测试 **122** / 34 / 42 / 40 / 83 / **26**
全绿，守卫 11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 路由层（与草案 connect 的对齐与边界）

草案 `connect(src, sig, dst, method)` 的订阅者是"节点 + 方法名"—— 方法
分发需要脚本 VM。本轮落的是**路由层**：`(名字, 可选源) -> 目标节点`，
命中给观察者一次**带目标上下文**的交付（`dst() == Some(dst)`），观察者
按 dst 分派行为；方法级分发仍归脚本 VM（`Script` 节点 `registry_key`
挂载点，届时 `method` 只是 dst 上的一张属性表）。

### 1.2 交付账目（一条信号可交付多次）

- 每条到达泵的信号（过了 `signal_filter`）：**广播 1 次 + 每条命中连接
  1 次**，全部计入 `signals_delivered` 并同守 `SIGNAL_DELIVERY_CAP`
  （每次调用都是真实处理器）；`signals_routed` 单独计路由子集；
- 顺序：同一信号先广播后路由（注册序）；路由发射的级联照常入队队尾；
- S6.16 的对账恒等式在无连接时不变（`routed = 0`）；有连接时
  `delivered` 可超过发射数 —— 这是路由的语义，不是账目错误
  （`signals_routed` 使两种成分可分离）。

### 1.3 自动清理（草案条款的落地时机）

tick 阶段 1（结构落地后）**修剪**：源或目标节点 arena 查无（销毁 ——
代际即身份，NodeId 永不复用）的连接移除。推论：被删节点的连接最多
"死"到本帧帧末，下一帧泵前必已清理；宿主 `disconnect` 早于修剪也无害。

### 1.4 约束

- 名字为空或目标节点不存在 -> `connect_signal` 返回 `None`（如实拒绝，
  不注册哑连接）；占位未落地的节点（arena 已占位）可连接；
- `disconnect_signal` 幂等：未知句柄返回 `false`；
- 句柄计数器只增不复用（与 ResId 同纪律）。

## 2. API 面

| 成员 | 职责 |
|---|---|
| `SignalConnection { id, name, src, dst }` | 一条连接（pub 字段，册视图可检视） |
| `SignalConnectionId(u64)` | 句柄 |
| `SceneTree::connect_signal` / `disconnect_signal` / `signal_connections` | 注册 / 注销（幂等）/ 只读视图 |
| `SignalCtx::dst()` | 路由交付的目标上下文（广播为 `None`） |
| `TickStats::signals_routed` | 路由交付计数 |

nes-runtime 零改动。

## 3. 出口准则（`s6_signal.rs` 追加，4/4）

| 编号 | 契约 | 结果 |
|---|---|
| T-Sig-13 | 路由交付：广播（dst=None）在前、命中连接按注册序（dst=Some）；`signals_routed`/`signals_delivered` 账目 | ✅ |
| T-Sig-14 | 源过滤：src=Some(a) 只被 a 的发射命中（宿主源不命中）；src=None 连接命中桥信号（引擎源） | ✅ |
| T-Sig-15 | disconnect：移除后不再路由；重复注销返回 false | ✅ |
| T-Sig-16 | 节点销毁自动清理：删源/目标 -> 册清空（视图可见）；再发射无路由不崩溃 | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 方法级分发（`method` -> 处理器表） | ✅ S6.18（见 `NES2.0_S6方法级分发_v1.md`；脚本 VM 将在处理器表注册解释器闭包） |
| 行为代码侧 connect/disconnect（`SignalCtx` 目前只读册） | 未启动（编辑器/宿主侧优先） |
| 连接载荷/优先级/一次性订阅（once） | 未启动 |
| WM_SIZE / DPI / headless Linux | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 122**（118 -> 122，+T-Sig-13..16）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  nes-runtime 26 —— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（连接类型/册/修剪/泵路由层/统计 + 导出）与新测试。

*（内容由AI生成，仅供参考）*
