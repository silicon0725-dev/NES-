# NES 2.0 S20 视口平移缩放 v1

- 分支：`s20-panzoom`（wt-panzoom 独立 worktree）；基线 HEAD `9ebf084`（S19.6 Scene 树真图标集）
- 改动面：`nes-runtime/examples/editor_shell.rs` 单文件（壳层相机会话态 + 换算 + 投影 + 冒烟）+ `.gitignore`（Ctrl+S 演示保存目标不入库）；**零 crate 源码改动、零新依赖、依赖分层不变**
- 用户基准：Godot 2D 工作区 —— 滚轮缩放朝光标、中键拖拽平移、标尺随缩放自适应、缩放百分比显示

## 0. 结论

1. **编辑器视口相机落地**：`EditorCam { center: (f32,f32), zoom: f32 }` 会话态（不进树逻辑、不进指纹 —— 编辑器视图不是游戏状态），zoom clamp 0.1..8.0。**关键既有假设被打破**：视口旧注释声明"视图空间 == 世界空间（相机每帧置中恒等映射）"，鼠标→世界、点击选择、框选、gizmo 拖拽、命中全部建立在此假设上；本轮起全部换算收敛到单点屏幕↔世界助手（`EditorCam::screen_to_world` / `world_to_screen`）。
2. **相机换算式（查证后的契约结论，修正了任务书里 "scale = 1/zoom" 的直觉猜想）**：引擎相机缩放权威在 `Camera2D` 节点的 **`zoom` 属性**（schema："缩放倍数。越大画面越近"，合法域 0.05..16；相机节点**自身变换缩放不参与视图矩阵** —— `nes-render-api` `Camera2DState` 契约冻结 + S19.1 像素契约测试 `t_camera_*` 同源）。视图矩阵冻结式 `view = T(viewport/2) ∘ S(zoom) ∘ T(-center) ∘ R(-rotation)`，即 **`screen = viewport_center + zoom × (world − center)`**。故每帧（extract 前）驱动式为 **`cam.pos = center` + `cam.zoom 属性 = EditorCam.zoom`**；zoom=1 + 初始 center=(开窗/2) 时与旧"置中恒等映射"逐位同值（既有固定坐标断言全部保持）。
3. **场景数据保护三时机**（编辑视图不进场景文件、不进游戏运行态）：装载时 stash cam 原始 local transform + zoom 属性；**Save（Ctrl+S / Scene>Save）前还原 stash、保存后重应用编辑视图**；**PLAY 时还原场景相机**（首 PLAY 的全树快照在还原之后捕获 ⇒ RESET 回到的也是场景相机），STOP/RESET 后重应用编辑视图。
4. **渲染面事实（投影改造依据）**：Control 类（有 SetRect 状态）走 HUD 口径（渲染侧经视图矩阵的**逆**折回世界 —— 钉屏幕像素，相机不动它）；**纯 Label 与 Sprite2D 走世界变换**（吃视图矩阵）。故网格条带/标尺刻度/选中框/轨迹点（Control）由投影换算出屏幕位；全部 UI Label 与 S19.6 图标精灵经 `place_at_screen` 反向放置（世界位 = screen_to_world(屏幕位) + 本地缩放 1/zoom —— 视图 zoom 与本地 1/zoom 相抵，屏上恒定 1:1 尺寸与位置）。
5. **门禁**：十 crate `cargo test --release` **792/792**（基线 792 + 新增 0 —— 断言进 NES_EDIT_DEMO 钩子与 editor_shell 示例测试面）+ editor_shell 单元测试 **13/13**（S20 新增 8 项，`--examples` 面）+ clippy `--all-targets` **0 警告 ×10** + 依赖守卫 **15/15** + editor_shell 双冒烟（120 帧干净退出 + NES_EDIT_DEMO 420 帧全断言，连跑 5 次稳定）+ first_game/tween_demo/frame_demo 180 帧冒烟回归全过。

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
| editor_shell 单元测试（`cargo test --example editor_shell`） | **13/13**（S20 新增 8：三组 zoom 往返 + zoom=1 恒等映射 + 缩放朝光标世界点不变 + clamp/中心档位/平移 + 标尺自适应与 zoom=1 钉住 + 网格倍增 + headless 滚轮→cam 节点驱动链 + 带几何冻结式；既有 scan_signal 5 项不动） |
| clippy `--all-targets` ×10 | **0 警告** |
| 依赖守卫 | **15/15**（check_dependency_direction.py） |
| editor_shell 冒烟 | 120 帧干净退出；NES_EDIT_DEMO=1 420 帧全断言（**连跑 5 次稳定**，含 Scenes/ 目录在场的复跑形态） |
| 回归冒烟 | first_game / tween_demo / frame_demo 各 180 帧干净退出 |

- **冒烟修正（S12-8 演示流的存量脆弱面，S20 顺手根治）**：fs 双击导航原按 "Media 在场时 spin.nes = 行 7 + 滚 4 格" 写死 —— 资产树在 S16..S19 间长大（Textures/ 等子目录条目增多），行号漂移导致写死坐标点击错行。改**目标现算**：spin.nes 实际行号（当帧 fs_entries）+ 单次多格滚轮（采集器同帧相加，无逐格丢格面）+ 行中点击 y。断言面（fs open / mount 行）不变。
- 既有断言全部保持：fs 双击/改名框/菜单点击在 UI 层（HUD 口径不吃相机）；视口内固定坐标断言（S19.5 轨迹端点 (280,130)→(2,4)、S19.6 图标列 (10,102)）在 zoom=1 现状下经恒等换算逐位复核通过。

## 7. 遗留

1. **触控板手势**：捏合缩放（WM_POINTER / WM_DPINCH）未接 —— 滚轮口径只吃 `Wheel.y`；触控板双指平移（精确触控板的高分辨率滚轮）会走同一滚轮通道被当缩放，体验与 Godot 的触控板档位有差距。
2. **空格拖拽平移**：Godot/主流编辑器的空格+左键抓手段未做（当前只有中键）；空格键位与改名框焦点门的优先级需要先裁决。
3. **缩放动画平滑**：滚轮缩放是即时跳档（×1.15 硬切），无 Godot 那样的指数趋近平滑；需要帧差驱动的插值会话态。
4. **帧选 F 键（frame selection）**：框选当前要求拖拽手势；"F 键把可视区内精灵全部入选"的 Godot 快捷键未做（F 键进 `Key::Other(vk)` 通道，接线成本低，归交互里程碑）。
5. **zoom 化的像素对齐**：非整数 zoom 下 1px 网格线/标尺刻度落在半像素上（无 MSAA 光栅下的抖动）；Godot 以抗锯齿线宽解决，P0 接受现状。
