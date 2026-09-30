# NES 2.0 · S6.2 生命周期 Tick 接线 v1

> 交付日期：2026-10-01　｜　状态：**帧循环四条腿齐了（tick + 提取 + 消费 + 呈现）**
> 前置：S6.1 窗口/Surface、M5 组装层。本轮把场景层早已冻结的 `tick`
>（结构落地 + enter/ready/process 生命周期 + `Cmd` 命令缓冲 + 变换冲洗）
> 接进运行时帧循环 —— M5 §4 候选 2 落地。

---

## 0. 一句话结论

`NesRuntime::frame` / `frame_windowed` 的帧内前半程从手工两步
（`apply_pending` + `refresh_transforms`）升级为 `SceneTree::tick(delta, obs)`：
**生命周期第一次由引擎帧循环驱动**，宿主行为代码经 `SceneObserver` 挂入
（`frame_with` / `frame_windowed_with`），回调里经 `NodeCtx` 发出的
`SetLocal`/`SetProp` 命令**本帧直达像素**、结构变更（`Tree`/`Spawn`）
**延迟一帧落地**。出口准则 T-Tick-01..04 全过；窗口示例的动画从"宿主逐帧
直改树"改为"观察者回调驱动"，截屏像素复验位置仍在正弦轨道解析值上。
全仓测试 75 / 34 / 42 / 40 / 83 / **8** 全绿，守卫 11/11，clippy 零警告。

---

## 1. 接的什么线

### 1.1 之前与之后

```text
之前（M5）:  frame        = apply_pending -> refresh_transforms -> extract -> consume
之后（S6.2）: frame_with    = tick(delta, obs) ------------------> extract -> consume
                           └ 结构落地 -> enter_tree -> ready -> process -> 变换冲洗
```

场景层的 `tick` / `Cmd` / `NodeCtx` / `SceneObserver` / `TickStats` 在
`nes-scene` 早已冻结（m1.rs 钉死遍历确定性、enter/ready 顺序与幂等、
延迟 Spawn、遍历中删节点安全），本轮**一行语义未动** —— 只是把运行时
从"绕过生命周期手工推两步"改成"走正规 tick"。E-Loop-01..04 复跑全绿，
证明这次替换对既有语义是透明的。

### 1.2 API 面（nes-runtime）

| 方法 | 职责 |
|---|---|
| `frame(frame)` / `frame_windowed(frame)` | 无行为代码的便捷形式：内部以 `NoObserver` 走同一条 tick 路径（生命周期标志照置，只是没人监听） |
| `frame_with(frame, obs)` / `frame_windowed_with(frame, obs)` | 挂宿主行为观察者推进一帧 |
| `nes_scene::NoObserver`（新增） | 空观察者缺省，保证"不接行为"与"接了行为"不出现两条代码路径 |

### 1.3 帧内时序契约（本轮在运行时层面钉住的口径）

- 回调 `SetLocal`/`SetProp`：**立即生效**（tick 第 4 阶段逐命令落地），
  本帧 extract 前的变换冲洗（第 5 阶段）已经算进世界矩阵 —— **本帧像素可见**；
- 回调 `Tree(TreeOp)`/`Spawn`：进下一帧 pending，**下一帧帧首落地**，
  落地帧内新节点先 `enter_tree`/`ready` 再 `process`（草案 §7 顺序规则）；
- 观察者拿到的是**只读树 + 命令缓冲**（`NodeCtx` 刻意不给 `&mut SceneTree`），
  回调不可能把遍历搅乱 —— 这是行为代码能挂进引擎的安全前提。

## 2. 出口准则（`nes-runtime/tests/criterion_tick.rs`，4/4）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Tick-01 | 运行时首帧驱动完整生命周期：enter 自顶向下（root→player→cam）、ready 自底向上（cam→player→root）、process 每节点一次；第二帧起 enter/ready 为空 | ✅ |
| T-Tick-02 | 回调里 `translate(16,0)`：**本帧**精灵像素从 (10,10) 移到 (26,10)，旧位置回背景 | ✅ |
| T-Tick-03 | 回调里 `queue(Remove)`：本帧仍渲染（drawn=1、像素未变、无事件），下一帧消失且 `Removed` 事件在落地帧送达 | ✅ |
| T-Tick-04 | 回调里 `spawn_child`：本帧结构不落地（树中查无），下一帧新节点恰好一次 enter（先于 process）、宿主帧间绑纹理后第三帧入画 —— 且 Spawn 挂在发起节点下，local 经父变换复合到位（世界坐标逐像素核对） | ✅ |

（场景层遍历/幂等语义不在此重复 —— 那是 `nes-scene/tests/m1.rs` 的领土；
本组证明的是"运行时真的在驱动 tick"与"回调命令直达像素"。）

## 3. 示例改造（`examples/engine_window.rs`）

动画的正弦轨道平移从宿主逐帧 `tree_mut().set_local(...)`（还要每帧前序
找节点）改为 `SineDrift` 观察者持有 `NodeId`、在 `on_process` 里
`ctx.set_local(...)`（时间自累计 `t += delta`，2 rad/s、振幅 24px）。
帧循环改走 `frame_windowed_with`。截屏像素复验：

- 绿帧（~1.0s）orange 精灵盒起点 x=50、红帧（~4.0s）x=55 —— 都在
  `32 + 24·sin(2t)` 的解析位置上（动画连续、由回调驱动）；
- 热重载绿→红对、HUD 边框 [288,496)×[16,64)、内部透明、背景覆盖
  —— 与 S6.1 基线逐项一致（tick 替换对窗口路径同样透明）。

**这是分水岭**：此前"动画"是宿主代码直接改树；此后行为代码有了正式入口
（观察者回调 + 命令缓冲），为 Script 节点（`registry_key` 挂载点）与
M5 Scratch/JS 兼容层铺平了路 —— 那些扩展最终也只是"另一种 SceneObserver"。

## 4. 边界与裁决

- **不改场景层一行语义**：本轮 nes-scene 仅新增 `NoObserver`（纯便利类型，
  全默认实现）；帧循环语义全部来自既有 `tick`；
- **TickStats 暂不外泄**：运行时帧返回值维持 `FrameOutcome`/`FrameStats`
  不变；tick 计数对测试可见性走观察者（计数器模式），不为记账扩渲染层结构；
- **delta 来源**：`FrameInfo.delta`（宿主构造，示例 1/60s）。`time_scale`/
  `ProcessMode` 暂停语义属场景层草案 §9，尚未在 `SceneTreeRt` 落地 ——
  接线时**不提前发明**；
- **窗口路径共用同一 tick**：`frame_windowed_with` 与 `frame_with` 仅消费端
  不同（表面 vs 离屏读回），行为代码对两种宿主完全可移植。

## 5. 遗留与后续

| 事项 | 状态 |
|---|---|
| `paused` / `time_scale` / `ProcessMode`（草案 §9 的 SceneTreeRt 语义） | 未启动（本轮刻意不提前实现） |
| `on_exit_tree` 的运行时侧可观测性（目前 Remove 走 `on_tree_event`） | 口径待定 |
| Script 节点接 `registry_key` 行为注册表（兼容层入口） | 未启动（观察者路径已铺好） |
| 场景序列化闭环（SceneDoc 实例化 -> 渲染，`scene_io` 已冻结） | M5 §4 候选 3 |
| WM_SIZE 重配置 / DPI / 多 GPU（S6.1 遗留） | 未启动 |

## 6. 记账

- 测试基线：nes-scene 75 / nes-asset 34 / nes-render-api 40 /
  nes-render-extract 42 / nes-render-wgpu 83 / **nes-runtime 8**（4 -> 8，
  +T-Tick 4）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 产出物：`nes-runtime/output/client_t2_green.png` / `client_t2_red.png`
  （观察者驱动动画的截屏复验取证）。

*（内容由AI生成，仅供参考）*
