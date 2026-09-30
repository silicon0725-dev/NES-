# NES 2.0 · S6.15 信号桥 v1

> 交付日期：2026-10-01　｜　状态：**TreeEvent -> `tree/*` 桥信号（结构事件自动入信号管道）**
> 前置：S6.14 信号总线。草案 TreeEvent 文档本就写着"用于日志、编辑器刷新、
> 以及将来 SignalBus 的上游" —— 本轮把"上游"接通。

---

## 0. 一句话结论

tick 阶段 1（结构落地）的每条 `TreeEvent` 现在以 `tree/*` 桥信号进入信号
泵：`on_tree_event` 即时回调照旧（**双通道不互斥**），桥信号帧末交付、
携带**事件原文**（`Signal.event`）、引擎源（`src = None`）、泵序最前。
出口准则 T-Sig-06..08 全过（含"处理器响应桥信号再改结构 -> 下一帧桥回流"
的跨帧闭环）。全仓测试 **114** / 34 / 42 / 40 / 83 / **26** 全绿，守卫
11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 事件原文随行（不编码进 Value）

`Signal` 新增 `event: Option<TreeEvent>`：桥信号携带事件**原文**，用户信号
恒 `None`。裁决理由：`TreeEvent` 装的是 `NodeId`（槽位+代际），塞进
`Value`（F32/I64/Str/Vec2/...）只能硬编码槽位或拆两个信号 —— 槽位没有
代际是**身份谎言**，拆散则一次事件两次交付。原文随行类型安全、一次交付；
`payload` 对桥信号为 `Bool(true)` 占位（事实在 `event`，文档即契约）。

### 1.2 双通道不互斥

`on_tree_event`（即时、阶段 1）与 `tree/*` 桥信号（帧末泵）同时送达。
即时通道给"落地当刻就要读树"的消费者；信号通道给"与用户信号统一处理"
的消费者。桥信号**泵序最前**（结构落地是帧内最早阶段，事件先于任何
process 发射 —— T-Tick-05 更新后的断言顺序即此契约）。

### 1.3 覆盖范围与回流

- 桥只覆盖 **tick 管道内**的结构落地（阶段 1）。宿主在 tick 外直接
  `apply_pending()` 拿到返回的事件 Vec，不入泵（无帧可交付）；
- 信号处理器经 `SignalCtx::queue` 排的结构变更延迟到**下一帧**阶段 1
  落地 -> 该帧桥发出对应 `tree/*` —— 事件驱动的结构变更**跨帧闭环**
  （T-Sig-08），且每帧至多一批新事件，不构成 runaway。

### 1.4 稳定名

`TreeEvent::signal_name()`：`tree/added` / `tree/removed` /
`tree/reparented` / `tree/renamed` / `tree/moved` / `tree/name_adjusted` /
`tree/rejected` —— **不得随重构改名**（消费者按名过滤，与
`ProcessMode::as_str` 同一纪律）。

## 2. 实现落点

| 位置 | 改动 |
|---|---|
| `TreeEvent::signal_name()` | 变体 -> 稳定名（穷尽 match） |
| `Signal.event` | 事件原文字段（三处内部构造补 `None`） |
| tick 阶段 1 | `on_tree_event` 循环里同步推桥信号进 `emitted` |

nes-runtime 零改动。

## 3. 出口准则

### 3.1 场景层（`s6_signal.rs` 追加，3/3）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Sig-06 | 挂起变更 tick 落地 -> `on_tree_event` 与 `tree/added` **双通道同帧**；事件原文逐字段相等；src = None | ✅ |
| T-Sig-07 | 全变体映射 + 顺序：重名自动调整/改名/换位批次 -> 桥信号按事件序、名字一一对应、原文可判别 | ✅ |
| T-Sig-08 | 处理器响应桥信号（借原文拿节点）排队改名 -> 下帧 `tree/renamed` 回流 -> 再下帧静默（不 runaway） | ✅ |

### 3.2 运行时（`criterion_tick.rs` T-Tick-05 更新）

首帧泵序断言升级为 `[tree/added, tree/added, go]` —— 桥在前、用户信号
同帧交付（S6.14 的"信号触发移动当帧入画"结论不变，桥信号多了两条前置）。

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| tick 外 `apply_pending` 的事件入泵口径（无帧可交付，当前不桥） | 文档口径（§1.3） |
| 桥信号订阅过滤（按 `tree/*` 前缀注册，省去全量 on_signal 调用） | 随订阅册（脚本 VM） |
| 编辑器事件监视器（桥信号 + `signals_dropped` 面板） | 未启动 |
| WM_SIZE / DPI / headless Linux | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 114**（111 -> 114，+T-Sig-06..08）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  **nes-runtime 26**（T-Tick-05 断言升级）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（TreeEvent 方法、Signal 字段、阶段 1 接桥）与测试。

*（内容由AI生成，仅供参考）*
