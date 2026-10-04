# NES 2.0 — S18 编辑器换肤 v1

状态：**已完成**（worktree `wt-skin`，分支 `s18-editor-skin`，基线 `6909d18`）。
范围：`nes-runtime/examples/editor_shell.rs` 换肤实战 + 两张皮肤纹理入库 +
设计提取笔记（`DESIGN-NOTES.md`，第一提交）。**crate 源码零改动、零新依赖、
行为语义零变化**（布局结构/交互路径/编辑功能全保持，本轮只换皮）。

## 0. 结论

- **色板锚定 nes-scene 八槽位契约，不扩展、不外置**（`DESIGN-NOTES.md` §6.1
  的裁决）：壳层新增 `editor_theme` 模块（文档通称 **EditorTheme**）作为
  色板/间距/行高/面板宽的统一出口，装配时写入一个 `Theme` 场景节点 ——
  "主题即场景节点"（nes-scene/ui.rs 既有机制），换肤入口从"改 nes-scene
  源码"收敛为"改壳层一张常量表"。
- **面板换皮走成品绝对色九宫格纹理**（`ns_modulate=false` 路线）：深色底 +
  1px 亮边框 + 边带微渐变 + 确定性噪点，代码生成 BMP 入库（walk_sheet
  先例）。乘法 tint（S16.7 modulate）出不了"亮边框比面板底亮"，故不走
  灰阶×槽色路线；主题槽继续管文本/选中/强调。
- **控件三态观感落地为"九宫格底板 + 透明底按钮"组合**：正常 = 纹理 +
  border 槽框；悬停 = 纹理 + accent 框；按下 = accent 填充 + accent 框
  （提取层既有四态透传，壳层零判定逻辑 —— egui `Widgets::style` 的状态
  单点判定在 NES 提取层已同构存在）。
- **验证**：编辑器双冒烟全绿（120 帧干净窗 + NES_EDIT_DEMO 420 帧全断言）、
  first_game 冒烟绿、十 crate `cargo test --release` 全绿（746 通过）、
  clippy 0 ×10、依赖方向守卫 15/15；另做了**实窗截图取证**（见 §3.4）。

## 1. 参考库研读摘要

详细出处与引文路径见 `DESIGN-NOTES.md`（同仓库根）；每库一段 + 可借鉴模式：

| 库 | 研习点 | 一段话结论 | NES 可借鉴模式（本轮是否落地） |
| --- | --- | --- | --- |
| egui | Widget 状态机 + style.rs | `Sense` 位flags 声明感知面；`Response` 聚合 hovered/clicked/dragged；`Visuals.widgets` 按状态五格表 + `Widgets::style()` 单点判定（active>hovered>inactive） | 状态→外观映射表化（✅ §2 映射表注释进 EditorTheme 文档；判定本身提取层已有）；panel_fill 一等槽位（✅ 已有） |
| iced | Theme + Layout | `Seed` 六色推导全套 Palette（Background 8 级/Swatch/Pair 可读对）；`Theme` 枚举命名预设；`Catalog::style(class, status)` 类×状态矩阵 | 种子推导（📝 记笔记备用，P0 手填）；布局纯树遍历每帧重算（✅ 与 NES 每帧投影同家法的互证） |
| xilem/masonry | retained UI 组织 | pass 系统（一文件一 pass）；`WidgetState` 失效旗标命名法（request_/needs_/is_/has_）+ merge_up 冒泡 + "zombie flags" 反模式警示 | 反面教材的正确用法：换肤零新增 per-frame 失效/缓存（✅ 皮肤参数全常量/纹理）；默认主题住基础层（✅ 八槽位留 nes-scene） |
| oxiui | crate 分层 | core/theme/render-wgpu/accessibility 切分与 NES 分层政策同构；theme crate 文件面 = tokens/typography/stylesheet(CSS 式)/ThemeManager/PartialTheme 叠加/gallery 预设 | PartialTheme 叠加思想（📝 契约不动、delta 表达 —— P0 色板显式列全即兼容）；token 三件套组织（✅ EditorTheme 分色板/间距/行高/字号块） |
| zed(gpui) | Dock/Inspector 面板 | `Panel` trait 自描述容量：persistent_name / DockPosition 校验 / default_size / min_size / flexible / zoomed | 每面板尺寸三元组（📝 本轮面板宽收敛进 EditorTheme，为 dock 拖拽留形状不引入行为）；persistent_name 由 uid/registry_key 承担（无需引入） |

取证方式备注：egui/xilem/oxiui 仓库工作树未检出，全部经 `git show HEAD:`
只读提取（egui HEAD `23348e9f`）；iced 直读工作树；zed 克隆为空、按任务书
预案 curl 单文件取 `gpui/src/style.rs` 与 `workspace/src/dock.rs`；oxiui 为
部分克隆（blob 不在线），结论基于目录树 + `lib.rs` re-export 面（笔记内
已标注证据强度）。

## 2. EditorTheme 结构

`nes-runtime/examples/editor_shell.rs` 顶部新增 `mod editor_theme`（文档通称
EditorTheme），**一处定义 + `use` 别名保持既有引用面逐字不变**（纯出口收敛，
diff 里无行为改动）：

```text
editor_theme
├── 色板（八槽位，I64 0xRRGGBBAA，序 = nes-scene THEME_SLOTS）
│   SLOT_BG/panel/border/text/text_dim/selected/accent/dANGER
│   PALETTE = [(&str, i64); 8]   ← 写 Theme 节点属性
├── 槽位名引用（fill_slot/border_slot/color_slot 属性值）
│   SLOT_PANEL_NAME / SLOT_BORDER_NAME / SLOT_ACCENT_NAME / SLOT_TEXT_DIM_NAME
├── 间距栅格
│   MARGIN 8 / SPACE_S 4
├── 面板宽（恒定宽口径不变）
│   LEFT_PANEL_W 180 / INSPECTOR_W 190 / TOP_BAND 40 / STATUS_BAND 24
│   DOCK_H 96 / TOOLBAR_H 24 / TOOLBAR_BTN 48×20 步 52
├── 行高三档
│   INS_ROW_H 20（真字体行）/ DOCK_ROW_H、FS_ROW_H 18（列表行）
│   DOCK_TITLE_H 18 / FS_TITLE_H 16 / FS_SEP_H 4
│   INSPECTOR_INSET 6 / RULER_W 16
├── 字号
│   UI_FONT_SIZE 14（S12-11 裁决不变）
└── 九宫格皮肤参数
    SKIN_PANEL_MARGIN 8（48×48 面板皮肤）/ SKIN_BTN_MARGIN 4（48×20 按钮皮肤）
```

配套收敛：

- 装配段新增 **Theme 节点**（`root/theme`，`NodeKind::Theme`，八槽位值 =
  PALETTE 表 = `ThemeColors::DEFAULT_DARK` 同值，观感零漂移）；层级树投影
  skips 加 theme_node（皮肤数据不是可编辑对象，同 grid/ruler/dock 纪律）。
- 槽位名字符串字面量（"panel"/"border"/"accent"/"text_dim"）全部改为
  EditorTheme 槽名常量引用（12 处）。
- 散落 const（原 MARGIN/LEFT_PANEL_W/.../UI_FONT_SIZE/INS_ROW_H 等约 20 个）
  全部撤出文件体、收敛进 editor_theme；`DOCK_TITLE_H`/`SPACE_S` 两个新名
  替换了 4 处裸魔法数（dock 标题行 18.0、工具栏缝 4.0）。

## 3. 九宫格面板实践

### 3.1 纹理生成

`skin_rgba(w, h, margin, base, border, top_hi)`（editor_shell.rs，紧随
`solid_rgba`）：最外 1px 边框 → 顶缘 1px 高光（bevel-up）→ margin px 边带
内垂直微渐变（贴边 +8 → 内缘 -4）→ 中心平坦；噪点 = 确定性整数哈希 ±3
（无浮点 RNG，与 walk_sheet/beep 同家法：缺了再写、仓库只背一份小文件）。

| 纹理 | 尺寸 | 边距 | base | border | top_hi | 入库路径 |
| --- | --- | --- | --- | --- | --- | --- |
| 面板皮肤 | 48×48 | 8 | (30,34,40) | (58,64,72) | (66,74,84) | `examples/assets/Textures/panel_skin.bmp` |
| 按钮皮肤 | 48×20 | 4 | (44,50,58) | (58,64,72) | (92,102,116) | `examples/assets/Textures/button_skin.bmp` |

base/border 取自 DEFAULT_DARK 的 panel/border 槽同系值（成品绝对色皮肤；
主题槽继续管文本/选中/强调）。

### 3.2 应用面

- **三个大面板**（裸 Control，提取层仅 Control 读 ns_*，S16.6 口径）：
  Output dock `dock_bg` / Inspector `hud_ins_bg` / FileSystem `fs_bg` ——
  `ns_tex` + `ns_l/t/r/b = 8` + `ns_modulate=false` + `ns_tiling=false`。
  ns_* 非 schema 键，走 `set_prop_raw` 前向通道（z_index/border_w/font_size
  先例）。S16.7 的 `ns_modulate`/`ns_tiling` 常量提取层 crate 根未再导出，
  壳层按同名直写（extractor.rs L124/L126 同源，注释已标）。
- **六个工具栏按钮底板**（`tool_plate` × 6，裸 Control，z=-70 同工具带，
  建于六按钮之前 = 同 z 前序序先画垫底）：`ns_tex` 按钮皮肤 + 边距 4，
  offset 每帧随按钮投影同步重写。**按钮本体** `fill_slot=""`（槽解析对
  空名不覆盖 → ControlState 缺省透明底），纹理从按钮矩形透出。

### 3.3 控件状态 → 槽位映射表（egui `Widgets::style` 的 NES 落地）

| 控件态 | 视觉 | 判定来源 | 本轮改动 |
| --- | --- | --- | --- |
| 面板正常 | 九宫格纹理（绝对色） | 恒 | 壳层 ns 属性 |
| 按钮正常 | 纹理底板 + border 槽 1px 框 | 恒 | 底板 + fill_slot="" |
| 按钮 hover | 同上 + accent 槽边框 | UiVm hover（提取层既有） | 无 |
| 按钮 pressed | accent 填充 + accent 边框（盖过纹理） | UiVm pressed（既有） | 无 |
| 输入框 focused | accent 边框 | UiVm focused（既有） | 无 |
| 列表选中行 | selected 槽高亮 | selected 投影（既有） | 无 |

判定优先序 pressed > hover > focused > 缺省 —— 提取层 `button_states_of`
既有 if 链已是此序，壳层零改动。**面板/标尺/分隔线/网格的取舍**：三大
面板换纹理；16px 标尺条带、1px 分隔线、网格条带保持 flat fill（条带厚度
≤ 边距带宽，纹理化无意义；Godot 标尺同为 flat）。

### 3.4 实窗截图取证

120 帧冒烟只证"不崩不脏"，不足以证"九宫格真的在画"（若 ns_tex 的资源
id 误接，渲染侧分臂会静默不画面板）。补一次实窗取证：`NES_GAME_FRAMES=
20000` 后台起编辑器，PowerShell 按 MainWindowHandle 定位 + 全屏截图 ——
Inspector / FileSystem / Output 三面板与六个按钮均见纹理边框与渐变
（bevel 观感），层级树/网格/标尺/精灵/选中高亮与换肤前逐位一致；取证图
未入库（临时文件已删）。纹理像素另行抽点验证（边框 58,64,72 / 高光
66,74,84 / 中心 ~30,34,40 ± 噪点，BMP BGR 序读出吻合生成参数）。

### 3.5 到站信号（S16.1 tween_done）

按任务书第 4 条**保留现状**：编辑器壳层无补间消费点（spin.nes 走 process
通道），tween_done 信号已由 S16.1 自身冒烟覆盖，换皮不与它交互。

## 4. 门禁

worktree `wt-skin`（分支 `s18-editor-skin`）实测：

| 门禁 | 结果 |
| --- | --- |
| 十 crate `cargo test --release` | ✅ 全绿，合计 **746 通过 0 失败**（基线 736 + nes-render-wgpu 期后计数口径差 10，逐 crate 结果见提交前日志；本轮零新增测试 —— 换肤为壳层常量/纹理，无新逻辑面可测） |
| `cargo clippy --release` × 10 crate | ✅ 0 warning |
| `check_dependency_direction.py` | ✅ 15/15 PASS |
| editor_shell 冒烟（`NES_GAME_FRAMES=120`） | ✅ 干净退出 |
| editor_shell 冒烟（`NES_EDIT_DEMO=1 NES_EDIT_FRAMES=420`） | ✅ 全断言通过（挂载/卸载/enabled/折叠/刷新/play/stop/reset/IME/字体探测/树形态） |
| first_game 冒烟 | ✅ |
| 实窗截图取证 | ✅ §3.4 |

## 5. 遗留

1. **深浅主题切换**：EditorTheme 色板表已单点化，但壳层无运行时切换入口
  （Theme 节点属性每帧被提取层读 —— 换肤 = 运行时改 8 个属性即可生效，
  待 UI 面板做"主题页"时接上）；纹理皮肤是绝对色，深浅切换需要两套皮肤
  纹理或转 modulate 路线（灰阶纹理 × 槽色，亮边改暗缝）。
2. **动画过渡**：主题切换的淡入/插值（iced/oxiui 的 anim_tokens 思路）未
  做；NES 侧可挂 S16.1 补间通道，壳层暂无消费点。
3. **icon 集**：工具栏按钮仍是文本（SEL/SNAP/...），无图标纹理；oxiui
  `IconSet`/zed `Panel::icon()` 的"图标 + tooltip + 切换动作"三件套是
  后续模板（UiVm 按钮已通，只差图标资产与绘制位）。
4. **ListView/TextInput 的面板纹理**：提取层 List/Button 摊平类不读 ns_*
  （S16.6 文档 §4 遗留），左栏 Scene 列表仍是 ListView 自带 panel 填充；
  层级树接真字体的老遗留（S12.11 §5）与本条同属"控件纹理化/字体化"后续。
5. **dock 拖拽**：面板尺寸三元组已进 EditorTheme（默认值级），F9 两档
  分割不变；zed Panel 式拖拽/缩放/最大化归编辑器布局里程碑。
