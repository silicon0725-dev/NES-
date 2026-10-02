# NES 2.0 · S12.3 裁剪与滚动 v1

> 交付日期：2026-10-03　｜　状态：**E-2 裁剪契约 + 滚轮输入 + 最小窗 + ScrollView/ListView/Tabs 三控件 + editor_shell 层级树换 ListView 真消费者**
> 前置：S12.0 组件库设计冻结（§4 E-2 前置、§5 P1 清单、§7 里程碑拆分）；S12.1 颜色管线/UiVm/Button；S12.2 TextInput/焦点路由。

---

## 0. 一句话结论

S12-3 全部落地：**E-2 裁剪契约反转裁决**——`SetClip { rect }` 是**渲染物属性**而非流式栈（对 S12.0 E-2 行"栈式，帧内生效"字样的显式修订：属性流的全量快照/跨帧幂等不变式优先），wgpu 侧 scissor 按连续相同 clip 值分段执行，缺省路径逐位不变；**滚轮输入**（WM_MOUSEWHEEL → 格归一一次性增量，+y=向上）与**最小窗钳制**（WM_GETMINMAXINFO，客户区 384x240，按窗口登记——小窗测试替身不受扰）；**三个新控件节点** ScrollView / ListView / Tabs（schema 封闭属性、UiVm `scrolls` 瞬态、ListView 行点击 `on_row_activate` 回调、滚动坐标约定单处实现）；**SetList 摊平**（单渲染物三件套 SetList+SetRect+SetClip，输出序冻结）；**editor_shell 层级树换 ListView**：行文本/选中行每帧投影、行点击经共享缓冲帧后落 Selection、面板点击护盾扩展——UI 仍是投影，零自有语义。六 crate **524 测试全绿**（较 S12-2 +19）、clippy 0、守卫 11/11；`NES_GAME_FRAMES=120` 冒烟干净退出。

---

## 1. E-2 裁剪（nes-render-api + nes-render-wgpu）

**D1 裁决：裁剪是按渲染物的属性（`SetClip`），不是流式栈**——这是对 S12.0
§4 渲染契约扩展表 E-2 行"`SetClip { rect }` 命令（栈式，帧内生效）"的**显式
修订**。理由：渲染契约的不变式是"属性流 = 全量快照、按序重放、跨帧幂等"
（漏推一帧不漂移），栈式流序状态一旦漏推一帧裁剪就整体错位，两者不可调和。
消费器侧的对应口径：**裁剪表是帧本地的**——每帧的裁剪状态只来自本帧命令流，
"未被本帧重申的裁剪 = 本帧不裁"自然成立；提取层每帧全量推送（无脏标记）。

| 层 | 变更 |
|---|---|
| render-api | `RenderCommand::SetClip { handle, rect: Option<Rect> }`：`Some(r)` 的 `r` 是**已解析的视口空间**矩形（后端按目标尺寸折算成帧缓冲像素 scissor，半开区间）；`None` = 清除；条目销毁（`DestroyItem`）裁剪随条目消亡；命令**恒在对应条目 `SetRect` 之后**（D1 推送序）。`RenderServer` trait 增 `set_clip`；`NullRenderServer` 同构簿记（帧本地裁剪表）。未知/空句柄按契约 I1 静默忽略 |
| extract | 嵌套裁剪**由提取层沿祖先链求交集**后以单条 `SetClip` 下发（本层不做栈语义）；推送见 §4 自动裁剪 |
| wgpu | `SpriteInstance` 增 `loc6 = [x,y,w,h]` 视口空间裁剪矩形（着色器**不读**，实例布局自洽）；缺省哨兵 `NO_CLIP = [0,0,0,0]` = 无裁剪。绘制按**连续相同 clip 值分段**，每段先显式 `SetScissorRect` 再 `draw`（哨兵段也显式设回全目标——状态不跨段继承，无"上段泄漏到下段"）。`clip_to_scissor`：视口空间按 `target_size / viewport` 比例折算、`floor` 取整、与目标边界求交；**交集为空返回 `None`，该段整段跳过**。FFI 增 `wgpuRenderPassEncoderSetScissorRect`（符号存在性已对本机 wgpu_native.dll 导出表实测） |

- **缺省路径逐位不变**（T-Clip-03）：无 `SetClip` 的帧全部实例带哨兵 = 单段
  全目标 scissor，输出与裁剪机制加入之前逐位相同——零基线重录。
- 消费语义：裁剪对**整个渲染物**生效（Control 一物多实例：矩形、字形、
  选中条、滚动条全部同受 scissor 约束）。

## 2. 滚轮输入 + 最小窗（nes-render-api + nes-render-wgpu）

**滚轮**（`InputEvent::Wheel { x, y }`）：

- **只垂直**：WM_MOUSEWHEEL 的 wparam 高 16 位原始增量按 `WHEEL_DELTA=120`
  归一成"格"（+1 格 = +y = 向上）；水平滚轮源不存在，`x` 恒 0（字段保留）。
- **量语义 + 一次性**：同帧多事件**相加**（两格就是两格——与鼠标位置"最新
  即真相"不同），快照 `wheel: Vec2` 取走即清（同 `text`/`resized`）。
- `InputView` 增默认方法 `wheel() -> (f32, f32)`（缺省 `(0.0, 0.0)`——既有
  实现者零破坏，`text()` 同款先例）；运行时 `SnapshotView` 透传快照值。
- trace 记法：`wheel X Y`（如 `5 wheel 0 1.5`），与键/鼠/字符同一份
  `parse_trace` 文本格式。

**最小窗**（WM_GETMINMAXINFO）：

- 用户把窗口拖窄会裁掉文本面板的字——`wnd_proc` 直答 `WM_GETMINMAXINFO`
  覆写 `pt_min_track_size` = 客户区 **384x240** 经 `AdjustWindowRect
  (WS_OVERLAPPEDWINDOW)` 外扩的整窗尺寸（与 `Window::open` 同一换算，不硬
  编码边框）；其余 MINMAXINFO 字段原样放行，按文档返回 0（不转发
  DefWindowProcW——实测它不回填结构体）。
- **钳制按窗口登记**：只对开窗时客户区不小于 384x240 的窗口生效（进程级
  句柄表 `CLAMPED_WINDOWS`，析构先除名再销毁——防句柄复用串钳制）。
  **小窗测试替身不受影响的原因**：实证 Windows 在创建/排列阶段就经
  `WM_WINDOWPOSCHANGING` 查询 `WM_GETMINMAXINFO`，小窗一旦挂钳制会被直接
  顶到 384x240，破坏 T-Surf-01"客户区精确等于请求尺寸"契约（256x128 开窗
  实测变 384x240）——故未登记窗口沿系统默认路径。窄窗溢出的**根修**另由
  §4 自动裁剪承担（控件自身矩形恒裁剪），钳制只是防用户拖出不可用窗口。

## 3. 三控件：ScrollView / ListView / Tabs（nes-scene）

`NodeKindTag`/`NodeKind` 各进三员（base=Control，ALL 序 10..12），继承
anchor/offset/size；schema 封闭属性（RON 往返）：

| 控件 | 属性 | 类型 | 缺省 | 语义 |
|---|---|---|---|---|
| ScrollView | `step` | I64 | 48 | 每格滚轮滚动的像素数 |
| ListView | `rows` | Str | `""` | 行文本，`'\n'` 分隔（空串 = 无行） |
| | `row_h` | I64 | 18 | 行高（像素）；滚轮步进同此值 |
| | `selected` | I64 | -1 | 选中行下标（-1 = 无）；落账由宿主经 `on_row_activate` 自做 |
| | `text_slot` / `sel_fill_slot` | Str | `text` / `selected` | 行文字 / 选中填充条槽位名 |
| Tabs | `tabs` | Str | `""` | 页签文本，`'\n'` 分隔 |
| | `tab_w` | I64 | 64 | 单个页签宽度（像素） |
| | `active` | I64 | -1 | 活动页签下标（-1 = 无）；由宿主落账 |
| | `text_slot` / `sel_fill_slot` | Str | `text` / `selected` | 同上 |

- **UiVm `scrolls` 瞬态**（`UiStates` 第三张子表）：ScrollView/ListView/Tabs
  的垂直滚动偏移住瞬态表，**不进属性表**（写属性 = 弄脏文档 + 进事务 + 进
  语义指纹，三重错误）；与 widgets/texts 同款生灭纪律——死节点清扫、无输入
  视图全清、不序列化、不进语义指纹（T-SC-03 钉死）。滚轮路由（`wheel`
  一次性字段当帧有效）：取前序序**最后命中**的滚动控件（与 `hit` 同一条
  last-write-wins 仲裁），`scroll = clamp(scroll − wheel.y × step, 0,
  scroll_max)`——向上滚（+y）内容回落向顶；值无变化不落表（不留 0.0 壳）。
  步进取向：ScrollView 用 `step`、ListView 用 `row_h`（行进）、Tabs 缺省
  行高 18。scroll_max：ListView = 行数×row_h+8 内衬 − 视口高；Tabs = 页签
  数×18+8 − 视口高；ScrollView = 可见后代控件底缘最大值 − 自身底（嵌套
  滚动容器以自身矩形计入、不再下探）。
- **行点击回调**：ListView 复用 Button 同款按下/抬键边沿机，**抬键仍命中**
  才结算——点击点换算回内容空间反解行下标（顶内衬区与超行数下标不回调），
  经 `on_row_activate(NodeId, u16)` 只报 (列表节点, 行下标)；**UiVm 零写权
  延续**——写 `selected` 属性等落账由宿主做。
- **坐标约定（单处实现）**：祖先 ScrollView 把内容**向上**平移滚动偏移渲染，
  命中矩形随之**减去**同一偏移和，再做祖先 ScrollView 视口矩形**交集**裁剪
  （滚出容器的内容不可命中）——`nes_scene::ui::scroll_context_of` /
  `contextual_rect` 是全 crate 唯一实现点，命中测算、scroll_max 测算与提取
  层烘焙三处共用。无滚动祖先 = `(0.0, None)`，既有 Button/TextInput 命中
  零变化（additive 保证）。

## 4. SetList 摊平 + 滚动烘焙 + 自动裁剪（nes-render-extract）

**命令与状态**：`RenderCommand::SetList { handle, rows: ListState }`——
ListView（垂直）/ Tabs（水平）共用一个命令与一份 `ListState`（`axis` 承载
方向，差异只在后端展开分臂）。**输出序冻结**：同一渲染物的属性流序为
`SetText → SetList → SetRect → SetClip`，null 与 wgpu 两处 submit 严格同序
（推送侧调用次序不影响命令流）。`ListState`：行文本（`'\n'` 分隔，与
`LabelState` 同款 `Arc<str>` 共享）、`font=NIL` = 默认字体、字号 16、
`row_h`/`tab_w`、`scroll`（提取层从 UiVm 瞬态表折进——后端零交互状态）、
`selected: Option<u16>`、`text_color` / `sel_fill` 双主题槽解析。

**滚动烘焙**（提取层每帧）：前序遍历每节点取一次滚动上下文——四类承载
ControlState 的准入（Control/Button/TextInput/List）把祖先滚动偏移和折进
自身四边 offset（`offset_top/bottom −= scroll_sum`，ScrollView 自身矩形
不动）；偏移和为 0 不写（加性缺省，既有输出逐位不变）。

**滚动条**：仅 ScrollView 自身配（`ControlState.scroll_bar: Option<ScrollBar>`
——additive 缺省 `None`，既有路径逐位不变）；`frac = 视口高/内容总高`、
`pos = scroll/scroll_max` 由提取层算好（装得下 = 满长滑块归零位，除零防），
滑块色取边框槽；ListView/Tabs 行进滚动不配条（任务冻结口径）。

**自动裁剪（D6）**：

- 有滚动祖先（**结构性**判定，与当前偏移值无关）：clip = 自身矩形 ∩ 祖先
  交集；**交空 = 零矩形仍推**（见下——零矩形 = 全裁）；
- 无滚动祖先：**恒裁剪类型**（Button/TextInput/List 四类 + ScrollView 自身）
  仍推自身矩形（防窄窗文本溢出的根修）；**裸 Control 不推**——既有输出
  逐位不变。

**零矩形全裁与哨兵的区分（裁决）**：实例的 clip 槽缺省哨兵
`NO_CLIP = [0,0,0,0]` 语义是"**无裁剪**"（= 全目标 scissor，缺省路径与
裁剪机制之前逐位相同）；而提取层**显式推下的零尺寸矩形**语义是"**全裁**"
——`clip_to_scissor` 对真实零矩形/空交集返回 `None`，该段实例整条不发
（T-Clip-07 钉死）。同形不同义：一个在实例缺省槽（没裁），一个在命令流
（裁没了）。

**选中行字形色裁决**：选中行的观感差异**只来自 sel_fill 填充条**，字形
tint **恒取 text_color**——若字形也用 sel_fill 会与填充条同色隐形。行
展开算式：垂直轴行 i 笔起点 y = 矩形顶 + 4 + i×row_h − scroll；水平轴
页签 i 笔起点 x = 矩形顶 + 4 + i×tab_w，且按 `(tab_w − 8) / 16` 截断
字符数（16px 等宽冻结口径）；垂直轴不截断（溢出交 clip）。

## 5. editor_shell 真消费者（S12.0 §7 验收形态）

层级树从"每帧 walk 投影成多行文本的 Label"换成 **ListView 节点**
（视口锚定：anchor (0,0) / offset (8,40) / size 164x360，`row_h=18`，
`text_slot` 缺省）——S12.0 §7"S12-3 层级树换 ListView（真消费者）"验收
兑现：

- **每帧投影**：树 walk 生成的层级文本行（ASCII 空格缩进 + 选中标记
  `* `，命名/图标约定不变）写入 `rows` 属性；**行→节点映射
  （`Vec<Uid>`）平行重建**，walk 顺序即行序（存活节点必有 uid，行与映射
  严格同长同序）。原 Label 首行的 "HIERARCHY" 标题行随之移除——rows
  必须与节点 1:1，不设幻影标题行；行文本不带尾随 `'\n'`（场景层
  rows_count 会把尾随空行计成幻影行，行点击行数上限随之失真）。
- **选中行投影**：`selected` 属性 = 主选中 uid 在映射表中的行下标
  （找不到 = -1）；与 sel_box/z_index 同款纪律——Selection 是唯一语义
  来源，宿主每帧直写属性。
- **行点击落账**：注册 `ui_vm.on_row_activate`——回调 (列表节点, 行下标)
  → 查共享映射 → 推入共享缓冲（**钩子内只记**，不改树结构，UiVm 零写权
  延续）；帧后 drain 逐个 `Selection::select`（与视口点选同款单选替换
  语义；选择是会话态，不进事务不落盘）。下一帧树投影与选中行高亮随之
  跟上。
- **面板点击护盾扩展**：S12-2 的 `press_in_name_input` 泛化为
  `press_in_control`（同一 UiVm 命中口径：anchor×viewport+offset+size、
  不可见不参与），ListView 矩形与改名输入框一起加入空白按压屏蔽——压在
  列表上的点击不清选中、不启动框选，交互让给 UiVm 行点击路径（否则宿主
  "空白 = sel.clear() + 框选"会与行点击同帧打架）。滚轮在列表上滚动由
  UiVm 自动路由，宿主零接线。
- 检查器/状态栏的自由文本 Label 不动；改名 TextInput 全链（S12-2）照旧。
- **窄窗核对**（代码逻辑口径）：editor_shell 开窗 768x432 ≥ 最小窗钳制
  基准 384x240，`WM_GETMINMAXINFO` 不可能把开窗顶变形；拖窄到下限后面板
  文字溢出由 §4 恒裁剪兜底（最小窗钳制本身由 T-In-03 覆盖）。

## 6. 契约回归与守卫计数

| 套件 | 编号 | 断言 |
|---|---|---|
| nes-render-api/tests/input_contract.rs | T-In-C5 | 滚轮折叠/清零/trace 解析：同帧多 Wheel 相加、`frame()` 取走即清、`wheel X Y` 记法往返、+y=向上 |
| nes-scene/tests/s12_scroll.rs | T-SC-01..04 | 滚轮路由（只命中滚动控件才改/夹紧 [0,max]/算式/last-write-wins）；行点击（载荷 (节点,行)、抬键仍命中边沿、顶内衬与超行不回调）；scrolls 生灭纪律（死节点清扫/无输入全清/不入指纹）；三新 kind schema 封闭属性/缺省值/ALL 序/RON 往返 |
| nes-render-extract/tests/s12_scroll.rs | T-SCL-01..06 | ListView 摊平三件套（SetList+SetRect+SetClip 载荷字段）；滚动烘焙（后代 offset 折 −scroll、自身不动）；scroll_bar frac/pos 算式；嵌套 ScrollView 三层交集/交空零矩形；基线（无滚动祖先裁剪形状）；Tabs 水平摊平 |
| nes-render-wgpu/tests/criterion_clip_contract.rs | T-Clip-01..07 | 命令流语义（推送序/None 清除/未知句柄忽略）；像素级 scissor 半开区间；缺省路径逐位不变；越界求交与空段跳过；分段各自 scissor；ListView 滚动+滚动条像素；零矩形全裁整段不发 |
| nes-render-wgpu/tests/criterion_window_input.rs | T-In-03 | WM_MOUSEWHEEL 真实消息入队按 WHEEL_DELTA 归一（+y=向上）；最小窗：登记窗口 SendMessage GETMINMAXINFO 得 384x240 外扩整窗、小窗替身不登记客户区精确如请求 |
| 六 crate 全量 | — | **524 全绿**（asset 34 / scene 227 / render-api 45 / render-extract 54 / render-wgpu 97 / runtime 67；较 S12-2 的 505 净增 19，全部由 S12-3 任务 1-4 落地：T-In-C5、T-SC-01..04、T-SCL-01..06、T-Clip-01..07+T-In-03） |

- **门禁实录（任务 5 收口轮）**：六 crate `cargo test --release` 0 failed；
  `cargo clippy --release --all-targets` ×6 全 0 警告；仓库根
  `python check_dependency_direction.py` **11/11**；
  `NES_GAME_FRAMES=120 cargo run --release --example editor_shell`
  干净退出、driver_errors=0（本任务宿主接线零新增测试——真消费验收以
  冒烟 + 既有契约回归共同覆盖）。
- 渲染契约 additive 复核：`SetList`/`SetClip` 为新增命令、
  `ControlState.scroll_bar`/`SpriteInstance.clip` 缺省（`None`/哨兵）下
  既有命令流语义与观感逐位不变——零基线重录。

## 7. 遗留与后置

- **表面 resize 重配仍属后续**（S6 文档遗留原样）：窗口缩放靠 NDC 拉伸
  铺满，裁剪矩形是视口空间坐标、随之等比折算——观感正确但像素密度不变，
  真正的 surface 重配置（swapchain resize）另立里程碑。
- **Tabs 截断按 16px 口径**：`(tab_w − 8) / 16` 字符硬截断（等宽冻结设计
  语言），无省略号/渐隐；多字号随真字体里程碑解冻。
- **ListView 滚动无滚条**：行进滚轮（step=row_h）+ 溢出裁剪，不画滑块
  （滚动条是 ScrollView 专属，任务冻结口径）；需要条的长列表后续按需补。
- ListView 行点击 P0 只有单选替换（回调载荷无修饰键）；Shift 多选、
  ScrollView 滚动条拖拽/可点、Button 回车激活等交互词汇留后续里程碑。
- editor_shell 层级树行文本沿用 `* ` 前缀标记选中 + sel_fill 条双重表达
  （文本约定与主题着色并存）——视觉去重留 S12-4 整体换装时定夺。
