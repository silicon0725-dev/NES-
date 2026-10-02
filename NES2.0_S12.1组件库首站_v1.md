# NES 2.0 · S12.1 组件库首站 v1

> 交付日期：2026-10-02　｜　状态：**E-1 颜色管线 + Theme/Button 节点 + UiVm 状态机落地；T-UI/T-WID 契约回归**
> 前置：S12.0 组件库设计冻结（三件套裁决）；S9 五层纪律；S4 控件/文本光栅化。

---

## 0. 一句话结论

S12-0 冻结的三件套首站全部落地：**渲染契约带色**（E-1：实例 tint 通道
× 中性图集，`ControlState` 得 fill/border/border_w、`LabelState` 得
color）；**Theme/Button 成为场景节点**（八槽位色板 / 槽位引用属性，
schema 封闭、RON 往返）；**UiVm 交互状态机**（ScriptVm 同构：悬停/
按下瞬态、视口锚定命中、抬键命中激活钩子）接入运行时帧序（simulate
后、提取前）。六 crate **492 测试全绿**、clippy 0、守卫 11/11；Dodge
基线按协议重录（schema 扩属性 → 属性表缺省物化进指纹，S9 加 uid 同
款先例）。

---

## 1. E-1 颜色管线（渲染契约扩展）

**实现路径**：一切皆精灵实例的既有架构上，实例数据 12→16 floats 追加
`tint`（RGBA 归一化），片段着色 `采样色 × tint`；内建图集图案格转
**中性白**（边框格 + 新增填充格），颜色一律经 tint 进入——中性 tint
与 E-1 之前逐位相同。

| 层 | 变更 |
|---|---|
| render-api | `ControlState` += `fill`/`border`/`border_w`（缺省透明填充 + **绿色哨兵边框** `[0,255,0]` = 历史观感经数据保留）；`LabelState` += `color`（缺省白） |
| wgpu | 图集格 1 白框 + **格 2 纯白填充**（哨兵 3..15）；实例布局 loc5（48..64B）；控件绘制 = 填充四边形 + **四条 border_w 宽边条**（像素精确平直边框，不再随矩形缩放）；按钮文字锚定矩形左上 + 4px 内衬 |
| 契约测试 | 像素断言从"缩放边框"更新为"1px 平直边框"（t_control_01/02/05、backend、stats、text_11）；视觉基线按 bless 协议重录（0x68d1_4f07_989f_cbaa） |

**附带解锁**：游戏侧彩色 UI/HUD（此前全引擎控件只有绿框白字）。

## 2. Theme / Button 节点（nes-scene）

- `NodeKindTag::Theme`（纯数据，base=Node）：八槽位 I64
  `0xRRGGBBAA`——bg/panel/border/text/text_dim/selected/accent/danger；
  缺省 = `ThemeColors::DEFAULT_DARK`（像素工程师深色，S12.0 §6）
- `NodeKindTag::Button`（base=Control）：`text` + `fill_slot`
  （缺省 panel）+ `border_slot`（缺省 border）+ `text_slot`（缺省
  text）；继承 anchor/offset/size
- Control/Label 补槽位引用属性（`fill_slot` 空名=透明、`border_slot`
  缺省 border、`color_slot` 缺省 text）——面板与文字全部主题化
- `ui` 模块：`ThemeColors`（解析/查找/缺省）+ `WidgetState`（四态）
  + `UiVm`（状态机）

## 3. UiVm 交互状态机（S12.0 §2.2）

```text
每帧（runtime 帧序：simulate 后、提取前）：
  读输入快照（InputView 注入，S7.2 形态复用）
  → 前序遍历 Button 视口矩形（anchor*视口+offset，S3 单级锚定）
  → 命中 = 前序最后者（与相机/主题 last-write-wins 同款仲裁）
  → 边沿检测先行：按下沿记目标、抬键沿命中才激活（标准 UI 语义）
  → 状态表更新（悬停/按下）
```

- **零写权**：激活走 `on_activate` 钩子（宿主注册；示例经共享缓冲
  传出帧后写树，借用不打结）
- **瞬态不入指纹**（T-UI-04 钉死）：UI 摇动不影响游戏确定性
- 提取层经 `attach_ui(Rc<RefCell<UiStates>>)` 只读状态做四态着色：
  hover → accent 边框；pressed → accent 填充+边框（S12.0 §3.2 只换
  槽位解析、不引入新形状词汇）

## 4. 提取层（Button 摊平）

**单节点单渲染物**：`Admission::Button(key, layout, label)` 同句柄
`SetRect` + `SetText` 双推——消费者已是"一物多实例"（Label 一字形一
四边形），按钮是其矩形+文字组合形态，**零契约新增命令**。主题在推送
前一次解析（前序最后 Theme 生效，Q4 裁决；无节点 = DEFAULT_DARK 兜底）。

## 5. 契约回归

| 套件 | 编号 | 断言 |
|---|---|---|
| nes-scene/tests/s12_ui.rs | T-UI-01..04 | schema 封闭属性与往返；色板解析/查找/缺省；状态机（悬停/按下/移出释放不激活/完整点击激活一次）；瞬态不入指纹 |
| nes-render-extract/tests/s12_button.rs | T-WID-01..04 | 摊平同句柄三槽齐达；主题 last-wins + 槽位覆写；pressed 换档；Control/Label 槽位 + 空槽名透明 |
| nes-render-wgpu | 既有 90 例 | 像素契约更新为 1px 平直边框 + 视觉基线重录 |
| nes-runtime | 62 例 | Dodge 基线重录（见 §6） |

## 6. 语义可见变更（按协议处理）

1. **控件边框从"随矩形缩放"改为"像素精确 1px 平直"**——S12.0 设计
   语言冻结的有意变更；wgpu 像素契约与视觉基线同步更新
2. **Dodge 基线漂移重录**（97148400→b5b51dac）：Label 新增
   color_slot 后属性表缺省物化（节点创建即写入）进指纹——与 S9 加
   uid 同款协议先例：有意变更 + 里程碑评审 + 重录
3. **控件/文本默认观感绿框→主题深色框**：提取层现在显式推主题化色
   （无 Theme 节点 = DEFAULT_DARK）；render-api 契约缺省仍保留绿色
   哨兵（直接下命令的旧路径观感不变）

## 7. 示例

`nes-runtime/examples/s12_widgets.rs`：512x288 深色窗口——背景/面板
（Control 槽位）、三按钮（OK/CANCEL/DANGER——danger 槽位变体）、
状态行；悬停/按下四态实时可见，点击经激活钩子更新状态行。
`NES_GAME_FRAMES=90` 冒烟通过。

## 8. 后续（S12-2 起按 S12.0 §7 推进）

- S12-2：TextInput（DraftInput 语义 + 光标 + input/text 消费）+
  焦点路由 + editor_shell 检查器接真消费者
- S12-3：E-2 裁剪（SetClip + scissor）+ ScrollView + ListView + Tabs
- S12-4：F-4 编辑器集成（文件选择器 + 脚本面板 + editor_shell 换装）

## 9. 里程碑记注（M4 口径）

- 渲染契约扩展 E-1（颜色三字段 + 实例 tint）： additive——既有命令
  流语义不变，缺省值保旧观感
- 提取层新增 Button 摊平推送段 + 主题解析；`themed_control`/
  `themed_label` 公开为独立重算路径（"推送=重算"契约延续）
- wgpu 像素基线与视觉基线双录：契约测试更新与 bless 协议留痕
