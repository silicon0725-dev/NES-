# NES 2.0 · S12.0 编辑器组件库 设计冻结 v1

> 交付日期：2026-10-02　｜　状态：**设计冻结提案——零代码里程碑，过审后 S12-1 起实施**
> 前置：S11-2 哨塔防线（组件库先行的用户裁决）；S9 五层纪律；S4 控件/文本光栅化；NGVGE 1.0 控件研究（架构参照，视觉不继承——用户裁决）。

---

## 0. 一句话结论

编辑器 GUI 不引外部框架（零第三方纪律），以**三件套**形态自建组件库：
**控件即场景节点**（新 NodeKind，schema 封闭属性，提取层摊平成
SetRect/SetText）+ **UiVm 交互状态机**（ScriptVm 同构：瞬态状态表
/焦点路由/DraftInput 提交语义）+ **主题即场景节点**（八槽位语义色板，
控件引用槽位不写死颜色）。设计语言走像素工程师风（深色默认、平直
1px 边框、4px 栅格、等宽 16px）。渲染侧两处契约扩展前置：**颜色
通道**（S12-1）与**裁剪**（S12-3）。

---

## 1. 引擎真实现状（设计的事实基础）

| 能力 | 现状 | 组件库含义 |
|---|---|---|
| 节点种类 | Node / Node2D / Sprite2D / Camera2D / **Control / Label** / Script | 控件节点有现成挂点；新 kind 走既有 schema/序列化/提取通路 |
| Control 属性 | `anchor`(Vec2) + `offset`(Vec2) + `size`(Vec2) | 场景侧简化锚点；提取层已摊平成四边锚 ControlState |
| Label 属性 | `text`(Str) + `font_size`(I64 8..128) | 文本光栅化闭环（S4）；**无颜色** |
| 渲染契约 | SetRect / SetText / SetCamera / Submit——**命令不带颜色**，wgpu 侧固定色 | **颜色通道是 P0 硬前置**（本库第一块基石） |
| 裁剪 | 无（全量绘制） | ScrollView/长文本靠它——S12-3 契约扩展 |
| 输入 | input/key_down/up、mouse_move/down/up、**input/text**（字符流）；InputView 直读快照（ScriptVm 同款注入） | TextInput 输入源已通；UiVm 复用 InputView 形态 |
| 命中 | `hit(x,y)` 仅 Sprite2D（z_index 降序世界盒） | 控件命中 = UiVm 自算布局矩形点测（§4） |
| 布局解析 | `ControlState::resolve` 纯函数，在 render-api | 下沉 nes-scene::ui::layout，提取层复用（extract 依赖 scene，无新依赖边） |

---

## 2. 架构裁决 A：三件套（建议冻结）

### 2.1 控件即场景节点（保留模式）

`Button` / `TextInput` / `ScrollView` / `ListView` / `Tabs` 等为
NodeKindTag 新成员，与 Control/Label 同构：

- **schema 封闭属性**（D1 裁决延续）：每控件固定属性集（样式槽位引用、
  绑定目标、内容参数），不开放动态 KV；
- **提取层摊平**：一个控件节点 → 多条渲染物（背景 rect + 边框 rect +
  文本 + 光标 rect…），对渲染层而言控件是**推导**不是新图元（Label→字形
  序列的同款先例）；
- **可序列化**：面板布局本身是场景文件（.ron）——编辑器 UI 用引擎自己
  的场景格式描述，可检视、可脚本驱动、游戏 HUD 同样可用。

复合控件（如 ListView = 视口 + 行渲染 + 滚动条）是**单节点多渲染物**，
不是子树组合——避免每实例几十个节点的 uid/事务噪音。

### 2.2 UiVm 交互状态机（ScriptVm 同构）

瞬态交互状态**不进属性表**（写属性 = 弄脏文档 + 进事务 + 进语义指纹
——三重错误）。UiVm 与 ScriptVm 同款形态：

```text
UiVm {
    states: HashMap<NodeId, WidgetState>,   // 草稿文本/光标位/滚动偏移/悬停/按下
    focus: Option<NodeId>,                  // 焦点路由单链
    input: InputSlot,                       // InputView 注入（S7.2 形态复用）
    on_commit: CommitHook,                  // 提交钩子（编辑器接事务 / 游戏接信号）
}
```

- 每帧顺序：读快照 → 布局点测算悬停 → 点击路由焦点 → 键盘/字符流进
  焦点控件 → 状态机更新 → 提交时机判定（DraftInput 语义，§3）；
- **提交不直写**：UiVm 发提交请求（载荷 = 目标 uid + 属性名 + 值），
  编辑器宿主把 Inspector 事务接上（一条 Modified 记录）；游戏侧可
  `on "ui/commit"` 信号响应。UiVm 自身零写权——五层纪律延续；
- 重挂载复位（与 ScriptVm locals 同款语义）。

### 2.3 主题即场景节点

`Theme` 节点：八个语义槽位固定 props（I64 0xRRGGBBAA 打包）：

```text
bg（窗口底）/ panel（面板）/ border（边框）/ text（正文）
text_dim（次级文字）/ selected（选中）/ accent（强调）/ danger（危险）
```

- 控件样式 prop 引用**槽位名**（Str，如 `"panel"`），提取期解析到活动
  Theme 节点取色——换主题节点 = 整个 UI 换肤，零逐控件改动；
- 1.0 研究教训落地：主题参数从第一天进组件库（theme 是 1.0 后补的）；
- 默认深色主题一个（内置常量兜底：无 Theme 节点时用缺省色板——场景
  不带主题也能渲染）。

### 2.4 宿主薄糖（方案 B 的薄封装）

编辑器代码侧给一层**纯构造糖**（非状态层）：`panel("inspector")`、
`button(text, slot)` 之类函数直接产出/更新控件节点——Rust 侧手感接近
声明式，但产物是场景节点，无第二套状态。糖在 nes-runtime（宿主层），
引擎核心无感。

---

## 3. 语义冻结

### 3.1 DraftInput 提交语义（1.0 实证移植）

```text
草稿（UiVm 瞬态） --Enter/失焦--> 提交（on_commit -> 事务 Modified）
                          Esc   --> 回滚（草稿 := 目标当前值）
数值型：非法输入拒绝提交（保持焦点）；min/max/step 校验复用 schema
        validate 的 clamp_to_hint（与 Inspector 同一条校验路径）
```

### 3.2 焦点与四态

- 焦点：点击命中获得；Tab 焦点链循环；点空白处失焦（= 提交）；
- 四态词汇（全控件统一，不逐控件发明）：`hover` / `pressed` /
  `selected` / `focused`——四态只影响**槽位解析**（如 focused 时边框
  换 accent 槽），不引入新形状；
- 悬停/按下判定：UiVm 布局点测（控件世界矩形 = anchor/offset/size
  经 nes-scene::ui::layout 递归解析——与提取层同源，无双实现）。

### 3.3 序列化与指纹裁决（建议）

| 层 | 进序列化？ | 进语义指纹？ |
|---|---|---|
| 控件节点/props/transform | 是（.ron 往返） | 是（游戏 HUD 状态 = props，一致性保持） |
| UiVm 瞬态（草稿/光标/滚动/悬停/焦点） | 否（内存态） | **否**——游戏确定性不被 UI 摇动（与 script locals 同口径） |

### 3.4 IME 边界

本阶段不做组合窗。WM_CHAR 字符流直进（`input/text`）——ASCII
标识符/数值编辑够用；中文输入与真字体（多字号/中文图集）同里程碑
后置。1.0 实证：其 IME 是浏览器 DOM 白送的，自绘引擎的 IME 是独立
工程量，不与组件库耦合。

---

## 4. 渲染契约扩展（两处，均配契约测试）

| # | 扩展 | 内容 | 前置于 |
|---|---|---|---|
| E-1 颜色 | `SetRect`/`SetText` 增颜色参（RGBA8）；wgpu 顶点/常量带上色 | 全部控件 | S12-1 |
| E-2 裁剪 | `SetClip { rect }` 命令（栈式，帧内生效）；wgpu scissor 实现 | ScrollView/TextView/Tabs 溢出 | S12-3 |

E-1 顺带解锁游戏侧彩色 UI/HUD（现状固定色是全引擎的显性限制）。

---

## 5. 控件清单与分级（1.0 实证 + 编辑器需求）

| 级 | 控件 | 说明 |
|---|---|---|
| **P0** | Button（含图标语义）、Label 增强（颜色/对齐）、Theme、**UiVm 骨架 + 四态** | 颜色管线 + 悬停/按下落地即验证 |
| **P0** | TextInput（单行，DraftInput 语义 + 光标 + input/text 消费）+ 焦点路由 | 检查器改名/数值编辑的直接消费者 |
| **P1** | 裁剪 E-2、ScrollView、ListView（行渲染 + 选中 + 滚动条）、Tabs | 层级树/文件列表的本体 |
| **P2** | Modal、Menu/ContextMenu、DraggableWindow、ColorPicker（I64 打包编辑）、TextView（多行，脚本面板本体） | 编辑器集成期按需取用 |

（Filter 搜索框 = TextInput 变体不单列；Controls 试玩条 = Button 组合
不单列——play-in-editor 里程碑再编排。）

---

## 6. 设计语言（像素工程师风，用户裁决）

- **基调**：深色默认；亮色经 Theme 节点切换；
- **色彩纪律**：只允许 §2.3 八槽位；组件硬编码颜色 = 回归测试失败
  （守卫加一条静态检查）；
- **几何纪律**：1px 边框、4px 间距栅格、无圆角无阴影无渐变；焦点/
  悬停用槽位变化表达；
- **字体纪律**：等宽 16px 是唯一字号（现图集）；排版以字符格为基本
  单位，多字号随真字体里程碑解冻；
- **交互词汇**：§3.2 四态，全控件一致。

---

## 7. 里程碑拆分（过审后逐个实施）

| 站 | 内容 | 验收形态 |
|---|---|---|
| S12-1 | UiVm 骨架 + 主题节点 + E-1 颜色管线 + Button + 四态 | 契约测试 + 窗口示例（深色面板上可悬停/按下的按钮） |
| S12-2 | TextInput + 焦点路由 + DraftInput 语义 | 契约测试 + editor_shell 检查器改名走 TextInput（真消费者） |
| S12-3 | E-2 裁剪 + ScrollView + ListView + Tabs | 层级树换 ListView（真消费者） |
| S12-4 | F-4 集成：文件选择器（ListView）+ 脚本面板（TextView）+ editor_shell 整体换装 | 编辑器全部面板由组件库驱动 |
| S12-5+ | P2 控件 + play-in-editor 编排 | 按需 |

---

## 8. 开放问题（过审时裁决）

| # | 问题 | 建议 |
|---|---|---|
| Q1 | ~~Tab 键是否在 Key 枚举~~ | **已查证**：VK_TAB→Key::Tab 映射与名字往返均齐全（window.rs:106 / input.rs:65），无需补 |
| Q2 | ui/commit 信号 vs 回调钩子，游戏侧是否首里程碑就通 | 先回调（编辑器足够）；信号通路 S12-4 随集成加 |
| Q3 | 光标闪烁（时间驱动）与固定步长模拟的关系 | 渲染侧动画（提取层自计时），不进 tick 语义 |
| Q4 | Theme 多节点并存时谁生效 | 前序遍历最后写入者（与 Camera2D 单槽同款规则） |

---

## 9. 反面教材对照（为什么这样设计）

1.0（TurboWarp 系）三大教训，本设计逐一结构性规避：

| 1.0 症状 | 本设计对策 |
|---|---|
| inspector 3228 行 / explorer 3140 行——UI 吞噬逻辑 | UI 是投影（S9）；UiVm 零写权；提交走事务 |
| 状态全在 UI store（Redux），撤销/选中/悬停混作一团 | 五层既有分层 + UiVm 瞬态与文档状态严格分离（§3.3） |
| 主题后补、逐控件改色 | 主题槽位从第一天进契约（§2.3） |
