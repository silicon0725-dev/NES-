# NES 2.0 S19.4/S19.5 Scene 类型标记 + 补间轨迹预览（蓝图收尾）v1

- 分支：`s19-4-marks-preview`（wt-marks 独立 worktree）；基线 HEAD `c124393`（S19.3 SIGNALS 面板）
- 改动面：`nes-runtime/examples/editor_shell.rs` 单文件（壳层投影 + 冒烟断言）；**零 crate 源码改动、零新依赖、依赖分层不变**
- 蓝图依据：`NES2.0_S19.0编辑器组织蓝图_v1.md` §3.3（Scene 树类型标记 + Q2 前缀字符集）、§4.5（S19.5 视口增强预览：补间轨迹线 + 九宫格面板实时预览）

## 0. 结论

1. **类型标记**：层级树 walk 投影行文本加类型前缀（等宽字体极简图标观，`* ` 选中标记同款形态）；行格式 `{indent}{* 或 空格}{prefix}{name}`，缩进/选中标记/行→uid 映射三逻辑逐位不动（行点击按映射查 uid，行文本只是视图）。
2. **轨迹预览**：主选中节点的活动 **Pos 通道**补间 → 视口内 12 点轨迹（from→to 线段等距、含两端）；无活动 Pos 补间 = 池整体熄灭（干净默认）；多补间只画登记序第一条（P0 口径，如实记录）。
3. **九宫格实时预览查证**（S19.5 遗留收口）：既有链路天然达成 —— 属性每帧投影 → 提取层每帧读 → SetNineSlice 每帧推，**编辑器改 ns_* 属性即下一帧反映**，零新代码（§3）。
4. **门禁**：十 crate `cargo test --release` **792/792**（基线 792 + 新增 0 —— 断言进 NES_EDIT_DEMO 钩子，不加 cargo test 计数）+ clippy `--all-targets` **0 警告 ×10** + 依赖守卫 **15/15** + editor_shell 双冒烟（120 帧干净退出 + NES_EDIT_DEMO 420 帧全断言）+ tween_demo/frame_demo/first_game 180 帧冒烟回归全过。

## 1. Scene 树类型标记（蓝图 §3.3）

### 1.1 前缀表（Q2 口径冻结，`kind_prefix` 单点映射）

| 类型 | 前缀 | 备注 |
|---|---|---|
| Sprite2D | `[S] ` | 蓝图 §3.3 原文 |
| Camera2D | `[C] ` | 蓝图 §3.3 原文 |
| Label | `[T] ` | 文本类 |
| Button | `[B] ` | |
| TextInput | `[X] ` | |
| Script | `[J] ` | 任务口径（Q2 表外补充 —— 脚本宿主节点在树里可见，须有标记） |
| Theme | `[H] ` | 任务口径（壳层 skips 含 theme_node 不会出现在编辑器树；映射照给 —— 用户场景里的 Theme 节点经 walk 照实标记） |
| Node / Node2D / Control / ScrollView / ListView / Tabs | （无前缀） | 蓝图 Q2 "无前缀 = 容器" |

- 判定源 = `tree.kind_tag(id)`（存活节点恒有类型）；死节点理论不可达（walk 只访存活），落缺省无前缀。
- 行宽预算：前缀 4 字符（3+空格）叠加在既有 `缩进+标记` 之上 —— 左栏 180px 下层级树行本就只做"能读"不做"截断"（既有行如 `    hud_ins_bg` 14 字照存），等宽 16px 位图回退模式下行宽变紧属既有取舍（与 S12-6 行宽纪律同账，真字体下无碍）。
- 交互零影响：行点击回调按**行→uid 映射**结算（walk 同次遍历产出），前缀只进行文本；`selected` 行下标、缩进、walk skips 全部不动。

### 1.2 NES_EDIT_DEMO 既有断言变更清单

**零既有断言改动** —— 既有断言面（Output 日志行、`demo_tl_rows`、Inspector 正文、树形态 `find_by_name`/children/prop 检查）均不引用层级树行文本；层级树行点击交互走映射不走文本，前缀不构成破坏面。新增断言（见 §4 前的第 4 条）：行文本含 `[S] obj1` / `[C] cam` / `[J] spin` / `[X] name_input` / `[T] hud_scene`、容器行 `    hud_tree` 无前缀、`traj` 不在层级树行文本中（walk skips 生效面）。

## 2. 补间轨迹预览（蓝图 §4.5）

### 2.1 点池（照网格条带池先例）

- `traj` 容器（Node）挂 root + 12 枚 `traj_dot`（Control，2x2px，`fill_slot=accent`，`visible=false` 备用）；`TRAJ_POOL=12`、`TRAJ_Z=6` 常量单点出。
- **walk skips**：`traj` 进层级树过滤表（观感节点不是场景对象，同 grid/ruler/dock/tldock/menubar 纪律）—— 树投影无感。
- **hit 护盾照 grid 先例 = 不进 over_ui**：精灵命中只滤 `Sprite2D`（traj 点是 Control 天然不拦）；2x2 点压在可编辑区按"注记不拦编辑点击"处理（同网格线 —— 点上去照常框选/选中），进护盾反而会在轨迹经过处吃掉精灵点击。
- **z 序查证**（`TRAJ_Z=6` 的依据）：既有 z 阶梯 = 网格 -100 < 标尺 -90 < dock/时间轴 -80/-79 < 菜单栏/工具带 -70 < 文件面板 -60 < **精灵 0** < **选中高亮 5** < **轨迹点 6** < 菜单弹层 90 < 框选 100。轨迹注记盖过精灵与高亮可见，永不盖编辑器顶层覆盖件；dock 系面板按既有"场景对象盖过观感"纪律本就低于精灵层，与轨迹点无交叠争议。

### 2.2 每帧投影（投影无状态口径）

- 数据面：`tree.tweens()` 登记序过滤 —— `target` uid == 主选中 uid（句柄 `to_id()` 回 NodeId 后经 `uid_of` 归一，死句柄 uid 已清自然不命中）**且**通道为 `TweenChannel::Pos`；`find_map` 取第一条（**多补间只画第一条，P0**）。
- 几何：12 点 t = i/11 沿 from→to 等距（两端全含）；点位 = from/to（补间登记的目标本地坐标）+ 父世界平移（`parent(p).world_position()` —— traj 挂 root，根坐标系对位；父不在原点也正确）。Vec2 无算子重载，分量手写插值（nes-scene 数学面零改动）。
- 熄灭：无选中 / 选中无活动 Pos 补间 / 补间到站移除 → 12 点全 `visible=false`（每帧覆写，无历史）。

### 2.3 语义边界（冻结）

轨迹是**编辑器会话可视化**：Control 池不进树逻辑语义、不进场景保存（编辑器 P0 无保存路径）、不进 headless 指纹（editor_shell 是壳层 example，不在指纹面内）；位置每帧覆写无历史。PLAY 快照/RESET 把池当普通节点数据往返（uid/NodeId 不动、位置下帧即被覆写）—— 无语义后效。

## 3. 九宫格实时预览查证（S19.5 遗留收口，零新代码）

链路逐环核实：

1. **壳层写**：编辑器改 `ns_tex/ns_l/ns_t/ns_r/ns_b/ns_modulate/ns_tiling` 走 `set_prop_raw` 前向通道（S18 皮肤写入器同款）—— 直入属性表；
2. **提取层读**：`nes-render-extract/src/extractor.rs` 每帧对每个 Control 调 `nine_slice_of(tree, node, source)`，`tree.prop` 逐属性现读（`set_prop_raw` 与 `prop` 同一张属性表，L2442/L2469 单点佐证）；
3. **推命令**：有效即逐帧 `server.set_nine_slice(...)`（照 tint/uv 全量快照口径）；失效迁移帧补推一次 IDENTITY 清除（`nines_active` 簿记）—— 关闭也即时反映。

**结论**：全量快照口径下属性投影天然实时 —— 编辑器改 ns_* 属性即下一帧上屏。S19.5 蓝图第二项（"换肤面板即时所见"）由既有机制达成，无需新代码；唯一缺口是 Inspector 尚无 ns_* 编辑行（见 §5）。

## 4. 门禁

| 项 | 结果 |
|---|---|
| 十 crate `cargo test --release` | **792/792 全绿**（nes-asset 34 / nes-audio 52 / nes-extension-api 7 / nes-scene 271 / nes-render-api 48 / nes-render-extract 65 / nes-media 27 / nes-extension-js 29 / nes-render-wgpu 143 / nes-runtime 116；基线 792 + 新增 0） |
| `cargo clippy --release --all-targets` | **0 警告 ×10** |
| `check_dependency_direction.py` | **15/15** |
| editor_shell 冒烟 | `NES_GAME_FRAMES=120` 干净退出；`NES_EDIT_FRAMES=420 NES_EDIT_DEMO=1` 全断言通过（连跑 2 次稳定） |
| 回归冒烟 | tween_demo / frame_demo / first_game(Dodge) 各 180 帧干净退出 |

**新增断言（NES_EDIT_DEMO 钩子内，滞容闩锁口径）**：

- **轨迹三态**：①APPLY 前窗（帧 240..=280，obj1 恒主选中、无补间）全灭；②活动窗（帧 ≥284、登记表出现 Pos 补间）点亮且端点对位 —— dot0=(280,130)=from、dot11=(2,4)=to（0.5px 容差；演示流未挪 obj1，from 即装配位）；③到站后（`demo_tl_seen_done` 闩住后）复灭。读面 = traj 池树节点的 `visible`/`offset` 属性（traj 池是真实树节点，headless/钩子同数据面可断言）。
- **类型标记**：行文本含 `[S] obj1`、`[C] cam`、`[J] spin`、`[X] name_input`、`[T] hud_scene`；容器行 `    hud_tree` 无前缀；`!contains("traj")`（walk skips 生效面）。

## 5. 遗留

1. **S19 蓝图全清声明**：§3.1 Inspector 重组（S19.2）、§3.2 信号面板（S19.3）、§3.3 Scene 类型标记（本轮）、§3.4 全局舞台控制收敛（S19.1 播放组迁菜单栏即达成）、§4.1 顶部菜单栏（S19.1）、§4.3 Inspector 分区折叠（S19.2）、§4.5 视口增强预览（本轮：补间轨迹 + 九宫格实时预览查证）—— **S19.1..S19.5 里程碑表全部落地，蓝图收尾**。
2. **真图标集**：`[S]` 等文本前缀是"角色缩略图观的文本等价"（蓝图 §3.3 原文）；真位图图标归 icon 集里程碑（真字体已就位、位图图标可后置）。
3. **轨迹 P0 边界**：多补间只画第一条；Scale/Alpha/Frame/Pivot 通道不画；轨迹是直线段不采样缓动曲线（缓动形状已在时间轴行/进度条可视化）。曲线采样与多轨迹并归后续。
4. **Inspector 无 ns_* 编辑行**：九宫格属性目前经脚本/代码写；Inspector 表单化编辑归属性面板后续轮（查证已证实时链路就绪，缺的只是编辑 UI）。
5. **层级树行宽**：位图回退模式（无系统字体机器）下前缀 4 字符挤占行宽属既有取舍；真字体为默认环境，影响面与 S12-6 行宽纪律同账。
