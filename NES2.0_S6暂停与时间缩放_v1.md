# NES 2.0 · S6.4 暂停与时间缩放 v1

> 交付日期：2026-10-01　｜　状态：**草案 §9 的 ProcessMode/暂停/时间缩放语义落地（五模式 + 继承解析）**
> 前置：S6.2 tick 接线（派发口径就定义在 tick 的 process 阶段里）。
> 本轮把接口草案第 9 节的 `paused` / `time_scale` / `ProcessMode` 从纸面
> 变成 tick 派发规则 —— 全部落在 nes-scene，运行时零改动（暂停只是
> `tree_mut()` 上的一个开关）。

---

## 0. 一句话结论

`SceneTree` 新增 `paused` / `time_scale` 树级状态与节点级 `ProcessMode`
（五模式：Inherit/Pausable/WhenPaused/Always/Disabled），tick 的 process
阶段按**生效模式**派发。派发口径冻结：暂停时 `Pausable`（含 Inherit 解析）
**不派发**、`Always` 照常且 delta 不受影响、`WhenPaused` 仅暂停时派发且
**delta = 0**（时间冻结）、`Disabled` 永不派发；`time_scale` 只乘 delta、
不改遍历次数；生命周期与结构变更**不受暂停影响**。出口准则 T-Pause-01..08
（场景层）+ T-Pause-R1..R2（运行时接线）全过。全仓测试
**83** / 34 / 42 / 40 / 83 / **14** 全绿，守卫 11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 派发口径（tick process 阶段，冻结在代码注释里）

| 生效模式 | 未暂停 | 暂停时 | delta |
|---|---|---|---|
| `Pausable`（含 `Inherit` 解析结果） | 派发 | **不派发** | `delta × time_scale` |
| `Always` | 派发 | 派发（不受影响） | `delta × time_scale` |
| `WhenPaused` | 不派发 | 派发 | **0.0**（时间冻结） |
| `Disabled` | 不派发 | 不派发 | — |

草案原文"暂停不是停止遍历，是 `delta = 0` 且跳过 `Pausable` 的 `process`
—— 结构变更与信号仍然工作，否则暂停期间 UI 会僵死"的落地解读：

- **`delta = 0`** 落在 `WhenPaused` 上（暂停菜单节点要跑逻辑，但时间运动
  冻结）；`Always` 按草案"不受影响"保持真实缩放 delta；
- **遍历照常**：enter/ready/exit、结构落地（apply_pending）、变换冲洗都
  不看暂停位 —— 暂停期间 UI 可以增删节点、新节点照常完成生命周期
  （T-Pause-07/08 钉死）。

### 1.2 继承解析

`Inherit` 沿父链取最近的非 `Inherit` 祖先；整条链都 `Inherit`（含根、或
节点不在树上）解析为 `Pausable`（与 Godot 缺省一致）。`effective_process_mode`
是唯一解析点，tick 与 `NodeCtx::process_mode()` 都走它。

### 1.3 挂载点裁决：NodeData 一等字段（非属性表）

`process_mode` 是 `NodeData` 的字段，与 `local` 变换同级 —— 先例是 M2 的
"变换刻意不进属性表：固有的一等字段"。理由：它是**调度数据**，tick 每帧
每个节点都要读（含父链解析），走属性表意味着每帧 Value 解析；而"暂停菜单
要 Always"这类意图与"空间位置"一样属于节点的固有调度属性，不是编辑面板
上的设计时数据。

**裁决已定（S6.5 落地）**：序列化走 **`NodeDoc` 一等字段**（与 `local`
同一裁决的对称延伸：`NodeData.process_mode` ↔ `NodeDoc.process_mode`）。
缺省 `Inherit` 不写出（存量文件逐字节不变），枚举以稳定字符串名编码
（`"Always"` 等），未知值报语义错误（调度语义不是可容忍的前向兼容数据，
不静默回落）。属性表方案否决：调度数据每帧每节点要读含父链解析，且
enum 进 `Value` 只有 Str 松校验 / I64 不可读两难。详见 S6.5 文档。

### 1.4 time_scale 钳制

写入钳到 `[0, +∞)`（负时间无语义、宁可夹住）；`NaN` 回落 1.0（拒绝无定义
值进帧循环）。只乘 delta，**不改遍历次数**（确定性优先，草案原文）。

## 2. API 面（nes-scene）

| 成员 | 职责 |
|---|---|
| `ProcessMode`（enum，再导出） | 五模式 |
| `set_paused` / `paused` | 树级暂停位 |
| `set_time_scale` / `time_scale` | 树级时间缩放（钳制见 §1.4） |
| `set_process_mode` / `process_mode` | 节点自身设置（未经解析） |
| `effective_process_mode` | 继承解析后的生效模式（唯一解析点） |
| `NodeCtx::process_mode` | 行为代码自省生效模式 |
| `TickStats::process_skipped` | 未派发计数（暂停跳过 + Disabled），与 `processed` 相加恒等于遍历节点数 |

运行时（nes-runtime）**零改动**：`frame_with` 把原始 delta 交给 tick，
缩放与暂停全部在树内单点发生（T-Pause-R2 证明无二次缩放）。

## 3. 出口准则

### 3.1 场景层（`nes-scene/tests/s6_process.rs`，8/8）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Pause-01 | 缺省全 Inherit 解析为 Pausable：暂停即停、恢复即续；`processed + process_skipped` 对账恒等 | ✅ |
| T-Pause-02 | `Always` 暂停期照常派发、delta 不受暂停影响 | ✅ |
| T-Pause-03 | `WhenPaused` 仅暂停期派发且 delta==0；非暂停期不派发 | ✅ |
| T-Pause-04 | `Disabled` 无论暂停与否都不派发 | ✅ |
| T-Pause-05 | 继承解析：父 Always 下 Inherit 子照常跑、同父显式 Pausable 子跳过、链尾默认 Pausable | ✅ |
| T-Pause-06 | `time_scale` 只乘 delta 不改遍历次数；负值→0、NaN→1.0 | ✅ |
| T-Pause-07 | 暂停期结构变更照常：Always 节点回调 Spawn，下一帧落地并 enter | ✅ |
| T-Pause-08 | 生命周期不受暂停影响：暂停帧入树的节点照样完成 enter/ready（但 process 按模式跳过） | ✅ |

### 3.2 运行时接线（`nes-runtime/tests/criterion_pause.rs`，2/2）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Pause-R1 | 暂停 = 行为停跳 + 渲染照常：精灵冻结在暂停点，drawn/driver_errors 逐帧不变，恢复后从冻结点续走 | ✅ |
| T-Pause-R2 | `time_scale` 单点缩放：帧循环传原始 delta，观察者收到缩放值（无二次缩放） | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| `process_mode` 序列化口径（NodeDoc 一等字段 vs 属性表） | ✅ S6.5 裁决落地：NodeDoc 一等字段（见 `NES2.0_S6序列化口径ProcessMode_v1.md`） |
| 信号总线（SignalBus，草案 §9 同节提及） | 未启动 |
| `time_scale` 对提取层动画时间（FrameInfo.time）的口径 | 宿主侧自算，树内只管 delta（现状够用，文档化） |
| 子场景嵌套 / WM_SIZE / DPI / headless Linux | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 83**（75 -> 83，+T-Pause 8）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  **nes-runtime 14**（12 -> 14，+T-Pause-R 2）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（tree.rs 语义 + 新测试）与 nes-runtime（新测试）；
  提取层 / 渲染层 / scene_io **零改动**。

*（内容由AI生成，仅供参考）*
