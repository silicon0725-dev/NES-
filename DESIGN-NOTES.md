# DESIGN-NOTES — Rust GUI 参考库设计提取笔记（S18 编辑器换肤）

性质：**研究笔记，不是契约文档**。服务对象 = `nes-runtime/examples/editor_shell.rs`
的 S18 换肤实战（EditorTheme 收敛 + 九宫格纹理面板 + 控件三态观感）。

来源与取证方式（参考库**只读不改**，纪律同任务书）：

- `ruference/egui`、`ruference/xilem`、`ruference/oxiui`：工作树未检出（目录只有
  `.git`），全部内容经 `git -C <repo> show HEAD:<path>` / `git ls-tree` 只读提取，
  不触碰工作树；egui HEAD = `23348e9f`（2026-10-04）。
- `ruference/iced`：工作树已检出，直接读文件。
- `ruference/zed`：克隆为空；按任务书预案用 `curl` 单文件取
  `crates/gpui/src/style.rs` 与 `crates/workspace/src/dock.rs`（GitHub raw，
  2026-10-03 取到）。slint 未取得（网络限制），Design tokens 章节缺席，
  以 oxiui 的 tokens.rs 文件面佐证。
- `ruference/oxiui` 是**部分克隆（promisor remote）**：blob 按需拉取且本轮网络
  拉取失败，故 oxiui 的结论基于**目录树 + `lib.rs` 的 re-export 面**（tree 对象
  在本地），未逐行读实现 —— 结论强度标注为"文件面证据"。

NES 侧对照代码（worktree，`6909d18`）：

- `nes-scene/src/ui.rs` —— `THEME_SLOTS` 八槽位（bg/panel/border/text/text_dim/
  selected/accent/danger，I64 `0xRRGGBBAA`）+ `ThemeColors::from_tree`（主题即
  场景节点）+ `WidgetState` 四态（hover/pressed/selected/focused）。
- `nes-render-extract/src/extractor.rs` —— `button_states_of`（hover→accent 边框、
  pressed→accent 填充+边框）、`themed_control`/`themed_label`（fill_slot/
  border_slot/color_slot 名解析）、`nine_slice_of`（ns_tex/ns_l/t/r/b/
  ns_modulate/ns_tiling，仅裸 Control）。
- `nes-render-wgpu/src/renderer.rs` —— `push_nine_slice`（九片拉伸/平铺、边距
  钳制、modulate 时 tint 取 `ControlState.fill`；九宫格模式下 fill/border 条带
  **不画**）。

---

## 1. egui —— Widget 交互状态机与集中式视觉样式

出处：`crates/egui/src/sense.rs`、`crates/egui/src/widgets/button.rs`、
`crates/egui/src/style.rs`（HEAD 23348e9f）。

### 1.1 布局组织

立即模式：每帧重建控件描述，**状态住 Context/Memory**（`memory`/`interaction.rs`），
控件本身无状态。布局与绘制在同一遍里由 `placer.rs`/`layout.rs` 完成。

### 1.2 Theme / 样式结构

`style.rs` 的集中定义（全部视觉参数一处出）：

- `Style { text_styles, spacing: Spacing, interaction: Interaction,
  visuals: Visuals, animation_time, ... }`（style.rs L244）。
- `Visuals`（L1086）：`dark_mode`、`widgets: Widgets`、`selection`、
  `panel_fill`（面板底色，L1642 附近）、`window_fill`、`window_stroke`、
  `faint_bg_color`/`extreme_bg_color`（"几乎不可见的一层底"与"极端底"两个
  灰阶级别）…… `Visuals::dark()` / `Visuals::light()` 是**两个完整定值预设**，
  深浅主题切换 = 整表替换，无半套配置。
- `WidgetVisuals`（L1387）：`bg_fill`（必须有底的控件）、`weak_bg_fill`
  （可有可无底的控件，如按钮）、`bg_stroke`（外框）、`corner_radius`、
  `fg_stroke`（文字/把手的颜色也在这 —— 前景即描边色）。
- `Widgets`（L1347）：**按交互状态分组的一张五格表**
  `{ noninteractive, inactive, hovered, active, open }`，每格一套
  `WidgetVisuals`。

### 1.3 控件状态模型

- `sense.rs`：`Sense` 位flags（`HOVER`/`CLICK`/`DRAG`/`FOCUSABLE`）——
  控件声明"感知什么交互"，而不是各自写命中逻辑。`Sense::click() =
  CLICK | FOCUSABLE`（按钮默认可聚焦）。
- 状态聚合在 `Response`（`response.rs`：`hovered()`/`clicked()`/`dragged()`/
  `is_pointer_button_down_on()`），由 Context 统一结算，控件零各自判定。
- **状态→外观的单一判定函数**（style.rs L1370，`Widgets::style`）：

```rust
pub fn style(&self, response: &Response) -> &WidgetVisuals {
    if !response.sense.interactive() { &self.noninteractive }
    else if response.is_pointer_button_down_on() || response.has_focus()
         || response.clicked()                                        { &self.active }
    else if response.hovered() || response.highlighted()              { &self.hovered }
    else                                                             { &self.inactive }
}
```

  优先序：按下/焦点 > 悬停 > 静止；不可交互恒走 noninteractive —— **一张表 +
  一处 if 链**就是全部"换档"，任何控件不得私开色源。

- `widgets/button.rs`：`Button` 的变体走 `ClassName` 字符串类
  （`CLASS_SELECTED`/`CLASS_SMALL`/`CLASS_NO_FRAME`……），配
  `widget_style::ButtonStyle` 按类覆写 —— "类"是外观开关，不是新控件。

### 1.4 NES 可借鉴

1. **状态→槽位映射表化**：NES 的 `button_states_of`（extractor.rs L786 附近）
   与 egui `Widgets::style` 同构（pressed 优先于 hover，hover 优先于缺省），
   方向已被验证正确；S18 在**壳层**做的对应物是把每个状态的视觉落点写成
   EditorTheme 里的映射表（见 §6.4），壳层不再散落魔法色值。
2. **`panel_fill` 直译**：egui 把"面板大面填充"设为 `Visuals` 的一等槽位 ——
   NES 的 `panel` 槽 + `fill_slot` 名解析正是同款，八槽位契约不需要扩。
3. **`weak_bg_fill` vs `bg_fill`**：按钮"底可有可无"（weak）与"底必须有"
   （slider/checkbox）分两色 —— S18 按钮换皮走"九宫格底板（板有底）+
   按钮本体 fill 透明"正是这一区分的壳层实现。

## 2. iced —— Seed 色板推导与 Catalog 类 × 状态矩阵

出处（工作树直读）：`core/src/theme/palette.rs`、`core/src/theme.rs`、
`core/src/layout.rs`、`widget/src/button.rs`。

### 2.1 布局组织

`core/src/layout.rs`（205 行，小而完整）：`Layout { position, size,
parent: Option<Rectangle> }`，`iter()` 沿 `widget::Tree` 递归出子布局，
`child = parent.position + child.translation`；`next_to_each_other` /
`atomic(limits, w, h)` 等纯函数助手。布局是**无状态树遍历**，每帧从
`Limits` 重算 —— 与 NES"宿主每帧投影重写 offset/size"同一家法
（S12-4 口径），互相印证。

### 2.2 Theme / 色板结构

**Seed → Palette 派生**（palette.rs）：

- `Seed { background, text, primary, success, warning, danger }`（L152）——
  一套主题只有 6 个种子色。
- `Palette::generate(Seed)`（L27）：`Background` 从 base 按固定偏差推出
  8 级（`deviate(base, 0.03)` weakest → `deviate(base, 0.20)` strongest，
  L88-94）；每个彩色轴出 `Swatch { base, weak(= base.mix(bg, 0.4)),
  strong(= deviate(base, 0.1)) }`（L122-131）。
- `Pair { color, text }`（L42）：每级底色配"保证可读"的文本色
  （`readable(color, text)`）—— **槽位对**思想：底与字永不脱钩。
- `Seed::LIGHT/DARK/DRACULA/NORD/SOLARIZED/...`（L169+）：预设 = 6 个
  hex 常量，整套主题一屏写得下。
- `core/src/theme.rs`：`Theme` 枚举 = 命名预设列表 + `Custom(Arc<Custom>)`；
  `palette()` 纯查表。**预设是枚举不是配置文件** —— 换肤面收敛为一小组
  命名常量。

### 2.3 控件状态模型

`widget/src/button.rs`：`Catalog` trait ——
`type Class<'a>`（Primary/Secondary/Danger 这类"按钮类"）×
`Status`（Disabled/Hovered/Pressed 等）→ `Style { background, text_color,
border, shadow, snap }`（L470 附近）。**类 × 状态矩阵**产出最终样式，
矩阵由主题实现一次、控件零私色。

### 2.4 NES 可借鉴

1. **种子推导**（最大启发）：NES 八槽位若未来做"主题包"，不必手填 8 个
   独立色 —— 用 2~3 个种子色 + 固定偏差公式（panel = bg 偏 0.03~0.07、
   border = bg 偏 0.12 一类）在 EditorTheme 里**编译期推导**，整组观感
   天然协调；P0 先手填 DEFAULT_DARK 同值，推导公式记进笔记备用。
2. **Pair 槽位对**：NES 已用 text/text_dim 两个独立槽覆盖，不必扩槽位；
   但换肤时"底色一动、文本色跟着复核"应成为检查单条目。
3. **Catalog 类矩阵**：NES 控件已按 NodeKind 分型（Button/TextInput/List
   各自 `*_states_of`），等价物已存在；S18 不新增类。

## 3. xilem / masonry —— retained UI 的失效纪律

出处：`masonry/ARCHITECTURE.md`、`masonry_core/src/core/widget_state.rs`、
`masonry_core/src/passes/`（HEAD 树）。

### 3.1 布局组织

保留式 widget 树：`WidgetState`（widget_state.rs）住每节点的几何与失效
旗标；**pass 系统**（passes/）每帧按需跑全树：`event / update / mutate /
anim / layout / compose / paint / accessibility / action`，一文件一 pass。

### 3.2 Theme / 色板结构

架构文档：theme 住 baseline crate（`masonry` = "baseline widgets +
**a default theme**"）—— 默认主题是基础组件库自带的一张表，不是外挂
配置系统（与 NES `THEME_SLOTS::DEFAULT_DARK` 兜底同构）。

### 3.3 控件状态模型

`WidgetState` 命名法（widget_state.rs 头注）：`request_xxx`（本节点请求
xxx pass）/ `needs_xxx`（本节点或后代请求）/ `is_xxx`（本节点处于 xxx 态）/
`has_xxx`（本节点或后代处于 xxx 态）；子→父 `merge_up` 冒泡；pass 结束
必须清旗标，否则**"zombie flags"**（失效旗标永生，每帧白跑全树）——
文档单列一节警示。

### 3.4 NES 可借鉴

1. **反面教材的正确用法**：NES 编辑器壳是"每帧全量投影"（UI 零自有状态、
   offset/size 每帧重写），没有失效系统 —— masonry 的整套旗标机制正是
   NES **刻意不引入**的东西；S18 换肤保持"皮肤参数全部是常量/纹理"，
   不新增任何 per-frame 失效/缓存，是这一教训的直接应用。
2. **默认主题住基础层**：masonry 把 default theme 放在 widgets 同层 ——
   NES 的 `THEME_SLOTS` 在 nes-scene 同位置，**结论：换肤色板应继续以
   nes-scene 八槽位为契约中心**（见 §6.1），而不是壳层外置一套平行色板。

## 4. oxiui —— crate 分层与 Design tokens（文件面证据）

出处：`git ls-tree` 目录树 + `crates/oxiui-theme/src/lib.rs`（blob 不可读，
见头部说明）。

### 4.1 布局组织（crate 切分）

`crates/`：`oxiui-core` / `oxiui-theme` / `oxiui-render-wgpu` /
`oxiui-render-soft` / `oxiui-accessibility` / `oxiui-table` / `oxiui-text` /
`oxiui-hot-reload-notify` + 后端绑定（egui/iced/slint/dioxus/web 各一 crate）。
**theme 独立成 crate、渲染按后端切分、accessibility 独立** —— 与 NES
分层政策（core 零依赖、render-api/render-extract/render-wgpu 三段）同构度
很高；oxiui 多出的 theme crate 是它规模（多后端绑定）带来的，NES 单后端
暂不需要。

### 4.2 Theme / 色板结构（lib.rs re-export 面）

`oxiui-theme` 文件面：`tokens.rs`（`DesignTokens`/`RadiusStep`/`SpacingStep`
—— 间距/圆角 token 表）、`typography.rs`（`TextStyleToken`/`TypographyScale`）、
`stylesheet.rs`（`Selector`/`Specificity`/`Rule`/`ComputedStyle` —— CSS 式
规则引擎）、`manager.rs`（`ThemeManager`/`ThemeListener` —— 运行时换主题）、
`overlay.rs`（`PartialTheme` —— **主题叠加 delta**）、`builder.rs`
（`PaletteBuilder` + `WcagLevel` 对比度校验）、`high_contrast.rs`、
`gallery.rs`（catppuccin/nord/solarized/dracula/material 预设工厂）、
`serial.rs`（`ThemeSnapshot` 序列化）、`anim_tokens.rs`、`breakpoint.rs`、
`icons.rs`（`IconSet`/`IconName`）。

### 4.3 控件状态模型

文件面无逐行证据（blob 未取到）；从 `ComputedStyle`/`Selector` 存在推断
状态以选择器表达（CSS 伪类式）—— 标注为**推断**。

### 4.4 NES 可借鉴

1. **PartialTheme 叠加思想**：换肤 = 默认表 + delta 表，而不是第二套全量表
   —— 对"八槽位扩展还是外置"的第三方答案：**槽位契约不动，主题包以
   delta 形式表达**。P0 的 EditorTheme 把 8 个槽位值显式列全（不加 delta
   机制），但组织上与 DEFAULT_DARK 对齐，将来引入 PartialTheme 语义时
   无需改契约。
2. **token 三件套的组织**：色板/间距/圆角/字号各自一小块（tokens.rs/
   typography.rs），编辑器壳层的 S18 对应物 = EditorTheme 里的色板块 +
   间距栅格块 + 行高块（见 §6.2），不再散落。

## 5. zed / gpui —— StyleRefinement 与 Panel 面板契约（curl 单文件）

出处：`crates/gpui/src/style.rs`、`crates/workspace/src/dock.rs`（GitHub raw，
2026-10-03）。

### 5.1 布局组织

`style.rs`：`Style` 大结构体 + **Refinement 模式**（`CornersRefinement`/
`PointRefinement`/`EdgesRefinement`/`SizeRefinement`，`refineable` 派生）——
样式 = 基础 Style + 部分 delta refinement，链式 `.bg(..).border_1(..)` 逐项
覆盖。长度系统类型化（`Pixels`/`DefiniteLength`/`Length`/`rems`）。

### 5.2 Theme / 色板结构

style.rs 本体无色板（zed 色板在 `crates/theme`，本轮未取）；从 refinement
模型可见的取舍：**颜色是样式字段不是 token 表**（语义色由上层 theme crate
供给，gpui 只管形状/描边/填充的载体类型）。

### 5.3 Dock/Inspector 面板结构（dock.rs，研习点）

`trait Panel`（L36）：面板 = 数据 + 一组**自描述容量**：

- `persistent_name()` / `panel_key()` —— 持久身份（设置/布局恢复的键）；
- `position()` / `position_is_valid()` / `set_position()` ——
  `DockPosition { Left, Bottom, Right }`（L324），面板声明自己允许的停靠位；
- `default_size()` / `min_size()` / `supports_flexible_size()` /
  `has_flexible_size()` —— 默认尺寸 + 最小尺寸 + 可否弹性拉伸三档；
- `initial_size_state()` / `size_state_changed()` —— 尺寸档位状态化；
- `is_zoomed()` / `set_zoomed()` —— 最大化档；
- `icon()` / `icon_tooltip()` / `toggle_action()` —— 状态栏开关钮三件套；
- `activation_priority()` —— 激活顺序。

Dock 聚合 `PanelHandle`（trait 对象擦除面，L106）持有全部面板。

### 5.4 NES 可借鉴

1. **每面板一个尺寸三元组**（默认/最小/弹性）：S18 把
   `LEFT_PANEL_W`/`INSPECTOR_W`/`DOCK_H` 收进 EditorTheme 时按
   "每面板：宽 + min + 档位"组织（P0 档位即 F9 两档分割比的常量表），
   为将来 dock 拖拽留好数据形状，**本轮零行为引入**。
2. **persistent_name 已被 NES 覆盖**：面板身份 = uid/registry_key（S9.0
   对象模型），不引入新机制。

---

## 6. 汇总：NES 编辑器换肤方案（本笔记的落地结论）

### 6.1 色板结构 —— 扩展八槽位还是外置？**结论：不扩不外置，锚定八槽位契约**

- `ThemeColors`/`THEME_SLOTS`（nes-scene/src/ui.rs L36/L68）已是"主题即
  场景节点"：换主题 = 写 8 个 I64 `0xRRGGBBAA` 属性，提取层/渲染层零改动
  （extractor.rs L285：前序序最后的 Theme 节点胜出）。
- iced（Seed 推导）与 oxiui（token 树/StyleSheet）都是为"任意应用 × 多主题
  包"设计的通用机器；NES 单编辑器单后端，P0 用不上 —— masonry 的取舍
  （默认主题住基础层）才是同量级答案。
- **落地**：editor_shell 新增一个 `Theme` 节点，八槽位值来自壳层
  `EditorTheme` 的色板常量（P0 取值 = `DEFAULT_DARK` 同值，观感零漂移，
  但换肤入口从"改 nes-scene 源码"变成"改壳层常量表"）。纹理皮肤
  （§6.3）承担可见观感变化。
- 槽位名不新增。控件→槽位的引用面（fill_slot/border_slot/color_slot）
  保持既有解析（themed_control/themed_label），壳层只写槽位名字符串。

### 6.2 间距 / 字号 token（EditorTheme 统一出口）

| token | 值 | 既有出处 |
| --- | --- | --- |
| `MARGIN`（外边距） | 8 | editor_shell.rs L266 |
| `SPACE_S`（缝/内衬） | 4 | 工具栏 4px 缝、分隔条、输入框内衬（散落值收敛） |
| `LEFT_PANEL_W` | 180 | L267 |
| `INSPECTOR_W` | 190 | L268 |
| `DOCK_H` | 96 | L274 |
| `TOP_BAND` / `STATUS_BAND` | 40 / 24 | L269-270 |
| `TOOLBAR_H` / `TOOLBAR_BTN_*` | 24 / 48×20 步 52 | L346-350 |
| 行高三档 | 16（标题行）/ 18（列表行）/ 20（真字体行 INS_ROW_H） | L277/307/394 |
| `UI_FONT_SIZE` | 14 | L388 |

形式：壳层 `EditorTheme`（关联常量的结构体命名空间），文件顶部一处定义；
散落字面量改引用。**不新增机制，只收敛出口**（egui Style"视觉参数一处出"
的最小对应物）。

### 6.3 面板九宫格纹理参数

代码生成 BMP 入库（walk_sheet 先例，frame_demo.rs L34/L94；缺了再写，
仓库只背一份小文件）：

- **panel 皮肤**：48×48，边距 8px（`ns_l/t/r/b = 8`）。深色底
  （≈ `panel` 槽 0x1E2228 同系）+ 1px 亮边框（≈ `border` 槽 0x3A4048 同系）
  + 8px 边带内垂直微渐变 + 确定性噪点（哈希 ± 少量，无浮点 RNG）。
- **button 皮肤**：48×20，边距 4px。底色比面板亮一档（weak_bg_fill 思想）+
  顶缘 1px 高光（bevel-up，按钮"浮起"直感）+ 同系边框。
- **染色模式**：`ns_modulate = false`（中性白，既有行为）。理由：乘法 tint
  下纹理亮度 ≤ fill 槽色 —— "亮边框比面板底亮"在 `fill_slot="panel"`
  （深色）下乘不出来；S16.7 注释（renderer.rs L2329-2337）明示 modulate
  面向"灰阶纹理 × 面板色"的换色场景，与"带亮边的成品皮肤"不同路。
  皮肤 = 绝对色成品纹理；主题槽继续管文本/选中/强调。
- **应用面**：三个裸 Control 大面板（Output dock `dock_bg` / Inspector
  `hud_ins_bg` / FileSystem `fs_bg`）设 `ns_tex` + 8px 边距。标尺条带、
  分隔线、网格线保持 flat fill（16px 条带 ≤ 边距带宽，纹理化无意义；
  Godot 标尺同为 flat）。ListView 自带填充保持（提取层 List 不读 ns，
  S16.6 文档 §4 遗留项，本轮不动 crate）。

### 6.4 控件状态 → 槽位映射表（egui `Widgets::style` 的 NES 落地）

| 控件态 | 视觉来源 | 状态判定 | 本轮改动 |
| --- | --- | --- | --- |
| 面板正常 | 九宫格纹理（绝对色） | 恒 | 壳层新增 ns 属性 |
| 按钮正常 | 九宫格底板纹理 + `border` 槽 1px 框 | 恒 | 壳层新增底板 Control + 按钮 `fill_slot=""`（透明底，egui weak_bg_fill 区分的壳层版） |
| 按钮 hover | 同上 + `accent` 槽边框 | UiVm `hover`（既有，button_states_of） | 无（提取层既有四态透传） |
| 按钮 pressed | `accent` 槽填充+边框（盖过底板纹理） | UiVm `pressed`（既有） | 无 |
| 输入框 focused | `accent` 槽边框 | UiVm `focused`（既有） | 无 |
| 列表选中行 | `selected` 槽行高亮 | 投影 `selected` 属性（既有） | 无 |
| 面板文本 | `text` / `text_dim` 槽 | `color_slot`（既有） | 无 |

判定优先序（egui L1370 同款）：pressed > hover > focused > 缺省 ——
提取层 `button_states_of` 的 if 链已是此序，**壳层零改动**。

### 6.5 到站信号（S16.1 tween_done）

本轮不强制演示（任务书第 4 条"保留现状即可"）：编辑器壳层无补间消费点，
`spin.nes` 走的是 process 通道非补间通道；S16.1 的到站信号已由其自身
冒烟覆盖，编辑器换皮不与它交互。

### 6.6 门禁口径

十 crate `cargo test --release` 全绿（基线 736 + 新增若有）+ clippy 0 ×10 +
`check_dependency_direction.py` 15/15 + editor_shell 双冒烟
（`NES_GAME_FRAMES=120` 干净窗 + `NES_EDIT_DEMO=1` 420 帧全断言）+
first_game 冒烟。壳层改动不得触碰任何 crate 源码（零新依赖、零语义变化）。
