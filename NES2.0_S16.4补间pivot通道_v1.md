# NES 2.0 · S16.4 补间 pivot 通道 v1

## 0. 结论

补间系统照 alpha 通道的同款模式新增第 4 条语句通道 `tween_pivot`（第 5 个
`TweenChannel` 变体）：把 Sprite2D 的 `pivot` 属性（S16.3 引入的归一化锚点，
Vec2）从当前值按缓动/模式推向终点。推进走**既有属性写路径**（`set_prop`，
进语义指纹，与 alpha/frame 同口径）；**零渲染改动** —— 提取层本就每帧直读
`pivot` 属性推 `SetPivot`（S16.3 既有路径），补间写属性后渲染自动跟随。
门禁全绿：十 crate `cargo test --release` 767 通过（基线 764 + 新增 3）、
clippy `-D warnings` ×10 零告警、依赖方向守卫 15/15、tween_demo / frame_demo
冒烟正常退出。既有三通道（pos/scale/alpha）与 frame 通道逐位不变
（无补间场景指纹零混入的口径未被触碰）。

## 1. 通道全景

| 通道 | 语句（形态） | 写入面 | 写入路径 | 引入 |
|---|---|---|---|---|
| pos | `tween_pos "name" x y ms [easing] [mode]` | `Transform2D.pos` | `set_local`（脏标记） | S16 第 1 期 |
| scale | `tween_scale "name" sx sy ms [easing] [mode]` | `Transform2D.scale` | `set_local`（脏标记） | S16.1 |
| alpha | `tween_alpha "name" a ms [easing] [mode]` | Sprite2D `alpha`（f32，落地夹取 0..1） | 属性写（进指纹） | S16.1 |
| frame | `tween_frame "name" from to ms [easing] [mode]` | Sprite2D `frame`（i64，线性插值 floor） | 属性写（进指纹） | S16.2 |
| **pivot** | `tween_pivot "name" px py ms [easing] [mode]` | Sprite2D `pivot`（Vec2 归一化） | 属性写（进指纹） | **S16.4（本期）** |

统一语义（全部通道共用）：last-wins 按（节点，通道）二元组；`from` 在 Cmd
落地时采样当前实际值；缓动 te = ease(t)；once/yoyo/loop 模式；完成发
`tween_done`（载荷 = 节点名，loop 永不到站）；登记表进语义指纹（条件混入，
非空才摺进）；`tween_stop "name"` = **全部通道**一并停（Pivot 臂天然包含，
 retain 按 target 不分通道，无需改码 —— 测试钉住）。

### pivot 属性写路径与渲染跟随

- **登记**：`Cmd::TweenPivot` 落地时读目标当前 `pivot` 属性作 `from`
  （缺失按缺省 `(0,0)`，与 schema 缺省及渲染"无记录 = 无平移"同口径）；
  px/py 越界照实接受不钳制（S16.3 越界锚定 = 锚点落在精灵外的合法语义）。
- **推进**：`apply_tween`/`land_tween` 的 Pivot 臂 x/y 按同一插值量各自推进
  （照 scale 的形状），经 `set_prop(id, "pivot", Value::Vec2)` 写入 ——
  真实树状态，进语义指纹；目标非 Sprite2D 时写入静默无效（照 SetProp 口径）。
- **渲染跟随（零新路径）**：提取层 S16.3 既有逻辑每帧直读 `pivot` 属性
  —— 非 `(0,0)` 逐帧推 `SetPivot`、`(0,0)→非(0,0)` 与 `非(0,0)→(0,0)`
  迁移补推由 `pivot_active` 簿记负责。补间只写属性，渲染自动跟随；
  nes-scene / nes-render-extract 本期均无渲染侧代码改动。

## 2. 契约

| 编号 | 契约 | 落点 |
|---|---|---|
| T-TP-01 | 解析产物形状（px/py/ms 三表达式 + 可选缓动/模式尾缀）、字面量 ms <= 0 解析期报错、保留字；登记 from = 当前 pivot（缺省 (0,0)）；中点 (0.5,0.5)@t=0.5 -> **pivot 属性读面 = (0.25,0.25)**；完成落 (0.5,0.5) + 到站信号；终点 (-0.25,1.5) 越界照实接受不钳制 | `nes-scene/tests/s16_tween.rs::t_tp_01_*` |
| T-TP-02 | yoyo 去回往返（p=1 到 to 换向不落位、p=2 回零落 from + 移除 + 信号恰一次）；last-wins 中途换程 from = 含本帧推进的当前 pivot（不跳变、不叠加、elapsed 归零）；`tween_stop` 全通道含 pivot（pivot+pos 并存一并停、各停当前值） | `nes-scene/tests/s16_tween.rs::t_tp_02_*` |
| T-TP-03 | 渲染跟随（extractor 侧，全链：编译 -> VM 登记 -> 树推进写属性 -> 提取）：推进中 pivot 属性 (0.25,0.25) 非 (0,0) -> **SetPivot 被推**（`srv.pivot_of` 断言，照既有 alpha 簿记断言形态）；时满落位 (0.5,0.5) 渲染照收；缺省帧零 SetPivot | `nes-render-extract/tests/criterion_extract.rs::tween_pivot_render_follows_set_pivot` |

脚本面：`tween_pivot` 照 `tween_scale` 同构双表达式语句（弹序 ms、py、px），
可选缓动/模式尾缀、保留字、ms 解析期校验全同既有通道；`tween_stop` 零改动。

## 3. 门禁（worktree wt-tp，HEAD b721e36）

| 门禁 | 结果 |
|---|---|
| `cargo test --release` ×10 crate（逐个构建） | 全绿，合计 **767**（基线 764 + 新增 3：asset 34 / render-api 47 / scene 259 / audio 52 / media 27 / extract 60 / wgpu 136 / ext-api 7 / ext-js 29 / runtime 116） |
| `cargo clippy --release --all-targets -- -D warnings` ×10 | 零告警 ×10 |
| 依赖方向守卫 `check_dependency_direction.py` | **15/15**（首版文档注释命中 G5 的 `nes_render` 符号扫描，已改写为不含 crate 符号的表述） |
| tween_demo 冒烟（`NES_GAME_FRAMES=180`） | 正常退出 |
| frame_demo 冒烟（`NES_GAME_FRAMES=180`） | 正常退出 |

## 4. 遗留

- **暂停门控**：补间推进不受暂停门控（S16 冻结的 v1 口径），pivot 通道沿用；
  若后续做"暂停冻结补间"，五通道一并动，无 pivot 特例。
- **pivot 非有限值**：与 scale 通道同口径（不做运行时非有限拒收；解析期
  字面量非有限会被词法层拒）。alpha 的 `to.clamp(0..1)` 刻意不适用于 pivot
  （越界锚定是合法语义）。
- **编辑器补间面**：编辑器属性面板对 pivot 的展示/编辑归 S16.3 既有面；
  补间登记表在编辑器侧的可视化（时间轴一类）仍归后续里程碑。
- **`tween_demo` 未演示 pivot**：现 harness 只走 pos 对比手写步进；pivot
  观感验证由 T-TP-03 的提取侧契约与 S16.3 的渲染测试覆盖，demo 扩展归后续。
