# NES 2.0 · S11-2 哨塔防线 v1

> 交付日期：2026-10-02　｜　状态：**第三完整项目落地——四脚本协作 + F-1 读面实战（T-TD-01..07 契约回归）**
> 前置：S11-1 脚本级只读共享面（F-1 落地）；S8.2b v1.1（写纪律冻结）；S7.3 headless 确定性运行时。

---

## 0. 一句话结论

S11-1 §6 预告的**第三完整游戏**落地：哨塔防线（`tower_defense.ron`），
四个具名 Script 节点（game / spawner / tower_ctl / hud_ui）协作——gold /
lives / wave / kills **只住 game 一个属主的 locals**，其余三个脚本全程
**只经 F-1 点读**（`game.gold` / `game.lives` / `game.wave` / `game.kills`）
消费，变更一律 emit 信号回属主（无第二状态总线）。900 帧实跑账本分毫不差
（gold = 每杀 5 金、lives = 10 − 漏怪、HUD 逐帧与属主局部一致）；双跑逐帧
指纹全等；六 crate 484 测试全绿、clippy 0、守卫 11/11。**过程中实证出两条
引擎硬边界**（process 入口写外节点即停机；属性写是命令缓冲，同帧读旧值）
——见 §4，均已如实记录并成为设计的一部分。

---

## 1. 玩法与设计

- 敌人从顶部按波次下行（配额 = 2 + wave，速度随 wave 递增），到底漏怪
  （lives −1）；
- 点击屏幕建塔（花 10 金）：tower_ctl 点读 `game.gold` 判可建 → emit
  buy → **扣钱只在 game**；塔按 `NodeData.timer` 周期攻击射程内敌人
  （距离平方判定），击杀 emit reward（game +5 金 +1 kills）；
- 一波清空推进波次（spawner emit waveclear，game.wave +1）；lives 归零
  LOSE；900 帧基线战果见 §3。

### 1.1 四脚本拆分（F-1 属主模式）

| 脚本 | 入口 | 职责 | F-1 读面 | 写面 |
|---|---|---|---|---|
| `game` | `on "game"` | gold/lives/wave/kills **唯一写者** | —— | 仅自己 locals |
| `spawner` | `every` | 波次记账 + 选空位（pend 局部）+ 每帧 emit "sim" | `game.wave`（配额/速度/节奏） | 仅自己 locals |
| `tower_ctl` | `on "sim"` / `on "tick"` | 落笔刷怪（读 `spawner.pend`）、建塔、计时、命中判定、敌人回收 | `game.gold`（可建判定）、`spawner.pend`/`spawned` | 哑实体 props（信号入口合法） |
| `hud_ui` | `on "tick"` | 每帧拼 HUD 文本 | `game.gold/lives/wave/kills` 一行全点读 | `hud.text`（schema 键） |

**单信号通道复用**：一脚本一入口是既有约束（S7.4 实证），game 要收四种
事件 → 单通道 `"game"` + 载荷编码：ping=0 / buy=100+cost / leak=200 /
reward=300+amt / waveclear=400。**属主模式的完整形态**：状态住属主、
变更走信号、消费侧零镜像（T-TD-06 断言 hud/spawner 局部里没有 game
状态的副本）。

---

## 2. F-1 使用实证（本里程碑的核心问题）

四个脚本对 F-1 读面（属性表未命中 → Script 节点局部回退）的使用结果：

| 点读 | 使用者 | 频率 | 实证结果 |
|---|---|---|---|
| `game.gold` | tower_ctl（建塔判定）、hud_ui | 每帧 | ✓ 20→10（一次 buy）→ 随击杀每杀 +5；HUD 与属主局部逐帧一致 |
| `game.lives` | hud_ui | 每帧 | ✓ 10→8（两次漏怪），终局 LOSE 判定可由 game 自己做也可点读 |
| `game.wave` | spawner、hud_ui | 每帧 | ✓ 1→3；spawner 拿它算配额（2+wave）与速度/节奏 |
| `game.kills` | hud_ui | 每帧 | ✓ 0→5，HUD 同步 |
| `spawner.pend` / `spawned` | tower_ctl | 每帧 | ✓ 决策（选中的空位句柄）经 F-1 传给落笔侧——**读面替代了参数传递** |
| `hud.text` 终值 | （对照） | —— | 属性表命中的键走属性面，不进回退链（S11-1 T-SHARE-02 口径不变） |

**账本自洽即读面可信**：900 帧后 `gold(25) = 5 × kills(5)`、
`lives(8) = 10 − 漏怪(2)`、HUD = `"GOLD:25 LIVES:8 WAVE:3 KILLS:5"`
——三条账全由跨脚本点读汇出，无一条走属性面或第二总线。

---

## 3. 契约回归（T-TD-01..07）

`nes-runtime/tests/tower_defense.rs`（3 例）+ `tower_defense_semantic.rs`
（4 例），全部装载**仓库真实资产** `examples/assets/tower_defense.ron` +
输入轨迹 `tower_defense_trace.txt`（两次点击建塔），与 headless 同管线逐帧：

| 编号 | 断言 |
|---|---|
| T-TD-01 | 帧 100：一次建塔扣 10 金（20→10），HUD 同步 `GOLD:10 LIVES:10 WAVE:1 KILLS:0` |
| T-TD-02 | 帧 900：kills ≥ 4；账本 `gold = 5 × kills`（唯一信号写路径）；漏怪扣命（lives < 10）且防线未破；波次推进 ≥ 3 |
| T-TD-03 | 同场景同轨迹双跑：逐帧语义指纹全等（确定性） |
| T-TDS-01..04 | 语义不变量：初局状态 / 首塔恰好 10 金 / 全程不变量（塔数 ≤ 2、账本闭合）/ 无塔必 LOSE |

终局账本（900 帧，确定性）：`gold 25 / kills 5 / lives 8 / wave 3`，
HUD 终值 `GOLD:25 LIVES:8 WAVE:3 KILLS:5`。

---

## 4. 摩擦记录（引擎硬边界的如实实证）

1. **process（every）入口只许写自身——哑实体也不豁免**。spawner/
   tower_ctl 初版挂 `every` 直接写敌人/塔的 pos/visible，当场
   `__halt = "process 入口只许写自身"`（`Op::SetProp`/`SetT` 的 NodeCtx
   纪律，写外节点即停机不停帧）。**解法**：spawner 收缩为纯规划器
  （every 里只动自己 locals + emit "sim"），落笔移到 `on "sim"` 信号侧
   ——信号入口写任意节点是既有合法路径（first_game/dungeon 同款）。
   这不是 bug 而是冻结过的纪律，但"哑实体豁免"的直觉是错的，本里程碑
   把它钉死成文。
2. **属性写是命令缓冲（S6 Diff 式回写的推论）：同帧写后读读到旧值**。
   spawner 在同一 tick 内 `it.visible = true` 后立刻数 alive，读到的还是
   停机坪旧值 → 开局瞬间假 waveclear。**解法**：刷怪决策（pend）与落笔
   拆开（写经命令缓冲下一帧生效，规划侧每帧重新读真实状态）——设计向
   引擎的时序语义低头，而不是给写加旁路。消费侧"读 game.* 永远拿到已
   落盘的当帧终值"不受影响（F-1 读的是 locals，属主 SetLocal 即时）。
3. **`button()` 未接输入读面即停机**。headless 测试宿主最初没调
   `mount_input_view`，tower_ctl 一执行 `button("left")` 就
   `__halt = "button(\"left\") 未接输入读面（宿主未注入）"`。是宿主装配
   缺步而非引擎缺口——但报错落在 `__halt` 局部里，第一现场可观测性一般，
   记一笔：宿主 checklist（open → load_scene → attach → **mount_input_view**
   → frame）缺一不可。
4. **单通道 + 载荷编码是"一脚本一入口"约束下的真实成本**。四种事件挤一个
   `"game"` 通道后，game 的处理器成了一台载荷译码器（阈值分桶 0/100/200/
   300/400）。能用，但可读性税明确——多入口脚本（一个脚本挂多个 `on`）
   值得列为未来语法扩展候选（与 S7.4 摩擦记录合并追踪）。

---

## 5. 回归证据

```text
nes-asset    34 通过 / 0 失败
nes-scene   213 通过 / 0 失败（含 T-SHARE-01..03）
nes-render-api    44 / 0
nes-render-extract 42 / 0
nes-render-wgpu   89 / 0
nes-runtime  62 通过 / 0 失败（含 T-TD-01..03 + T-TDS-01..04 新 7 例）
合计        484 通过 / 0 失败
clippy      六 crate 全 0 警告
守卫        check_dependency_direction.py 11/11 通过
headless CLI 双跑（900 帧）：trace hash d54e1d9a1a803655 两跑全等
```

Dodge 等既有项目零影响：本里程碑零引擎改动（纯场景资产 + 测试 +
文档），F-1 语义由 S11-1 冻结，T-SHARE 全绿未动。

---

## 6. 后续

- **F-4 编辑器工具**（A 类）：Inspector "从文件挂载脚本"/脚本面板对
  四脚本协作场景的友好化——后置
- **多入口脚本**（摩擦 4）：一个 Script 节点挂多个 `on` 的语法扩展候选，
  先在下一个真实项目里再估一次成本
- **F-8 for_each 性能**（D 类）：900 帧 × 多个 for_each 无实测瓶颈，不动
- 回扣 S11-0 审计表：F-1 经三个项目（Dodge 形态 / Mini Dungeon /
  哨塔防线）验证非过拟合；剩余 B 类缺口无新增。

## 7. 里程碑记注（M4 口径）

- 渲染侧零改动（复用 enemy/bullet 纹理与既有 HUD 字体管线）
- 引擎（nes-scene/nes-runtime src）零改动：全部增量 = 场景资产 +
  测试 + 文档——第三项目是对既有契约的**消费**，不是对引擎的修改
