# NES 2.0 · S7.3 Headless / 确定性运行时 v1

> 交付日期：2026-10-01　｜　状态：**同一运行时、同一 tick 语义，无 GPU/窗口 —— 固定 Scene + 固定 InputTrace 跑 N 帧得稳定状态哈希（逐帧 + 轨迹两级）**
> 前置：S7.1 运行时语义冻结（确定性正是那些冻结的直接兑现）；S7.2 输入系统（`inject_input` 与真实消息同队列 = headless 无需模拟 Win32）。

---

## 0. 一句话结论

五件冻结全部落地：**Headless Host**（`NesRuntime::open_headless` —— GPU
端 `Option` 化，装配缺席而非语义分支；`nes` CLI 是 `headless::run` 的
薄壳，不是第二个运行时）、**InputTrace**（帧号 + 事件批的纯文本轨迹
+ `parse_trace`）、**State Hash**（`scene_fingerprint`：只取语义状态 ——
前序结构/变换 f32 位形/属性/生命周期位/脚本局部；**绝不取**句柄/指针/
HashMap 布局/HWND/时间戳）、**Frame Hash**（逐帧指纹数组 → 链式
`trace_hash`，第一处差异定位到帧）、**Determinism Tests**（T-HR-01..08，
**全部无 GPU 依赖**）。CLI 实测**跨进程**确定（两次独立进程同轨迹同
哈希）。全仓测试 **424**（34 / 174 / 44 / 42 / 88 / 42）全绿，守卫
11/11，clippy 零警告。

---

## 1. 架构裁决：同一运行时，不是第二套

```text
                 NesRuntime（装配层）
                     │
        ┌────────────┴────────────┐
        │                         │
   Window Host               Headless Host
   （open_windowed）          （open_headless / nes CLI）
   GPU consumer ✅            GPU consumer ❌（Option = None）
   surface ✅                 surface ❌
        │                         │
        └────────────┬────────────┘
                     ↓
          同一条 SceneTree::tick / collect_input
          / emit_input_signals / 装载 / 指纹路径
```

- `consumer: Option<CommandConsumer>`：headless 装配是**渲染端缺席**，
  不是 `if headless { 特殊逻辑 }` —— `frame*`/`upload_pending_textures`
  如实报"headless 运行时没有渲染端"，语义方法（装载/输入/tick/指纹）
  与窗口模式**逐字节同路径**（`tick_headless` 内部就是
  `self.tree.tick`）。
- headless 输入**不需要模拟 Win32**：`inject_input` 本来就与真实消息
  同一队列（S7.2），轨迹注入 = 合成输入 = 自动化路径，一条不分叉。
- 确定性不靠渲染端背书：**全部 T-HR 用例无 GPU 依赖**（连 wgpu-native
  DLL 都不加载）—— 这是架构主张本身的测试形态。

## 2. 五件冻结

### 2.1 Headless Host

- `NesRuntime::open_headless(root)`：无窗口/无 GPU 装配；
- `NesRuntime::run_headless(scene, trace, frames, delta)`：加载场景 →
  装载脚本（外置 `.nes` 走磁盘闭包，与窗口宿主同一路径）→ 接键探针 →
  逐帧循环 → `HeadlessReport { frame_hashes, trace_hash }`；
- `nes_runtime::headless::run(root, ...)`：便捷入口（**CLI 与测试共用
  同一函数** —— CLI 不 owning 任何运行时逻辑）；
- CLI：`nes --headless <场景.ron> [--frames N] [--trace <文件>]
  [--delta F]`，输出逐帧 `frame N hash X` + `trace hash Y`。
  `delta` 是固定值 —— **确定性口径下时间也是输入**。

### 2.2 Input Trace

`InputTrace { frame, events }`；纯文本（每行一帧：`帧号 事件...`，
`#` 注释，同帧多行合并，按帧排序）：

```text
0 key_down W
1 mouse_move 100 100
2 key_up W key_down Space
3 char 104            # UTF-16 单元码
4 mouse_down left resize 800 600
```

未知键名/事件/缺参数如实报错带行号（与手写解析器同纪律）。回放
（Replay）是同一数据结构的天然延伸 —— 记录 = 逐帧收集真实事件。

### 2.3 State Hash（语义状态白名单）

| 进哈希 | 不进哈希（伪确定性来源） |
|---|---|
| 树级：帧号 / paused / time_scale（位形） | GPU 句柄 / 指针 / 分配地址 |
| 节点（**前序**序）：前序下标 / 名字 / 类型 / 父（前序下标）/ process_mode / enter/ready 位 / 本地变换（pos/rot/skew/scale 逐字段 **f32 位形**）/ 属性表（BTreeMap 名序） | HashMap 迭代布局（全库帧路径已零 HashMap 迭代 —— S7.0 审计结论在此兑现价值） |
| 脚本局部：按节点前序 × 局部名 BTree 序 | HWND / 时间戳 / 世界变换缓存（派生量） |
| 输入按住态：held 键名（字典序）+ 按钮 | TickStats（观测计数，非状态） |

节点哈希身份 = **前序位置**（slot/代际是内部实现，语义身份 = 结构
位置 + 名字）。f32 取**位形**（`to_bits`）：IEEE 语义下位形相同即
不可区分；`-0.0` 与 `0.0` 位形不同是如实口径。域标签版本化
（`NES_SCENE_FP_V1` / `NES_TRACE_V1`）：口径变更时旧哈希自然失效，
不误判相等。分组表暂不进（v1 无按组分派的行为面；出现时再裁剪）。

### 2.4 Frame Hash → Trace Hash

```text
frame_hashes[0..N]（每帧末的语义状态指纹）
    ↓ 链式 FNV-1a 混合
trace_hash（两运行等价 ⟺ 相等；不等 ⟹ frame_hashes 定位第一处差异）
```

差分用法（Scratch `.sb3` 路线的同一条口径）：两个实现的输出逐行
diff，第一处差异即定位到帧（T-HR-08 演示：同场景两条只差一帧松开
时机的轨迹，恰在第 3 帧分岔、之前全等）。

### 2.5 Determinism Tests（T-HR-01..08，无 GPU）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-HR-01 | 同场景 + 同轨迹两次运行：逐帧 + 轨迹指纹全等；状态非常数（哈希在动） | ✅ |
| T-HR-02 | 空输入确定（跨独立运行时实例） | ✅ |
| T-HR-03 | 键盘轨迹确定；**≠** 空输入指纹（输入真进状态） | ✅ |
| T-HR-04 | 鼠标轨迹（移动/脉冲/resize/负坐标）确定 | ✅ |
| T-HR-05 | 文本轨迹（char 码点串）确定 | ✅ |
| T-HR-06 | 信号级联（链式 emit + 双订阅者路由序）确定 —— S7.1 BFS 泵序的直接兑现 | ✅ |
| T-HR-07 | 状态混合演化（输入驱动局部 + 持续 f32 漂移）确定 —— 浮点累积受检 | ✅ |
| T-HR-08 | 差分口径：分歧帧之前全等、第一处差异恰在预期帧 | ✅ |

另：T-In-C04（轨迹解析契约，render-api）。CLI 实测**跨进程**确定
（两次独立进程同轨迹同 `trace hash`）。

## 3. 实现落点

| 位置 | 改动 |
|---|---|
| `nes-render-api/src/input.rs` | `InputTrace` + `parse_trace`（纯文本帧事件表） |
| `nes-scene/src/determinism.rs` | `scene_fingerprint`（语义状态白名单哈希，fnv1a64 链式） |
| `nes-runtime/src/lib.rs` | `consumer: Option<...>`（GPU 端缺席装配）；`open_headless`；渲染路径守卫；`consumer_mut -> Option` |
| `nes-runtime/src/headless.rs` | `tick_headless` / `state_fingerprint` / `run_headless` / `run` / `HeadlessReport` |
| `nes-runtime/src/bin/nes.rs` | CLI（`--headless/--frames/--trace/--delta`；`headless::run` 薄壳） |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 窗口视觉验收（S7.2 环境问题，独立验证项） | 待环境（不阻塞本线） |
| 回放记录器（真实会话逐帧收集事件 → InputTrace 文件） | 未启动（数据结构已就绪） |
| 基线哈希档案（把当前哈希存档进仓库，CI 跨版本比对） | 未启动（先有契约后有档案） |
| Scratch `.sb3` 差分执行（NES vs Scratch 参照实现逐帧比对） | S8 后路线（本里程碑的差分口径就是为它铺的） |
| 分组表/信号队列残量进指纹（出现相应行为面时） | 按需 |
| **差分接口演进**（评审注记，S7.4 前记录）：`trace_hash` 不作为差分
    接口的终点 —— 后续形态 `DeterminismReport { frame_hashes,
    trace_hash, first_divergence: Option<Frame> }` 与 `FrameDiff
    { tree/property/script/input diff }`；**现在不做**（"定位到帧"的
    第一层能力已足够当前差分口径） | 方向记录 |
| **语义身份分层**（评审注记）：`前序位置 + name` 是"当前 Scene 结构
    状态"的指纹身份，**不是持久 NodeId** —— 两者不是同一概念。若未来
    引入持久 NodeId，最终分层应为 `NodeId → 语义身份`、
    `Preorder index → 规范遍历位`，勿让前序位置最终承担 NodeId 的
    职责（slot/代际已是 Execution Storage 细节，前序位只是它的规范化
    投影） | 方向记录 |
| 跨机器/跨编译器版本的位级确定声明（f32 位形在 IEEE 下成立；正式声明待跨机验证） | 待验证 |

## 5. 记账

- 测试基线：nes-asset 34 / nes-scene 174 / **nes-render-api 44**
  （43 -> 44，+T-In-C04）/ nes-render-extract 42 / nes-render-wgpu 88 /
  **nes-runtime 42**（34 -> 42，+T-HR-01..08）—— 全绿，合计 **424**；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- API 变更：`consumer_mut` 返回 `Option`（headless 缺席是事实，调用方
  `expect` 指名）；其余全为新增。

*（内容由AI生成，仅供参考）*
