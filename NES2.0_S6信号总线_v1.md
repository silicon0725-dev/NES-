# NES 2.0 · S6.14 信号总线 v1

> 交付日期：2026-10-01　｜　状态：**SignalBus 落地（帧末泵、同帧生效、级联带上限）**
> 前置：S6.2 tick 接线（泵是 tick 的一个阶段）。草案 §12 挂了六轮的遗留。

---

## 0. 一句话结论

`nes-scene` 新增 `Signal`（名字键 + `Value` 载荷 + 源节点）与帧末**信号泵**：
行为代码经 `NodeCtx::emit`（或宿主 `SceneTree::emit_signal` 预发）入队，
tick 在 process 之后、变换冲洗之前统一交付给
`SceneObserver::on_signal(&mut SignalCtx, &Signal)` —— 处理器的 Cmd **立即
落地且同帧进入冲洗**（信号触发的变更当帧入画，T-Tick-05 像素证据）；级联
（处理器再发射）以工作队列迭代交付（非同步递归），单帧上限
`SIGNAL_DELIVERY_CAP = 1024`，超出丢弃并计入 `TickStats::signals_dropped`。
出口准则 T-Sig-01..05 + T-Tick-05 全过。全仓测试 **111** / 34 / 42 / 40 /
83 / **26** 全绿，守卫 11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 与草案 §12 的对齐与分歧

| 草案口径 | 本轮落地 |
|---|---|
| 入队、帧末统一 flush | ✅ 泵点 = process 后、变换冲洗前（处理器 Cmd 同帧生效） |
| 禁止 emit 中同步递归 | ✅ 级联走工作队列**迭代**（先入队后交付），非同步重入 |
| `connect(src, sig, dst, method)` 订阅册 | **推迟** —— method 分发需要脚本 VM（`Script` 节点的 `registry_key` 挂载点，M5 兼容层）；当前交付给观察者、按名字过滤，宿主内部自行分发 |
| 连接表存 NodeId、销毁自动清理 | **不适用而消解** —— 观察者交付没有连接表，天然无悬挂连接（草案该条的动机即悬挂清理） |
| 同一根管道（可视化脚本事件 + Scratch 广播） | 形态已备：名字键 + 值载荷，兼容层将来直连 |

### 1.2 交付时序（泵点的选择）

泵放在 process 之后、`refresh_transforms` 之前：处理器的 `SetLocal` 等 Cmd
逐条落地，紧随其后的冲洗把它们算进世界矩阵 —— **信号触发的移动/属性变更
当帧入画**（T-Sig-03 断言 `world_position` 同帧、T-Tick-05 断言像素同帧）。
若放在冲洗后则晚一帧，若放在 process 前则收不到本帧发射 —— 唯此位置两全。

### 1.3 生命周期与上限

- 信号生命周期 = **单帧**：泵清空队列（宿主预发 + 各回调阶段发射 + 级联），
  跨帧留存请宿主自行存状态（与结构变更"帧首落地"对称）；
- runaway 级联（自激发）到 `SIGNAL_DELIVERY_CAP` 即丢弃并计数 —— 编程错误
  不该挂起帧循环，但必须被看见（`signals_dropped > 0` 可观测）；
- FIFO：交付顺序 = 发射顺序（含级联追加到队尾），确定性可断言。

### 1.4 SignalCtx（处理器句柄）

与 `NodeCtx` 同一形状（只读树 + Cmd 缓冲 + 再发射），但没有"当前节点"——
信号面向**通信**不面向节点身份；因此 `set_local`/`set_prop`/`queue` 以显式
`NodeId` 为参。Cmd 语义与 NodeCtx 完全一致（SetLocal/SetProp 立即生效、
结构变更延迟落地、属性写错静默忽略）。

## 2. API 面

| 成员 | 职责 |
|---|---|
| `Signal { src, name, payload }` | 一条信号（`Value` 载荷，值语义） |
| `NodeCtx::emit(name, payload)` | 行为代码发射（源自动填当前节点） |
| `SceneTree::emit_signal` / `pending_signals` | 宿主预发 / 泵前查询 |
| `SceneObserver::on_signal(ctx, sig)` | 交付回调（按名过滤） |
| `SignalCtx` | 处理器句柄：`tree()` / `emit` / `queue` / `set_local` / `set_prop` |
| `TickStats::signals_delivered` / `signals_dropped` | 泵对账 |
| `SIGNAL_DELIVERY_CAP` | 单帧交付上限（1024） |

nes-runtime 零改动（`frame_with` 透传观察者，信号随 tick 自动流动）。

## 3. 出口准则

### 3.1 场景层（`nes-scene/tests/s6_signal.rs`，5/5）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Sig-01 | process 发射 -> 帧末同帧交付；源自动填发射节点；载荷值语义；次帧（不发射）零交付 | ✅ |
| T-Sig-02 | 级联 a->b->c 同泵按发射序交付（迭代非递归） | ✅ |
| T-Sig-03 | 处理器 `set_local` 同帧落地并进入冲洗（`local` 与 `world_position` 均当帧可见） | ✅ |
| T-Sig-04 | runaway 级联恰好交付到上限、丢弃如实计数、不挂起；泵后清空 | ✅ |
| T-Sig-05 | 宿主预发本帧交付一次、交付后清空不重投；`pending_signals` 泵前可查 | ✅ |

### 3.2 运行时端到端（`nes-runtime/tests/criterion_tick.rs` 追加，1/1）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Tick-05 | on_process 发射 "go" -> on_signal 把精灵平移到 (26,10) -> **同一帧**像素在新位置（发射-交付-落地-冲洗-提取全链同帧） | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 订阅册 `connect/disconnect`（节点-方法级，随脚本 VM） | 未启动（§1.1 分歧表） |
| 节点销毁时信号源的可追溯清理（无连接表则无需清理；src 失效仅指无效 id，交付侧已只读） | 消解 |
| TreeEvent -> 信号桥（`tree/*` 自动入管道） | ✅ S6.15（见 `NES2.0_S6信号桥_v1.md`） |
| 编辑器信号监视器（`pending_signals`/统计面板） | 未启动 |
| WM_SIZE / DPI / headless Linux | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 111**（106 -> 111，+T-Sig 5）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  **nes-runtime 26**（25 -> 26，+T-Tick-05）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（tree.rs 信号结构/泵/ctx/统计 + 导出）与新测试；
  nes-runtime 零改动。

*（内容由AI生成，仅供参考）*
