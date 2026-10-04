# NES 2.0 · S16 动画与补间第 1 期 —— 位置补间成为引擎一等公民

日期：2026-10-03 · 分支：`s16-tween`（独立 worktree `wt-tween`）· 基线：8903814
（AV 严格同步）· 依赖 **零新增**（G13 白名单原样覆盖；核心 crate 零第三方纪律不动）

---

## §0 结论

**交付**：三个真实项目都在手搓平滑移动（每帧循环累加步进）的现状，被一句
脚本终结 —— `tween_pos "name" x y ms`。位移补间做进 nes-scene：引擎在
`SceneTree::tick` 的**专属阶段**（结构落地后、enter/process 前）逐 tick
**确定性**推进，每 tick 直写节点 local（经既有 `set_local` 脏标记路径，
世界矩阵照常冲洗）；补间全程进**语义指纹**（补间是游戏可见状态，与音频/
视频的"渲染侧不进指纹"相反口径）。配套 `tween_stop "name"` 停补间（位置
停在当前值）。**last-wins** 语义：同一目标的已有补间被新补间替换，新起点
在 **Cmd 落地时采样**（当前实际位置，不跳变）。序列化面：补间是**会话态**
——不进 RON 往返（保存时进行中的补间丢弃，位置字段已是最新，无损）。

**门禁**：八 crate `cargo test --release` 全绿 **670**（基线 665 +
nes-scene 契约新增 5）；clippy 0 警告 × 8；worktree 根守卫 **13/13**；
冒烟双通道 —— `NES_GAME_FRAMES=180 cargo run --release --example
tween_demo` 干净跑完退出；headless CLI 对 tween_demo 场景 300 帧跑两遍
`trace hash` 逐位相同（`6de99224686bf7da`，全链补间确定性实证）。已
git commit（未 push）。

**基线零漂移实证**：指纹采样面做成**条件混入**（登记表非空才摺进），无
补间场景的指纹与旧口径逐位相同 —— 既有 665 测试（含 Dodge 等确定性基线）
原样全绿即证明。

---

## §1 设计冻结与落地细节

### 1.1 确定性位置：推进是 tick 的专属阶段

帧模型更新（`tree.rs` 模块文档同步改写）：

```text
tick(delta)
  ├─ 1.    apply_pending     结构变更统一落地
  ├─ 1.5  timer 递减        每节点倒计时（S10-1）
  ├─ 1.75 tween 推进        位置补间直写 local（S16；先于一切脚本）★新
  ├─ 2.    enter_tree        自顶向下，仅新入树节点
  ├─ 3.    ready             自底向上（逆前序），仅新就绪节点
  ├─ 4.    process           自顶向下，全树
  └─ 5.    flush_transforms  脏传播 → 世界矩阵
```

- **落位通道**：推进阶段不碰 `world`、不绕过脏标记 —— 每次推进构造目标
  当前的 `Transform2D`、只改 `pos`，走既有 [`SceneTree::set_local`]
  （`DIRTY_XFORM` + 祖先链 `DIRTY_SUBTREE`），世界矩阵照常在阶段 5 冲洗。
  渲染、命中测试、脚本 `pos` 读全部自动一致。
- **指纹口径**：与音频（`played_sounds`）/视频（`video_cmds`）的"取走
  缓冲、不进指纹"相反 —— 补间登记表是**树状态**，每 tick 的位置是真实
  树状态，**登记表 + 位置双双进语义指纹**（同 tick 同轨迹必同结果；
  实测见 §3 T-TW-03 与 §4 headless 双跑）。
- **时间口径**（冻结条文的 `elapsed += delta_ms` 落地形）：
  `elapsed_ms += (delta * time_scale) as f64 * 1000.0` —— 与 process 同一条
  `time_scale` 缩放（"游戏时间"全引擎一元）；f64 累计毫秒，
  `t = clamp(elapsed/duration, 0..1)`（f64），lerp 在 f32 域（与 local 同
  精度）。`t >= 1` 当帧**精确落位 `to`** 并移除登记 —— 推进先于 process，
  同帧脚本读到的就是终值（T-TW-05 钉死）。
- **暂停交互（v1 冻结面外，见 §5）**：补间推进是引擎阶段不是 process
  派发，v1 不受暂停门控；`time_scale` 生效（纯时间缩放）。

### 1.2 last-wins 与 from 采样时机（冻结条文的落地裁决）

- 登记表：`SceneTree.tweens: Vec<Tween>`（登记序），
  `Tween { target: NodeHandle, from: Vec2, to: Vec2, elapsed_ms: f64,
  duration_ms: f64 }`。`target` 用临时句柄：推进阶段每 tick 经 arena
  resolve，**失败即移除** —— 死节点补间自动清，不悬挂（T-TW-03）。
- **last-wins**：`Cmd::TweenPos` 落地时先 `retain` 掉同目标旧补间再追加
  —— 注册表长度恒 1/目标，替换而非叠加（T-TW-02）。
- **from 采样时机 = Cmd 落地时**（apply 阶段，非发射时）。选择理由：
  Cmd 落地发生在回调尾部/信号泵（本 tick 的推进阶段 1.75 **已经跑过**），
  落地时采样拿到的起点**含本帧推进** —— 进行中换程时新程从"此刻真实
  位置"出发，既不跳变也不回头重走已走路程（T-TW-02 实证：250ms 处换程，
  from = 53.33 而非 50 或 0）。若在发射时采样，脚本与落地之间的一次推进
  会让新程起步回退一帧 —— 违反"不跳变"直觉。
- **非法请求的兜底**（解析期拦不住的运行时值）：`duration_ms <= 0` 或
  非有限 → **拒收不落地**（不登记、不停机、不编造"瞬时移动"语义）——
  与"属性写错静默"同家法；字面量则在**解析期**报错（见 §2）。
  目标节点查无（已死）→ 落地处静默丢弃（与 `SetLocal` 同口径）。

### 1.3 指纹条件混入（基线不动的实现保证）

`determinism.rs` 的 `scene_fingerprint` 在节点循环后追加：

```text
if 登记表非空 {
    mix "tweens" + len
    每条（登记序）：target uid（16B，确定性锚定）
                 + from.x/y、to.x/y（f32 位形）
                 + elapsed_ms、duration_ms（f64 位形）
}
```

- **条件混入**：登记表空 → 一字节不混 —— 哈希流与旧实现逐指令相同，
  无补间场景基线指纹**逐位不变**（665 存量测试全绿即回归证明）。
- **uid 锚定**：目标身份用 uid（与节点身份同源，S9-1 口径），句柄位形/
  代际是 allocator 历史，不进指纹；查无按全 1 位形规范 Dead 态如实混入
  （防御口径 —— 推进阶段理应已自动清）。
- 确定性要求登记表本身可复现：登记序 + 全字段位形，同轨迹两跑逐位同。

### 1.4 序列化 = 会话态

补间登记表挂在 `SceneTree`（调度态，与 `paused`/`time_scale` 同级），
不进 `NodeData` —— `to_doc`/RON 往返**天然不携带**，零序列化面改动。
语义：保存时进行中的补间丢弃；因为推进每 tick 直写 `local`，位置字段
保存时已是最新值，**丢弃无损**。编辑器 undo（事务快照 `NodeData`）同理
不追踪补间 —— 会话态不进时间机器。

---

## §2 脚本面

### 2.1 语句

```text
tween_pos "box" 360.0 40.0 1500    // 目标按名 + 终点 xy + 时长毫秒
tween_stop "box"                   // 停补间，位置停在当前值
```

- `tween_pos`：目标名是编译期常量（照 `play` 的解析样式）；`x`/`y`/`ms`
  **各是独立表达式**（运行时求值，`arg`/局部/算式都可用），压序 = 源序；
  编译产物 `[x][y][ms] Op::TweenPos{name}`，执行弹序 ms、y、x（须全数值，
  否则停机记录）。目标按 **NodeByName 语义**解析（整树首个命中），找不到
  = **停机记录**（`__halt` 指名目标，照既有纪律）。
- **ms <= 0 在解析期报错**（T-TW-04）：字面量形态（含一元负号脱糖
  `0 - n`）在编译器里当场拒绝 —— `tween_pos "box" 0 0 0` /
  `tween_pos "box" 0 0 -5` 都是 `ParseError`。
- `tween_stop`：零栈交互（照 `play` 形态），产物 `Op::TweenStop{name}`；
  目标找不到同样停机记录（目标名写错应如实暴露，两语句一致）。
- 两入口**同权**（照 `emit`/`play` 口径）：process（`every`）与信号
  （`on "..."`）都可发 —— `NodeCtx` 与 `SignalCtx` 各加
  `tween_pos`/`tween_stop`，同一 `Cmd` 通道。tween_demo 的方块走 process
  入口、对照标记走信号入口，顺带双覆盖。
- **Cmd 直接操作登记表**（不走单帧取走缓冲）：与 `PlaySound`/`VideoPlay`
  的"树无法解释才外送"不同 —— 登记表本来就在树上，`apply_cmd` 即刻落地
  （回调尾部/信号泵内），下一帧帧首的推进阶段即可生效。
- 保留字表扩到 20：`tween_pos`/`tween_stop` 不得作变量名。

### 2.2 引擎 API 面（非脚本）

- `SceneTree::tweens() -> &[Tween]`（只读视图，登记序；指纹与宿主检视同
  入口）；
- `Cmd::TweenPos { node, to, duration_ms }` / `Cmd::TweenStop { node }`；
- `nes_scene::Tween` 导出。

---

## §3 契约回归（nes-scene/tests/s16_tween.rs，5 条全绿）

| 编号 | 钉死的契约 | 关键断言 |
|---|---|---|
| T-TW-01 | 生命周期全链 | 登记（from=当前位/elapsed=0）→ 单 tick 500ms 推进 → 中点 (50,25) **f32 精确** → 时满精确落位 (100,50) + 登记移除 → 额外 tick 零行为 |
| T-TW-02 | last-wins + stop | 250ms 处换程：登记表长度仍 1（替换非叠加）；from = 落地时当前位（53.33，含本帧推进 > 旧位 50，不跳变）；新程中点正确；`tween_stop` 后位置定格 |
| T-TW-03 | 确定性 + 死目标 | uid 钉死后同轨迹两跑逐帧指纹**逐位相同**；含补间 vs 不含指纹**必不同**（条件混入生效）；删除目标节点 → 下一帧推进阶段 resolve 失败 → 补间自动清 |
| T-TW-04 | 解析面 | 编译产物逐指令相等；`ms<=0` 字面量（0/-5/0.0/一元负号）解析期报错；保留字拒绝；目标不存在 → `__halt` 指名 + 零登记；运行时非法 ms（变量传 0）→ 树侧拒收、不停机 |
| T-TW-05 | 推进阶段时序 | tick 内先推进（1.75）后 process：登记帧读旧值 0，次帧起读到含本帧推进的位置（250ms 档：100/200/300），**时满帧读到终值 400** 且登记同帧移除 |

测试字面量全 ASCII；断言用 f32 精确值（0.25s 档位全部落在 2 的幂上，
无浮点比较噪声）。

---

## §4 门禁

| 项 | 结果 |
|---|---|
| `cargo test --release` × 8 crate | **670 通过 / 0 失败**：nes-asset 34 · nes-scene 244（基线 239 + 新 5）· nes-render-api 45 · nes-render-extract 56 · nes-render-wgpu 123 · nes-audio 52 · nes-media 27 · nes-runtime 89（基线 665 + 新增 5） |
| `cargo clippy --release --all-targets` × 8 | **0 警告 × 8**（本轮新增代码引出的 3 处当场修：2 处 doc 缩进 + 1 处 `== false`） |
| 守卫 `check_dependency_direction.py` | **13/13**（G13 白名单原样，零新依赖） |
| 冒烟（窗口） | `NES_GAME_FRAMES=180 cargo run --release --example tween_demo` 干净跑完退出（补间方块全程往返） |
| 冒烟（headless 确定性） | `nes --headless examples/assets/tween_demo.ron --frames 300` 跑两遍 `trace hash` 逐位相同：`6de99224686bf7da` |
| 基线漂移 | 无 —— 665 存量测试（含 Dodge/S10 压力图等确定性基线）原样全绿 |

---

## §5 遗留（后续里程碑候选）

1. **缓动函数族**：v1 只有线性 lerp；ease-in/out、弹性、回弹等待
   `tween_pos ... ease` 一类参数面（登记表需加 easing 字段 + 指纹口径
   随之扩一位）。
2. **scale / alpha / rot 通道**：位置之外的补间通道（`tween_scale`/
   `tween_alpha`），落同一张登记表的多通道形态。
3. **循环/往返**：`loop`/`yoyo` 修饰 —— v1 由脚本在到站时换程替代
   （tween_demo 即此写法，实测顺滑）。
4. **速度曲线与 scrub**：每补间 time_scale、暂停门控交互（v1 冻结面
   不受暂停门控 —— 引擎阶段不是 process 派发）、编辑器拖动时间轴时
   补间的 scrub 语义。
5. **编辑器时间轴**：补间的可视化编辑（关键帧面板）；会话态语义下
   保存/恢复策略（当前 = 丢弃 + 位置无损）。
6. **到站信号**：`tween_pos` 完成时自动 `emit` 一条信号（现在脚本轮询
   位置判断到站）。
