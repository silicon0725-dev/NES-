# NES 2.0 · S12.2 文本输入 v1

> 交付日期：2026-10-02　｜　状态：**TextInput 节点 + 焦点路由 + DraftInput 语义 + 光标渲染；editor_shell 检查器接真消费者**
> 前置：S12.0 组件库设计冻结（三件套裁决 + §3.4 IME 边界）；S12.1 组件库首站（UiVm/E-1/Button）。

---

## 0. 一句话结论

S12-2 全部落地：**TextInput 成为场景节点**（schema 五属性封闭：text /
fill_slot / border_slot / text_slot / placeholder，RON 往返）；**焦点
路由**（单一焦点槽：点击夺焦、Tab 按场景序轮转、点空白失焦）；
**DraftInput 语义**（草稿/光标是 UiVm 瞬态，Enter 提交 / Esc 回滚 /
失焦=提交，提交不直写属性表、只走 `on_commit` 钩子——零写权延续）；
**光标渲染**（契约 `LabelState::caret` 字符下标位 + 渲染侧 16px 等宽
步长 1px 竖条，闪隐节拍由提取层 30 帧奇偶裁决、后端零动画状态）；
**P0 只收 ASCII 可打印（0x20..=0x7E），非 ASCII 忽略**（S12.0 §3.4
IME 边界：不做组合窗，中文/真字体后置）。editor_shell 检查器换真
消费者：节点重命名走 TextInput，一次提交 = 一条 `modify_name` 事务。
六 crate **505 测试全绿**、clippy 0、守卫 11/11；渲染契约 additive
（`caret: Option<u16>` 缺省 `None` = 既有路径逐位不变），无基线重录。

---

## 1. TextInput 节点（nes-scene）

- `NodeKindTag::TextInput`（base=Control，ALL 序 9）——继承
  anchor/offset/size；`NodeKind::TextInput` 同步入枚举
- schema 封闭属性（RON 往返）：

| 属性 | 类型 | 缺省 | 语义 |
|---|---|---|---|
| `text` | Str | `""` | **已提交值**（也是初始值）；编辑中的草稿是 UiVm 瞬态，不落此属性 |
| `fill_slot` | Str | `panel` | 填充槽位名（主题解析） |
| `border_slot` | Str | `border` | 边框槽位名（获得焦点时换档 accent） |
| `text_slot` | Str | `text` | 文字槽位名 |
| `placeholder` | Str | `""` | 占位提示（**P0 仅存储，不参与渲染**） |

- `InputView` 契约加默认方法 `text() -> Vec<u32>`（Unicode 标量值序
  列；缺省空 Vec——S7.2/S8.2b 既有实现者不破坏）

## 2. UiStates 重构 + 焦点路由（ui.rs）

- **UiStates 从单一 HashMap 重构为两张子表**：`widgets`
  （NodeId → WidgetState，`Copy` 四态旗标）+ `texts`（NodeId →
  `TextState { draft, caret }` 编辑会话，**非 Copy**，仅持焦期间
  存在）；死亡节点双表清扫 + 焦点槽悬垂清空
- **焦点路由**：UiVm 增单一焦点槽 `focus: Option<NodeId>`——
  - 按下沿点到 TextInput → 夺焦（开编辑会话：草稿 := 节点 `text`
    已提交值，光标到末尾）；点到非焦点物 → 先失焦（=提交）
  - 抬键沿命中空白 → 失焦 + 提交（标准"点外面收起"语义）
  - Tab 按下沿：可焦点控件（Button/TextInput，可见者）按**前序**
    （提取层同款确定性序）循环轮转
  - 激活回调**仅 Button**——TextInput 点击是夺焦，不是激活
  - `focused` 进 `WidgetState` 四态词汇，提取层据此换 accent 边框
- 键探针双名兼容（`"Enter"`/`"enter"`）：真实快照桥的冻结大小写
  口径与 S12 单测假读面同语义

## 3. DraftInput 提交语义

```text
文本泵（仅持焦点的 TextInput；全部按下沿）：
  字符   → 光标处插入（P0 仅 ASCII 可打印 0x20..=0x7E，非 ASCII 忽略）
  Backspace → 删光标前一字符
  Enter  → 提交草稿整体值（经 on_commit 钩子），焦点保留、会话继续
  Escape → 回滚：草稿 := 节点 text 已提交值
  失焦   → 提交草稿（仅 TextInput；无会话 = 提交节点现值——会话从未
           开始）。Button 等纯占焦控件失焦**不发** on_commit：无编辑
           会话，不回读 `text` 属性伪造载荷（T-UI-05 对照组钉死）
```

- **零写权延续**：提交不直写属性表，只回调 `on_commit(NodeId,
  Value::Str)`——落不落属性表、怎么落事务由宿主决定
- **瞬态不入指纹**（T-UI-08 钉死）：聚焦 + 打字前后
  scene_fingerprint 逐位相同
- 光标/草稿字符下标口径：`chars().count()`（Unicode 标量值）
- **P0 缺口：左右光标移动未做**（泵注释与 T-UI-06 均留痕，后续
  里程碑补）

## 4. 光标渲染（契约 + wgpu）

| 层 | 变更 |
|---|---|
| render-api | `LabelState` += `caret: Option<u16>`（字符下标；缺省 `None` = 不画，additive——既有路径逐位不变） |
| extract | 输入帧序 `stamp / 30 % 2` 裁决闪隐半拍（**纯渲染侧动画**，不进 tick 语义）；持焦点 + 可见半拍才置 `Some`（越界夹 u16::MAX）；`Admission::TextInput` 与 Button 完全同款同句柄 rect+text 双推，**零契约新增命令** |
| wgpu | `CARET_ADVANCE_PX = 16.0`（等宽 16px 冻结设计语言，与字距同口径）：笔起点 + `caret * 16px` 画 1px 宽、字格高实心竖条，颜色取 `LabelState::color`；**后端零动画状态**——画不画全由提取层的 `caret` 位裁决 |

- focused 边框换 accent：Control 准入路径同步获得（取主题 accent
  槽，与 hover 同一槽位解析机制，不另开色源）——Button/Control/
  TextInput 三者口径一致
- 无基线漂移：缺省 `caret: None`、TextInput 无节点即零影响——
  wgpu 像素契约与视觉基线**未动**

## 5. editor_shell 真消费者（S12.0 §7 验收形态）

检查器节点重命名接 TextInput：选中即显示（视口锚定 616,76 /
148x20）、text 投影绑定选中节点名（**换选中才重绑**——编辑会话中
不改草稿）；`on_commit` 钩子经共享缓冲传出帧后，宿主落
`Inspector::modify_name` **一条 Modified 事务**（值未变则跳过），
输入框 text 投影跟着落账后的新名走。`NES_GAME_FRAMES` 冒烟通过。

评审修正（两处宿主/状态机口径收紧）：

- **护住检查器面板**：编辑器自身的点击选择路径原本把"没压到
  Sprite2D 的按压"一律当空白（框选 + 清空选中）——压在改名输入框
  上会连带清掉选中，输入框同帧被投影隐藏，UiVm 命中永远够不着它
  （点击夺焦路径失效，改名只剩 Tab 轮转可达）。修法：宿主按压沿
  先把鼠标按 `mouse_view_scale`（新升公开的运行时折算面）折到视图
  空间，落输入框矩形内即跳过清选/框选，把交互让给 UiVm 夺焦。
- **编辑会话中不换绑**：换选中触发的失焦提交要落到**开会话时**
  绑定的节点头上（提交回调读的是绑定槽，此刻换绑会把旧草稿安到
  新选中节点上）——会话持焦期间绑定冻结，失焦落账后下一帧再重绑。

再修正（S12-2 收尾补刀）：上条"护住检查器面板"落地后实测**点击
仍不夺焦**——盾被挂在 `snap.is_down("left")` 上，而 `is_down` 是
**键探针**（`Key::from_name` 名字空间），键名表里根本没有 `"left"`
（那是 `MouseButton` 名），故恒 false，宿主整段点击路径（盾 + 点选
+ 框选）实为死代码，"空白点击清选中"的击穿链原样健在。根修在宿主
接线层：`InputSnapshot` 新增 `button_down`（读 `buttons_held` 鼠标
按钮表，与键探针分立），editor_shell 改读 `button_down("left")`，
盾恢复生效后 UiVm 按下沿夺焦路径可达。焦点赋值边沿与清扫规则语义
零变更（UiVm 层 T-UI-05 / T-UI-R3 本就覆盖点击夺焦且为绿）；本轮补
四条回归：T-UI-09（单元层点击获焦 → focused/草稿/光标进 states_rc
共享面）、T-WID-06（真实 UiVm 驱动提取层 accent/草稿/光标换档）、
T-UI-R4（点击重命名运行时全链 + modify_name 事务落账）、T-In-R3
（键/鼠读面分立）。

## 6. 契约回归

| 套件 | 编号 | 断言 |
|---|---|---|
| nes-scene/tests/s12_ui.rs | T-UI-05..09 | 焦点路由（点击夺焦/Tab 场景序轮转/点空白失焦/**Button 失焦零提交**）；文本泵（ASCII 插入 + 非 ASCII 忽略 + Backspace）；草稿语义（Enter 提交整体值不直写属性表/Esc 回滚/失焦=提交）；文本瞬态不入指纹；点击获焦 → focused/草稿/光标经 `states_rc` 共享面换档（提取层数据源交接点） |
| nes-render-extract/tests/s12_button.rs | T-WID-05..06 | TextInput 摊平同句柄（三主题槽 + focused 换 accent + 草稿优先/无会话显已提交值）；光标 Some(字符下标) 与 30 帧奇偶节拍（第 31 帧隐半拍不画）；点击夺焦进提取层（真实 UiVm 驱动 → accent/草稿/caret，失焦回落） |
| nes-runtime/tests/criterion_ui_interaction.rs | T-UI-R3..R4 | **全链真实运行时路径**：`inject_input` Char（真实 UTF-16 消息序）→ `SnapshotView::text` 读面 → 草稿 → Enter 提交经 `on_commit` 回调一次；属性表零直写；Enter 抬起不重复提交；鼠标点击重命名全链（MouseMove+按下/抬键 → 夺焦 → 字符落草稿 → Enter 进 rename_sink → `modify_name` 一条 Modified 事务 + undo 还原） |
| nes-runtime/tests/criterion_input.rs | T-In-R3 | 键/鼠读面分立：`button_down("left")` 按住为真、键探针 `is_down("left")` 恒 false、未知名不猜、抬起归假（S12-2 记注回归） |
| 六 crate 全量 | — | **505 全绿**（asset 34 / scene 223 / render-api 44 / render-extract 48 / render-wgpu 89 / runtime 67，本轮 +4：T-UI-09/T-WID-06/T-UI-R4/T-In-R3）、clippy 0（`--all-targets`）、依赖方向守卫 11/11 |

## 7. 跳过与后置

- **左右光标移动**：P0 可选缺口，未实现（ui.rs 泵注释 + T-UI-06 留
  痕）；Home/End/选中同理后置
- **非 ASCII 输入**：S12.0 §3.4 IME 边界——不做组合窗，WM_CHAR 直进
  只收 ASCII 可打印；中文输入与真字体（多字号/中文图集）同里程碑
  后置
- **placeholder**：P0 仅存储不渲染；**Enter 激活 Button**（焦点在
  Button 上回车）：焦点槽已占、激活留待后续里程碑
- **Tab 双发语义**：editor_shell 宿主侧 Tab 沿仍同时驱动 Sprite 选中
  轮转（鼠标点选之前的备用路径），与 UiVm 的 Tab 焦点轮转同帧双发——
  重命名会话中按 Tab 会既换焦点又换选中（有"会话持焦绑定冻结"兜底，
  提交不落错账，仅交互语义含混）；**S12-3 收口**：焦点会话期间宿主
  侧 Tab 让路给 UiVm
- 契约回归：无命令流/指纹/视觉基线变更——本轮全 additive，零重录

## 8. 后续（S12-3 起按 S12.0 §7 推进）

- S12-3：E-2 裁剪（SetClip + scissor）+ ScrollView + ListView + Tabs
  （层级树换 ListView 真消费者）；editor_shell 宿主 Tab 双发收口（§7
  记注：焦点会话期间宿主侧让路给 UiVm）
- S12-4：F-4 编辑器集成（文件选择器 + 脚本面板 + editor_shell 整体
  换装）
- 文本泵补课：左右光标移动 / Home/End / Button 回车激活

## 9. 里程碑记注（M4 口径）

- 渲染契约扩展（`LabelState::caret`）：additive——缺省 `None` 既有
  命令流语义与观感逐位不变
- 提取层新增 TextInput 摊平推送段 + 30 帧闪隐节拍判定（动画只在
  这一个判定点，渲染侧零动画状态）；`themed_control` 焦点换档并入
  既有重算路径（"推送=重算"契约延续）
- `InputView::text()` 默认方法：既有实现者零破坏（S7.2/S8.2b 桥不
  动，需要文本输入的宿主按需覆写）
