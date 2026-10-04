# NES 2.0 · S18.1 编辑器时间轴 dock（补间可视化 + 创建控制）契约文档 v1

分支：`s18-1-timeline`（worktree，基线 ff04cea = S18 换肤阶段 B）。
改动面：`nes-scene`（宿主 API 三件套 + TweenRow，additive）、
`nes-runtime/examples/editor_shell.rs`（时间轴 dock）。零新依赖、依赖
分层不变（nes-scene 不依赖任何上层；编辑器壳层只是 nes-runtime 的
example 宿主）。

## §0 结论

- 补间不再只能脚本创建：树公开宿主面 **register / stop / rows 三件套**，
  与脚本 `Op::Tween*`/`Cmd::Tween*` 走**同一条登记表路径**（不存在第二
  注册表）；`from` 的"落地时采样当前值"、last-wins 按（节点，通道）二元
  组、非法请求拒收等 S16/S16.1 冻结语义逐位保持（S16 全量契约测试未改
  一行、全绿）。
- 编辑器壳层新增 **TIMELINE dock**（Output 上方全宽 110px）：选中节点
  活动补间的行投影 + 每行进度细条 + 创建控制行（POS/SCALE/ALPHA → 目标
  值/时长/缓动/模式 → APPLY）。APPLY 从当前值起算经宿主 API 落地；输入
  非数值 / 无选中 / 拒收 → Output 报行不落地。
- 确定性边界兑现：编辑器创建的补间 = 编辑态会话态（不进 RON 往返、
  不进任何 headless 指纹基线 —— 见 §3）；S16 的"补间进语义指纹"口径
  不变（指纹采样面仍是 `tweens()` 内部切片，逐位未动）。
- 门禁：十 crate `cargo test --release` **787 全绿**（ff04cea 实测基线
  784 + 新增 3），clippy `--all-targets` **0 警告 ×10**，依赖方向守卫
  **15/15**，editor_shell 冒烟（120 帧干净退出 + NES_EDIT_DEMO 420 帧
  全链路断言），first_game(Dodge)/tween_demo/dungeon_game 冒烟回归干净。

## §1 树宿主 API 三件套（nes-scene，additive）

### 1.1 登记：`register_tween_channel`

```rust
pub fn register_tween_channel(
    &mut self,
    node: NodeId,
    channel: TweenChannel,
    duration_ms: f64,
    easing: TweenEasing,
    mode: TweenMode,
) -> bool
```

- **查证结论（任务要求的报告项）**：私有 `register_tween` 的既有签名
  `(&mut self, node, channel, duration_ms, easing, mode) -> bool` 与目标
  形态**完全一致**，但它承接的是 apply_cmd **已采样好 `from`** 的通道；
  宿主面需要"落地时采样"语义在树内单点兑现。最终落地为三段：
  1. 私有 `with_landed_from(node, channel)` —— S16 冻结采样语义的**单点
     实现**：Pos/Scale/Alpha/Pivot 的 `from` 按通道各自重采样（local pos /
     local scale / alpha 属性缺省 1.0 / pivot 属性缺省 (0,0)），调用侧
     携带的 `from` 不参与；**Frame 通道的 from/to 是语句字面量口径**，
     照实登记不采样（S16.2 冻结差异）；
  2. 私有 `register_tween`（原函数原语义：duration <= 0 / 非有限拒收、
     死节点拒收、alpha 终点夹 0..1 且非有限拒收、last-wins 替换同通道、
     登记序追加）；
  3. 公开 `register_tween_channel` = 1 + 2。
  apply_cmd 的 TweenPos/TweenScale/TweenAlpha/TweenFrame/TweenPivot 五臂
  **收拢改走同一条公开路径**（采样逻辑从五个 arm 的内联体上提为
  `with_landed_from` 单点 —— 纯代码 motion，S16 契约测试逐位回归通过）。
- 返回 `true` = 已登记；`false` = 节点无效 / 参数拒收（口径与脚本面同表）。

### 1.2 停止：`stop_tweens`

```rust
pub fn stop_tweens(&mut self, node: NodeId)
```

`Cmd::TweenStop` 落地的树侧等价（retain 目标句柄 ≠ node）：**全部通道**
登记一并丢弃，各通道停在当前值（local/属性不动）。幂等。apply_cmd 的
TweenStop 臂改走本公开面（单点实现，宿主/脚本同路）。

### 1.3 投影：`tween_rows` 与 `TweenRow`

```rust
pub fn tween_rows(&self, node: NodeId) -> Vec<TweenRow>

pub struct TweenRow {
    pub channel: String,     // "pos"/"scale"/"alpha"/"frame"/"pivot"
    pub progress: f32,       // 0..1，线性时间进度（未过缓动）
    pub easing: String,      // "linear"/"smoothstep"/"ease_in"/"ease_out"/"ease_in_out"
    pub mode: String,        // "once"/"yoyo"/"loop"
    pub elapsed_ms: f64,
    pub duration_ms: f64,
}
```

- **行序 = 注册序**（filter 保序 —— 确定性：同帧同树必同行序）；目标
  无关/死节点 = 空行集。
- `progress` 口径与 tick 推进阶段同式：once = elapsed/duration 夹取、
  yoyo = 去回折返形状（p<1 ? p : 2-p）、loop = 对 1 取模；**缓动不掺入
  progress**（曲线以名字单独成列 —— 时间轴行显示的是时间进度）。
- 既有 `tweens() -> &[Tween]`（内部结构切片）**原样保留**：字段本就
  pub，无需新投影即被 determinism.rs 的指纹采样面继续使用（逐位不变，
  T-TW-03 指纹契约未动）。`tween_rows` 是编辑器友好的只读视图，**不进
  指纹、不进序列化**。

### 1.4 契约测试（nes-scene/tests/s18_timeline.rs）

| 编号 | 断言 |
|---|---|
| T-TL-01 | 宿主登记双通道并存（行序 = 注册序、稳定名/进度/时长字段正确）→ tick 推进进度单调增长（scale 行领先）→ 800ms scale 到站移除、1000ms pos 落位 (100,50) + 行清空；无关节点空行集 |
| T-TL-02 | stop 全通道清行且停在当前值（幂等）；last-wins 宿主面同通道替换（from 重采样 = 当前实际值）；frame 字面量直通；alpha 终点 1.5 夹取 1.0 |
| T-TL-03 | duration 0 / 负 / NaN、alpha 终点 NaN、死节点 NodeId → 全部 `false` 且登记表零落地 |

## §2 编辑器时间轴 dock（editor_shell.rs）

### 2.1 布局（每帧投影，既有常量结构内改）

- **位置**：Output dock 上方、全宽、高 `TIMELINE_H = 110`（九宫格面板
  皮肤 + "TIMELINE" 标题行 —— 与 Output dock 同风格同纪律，z=-80 垫底、
  场景对象优先）。内部：标题 18 + 行区 58（3 行 ×18 + 4 内衬）+ 缝 4 +
  创建控制行 20。
- **让位**：左面板可用高、右 Inspector 面板高、可编辑区底缘 `gy1`
  统一上移 110（全部走既有每帧布局投影块的常量结构；演示对象 y
  180→130 留在缩小后的视口带内）。
- **容器与过滤**：全部观感/工具节点挂 "tldock" 容器 —— 层级树 walk
  skips 表加 `tldock`（照 grid/ruler/dock 先例）。

### 2.2 补间行区与进度条

- 行区 = ListView 复用（row_h 18，ListState 位图路径）。每帧从
  `tween_rows(primary)` 投影（Selection 主选中驱动 —— 照 Inspector 同款
  纪律）。**行格式（冻结，全 ASCII）**：

  ```
  POS  43%  ease_out  yoyo  (812ms/1500ms)
  ```

  通道大写 / 进度百分比（线性时间取整）/ 缓动 / 模式 / 已耗/时长。
  无补间 = 单行 `(no tweens on selection)`；无选中 = `(no selection)`。
- **进度条**：每行下沿 2px 细条（`fill_slot="selected"` 色，宽 = 行宽 ×
  progress）—— 选 **Control 池路线**（`TL_BARS = 4` ≥ 可见 3 行，照网格
  条带池先例，z=-79 列表之上精灵之下）；放弃文本进度条 `[===>   ]`：
  与九宫格面板观感同语言、行宽自适应免截断、且不占行文本预算。行超出
  可见窗的条不画（列表滚动是 UiVm 瞬态，宿主无钉底通道）。

### 2.3 创建控制行

- 布局（`TL_CTL_LAYOUT` 单点出表，x 恒定、y 每帧重写）：
  `NEW: [POS][SCALE][ALPHA]  to= [x][y]  ms= [500]  [linear] [once] [APPLY]`
  —— 三枚通道按钮（九宫格底板 ×6 同工具栏口径；`*` 后缀 = 选中通道，
  SEL/SNAP 同款）+ 两个数值 TextInput + 时长 TextInput + 缓动/模式循环
  按钮（点按在合法名表内轮换：linear→smoothstep→ease_in→ease_out→
  ease_in_out；once→yoyo→loop）+ APPLY。alpha 单值复用 x 框。
- **APPLY 语义**：从当前值起算 —— 组装 `TweenChannel`（Pos/Scale 带
  to；Alpha 只带 to）后调 `register_tween_channel`，from 由登记处按通道
  采样当前实际值（= "从现在走到目标"，与脚本 Cmd 落地逐位同源）。
  输入非数值 / duration <= 0 / 无选中 / 拒收 → Output 报一行
  （`tween: bad input …` / `tween: bad ms …` / `tween: no selection` /
  `tween: rejected …`），**不落地**。成功记
  `tween pos obj1 to=(2,4) ms=500 linear once`（from 不进日志 —— 登记
  处采样，行投影即现态真相）。
- **输入框落账**：三个 TextInput 的提交经 UiVm `on_commit` 钩子分流
  （tl_ 输入框优先于改名绑定，互不串账），帧后宿主双写会话值 + `text`
  属性（失焦后显示已提交值）。运行态滞留提交直接丢弃（与改名同护盾）。

### 2.4 交互防护

- 时间轴全部控件（bg / 行列表 / 三枚通道按钮 / 三输入框 / 缓动 / 模式
  / APPLY）进 hit 护盾数组（`press_in_control` 同款）—— 压上不清选中、
  不框选，交互让给 UiVm。
- 三个 TextInput 并入**焦点门**（改名框同门）：持焦时 Enter/字母/数字
  属于输入框 —— 键盘挂载流（F6/Enter/U/E）与音乐键整体让位。

### 2.5 冒烟钩子（NES_EDIT_DEMO=1）

- 新增注入段（帧 250..282）：点 x 框 → Backspace 清缺省 → 键入 '2' →
  点 y 框（x 框失焦即提交）→ 同法键入 '4' → 点 APPLY（缺省 ms=500）
  → 登记 pos 补间 to=(2,4)。全链路实走 夺焦/键入/失焦提交/APPLY 落地，
  且点击全程不清选中（护盾同帧受验）。
- 取证：Output 必有 `tween pos obj1 to=(2,4) ms=500 linear once` 行；
  时间轴行投影含 "POS" 行（帧 288 采样）；两个**刷新率无关闩锁**（帧
  ≥284 逐帧观察登记表长度）—— 见过 >0 = 活动补间真实入表、其后见过
  =0 = 推进/到站移除真实发生（时间基准是帧差累计毫秒，固定帧号采样会
  随刷新率漂移，闩锁不会）。
- 既有钩子的必要随动：fs 双击挂载段的坐标按让位后的左栏布局重算
  （fs 列表变矮，spin.nes 落到可见窗外 —— 先在 fs 树上滚轮 4 格再双击）；
  IME 取证从固定帧点采样改为 **211..=218 滞容闩锁**（Char 经真实消息泵
  投递、到达帧有 1..数帧抖动，点采样偶发扑空 —— 框架行为未变，取证
  更稳）。

## §3 确定性边界

- **同一登记表**：编辑器经 `register_tween_channel` 创建的补间与脚本
  `Cmd::Tween*` 同表同路 —— last-wins、落地采样、到站信号、tick 推进
  全部同源。没有"编辑器补间"这种第二状态。
- **会话态**：补间登记表不进 RON 往返（S16 冻结 —— 保存时进行中的补间
  丢弃，各通道字段已是最新值，无损）；编辑器壳层的通道/缓动/模式档位、
  输入框会话值同样是编辑器会话态（不进树、不落盘）。
- **指纹**：补间是游戏可见状态、**进语义指纹**（S16 冻结，逐位保持）；
  但编辑器本身不参与 headless 指纹 —— Dodge 等 headless 基线轨迹里
  从不出现编辑器创建的补间，基线不含 timeline、亦不受本里程碑影响
  （实测：determinism.rs 零改动，S16 指纹契约测试原样全绿）。
- 编辑态（NoObserver）下树照常 tick：时间轴创建的补间在编辑会话内真实
  推进/到站/移除（冒烟闩锁取证的就是这个事实）；PLAY/STOP/RESET 与之
  的交互 = 运行期树状态（Godot 语义：运行期改动就是真改；RESET 的数据
  面还原不含补间 —— 快照本就不携带会话态）。

## §4 门禁（worktree 实测）

| 门 | 结果 |
|---|---|
| `cargo test --release` ×10 crate | **787 passed / 0 failed**（基线 ff04cea 实测 784 + 新增 3：T-TL-01..03；任务书所记 771 为陈旧口径） |
| `cargo clippy --release --all-targets` ×10 | **0 警告 ×10** |
| `check_dependency_direction.py` | **15/15 通过** |
| editor_shell 冒烟 | 120 帧干净退出；`NES_EDIT_DEMO=1 NES_GAME_FRAMES=420` 全链路断言通过（含时间轴 APPLY/入表/到站移除；连跑 3 次稳定） |
| first_game(Dodge) / tween_demo / dungeon_game 冒烟 | NES_GAME_FRAMES=240/180/240 干净退出（S16 脚本面回归） |

## §5 遗留（后续里程碑）

- **Scrub 拖拽**：行上拖动改 elapsed（当前只读可视化；需要 host 侧
  seek 通道 + 与 last-wins 的语义裁决）。
- **关键帧多段**：单通道单段（from→to）是 S16 冻结形态；多关键帧 =
  登记表结构升级（TweenRow 的 progress 口径随之重定义）。
- **曲线编辑**：缓动仅五枚举循环按钮；自定义贝塞尔归 S16.1 §5 既有
  遗留，UI 侧曲线绘制/编辑随之。
- **暂停门控联动**：补间推进不受暂停门控（S16 冻结 v1 面）；时间轴
  暂停/单步与 S6 暂停语义的联动归后续里程碑。
- **行区真字体**：列表行走 ListState 位图路径（16px 等宽）；S12.11
  的"列表行接真字体"既有遗留同样覆盖时间轴行区。
