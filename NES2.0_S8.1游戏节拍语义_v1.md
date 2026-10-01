# NES 2.0 · S8.1 游戏节拍语义（Game Loop Semantics）v1

> 交付日期：2026-10-01　｜　状态：**三权分立冻结：`every`（固定步长模拟步）× 内建 `tick`（帧级确定性事件）× 帧路径（渲染帧）—— 引擎拥有节拍，宿主不再手搓**
> 前置：S7.1 黄金帧序（宿主预发位）；S7.4 压力图 #1（`on "tick"` 手搓节拍是三条正确规则的合取效应）。

---

## 0. 一句话结论

游戏节拍收归引擎：**内建 `tick` 信号**（帧路径与 headless 共用同一
`simulate` 实现，每帧恰一次、载荷 = 树帧号 —— 宿主不再手发，referee
类脚本直接用 `arg` 当帧计数）+ **固定步长蓄步器**（`set_fixed_step`：
变帧率渲染 × 固定率模拟；快帧 0 步余量进位、慢帧补步、螺旋钳制 5 步
/帧 + 丢弃如实计数）。三权分立冻结：`every` = **确定性模拟步**
（delta 恒等于 step，运行时保证）；`tick` = **帧级确定性事件**；
帧 = 渲染节奏（提取/消费每帧一次）。**不新增第二脚本入口率**（单模拟
率是确定性根基；渲染率 process 属未来子系统出现时的新冻结）。ABI 按
协议重生成（`070713bc082dca22`，含评审注记）；S6.15 泵序测试按 S7.1
黄金序补齐内建 tick 位。全仓测试 **435**（34 / 180 / 44 / 42 / 88 / 47）
全绿，守卫 11/11，clippy 零警告。

---

## 1. 三权分立（冻结）

```text
帧（渲染节奏；宿主帧循环）
 │  collect_input → emit_input_signals
 │  【内建 tick：每帧恰一次，载荷 = 树帧号（宿主预发位，泵序最先）】
 │  蓄步器分步：N × SceneTree::tick(step)     ← N ∈ {0..5}，余量跨帧
 │  extract → consume（每帧一次）
 ▼
┌─ every（process）：模拟步 ─────────────────────────────┐
│ delta 恒 = 固定步长（set_fixed_step 后运行时保证）      │
│ 对象自身连续行为 + 确定性模拟 —— 与渲染帧率解耦          │
└─────────────────────────────────────────────────────────┘
┌─ tick（内建信号）：帧级事件 ────────────────────────────┐
│ 每帧恰一次；UI/触发器/裁判（Dodge referee 用 arg 计帧）  │
│ arg = 树帧号（0 基）—— 宿主级确定性事件                 │
└─────────────────────────────────────────────────────────┘
```

### 1.1 裁决表

| 裁决点 | 口径 |
|---|---|
| `every` 的角色 | **固定步长模拟步**。未设 `set_fixed_step`（缺省）= 宿主纪律模式：帧 delta 原样一步（既有宿主全传固定 1/60，行为不变）；设置后 delta 恒 = step（`arg` 可验证） |
| 第二脚本入口率 | **不引入**。`fixed_process` 类双率模型被否决：单模拟率是确定性（replay/网络同步）的根基；渲染率行为属未来子系统（animation 预留行）出现时的新冻结，不提前造 |
| 内建 `tick` | 引擎在 `simulate`（帧路径 + headless 共用唯一实现）里发射：每帧**恰一次**（补步帧也只一次），载荷 `I64(树帧号)`。宿主不得再手发 `tick`（双交付如实发生）。与黄金帧序的衔接：落在宿主预发位 → 泵序最前（先于 `tree/*` 桥） |
| 蓄步器 | `remainder += clamp0(frame_delta)`；步数 = `floor(remainder/step)`；余量跨帧携带（浮点尾差如实，不抹）。快帧可为 **0 步**（tick 仍发；结构落地/生命周期该帧不推进 —— 确定性不受影响，文档口径） |
| 螺旋钳制 | 一帧至多补 5 步；超限丢弃并计入 `steps_dropped()`（如实观测） |
| `tick_headless` | 保留为**裸单步**（不含内建 tick —— 高级用途）；常规宿主走 `step_headless`（与窗口帧路径同一 `simulate`，节拍语义零宿主分支） |

### 1.2 与 S7.1 黄金帧序的关系

序本身零改动 —— 内建 tick 是**宿主预发位**的一条新发射（引擎代替宿主
发）。S6.15 的"桥信号在泵序最前"精确序测试按 S7.1 已冻结的口径补齐
内建 tick 位（t_tick_05：`tick → tree/* 桥 → 用户信号`）。

## 2. 实现落点

| 位置 | 改动 |
|---|---|
| `nes-runtime/src/lib.rs` | `fixed_step/step_remainder/steps_dropped` 字段；`set_fixed_step`/`steps_dropped()`；`simulate`（内建 tick + 蓄步 + 钳制）；`step_headless`；frame_with/frame_windowed_with 接 simulate（表面借用序随之调整） |
| `nes-runtime/src/headless.rs` | `run_headless` 走 `step_headless`（与窗口同节拍） |
| `nes-runtime/examples/first_game.rs` | 宿主删手发 tick |
| `nes-runtime/examples/assets/first_game.ron` | referee `t = arg`（内建 tick 载荷当帧计数，win@1799） |
| `nes-runtime/tests/criterion_tick.rs` | t_tick_05 泵序补内建 tick 位（按 S7.1 口径） |
| `nes-runtime/examples/regression/dodge/` | ABI 重生成：`070713bc082dca22`（**评审注记**：变更 = 内建 tick 替代宿主手发 + 载荷帧号；玩法语义不变，T-GP-01 玩法闭环复验绿） |

## 3. 出口准则

| 编号 | 契约 | 结果 |
|---|---|---|
| T-LP-01 | 内建 tick 每帧恰一次；载荷 = 树帧号（0 基）；宿主零手发 | ✅ |
| T-LP-02 | 蓄步器：慢帧补步（1/30→2 步）、快帧 0 步余量进位、进位合并出步；`every` 的 arg 恒 = step；钳制 5 步 + 丢弃如实计数 | ✅ |
| T-LP-03 | 缺省宿主纪律模式不变（任意帧 delta 原样一步，arg 跟随） | ✅ |
| T-GP-01 | Dodge 玩法闭环 + 确定性（内建 tick 版）复验 | ✅ |
| T-ABI-01 | 基线重生成后比对绿（漂移→评审→更新流程的首次实践） | ✅ |
| t_tick_05 | 泵序含内建 tick 位（tick → 桥 → 用户） | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| S8.2 集合原语（Array/for_each —— 评审调序后提前；第二个项目的规模化前板） | 下一里程碑 |
| S8.3 数学内建（sqrt/abs —— 后置） | 排队 |
| S8.4 第二个真实项目（不同类型压生命周期/多实体/UI/状态管理） | 排队 |
| 渲染率脚本入口（若未来 animation/渲染行为子系统需要） | 新冻结才开（本里程碑明确否决提前造） |
| 0 步帧的生命周期/结构推进口径（当前不推进；若未来子系统依赖帧级推进再裁） | 文档口径 |
| 内建 tick 的订阅过滤口径（当前随广播过滤；系统信号是否豁免未裁决） | 未裁决 |

## 5. 记账

- 测试基线：nes-asset 34 / nes-scene 180 / nes-render-api 44 /
  nes-render-extract 42 / nes-render-wgpu 88 / **nes-runtime 47**
  （44 -> 47，+T-LP-01..03）—— 全绿，合计 **435**；
- 守卫 11/11；六 crate `clippy --all-targets` 零警告；
- 行为变更面：帧路径现在每帧发射内建 `tick`（既有 `on "tick"` 订阅者
  从"宿主手发"切换为"引擎发"—— Dodge 已迁移；若宿主继续手发则如实
  双交付）。缺省节拍（未设 fixed_step）不变。

*（内容由AI生成，仅供参考）*
