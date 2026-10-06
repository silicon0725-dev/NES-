# NES 2.0 S19.1 顶部菜单栏（Godot 骨架件）v1

- 分支：`s19-1-menubar`（wt-menu 独立 worktree）；基线 HEAD `01ef7d1`（S19.0 蓝图）
- 改动面：**仅 `nes-runtime/examples/editor_shell.rs`**（壳层件）—— 十 crate 源码零改动、零新依赖
- 蓝图依据：`NES2.0_S19.0编辑器组织蓝图_v1.md` §4.1（窗口顶 20px 菜单栏：Scene/Project/Debug/Help + 右端播放组迁入 + 快捷键表 Help 项）

## 0. 结论

1. 顶部菜单栏落地：窗口顶 20px 全宽条 + 四顶层项（Scene/Project/Debug/Help，x=16/88/184/264 冻结位）+ 下拉菜单（九宫格小面板铺底 + 项底板/文本池，悬停 selected 槽高亮）。开合状态 = 编辑器会话态（`open_menu: Option<usize>`，不进树不进指纹）。
2. 播放组迁入：PLAY/STOP/RESET 自视口工具栏迁到菜单栏右端（y=0 满高 20px，x 右缘锚定每帧投影）——Godot 播放按钮位；视口工具带只剩 SEL/SNAP/GRID。控件语义（on_activate 落账、PLAY* 后缀、快照/RESET 数据面还原）逐位不动。
3. 下移连锁：标尺/视口/左右面板的顶部让位从 `TOP_BAND`(40) 变为 `MENU_H + TOP_BAND`(60) 起；标尺从 64 移到 84，可编辑区顶从 80 移到 100，底部三带（时间轴/Output/状态栏）不动。
4. 菜单内容 P0 全部为已有功能的菜单化，**零新行为**；两处无既有能力的项（New Scene / Save Scene）如实报 `not in beta`（照 S12-8 .ron 双击先例）。快捷键全部照旧（F5/Shift+F5/F6/F7/F8/F9/Ctrl+Z/Ctrl+Y/U/E/Delete/Tab/方向键）—— 菜单只是快捷键的可视化入口。
5. 命中序：菜单命中（顶层项带 → 下拉项 → 收起）先于一切编辑点击路径；菜单开着时视口第一击只收菜单不产生编辑动作（Godot 口径）。下拉控件全部进 hit 护盾 + `menubar` 容器进 walk skips（照 tldock 先例）。
6. 冒烟钩子扩展：NES_EDIT_DEMO 新增菜单链路（开 Debug → 断言弹层 Control 可见面 + 项文本 → 切 Help → 视口空白第一击只收菜单 → 重开 Help → 点 Shortcut Table → Output 快捷键表断言）；既有断言（挂载/play/stop/reset/时间轴 APPLY/IME/音乐/字体）全部保持。

## 1. 布局与迁移

### 1.1 菜单栏条（蓝图 §4.1）

- 高 `MENU_H = 20`，全宽；`fill_slot="panel"` 铺底 + 底缘 1px `border` 分隔线（与视口工具带同语言）。
- **铺底选型**：九宫格面板皮肤（panel_skin.bmp）在 20px 高度下上下边带（8+8=16px）吃掉条带 80%，观感不稳 → 弃用，取 fill_slot 深色（观感稳者）。下拉弹层面板高度 28..68px，皮肤边带比例健康 → 用九宫格（与 dock 同观感语言）。
- 顶层项 = Label ×4（`MENU_ITEM_X = [16, 88, 184, 264]`，y=3 使 14px 文本在 20px 内居中），text 槽色；**打开项 accent 槽**（会话态投影，每帧重写）。
- 命中带 = 相邻项 x 区间 × 菜单栏高（末位 Help 宽 72 单点出：`MENU_HIT_LAST_W`）；z=-70（工具带同款"场景对象优先于观感"纪律）。

### 1.2 播放组迁入（S12-9 三键 → 菜单栏右端）

- 按钮本体挂在 `menubar` 容器下（新建，名字沿用 tool_play/tool_stop/tool_reset —— on_activate 映射、demo 取样、落账段零改动）；底板池 `tool_plates` 仍开 6 槽：前 3 槽随编辑三键、后 3 槽随播放组（迁位不换控件）。
- 位置每帧投影：`x = cw - 4 - 48 - (2-k)*52`（k=0..2 = PLAY/STOP/RESET，768 宽下 612/664/716 起），`y = 0`（按钮高 20 == 菜单栏高，满高嵌条内）。PLAY 运行中 `*` 后缀照旧。
- 视口工具带（y = 60..84）只剩 SEL/SNAP/GRID；工具带底缘 1px 分隔线、按钮四态（纹理/hover/pressed accent）逐位不动。

### 1.3 下移连锁清单（全部进每帧布局投影块）

| 元素 | 原 y | 新 y（+MENU_H=20） |
|---|---|---|
| 菜单栏条 | — | 0..20（新增） |
| 左/右面板标题（Scene/Inspector 文本） | 12 | 32 |
| 右检查器面板底 | 8 起 | 28 起 |
| 左栏（层级树列表顶） | 40（TOP_BAND） | 60 |
| 视口工具带（SEL/SNAP/GRID） | 40..64 | 60..84 |
| 标尺（ruler_y） | 64 | 84 |
| 可编辑区顶（vy0） | 80 | 100 |
| 左栏可用高（avail_h） | ch-40-底带 | ch-60-底带（432 窗：162→142） |
| 时间轴/Output dock/状态栏 | 底部锚定 | 不动 |

底部三带不动 → 时间轴/Output 的 demo 点击坐标不受影响；受影响的既有 demo 坐标（RESET 按钮、改名输入框、fs 双击行）已逐一重算（见 §4）。

## 2. 菜单内容表（每项 → 行为映射）

P0 纪律：**全部为已有功能的菜单化，不新增行为**；无既有能力者如实报 `not in beta`（照 S12-8 .ron 双击 "open → play-in-editor milestone" 先例）。日志行全 ASCII。

| 菜单 | 项 | 显示 | 点击行为 | 依据/查证 |
|---|---|---|---|---|
| Scene | New Scene | 静态 | Output 报 `new scene: not in beta (P0)`，不落账 | **查证：无既有"清空树重建"能力**——树重建会换 NodeId，壳层手柄/行映射全散（S12-9 RESET 裁决过同款边界）；归后续里程碑 |
| Scene | Save Scene | 静态 | Output 报 `save scene: not in beta (P0)`，不落账 | **查证：`rt.save_scene` API 存在（nes-runtime/src/lib.rs:495），但编辑器无既有 Ctrl+S 接线、保存路径口径未冻结**（写盘会污染资产根）——任务书所称"既有 Ctrl+S 保存路径"实际不存在，如实报告并按 not in beta 处理 |
| Scene | Load via FileSystem (F9) | 静态 | 切 F9 files 档（`fs_focus=true`，既有行为）+ Output 提示 `load: FileSystem dock (F9 -> files)` | F9 分割档切换的菜单化 |
| Project | Audio: On/Off | 现态后缀（`rt.audio_open()` 读面） | Off→点击经 `open_audio`（幂等）开 + `audio on`；**On→点击只报 `audio: close not in P0 (no api)`** | **查证：无关闭音频 API**（nes-audio 设备一次装配、STOP/编辑不关是既有口径）——任务书预案"显示态不可点"按更诚实的落地：Off 向可用（既有幂等 API）、On 向只报行，§5 记录 |
| Project | Extensions: N loaded | 现态后缀（`rt.extension_count()`） | Output 列已装载清单：`extensions: N loaded` + 每扩展一行 `ext: <文件名>`（宿主装载时收集的 `ext_loaded`） | 扩展装载日志的菜单化 |
| Debug | Show Diagnostics: On/Off | 现态后缀（会话态 `diag_on`） | 翻转 `diag_on` + `diagnostics on/off`；**状态栏尾部切换为诊断段** `\| diag underruns:N faults:N ext:N` | **查证：读面可达，已接线**——`nes_audio::underruns()`（设备队列打干计数）、`rt.extension_faults()`（扩展/JS 故障累计）、`rt.extension_count()`（已装载扩展数）。任务书"js 计数"映射到后两者（扩展数 + 故障数） |
| Help | Shortcut Table | 静态 | Output 打印 9 行快捷键表（`SHORTCUT_TABLE` 常量，全 ASCII） | 实现选型：Output 打印（选稳者）——弹层 ListView 需要第二套弹层开合/关闭交互，P0 不做；为此 `EDITOR_LOG_KEEP` 29→48（帮助表 9 行 + 菜单操作 ~6 行，既有断言行余量保住） |

## 3. 开合状态机与命中序

### 3.1 状态机

- 状态：`open_menu: Option<usize>`（None = 全收；Some(0..=3) = Scene/Project/Debug/Help）。编辑器会话态：不进树、不落盘、不进指纹（与工具栏开关/分割档同一纪律）。
- 迁移：
  - 点顶层项 i：`open_menu == Some(i)` → None（再点收起）；否则 → `Some(i)`（打开/切换）。
  - 点下拉项 (m, i)：执行菜单项（§2 行为表）→ None。
  - 点其它任何处（视口/其它 UI/下拉衬边）：→ None。
  - Esc：`open_menu.is_some()` → None（Godot 口径第三条收起路径；无菜单时 Esc 照旧走既有路径——改名草稿回滚不受影响）。

### 3.2 命中序（帧首点下沿结算，先于一切编辑点击路径）

```
点下沿（mouse_left_held 且非前帧）
① 顶层项带命中（MENU_ITEM_X 相邻区间 × [0,20)）→ 开合切换，消费
② 下拉项命中（上一帧 menu_item_rows 矩形 + open_menu 匹配）→ 执行 + 收起，消费
③ open_menu 非空（其余一切落点）→ 只收菜单，消费 ← Godot：点外部关菜单不产生编辑动作
④ 未被菜单消费 → 既有编辑点击路径原样（框选/选中/标题折叠/护盾判定序不变）
```

- `menu_ate_click` 门：菜单消费的按下沿不进既有两条点击分支（第一击只收菜单——不清选中、不框选、不折叠）。
- 下拉项矩形 = **上一帧**投影产出（`menu_item_rows`，帧首命中）——一帧滞后与既有 UI 命中同口径（title_rows 先例）；悬停高亮用**本帧**鼠标位 × 本帧投影矩形（投影无状态）。
- 护盾与 skips：`menu_bg`/`menu_pop_bg` + 项底板池全进 `over_ui` press_in_control 数组（菜单收着时压菜单条也不清选中不框选——播放组按钮迁入后仍护，Godot：点播放不清选中）；`menubar` 容器进 walk skips（含播放组按钮——按钮不是场景对象）。
- z 纪律：菜单条 z=-70（常驻观感件"场景对象优先"同款）；**下拉弹层 z=90** —— 瞬时 UI 盖过场景对象（Godot popup 口径的显式例外；仍压不过选中框 z=100）。
- 快捷键零改动：F5/Shift+F5/F6/F7/F8/F9/Ctrl+Z/Ctrl+Y/U/E/Delete/Tab/方向键/数字 0 检测路径原样（菜单开着时快捷键照常生效——菜单不是模态）。

## 4. 门禁

| 项 | 结果 |
|---|---|
| `cargo test --release` ×10 crate | **787 passed / 0 failed**（基线 01ef7d1 实测 787，本里程碑零新增测试 —— 壳层件） |
| `cargo clippy --release --all-targets` ×10 | **0 警告 ×10** |
| 依赖方向守卫 `check_dependency_direction.py` | **15/15 通过** |
| editor_shell 冒烟：120 帧干净窗 | 干净退出 |
| editor_shell 冒烟：`NES_EDIT_DEMO=1 NES_GAME_FRAMES=420` | 全链路断言通过（新增菜单链路 4 断言：下拉曾可见 / Debug 项文本含 "Show Diagnostics" / 外点收起 / Output 快捷键表行；+ 收尾互证 obj1 仍主选中 = 外点未产生编辑动作），连跑 3 次稳定 |
| first_game(Dodge) / tween_demo 冒烟 | NES_GAME_FRAMES=240/180 干净退出（回归） |

demo 坐标重算记录（768x432）：RESET 点击 (476,52)→**(740,10)**（播放组迁位）；改名输入框点击 (600,145)→**(600,166)**（input_y 136→156）；fs 双击行 y 189→**(198)**（Media 在场分支：fs 列表顶 123.2→135.2、spin 行 7 顶 193.2；Media 缺席分支 188 不变仍命中行 5）。

已知抖动（HEAD 既有、非本里程碑引入）：注入钩子是"每帧一条"粒度，宿主偶发卡帧会把相邻 down/up 合并进同一输入快照导致点击沿丢失（本次基线验证时冷首跑复现一次 fs 双击丢失，热跑稳定）——缓解：菜单链路断言全部改为**滞容闩锁**（窗口内逐帧观察，单帧抖动不扑空，照 S12-11 IME 闩锁先例）；根修（注入重试/状态注入通道）归 §5。

## 5. 遗留（后续里程碑/Q1）

1. **submenu 二级**：Scene/Project 项未来扩组（如 File > Open Recent）；P0 单层下拉。
2. **图标**：顶层项/菜单项无图标（绿旗/停旗等 Godot 图标位观感）；需图标图集通道。
3. **菜单进 schema**（蓝图 Q1）：菜单是宿主壳层件，不进树不进指纹；"编辑器自身 UI 也是场景"的控件即节点延伸归 Q1 裁决。
4. **New Scene / Save Scene**：树重建的 NodeId 稳定性方案（句柄重绑或 uid 锚定重建）+ 保存路径口径冻结后菜单化；`rt.save_scene` 运行时 API 已就绪。
5. **音频关闭 API**：`close_audio`（设备停机 + 混音器保活口径）落地后，Audio 菜单项升级为真开关。
6. **诊断段细化**：任务书"js 计数"当前映射 = 扩展数 + 扩展故障数；逐扩展 JS 调用计数需扩展管理器新增读面（nes-runtime 源码改动，本里程碑零源码纪律不碰）。
7. **注入钩子抗抖**：冒烟注入的 down/up 合并丢沿问题的通道级根修（见 §4 已知抖动）。
8. Help 弹层 ListView 展示形态（当前 Output 打印）；菜单键盘导航（方向键/Enter）。
