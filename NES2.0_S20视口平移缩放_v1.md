# NES 2.0 S20 视口平移缩放 v1

- 分支：`s20-panzoom`（wt-panzoom 独立 worktree）；基线 HEAD `9ebf084`（S19.6 Scene 树真图标集）
- 追平轮 `s20-1-play-view-fix`（wt-playview 独立 worktree；基线 HEAD `b17a46f`）：实测"点 PLAY 后 UI 乱飞"根因修复 —— §3.5 运行态视图与 UI 锚定（active_view 单点 + 护盾拆分 + gizmo 退场），单文件改动照旧
- 追平轮 `s20-2-stage-bounds`（wt-stage 独立 worktree；基线 HEAD `250be95`）：舞台边界三件套（§8）—— 用户观察"物体可拖出场景无限制/无提示"的完整回应，单文件改动照旧
- 改动面：`nes-runtime/examples/editor_shell.rs` 单文件（壳层相机会话态 + 换算 + 投影 + 冒烟）+ `.gitignore`（Ctrl+S 演示保存目标不入库）；**零 crate 源码改动、零新依赖、依赖分层不变**
- 用户基准：Godot 2D 工作区 —— 滚轮缩放朝光标、中键拖拽平移、标尺随缩放自适应、缩放百分比显示

## 0. 结论

1. **编辑器视口相机落地**：`EditorCam { center: (f32,f32), zoom: f32 }` 会话态（不进树逻辑、不进指纹 —— 编辑器视图不是游戏状态），zoom clamp 0.1..8.0。**关键既有假设被打破**：视口旧注释声明"视图空间 == 世界空间（相机每帧置中恒等映射）"，鼠标→世界、点击选择、框选、gizmo 拖拽、命中全部建立在此假设上；本轮起全部换算收敛到单点屏幕↔世界助手（`EditorCam::screen_to_world` / `world_to_screen`）。
2. **相机换算式（查证后的契约结论，修正了任务书里 "scale = 1/zoom" 的直觉猜想）**：引擎相机缩放权威在 `Camera2D` 节点的 **`zoom` 属性**（schema："缩放倍数。越大画面越近"，合法域 0.05..16；相机节点**自身变换缩放不参与视图矩阵** —— `nes-render-api` `Camera2DState` 契约冻结 + S19.1 像素契约测试 `t_camera_*` 同源）。视图矩阵冻结式 `view = T(viewport/2) ∘ S(zoom) ∘ T(-center) ∘ R(-rotation)`，即 **`screen = viewport_center + zoom × (world − center)`**。故每帧（extract 前）驱动式为 **`cam.pos = center` + `cam.zoom 属性 = EditorCam.zoom`**；zoom=1 + 初始 center=(开窗/2) 时与旧"置中恒等映射"逐位同值（既有固定坐标断言全部保持）。
3. **场景数据保护三时机**（编辑视图不进场景文件、不进游戏运行态）：装载时 stash cam 原始 local transform + zoom 属性；**Save（Ctrl+S / Scene>Save）前还原 stash、保存后重应用编辑视图**；**PLAY 时还原场景相机**（首 PLAY 的全树快照在还原之后捕获 ⇒ RESET 回到的也是场景相机），STOP/RESET 后重应用编辑视图。
4. **渲染面事实（投影改造依据）**：Control 类（有 SetRect 状态）走 HUD 口径（渲染侧经视图矩阵的**逆**折回世界 —— 钉屏幕像素，相机不动它）；**纯 Label 与 Sprite2D 走世界变换**（吃视图矩阵）。故网格条带/标尺刻度/选中框/轨迹点（Control）由投影换算出屏幕位；全部 UI Label 与 S19.6 图标精灵经 `place_at_screen` 反向放置（世界位 = screen_to_world(屏幕位) + 本地缩放 1/zoom —— 视图 zoom 与本地 1/zoom 相抵，屏上恒定 1:1 尺寸与位置）。
5. **门禁（追平轮复跑）**：十 crate `cargo test --release` **792/792** 全绿（nes-asset 34 + nes-scene 271 + nes-render-api 48 + nes-render-extract 65 + nes-audio 52 + nes-media 27 + nes-extension-api 7 + nes-extension-js 29 + nes-render-wgpu 143 + nes-runtime 116 —— 基线面零改动）+ editor_shell 单元测试 **15/15**（追平轮 +2：活动视图单点 / UI 摆位量化证据）+ clippy `--all-targets` **0 警告 ×10** + 依赖守卫 **15/15** + editor_shell 双冒烟（120 帧干净退出 + NES_EDIT_DEMO 420 帧全断言 —— 含运行态 UI 锚定三闩锁）+ first_game 180 帧冒烟。

## 1. EditorCam 与三时机还原语义

### 1.1 会话态形状

```text
EditorCam { center: (f32, f32),   // 视口中心的世界坐标
            zoom: f32 }           // clamp 0.1..8.0（NaN 归 1）
CamRig { node,                    // 场景 Camera2D 节点句柄
         stash_transform,         // 装载时的 local transform
         stash_zoom: Option<f32>, // 装载时的 zoom 属性（None = 键不存在）
         cam: EditorCam }
```

- 会话态纪律与工具栏三开关/F9 分割档/菜单开合同款：不进树逻辑、不进指纹。cam 节点上的 `zoom` 属性**是**树数据（引擎相机契约字段），但编辑视图对它的每帧写入受三时机保护（下），场景文件与游戏运行态看到的永远是场景定义值。
- 初始值：center = 装配开窗中心（OPEN_CLIENT/2 = 384,216）、zoom = 1 —— 与旧"相机置中 (cw/2, ch/2)"逐位同值，编辑器首帧观感零漂移。
- stash 时机：树装配 `apply_pending` 之后、首帧投影之前（编辑视图尚未写过 cam）。schema 属性"出生即满配"（未写的 zoom = 缺省 F32(1.0) 出生在属性表上），stash 如实捕获该值；显式摘过键的节点 stash None，还原走 `remove_prop` 分支（防御路径，单元测试覆盖两条）。

### 1.2 三时机清单

| 时机 | 动作 | 落点 |
|---|---|---|
| **Save**（Ctrl+S / Scene>Save，S20 起为真实保存） | `restore_scene`（stash 写回）→ `rt.save_scene("Scenes/editor_shell.ron")` → `apply_editor`（编辑视图重应用） | `save_scene_protected` 单点；场景文件里的相机 = 场景定义值，不受编辑视图污染 |
| **PLAY**（F5 / PLAY 按钮，两处入口） | `restore_scene` 在 `play.start` **之前** —— 首 PLAY 的全树快照（RESET 的还原基准）在还原之后捕获 ⇒ RESET 回到的也是场景相机；运行态投影 `!playing` 护盾**停写** cam（游戏/脚本自由驱动场景相机） | F5 键路径 + 工具按钮路径，同一段还原前置 |
| **STOP / RESET** | 重应用编辑视图：投影护盾天然重应用（STOP 后当帧/下一帧）；RESET 落账点再显式 `apply_editor` 一次（快照把 cam 还原成场景值 —— 免下一帧投影前的一帧场景相机闪烁；运行态 RESET 被拒时不动游戏相机） | 投影 `if !play.playing` + `reset` 落账臂 |

- Ctrl+S 在改名框持焦时让位输入（与挂载流同焦点门）；运行态 Ctrl+S / 菜单 Save 均让路（相机还原语义是编辑态时机，菜单 Save 如实报 "save: stop first (playing)"）。
- 保存目标 `Scenes/editor_shell.ron`（相对资产根）：演示场景是内存装配、无磁盘来源可回写，落固定演示路径；目录入 `.gitignore`（运行期产物不入库，Media/*.amv 先例）。

## 2. 换算单点与交互改造清单

### 2.1 单点换算（契约式）

```text
screen_to_world(s) = center + (s − viewport_center) / zoom
world_to_screen(w) = viewport_center + (w − center) × zoom
viewport_center = (cw/2, ch/2)   // 相机视图覆盖全客户区；面板是画在上面的 HUD
```

### 2.2 改造清单

| 交互 | 改造 | zoom=1 行为 |
|---|---|---|
| 点击选择 | 鼠标 → 世界域后进命中；`sprite_hit_origin` 16px 盒是**世界单位** —— 缩放下选择盒随 zoom 缩放（放大后精灵屏显 >16px，命中盒按世界单位仍是 16 —— 引擎命中契约不动，编辑器点击换算到世界后行为自洽，**无分叉**） | 逐位保持 |
| gizmo 拖拽 | 拖拽偏移量在世界域记录（`mouse_world − world.tx`）—— 1/zoom 折算天然含在偏移里，任意缩放下 1:1 跟手；Ctrl/SNAP 吸附仍取整到 8px **世界**栅格 | 逐位保持 |
| 框选 | 起终点 → 世界域；矩形与命中判定（盒中心 probe）全在世界域 | 逐位保持 |
| 方向键移动 | 不变（本就是世界单位） | 逐位保持 |
| hit 护盾 / 菜单命中 / 标题行命中 / UiVm 行点击 | 全在屏幕域，不改（面板是 HUD，不吃相机） | 逐位保持 |
| 投影：网格条带/标尺刻度（Control） | 世界锚 → world_to_screen 换算屏位（1px 屏宽细条不随 zoom 变粗 —— Godot 网格观感） | 逐位保持 |
| 投影：选中框 | 屏位换算 + 尺寸 ×zoom（精灵屏显 16×zoom，框 20×zoom 保持 2px 内衬观感） | 逐位保持 |
| 投影：轨迹点（S19.5） | 屏位换算；2×2px 注记尺寸不随 zoom 缩放（编辑器注记带恒定观感） | 逐位保持 |
| 投影：全部 UI Label + S19.6 图标精灵 | `place_at_screen` 反向放置（世界位 = screen_to_world(屏位)、本地缩放 1/zoom 相抵视图缩放）—— hud_scene/menu 顶层项等装配期定位同步退役为每帧投影 | 逐位保持（pos 与旧直写同值、scale=1） |

- 视带几何收敛为 `BandRects::compute` 单点（S12-4/S12-6/S12-7/S19.1 冻结式的出口），帧首算一次、输入段（滚轮命中域判定）与投影段共用 —— 杜绝两处口径漂移。

## 3. 滚轮缩放朝光标（公式）与中键平移

### 3.1 缩放朝光标

- `wheel.y > 0` 放大 ×1.15、`< 0` 缩小 ÷1.15（clamp 0.1..8.0）。标准式：

```text
mouse_world   = center + (s − viewport_center) / zoom        // 缩放前
zoom_new      = clamp(zoom × factor, 0.1, 8.0)
center_new    = mouse_world − (s − viewport_center) / zoom_new
```

代入 `screen_to_world` 即"**缩放前后光标下的世界点保持不动**"的恒等式（纯函数断言覆盖放大/缩小两向 × clamp 边界内侧三点）。

### 3.2 命中域判定（滚轮落点护盾）

- 滚轮只在**可编辑区**（标尺内侧：`BandRects::in_editable`）生效 —— 面板/dock/标尺/工具带/菜单条全在区外，滚轮不缩放（照 hit 护盾口径的补集）。
- 与列表滚轮滚动互不打架：UiVm 只把滚轮路由给**悬停的滚动控件**（`scroll_hit`），视口滚轮天然无人认领；宿主读 `snap.wheel` 与 UiVm 读同一帧快照（快照非消费式），两路读面无冲突。

### 3.3 中键拖拽平移

- `MouseButton::Middle`（S12-3 button_down 表先例：`button_down("middle")` + `buttons_pressed/released[2]` 边沿）。按下沿记（鼠标屏位，起始 center）锚点，拖拽 delta（屏像素）经 `pan_screen` 反向加到 center（`center -= delta / zoom`，拽着世界走）—— 绝对式锚定比逐帧增量抗丢帧。
- 运行态：滚轮缩放/中键平移/工具栏 ± 全部让路（编辑视图在运行态冻结 —— 投影护盾不写 cam）；PLAY 启动时半途的 pan 锚点与拖拽/框选一并作废。

## 3.5 运行态视图与 UI 锚定（实测"点 PLAY 后 UI 乱飞"根因修复）

### 3.5.1 根因

用户实测（截图确诊）：点 PLAY 后菜单项/标尺数字/面板标题/图标列在视口里乱飞。链条：

1. S20 起，菜单栏 Label/标尺数字/面板标题/dock 标题/时间轴标签/图标精灵全是**世界空间树节点**，每帧经 `place_at_screen`（用相机把屏幕槽位换算成世界位）摆放；
2. PLAY 三时机还原把**渲染相机**换成场景相机（位置/zoom ≠ 编辑视图时两者不等）；
3. 而运行态 `!playing` 护盾把 **UI 投影所依赖的编辑视图驱动**一并停了 —— UI 节点停在按编辑视图算出的旧世界位，渲染却用场景相机 ⇒ 世界位与相机的配对错位，全部 UI 飞散。

### 3.5.2 活动视图文法（`CamRig::active_view`）

单点函数，返回本帧 UI 投影换算基准 `(center, zoom)`：

| 态 | 值 | 依据 |
|---|---|---|
| 编辑态 | `EditorCam` 会话态（center, zoom） | 与交互换算同源 —— 编辑期行为零变化 |
| 运行态 | **实时读场景 cam 节点**：center = `tree.world(cam)` 平移分量、zoom = `zoom` 属性（缺省 1；非正/非有限回 1 —— 防御路径，schema 面上 set_prop 已钳 [0.05,16]） | 与提取层 `camera_state_of` **同源同序**（渲染侧相机权威就是这两个读面）—— UI 换算与渲染矩阵恒用同一台相机 |

语义效果：

- **UI 摆位投影运行态照跑**（`!playing` 护盾拆分，见 3.5.3）：place_at_screen 全家（菜单栏/标尺数字/面板标题/dock/时间轴/按钮标签/图标列/进度条/状态栏/检查器）每帧按**活动视图**换算 ⇒ PLAY 后 UI 钉屏幕不动（Godot 语义：编辑器 UI 不随游戏相机飞），游戏世界按场景相机渲染；
- **游戏脚本动相机，UI 跟随语义正确**：运行态 active_view 就是脚本正在驱动的那台相机（当帧 `refresh_transforms` 后读 world 缓存）——脚本把相机拉远，钉屏 UI 仍钉屏；
- **读数一帧滞后**：投影先于当帧脚本执行，脚本当帧的相机写入下一帧才反映在 UI 摆位上 —— 与既有 UI 命中一帧滞后同口径；
- **不做编辑器域 clamp**：运行态 zoom 原样跟随（schema 合法域 0.05..16 可超出编辑器 0.1..8），夹了反而与渲染矩阵错位。

### 3.5.3 `!playing` 护盾拆分清单

| 护盾 | 处置 | 面 |
|---|---|---|
| cam 节点写入（`apply_editor`） | **保留** | 游戏运行态用场景定义的相机，编辑视图停写 |
| 编辑交互（Tab 循环/点选/框选/gizmo 拖拽/方向键/Delete/undo-redo/F6..F9/Enter/U/E/改名提交/行点击落账/时间轴落账/fs 落账/音乐键） | **保留**（零变化） | 全部编辑动作让路 |
| 滚轮缩放/中键平移/工具栏 ± | **保留**（零变化） | 运行态相机归游戏 |
| **UI 摆位投影**（place_at_screen 全家 + 网格/标尺/轨迹/选中框换算） | **拆除**（改按活动视图继续） | 乱飞根因所在 —— 投影本就每帧无状态重写，继续跑只是换算基准换成活动视图 |

### 3.5.4 gizmo 退场语义（Godot：运行时编辑器辅助件退场）

运行态 `visible=false`（STOP/RESET 后下一帧投影自然复燃，投影无状态口径天然收口）：

| 容器/池 | 实现 | 备注 |
|---|---|---|
| 网格条带池（grid_bars ×340） | 布局条件加 `&& !playing`（GRID 开关同门）—— 全池走既有熄灭分支 | GRID 关闭路径的复用，无新机制 |
| 标尺刻度池（ruler_ticks） | 顶横/左竖点亮循环加 `&& !playing` —— used=0 ⇒ 全池走余量熄灭分支 | 16px 标尺带（ruler_h/v/corner 屏幕镀边）**不藏**（纯屏幕 chrome，不飞不挡游戏） |
| 标尺数字池（ruler_labels） | 两段点亮循环加 `&& !playing` —— lab_used=0 ⇒ 全池走余量置空分支（空文本不上屏） | 数字与刻度统一退场 |
| 轨迹点池（traj_dots ×12） | `traj_from_to` 运行态强制 None ⇒ 全池走熄灭分支 | 编辑器会话可视化不叠游戏画面 |
| 选中框（sel_box） | 投影尾段 `visible = !playing`（照 icons 池显隐先例） | 精灵 z=5 选中高亮是引擎机制，不在此列 |

### 3.5.5 恢复路径

STOP/RESET 重应用编辑视图（三时机之三，`apply_editor`）⇒ 下一帧投影按编辑视图（= 活动视图）换算 ⇒ UI/gizmo 自然回位，无残留。冒烟闩锁：PLAY 窗内网格整池熄灭 + 选中框隐藏；RESET 后网格首条带复燃。

### 3.5.6 契约测试（editor headless 可测面）

- `active_view_follows_scene_camera_when_playing`：编辑态 active_view == EditorCam；PLAY（restore_scene）后 == 场景 cam（stash 还原值）；脚本动相机当帧跟随；schema 钳制值（0.05/16）原样跟随、裸通道 NaN 归 1；STOP 重应用后回编辑视图。
- `ui_label_lands_on_active_view_slot_when_playing`：编辑视图 zoom=2 平移后 PLAY，菜单 Label 的 world 位 == 屏幕槽位经活动视图（场景 cam 恒等映射）的换算值（乱飞修复的量化证据，含"编辑视图换算值可判然不同"的反证 + world→screen 渲染闭合核对）。
- NES_EDIT_DEMO 冒烟闩锁：gizmo 退场三态（3.5.5）。

## 4. 标尺/网格自适应

### 4.1 标尺（S12-3 标尺的 S20 升级）

- 步长从固定 64 改**融合序列 {1,2,5}×10^k ∪ 2^n 自适应**：取 `step × zoom ≥ 60px` 的最小步长（`RULER_STEPS` 常量表，编辑器 zoom 域内命中步长 ∈ [8,1000]）。
- **屏上刻度间距落 [60,96)px**（融合序列相邻比 ≤2）⊂ 任务口径 [60,150)。**zoom=1 命中 64** ⇒ 现状 "128px 数字距 / 64px 刻度" 观感逐位保持（序列含 2 幂正是为钉住这个基线；纯 {1,2,5}×10^k 十进位无法产出 64 —— 融合是两条款同时成立的解）。
- 数字标签 = **世界坐标值**（含负数 —— 平移后原点可离屏），每 2 格一个（主刻度位）；屏位 = world_to_screen + 2px 内衬，Label 经 `place_at_screen` 反向放置（屏上恒定 14px 字号）。
- **池容量复核**：刻度屏上间距 ≥60 ⇒ 需求 = `ceil(可视宽/60)+2`（顶横）与 `ceil(可视高/60)+2`（左竖）—— 2560×1440 设计目标 ⇒ 45/26，池 48/32 照旧够用（4K 超限少画几根，既有口径）；数字 ≥120px 间距 ⇒ `ceil(可视宽/120)+2` ≈ 23，池 24/16 照旧。

### 4.2 网格（S12-S19 32px 世界间距的 S20 升级）

- 世界间距 = `grid_spacing_world(zoom)`：32px 基准档，屏上密度超限（间距 × zoom < 12px）时 ×2 递进（带 30 次防污染值死循环护栏）。保持 32 的 2 幂倍数 ⇒ 方形网格、世界原点对齐不变；zoom=1 时 32px 逐位保持。
- **池容量复核**：自适应后屏上间距 ≥12 ⇒ 需求 = `ceil(可视宽/12)+2 + ceil(可视高/12)+2` —— 2560×1440 ⇒ 竖 216 + 横 122 = 338，`GRID_POOL` 90 → **340**（4K 超限少画，同标尺池口径）。

## 5. 工具栏缩放 UI

- 工具带右端 `[-] 100% [+]`（Godot 观感）：−/+ 两枚 Button（九宫格底板池扩到 8 槽，第 7/8 槽随缩放键）+ 百分比 Label（取整 %，实时显示 —— 会话态投影）。± = 以视口中心缩放一档（`zoom_step` = `zoom_toward` 在 s=vc 的退化式，center 不动），1.15 步进 clamp 0.1..8.0。
- SEL/SNAP/GRID 与缩放组并存：左三键 156px + 右缩放组 ~172px < 最小窗带宽 374px；缩放键进 hit 护盾（压上不清选中不框选）；on_activate 落账与工具栏同一条共享缓冲通道；运行态静默忽略（SEL/SNAP/GRID 同口径）。
- 状态栏快捷键表（Shortcut Table）加 `Wheel=zoom to cursor  MidDrag=pan  Ctrl+S=save` 行（9 → 10 行，EDITOR_LOG_KEEP=48 余量复核通过）。

## 6. 门禁

| 门禁 | 结果 |
|---|---|
| 十 crate `cargo test --release` | **792/792 全绿**（nes-asset 34 + nes-audio 52 + nes-extension-api 7 + nes-extension-js 29 + nes-media 27 + nes-render-api 48 + nes-render-extract 65 + nes-render-wgpu 143 + nes-scene 271 + nes-runtime 116；基线 792 + 0 —— 本轮断言走示例测试与冒烟钩子，零 crate 测试面改动） |
| editor_shell 单元测试（`cargo test --example editor_shell`） | **15/15**（S20 新增 8 + 追平轮 +2：`active_view_follows_scene_camera_when_playing` 活动视图单点（编辑态==EditorCam / PLAY 后==场景 cam / 脚本动相机当帧跟随 / schema 钳制值跟随 + 裸通道 NaN 归 1 / STOP 回位）、`ui_label_lands_on_active_view_slot_when_playing` UI 摆位量化证据（菜单 Label world 位 == 屏幕槽位经活动视图换算 + 旧路径反证 + 渲染闭合）；既有 scan_signal 5 项不动） |
| clippy `--all-targets` ×10 | **0 警告** |
| 依赖守卫 | **15/15**（check_dependency_direction.py） |
| editor_shell 冒烟 | 120 帧干净退出；NES_EDIT_DEMO=1 NES_GAME_FRAMES=420 全断言（含 §3.5 三闩锁：PLAY 窗网格整池熄灭 / 选中框隐藏 / RESET 后首条带复燃） |
| 回归冒烟 | first_game / tween_demo / frame_demo 各 180 帧干净退出 |

- **冒烟修正（S12-8 演示流的存量脆弱面，S20 顺手根治）**：fs 双击导航原按 "Media 在场时 spin.nes = 行 7 + 滚 4 格" 写死 —— 资产树在 S16..S19 间长大（Textures/ 等子目录条目增多），行号漂移导致写死坐标点击错行。改**目标现算**：spin.nes 实际行号（当帧 fs_entries）+ 单次多格滚轮（采集器同帧相加，无逐格丢格面）+ 行中点击 y。断言面（fs open / mount 行）不变。
- 既有断言全部保持：fs 双击/改名框/菜单点击在 UI 层（HUD 口径不吃相机）；视口内固定坐标断言（S19.5 轨迹端点 (280,130)→(2,4)、S19.6 图标列 (10,102)）在 zoom=1 现状下经恒等换算逐位复核通过。

## 7. 遗留

1. **触控板手势**：捏合缩放（WM_POINTER / WM_DPINCH）未接 —— 滚轮口径只吃 `Wheel.y`；触控板双指平移（精确触控板的高分辨率滚轮）会走同一滚轮通道被当缩放，体验与 Godot 的触控板档位有差距。
2. **空格拖拽平移**：Godot/主流编辑器的空格+左键抓手段未做（当前只有中键）；空格键位与改名框焦点门的优先级需要先裁决。
3. **缩放动画平滑**：滚轮缩放是即时跳档（×1.15 硬切），无 Godot 那样的指数趋近平滑；需要帧差驱动的插值会话态。
4. **帧选 F 键（frame selection）**：框选当前要求拖拽手势；"F 键把可视区内精灵全部入选"的 Godot 快捷键未做（F 键进 `Key::Other(vk)` 通道，接线成本低，归交互里程碑）。
5. **zoom 化的像素对齐**：非整数 zoom 下 1px 网格线/标尺刻度落在半像素上（无 MSAA 光栅下的抖动）；Godot 以抗锯齿线宽解决，P0 接受现状。
6. **运行态 UI 摆位一帧滞后**（§3.5.2 既有裁决，非缺陷）：投影先于当帧脚本执行，脚本当帧的相机写入下一帧才反映在 UI 摆位上 —— 与既有 UI 命中一帧滞后同口径；要做到零滞后需把 UI 投影挪到 simulate 之后（帧序重构，收益不抵风险）。
7. **运行态标尺带镀边不退场**：gizmo 退场清单（§3.5.4）只藏刻度/数字，16px 标尺带底条（ruler_h/v/corner 纯屏幕 chrome）运行态保留 —— Godot 运行态整个 2D 编辑视口被游戏画面替换，壳层是面板常驻形态，带底条保留属刻意边界；若要"全屏游戏感"归后续编辑器布局里程碑。

## 8. S20.2 舞台边界三件套（wt-stage 追平轮）

用户观察："物体可拖出场景无限制/无提示"。回应 = 三件常显的编辑器会话件 + 一个默认开的拖拽钳制 —— **全部是编辑器件，零 schema prop、零 crate 源码改动**（进不了游戏场景、进不了指纹）。

### 8.1 舞台矩形（编辑器常量）

- `STAGE = (0, 0, 768, 432)`（世界坐标，`(x0, y0, w, h)`）：与引擎基准设计分辨率**同源** —— 装配开窗 `OPEN_CLIENT` 768×432 同值，默认视图（恒等映射）下舞台恰铺满窗口。未来按场景/项目设置配置舞台尺寸（"场景化配置"）归后续里程碑；P0 单一常量单一出口。
- `STAGE_CELL = 16`：精灵基准格（与引擎命中盒/`SPRITE_PX=16` 同源）—— 钳制公式"减一格"的一格。

### 8.2 ① 边界可视化（常显，编辑态）

- 舞台矩形 1px **accent 描框**：4 条细 Control（顶/底/左/右池 `STAGE_BORDER_POOL=4`），照网格条带池的屏上几何换算 —— 世界角 → `world_to_screen` → 屏上条带，几何单点出口 = 纯函数 `stage_border_rects`（T-SB-02 单元测试面）。
- 每条边线**在可视域内才点亮**、沿长度被可视域裁剪（描框永不画到面板/菜单带上 —— 1px accent 只出现在可编辑区矩形内）；1px 落在舞台内侧（底/右取 `sy1-1`/`sx1-1`，描框标记舞台本体）。
- 编辑态显示、运行态隐藏（照 S20.1 §3.5 gizmo 退场门 —— 投影 `!playing` 单门）。

### 8.3 ② 场景外变暗（编辑态）

- 4 条**深色实心条带**覆盖"可视域 − 舞台矩形"（上/下/左/右池 `STAGE_DIM_POOL=4`）：几何 = 纯函数 `stage_dim_rects` —— 舞台角屏幕位先钳进可视域再按四向补集出条带（左/右条带纵向只补到舞台纵向跨度，四向几何不重叠、面积和恒等于可视域面积 − 舞台面积）。**舞台离屏时条带覆盖整个可视域**（钳制把舞台角压到可视域边缘，某一向补集 = 整域）；舞台全含可视域时四条全隐（默认视图的干净常态 —— 无缝 = 无提示噪音）。
- **色**：主题 bg 加深档。查证：`PALETTE` 八槽位（bg/panel/border/text/text_dim/selected/accent/danger）**无 darker 槽**，且 nes-scene `THEME_SLOTS` 是封闭八槽位契约不扩槽 —— 故取 **bg(20,22,26) × 0.6 的预计算常量 `STAGE_DIM_RGB = (12,13,16)`**，代码生成 16×16 纯色纹理 `stage_dim.bmp`（缺了再写，icons/panel_skin 同一家法），走 S18 九宫格皮肤通道上屏（`ns_tex` + 1px 边距过 `nine_slice_of` "至少一条 > 0" 启用判据 + `ns_modulate=false` 成品绝对色 —— 纯色纹理下 1px 边距与实心填充逐像素同观感）。

**z 序表**（查证现有 z 常量后的裁决；变暗与描框同带 z=8）：

| z | 内容 | 与舞台注记的关系 |
|---|---|---|
| −100 | 网格条带池 | 垫底，无交叠争议 |
| −90 | 标尺条带/刻度/数字 | 垫底 |
| −80 | Output dock / 时间轴 dock 铺底（−79 进度条） | 垫底 |
| −70 | 工具带 + 菜单栏（含播放组） | 垫底 |
| −60 | FileSystem dock | 垫底 |
| 0 | 精灵（缺省） | **变暗之下**（场景外精灵被盖） |
| 5 | 选中精灵高亮（引擎机制） | 之下 |
| 6 | 补间轨迹点 | 之下 |
| 7 | Scene 树图标列 | 之下 |
| **8** | **舞台描框 + 变暗条带**（新；同 z 下树序描框池建在变暗池之后 —— 共享边缘行描框胜出，界线保持 accent） | —— |
| 90 | 菜单下拉弹层（瞬时 UI 例外） | **盖回**舞台注记 |
| 100 | 选中框 sel_box | **盖回** |

- 面板带（负 z）在 z 序上低于 8，但条带几何被可视域裁剪（两纯函数只在可编辑区矩形内出几何），与面板带**零几何重叠** —— "被面板盖回"由裁剪保证，"被菜单/选中框盖回"由 z 序保证。
- **不进 hit 护盾**（照 grid/traj 先例 —— 注记不拦编辑点击：舞台外的精灵照常可选可拖，变暗只是视觉不是模态）；`"stage"` 容器进 walk skips（不进层级树行列表，冒烟加 `!scene_rows.contains("stage")` 断言）。

### 8.4 ③ 拖拽钳制（默认开，可关）

- `clamp_enabled: bool` 会话态，默认 `CLAMP_DEFAULT = true`（用户直觉：拖出场景=不对）。**只影响 gizmo 拖拽落笔，不进树、不进指纹**（拖拽本身就是会话编辑 —— 指纹语义不变）。
- **钳制公式**（`clamp_to_stage` 纯函数，生效点 = gizmo 拖拽落笔处 —— 帧循环 preview 直写的**唯一写位点**；松开提交从树回读同一值，天然一致）：

```text
pos = clamp(pos,
            (STAGE.x0, STAGE.y0),
            (STAGE.x0 + STAGE.w - 16, STAGE.y0 + STAGE.h - 16))
```

  即**节点 origin（16px 精灵基准格左上角）钳在舞台内减一格**：完整精灵格恒在舞台矩形内。逐轴独立：拖到 (5000, −3000) 落笔 (752, 0)（x 钳右缘、y 钳上缘）。pivot ≠ (0,0) 时的视觉主体偏移 P0 不做原点补偿（公式按 origin 冻结，记此）。
- **只钳 gizmo 拖拽**：方向键移动 / Inspector 数值输入**不钳**（精确输入是故意的 —— 文档裁决：键盘/数值路径是"我就是要放到这"的表达，钳制是"随手拖"的安全网）。
- **CLAMP 关 = 自由拖拽**（Godot 对齐：舞台边界只是参考线不是墙 —— Godot 2D 编辑器默认允许把节点放到任意远，舞台矩形是项目设置里的可选参考）。开关双入口：
  - 工具栏 `CLAMP` 按钮（左四键第 4 键，照 GRID* 模式：文本后缀 `*` = ON，`toggle_text` 单点出口）；
  - Project 菜单镜像项 `Stage Clamp: On/Off`（菜单化既有行为 —— 与按钮同一会话态翻转、同一落行 `tool clamp on/off`）。
- 布局连锁：`TOOLBAR_BTN_STEP` 52 → **50**（最小窗 768 的工具带宽 374px 下编辑四键 202px + 缩放组 172px = 374 恰好无重叠）；底板池 8 → **9 槽**（0..4 编辑 / 4..7 播放组 / 7..9 缩放）。状态栏 `tools:S/N/G` 加第 4 位 `C`。
- CLAMP 关闭后的自由度与既有语义：undo 事务照常回滚自由落点；RESET/保存不受影响（钳制只发生在落笔瞬间，不改事务/保存面）。

### 8.5 契约测试（T-SB 系列，editor headless 可测面，字面量全 ASCII）

| 项 | 测试 | 断言面 |
|---|---|---|
| T-SB-01 | `tsb01_clamp_on_clamps_drop_off_passthrough` + `tsb01_gizmo_write_path_lands_clamped_pos_in_tree` | CLAMP 开：拖到 (5000,−3000) 落笔 == (752,0)、右下远点 == (752,416)；CLAMP 关：原值直通；树写面同款表达式 headless 断言 |
| T-SB-02 | `tsb02_border_rects_track_stage_edges` + `tsb02_dim_rects_cover_viewport_minus_stage` | 描框：边线域内才可见/沿长裁剪/离屏隐藏；变暗：全含全隐（干净默认）/离屏覆盖整域/半交精确补集 + 面积守恒；编辑态可见/运行态隐藏的**树读面**归冒烟闩锁（投影块在 main 帧循环内，headless 不可达 —— 与 S20 活动视图断言分工同口径） |
| T-SB-03 | `tsb03_clamp_defaults_on_and_dual_entry_semantics` | 默认值 true；按钮 `CLAMP*`/`CLAMP`（toggle_text 单点）；Project 菜单镜像项 `Stage Clamp: On/Off`（既有项零变化）；快捷键表舞台行 |

### 8.6 冒烟（demo 断言变更清单）

注入流 410 帧后追加 S20.2 段（demo 总帧 420 → **520**）：

1. **416**：闩锁编辑相机（center/zoom）—— 尾段相机自由漂移，S20 既有断言改读闩锁值（语义不变：416 == S20 段收尾值）。
2. **426..443**：12 格滚轮缩小（1 帧 1 格）+ 平移 (−30,−24) 屏像素 —— 几何推导把"舞台底/左边线 + obj2 + 钳制落点 (752,416)"同时摆进 102px 高的可编辑带（zoom ≈ 0.247）。
3. **445..452**：CLAMP 开（默认）拖拽链 —— 点选 obj2 → 点住 → 拖到窗外右下远点 (1500,3000)（注入坐标可出窗 —— 纯队列通道无裁剪）→ 落笔钳 (752,416)；闩锁 `move 752,416` 落行 + obj2 位 == 钳制端点（精确）。
4. **454..461**：点工具栏 CLAMP（中心 (366,72)）→ 按钮文本 `CLAMP*` → `CLAMP` 闩锁 + `tool clamp off` 落行 → 再拖 → 自由值远超舞台闩锁（`move 4927,11796`）。
5. **463..474**：Ctrl+Z×2（两次拖拽事务各退一步，obj2 回 (380,130)）+ Tab×2（主选中回 obj1 —— 终局选择断言面复位）。
6. **480..495**：第二 PLAY 窗（spin 已卸载 = 0 脚本空转）—— 舞台条带运行态整池熄灭闩锁（同 gizmo 退场门）→ STOP。
7. 既有面同步：`EDITOR_LOG_KEEP` 48 → **72**（舞台段 ~13 行新日志，最早断言行 "cand spin.nes" 余量保住）；绑定资产计数 9/10 纹理（+stage_dim.bmp）、上传 8 → 9；walk skips 断言加 `stage`；Shortcut Table 10 → 11 行（舞台行）；`sb_click`/`sb_click2` 点击坐标按当帧相机**现算**（fs_nav 先例 —— 写死坐标跨相机状态不稳）。

### 8.7 门禁（wt-stage 追平轮）

| 门禁 | 结果 |
|---|---|
| 十 crate `cargo test --release` | **792/792 全绿**（nes-asset 34 + nes-scene 271 + nes-render-api 48 + nes-render-extract 65 + nes-audio 52 + nes-media 27 + nes-extension-api 7 + nes-extension-js 29 + nes-render-wgpu 143 + nes-runtime 116 —— 与 §6 基线逐位同值，零 crate 测试面改动） |
| editor_shell 单元测试 | **20/20**（基线 15 + S20.2 新增 5：T-SB-01×2 / T-SB-02×2 / T-SB-03×1） |
| clippy `--all-targets` ×10 | **0 警告** |
| 依赖守卫 | **15/15**（check_dependency_direction.py） |
| editor_shell 冒烟 | 120 帧干净退出；`NES_EDIT_DEMO=1 NES_GAME_FRAMES=520` 全断言（含 §8.6 六段：钳制/自由落点、按钮翻面、条带编辑态点亮/运行态退场；基线哈希不动 —— CLAMP/描框/变暗全是编辑器会话件，无新 schema prop，进不了游戏场景） |
| 回归冒烟 | first_game 180 帧 / tween_demo 180 帧干净退出 |

### 8.8 遗留

1. **舞台尺寸场景化配置**：P0 编辑器常量 (0,0,768,432)；按项目设置/场景属性读取舞台矩形（含多舞台/多矩形）归编辑器项目系统里程碑。
2. **拖拽中的实时越界提示**：钳制点写"拖拽落笔"，拖拽过程中无"到边"视觉反馈（Godot 有红色边框提示）；可在钳制生效帧闪描框 accent → danger 色，归交互打磨。
3. **变暗纹理的换肤联动**：`STAGE_DIM_RGB` 是 bg×0.6 预计算常量，换肤（改 PALETTE bg 槽）不会联动变暗色 —— 联动需要提取层每帧乘法 tint（`ns_modulate=true` 路线），P0 接受静态值。
4. **CLAMP 对框选拖拽的语义**：P0 只钳 gizmo 单选拖拽；多选拖拽（P0 无此交互）与方向键（精确输入豁免）不涉及。
