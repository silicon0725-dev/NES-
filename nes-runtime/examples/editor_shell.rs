//! S9-3b **Editor Shell**：建立在已验证状态模型上的编辑器 UI。
//!
//! 架构（评审冻结）：**UI 只消费状态模型，不成为语义来源** ——
//! Hierarchy View 是 SceneTree 的投影（S12-3 起 ListView 行文本 +
//! selected 行高亮），Inspector 是选择节点数据的投影，Viewport 高亮
//! 是 Selection 的投影。一切修改经 Inspector/Hierarchy 适配器 →
//! TransactionLog。ui 零自有状态（除面板滚动等会话态）。
//!
//! 布局（S12-4 自适应口径）：**宿主每帧投影** —— 视口 = 窗口真实
//! 客户区（最大化/拖拽当帧跟上），面板**恒定宽**不随窗口拉伸：
//! - 左侧 180px：Hierarchy 面板（树投影，ListView 自带 panel 填充）
//! - 右侧 190px：Inspector 面板（panel 槽铺底 + 标题/信息/改名输入框）
//! - 中间：Viewport（世界吃剩余区域，相机置中 = (cw/2, ch/2)）
//! - 底部：状态栏（undo/redo 可用性、操作提示）
//!
//! 操作：Tab 循环选择；方向键移动选中；Delete 删除子树；
//! Ctrl+Z undo；Ctrl+Y redo。
//!
//! S12-5（Godot 观感起步）：① 拖拽同帧冲洗 —— 投影块读 world() 前先
//! `refresh_transforms`（S12-4 选中框错位的根修：框/命中不再吃上一帧
//! 缓存）；② 视口网格（"grid" 容器下的 1px 条带池，z=-100 垫底，
//! 层级树跳过）；③ 选中框 accent 槽 + 2px 线宽 + z=100 垫顶；
//! ④ 面板 Godot 命名（Scene / Inspector）；⑤ Ctrl 拖拽 8px 吸附。
//!
//! S12-6（Godot 对齐）：① 改名输入框纳入每帧布局投影 —— offset 不再
//! 是装配期写死的旧值，窗口一变就跟手；② Inspector 改 Godot 属性行
//! 模式：一行一属性短标签（"x 312"），16px 等宽 advance=16 下绝不超
//! 面板宽；③ 底部 Output dock（复用 ListView 显编辑器日志，新行在下）
//! + 2D 标尺（顶横/左竖各 16px，64px 刻度 128px 数字，对齐世界原点）
//!
//! 三者全部进布局投影块，层级树 walk 跳过，命中护盾覆盖。
//!
//! S12-7（F-4 从文件挂载脚本 + Godot 化延续）：
//! ① Inspector 分区化 —— Transform（name/x/y/z + 改名框）/ Script
//!    （挂载流）两组，组标题行（text_dim 色 + "+"/"-" 前缀）点击或
//!    F7 循环切换折叠；折叠 = 该组行不投影，后续行上移（行序即布局）。
//! ② F-4 挂载流（键盘两步）：F8 扫 `Scripts/*.nes`（原 F5，S12-9 起
//!    让位给 PLAY；资产根相对路径，另每 60 帧自动刷）→ F6 轮换候选
//!    → Enter 挂载 → U 卸载 → E 切
//!    enabled。**落账走 Inspector 事务**：选中不是 Script 节点时挂载 =
//!    同一事务内 Created（Script 子节点，名字 = 脚本基名）+ Modified
//!    （registry_key）—— 引擎口径脚本住 Script 节点（attach 只认
//!    Script 类型），挂载必须在正确的数据形状上；undo/redo 一步整回。
//!    **边界（文档裁决）**：挂载只落数据（registry_key 属性），运行时
//!    装载是游戏运行路径的事（S6 既有 attach/attach_all_with_sources
//!    —— 宿主按同键注册或经装载器闭包读文件）。
//! ③ 视口工具栏：标尺上方 24px 工具带（panel 铺底 + border 分隔线），
//!    SEL/SNAP/GRID 三个 Button —— 选择总开关 / 恒吸附（Ctrl 反转）/
//!    网格显隐，开关态是编辑器会话态（不进树），文本后缀 * = ON。
//! 改名框持焦时 Enter/字母属于输入框（焦点门，键盘挂载流让路）；
//! F 键不产生文本、且 Key 契约未列举 F 键 —— 平台层保留原码为
//! `Key::Other(vk)`，按码比对（见 VK_F5.. 注）。
//!
//! S12-8（文件系统 dock，Godot 左下 res:// 面板）：左栏从单段 Scene
//! 扩成 Godot 式上下两段 —— 上 **Scene**（既有层级树投影）+ 4px
//! border 分隔条 + 下 **FileSystem**（"res:/" 标题 + 资产树 ListView，
//! 复用控件）。数据面 = 通用递归资产扫描（scan_scripts 先例的推广，
//! 每 60 帧 + F5；目录优先字典序、白名单后缀、隐藏项跳过、两层封顶）。
//! 交互（Godot 惯例）：行单击选中（selected 行高亮投影）；双击分派
//! —— .ron 场景 = Output 提示（场景打开归 play-in-editor 里程碑，P0
//! 不实现）、.nes = 直接挂载（与 Inspector Enter 同一 mount_script
//! 事务）、目录/其余后缀提示。UiVm 行回调只有单击沿，双击由宿主
//! 会话态合成（同行 <30 帧两次点击）。单击 .nes 顺手指为 F6 候选
//! 起点 —— FileSystem 与 Inspector 两处入口同一挂载流。F9 切换两段
//! 分割档（焦点段占大头；P0 不做拖拽，比例是会话态不进树）。
//!
//! S12-9（**play-in-editor**，Godot F5 的直感）：工具栏加 PLAY/STOP/
//! RESET 三按钮（F5 = PLAY / 重启，Shift+F5 = STOP，Godot 同款；
//! 手动资产扫描让位给 F8 —— F 键不产文本，按码比对的既有路径照抄）。
//! 核心裁决：**运行态 = 原地换观察者**（引擎纪律"CLI 不是第二个运行
//! 时"同款）—— PLAY 时建 `ScriptVm`：① 全树快照（SubtreeSnapshot，
//! uid 锚定，见 RESET）；② **宿主按同键注册**（S12-7 挂载只落
//! registry_key 数据，这里把键 = 资产根相对路径的 .nes 文本读入、
//! 编译、按同键 register —— 编译失败报行号错误、该节点跳过）；③
//! attach_all_with_sources 装载全部挂载脚本（issues 通道逐行上报，
//! **不回编辑态** —— Godot 行为近似：带病也能跑）；④
//! mount_input_view 接输入读面。此后每帧 `frame_windowed_with` 的
//! 观察者参数从 NoObserver 换成该 VM —— ScriptVm 实现
//! SceneObserver，process 脚本（every）由此逐帧驱动；信号脚本经
//! attach 时装进树的处理器表照常交付。渲染/UiVm/布局投影**零分支
//! 照常**。STOP：playing=false、drop VM —— 脚本停、事务历史保留、
//! **不自动还原**（Godot 语义：运行期改动就是真改）。
//!
//! RESET（从快照还原）的方案裁决：整树重载（parse RON ->
//! instantiate_scene）会换掉全部 NodeId —— 壳层持有的工具栏/面板/
//! 输入框手柄与 UiVm 行映射全部打散，不可用。改走**数据面还原**：
//! 快照 = PLAY 时全树前序逐节点的 SubtreeSnapshot（uid 锚定、含属性
//! 表全集）；RESET 时按 uid 寻回节点、摘掉快照没有的键（运行期新增
//! —— set_prop 只认 schema 键且出生即满配，新增键只可能来自
//! set_prop_raw 前向兼容通道）、`SubtreeSnapshot::apply_data` 整体
//! 写回（S12-9 起公开 —— 与事务 Modified 方向同一条代码）。uid 与
//! NodeId 双双不动：层级树行映射、选择、输入框绑定、UiVm 状态（其
//! update 本就有死节点清扫，见 ui.rs）全部无感。正确性前提（文档写
//! 明）：**运行期树结构不变** —— 脚本没有结构指令（process 只写自
//! 身、信号只写属性/变换），编辑交互在运行态全部禁用（下方护盾），
//! 故快照 uid 在 RESET 时必然尽数存活。
//!
//! S12-11（**编辑器壳层切换真字体**）：渲染器第 2 期（动态字形图集 +
//! `set_ttf_default`）落地后，`font == NIL` 的文本自动走真字体比例排
//! 版 —— 壳层要做的是供给与口径收口：
//! ① **启动字体探测链**（[`FONT_CANDIDATES`]：msyh.ttc → simhei.ttf →
//!    segoeui.ttf，第一个可读且可解析者装载；全部缺失 = 保持位图字体，
//!    Output 记一行、不 panic —— 优雅回退契约）；
//! ② **统一字号 14**：全部 Label（含标尺数字）经 schema 键 `font_size`
//!    写 14；输入框/工具栏按钮 schema 无该键，经 set_prop_raw 前向通
//!    道写 14（提取层 TextInput/Button 读 `font_size` 属性、缺省 16
//!    逐位不变）。行步进常量从 16 放宽到 [`INS_ROW_H`] 20（msyh 行高
//!    ≈1.32em，14px ≈ 18.5px —— 16px 步进下属性行会顶进改名框）；
//! ③ **IME 组合窗锚点真字宽累加**（第 1 期的 10px 平均步进退役）：
//!    壳层持同一份字体数据的 TtfFont 实例（重复解析一次可接受 —— 解
//!    析器纯 CPU 无共享状态，与消费器内实例不共享是刻意的，免去跨所
//!    有权借用的复杂度），caret_x = 输入框位 + 4px 内衬 + Σ advance
//!    （与渲染器 push_ttf_label 光标算式同源，见 [`ime_caret_offset`]）；
//! ④ 布局口径复核：等宽 16px 假设处逐处过一遍（见各常量注）—— 比例
//!    字体同 px 容字更多，溢出只会变少不会变多，截断预算按"位图回退
//!    模式仍安全"取界（真字体是可选增强，回退模式不许破相）。
//! ListView 行文本（层级树/Output dock/FileSystem）走 ListState 位图
//! 路径不受影响（16px 等宽，字数预算照旧）—— 列表行接真字体归后续
//! 里程碑（见 S12.11 文档 §5 遗留）。
//!
//! 运行态编辑禁用口径：gizmo/框选/点选/Tab 循环/方向键/Delete/
//! undo-redo/F6..F9/Enter/U/E/改名提交/行点击落账全部让路（各动作
//! 入口 `if !playing` 一层护盾）；相机置中与全部面板投影照常 ——
//! 编辑器快捷键（F5/F6/Del…）在运行态仍会进检测，但动作被禁用故
//! 无副作用；游戏键（WASD 等）经同一输入快照直达脚本 —— 编辑器与
//! 游戏共享同一快照源（S8.2b-3 既有口径）。
//!
//! S13 第 2 期（音频接入）：启动时声明一个演示声音资产（Audio/beep.wav
//! —— 440Hz 蜂鸣，代码生成与 bmp 同口径）并随 bind 装载；PLAY 会话时
//! 若场景有 Sound 资源则自动 `open_audio`（Output 记 "audio on"；失败
//! 报一行不中断 —— 带病也能跑的既有口径）。STOP **不关**音频（幂等
//! 无害：空混音器静音填充）。游戏脚本 `play "…"` 的 Cmd::PlaySound 由
//! 运行时在 tick 后转交混音器 —— play-in-editor 运行态由此出声。
//!
//! S14 第 1 期（媒体解码适配层接入）：① res:// 白名单扩到外部交付格式
//! （jpg/jpeg/webp/gif + flac/mp3/ogg/m4a —— 只是**列出**；装载/解码在
//! 场景声明它们之后走 nes-media 适配层，见 runtime 的 declare_image 与
//! Sound 装载回落序）；② **用户实测音乐接入**：装配时若用户音乐目录里
//! 有实测曲（FLAC/MP3，不在仓库 —— CI/他机安全跳过），经 nes-media
//! 解码成全量 PCM 宿主直注混音器（键 "music"/"music2"）。**数字键 0 =
//! 三态循环**：心似烟火(FLAC) -> Montagem Nada(MP3) -> 停 -> 回到第一首
//! （编辑态专属；改名框持焦让位输入）。整曲解码进内存（一首 4 分钟
//! 44.1kHz 立体声约 40-80MB PCM，P0 可接受）—— **流式是后续**（见 S14
//! 文档 §5 遗留）。Output 记 `music loaded (flac, 96000Hz stereo, N sec)`
//! 一行/曲，三态切换各记一行（ASCII 标签 —— 文件名是用户数据，不进
//! 日志与断言）。
//!
//! S15（视频资产面接入）：① res:// 白名单再扩 amv/avi（只是**列出**；
//! 装载/解析在场景声明 Video 资源之后走 runtime 的 declare_video 链）；
//! ② **演示视频接入**：装配时若用户视频目录里有实测 AMV（不在仓库
//! —— CI/他机安全跳过），复制进 assets/Media/ 并 declare_video 随
//! bind 解析（首帧即上 GPU）。PLAY 会话自动起播（Output 记 "video on"，
//! 音频钟主控严格同步 —— S15.1）、STOP 停播（记 "video stopped"，
//! stop_key 点名停音轨）。
//!
//! S18.1（**编辑器时间轴 dock**）：Output dock 上方的全宽面板（高 110，
//! 九宫格皮肤 + "TIMELINE" 标题行 —— 其余 dock 布局高度相应让位：左右
//! 面板与可编辑区底缘统一上移）。三块内容：
//! ① **补间行区**（ListView 复用）：选中节点的活动补间投影 —— 每帧从
//!    `SceneTree::tween_rows(primary)` 现算（Selection 主选中驱动，照
//!    Inspector 同款纪律），行格式冻结 ASCII：
//!    `POS  43%  ease_out  yoyo  (812ms/1500ms)`；无补间 = 单行
//!    `(no tweens on selection)`、无选中 = `(no selection)`；
//! ② **进度条**：每行下沿 2px 细条（fill_slot selected 色，宽 = 行宽 ×
//!    progress）—— Control 池路线（TL_BARS=4，照网格条带池先例，选池
//!    而非文本进度条：与九宫格面板观感同语言、行宽自适应免截断）；
//! ③ **创建控制行**：`NEW: [POS][SCALE][ALPHA]  to=(x,y)  ms=500
//!    [linear][once]  [APPLY]` —— 通道按钮三枚（* 后缀 = 选中通道）+
//!    目标数值 TextInput×2（pos 用 x/y；scale 用 sx/sy；alpha 单值复用
//!    x 框）+ 时长 TextInput + 缓动/模式循环按钮（照 SEL/SNAP 循环口径）
//!    + APPLY。
//!
//!    APPLY 语义 = **从当前值起算**：经树宿主 API
//!    `register_tween_channel` 落地（from = 登记处采样当前实际值，与脚本
//!    Cmd 同一条登记表路径）；输入非数值/无选中/拒收 → Output 报行不落地。
//!
//! 交互防护：时间轴全部控件进 hit 护盾数组（press_in_control 同款 ——
//! 压上不清选中不框选）；三个 TextInput 并入焦点门（持焦时键盘挂载流/
//! 音乐键让位输入）；"tldock" 容器进层级树 walk 过滤表。确定性边界：
//! 编辑器创建的补间与脚本 Cmd 同一登记表 = 编辑态会话态（不进 RON、
//! 编辑器不参与 headless 指纹 —— S18.1 文档 §3）。
//!
//! 运行：`cargo run --example editor_shell`

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use nes_asset::AssetKind;
use nes_audio::wav::write_wav;
use nes_render_api::input::{InputEvent, Key, MouseButton};
use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::{
    PROP_CONTROL_ANCHOR, PROP_CONTROL_OFFSET, PROP_CONTROL_SIZE, PROP_LABEL_TEXT, PROP_NS_B,
    PROP_NS_L, PROP_NS_R, PROP_NS_T, PROP_NS_TEX, PROP_TEXTURE,
};
use nes_render_wgpu::window::inject_input;
use nes_render_wgpu::ttf::TtfFont;
use nes_render_wgpu::{bmp, FontParams};
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::editor::{Hierarchy, Inspector, Selection};
use nes_scene::transaction::{SubtreeSnapshot, TransactionLog};
use nes_scene::{compile_script, NodeKind, NoObserver, ScriptVm, Transform2D, Value, Uid};

/// S16.7 九宫格开关属性名（`ns_modulate` / `ns_tiling` —— 与
/// nes-render-extract/src/extractor.rs L124/L126 的定义同源；提取层 crate
/// 根未再导出这两个常量，壳层前向通道按 set_prop_raw 口径直写同名字符
/// 串，不改 crate 面）。
const PROP_NS_MODULATE: &str = "ns_modulate";
const PROP_NS_TILING: &str = "ns_tiling";

/// S18 编辑器换肤：**EditorTheme —— 色板 / 间距栅格 / 行高 / 面板宽的统一
/// 出口**（DESIGN-NOTES §6.2，egui `Style`"视觉参数一处出"的最小对应物）。
///
/// 取舍（笔记 §6.1 结论）：色板**锚定 nes-scene 八槽位契约**，不扩槽位、
/// 不外置色板系统 —— 八个槽位值以 `0xRRGGBBAA` 打包常量列全（P0 取值 =
/// `ThemeColors::DEFAULT_DARK` 同值，观感零漂移），装配时写入一个 `Theme`
/// 节点（"主题即场景节点"，nes-scene/ui.rs 既有机制）：将来换肤 =
/// 改这一张表，提取/渲染层零改动。纹理皮肤（[`skin_rgba`]）承担可见观感。
///
/// 间距/行高/面板宽沿用 S12-4..S12-11 冻结值，从散落 const 收敛到本表；
/// 表内常量经下方 `use` 别名保持原引用面不变（零行为语义，纯出口收敛）。
mod editor_theme {
    // ---- 色板：八槽位（序与 nes-scene `THEME_SLOTS` 一致）----
    /// 窗口底。
    pub const SLOT_BG: i64 = 0x14_16_1A_FF;
    /// 面板。
    pub const SLOT_PANEL: i64 = 0x1E_22_28_FF;
    /// 边框。
    pub const SLOT_BORDER: i64 = 0x3A_40_48_FF;
    /// 正文。
    pub const SLOT_TEXT: i64 = 0xD8_DC_E2_FF;
    /// 次级文字。
    pub const SLOT_TEXT_DIM: i64 = 0x7A_82_8C_FF;
    /// 选中。
    pub const SLOT_SELECTED: i64 = 0x2E_4A_6B_FF;
    /// 强调。
    pub const SLOT_ACCENT: i64 = 0x4A_9E_FF_FF;
    /// 危险。
    pub const SLOT_DANGER: i64 = 0xD2_4B_4B_FF;

    /// 槽位名 + 色值（写 Theme 节点属性用，序同上）。
    pub const PALETTE: &[(&str, i64)] = &[
        ("bg", SLOT_BG),
        ("panel", SLOT_PANEL),
        ("border", SLOT_BORDER),
        ("text", SLOT_TEXT),
        ("text_dim", SLOT_TEXT_DIM),
        ("selected", SLOT_SELECTED),
        ("accent", SLOT_ACCENT),
        ("danger", SLOT_DANGER),
    ];

    // ---- 槽位名引用（fill_slot / border_slot / color_slot 属性值）----
    /// 面板填充槽。
    pub const SLOT_PANEL_NAME: &str = "panel";
    /// 边框槽。
    pub const SLOT_BORDER_NAME: &str = "border";
    /// 强调槽。
    pub const SLOT_ACCENT_NAME: &str = "accent";
    /// 次级文字槽。
    pub const SLOT_TEXT_DIM_NAME: &str = "text_dim";
    /// 选中槽（S18.1 时间轴进度条 —— 进度 = "走向哪里"的语义色）。
    pub const SLOT_SELECTED_NAME: &str = "selected";

    // ---- 间距栅格 ----
    /// 外边距（S12-4 冻结）。
    pub const MARGIN: f32 = 8.0;
    /// 小缝 / 内衬（工具栏 4px 缝的统一值）。
    pub const SPACE_S: f32 = 4.0;

    // ---- 面板宽（恒定宽，S12-4 口径：最大化只扩中间视口）----
    /// 左层级面板宽。
    pub const LEFT_PANEL_W: f32 = 180.0;
    /// 右检查器面板宽。
    pub const INSPECTOR_W: f32 = 190.0;
    /// 顶带高。
    pub const TOP_BAND: f32 = 40.0;
    /// 状态栏带高。
    pub const STATUS_BAND: f32 = 24.0;
    /// 底部 Output dock 高。
    pub const DOCK_H: f32 = 96.0;
    /// 底部时间轴 dock 高（S18.1，Output 上方 —— 标题 18 + 行区 58 + 缝 4
    /// + 创建控制行 20 + 上下留白 10）。
    pub const TIMELINE_H: f32 = 110.0;
    /// 时间轴标题行高（"TIMELINE"，与 dock 标题行同款）。
    pub const TL_TITLE_H: f32 = 18.0;
    /// 时间轴补间行区列表高（3 行 × 18 + 4px 顶内衬 —— ListView 行几何
    /// 与 Output dock 同一口径）。
    pub const TL_LIST_H: f32 = 58.0;
    /// 视口工具栏高。
    pub const TOOLBAR_H: f32 = 24.0;
    /// 工具栏按钮尺寸与步进。
    pub const TOOLBAR_BTN_W: f32 = 48.0;
    pub const TOOLBAR_BTN_H: f32 = 20.0;
    pub const TOOLBAR_BTN_STEP: f32 = 52.0;

    // ---- 行高三档（位图标题行 / 列表行 / 真字体行，S12-11 口径）----
    /// Inspector 真字体行步进。
    pub const INS_ROW_H: f32 = 20.0;
    /// dock / fs 列表行高（ListView row_h 同值）。
    pub const DOCK_ROW_H: f32 = 18.0;
    pub const FS_ROW_H: f32 = 18.0;
    /// dock 标题行高（"Output" 一行）。
    pub const DOCK_TITLE_H: f32 = 18.0;
    /// fs 分隔条厚度。
    pub const FS_SEP_H: f32 = 4.0;
    /// fs 标题行高。
    pub const FS_TITLE_H: f32 = 16.0;
    /// 面板内容水平内衬。
    pub const INSPECTOR_INSET: f32 = 6.0;
    /// 2D 标尺条带厚度。
    pub const RULER_W: f32 = 16.0;

    // ---- 字号 ----
    /// 编辑器 UI 统一字号（S12-11 壳层裁决：真字体 14px）。
    pub const UI_FONT_SIZE: i64 = 14;

    // ---- 九宫格皮肤纹理参数（DESIGN-NOTES §6.3）----
    /// 面板皮肤边距（48×48 纹理，8px 边带）。
    pub const SKIN_PANEL_MARGIN: i64 = 8;
    /// 按钮皮肤边距（48×20 纹理，4px 边带）。
    pub const SKIN_BTN_MARGIN: i64 = 4;
}

// 短名别名：既有引用面（MARGIN/LEFT_PANEL_W/...）逐字保留，定义单一出口。
use editor_theme::{
    DOCK_H, DOCK_ROW_H, DOCK_TITLE_H, FS_ROW_H, FS_SEP_H, FS_TITLE_H, INS_ROW_H, INSPECTOR_INSET,
    INSPECTOR_W, LEFT_PANEL_W, MARGIN, PALETTE, RULER_W, SKIN_BTN_MARGIN, SKIN_PANEL_MARGIN,
    SPACE_S, SLOT_ACCENT_NAME, SLOT_BORDER_NAME, SLOT_PANEL_NAME, SLOT_SELECTED_NAME,
    SLOT_TEXT_DIM_NAME, STATUS_BAND, TL_LIST_H, TL_TITLE_H, TIMELINE_H, TOOLBAR_BTN_H,
    TOOLBAR_BTN_STEP, TOOLBAR_BTN_W, TOOLBAR_H, TOP_BAND, UI_FONT_SIZE,
};

fn solid_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    [r, g, b, 255].repeat(16 * 16)
}

/// 九宫格皮肤纹理（S18，DESIGN-NOTES.md §6.3）：深色主题**成品绝对色**
/// —— 走 `ns_modulate=false` 路线（中性白 tint）。理由：乘法 tint 下
/// 纹理亮度 ≤ fill 槽色，"亮边框比面板底亮"在 `fill_slot="panel"`（深
/// 色）下乘不出来；S16.7 modulate 面向"灰阶纹理 × 面板色"的换色场景，
/// 与"带亮边的成品皮肤"不同路。主题槽继续管文本/选中/强调。
///
/// 布局：最外 1px 边框（border 槽同系）→ 顶缘 1px 高光（bevel-up，按钮
/// "浮起"直感；面板上退化为微妙亮线）→ `margin` px 边带内垂直微渐变
///（贴边 +8 → 内缘 -4）→ 中心平坦（同微噪点）。
///
/// 噪点 = 确定性整数哈希（无浮点 RNG —— 代码生成确定性口径，与
/// walk_sheet/beep 同一家法：缺了再写，仓库只背一份小文件）。
fn skin_rgba(w: u32, h: u32, margin: u32, base: [u8; 3], border: [u8; 3], top_hi: [u8; 3]) -> Vec<u8> {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            // 距最近外缘的像素数（0 = 最外一圈）。
            let edge = x.min(y).min(w - 1 - x).min(h - 1 - y);
            // 确定性噪点：±3 的整数抖动（逐像素稳定，"微妙噪点"）。
            let noise = ((x.wrapping_mul(73) ^ y.wrapping_mul(151)) % 7) as i32 - 3;
            let [r, g, b] = if edge == 0 {
                border
            } else if edge == 1 && y == 1 {
                // 顶缘高光行（边框内第一行）。
                top_hi
            } else {
                // 边带内垂直微渐变：band = 0（贴边）..=margin（中心区），
                // delta 从 +8 线性降到 -4（深色底上的克制落差）；中心区
                // band 封顶在 margin。
                let band = edge.min(margin);
                let delta = 8 - (band as i32 * 12) / margin.max(1) as i32;
                let mix = |c: u8| (c as i32 + delta + noise).clamp(0, 255) as u8;
                [mix(base[0]), mix(base[1]), mix(base[2])]
            };
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    rgba
}

/// 440Hz 蜂鸣样本（`duration_ms` 毫秒、单声道 16-bit；演示声音资产生成
/// 与 write_bmp_rgba 同一家法：代码生成、缺了再写、仓库只背一份小文件）。
fn beep_samples(duration_ms: u32, amplitude: i16) -> Vec<i16> {
    let rate = 22050u32;
    let frames = (u64::from(rate) * u64::from(duration_ms) / 1000) as usize;
    (0..frames)
        .map(|i| {
            // 整数近似的 440Hz 正弦相位（无浮点三角依赖；确定性生成）。
            let t = (i as i64 * 440) % rate as i64;
            ((t * amplitude as i64) / rate as i64) as i16
        })
        .collect()
}

// ---- S14：用户实测音乐（nes-media 适配层的真实交付物实测链路）----
//
// 文件在**用户机器**上（不在仓库 —— CI/他机安全跳过）；文件名是用户
// 数据：as-is 字符串常量拼接路径，但**打印与断言一律 ASCII 标签**
// （"flac"/"mp3"），中文文件名不进 Output 也不进 demo 断言。

/// 用户音乐目录（实测曲所在）。
const MUSIC_DIR: &str = "C:/Users/Administrator/Music/text";
/// 实测曲 1：FLAC（无损）。键 = "music"。
const MUSIC_FLAC_NAME: &str = "心似烟火.flac";
/// 实测曲 2：MP3（有损）。键 = "music2"。
const MUSIC_MP3_NAME: &str = "Montagem Nada.mp3";

// ---- S15：演示视频（Video 资产面的真实交付物实测链路）----
//
// 与音乐同一条纪律：文件在用户机器上、不入库（.gitignore Media/*.amv）、
// 缺失即整段跳过；打印与断言全 ASCII（"video on"/"video stopped"）。

/// 用户视频目录（实测 AMV 所在）。
const VIDEO_SOURCE: &str = "C:/Users/Administrator/Videos/text/spider_amv.amv";
/// 演示视频在资产根内的路径（拷入后的形态；gitignore 覆盖）。
const VIDEO_REL: &str = "Media/spider.amv";
/// 派生键（= 路径去扩展名；PLAY 会话起播/停播按它引用）。
const VIDEO_KEY: &str = "Media/spider";

/// 装载一首用户音乐：读盘 -> nes-media 全量解码 -> 宿主直注混音器。
/// 成功返回 `Some(信息行)`（ASCII：`music loaded (flac, 44100Hz stereo,
/// N sec, X KB, Y ms)`），文件缺失/解码失败返回 `None`（后者另记失败行）。
fn load_user_music(
    rt: &mut NesRuntime,
    file_name: &str,
    fmt_label: &str,
    mixer_key: &str,
    ring: &Rc<RefCell<VecDeque<String>>>,
) -> Option<String> {
    let bytes = std::fs::read(std::path::Path::new(MUSIC_DIR).join(file_name)).ok()?;
    let started = Instant::now();
    match nes_media::decode_audio(&bytes) {
        Ok(wav) => {
            let secs = if wav.sample_rate > 0 {
                wav.frames() as u64 / u64::from(wav.sample_rate)
            } else {
                0
            };
            let channels = if wav.channels == 1 { "mono" } else { "stereo" };
            let rate = wav.sample_rate;
            rt.register_host_sound(mixer_key, std::sync::Arc::new(wav));
            let line = format!(
                "music loaded ({}, {}Hz {}, {} sec, {} KB, {} ms)",
                fmt_label,
                // 采样率从解码产物读 —— 以实测为准，不预设文件元数据。
                rate, channels, secs, bytes.len() / 1024,
                started.elapsed().as_millis(),
            );
            Some(line)
        }
        Err(e) => {
            log_line(ring, format!("music load failed ({fmt_label}): {e}"));
            None
        }
    }
}

/// 布局常量补注（S12-4 冻结、S12-6 扩底部 dock、S18.1 再扩时间轴 dock；
/// 数值定义已收敛进 [`EditorTheme`]）：面板**恒定宽** —— 最大化只扩中间
/// 世界视口，侧面板不跟着拉伸（消除"整个画面被拉长"观感的关键）。
/// - 左层级面板：x = 8..188（宽 180），y = 40..ch-dock-timeline 上缘；
/// - 右检查器面板：x = cw-198..cw-8（宽 190），y = 8..ch-dock-timeline 上缘；
/// - 底部 Output dock：高 96，y = ch-timeline-dock-状态栏..ch-状态栏，全宽；
/// - 底部时间轴 dock（S18.1）：高 110，紧贴 Output dock 上方，全宽；
/// - 状态栏文本：y = ch-20（底部 16 文本 + 8 边距）；
/// - 视口可编辑区 = 两面板之间再让出顶/左各 16px 标尺（标尺不属于
///   可编辑区，Godot 口径）：视口高 = ch - 40 - (timeline 110 + dock 96
///   + 状态栏 24)。
///
/// 编辑器日志环形保留行数（新行在下，满 N 丢最旧 —— Godot Output
/// 的最小语义；可见窗只放最新能放下的几行，最新行永远可见）。S12-8
/// 起 12 行、S12-9 起 20 行：冒烟钩子要同时断言 Enter 与 FileSystem
/// 双击**两条挂载路径**、play/stop/reset 全链路日志（一轮流程约 17
/// 行，早期行不再被新行挤出断言窗；dock 可见窗仍只显最新几行，显示
/// 面不变）。S14 第 1 期起 26 行：音乐接入再加 2 行装载 + 3 行三态
/// 切换，既有断言行的窗口余量照旧保住。S15 起 29 行：视频接入再加
/// 2 行（video on / video stopped）+ 1 行 F9 分割切换（Media/ 目录把
/// spin.nes 挤出 fs 可见窗 —— 冒烟先切 files 档再加双击，见注入段），
/// 余量口径不变。
const EDITOR_LOG_KEEP: usize = 29;
/// dock 行显示截宽（字符数）：Output dock 是 ListView 行（ListState
/// **位图路径**，S12-11 壳层接入不改 —— 见模块头），等宽 advance=16
/// 不随真字体装载变化，40 字 × 16px = 640px，最小窗 768 下 dock 内衬
///（≈748px）也放得下，行尾不裁字。口径复核（S12-11）：不变。
const DOCK_LINE_CHARS: usize = 40;

// ---- S18.1 时间轴 dock（编辑器补间可视化 + 创建控制）----
//
// 布局裁决（Output dock 上方，全宽）：标题行 + 补间行区（ListView 复用）
// + 创建控制行。其余 dock 布局高度相应让位（见循环内布局投影块 —— 左右
// 面板与可编辑区底缘统一上移 TIMELINE_H）。全部控件进 hit 护盾数组、
// "tldock" 容器进层级树 walk 过滤表（照 grid/dock 先例）。

/// 时间轴补间行池上限（= 行区可见行数 (58-4)/18 = 3，留 1 备用）：进度
/// 细条按行渲染（fill_slot selected 色、行下沿 2px）—— 照网格条带池
/// 先例，控件数恒定有界。行数超出可见窗的进度条不画（列表滚动是 UiVm
/// 瞬态，宿主无钉底通道 —— 宁可少画也不画到列表外）。
const TL_BARS: usize = 4;
/// 创建控制行的通道按钮（循环选中态 * 后缀 = 工具栏 SEL/SNAP 同款口径）。
/// alpha 单值复用 x 输入框（y 框对 alpha 无语义，投影禁用但不隐藏）。
const TL_CHANNELS: [&str; 3] = ["POS", "SCALE", "ALPHA"];
/// 缓动循环档序（TweenEasing::from_str_exact 的合法名单同序）。
const TL_EASINGS: [&str; 5] = ["linear", "smoothstep", "ease_in", "ease_out", "ease_in_out"];
/// 模式循环档序（TweenMode::from_str_exact 的合法名单同序）。
const TL_MODES: [&str; 3] = ["once", "yoyo", "loop"];
/// 创建控制行水平原点（面板内衬，与 dock 标题同款）。
const TL_CTL_X: f32 = 10.0;
/// 创建控制行控件表：(x, 宽)。标签与按钮/输入框的水平布局单点出 ——
/// 投影块每帧按此表重写 offset/size（窗口一变当帧跟上，S12-4 口径）。
/// 768 最小窗全宽 752 下总占地 ~562px，放得下。
const TL_CTL_LAYOUT: [(f32, f32); 9] = [
    (44.0, 40.0),  // POS
    (88.0, 50.0),  // SCALE
    (142.0, 48.0), // ALPHA
    (218.0, 40.0), // x 输入框（pos x / scale sx / alpha a 单值复用）
    (262.0, 40.0), // y 输入框（pos y / scale sy；alpha 无语义）
    (332.0, 48.0), // ms 输入框
    (384.0, 76.0), // 缓动循环按钮
    (464.0, 48.0), // 模式循环按钮
    (516.0, 56.0), // APPLY
];

/// 时间轴行文本（S18.1 冻结格式，全 ASCII）：
/// `POS  43%  ease_out  yoyo  (812ms/1500ms)` —— 通道大写 / 进度百分比
///（线性时间进度取整，缓动以名字单独成列）/ 缓动 / 模式 / 已耗/时长。
/// 字段全部来自 [`nes_scene::TweenRow`] 稳定名，壳层零自有解析。
fn tl_row_text(r: &nes_scene::TweenRow) -> String {
    format!(
        "{}  {:.0}%  {}  {}  ({}ms/{}ms)",
        r.channel.to_ascii_uppercase(),
        r.progress * 100.0,
        r.easing,
        r.mode,
        r.elapsed_ms as i64,
        r.duration_ms as i64,
    )
}

/// 文件系统 dock（S12-8，Godot 左下 res:// 面板）布局常量：
/// - 分隔条厚度（4px border 槽条）与标题行高（"res:/" 16px 文本行）；
/// - P0 布局裁决：**固定分割 + F9 两档** —— 不做拖拽，F9 在
///   "Scene 55% / FileSystem 40%" 与 "Scene 40% / FileSystem 55%"
///   两档间切换（焦点段占大头）；比例是编辑器会话态，不进树。
///
/// Scene / FileSystem 分割比（上段 = Scene；F9 切到 ALT 档）。
const FS_SPLIT_TOP: f32 = 0.55;
const FS_SPLIT_ALT: f32 = 0.40;
/// 资产扫描深度（P0 两层条目：根一层 + 子目录一层）。
const FS_SCAN_DEPTH: usize = 2;
/// 资产白名单后缀（`.` 隐藏项与无后缀垃圾一律不进树）。S13 第 2 期起
/// 含 `wav` —— 声音资产与纹理/脚本同为项目资产，res:// 树如实列出。
/// S14 第 1 期起再扩外部交付格式：图片（jpg/jpeg/webp/gif —— 解码经
/// nes-media 适配层，PNG/BMP 快路径在先）与音频（flac/mp3/ogg/m4a ——
/// Sound 装载先试手写 WAV、失手回落 nes-media）。S15 起再扩视频
///（amv/avi —— Video 资源装载链，见 declare_video）。白名单只是**列出**：
/// 场景没声明它们就只是树里的一行，不产生解码成本。
const FS_EXT_WHITELIST: [&str; 17] = [
    "nes", "bmp", "png", "ron", "ttf", "txt", "wav", //
    "jpg", "jpeg", "webp", "gif", "flac", "mp3", "ogg", "m4a", "amv", "avi",
];
/// 双击裁决窗（帧）：同行两次行点击报告沿间隔 <30 帧 = 双击。UiVm
/// 行回调只有单击 —— 双击是宿主会话态的边沿合成（60fps 下 <0.5s，
/// 与鼠标双击时长同量级；行回调沿 = 抬键沿，与按下沿间隔至差一帧，
/// 同一裁决口径）。
const FS_DBLCLICK_FRAMES: u64 = 30;

/// 标尺最小刻度间距（1px 细条）；数字标签每 2 格（=128px）一个。
/// 密度复核（S12-11）：数字改真字体 14px 后 3 位数 ≈21px、4 位数
/// ≈28px，128px 间距余量巨大；位图回退 3 位 48px 同样放得下 ——
/// 两种模式都无溢出，密度不变。
const RULER_TICK: f32 = 64.0;
/// 刻度条带池上限（顶横 48 + 左竖 32 共用一池）：按 2560×1440 客户
/// 区实测留量（宽向 ≈37 根、高向 ≈21 根），4K 超限少画几根，控件数
/// 与提取/渲染成本恒定有界（同 GRID_POOL 纪律）。
const RULER_TICKS_H: usize = 48;
const RULER_TICKS_V: usize = 32;
/// 刻度数字标签池上限（顶横 24 + 左竖 16）：128px 密度下 2560×1440
/// 用 ≈19 个，超出少标（同上）。
const RULER_LABELS_H: usize = 24;
const RULER_LABELS_V: usize = 16;

/// F5/F6/F7/F8/F9 的 Win32 虚拟键码。Key 契约未列举 F 键 —— 平台层把未列举
/// 虚拟键原样保留为 `Key::Other(原码)`（vk_to_key 兜底分支），边缘
/// 检测直接按 `Key::Other(VK_*)` 比对 pressed 集。选 F 键有个工程
/// 理由：F 键不产生 WM_CHAR 文本 —— 与改名输入框的键入天然无冲突
/// （字母键做不到）。S12-9 起 F5 = PLAY/重启（Shift+F5 = STOP，Godot
/// 同款）；原 F5 手动资产扫描让位给 F8（同码路径照抄）。
const VK_F5: u32 = 0x74;
const VK_F6: u32 = 0x75;
const VK_F7: u32 = 0x76;
/// F8：手动刷新资产扫描（原 F5 职责，S12-9 让位给 PLAY —— 见上注）。
const VK_F8: u32 = 0x77;
/// F9：左栏 Scene/FileSystem 分割档切换（S12-8，两档见 FS_SPLIT_*）。
const VK_F9: u32 = 0x78;

/// 脚本候选池自动刷新周期（帧）：std::fs::read_dir 每帧调用 = 每帧
/// 一次目录枚举 + 若干次分配，60fps 下纯属浪费。裁决：**每 60 帧
/// （约 1s）自动刷一次 + F5 手动即时刷** —— 不用每帧（代价无谓），
/// 也不只靠 F5（外部增删 .nes 文件要等按键才可见，观感差）。
const SCRIPT_SCAN_EVERY: u64 = 60;
/// Inspector 行内字符预算（S12-6 口径：面板内衬宽 178px、位图等宽
/// advance=16 ≈ 11 字/行）—— 候选文件名显示按此截断。口径复核
///（S12-11）：真字体 14px 比例字宽下同 px 容字更多（≈25 字），溢出
/// 只会变少；截断值**保持 11** —— 真字体是可选增强（系统字体缺失时
/// 回退位图），预算必须按两种模式都安全取界（位图 11×16=176 ≤ 178）。
const INS_LINE_CHARS: usize = 11;
/// IME 组合窗锚点的框内内衬（像素，x/y 同值 —— 单行输入框的 P0 近似）。
const IME_CARET_INSET: f32 = 4.0;

/// 启动字体探测链（S12-11 壳层，按优先级）：微软雅黑（CJK+拉丁全覆盖
/// 的现代 UI 字体）→ 黑体（CJK 兜底）→ Segoe UI（纯拉丁兜底）。第一个
/// 可读且可解析的装载为 TTF 默认字体；全部缺失 = 位图回退（见模块头
/// ①）。路径用正斜杠：Windows API 接受，跨字符串书写免转义。
const FONT_CANDIDATES: [&str; 3] = [
    "C:/Windows/Fonts/msyh.ttc",
    "C:/Windows/Fonts/simhei.ttf",
    "C:/Windows/Fonts/segoeui.ttf",
];

/// 装配时开窗尺寸（客户区 (0,0) 的最小化帧沿用的"上次有效值"初值）。
const OPEN_CLIENT: (u32, u32) = (768, 432);

/// 视口网格间距（Godot 2D 编辑器的默认网格观感）。
const GRID_SPACING: f32 = 32.0;
/// 网格吸附步长（Godot 按住 Ctrl 拖动的取整直感）。
const GRID_SNAP: f32 = 8.0;
/// 网格条带池上限（竖条 + 横条共用一个池）：超大窗口下网格密度自适应
/// 上限 —— 线条数超出池容量就少画几根，不动态扩池，控件数与提取/渲染
/// 成本恒定有界（90 根 ≈ 1080p 中等窗口两方向都够用）。
const GRID_POOL: usize = 90;

/// IME 光标 x 偏移（S12-11 第 2 期，**真字宽累加**）：草稿前 `caret`
/// 个字符的逐字 advance 之和。算式与渲染器 `push_ttf_label` 的光标条
/// 同源 —— TTF 模式按 (char, 字号) 查 hmtx 度量、缺字形走 `.notdef`
/// 的 advance（缺字形推进与正文笔位轨迹严格一致，渲染器同款语义）；
/// `'\n'` 归零换行（单行输入框实际不出现，防御性对齐渲染器口径）。
/// TTF 未装载（位图回退）按默认字体等宽 advance 逐字累加 —— 也比第 1
/// 期的 10px 平均步进准（位图 advance=16）。
///
/// 字号必须与输入框**渲染字号同源**（壳层给输入框写 font_size 14、提
/// 取层读同一属性）—— 同字体同字号下累加值与光标条逐位一致，这才是
/// "锚点精确化"的判据。
fn ime_caret_offset(
    font: Option<&TtfFont>,
    draft: &str,
    caret: usize,
    size_px: f32,
    bitmap_advance: f32,
) -> f32 {
    let mut x = 0.0f32;
    for ch in draft.chars().take(caret) {
        if ch == '\n' {
            x = 0.0;
            continue;
        }
        x += match font {
            Some(f) => f
                .glyph_index(ch)
                .and_then(|gid| f.advance(gid, size_px).ok())
                .unwrap_or_else(|| f.advance(TtfFont::NOTDEF, size_px).unwrap_or(0.0)),
            None => bitmap_advance,
        };
    }
    x
}

/// 按压点（**视图空间**）是否落在控件矩形内 —— 与 UiVm 命中同一口径
///（`anchor * viewport + offset` + `size`，不可见即不参与命中）。宿主
/// 用它护住自己的面板交互：压在控件上的点击不清选中、不启动框选，
/// 把交互让给 UiVm 的点击路径（S12-2：改名输入框夺焦；S12-3：层级树
/// ListView 行点击选择）。
fn press_in_control(
    tree: &nes_scene::SceneTree,
    node: nes_scene::NodeId,
    viewport: (f32, f32),
    view_pos: (f32, f32),
) -> bool {
    let visible = tree
        .prop(node, "visible")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !visible {
        return false;
    }
    let vec2 = |name: &str| match tree.prop(node, name) {
        Some(Value::Vec2(v)) => *v,
        _ => nes_scene::Vec2::ZERO,
    };
    let anchor = vec2(PROP_CONTROL_ANCHOR);
    let offset = vec2(PROP_CONTROL_OFFSET);
    let size = vec2(PROP_CONTROL_SIZE);
    let (x, y) = (
        anchor.x * viewport.0 + offset.x,
        anchor.y * viewport.1 + offset.y,
    );
    view_pos.0 >= x && view_pos.0 < x + size.x && view_pos.1 >= y && view_pos.1 < y + size.y
}

/// 编辑器日志入列（Output dock 的数据面）：环形保留最近
/// [`EDITOR_LOG_KEEP`] 行，新行在下、满员丢最旧。引擎没有结构化
/// 日志通道，undo/redo/选择/删除/改名/拖移这些编辑器事件在各自
/// 落账点就地推一行（Godot Output dock 的最小等价物）。
/// 文件系统 dock 的一个资产条目（S12-8）：`rel` = 资产根相对路径
///（正斜杠分隔，`Scripts/blink.nes` —— registry_key 落账口径，与游戏
/// 路径 `read(assets_root.join(rel))` 同一相对系；目录条目尾带 `/`）；
/// `is_dir` 目录/文件；`depth` = 相对根的层级（根条目 0）—— 行缩进
/// 由它推导。
struct FsEntry {
    rel: String,
    is_dir: bool,
    depth: usize,
}

/// 递归扫描资产根（FileSystem 树数据面 + F-4 脚本候选池的**同一数据
/// 源**，scan_scripts 先例的推广）：每层目录优先、目录/文件各自字典
/// 序；文件按白名单后缀过滤（[`FS_EXT_WHITELIST`]），`.` 开头隐藏项
///（.mimosa 等）与无后缀垃圾一律跳过；深度 [`FS_SCAN_DEPTH`] 封顶
///（P0 两层：根 + 子目录一层）。目录不存在/不可读 = 空列表（如实，
/// 不猜）。返回的相对路径直接就是挂载/打开口径（无需再拼前缀）。
fn scan_assets(root: &Path) -> Vec<FsEntry> {
    let mut out: Vec<FsEntry> = Vec::new();
    scan_assets_dir(root, "", 0, &mut out);
    out
}

/// [`scan_assets`] 的单层实现：`rel_prefix` = 相对资产根的目录前缀
///（`Scripts/`，空串 = 根）；`level` = 当前层级（根 = 0）。
fn scan_assets_dir(root: &Path, rel_prefix: &str, level: usize, out: &mut Vec<FsEntry>) {
    let dir = if rel_prefix.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel_prefix)
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return; // 目录不存在/不可读：该层如实为空。
    };
    let (mut dirs, mut files): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for e in rd.flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_string) else {
            continue; // 非 UTF-8 名：面板行文本是 UTF-8 口径，跳过。
        };
        if name.starts_with('.') {
            continue; // 隐藏项（.mimosa 等）不进树。
        }
        let p = e.path();
        if p.is_dir() {
            dirs.push(name);
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| FS_EXT_WHITELIST.contains(&x))
        {
            files.push(name);
        }
    }
    dirs.sort();
    files.sort();
    for d in dirs {
        let prefix = format!("{rel_prefix}{d}/");
        out.push(FsEntry {
            rel: prefix.clone(),
            is_dir: true,
            depth: level,
        });
        if level + 1 < FS_SCAN_DEPTH {
            scan_assets_dir(root, &prefix, level + 1, out);
        }
    }
    for f in files {
        out.push(FsEntry {
            rel: format!("{rel_prefix}{f}"),
            is_dir: false,
            depth: level,
        });
    }
}

/// fs 行文本：每层两空格缩进 + 基名（目录尾带 `/`）—— Godot res://
/// 树的缩进直感，行文本经默认字体等宽渲染。
fn fs_row_text(e: &FsEntry) -> String {
    let name = base_name(e.rel.trim_end_matches('/'));
    let indent = "  ".repeat(e.depth);
    if e.is_dir {
        format!("{indent}{name}/")
    } else {
        format!("{indent}{name}")
    }
}

/// 脚本候选池 = 扫描结果中的全部 .nes（资产根相对路径）—— 与 res://
/// 树同一数据源：FileSystem 选中的 .nes 必在池内（F6 候选起点裁决的
/// 前提），两处入口看到同一个资产世界。
fn script_pool(entries: &[FsEntry]) -> Vec<String> {
    entries
        .iter()
        .filter(|e| !e.is_dir && e.rel.ends_with(".nes"))
        .map(|e| e.rel.clone())
        .collect()
}

/// 相对路径后缀（不含点；无后缀 = 空串）—— 双击分派的提示行用。
fn extension_suffix(rel: &str) -> &str {
    rel.rsplit_once('.').map(|(_, e)| e).unwrap_or("")
}

/// 路径基名（面板行宽只放得下文件名，不含目录前缀）。
fn base_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// 挂载建的 Script 子节点名 = 脚本基名去后缀（截 12 字）—— 挂载在
/// 层级树里可见（Godot 的脚本附着直感），不再是隐形数据。
fn script_node_name(rel: &str) -> String {
    let b = base_name(rel);
    b.strip_suffix(".nes").unwrap_or(b).chars().take(12).collect()
}

/// 挂载目标解析（只读）：选中本身是 Script 节点 → 挂它；否则其第一个
/// 直接 Script 子节点。返回 (uid, registry_key 是否非空, enabled)。
fn mount_target(
    tree: &nes_scene::SceneTree,
    primary: nes_scene::NodeId,
) -> Option<(Uid, bool, bool)> {
    let m = if tree.kind_tag(primary) == Some(nes_scene::NodeKindTag::Script) {
        Some(primary)
    } else {
        tree.children(primary)
            .iter()
            .copied()
            .find(|&c| tree.kind_tag(c) == Some(nes_scene::NodeKindTag::Script))
    }?;
    let mounted =
        matches!(tree.prop(m, "registry_key"), Some(Value::Str(s)) if !s.is_empty());
    let enabled = matches!(tree.prop(m, "enabled"), Some(Value::Bool(true)));
    tree.uid_of(m).map(|u| (u, mounted, enabled))
}

/// F-4 挂载事务（S12-8 起为 FileSystem 双击与 Inspector Enter **两处
/// 入口的同一事务**，原 Enter 内联体上提）：目标解析（选中本身是
/// Script -> 挂它；否则第一个 Script 子节点；再没有 -> 同事务新建，
/// 名字 = 脚本基名，层级树里可见）+ registry_key 落账 —— undo 一步
/// 整回（T-INS-03 契约）。无选中 = Output 一行说明，不落账。
fn mount_script(
    tree: &mut nes_scene::SceneTree,
    log: &mut TransactionLog,
    sel: &Selection,
    ring: &Rc<RefCell<VecDeque<String>>>,
    rel: &str,
) {
    let Some(puid) = sel.primary(tree).and_then(|p| tree.uid_of(p)) else {
        log_line(ring, "mount: no selection".into());
        return;
    };
    let target: Option<Uid> = match tree.find_by_uid(&puid) {
        Some(p) if tree.kind_tag(p) == Some(nes_scene::NodeKindTag::Script) => {
            Some(puid.clone())
        }
        Some(p) => tree
            .children(p)
            .iter()
            .find(|&&c| tree.kind_tag(c) == Some(nes_scene::NodeKindTag::Script))
            .and_then(|c| tree.uid_of(*c)),
        None => None,
    };
    log.begin().unwrap();
    let (mount_uid, created) = match target {
        Some(u) => (u, false),
        None => {
            let u = Hierarchy::new(tree, log)
                .create_child(&puid, &script_node_name(rel), NodeKind::Script)
                .unwrap();
            (u, true)
        }
    };
    Inspector::new(tree, log)
        .modify_prop(&mount_uid, "registry_key", Value::Str(rel.to_string()))
        .unwrap();
    log.commit().unwrap();
    let host = tree
        .find_by_uid(&puid)
        .and_then(|id| tree.name(id).map(str::to_string));
    log_line(
        ring,
        format!(
            "mount {}{} <- {}",
            host.unwrap_or_default(),
            if created { " (+script)" } else { "" },
            base_name(rel),
        ),
    );
}

fn log_line(ring: &Rc<RefCell<VecDeque<String>>>, line: String) {
    let mut q = ring.borrow_mut();
    if q.len() >= EDITOR_LOG_KEEP {
        q.pop_front();
    }
    q.push_back(line);
}

/// play-in-editor 运行会话态（S12-9；编辑器会话态 —— 不进树、不落盘）：
/// - `playing`：运行态开关 —— 帧循环观察者参数与编辑交互护盾的唯一判据；
/// - `vm`：运行态的 ScriptVm（编辑态为 None；STOP 即 drop —— 脚本停、
///   局部/连接随 VM 蒸发，事务历史在宿主手里不动）；
/// - `snapshot`：PLAY 时的全树快照（前序逐节点 SubtreeSnapshot，uid
///   锚定）。RESET 从它数据面还原（见模块头 S12-9 裁决）。从首次 PLAY
///   存活到 RESET；运行中重启（再按 PLAY）**不刷新** —— 重启也要能
///   回到同一个"进入运行态之前"。
struct PlaySession {
    playing: bool,
    vm: Option<ScriptVm>,
    snapshot: Vec<SubtreeSnapshot>,
}

impl PlaySession {
    fn new() -> Self {
        PlaySession {
            playing: false,
            vm: None,
            snapshot: Vec::new(),
        }
    }

    /// PLAY（编辑态进入运行态；运行态再按 = 重启：drop 旧 VM 重建，
    /// 快照保留首次进入时的那份）。全链路：快照 -> 同键注册 ->
    /// attach_all_with_sources -> mount_input_view。装载缺口（编译错
    /// /读失败/空键）逐行进 Output、对应节点跳过，**不回编辑态**。
    fn start(
        &mut self,
        rt: &mut NesRuntime,
        assets: &Path,
        ring: &Rc<RefCell<VecDeque<String>>>,
    ) {
        if self.playing {
            self.vm = None; // 重启：只换 VM，快照不动（见结构体注）。
        } else {
            // 首次进入：快照全树（前序逐节点，uid 锚定、含属性表全集）。
            self.snapshot = {
                let tree = rt.tree_mut();
                let root_uid = tree.uid_of(tree.root()).unwrap();
                tree.preorder()
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, n)| {
                        let parent = tree
                            .parent(n)
                            .and_then(|p| tree.uid_of(p))
                            .unwrap_or_else(|| root_uid.clone());
                        SubtreeSnapshot::capture(tree, n, parent, i)
                    })
                    .collect()
            };
            log_line(
                ring,
                format!("snapshot {} nodes (RESET to restore)", self.snapshot.len()),
            );
        }
        // 宿主按同键注册（S12-7 口径的运行时半边）：registry_key =
        // 资产根相对路径，读文件 -> 编译 -> register。编译失败报行号
        // 错误、该键不注册（对应节点随后的 attach 如实报缺口、跳过）。
        let mut vm = ScriptVm::new();
        let keys: Vec<String> = {
            let tree = rt.tree_mut();
            let mut keys: Vec<String> = tree
                .preorder()
                .into_iter()
                .filter(|&n| tree.kind_tag(n) == Some(nes_scene::NodeKindTag::Script))
                .filter_map(|n| match tree.prop(n, "registry_key") {
                    Some(Value::Str(k)) if !k.is_empty() => Some(k.clone()),
                    _ => None,
                })
                .collect();
            keys.sort();
            keys.dedup();
            keys
        };
        for key in &keys {
            match std::fs::read_to_string(assets.join(key)) {
                Ok(text) => match compile_script(&text) {
                    Ok(script) => {
                        vm.register(key, script);
                    }
                    Err(e) => {
                        // ParseError 的 Display 自带行/列（"第 L 行第 C 列"）。
                        log_line(ring, format!("play: {} {e}", base_name(key)));
                    }
                },
                Err(_) => {
                    log_line(ring, format!("play: {} read failed", base_name(key)));
                }
            }
        }
        // 全路径装载（内嵌 source / registry_key 注册表 / 外置 script 槽
        // 三路同口）—— issues 通道逐行上报，不挡其他节点。
        let table = rt.resources_mut().clone();
        let total;
        let issues;
        {
            let tree = rt.tree_mut();
            total = tree
                .preorder()
                .into_iter()
                .filter(|&n| tree.kind_tag(n) == Some(nes_scene::NodeKindTag::Script))
                .count();
            issues = vm.attach_all_with_sources(tree, &table, &mut |rel| {
                std::fs::read_to_string(assets.join(rel)).map_err(|e| e.to_string())
            });
        }
        for (node, why) in &issues {
            let name = rt.tree_mut().name(*node).unwrap_or("?").to_string();
            log_line(ring, format!("play: skip {name}: {why}"));
        }
        let attached = total - issues.len();
        rt.mount_input_view(&mut vm);
        // S13 第 2 期：场景有 Sound 资源则自动开音频（幂等 —— 运行中重启
        // 不会重复开；失败报一行不中断，"带病也能跑"的既有口径）。STOP
        // 不关音频：混音器与设备跨会话存活（空混音器静音填充，幂等无害）。
        let has_sound = rt
            .resources_mut()
            .iter()
            .any(|e| e.kind() == Some(AssetKind::Audio));
        if has_sound {
            match rt.open_audio() {
                Ok(()) => log_line(ring, "audio on".into()),
                Err(e) => log_line(ring, format!("audio: {e}")),
            }
        }
        // S15：有视频资源则随 PLAY 起播（音轨同步出声 —— P0 起点对齐；
        // 换页在帧路径推进 —— Sprite 引用它即播画面，本演示场景未挂
        // Sprite，播放纯走渲染侧状态机 + 音轨）。失败报一行不中断。
        if rt.video_count() > 0 {
            if rt.play_video(VIDEO_KEY) {
                log_line(ring, "video on".into());
            } else {
                log_line(ring, "video: key not declared".into());
            }
        }
        self.vm = Some(vm);
        self.playing = true;
        log_line(ring, format!("play ({attached} scripts)"));
    }

    /// STOP（Shift+F5 / 工具栏）：脚本停（drop VM）、事务历史不动、
    /// **不自动还原** —— Godot 语义：运行期改动就是真改；RESET 才回。
    /// 视频随 STOP 停播（点名停音轨声部；记一行取证）。
    fn stop(&mut self, rt: &mut NesRuntime, ring: &Rc<RefCell<VecDeque<String>>>) {
        if !self.playing {
            return;
        }
        self.vm = None;
        self.playing = false;
        if rt.stop_video(VIDEO_KEY) {
            log_line(ring, "video stopped".into());
        }
        log_line(ring, "stop".into());
    }

    /// RESET（工具栏；仅编辑态可用）：从快照数据面还原 —— 按 uid 寻回
    /// 节点、摘掉快照没有的键、apply_data 整体写回（名字/变换/处理模
    /// 式/属性全集）。结构不变是正确性前提（运行期编辑禁用 + 脚本无
    /// 结构指令），寻不回的快照如实跳过（不发生，防御性容错）。
    fn reset(&mut self, rt: &mut NesRuntime, ring: &Rc<RefCell<VecDeque<String>>>) {
        if self.playing {
            log_line(ring, "reset: playing (stop first)".into());
            return;
        }
        if self.snapshot.is_empty() {
            log_line(ring, "reset: no snapshot".into());
            return;
        }
        {
            let tree = rt.tree_mut();
            for snap in &self.snapshot {
                let Some(id) = tree.find_by_uid(&snap.data.uid) else {
                    continue;
                };
                // 快照没有的键 = 运行期新增 —— 摘掉（apply_data 只覆盖
                // 快照键；schema 键出生即满配，这只在裸通道写入时发生）。
                let extra: Vec<String> = tree
                    .props(id)
                    .map(|p| {
                        p.iter()
                            .map(|(k, _)| k.to_string())
                            .filter(|k| snap.data.props.get(k).is_none())
                            .collect()
                    })
                    .unwrap_or_default();
                for k in extra {
                    tree.remove_prop(id, &k);
                }
                let _ = SubtreeSnapshot::apply_data(tree, id, &snap.data);
            }
            tree.apply_pending();
        }
        self.snapshot.clear();
        log_line(ring, "reset".into());
    }
}

fn main() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    let tex = assets.join("Textures");
    std::fs::create_dir_all(&tex).unwrap();
    for (name, rgb) in [
        ("player.bmp", (90, 130, 255)),
        ("enemy.bmp", (255, 80, 80)),
        ("bullet.bmp", (255, 220, 60)),
        ("heart.bmp", (255, 120, 200)),
        ("door.bmp", (90, 220, 120)),
    ] {
        if !tex.join(name).exists() {
            let (r, g, b) = rgb;
            write_bmp_rgba(&tex.join(name), 16, 16, &solid_rgba(r, g, b)).expect("写纹理");
        }
    }
    // S18 皮肤纹理（九宫格面板/按钮，DESIGN-NOTES §6.3）：代码生成、缺了
    // 再写（walk_sheet/beep 同一家法）。panel 48×48 边距 8、button 48×20
    // 边距 4；绝对色成品纹理（深色底 + 1px 亮边框 + 微渐变 + 噪点）。
    let panel_skin = tex.join("panel_skin.bmp");
    if !panel_skin.exists() {
        write_bmp_rgba(
            &panel_skin,
            48,
            48,
            &skin_rgba(48, 48, 8, [30, 34, 40], [58, 64, 72], [66, 74, 84]),
        )
        .expect("写面板皮肤");
    }
    let button_skin = tex.join("button_skin.bmp");
    if !button_skin.exists() {
        write_bmp_rgba(
            &button_skin,
            48,
            20,
            &skin_rgba(48, 20, 4, [44, 50, 58], [58, 64, 72], [92, 102, 116]),
        )
        .expect("写按钮皮肤");
    }
    // 演示声音资产（S13 第 2 期）：440Hz / 250ms，缺了再写（bmp 同口径）。
    let audio_dir = assets.join("Audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let beep = audio_dir.join("beep.wav");
    if !beep.exists() {
        let wav = nes_audio::Wav { sample_rate: 22050, channels: 1, samples: beep_samples(250, 8000) };
        write_wav(&beep, &wav).expect("写蜂鸣 WAV");
    }

    let mut rt = NesRuntime::open_windowed_with_root(
        &assets,
        "NES 2.0 - Editor Shell (S9-3b)",
        768,
        432,
    )
    .expect("窗口装配");
    for t in ["player", "enemy", "bullet", "heart", "door"] {
        let _ = rt.declare_texture(&format!("Textures/{t}.bmp")).expect("声明纹理");
    }
    // S18 皮肤纹理声明（资源 id 经返回值取用，不写死槽位号；面板/按钮
    // 九宫格 ns_tex 引用这两个键，见装配段 skin_panel/skin_button）。
    let panel_skin_id = rt.declare_texture("Textures/panel_skin.bmp").expect("声明面板皮肤");
    let button_skin_id = rt.declare_texture("Textures/button_skin.bmp").expect("声明按钮皮肤");
    let _ = rt.declare_sound("Audio/beep.wav").expect("声明演示声音");
    // 演示视频资产（S15）：用户实测 AMV 不入库 —— 用户目录有就复制进
    // Media/（gitignore 覆盖）并声明；缺失即整段跳过（音乐同口径）。
    let video_present = if Path::new(VIDEO_SOURCE).exists() || assets.join(VIDEO_REL).exists() {
        let media_dir = assets.join("Media");
        std::fs::create_dir_all(&media_dir).unwrap();
        if !assets.join(VIDEO_REL).exists() {
            let bytes = std::fs::read(VIDEO_SOURCE).expect("读演示视频（存在性已判）");
            std::fs::write(assets.join(VIDEO_REL), &bytes).expect("复制演示视频");
        }
        let id = rt.declare_video(VIDEO_REL).expect("声明演示视频");
        let _ = id;
        true
    } else {
        false
    };
    let expected_loaded = if video_present { 9 } else { 8 };
    let report = rt.bind_assets();
    assert_eq!(
        report.loaded.len(),
        expected_loaded,
        "7 纹理（5 演示 + 2 皮肤，S18）+ 1 声音（S13）+ 1 视频（S15，在场时）：{report:?}"
    );
    assert_eq!(rt.upload_pending_textures().expect("上传"), 7);
    if video_present {
        assert_eq!(rt.video_count(), 1, "演示视频解析入表（首帧已上 GPU）");
    }
    // 位图默认字体的等宽 advance（IME 锚点在位图回退模式下的累加步进
    // —— 从 font_metrics 实读，不写死 16）。
    let bitmap_advance: f32;
    {
        let font_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../nes-render-wgpu/examples/assets");
        let (w, h, sheet) =
            bmp::load_rgba(&std::fs::read(font_dir.join("font_atlas.bmp")).unwrap()).unwrap();
        let metrics = std::fs::read_to_string(font_dir.join("font_metrics.txt")).unwrap();
        let field = |k: &str| -> f32 {
            metrics
                .split_whitespace()
                .find_map(|t| t.strip_prefix(&format!("{k}=")))
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("font_metrics 缺 {k}"))
        };
        let cell = metrics
            .split_whitespace()
            .find_map(|t| t.strip_prefix("cell="))
            .and_then(|c| c.split_once('x'))
            .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
            .expect("cell 格式");
        bitmap_advance = field("advance");        rt.consumer_mut()
            .expect("GPU 消费器")
            .set_default_font(
                FontParams {
                    width: w,
                    height: h,
                    cell_w: cell.0,
                    cell_h: cell.1,
                    cols: field("cols") as u32,
                    first_char: field("first") as u32,
                    count: field("count") as u32,
                    advance: field("advance"),
                    line_height: field("line_height"),
                },
                &sheet,
            )
            .expect("登记默认字体");
    }

    // 编辑目标场景（自建 —— 编辑器也可以加载任意场景文件）。
    let (grid, grid_bars, ruler, ruler_h, ruler_v, ruler_corner, ruler_ticks, ruler_labels, dock, dock_bg, dock_title, hud_dock, toolbar, tool_bg, tool_sep, theme_node, tool_plates, tool_sel, tool_snap, tool_grid, tool_play, tool_stop, tool_reset, ins_tf_title, ins_sc_title, ins_script, cam, obj1, obj2, obj3, hud_tree, hud_ins_bg, hud_ins, hud_st, sel_box, name_input, hud_scene, fsdock, fs_bg, fs_title, fs_sep, fs_tree, tldock, tl_bg, tl_title, hud_tl, tl_bars, tl_new_label, tl_to_label, tl_ms_label, tl_plates, tl_pos, tl_scale, tl_alpha, tl_ease, tl_mode, tl_apply, tl_x_in, tl_y_in, tl_ms_in) = {
        let tree = rt.tree_mut();
        let root = tree.root();
        // S18：主题节点（"主题即场景节点"，nes-scene/ui.rs 既有机制 ——
        // 提取层取前序序最后的 Theme 节点为当前色板）。八槽位值来自
        // [`EditorTheme`] 色板表（P0 = DEFAULT_DARK 同值，观感零漂移；
        // 换肤入口从此收敛到壳层常量表，提取/渲染层零改动）。纯数据
        // 节点不渲染；层级树投影把它加进 skips（皮肤节点不是可编辑
        // 对象，同 grid/ruler/dock 纪律，见下方 walk 的 skips 表）。
        let theme_node = tree.add_node(root, "theme", NodeKind::Theme);
        for (name, packed) in PALETTE {
            let _ = tree.set_prop(theme_node, name, Value::I64(*packed));
        }
        // S18 皮肤九宫格属性写入器（裸 Control 专用 —— 提取层仅对
        // Control 读 ns_*，Button/TextInput/List 等摊平类不读，S16.6
        // 口径）。ns_* 非 schema 键，走 set_prop_raw 前向通道（z_index/
        // border_w/font_size 先例）。modulate=false = 皮肤是成品绝对色
        // （DESIGN-NOTES §6.3：乘法 tint 出不了"亮边框比底亮"）；
        // tiling=false = 渐变拉伸（平铺留给未来噪点面板的选项）。
        let skin_panel = |tree: &mut nes_scene::SceneTree, n: nes_scene::NodeId| {
            tree.set_prop_raw(n, PROP_NS_TEX, Value::Resource(panel_skin_id.get() as u64));
            for p in [PROP_NS_L, PROP_NS_T, PROP_NS_R, PROP_NS_B] {
                tree.set_prop_raw(n, p, Value::I64(SKIN_PANEL_MARGIN));
            }
            tree.set_prop_raw(n, PROP_NS_MODULATE, Value::Bool(false));
            tree.set_prop_raw(n, PROP_NS_TILING, Value::Bool(false));
        };
        let skin_button = |tree: &mut nes_scene::SceneTree, n: nes_scene::NodeId| {
            tree.set_prop_raw(n, PROP_NS_TEX, Value::Resource(button_skin_id.get() as u64));
            for p in [PROP_NS_L, PROP_NS_T, PROP_NS_R, PROP_NS_B] {
                tree.set_prop_raw(n, p, Value::I64(SKIN_BTN_MARGIN));
            }
            tree.set_prop_raw(n, PROP_NS_MODULATE, Value::Bool(false));
            tree.set_prop_raw(n, PROP_NS_TILING, Value::Bool(false));
        };
        // 视口网格（S12-5 Godot 观感）：条带池 —— 竖条 1px 宽 × 视口高、
        // 横条 1px 高 × 视口宽，fill_slot="border" 吃边框槽色，visible=false
        // 备用（每帧投影按视口布线，见循环内网格段）。全部挂在 "grid" 容器
        // 之下：层级树投影跳过该容器（网格是观感，不是可编辑对象，不进
        // 行列表）。z_index 经 set_prop_raw 置 -100 —— Control 继承链
        //（Control→Node）没有 z_index schema 键，而提取层 z_of 直读属性表；
        // -100 压在精灵（z=0）与选中高亮（z=5）之下，网格永远垫底。
        // 建在树前部（先于相机/精灵），双保险：同 z 时前序序也更早。
        let grid = tree.add_node(root, "grid", NodeKind::Node);
        let mut grid_bars = Vec::with_capacity(GRID_POOL);
        for _ in 0..GRID_POOL {
            let bar = tree.add_node(grid, "grid_bar", NodeKind::Control);
            let _ = tree.set_prop(bar, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(bar, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(bar, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(1.0, 1.0)));
            let _ = tree.set_prop(bar, "fill_slot", Value::Str(SLOT_BORDER_NAME.into()));
            let _ = tree.set_prop(bar, "visible", Value::Bool(false));
            tree.set_prop_raw(bar, "z_index", Value::I64(-100));
            grid_bars.push(bar);
        }
        // 2D 标尺（S12-6，Godot CanvasItemEditor::_draw_rulers 的自绘
        // 版）：顶横条带 + 左竖条带（panel 槽铺底）+ 左上角块（border
        // 槽，Godot 角块同款）+ 刻度细条池 + 数字标签池。刻度/标签挂在
        // "ruler" 容器下：层级树 walk 整子树跳过（观感节点不是可编辑
        // 对象）。z_index 经 set_prop_raw 垫底但在网格之上（-90：网格
        // -100、精灵 0、选中高亮 5）—— 场景对象永远盖过观感。建在树
        // 前部，与网格同款双保险。
        let ruler = tree.add_node(root, "ruler", NodeKind::Node);
        let mk_strip = |tree: &mut nes_scene::SceneTree, name: &str, slot: &str| {
            let n = tree.add_node(ruler, name, NodeKind::Control);
            let _ = tree.set_prop(n, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(n, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(n, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(1.0, 1.0)));
            let _ = tree.set_prop(n, "fill_slot", Value::Str(slot.into()));
            tree.set_prop_raw(n, "z_index", Value::I64(-90));
            n
        };
        let ruler_h = mk_strip(tree, "ruler_h", "panel");
        let ruler_v = mk_strip(tree, "ruler_v", "panel");
        let ruler_corner = mk_strip(tree, "ruler_corner", "border");
        // 刻度细条池：顶横在前、左竖在后（同网格条带池的布线纪律）。
        // 主刻度（整 128）全高、次刻度（64）半高贴视口缘 —— Godot
        // graduation 的层级观感（major 全长 / minor 0.75 段）。
        let mut ruler_ticks = Vec::with_capacity(RULER_TICKS_H + RULER_TICKS_V);
        for _ in 0..RULER_TICKS_H + RULER_TICKS_V {
            let tick = tree.add_node(ruler, "ruler_tick", NodeKind::Control);
            let _ = tree.set_prop(tick, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(tick, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(tick, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(1.0, 1.0)));
            let _ = tree.set_prop(tick, "fill_slot", Value::Str(SLOT_BORDER_NAME.into()));
            let _ = tree.set_prop(tick, "visible", Value::Bool(false));
            tree.set_prop_raw(tick, "z_index", Value::I64(-90));
            ruler_ticks.push(tick);
        }
        // 数字标签池：Label 空文本 = 提取层判空不上屏（免 visible 接
        // 线）。Godot 竖标尺数字是旋转 90° 排版，等宽点阵字体先横排
        // —— 3 位数（48px 宽）会溢出 16px 条带压到视口最左缘，观感
        // 等同刻度注记，取舍记此。S12-11：字号 14（真字体下 3 位 ≈21px
        // 且行高 ≈18.5px 更贴条带；密度复核见 RULER_TICK 注）。
        let mut ruler_labels = Vec::with_capacity(RULER_LABELS_H + RULER_LABELS_V);
        for _ in 0..RULER_LABELS_H + RULER_LABELS_V {
            let lab = tree.add_node(ruler, "ruler_label", NodeKind::Label);
            tree.set_local(lab, Transform2D::from_pos(-1000.0, -1000.0));
            let _ = tree.set_prop(lab, PROP_LABEL_TEXT, Value::Str(String::new()));
            let _ = tree.set_prop(lab, "font_size", Value::I64(UI_FONT_SIZE));
            tree.set_prop_raw(lab, "z_index", Value::I64(-90));
            ruler_labels.push(lab);
        }
        // 底部 Output dock（S12-6，Godot 底部"输出"面板）：panel 槽
        // 铺底 + 顶部 "Output" 标题 + ListView 显编辑器日志行（复用
        // 控件，新行在下）。挂 "dock" 容器：walk 整子树跳过。z=-80
        // 垫底（网格 -100、标尺 -90 之上，仍在精灵 0 之下 —— 场景
        // 对象优先于观感，同上）。
        let dock = tree.add_node(root, "dock", NodeKind::Node);
        let dock_bg = tree.add_node(dock, "dock_bg", NodeKind::Control);
        let _ = tree.set_prop(dock_bg, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(dock_bg, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(dock_bg, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(1.0, 1.0)));
        let _ = tree.set_prop(dock_bg, "fill_slot", Value::Str(SLOT_PANEL_NAME.into()));
        // S18 换肤：Output dock 面板换九宫格皮肤（纹理自带边 —— 九宫格
        // 模式下 fill/border 条带不画，renderer.rs S16.6 分臂）。
        skin_panel(tree, dock_bg);
        tree.set_prop_raw(dock_bg, "z_index", Value::I64(-80));
        let dock_title = tree.add_node(dock, "dock_title", NodeKind::Label);
        tree.set_local(dock_title, Transform2D::from_pos(MARGIN + 2.0, 320.0));
        let _ = tree.set_prop(dock_title, PROP_LABEL_TEXT, Value::Str("Output".into()));
        let _ = tree.set_prop(dock_title, "font_size", Value::I64(UI_FONT_SIZE));
        tree.set_prop_raw(dock_title, "z_index", Value::I64(-80));
        let hud_dock = tree.add_node(dock, "hud_dock", NodeKind::ListView);
        let _ = tree.set_prop(hud_dock, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(hud_dock, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(MARGIN, 320.0)));
        let _ = tree.set_prop(hud_dock, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(600.0, DOCK_H - DOCK_TITLE_H - 2.0)));
        let _ = tree.set_prop(hud_dock, "rows", Value::Str(String::new()));
        let _ = tree.set_prop(hud_dock, "row_h", Value::I64(DOCK_ROW_H as i64));
        tree.set_prop_raw(hud_dock, "z_index", Value::I64(-80));
        // 文件系统 dock（S12-8，Godot 左下 res:// 面板）：panel 槽
        // 铺底 + "res:/" 标题行 + 资产树 ListView（复用控件，行文本 =
        // 相对资产根的缩进树；选中/行点击与层级树同款投影-回调口径）。
        // 分隔条（4px border 槽条）独立成控件 —— Scene 与 FileSystem
        // 两段的界线（P0 固定分割 + F9 两档，见布局投影块）。挂
        // "fsdock" 容器：walk 整子树跳过（资产观感不是场景对象，不进
        // 行列表）。z=-60 垫底（网格 -100、标尺 -90、dock -80、工具栏
        // -70 之上，仍在精灵 0 之下 —— 同款纪律；左栏与视口不重叠，
        // 纯口径一致）。offset/size 装配期只给占位初值，每帧由布局
        // 投影重写（窗口一变当帧跟上，S12-4 ①口径）。
        let fsdock = tree.add_node(root, "fsdock", NodeKind::Node);
        let fs_bg = tree.add_node(fsdock, "fs_bg", NodeKind::Control);
        let _ = tree.set_prop(fs_bg, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(fs_bg, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(MARGIN, 240.0)));
        let _ = tree.set_prop(fs_bg, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, 100.0)));
        let _ = tree.set_prop(fs_bg, "fill_slot", Value::Str(SLOT_PANEL_NAME.into()));
        // S18 换肤：FileSystem 面板九宫格皮肤（同 dock_bg 口径）。
        skin_panel(tree, fs_bg);
        tree.set_prop_raw(fs_bg, "z_index", Value::I64(-60));
        let fs_title = tree.add_node(fsdock, "fs_title", NodeKind::Label);
        tree.set_local(fs_title, Transform2D::from_pos(MARGIN + 2.0, 242.0));
        let _ = tree.set_prop(fs_title, PROP_LABEL_TEXT, Value::Str("res:/".into()));
        let _ = tree.set_prop(fs_title, "font_size", Value::I64(UI_FONT_SIZE));
        tree.set_prop_raw(fs_title, "z_index", Value::I64(-60));
        let fs_sep = tree.add_node(fsdock, "fs_sep", NodeKind::Control);
        let _ = tree.set_prop(fs_sep, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(fs_sep, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(MARGIN, 236.0)));
        let _ = tree.set_prop(fs_sep, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, FS_SEP_H)));
        let _ = tree.set_prop(fs_sep, "fill_slot", Value::Str(SLOT_BORDER_NAME.into()));
        tree.set_prop_raw(fs_sep, "z_index", Value::I64(-60));
        let fs_tree = tree.add_node(fsdock, "fs_tree", NodeKind::ListView);
        let _ = tree.set_prop(fs_tree, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(fs_tree, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(MARGIN, 258.0)));
        let _ = tree.set_prop(fs_tree, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, 80.0)));
        let _ = tree.set_prop(fs_tree, "rows", Value::Str(String::new()));
        let _ = tree.set_prop(fs_tree, "row_h", Value::I64(FS_ROW_H as i64));
        let _ = tree.set_prop(fs_tree, "selected", Value::I64(-1));
        tree.set_prop_raw(fs_tree, "z_index", Value::I64(-60));
        // 时间轴 dock（S18.1，Output 上方的全宽面板）：九宫格皮肤铺底 +
        // "TIMELINE" 标题 + 补间行区 ListView（选中节点的活动补间投影，
        // 行文本 = tl_row_text 冻结格式）+ 进度细条池（fill_slot selected、
        // 行下沿 2px —— 照网格条带池先例）+ 创建控制行（POS/SCALE/ALPHA
        // 通道按钮 + to x/y + ms 三个 TextInput + 缓动/模式循环按钮 +
        // APPLY）。offset/size 装配期占位，每帧布局投影重写。挂 "tldock"
        // 容器：walk 整子树跳过（时间轴是观感/工具，不是可编辑对象）。
        // z 纪律同 dock：铺底/标题/列表 -80（场景对象优先于观感），进度条
        // -79（列表之上、精灵之下），交互控件缺省 z（工具栏按钮同款）。
        let tldock = tree.add_node(root, "tldock", NodeKind::Node);
        let tl_bg = tree.add_node(tldock, "tl_bg", NodeKind::Control);
        let _ = tree.set_prop(tl_bg, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tl_bg, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(MARGIN, 200.0)));
        let _ = tree.set_prop(tl_bg, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(600.0, TIMELINE_H)));
        let _ = tree.set_prop(tl_bg, "fill_slot", Value::Str(SLOT_PANEL_NAME.into()));
        skin_panel(tree, tl_bg);
        tree.set_prop_raw(tl_bg, "z_index", Value::I64(-80));
        let tl_title = tree.add_node(tldock, "tl_title", NodeKind::Label);
        tree.set_local(tl_title, Transform2D::from_pos(MARGIN + 2.0, 201.0));
        let _ = tree.set_prop(tl_title, PROP_LABEL_TEXT, Value::Str("TIMELINE".into()));
        let _ = tree.set_prop(tl_title, "font_size", Value::I64(UI_FONT_SIZE));
        tree.set_prop_raw(tl_title, "z_index", Value::I64(-80));
        let hud_tl = tree.add_node(tldock, "hud_tl", NodeKind::ListView);
        let _ = tree.set_prop(hud_tl, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(hud_tl, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(MARGIN + 2.0, 219.0)));
        let _ = tree.set_prop(hud_tl, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(600.0, TL_LIST_H)));
        let _ = tree.set_prop(hud_tl, "rows", Value::Str(String::new()));
        let _ = tree.set_prop(hud_tl, "row_h", Value::I64(DOCK_ROW_H as i64));
        tree.set_prop_raw(hud_tl, "z_index", Value::I64(-80));
        // 进度细条池：行 i 的下沿 = 列表顶 +4 + i×18 + (18-2)。visible=false
        // 备用（每帧按 tween_rows 投影布线，照网格条带池纪律）。
        let mut tl_bars = Vec::with_capacity(TL_BARS);
        for _ in 0..TL_BARS {
            let bar = tree.add_node(tldock, "tl_bar", NodeKind::Control);
            let _ = tree.set_prop(bar, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(bar, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(bar, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(1.0, 2.0)));
            let _ = tree.set_prop(bar, "fill_slot", Value::Str(SLOT_SELECTED_NAME.into()));
            let _ = tree.set_prop(bar, "visible", Value::Bool(false));
            tree.set_prop_raw(bar, "z_index", Value::I64(-79));
            tl_bars.push(bar);
        }
        // 创建控制行（行 y 由布局投影每帧重写；x 恒定 —— TL_CTL_LAYOUT
        // 单点出表）。三枚说明标签（text_dim 色）+ 六按钮（九宫格底板 ×6
        // 同工具栏口径）+ 三输入框。
        let tl_new_label = tree.add_node(tldock, "tl_new_label", NodeKind::Label);
        tree.set_local(tl_new_label, Transform2D::from_pos(-1000.0, -1000.0));
        let _ = tree.set_prop(tl_new_label, PROP_LABEL_TEXT, Value::Str("NEW:".into()));
        let _ = tree.set_prop(tl_new_label, "font_size", Value::I64(UI_FONT_SIZE));
        let _ = tree.set_prop(tl_new_label, "color_slot", Value::Str(SLOT_TEXT_DIM_NAME.into()));
        let tl_to_label = tree.add_node(tldock, "tl_to_label", NodeKind::Label);
        tree.set_local(tl_to_label, Transform2D::from_pos(-1000.0, -1000.0));
        let _ = tree.set_prop(tl_to_label, PROP_LABEL_TEXT, Value::Str("to=".into()));
        let _ = tree.set_prop(tl_to_label, "font_size", Value::I64(UI_FONT_SIZE));
        let _ = tree.set_prop(tl_to_label, "color_slot", Value::Str(SLOT_TEXT_DIM_NAME.into()));
        let tl_ms_label = tree.add_node(tldock, "tl_ms_label", NodeKind::Label);
        tree.set_local(tl_ms_label, Transform2D::from_pos(-1000.0, -1000.0));
        let _ = tree.set_prop(tl_ms_label, PROP_LABEL_TEXT, Value::Str("ms=".into()));
        let _ = tree.set_prop(tl_ms_label, "font_size", Value::I64(UI_FONT_SIZE));
        let _ = tree.set_prop(tl_ms_label, "color_slot", Value::Str(SLOT_TEXT_DIM_NAME.into()));
        let mut tl_plates = Vec::with_capacity(6);
        for &(px, pw) in TL_CTL_LAYOUT.iter().take(6) {
            let plate = tree.add_node(tldock, "tl_plate", NodeKind::Control);
            let _ = tree.set_prop(plate, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(plate, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(TL_CTL_X + px, 280.0)));
            let _ = tree.set_prop(plate, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(pw, TOOLBAR_BTN_H)));
            let _ = tree.set_prop(plate, "fill_slot", Value::Str(SLOT_PANEL_NAME.into()));
            skin_button(tree, plate);
            tree.set_prop_raw(plate, "z_index", Value::I64(-79));
            tl_plates.push(plate);
        }
        let mk_tl_btn = |tree: &mut nes_scene::SceneTree, name: &str, i: usize| {
            let (px, pw) = TL_CTL_LAYOUT[i];
            let b = tree.add_node(tldock, name, NodeKind::Button);
            let _ = tree.set_prop(b, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(b, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(TL_CTL_X + px, 280.0)));
            let _ = tree.set_prop(b, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(pw, TOOLBAR_BTN_H)));
            let _ = tree.set_prop(b, "text", Value::Str(String::new()));
            // 字号 14 与工具栏同源；fill_slot 置空 = 透明底，九宫格底板
            // 纹理透出（S18 换肤口径，hover/pressed 四态照常叠加）。
            tree.set_prop_raw(b, "font_size", Value::I64(UI_FONT_SIZE));
            tree.set_prop_raw(b, "fill_slot", Value::Str(String::new()));
            b
        };
        let tl_pos = mk_tl_btn(tree, "tl_pos", 0);
        let tl_scale = mk_tl_btn(tree, "tl_scale", 1);
        let tl_alpha = mk_tl_btn(tree, "tl_alpha", 2);
        let tl_ease = mk_tl_btn(tree, "tl_ease", 6);
        let tl_mode = mk_tl_btn(tree, "tl_mode", 7);
        let tl_apply = mk_tl_btn(tree, "tl_apply", 8);
        let mk_tl_input = |tree: &mut nes_scene::SceneTree, name: &str, i: usize, init: &str| {
            let (px, pw) = TL_CTL_LAYOUT[i];
            let n = tree.add_node(tldock, name, NodeKind::TextInput);
            let _ = tree.set_prop(n, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(n, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(TL_CTL_X + px, 280.0)));
            let _ = tree.set_prop(n, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(pw, TOOLBAR_BTN_H)));
            let _ = tree.set_prop(n, "text", Value::Str(init.to_string()));
            // 字号 14（S12-11 前向通道 —— schema 无该键，提取层读属性缺省 16）。
            tree.set_prop_raw(n, "font_size", Value::I64(UI_FONT_SIZE));
            n
        };
        let tl_x_in = mk_tl_input(tree, "tl_x_in", 3, "0");
        let tl_y_in = mk_tl_input(tree, "tl_y_in", 4, "0");
        let tl_ms_in = mk_tl_input(tree, "tl_ms_in", 5, "500");
        // 视口工具栏（S12-7/F-4，Godot 2D 视口顶部工具条观感）：标尺
        // 之上一条 24px 工具带 —— panel 槽铺底 + 底缘 1px border 分隔
        // 线 + SEL/SNAP/GRID 三个开关按钮（UiVm on_activate 已通）。
        // 挂 "toolbar" 容器：walk 整子树跳过（工具观感不是可编辑对象，
        // 不进行列表）。z=-70 垫底（网格 -100、标尺 -90、dock -80 之上，
        // 仍在精灵 0 之下 —— 场景对象优先于观感，同款纪律）。
        let toolbar = tree.add_node(root, "toolbar", NodeKind::Node);
        let tool_bg = tree.add_node(toolbar, "tool_bg", NodeKind::Control);
        let _ = tree.set_prop(tool_bg, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_bg, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(188.0, TOP_BAND)));
        let _ = tree.set_prop(tool_bg, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(380.0, TOOLBAR_H)));
        let _ = tree.set_prop(tool_bg, "fill_slot", Value::Str(SLOT_PANEL_NAME.into()));
        tree.set_prop_raw(tool_bg, "z_index", Value::I64(-70));
        let tool_sep = tree.add_node(toolbar, "tool_sep", NodeKind::Control);
        let _ = tree.set_prop(tool_sep, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_sep, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(188.0, TOP_BAND + TOOLBAR_H - 1.0)));
        let _ = tree.set_prop(tool_sep, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(380.0, 1.0)));
        let _ = tree.set_prop(tool_sep, "fill_slot", Value::Str(SLOT_BORDER_NAME.into()));
        tree.set_prop_raw(tool_sep, "z_index", Value::I64(-70));
        // S18 换肤：按钮九宫格底板 × 6（与六按钮一一配对）。egui
        // `weak_bg_fill`/`bg_fill` 区分（DESIGN-NOTES §1.4）的壳层版：
        // 底板有底（按钮皮肤纹理，绝对色 bevel-up），按钮本体 fill 走
        // 透明（fill_slot 置空串 —— themed/button 槽解析对空名不覆盖，
        // ControlState 缺省透明），纹理从按钮矩形里透出；hover/pressed
        // 的 accent 换档是提取层既有四态（按钮矩形画在底板之上，同 z
        // 下前序序先画 —— 底板先建垫底），按下时 accent 填充盖过纹理
        // = 强反馈。三态观感：正常 = 纹理 + border 槽框；悬停 = 纹理 +
        // accent 框；按下 = accent 填充 + accent 框。offset 装配期占位，
        // 每帧布局投影随按钮同步重写（见循环内 tool_btns/tool_plates）。
        let mut tool_plates = Vec::with_capacity(6);
        for i in 0..6 {
            let plate = tree.add_node(toolbar, "tool_plate", NodeKind::Control);
            let _ = tree.set_prop(plate, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
            let _ = tree.set_prop(plate, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(
                192.0 + i as f32 * TOOLBAR_BTN_STEP,
                TOP_BAND + 2.0,
            )));
            let _ = tree.set_prop(plate, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(TOOLBAR_BTN_W, TOOLBAR_BTN_H)));
            let _ = tree.set_prop(plate, "fill_slot", Value::Str(SLOT_PANEL_NAME.into()));
            skin_button(tree, plate);
            tree.set_prop_raw(plate, "z_index", Value::I64(-70));
            tool_plates.push(plate);
        }
        let tool_sel = tree.add_node(toolbar, "tool_sel", NodeKind::Button);
        let _ = tree.set_prop(tool_sel, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_sel, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(192.0, TOP_BAND + 2.0)));
        let _ = tree.set_prop(tool_sel, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(TOOLBAR_BTN_W, TOOLBAR_BTN_H)));
        let _ = tree.set_prop(tool_sel, "text", Value::Str("SEL".into()));
        // S12-11：按钮字号 14（16px 真字体下 "RESET" 溢出 48px 按钮宽；见 UI_FONT_SIZE 注）。
        tree.set_prop_raw(tool_sel, "font_size", Value::I64(UI_FONT_SIZE));
        // S18：fill_slot 置空 = 按钮本体透明底（槽解析对空名不覆盖，
        // ControlState 缺省透明），九宫格底板纹理透出；hover/pressed 的
        // accent 换档是提取层既有四态，照常叠加（按下填充盖过纹理）。
        // 五键同款，注释唯一。
        tree.set_prop_raw(tool_sel, "fill_slot", Value::Str(String::new()));
        let tool_snap = tree.add_node(toolbar, "tool_snap", NodeKind::Button);
        let _ = tree.set_prop(tool_snap, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_snap, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(192.0 + TOOLBAR_BTN_STEP, TOP_BAND + 2.0)));
        let _ = tree.set_prop(tool_snap, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(TOOLBAR_BTN_W, TOOLBAR_BTN_H)));
        let _ = tree.set_prop(tool_snap, "text", Value::Str("SNAP".into()));
        // S12-11：按钮字号 14（16px 真字体下 "RESET" 溢出 48px 按钮宽；见 UI_FONT_SIZE 注）。
        tree.set_prop_raw(tool_snap, "font_size", Value::I64(UI_FONT_SIZE));
        tree.set_prop_raw(tool_snap, "fill_slot", Value::Str(String::new()));
        let tool_grid = tree.add_node(toolbar, "tool_grid", NodeKind::Button);
        let _ = tree.set_prop(tool_grid, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_grid, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(192.0 + 2.0 * TOOLBAR_BTN_STEP, TOP_BAND + 2.0)));
        let _ = tree.set_prop(tool_grid, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(TOOLBAR_BTN_W, TOOLBAR_BTN_H)));
        let _ = tree.set_prop(tool_grid, "text", Value::Str("GRID".into()));
        // S12-11：按钮字号 14（16px 真字体下 "RESET" 溢出 48px 按钮宽；见 UI_FONT_SIZE 注）。
        tree.set_prop_raw(tool_grid, "font_size", Value::I64(UI_FONT_SIZE));
        tree.set_prop_raw(tool_grid, "fill_slot", Value::Str(String::new()));
        // S12-9：PLAY / STOP / RESET（Godot 视口工具栏右上角的运行三键
        // 直感，P0 摆在编辑三键右侧同一工具带）。文本投影每帧重写
        //（PLAY 运行中带 * 后缀），offset 装配期占位、每帧布局投影重写。
        let tool_play = tree.add_node(toolbar, "tool_play", NodeKind::Button);
        let _ = tree.set_prop(tool_play, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_play, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(192.0 + 3.0 * TOOLBAR_BTN_STEP, TOP_BAND + 2.0)));
        let _ = tree.set_prop(tool_play, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(TOOLBAR_BTN_W, TOOLBAR_BTN_H)));
        let _ = tree.set_prop(tool_play, "text", Value::Str("PLAY".into()));
        // S12-11：按钮字号 14（16px 真字体下 "RESET" 溢出 48px 按钮宽；见 UI_FONT_SIZE 注）。
        tree.set_prop_raw(tool_play, "font_size", Value::I64(UI_FONT_SIZE));
        tree.set_prop_raw(tool_play, "fill_slot", Value::Str(String::new()));
        let tool_stop = tree.add_node(toolbar, "tool_stop", NodeKind::Button);
        let _ = tree.set_prop(tool_stop, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_stop, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(192.0 + 4.0 * TOOLBAR_BTN_STEP, TOP_BAND + 2.0)));
        let _ = tree.set_prop(tool_stop, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(TOOLBAR_BTN_W, TOOLBAR_BTN_H)));
        let _ = tree.set_prop(tool_stop, "text", Value::Str("STOP".into()));
        // S12-11：按钮字号 14（16px 真字体下 "RESET" 溢出 48px 按钮宽；见 UI_FONT_SIZE 注）。
        tree.set_prop_raw(tool_stop, "font_size", Value::I64(UI_FONT_SIZE));
        tree.set_prop_raw(tool_stop, "fill_slot", Value::Str(String::new()));
        let tool_reset = tree.add_node(toolbar, "tool_reset", NodeKind::Button);
        let _ = tree.set_prop(tool_reset, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_reset, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(192.0 + 5.0 * TOOLBAR_BTN_STEP, TOP_BAND + 2.0)));
        let _ = tree.set_prop(tool_reset, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(TOOLBAR_BTN_W, TOOLBAR_BTN_H)));
        let _ = tree.set_prop(tool_reset, "text", Value::Str("RESET".into()));
        // S12-11：按钮字号 14（16px 真字体下 "RESET" 溢出 48px 按钮宽；见 UI_FONT_SIZE 注）。
        tree.set_prop_raw(tool_reset, "font_size", Value::I64(UI_FONT_SIZE));
        // S18：fill_slot 置空 = 按钮本体透明底（槽解析对空名不覆盖），
        // 九宫格底板纹理透出（hover/pressed 的 accent 换档照常叠加）。
        tree.set_prop_raw(tool_reset, "fill_slot", Value::Str(String::new()));
        let cam = tree.add_node(root, "cam", NodeKind::Camera2D);
        tree.set_local(cam, Transform2D::from_pos(384.0, 216.0));
        // 演示对象 y=130（S18.1 起：时间轴 dock 让走了下方 ~110px ——
        // 对象留在缩小后视口带内（80..dock 上缘），不再压进时间轴面板）。
        let obj1 = tree.add_node(root, "obj1", NodeKind::Sprite2D);
        tree.set_prop(obj1, PROP_TEXTURE, Value::Resource(1)).unwrap();
        tree.set_local(obj1, Transform2D::from_pos(280.0, 130.0));
        let obj2 = tree.add_node(root, "obj2", NodeKind::Sprite2D);
        tree.set_prop(obj2, PROP_TEXTURE, Value::Resource(2)).unwrap();
        tree.set_local(obj2, Transform2D::from_pos(380.0, 130.0));
        let obj3 = tree.add_node(root, "obj3", NodeKind::Sprite2D);
        tree.set_prop(obj3, PROP_TEXTURE, Value::Resource(3)).unwrap();
        tree.set_local(obj3, Transform2D::from_pos(480.0, 130.0));
        // Hierarchy 面板（S12-3 ListView 真消费者）：视口锚定控件，
        // 行文本 `rows` 与选中下标 `selected` 由宿主每帧投影（树是
        // 投影不是语义来源），行点击与滚轮滚动由 UiVm 驱动（宿主零
        // 滚动接线 —— scrolls 是 UiVm 瞬态）。size 每帧按客户区重写
        //（S12-4 自适应：高度 = ch-64，宽恒 180）。
        let hud_tree = tree.add_node(root, "hud_tree", NodeKind::ListView);
        tree.set_prop(hud_tree, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::new(0.0, 0.0))).unwrap();
        tree.set_prop(hud_tree, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(MARGIN, TOP_BAND))).unwrap();
        tree.set_prop(hud_tree, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, 360.0))).unwrap();
        tree.set_prop(hud_tree, "rows", Value::Str(String::new())).unwrap();
        tree.set_prop(hud_tree, "row_h", Value::I64(18)).unwrap();
        // Inspector 面板底（S12-4 工作区分离）：panel 槽铺底的裸
        // Control —— 与左面板（ListView 自带 panel 填充）同槽位区分
        // 中间视口。offset/size 每帧按客户区重写；裸 Control 不参与
        // 自动裁剪（S12-3 D6），铺底矩形不裁任何东西。
        let hud_ins_bg = tree.add_node(root, "hud_ins_bg", NodeKind::Control);
        tree.set_prop(hud_ins_bg, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::new(0.0, 0.0))).unwrap();
        tree.set_prop(hud_ins_bg, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(570.0, 8.0))).unwrap();
        tree.set_prop(hud_ins_bg, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(INSPECTOR_W, 400.0))).unwrap();
        tree.set_prop(hud_ins_bg, "fill_slot", Value::Str(SLOT_PANEL_NAME.into())).unwrap();
        // S18 换肤：Inspector 面板九宫格皮肤（同 dock_bg 口径）。
        skin_panel(tree, hud_ins_bg);
        // Inspector 标题 + 选中信息（短文本：标题一行 + 信息另起，
        // "(none)" = 无选中）。位置每帧投影（x = cw-190, y = 12，跟随
        // 右面板）—— 修 S12-4 ⑤"标题被表面边缘裁剪"。
        let hud_ins = tree.add_node(root, "hud_ins", NodeKind::Label);
        tree.set_local(hud_ins, Transform2D::from_pos(578.0, 12.0));
        tree.set_prop(hud_ins, PROP_LABEL_TEXT, Value::Str(String::new())).unwrap();
        let _ = tree.set_prop(hud_ins, "font_size", Value::I64(UI_FONT_SIZE));
        // 状态栏。
        // Selection indicator (Control border following primary selection).
        // S12-5 Godot 化：边框换 accent 槽（Godot 2D 选中的浅蓝高亮）；
        // 线宽 2px 经 set_prop_raw 写 border_w（S12-1 契约字段，schema 暂
        // 未暴露该键 —— 提取层 control_state_of 已直读属性表，未写 = 缺省
        // 1px）；z_index=100（同 set_prop_raw 通道）压过选中精灵的高亮
        // z=5，选中框永远在最上层。
        let sel_box = tree.add_node(root, "sel_box", NodeKind::Control);
        tree.set_prop(sel_box, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::new(0.0, 0.0))).unwrap();
        tree.set_prop(sel_box, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(-100.0, -100.0))).unwrap();
        tree.set_prop(sel_box, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(20.0, 20.0))).unwrap();
        tree.set_prop(sel_box, "border_slot", Value::Str(SLOT_ACCENT_NAME.into())).unwrap();
        tree.set_prop_raw(sel_box, "border_w", Value::F32(2.0));
        tree.set_prop_raw(sel_box, "z_index", Value::I64(100));

        // 左面板标题（S12-5 Godot 命名）：与右侧 Inspector 标题同款 Label。
        // 左面板 x 恒定（MARGIN），位置装配期一次写定即可，无需每帧投影。
        let hud_scene = tree.add_node(root, "hud_scene", NodeKind::Label);
        tree.set_local(hud_scene, Transform2D::from_pos(MARGIN + 2.0, 12.0));
        tree.set_prop(hud_scene, PROP_LABEL_TEXT, Value::Str("Scene".into())).unwrap();
        let _ = tree.set_prop(hud_scene, "font_size", Value::I64(UI_FONT_SIZE));

        let hud_st = tree.add_node(root, "hud_st", NodeKind::Label);
        tree.set_local(hud_st, Transform2D::from_pos(8.0, 410.0));
        tree.set_prop(hud_st, PROP_LABEL_TEXT, Value::Str(String::new())).unwrap();
        let _ = tree.set_prop(hud_st, "font_size", Value::I64(UI_FONT_SIZE));
        // Inspector 的节点重命名输入框（S12-2 TextInput —— 视口锚定，
        // 与 UiVm 命中/焦点路由同一口径）。选中节点时显示并绑定其名字。
        // S12-6 根修"改名框浮在网格上"：offset 不再是装配期写死的旧值
        // —— 位置/宽度由布局投影块每帧重写（右面板内、Inspector 标题
        // 与属性行下方的固定槽位，窗口一变就跟手）。装配期只给初值。
        let name_input = tree.add_node(root, "name_input", NodeKind::TextInput);
        tree.set_prop(name_input, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::new(0.0, 0.0))).unwrap();
        tree.set_prop(name_input, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(576.0, 112.0))).unwrap();
        tree.set_prop(name_input, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(INSPECTOR_W - 2.0 * INSPECTOR_INSET, 20.0))).unwrap();
        tree.set_prop(name_input, "text", Value::Str(String::new())).unwrap();
        tree.set_prop(name_input, "visible", Value::Bool(false)).unwrap();
        // 字号 14 与面板文字同源（S12-11）：TextInput schema 无 font_size
        // 键，走 set_prop_raw 前向通道（z_index/border_w 先例）；提取层
        // TextInput 读该属性、缺省 16 逐位不变。IME 锚点累加同字号（见
        // ime_caret_offset 注 —— 同字体同字号才是"精确"的判据）。
        tree.set_prop_raw(name_input, "font_size", Value::I64(UI_FONT_SIZE));
        // Inspector 分区标题（S12-7 Godot 分组观感）：text_dim 色小节
        // 标题，"+" 折叠 / "-" 展开；点击标题行（宿主矩形命中）或 F7
        // 切换折叠。位置/文本每帧投影（跟随右面板与组布局）。
        // ins_script 是 Script 分区正文（mount/unmount/enabled 行）。
        let ins_tf_title = tree.add_node(root, "ins_tf_title", NodeKind::Label);
        tree.set_local(ins_tf_title, Transform2D::from_pos(-1000.0, -1000.0));
        let _ = tree.set_prop(ins_tf_title, PROP_LABEL_TEXT, Value::Str("- Transform".into()));
        let _ = tree.set_prop(ins_tf_title, "font_size", Value::I64(UI_FONT_SIZE));
        let _ = tree.set_prop(ins_tf_title, "color_slot", Value::Str(SLOT_TEXT_DIM_NAME.into()));
        let ins_sc_title = tree.add_node(root, "ins_sc_title", NodeKind::Label);
        tree.set_local(ins_sc_title, Transform2D::from_pos(-1000.0, -1000.0));
        let _ = tree.set_prop(ins_sc_title, PROP_LABEL_TEXT, Value::Str("- Script".into()));
        let _ = tree.set_prop(ins_sc_title, "font_size", Value::I64(UI_FONT_SIZE));
        let _ = tree.set_prop(ins_sc_title, "color_slot", Value::Str(SLOT_TEXT_DIM_NAME.into()));
        let ins_script = tree.add_node(root, "ins_script", NodeKind::Label);
        tree.set_local(ins_script, Transform2D::from_pos(-1000.0, -1000.0));
        let _ = tree.set_prop(ins_script, PROP_LABEL_TEXT, Value::Str(String::new()));
        let _ = tree.set_prop(ins_script, "font_size", Value::I64(UI_FONT_SIZE));
        tree.apply_pending();
        (grid, grid_bars, ruler, ruler_h, ruler_v, ruler_corner, ruler_ticks, ruler_labels, dock, dock_bg, dock_title, hud_dock, toolbar, tool_bg, tool_sep, theme_node, tool_plates, tool_sel, tool_snap, tool_grid, tool_play, tool_stop, tool_reset, ins_tf_title, ins_sc_title, ins_script, cam, obj1, obj2, obj3, hud_tree, hud_ins_bg, hud_ins, hud_st, sel_box, name_input, hud_scene, fsdock, fs_bg, fs_title, fs_sep, fs_tree, tldock, tl_bg, tl_title, hud_tl, tl_bars, tl_new_label, tl_to_label, tl_ms_label, tl_plates, tl_pos, tl_scale, tl_alpha, tl_ease, tl_mode, tl_apply, tl_x_in, tl_y_in, tl_ms_in)
    };
    let _ = (obj1, obj2, obj3);

    // 编辑器状态（会话态 —— 不进事务、不落盘）。
    let mut sel = Selection::new();
    let mut log = TransactionLog::new();
    // S12-9：运行会话态（PLAY/STOP/RESET）。编辑态的帧循环观察者是
    // NoObserver —— 既有 editor ScriptVm 空转观察者退役（它从未装载过
    // 脚本，语义与 NoObserver 等价；输入读面在装配处已挂 UiVm）。
    let mut play = PlaySession::new();
    // 初始选择第一个对象。
    if let Some(uid) = rt.tree_mut().uid_of(obj1) {
        sel.select(uid);
    }

    // F-4 脚本挂载与编辑器会话态（不进树、不进指纹）：
    // - 候选池 = Scripts/*.nes（资产根相对路径）+ F6 轮换下标；
    // - 分组折叠 stage：bit0 = Transform 折叠、bit1 = Script 折叠
    //   （F7 循环 0..=3，点组标题翻对应位）；
    // - 工具栏三开关（SEL 选择/拖拽总开关、SNAP 恒吸附、GRID 网格）；
    // - 分区标题行矩形（上一帧投影产出 -> 帧首命中，一帧滞后与既有
    //   UI 命中同口径）：(面板左 x, 行顶 y, 组下标)。
    // 文件系统 dock 数据面（S12-8）：资产条目（递归扫描，每 60 帧 +
    // F5 刷新）与脚本候选池**同源派生**（script_pool —— FileSystem
    // 选中的 .nes 必在池内，F6 候选起点裁决的前提）。
    let mut fs_entries: Vec<FsEntry> = scan_assets(&assets);
    let mut scripts: Vec<String> = script_pool(&fs_entries);
    let mut script_idx: usize = 0;
    let mut group_stage: u8 = 0;
    let mut tool_sel_on = true;
    let mut tool_snap_on = false;
    let mut tool_grid_on = true;
    let mut title_rows: Vec<(f32, f32, usize)> = Vec::new();
    // 文件系统 dock 会话态（S12-8，不进树、不落盘）：选中行（None =
    // 无选中，投影 -1）、F9 两档分割的焦点段（false = Scene 占大头）、
    // 双击合成的上次行点击 (帧号, 行)。
    let mut fs_sel: Option<usize> = None;
    let mut fs_focus = false;
    let mut fs_last_press: Option<(u64, usize)> = None;
    // 时间轴 dock 会话态（S18.1，不进树、不落盘）：通道/缓动/模式循环
    // 档下标（创建控制行的循环按钮现态）+ 三个输入框的已提交值（会话
    // 值 —— APPLY 落地取这里，输入框 text 属性只管显示）。
    let mut tl_channel: usize = 0; // 0 = POS / 1 = SCALE / 2 = ALPHA
    let mut tl_ease_idx: usize = 0; // TL_EASINGS 档序（缺省 linear = 脚本面缺省）
    let mut tl_mode_idx: usize = 0; // TL_MODES 档序（缺省 once = 脚本面缺省）
    let mut tl_x = String::from("0");
    let mut tl_y = String::from("0");
    let mut tl_ms = String::from("500");

    // 状态栏的 undo/redo 键按下沿检测。
    let mut prev_z = false;
    let mut prev_y = false;
    let mut prev_del = false;
    let mut prev_tab = false;
    let mut prev_click = false;
    // 框选拖拽状态（编辑器会话态 —— 不进事务/不落盘）。
    let mut drag_start: Option<(f32, f32)> = None;
    // Gizmo 拖拽（选中的对象直接拖动移动）：(uid, 鼠标偏移)。
    let mut gizmo: Option<(Uid, f32, f32)> = None;
    // 重命名输入框的绑定（会话态）：当前 text 属性投影的是哪个选中节点。
    let mut bound_sel: Option<Uid> = None;
    // 编辑器日志环形缓冲（Output dock 的数据面，会话态不落盘）：
    // undo/redo/选择/删除/改名/拖移在各自落账点推一行，投影块每帧
    // 把最近几行写进 dock 的 ListView。
    let editor_log: Rc<RefCell<VecDeque<String>>> =
        Rc::new(RefCell::new(VecDeque::with_capacity(EDITOR_LOG_KEEP)));
    log_line(&editor_log, "editor ready".into());
    // S17.4 宿主惯例（与 first_game/dungeon_game 同一款）：启动时全装载
    // 资产根 `Extensions/*.js`（字典序 = 确定装载序）。编辑器里扩展照跑
    // —— 生态面 = play-in-editor：update 推进只在运行态（见帧循环），
    // 编辑态不推进（扩展写树属运行期改动；编辑期写会绕过事务污染编辑
    // 会话）。装载失败只记一行不中断 —— 一个坏扩展不挡编辑器。
    {
        let ext_dir = assets.join("Extensions");
        if let Ok(entries) = std::fs::read_dir(&ext_dir) {
            let mut files: Vec<std::path::PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("js"))
                .collect();
            files.sort();
            for f in files {
                match rt.load_extension_file(&f) {
                    Ok(id) => {
                        println!("[扩展] 已装载 {id} <- {}", f.display());
                        log_line(&editor_log, format!("ext loaded {id}"));
                    }
                    Err(e) => {
                        eprintln!("[扩展] 装载失败（{}）：{e} —— 跳过", f.display());
                        log_line(
                            &editor_log,
                            format!(
                                "ext load failed: {}",
                                base_name(&f.to_string_lossy())
                            ),
                        );
                    }
                }
            }
        }
    }
    // 真字体默认字体装载（S12-11 壳层①，见模块头与 FONT_CANDIDATES 注）：
    // 按优先级探测，第一个可读且可解析的经 `set_ttf_default` 装载 —— 此后
    // font==NIL 文本（全部 Label/输入框/按钮）自动走真字体动态字形图集排
    // 版。全部缺失/解析失败 = 位图回退，只记一行 Output、不 panic（优雅
    // 回退契约；位图默认字体已在上方登记，回退路径永远可用）。每次尝试
    // 都落一行（命中/失败/回退），冒烟断言按行取证。
    let mut ime_font: Option<TtfFont> = None;
    let mut ttf_active = false;
    for path in FONT_CANDIDATES {
        let Ok(data) = std::fs::read(path) else {
            continue; // 读不到（不存在/无权限）：静默试下一个候选。
        };
        match rt.consumer_mut().expect("GPU 消费器").set_ttf_default(&data) {
            Ok(()) => {
                ttf_active = true;
                // IME 锚点累加器持同一份字体数据的独立 TtfFont 实例（重复
                // 解析一次可接受 —— 解析器纯 CPU 无共享状态；不共享是刻意
                // 的：消费器在 rt 内部，取出实例要跨所有权，抄一份字节再
                // parse 最省事，见模块头③）。
                ime_font = TtfFont::parse(&data).ok();
                log_line(&editor_log, format!("font: {} (ttf)", base_name(path)));
                break;
            }
            Err(err) => {
                // 解析失败（损坏/截断的字体文件）：如实记行，继续下一个
                // 候选 —— 探测链的意义就是单点失败不致命。
                log_line(
                    &editor_log,
                    format!("font: {} ttf parse failed ({err})", base_name(path)),
                );
            }
        }
    }
    if !ttf_active {
        log_line(&editor_log, "font: bitmap fallback (no system font)".into());
    }
    // 用户实测音乐装载（S14，nes-media 适配层实测链路；缺失只静默跳过
    // —— 文件不在仓库，CI/他机安全；解码失败记一行不阻塞 —— 带病也能
    // 跑的既有口径）。music_tracks = 实际装载成功的 (混音器键, ASCII 标签)
    // 表，数字键 0 的三态循环按它轮换；music_state 是编辑器会话态。
    let mut music_tracks: Vec<(&str, &str)> = Vec::new();
    for (file_name, fmt_label, key) in [
        (MUSIC_FLAC_NAME, "flac", "music"),
        (MUSIC_MP3_NAME, "mp3", "music2"),
    ] {
        if let Some(line) = load_user_music(&mut rt, file_name, fmt_label, key, &editor_log) {
            log_line(&editor_log, line);
            music_tracks.push((key, fmt_label));
        }
    }
    let mut music_state: usize = 0; // 0 = 停；1..=len = music_tracks[i-1] 在播
    // UiVm 提交钩子的落点（UiVm 零写权 —— 值经共享缓冲传回宿主，
    // 宿主帧后落 Inspector::modify_name 一条 Modified 事务）。
    let rename_sink: Rc<RefCell<Vec<(Uid, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let rename_bound: Rc<RefCell<Option<Uid>>> = Rc::new(RefCell::new(None));
    // 时间轴输入框提交落点（S18.1，UiVm 零写权延续 —— 帧内只报
    // (字段号, 值)，帧后宿主落会话值 + text 属性双写）。
    let tl_input_sinks: Rc<RefCell<Vec<(usize, String)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let sink = rename_sink.clone();
        let bound = rename_bound.clone();
        let tl_sink = tl_input_sinks.clone();
        let tl_map: std::collections::BTreeMap<nes_scene::NodeId, usize> = [
            (tl_x_in, 0usize),
            (tl_y_in, 1),
            (tl_ms_in, 2),
        ]
        .into_iter()
        .collect();
        rt.ui_vm_mut().on_commit(move |node, value| {
            if let Value::Str(s) = value {
                // 时间轴输入框优先分派（字段号带回 —— 不进改名落账面）。
                if let Some(f) = tl_map.get(&node) {
                    tl_sink.borrow_mut().push((*f, s));
                    return;
                }
                if let Some(uid) = bound.borrow().clone() {
                    sink.borrow_mut().push((uid, s));
                }
            }
        });
    }
    // 层级树行点击的落点（S12-3，UiVm 零写权延续 —— 回调在帧内只报
    // (节点, 行下标)，经共享缓冲传回宿主，帧后落 Selection）。行→节点
    // 映射由投影段每帧整体刷新（walk 顺序即行序），回调只按行查 uid。
    let row_clicks: Rc<RefCell<Vec<Uid>>> = Rc::new(RefCell::new(Vec::new()));
    let row_map_shared: Rc<RefCell<Vec<Uid>>> = Rc::new(RefCell::new(Vec::new()));
    // 文件系统 dock 行点击落点（S12-8，UiVm 零写权延续 —— 帧内只报
    // 行下标，帧后宿主结算选中/双击分派；与层级树同一共享缓冲模式）。
    let fs_clicks: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let clicks = row_clicks.clone();
        let map = row_map_shared.clone();
        let fs_sink = fs_clicks.clone();
        rt.ui_vm_mut().on_row_activate(move |node, row| {
            if node == hud_tree {
                if let Some(uid) = map.borrow().get(row as usize) {
                    clicks.borrow_mut().push(uid.clone());
                }
            } else if node == fs_tree {
                fs_sink.borrow_mut().push(row as usize);
            }
        });
    }
    // 工具栏 + 时间轴按钮激活落点（UiVm 零写权延续 —— 帧内报按钮名，帧后
    // 宿主翻开关/落创建控制；与行点击同一共享缓冲模式）。时间轴按钮名带
    // tl_ 前缀（落账段按前缀分流，见循环尾）。
    let tool_clicks: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let names: std::collections::BTreeMap<nes_scene::NodeId, &str> = [
            (tool_sel, "sel"),
            (tool_snap, "snap"),
            (tool_grid, "grid"),
            (tool_play, "play"),
            (tool_stop, "stop"),
            (tool_reset, "reset"),
            (tl_pos, "tl_pos"),
            (tl_scale, "tl_scale"),
            (tl_alpha, "tl_alpha"),
            (tl_ease, "tl_ease"),
            (tl_mode, "tl_mode"),
            (tl_apply, "tl_apply"),
        ]
        .into_iter()
        .collect();
        let sink = tool_clicks.clone();
        rt.ui_vm_mut().on_activate(move |btn| {
            if let Some(n) = names.get(&btn) {
                sink.borrow_mut().push((*n).to_string());
            }
        });
    }

    let total: u64 = std::env::var("NES_GAME_FRAMES")
        .or_else(|_| std::env::var("NES_EDIT_FRAMES"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
    // 自动化钩子（S12-7，script_panel 的 NES_PANEL_* 同款注入模式）：
    // NES_EDIT_DEMO=1 时按固定帧号向平台队列注入键盘流（与真实消息
    // 同一 inject_input 通道）—— 冒烟不再只是"空转 120 帧干净退出"，
    // 而是实走 挂载/卸载/enabled/折叠/刷新 全链路，退出时断言 Output
    // 日志与树的最终形态。默认关闭：正常运行零注入零断言。
    let demo = std::env::var("NES_EDIT_DEMO").ok().as_deref() == Some("1");
    let mut transient = 0u64;
    const TRANSIENT_LIMIT: u64 = 120;
    // 运行态取证（S12-9 冒烟钩子，帧 176 采样 —— 见循环内注）：
    // spin 节点位移（> 0 = 脚本在运行态真实驱动过）与工具栏 PLAY 文本
    //（运行中应为 "PLAY*"）。IME 第 1 期：帧 214 采样改名框草稿
    //（Char(0x4E2D) 注入后应为 "obj1中" —— Unicode 泵端到端取证）。
    let mut demo_spin_x = 0.0f32;
    let mut demo_play_text = String::new();
    let mut demo_ime_draft = String::new();
    // S18.1 时间轴取证：APPLY 后补间行投影文本（帧 288 采样）+ 两个与
    // 刷新率无关的闩锁（帧 ≥284 逐帧观察登记表长度：见过 >0 = 活动补间
    // 真实入表；其后见过 ==0 = 推进/到站移除真实发生 —— 时间基准是帧差
    // 累计的毫秒，固定帧号采样会随刷新率漂移，闩锁不会）。
    let mut demo_tl_rows = String::new();
    let mut demo_tl_seen_active = false;
    let mut demo_tl_seen_done = false;

    // 自适应口径（S12-4 ①）：视口 = 窗口真实客户区，每帧实测。最小化
    // /遮蔽帧客户区可暂为 (0,0)（表面也不可重配）—— 沿用上次有效值，
    // 布局与命中保持上一帧口径，窗口恢复后下一帧自动跟上。帧首
    // sync_surface_to_window（frame_windowed_with 内）与本读数同源：
    // 当帧表面尺寸 == 当帧视口 == 当帧布局基准。
    let mut last_client = OPEN_CLIENT;
    // 帧节拍（S12-4 ⑥）：实测帧差进 FrameInfo（旧代码固定
    // sleep(16ms) + FIFO present 双重等待 —— 延迟不跟手的根因之一）。
    // clamp ≤0.1s：切后台回来的一步大步长不进模拟。NES_GAME_FRAMES
    // 冒烟语义不变（帧数口径，非墙钟口径）。
    let mut last_frame = Instant::now();
    let mut elapsed = 0.0f64;

    for index in 0..total {
        let (raw_w, raw_h) = rt.window_client_size();
        let (cw_u, ch_u) = if raw_w == 0 || raw_h == 0 {
            last_client
        } else {
            (raw_w, raw_h)
        };
        last_client = (cw_u, ch_u);
        let viewport = (cw_u as f32, ch_u as f32);

        let now = Instant::now();
        let delta = (now - last_frame).as_secs_f32().min(0.1);
        last_frame = now;
        elapsed += delta as f64;

        if demo {
            // 同键连发必须隔一次 key_up：折叠器对已按住的键不重复闩锁
            // （自动重发幂等，T-In-C01 口径）—— 第二次 F7 down 前先抬键。
            match index {
                10 => inject_input(InputEvent::Key { key: Key::Other(VK_F6), down: true }),
                12 => inject_input(InputEvent::Key { key: Key::Other(VK_F6), down: false }),
                20 => inject_input(InputEvent::Key { key: Key::Enter, down: true }),
                22 => inject_input(InputEvent::Key { key: Key::Enter, down: false }),
                30 => inject_input(InputEvent::Key { key: Key::E, down: true }),
                32 => inject_input(InputEvent::Key { key: Key::E, down: false }),
                40 => inject_input(InputEvent::Key { key: Key::U, down: true }),
                42 => inject_input(InputEvent::Key { key: Key::U, down: false }),
                50 => inject_input(InputEvent::Key { key: Key::Other(VK_F7), down: true }),
                52 => inject_input(InputEvent::Key { key: Key::Other(VK_F7), down: false }),
                60 => inject_input(InputEvent::Key { key: Key::Other(VK_F7), down: true }),
                62 => inject_input(InputEvent::Key { key: Key::Other(VK_F7), down: false }),
                70 => inject_input(InputEvent::Key { key: Key::Other(VK_F8), down: true }),
                72 => inject_input(InputEvent::Key { key: Key::Other(VK_F8), down: false }),
                // S12-8：FileSystem 双击挂载 —— 鼠标先移到 fs 树
                // spin.nes 行（768x432 客户区）。S15 起 res:// 树多出
                // Media/（演示视频目录 + 条目两行）；S18.1 起时间轴 dock
                // 又让走 110px —— 左栏可用高 162，files 档 fs 列表仅
                // ~4.3 行可见（顶 y=123.2），spin.nes（Media 在场时行 7）
                // 在可见窗之外：先在 fs 树上滚轮下滚 4 格（ListView 步长
                // = row_h 18；scroll 钳到 scroll_max ≈ 73），spin 行落进
                // 窗内（屏上 y ≈ 181/180）再双击（两次点击沿间隔 <30 帧
                // = 双击裁决窗）。Media/ 缺席的机器保持默认档布局，行 5
                // 同法可达 —— 断言面（fs open / mount 行）两种形态都成立。
                74 if video_present => inject_input(InputEvent::Key { key: Key::Other(VK_F9), down: true }),
                76 if video_present => inject_input(InputEvent::Key { key: Key::Other(VK_F9), down: false }),
                78 => inject_input(InputEvent::MouseMove { x: 60.0, y: 160.0 }),
                80 => inject_input(InputEvent::Wheel { x: 0.0, y: -1.0 }),
                82 => inject_input(InputEvent::Wheel { x: 0.0, y: -1.0 }),
                84 => inject_input(InputEvent::Wheel { x: 0.0, y: -1.0 }),
                86 => inject_input(InputEvent::Wheel { x: 0.0, y: -1.0 }),
                88 => inject_input(InputEvent::MouseMove {
                    x: 60.0,
                    y: if video_present { 189.0 } else { 188.0 },
                }),
                90 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
                92 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
                96 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
                98 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
                100 => inject_input(InputEvent::Key { key: Key::U, down: true }),
                102 => inject_input(InputEvent::Key { key: Key::U, down: false }),
                // S12-9：play-in-editor 全链路 —— Enter 重挂 spin.nes
                //（上面 100 的 U 已卸载；fs 单击已把候选指回 spin.nes）
                //→ F5 PLAY → 跑约 56 帧（spin 脚本每帧右移 0.3）→
                // Shift+F5 STOP → 点工具栏 RESET（第 6 个按钮：768 宽
                // 客户区下 gx0=188，x = 188+4+5*52 = 452..500，取中
                // (476, 52)）→ U 卸载回空 registry_key（树形态断言兼容）。
                104 => inject_input(InputEvent::Key { key: Key::Enter, down: true }),
                106 => inject_input(InputEvent::Key { key: Key::Enter, down: false }),
                112 => inject_input(InputEvent::Key { key: Key::Other(VK_F5), down: true }),
                114 => inject_input(InputEvent::Key { key: Key::Other(VK_F5), down: false }),
                168 => inject_input(InputEvent::Key { key: Key::LShift, down: true }),
                170 => inject_input(InputEvent::Key { key: Key::Other(VK_F5), down: true }),
                172 => inject_input(InputEvent::Key { key: Key::Other(VK_F5), down: false }),
                174 => inject_input(InputEvent::Key { key: Key::LShift, down: false }),
                178 => inject_input(InputEvent::MouseMove { x: 476.0, y: 52.0 }),
                180 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
                182 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
                190 => inject_input(InputEvent::Key { key: Key::U, down: true }),
                192 => inject_input(InputEvent::Key { key: Key::U, down: false }),
                // IME 第 1 期冒烟（真人在改名框打中文留人工 —— 自动化只
                // 钉"Unicode 字符入草稿"链路）：点击改名输入框（768x432
                // 客户区、obj1 选中、Transform 组展开：S12-11 起行步进
                // INS_ROW_H=20，offset = (568,136) 尺寸 (178,20)，取
                // (600,145)）夺焦 → 注入 Char(0x4E2D)（'中'，走
                // inject_input 同队列通道 = WM_CHAR 直投口径，不经系统
                // IME 合成 —— 与环境键盘布局无关，t_in_01 同款确定性）
                // → 帧 214 取证草稿。
                200 => inject_input(InputEvent::MouseMove { x: 600.0, y: 145.0 }),
                202 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
                204 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
                210 => inject_input(InputEvent::Char(0x4E2D)),
                // S14：数字键 0 三态音乐循环取证 —— 先 Esc 回滚改名草稿
                //（帧 210 注入的 '中' 还在草稿里；失焦 = 提交是 UiVm 契约，
                // 不回滚就把 obj1 改名成 "obj1中"，后续树形态断言会踩空），
                // 再点 Output dock（在 over_ui 护盾内：不清选中，且把焦点
                // 从改名框挪走 —— 焦点门让位输入的对面即"失焦后 0 键归
                // 编辑器"）再连按三次 0（间隔 >1 帧，每次 down/up 成对）：
                // flac -> mp3 -> 停，Output 三行状态由循环尾断言取证。
                218 => inject_input(InputEvent::Key { key: Key::Escape, down: true }),
                220 => inject_input(InputEvent::Key { key: Key::Escape, down: false }),
                222 => inject_input(InputEvent::MouseMove { x: 400.0, y: 396.0 }),
                224 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
                226 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
                232 => inject_input(InputEvent::Key { key: Key::Num0, down: true }),
                234 => inject_input(InputEvent::Key { key: Key::Num0, down: false }),
                238 => inject_input(InputEvent::Key { key: Key::Num0, down: true }),
                240 => inject_input(InputEvent::Key { key: Key::Num0, down: false }),
                244 => inject_input(InputEvent::Key { key: Key::Num0, down: true }),
                246 => inject_input(InputEvent::Key { key: Key::Num0, down: false }),
                // S18.1 时间轴创建流取证（768x432 客户区：时间轴 dock 顶
                // tl_y = 432-24-96-110 = 202，创建控制行 ctl_y = 202+18+58+4
                // = 282，控件高 20 → 取中 y=292）：点 x 输入框（中心 238）
                // 夺焦 → Backspace 清缺省 "0" → 键入 '2' → 点 y 输入框
                //（中心 282；x 框失焦即提交 "2"）→ 同法键入 '4' → 点
                // APPLY（中心 544）登记 pos 补间 to=(2,4) ms=500。全链路
                // 实走：夺焦/键入/失焦提交/APPLY 落地，时间轴面板与护盾
                // 同帧受验（点击全程不清选中 —— obj1 始终是主选中）。
                250 => inject_input(InputEvent::MouseMove { x: 238.0, y: 292.0 }),
                252 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
                254 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
                256 => inject_input(InputEvent::Key { key: Key::Backspace, down: true }),
                258 => inject_input(InputEvent::Key { key: Key::Backspace, down: false }),
                260 => inject_input(InputEvent::Char(0x32)),
                264 => inject_input(InputEvent::MouseMove { x: 282.0, y: 292.0 }),
                266 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
                268 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
                270 => inject_input(InputEvent::Key { key: Key::Backspace, down: true }),
                272 => inject_input(InputEvent::Key { key: Key::Backspace, down: false }),
                274 => inject_input(InputEvent::Char(0x34)),
                278 => inject_input(InputEvent::MouseMove { x: 544.0, y: 292.0 }),
                280 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
                282 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
                _ => {}
            }
        }
        let snap = rt.collect_input();

        // ---- 编辑器命令（消费输入快照 —— 与游戏脚本同一读面）----
        let (z_now, y_now, del_now, tab_now) = (
            snap.is_down("LCtrl") && snap.is_down("Z"),
            snap.is_down("LCtrl") && snap.is_down("Y"),
            snap.is_down("Delete"),
            snap.pressed.contains(&nes_render_api::input::Key::Tab),
        );
        let _ = (z_now, y_now);
        // F 键与挂载流的按下沿（pressed 集即本帧边缘 —— 无需 prev 表）。
        // F5 = PLAY / 重启（Shift+F5 = STOP，S12-9）/ F6 轮换候选 /
        // F7 循环组折叠 / F8 刷资产扫描 / F9 切左栏分割档；Enter 挂载、
        // U 卸载、E 切 enabled（后三者有焦点门，见挂载段）。
        let (f5_now, f6_now, f7_now, f8_now, f9_now, enter_now, u_now, e_now) = (
            snap.pressed.contains(&Key::Other(VK_F5)),
            snap.pressed.contains(&Key::Other(VK_F6)),
            snap.pressed.contains(&Key::Other(VK_F7)),
            snap.pressed.contains(&Key::Other(VK_F8)),
            snap.pressed.contains(&Key::Other(VK_F9)),
            snap.pressed.contains(&Key::Enter),
            snap.pressed.contains(&Key::U),
            snap.pressed.contains(&Key::E),
        );
        // 冒烟钩子的运行态取证（S12-9）：帧 160（运行中）读工具栏
        // PLAY 文本（应为 "PLAY*"）；STOP 沿后的帧 176 读 spin 节点位移
        // —— 退出断言要用"脚本在运行态真实驱动过"与"PLAY* 文本投影"
        // 两件事实；此刻快照未被 RESET 污染。
        if demo && index == 160 {
            demo_play_text = rt
                .tree_mut()
                .prop(tool_play, "text")
                .and_then(|v| match v {
                    Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
        }
        if demo && index == 176 {
            let tree = rt.tree_mut();
            if let Some(spin) = tree.find_by_name("spin") {
                demo_spin_x = tree.local(spin).unwrap_or_default().pos.x;
            }
        }
        // S12-11 IME 取证（S18.1 起为**滞容闩锁**）：Char 经真实消息泵
        // 投递，到达帧有 1..数帧抖动（帧负载越大越明显）—— 固定帧号点
        // 采样会偶发扑空。改为 211..=218 窗内逐帧观察，草稿一旦含中字符
        // 即闩住取证值（Escape 在 218 抬起、222 才点击别处 —— 窗内草稿
        // 不会被回滚/提交打断）。
        if demo && (211..=218).contains(&index) && !demo_ime_draft.contains('\u{4E2D}') {
            let d = rt
                .ui_vm_mut()
                .text_state(name_input)
                .map(|t| t.draft.clone())
                .unwrap_or_default();
            if d.contains('\u{4E2D}') {
                demo_ime_draft = d;
            }
        }
        // S18.1 时间轴闩锁（APPLY 落地在帧 282 沿之后 —— 从 284 起观察）：
        // 补间登记表只在树 tick 的推进阶段变化（编辑态 NoObserver 照常
        // tick），活动 >0 与其后 ==0 两个事实各闩一次，与刷新率无关。
        if demo && index >= 284 {
            let n = rt.tree_mut().tweens().len();
            if n > 0 {
                demo_tl_seen_active = true;
            } else if demo_tl_seen_active {
                demo_tl_seen_done = true;
            }
        }
        if demo && index == 288 {
            demo_tl_rows = rt
                .tree_mut()
                .prop(hud_tl, "rows")
                .and_then(|v| match v {
                    Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
        }
        // 点击选择（hit 命中 + Selection）：左键单选 / Shift+左键多选。
        // 运行态（S12-9）：编辑交互整体让路 —— Tab 循环也一样。
        if !play.playing && tab_now && !prev_tab {
            // Tab 保留（备用循环）— 但主要路径改为鼠标点击。
            let sprites: Vec<Uid> = {
                let tree = rt.tree_mut();
                tree.preorder()
                    .into_iter()
                    .filter(|&n| tree.kind_tag(n) == Some(nes_scene::NodeKindTag::Sprite2D))
                    .filter_map(|n| tree.uid_of(n))
                    .collect()
            };
            if !sprites.is_empty() {
                let cur = sel.primary(rt.tree_mut()).and_then(|p| rt.tree_mut().uid_of(p));
                let next = match cur {
                    Some(u) => {
                        let i = sprites.iter().position(|s| s == &u).unwrap_or(0);
                        sprites[(i + 1) % sprites.len()].clone()
                    }
                    None => sprites[0].clone(),
                };
                sel.select(next);
            }
        }
        // 鼠标点击选择：button down 沿 → hit(mouse) → uid → Selection。
        // 按钮前沿检测（held 前后差）：down 沿 -> 一次点击。
        // 注意读**鼠标按钮表**（button_down）而非 is_down —— 键探针的
        // 名字空间里没有 "left"，is_down("left") 恒 false（曾让护住
        // 输入框的盾与整段点击路径变死代码，S12-2 记注）。
        // 鼠标坐标统一折算到**视图空间**（客户区→视图；viewport ==
        // 客户区时 1:1，resize 当帧 ≤1 帧的 skew 也被同一折算吸收）。
        // 视图空间 == 世界空间（相机每帧置中 (cw/2, ch/2)，恒等映射）
        // —— 精灵命中、Gizmo 拖拽、框选矩形全用同一坐标（S12-4 ①：
        // 旧口径对精灵/Gizmo 用生客户区像素，缩放窗口后命中错位）。
        let (msx, msy) = rt.mouse_view_scale(viewport);
        let (mx, my) = (snap.mouse.x * msx, snap.mouse.y * msy);
        let mouse_left_held = snap.button_down("left");
        let mouse_shift = snap.is_down("LShift");
        // 分区标题行命中（S12-7 分组折叠）：上一帧投影记出的标题矩形。
        // Label 无 anchor/size —— 矩形 = 标题行整条面板宽 x 行高
        //（S12-11 起 INS_ROW_H：真字体 14px 行高 ≈18.5px，16px 命中带
        // 会漏下半行）。
        let title_click: Option<usize> = title_rows
            .iter()
            .find(|(rx, ry, _)| {
                mx >= *rx && mx < *rx + INSPECTOR_W && my >= *ry && my < *ry + INS_ROW_H
            })
            .map(|(_, _, gi)| *gi);
        if mouse_left_held && !prev_click && title_click.is_none() && tool_sel_on && !play.playing {
            // hit 在脚本中做；宿主侧直接查树（与 hit 同一几何：盒原点
            // 经 SceneTree::sprite_hit_origin 单点助手 —— S16.5 收敛，
            // pivot 平移后的锚点角，脚本/宿主不再各持一份）。
            // 压在编辑器 UI（改名输入框 / 层级树 / Output dock / 标尺
            // 条带）上 = 面板交互：护住选中（不清空、不框选）。输入框
            // 与层级树的点击让给 UiVm 的夺焦/行点击路径；标尺与 dock
            // 照 Godot 口径不属于可编辑区 —— 点上去既不清选中也不框选。
            let over_ui = {
                let tree = rt.tree_mut();
                [
                    name_input, hud_tree, hud_dock, ruler_h, ruler_v, ruler_corner,
                    tool_sel, tool_snap, tool_grid, tool_play, tool_stop, tool_reset,
                    fs_bg, fs_sep, fs_tree,
                    // S18.1 时间轴面板全部控件：压上不清选中、不框选
                    //（输入框/按钮的交互让给 UiVm 同款纪律）。
                    tl_bg, hud_tl, tl_pos, tl_scale, tl_alpha, tl_x_in, tl_y_in,
                    tl_ms_in, tl_ease, tl_mode, tl_apply,
                ]
                .iter()
                .any(|&n| press_in_control(tree, n, viewport, (mx, my)))
            };
            let hit_uid: Option<Uid> = {
                let tree = rt.tree_mut();
                let mut cands: Vec<(i64, nes_scene::NodeId)> = tree
                    .preorder()
                    .into_iter()
                    .filter(|&n| tree.kind_tag(n) == Some(nes_scene::NodeKindTag::Sprite2D))
                    .filter(|&n| !matches!(tree.prop(n, "visible"), Some(Value::Bool(false))))
                    .map(|n| {
                        let z = tree.prop(n, "z_index")
                            .and_then(|v| if let Value::I64(i) = v { Some(*i) } else { None })
                            .unwrap_or(0);
                        (z, n)
                    })
                    .collect();
                cands.sort_by_key(|(z, _)| std::cmp::Reverse(*z));
                let mut found: Option<Uid> = None;
                for (_, n) in cands {
                    // 盒原点走 nes-scene 单点助手（S16.5）：pivot 联动 +
                    // 与脚本 hit(..) 同一几何；16px 边长为既有命中口径。
                    let o = tree.sprite_hit_origin(n);
                    if mx >= o.x && mx < o.x + 16.0 && my >= o.y && my < o.y + 16.0 {
                        found = tree.uid_of(n);
                        break;
                    }
                }
                found
            };
            if let Some(uid) = hit_uid {
                // Gizmo：点在已选对象上 → 拖拽移动（记录鼠标-对象偏移）。
                if sel.contains(&uid) {
                    let tree = rt.tree_mut();
                    if let Some(id) = tree.find_by_uid(&uid) {
                        let w = tree.world(id).unwrap_or_default();
                        gizmo = Some((uid.clone(), mx - w.tx, my - w.ty));
                    }
                }
                if mouse_shift {
                    sel.toggle(uid.clone());
                    let name = {
                        let tree = rt.tree_mut();
                        tree.find_by_uid(&uid).and_then(|id| tree.name(id).map(str::to_string))
                    };
                    log_line(&editor_log, format!("toggle {}", name.unwrap_or_default()));
                } else {
                    // 单选：与上次主选中相同就不刷日志（重复点击不灌水）。
                    let already = {
                        let cur = sel.primary(rt.tree_mut()).and_then(|p| rt.tree_mut().uid_of(p));
                        cur == Some(uid.clone())
                    };
                    sel.select(uid.clone());
                    if !already {
                        let name = {
                            let tree = rt.tree_mut();
                            tree.find_by_uid(&uid).and_then(|id| tree.name(id).map(str::to_string))
                        };
                        log_line(&editor_log, format!("sel {}", name.unwrap_or_default()));
                    }
                }
                drag_start = None; // 点击命中：不是框选
            } else if !mouse_shift && !over_ui {
                // 空白处按下：开始框选（拖拽矩形）。压在编辑器 UI 上的
                // 除外（上方护住 —— 清了选中输入框即隐藏、列表行点击即
                // 丢账，UiVm 的点击路径就永远够不着了；标尺/dock 点击
                // 也不能把可编辑区外的落点当框选起点）。
                drag_start = Some((mx, my));
                sel.clear(); // 框选重置（Shift 保留已有选择）
            }
        } else if mouse_left_held && !prev_click && !play.playing {
            // 标题点击 = 翻对应组折叠位（会话态）；本次按下就此消费 ——
            // 不清选中、不框选、不给精灵命中（护盾口径与面板点击一致）。
            // SEL off：纯观察 —— 点击不选中不拖拽不框选（工具栏按钮
            // 自身的点击由 UiVm 帧内路径接手，不受此门影响）。
            if let Some(gi) = title_click {
                group_stage ^= 1 << gi;
                let (gname, open) = if gi == 0 {
                    ("transform", group_stage & 1 == 0)
                } else {
                    ("script", group_stage & 2 == 0)
                };
                log_line(
                    &editor_log,
                    format!("group {} {}", gname, if open { "open" } else { "closed" }),
                );
                drag_start = None;
            }
        }
        // Gizmo 拖拽：鼠标移动 → 选中对象跟随（preview 直写，不入账）；
        // 松开 → Inspector.modify_local 一次事务。按住 Ctrl 吸附 8px 栅格
        //（S12-5：Godot 2D 的 Ctrl 拖动直感，状态栏 Ctrl=snap）—— 目标
        // 位置取整到 GRID_SNAP 的整数倍，松开提交的也是已取整的终值。
        // 运行态：编辑动作让路（gizmo 在 PLAY 时已强制收尾/丢弃）。
        if !play.playing {
        if let Some((ref uid, ox, oy)) = gizmo {
            if mouse_left_held {
                // preview：直写树位置（会话态，微批次之外）。吸附口径
                //（S12-7）：SNAP 开关 ON 恒吸附、Ctrl 反转；OFF 时 Ctrl
                // 临时吸附（S12-5 既有）—— 即 tool_snap XOR Ctrl。
                let (tx, ty) = (mx - ox, my - oy);
                let (tx, ty) = if tool_snap_on != snap.is_down("LCtrl") {
                    (
                        (tx / GRID_SNAP).round() * GRID_SNAP,
                        (ty / GRID_SNAP).round() * GRID_SNAP,
                    )
                } else {
                    (tx, ty)
                };
                let tree = rt.tree_mut();
                if let Some(id) = tree.find_by_uid(uid) {
                    tree.set_local(id, Transform2D::from_pos(tx, ty));
                }
            } else {
                // 松开：一次事务提交最终位置。
                let final_pos = {
                    let tree = rt.tree_mut();
                    tree.find_by_uid(uid)
                        .and_then(|id| tree.local(id))
                        .map(|t| (t.pos.x, t.pos.y))
                };
                if let Some((fx, fy)) = final_pos {
                    log.begin().unwrap();
                    Inspector::new(rt.tree_mut(), &mut log)
                        .modify_local(uid, Transform2D::from_pos(fx, fy))
                        .unwrap();
                    log.commit().unwrap();
                    log_line(&editor_log, format!("move {:.0},{:.0}", fx, fy));
                }
                gizmo = None;
            }
        }
        }

        // 框选拖拽中：mouse up → 选中矩形内全部 Sprite（运行态让路）。
        if !play.playing {
        if let Some((sx, sy)) = drag_start {
            if !mouse_left_held {
                // 松开：框选完成。
                let (ex, ey) = (mx, my);
                let (rx0, ry0) = (sx.min(ex), sy.min(ey));
                let (rx1, ry1) = (sx.max(ex), sy.max(ey));
                let in_rect: Vec<Uid> = {
                    let tree = rt.tree_mut();
                    tree.preorder()
                        .into_iter()
                        .filter(|&n| tree.kind_tag(n) == Some(nes_scene::NodeKindTag::Sprite2D))
                        .filter(|&n| !matches!(tree.prop(n, "visible"), Some(Value::Bool(false))))
                        .filter(|&n| {
                            // 框选探针 = 命中盒中心：与点击命中同一盒几何
                            //（S16.5 收敛 —— sprite_hit_origin + 半格 8px；
                            // pivot 平移后点击选得中、框选也得罩得住同一
                            // 只精灵）。无 pivot 时 == (tx+8, ty+8) 旧口径。
                            let o = tree.sprite_hit_origin(n);
                            let (cx, cy) = (o.x + 8.0, o.y + 8.0); // 中心
                            cx >= rx0 && cx <= rx1 && cy >= ry0 && cy <= ry1
                        })
                        .filter_map(|n| tree.uid_of(n))
                        .collect()
                };
                let count = in_rect.len();
                for uid in &in_rect {
                    sel.select(uid.clone());
                }
                if count > 0 {
                    log_line(&editor_log, format!("box {count}"));
                }
                drag_start = None;
            }
        }
        }
        prev_click = mouse_left_held;

        // 方向键：移动选中（Inspector 事务）。
        let (dx, dy) = {
            let s = &snap;
            let mut d = (0.0f32, 0.0f32);
            if s.is_down("ArrowLeft") { d.0 -= 2.0; }
            if s.is_down("ArrowRight") { d.0 += 2.0; }
            if s.is_down("ArrowUp") { d.1 -= 2.0; }
            if s.is_down("ArrowDown") { d.1 += 2.0; }
            d
        };
        // 方向键：移动选中（Inspector 事务）。运行态让路（方向键属于
        // 游戏输入 —— WASD/方向键直达脚本）。
        if (dx != 0.0 || dy != 0.0) && !play.playing {
            if let Some(p) = sel.primary(rt.tree_mut()) {
                if let Some(uid) = rt.tree_mut().uid_of(p) {
                    let cur = rt.tree_mut().local(p).unwrap_or_default();
                    let _ = &mut Inspector::new(rt.tree_mut(), &mut log);
                    // 简化：直接经 Inspector（一步一事务的演示口径 ——
                    // gizmo 合并提交见 T-INS-02）。
                    log.begin().unwrap();
                    Inspector::new(rt.tree_mut(), &mut log)
                        .modify_local(&uid, Transform2D::from_pos(cur.pos.x + dx, cur.pos.y + dy))
                        .unwrap();
                    log.commit().unwrap();
                }
            }
        }
        // Delete：删除子树（Hierarchy 事务）。运行态让路（Del 是编辑
        // 快捷键，运行期不得动树 —— 结构不变是 RESET 的正确性前提）。
        if del_now && !prev_del && !play.playing {
            if let Some(p) = sel.primary(rt.tree_mut()) {
                if let Some(uid) = rt.tree_mut().uid_of(p) {
                    let root_uid = { let tree = rt.tree_mut(); tree.uid_of(tree.root()).unwrap() };
                    if uid != root_uid {
                        // 删前记账（节点没了名字也没了）。
                        let del_name = {
                            let tree = rt.tree_mut();
                            tree.find_by_uid(&uid).and_then(|id| tree.name(id).map(str::to_string))
                        };
                        log.begin().unwrap();
                        Hierarchy::new(rt.tree_mut(), &mut log)
                            .delete_subtree(&uid)
                            .unwrap();
                        log.commit().unwrap();
                        log_line(&editor_log, format!("del {}", del_name.unwrap_or_default()));
                    }
                }
            }
        }
        // ---- F-4 脚本挂载与编辑器会话键（S12-7）----
        // 资产扫描周期刷新（每 60 帧 = 约 1s；F5 手动即时刷 —— 代价
        // 取舍见 SCRIPT_SCAN_EVERY 注）：res:// 树与脚本池**同一数据
        // 源**一并重扫（S12-8）。轮换下标越界即回 0（池变小/清空）；
        // fs 选中行越界即清除（条目变少时高亮不悬空）。
        if index % SCRIPT_SCAN_EVERY == 0 {
            fs_entries = scan_assets(&assets);
            scripts = script_pool(&fs_entries);
            if script_idx >= scripts.len() {
                script_idx = 0;
            }
            if let Some(i) = fs_sel {
                if i >= fs_entries.len() {
                    fs_sel = None;
                }
            }
        }
        // 焦点门：文本输入框（改名框或时间轴三个输入框 —— S18.1 并入同
        // 一道门）持焦时 Enter/字母/数字属于输入框 —— 键盘挂载流与音乐
        // 键整体让路。focus 是上一帧 UiVm 更新的结果（一帧滞后，与既有
        // UI 命中口径一致）。
        let renaming = matches!(
            rt.ui_vm_mut().focus(),
            Some(f) if f == name_input || f == tl_x_in || f == tl_y_in || f == tl_ms_in
        );
        if !renaming {
            // F5：PLAY / 重启（Shift+F5 = STOP，Godot 同款；S12-9）。
            // 运行态编辑会话随 PLAY 收尾：拖拽/框选半途即刻作废（结构
            // 不变是 RESET 还原的正确性前提）。
            if f5_now {
                if snap.is_down("LShift") {
                    play.stop(&mut rt, &editor_log);
                } else {
                    drag_start = None;
                    gizmo = None;
                    play.start(&mut rt, &assets, &editor_log);
                }
            }
            // 编辑态专属键（S12-9 护盾：运行态编辑动作全部让路 —— 检测
            // 照常、动作禁用，故无副作用）。
            if !play.playing {
            // F8：手动刷新资产扫描（带日志；周期刷新不打扰 Output。原
            // F5 职责，S12-9 让位给 PLAY）。
            if f8_now {
                fs_entries = scan_assets(&assets);
                scripts = script_pool(&fs_entries);
                if script_idx >= scripts.len() {
                    script_idx = 0;
                }
                if let Some(i) = fs_sel {
                    if i >= fs_entries.len() {
                        fs_sel = None;
                    }
                }
                log_line(&editor_log, format!("scan {} script(s)", scripts.len()));
            }
            // F9：左栏 Scene/FileSystem 分割档切换（焦点段占大头；
            // 会话态不进树 —— 比例只落在每帧重写的 offset/size 上，
            // 与工具栏三开关同一纪律）。
            if f9_now {
                fs_focus = !fs_focus;
                log_line(
                    &editor_log,
                    format!("split {}", if fs_focus { "files" } else { "scene" }),
                );
            }
            // F7：循环切换分组折叠（4 态：全开 -> 折 Transform -> 全折
            // -> 折 Script -> 全开）。会话态，不进树。
            if f7_now {
                group_stage = (group_stage + 1) % 4;
                log_line(&editor_log, format!("groups stage {}", group_stage));
            }
            // F6：轮换挂载候选。
            if f6_now {
                if scripts.is_empty() {
                    log_line(&editor_log, "cand: none (F8 to scan)".into());
                } else {
                    script_idx = (script_idx + 1) % scripts.len();
                    log_line(
                        &editor_log,
                        format!("cand {}", base_name(&scripts[script_idx])),
                    );
                }
            }
            // Enter：挂载候选 -> 选中节点。合法性（候选存在 + .nes 后
            // 缀）不过 -> Output 一行错误，不落账；过 -> 与 FileSystem
            // 双击走**同一挂载事务**（mount_script：目标解析 + 缺
            // Script 子节点同事务新建 + registry_key 落账，undo 一步
            // 整回 —— T-INS-03 契约；S12-8 起内联体上提为公共函数）。
            if enter_now {
                let cand = scripts.get(script_idx).cloned();
                let valid = match cand {
                    Some(ref rel) if rel.ends_with(".nes") && assets.join(rel).is_file() => true,
                    Some(ref rel) => {
                        log_line(&editor_log, format!("mount: not found {rel}"));
                        false
                    }
                    None => {
                        log_line(&editor_log, "mount: no candidate (F8 to scan)".into());
                        false
                    }
                };
                if valid {
                    let rel = cand.unwrap_or_default();
                    mount_script(rt.tree_mut(), &mut log, &sel, &editor_log, &rel);
                }
            }
            // U：卸载 = registry_key 写空串（schema 缺省 = 未挂载）。
            // 无挂载目标 / 本就空 -> Output 一行说明，不落空账。
            if u_now {
                match sel
                    .primary(rt.tree_mut())
                    .and_then(|p| mount_target(rt.tree_mut(), p))
                {
                    Some((u, true, _)) => {
                        log.begin().unwrap();
                        Inspector::new(rt.tree_mut(), &mut log)
                            .modify_prop(&u, "registry_key", Value::Str(String::new()))
                            .unwrap();
                        log.commit().unwrap();
                        log_line(&editor_log, "unmount".into());
                    }
                    Some((_, false, _)) => {
                        log_line(&editor_log, "unmount: nothing mounted".into())
                    }
                    None => log_line(&editor_log, "unmount: no script node".into()),
                }
            }
            // E：enabled 切换（作用于挂载目标 Script 节点）。enabled
            // 只对已挂载脚本有语义，无目标 -> 报一行不落账。
            if e_now {
                match sel
                    .primary(rt.tree_mut())
                    .and_then(|p| mount_target(rt.tree_mut(), p))
                {
                    Some((u, _, cur)) => {
                        log.begin().unwrap();
                        Inspector::new(rt.tree_mut(), &mut log)
                            .modify_prop(&u, "enabled", Value::Bool(!cur))
                            .unwrap();
                        log.commit().unwrap();
                        log_line(
                            &editor_log,
                            format!("script enabled {}", if cur { "-" } else { "Y" }),
                        );
                    }
                    None => log_line(&editor_log, "enable: no script node".into()),
                }
            }
            // 数字键 0：三态音乐预览循环（S14；编辑态专属 —— 运行态混音器
            // 归游戏脚本；改名框持焦让位输入，焦点门与挂载流同门）。三态 =
            // 停 -> 曲1(FLAC) -> 曲2(MP3) -> 停…，只按**实际装载成功**的
            // 曲目轮换（缺曲/失败时循环自动退化为二态/一态 —— 用户目录
            // 不在仓库，CI/他机天然安全）。切态先 stop_all（曲间不打架）；
            // looped 循环 —— Output 状态行与实际出声一致。整曲 PCM 已在
            // 内存（一首 4 分钟约 40-80MB）—— **流式是后续**（S14 文档 §5）。
            // 播放失败（无 waveOut 设备等）如实记行、回停态，不崩帧。
            if snap.pressed.contains(&Key::Num0) && !music_tracks.is_empty() {
                let next = (music_state + 1) % (music_tracks.len() + 1);
                rt.stop_host_sounds();
                let mut landed = 0usize; // 失败落点 = 停态（如实）
                if next == 0 {
                    log_line(&editor_log, "music: stopped".into());
                } else {
                    let (key, label) = music_tracks[next - 1];
                    match rt.play_host_sound(key, 1.0, true) {
                        Ok(()) => {
                            log_line(&editor_log, format!("music: {label} (looped)"));
                            landed = next;
                        }
                        Err(e) => log_line(&editor_log, format!("music: play failed: {e}")),
                    }
                }
                music_state = landed;
            }
            }
        }
        // Ctrl+Z / Ctrl+Y：undo / redo（直接消费事务历史）。落账后
        // 文档真相可能已变（改名被回滚/重放）—— 输入框投影与草稿
        // 跟随（S12-4 ④）：reset_text 置草稿 = 当前名、不触发
        // on_commit（回滚值不会再记账），持焦中的旧草稿即刻作废。
        // 运行态让路（事务历史只在编辑态动 —— STOP 后原样保留）。
        let mut doc_changed = false;
        if !play.playing && z_now && !prev_z && log.undo(rt.tree_mut()).unwrap_or(false) {
            doc_changed = true;
            log_line(&editor_log, "undo".into());
        }
        if !play.playing && y_now && !prev_y && log.redo(rt.tree_mut()).unwrap_or(false) {
            doc_changed = true;
            log_line(&editor_log, "redo".into());
        }
        prev_z = z_now;
        prev_y = y_now;
        prev_del = del_now;
        prev_tab = tab_now;
        if doc_changed {
            if let Some(uid) = bound_sel.clone() {
                let name = {
                    let tree = rt.tree_mut();
                    tree.find_by_uid(&uid)
                        .and_then(|id| tree.name(id).map(str::to_string))
                };
                if let Some(name) = name {
                    let _ = rt
                        .tree_mut()
                        .set_prop(name_input, "text", Value::Str(name.clone()));
                    rt.ui_vm_mut().reset_text(name_input, &name);
                }
            }
        }

        // ---- UI 投影（每帧从状态模型重算，零自有状态）----
        // 宿主每帧布局投影（S12-4 ①，与 sel_box 同款投影纪律）：面板
        // 恒定宽、状态栏贴底、相机置中 —— 世界坐标 == 视图坐标恒等
        // 映射，HUD/sel_box/命中全部免换算。换绑草稿在 tree 借用外做
        //（ui_vm_mut 与 tree_mut 不共存），见块后的 rebind_name。
        let mut rebind_name: Option<String> = None;
        {
            let tree = rt.tree_mut();
            // S12-5 错位根修（本帧同源）：Gizmo 拖拽的 set_local 只标脏
            //（DIRTY_XFORM / DIRTY_SUBTREE），世界矩阵要等 simulate/tick
            // 里的 refresh_transforms 才重算 —— 投影块此刻读 tree.world()
            // 拿到的是**上一帧**缓存，选中框/命中恒落后一帧，快速拖动把
            // 一帧之差积累成几十像素的可见错位。在一切 world() 读数之前
            // 做一次引擎权威冲洗（增量式，只算脏子树，代价可忽略）：
            // 框 / 命中 / 任何 world() 读数从此与拖拽写入同帧。这只是
            // 宿主读数前的自取，不改 runtime/提取层的刷新时序（契约）。
            tree.refresh_transforms();
            // 相机置中 = (cw/2, ch/2)：最大化/拖拽后世界视口吃中间
            // 剩余区域（面板不随窗口拉伸）。相机恒等映射保住：世界
            // 坐标 == 视图坐标，标尺刻度/命中/框选全部免换算。
            tree.set_local(cam, Transform2D::from_pos(viewport.0 / 2.0, viewport.1 / 2.0));
            // 视口区与可编辑区（S12-6 加标尺/dock 后的口径）：视口区
            // 底缘上移到 dock 上缘；S18.1 起再让位时间轴 dock（Output
            // 上方 ~110px）；可编辑区再让出顶/左各 16px 标尺
            //（标尺不属于可编辑区，Godot 口径 —— 网格/命中/框选只在
            // 可编辑区内）。
            let gx0 = MARGIN + LEFT_PANEL_W;
            let gx1 = viewport.0 - INSPECTOR_W - 2.0 * MARGIN;
            let gy1 = viewport.1 - STATUS_BAND - DOCK_H - TIMELINE_H;
            let vx0 = gx0 + RULER_W;
            // S12-7：标尺整体下移让出视口工具栏（工具带 24px 在标尺之
            // 上 —— Godot 2D 视口顶部工具条的堆叠顺序）。
            let ruler_y = TOP_BAND + TOOLBAR_H;
            let vy0 = ruler_y + RULER_W;
            let vx1 = gx1;
            let vy1 = gy1;

            // 视口工具栏布线（S12-7）：panel 槽铺底 + 底缘 1px border
            // 分隔线 + 三个开关按钮 + PLAY/STOP/RESET 运行三键（S12-9
            // —— PLAY 运行中带 * 后缀，同 * = ON 的会话态口径；STOP/
            // RESET 无 ON 态，恒显素文本）。gx0 恒定（面板恒宽），沿
            // 投影纪律每帧重写；按钮文本后缀 * = ON —— 开关态是编辑器
            // 会话态，每帧重写进文本投影（投影无状态口径）。
            let _ = tree.set_prop(tool_bg, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(gx0, TOP_BAND)));
            let _ = tree.set_prop(tool_bg, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(gx1 - gx0, TOOLBAR_H)));
            let _ = tree.set_prop(tool_sep, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(gx0, TOP_BAND + TOOLBAR_H - 1.0)));
            let _ = tree.set_prop(tool_sep, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(gx1 - gx0, 1.0)));
            let tool_btns = [
                (tool_sel, tool_sel_on, "SEL"),
                (tool_snap, tool_snap_on, "SNAP"),
                (tool_grid, tool_grid_on, "GRID"),
                (tool_play, play.playing, "PLAY"),
                (tool_stop, false, "STOP"),
                (tool_reset, false, "RESET"),
            ];
            for (i, (b, on, name)) in tool_btns.iter().enumerate() {
                let bx = gx0 + SPACE_S + i as f32 * TOOLBAR_BTN_STEP;
                let _ = tree.set_prop(*b, PROP_CONTROL_OFFSET,
                    Value::Vec2(nes_scene::Vec2::new(
                        bx,
                        TOP_BAND + 2.0,
                    )));
                let _ = tree.set_prop(*b, "text",
                    Value::Str(if *on { format!("{name}*") } else { (*name).to_string() }));
                // S18：底板随按钮同步布线（同位同尺寸 —— 底板在按钮正
                // 下方，纹理透出按钮透明底；投影无状态，每帧重写口径）。
                if let Some(&p) = tool_plates.get(i) {
                    let _ = tree.set_prop(p, PROP_CONTROL_OFFSET,
                        Value::Vec2(nes_scene::Vec2::new(bx, TOP_BAND + 2.0)));
                }
            }
            // 左栏两段布线（S12-8，Godot 左栏 Scene + res:// 两段）：
            // 可用高 = 顶带到 dock 上缘；上段 Scene（层级树）+ 4px
            // border 分隔条 + 下段 FileSystem（"res:/" 标题 + 资产树）。
            // 分割比例由 F9 档位推导（fs_focus 会话态 —— 比例本身不进
            // 树，只落在每帧重写的 offset/size 上，焦点段占大头）。
            // 最小窗口下段高钳 0（列表/条带照画零矩形，提取层口径）。
            let avail_h = (viewport.1 - TOP_BAND - STATUS_BAND - DOCK_H - TIMELINE_H).max(0.0);
            let top_frac = if fs_focus { FS_SPLIT_ALT } else { FS_SPLIT_TOP };
            let scene_h = ((avail_h - FS_SEP_H) * top_frac).max(0.0);
            let fs_h = (avail_h - FS_SEP_H - scene_h).max(0.0);
            let sep_y = TOP_BAND + scene_h;
            let fs_y = sep_y + FS_SEP_H;
            let _ = tree.set_prop(hud_tree, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, scene_h)));
            let _ = tree.set_prop(fs_sep, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(MARGIN, sep_y)));
            let _ = tree.set_prop(fs_sep, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, FS_SEP_H)));
            let _ = tree.set_prop(fs_bg, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(MARGIN, fs_y)));
            let _ = tree.set_prop(fs_bg, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, fs_h)));
            tree.set_local(fs_title, Transform2D::from_pos(MARGIN + 2.0, fs_y + 1.0));
            let _ = tree.set_prop(fs_tree, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(MARGIN, fs_y + FS_TITLE_H)));
            let _ = tree.set_prop(fs_tree, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, (fs_h - FS_TITLE_H).max(0.0))));
            // res:// 行文本投影（投影无状态口径）：缩进树形（每层两空
            // 格，目录尾斜杠）+ 选中行下标随 fs_sel（-1 = 无选中，与
            // 层级树 selected 行高亮同款）。空列表 = 空 rows（行数 0，
            // UiVm 不回调行）。
            let fs_rows: Vec<String> = fs_entries.iter().map(fs_row_text).collect();
            let _ = tree.set_prop(fs_tree, "rows", Value::Str(fs_rows.join("\n")));
            let fs_sel_row = fs_sel
                .filter(|&i| i < fs_entries.len())
                .map(|i| i as i64)
                .unwrap_or(-1);
            let _ = tree.set_prop(fs_tree, "selected", Value::I64(fs_sel_row));
            // 右检查器面板底：x = cw-198（宽 190 + 右缘 8），y = 8..时间轴上缘
            //（S18.1 起 dock 之上再让出时间轴带）。
            let _ = tree.set_prop(hud_ins_bg, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(viewport.0 - INSPECTOR_W - 2.0 * MARGIN, MARGIN)));
            let _ = tree.set_prop(hud_ins_bg, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(INSPECTOR_W, viewport.1 - MARGIN - STATUS_BAND - DOCK_H - TIMELINE_H)));
            // 状态栏贴底：y = ch-20。
            tree.set_local(hud_st, Transform2D::from_pos(MARGIN, viewport.1 - 20.0));

            // 视口网格布线（S12-5）：世界可视区 = 标尺内侧的可编辑区
            //（S12-6 起网格不再铺到标尺底下）。32px 间距、从视口原点
            //（世界原点，相机恒等映射）对齐 —— 缩放窗口时线条钉在
            // 世界坐标上不漂移。池条带竖条在前、横条在后依次吃满；
            // 线条数超出池容量就少画几根（GRID_POOL 上限注释）；落
            // 不进可视区的条带 visible=false 不画。控件是单级视口锚
            // 定，直接写 offset/size（锚 (0,0) + 客户区坐标）。
            let (mut nv, mut nh) = (0usize, 0usize);
            let (mut v0, mut h0) = (0i64, 0i64);
            // GRID 开关（S12-7）：off = 全部条带走池尾熄灭分支（布局
            // 尺寸照算，只关显示 —— 投影无状态，每帧重写一遍口径）。
            if vx1 > vx0 && vy1 > vy0 && tool_grid_on {
                v0 = (vx0 / GRID_SPACING).ceil() as i64;
                let v1 = ((vx1 - 1.0) / GRID_SPACING).floor() as i64;
                h0 = (vy0 / GRID_SPACING).ceil() as i64;
                let h1 = ((vy1 - 1.0) / GRID_SPACING).floor() as i64;
                nv = ((v1 - v0 + 1).max(0) as usize).min(GRID_POOL);
                nh = ((h1 - h0 + 1).max(0) as usize).min(GRID_POOL - nv);
            }
            for (i, &bar) in grid_bars.iter().enumerate() {
                if i < nv {
                    // 竖条：x 钉在 32 的整数倍，纵贯可视区全高。
                    let x = (v0 + i as i64) as f32 * GRID_SPACING;
                    let _ = tree.set_prop(bar, PROP_CONTROL_OFFSET,
                        Value::Vec2(nes_scene::Vec2::new(x, vy0)));
                    let _ = tree.set_prop(bar, PROP_CONTROL_SIZE,
                        Value::Vec2(nes_scene::Vec2::new(1.0, vy1 - vy0)));
                    let _ = tree.set_prop(bar, "visible", Value::Bool(true));
                } else if i < nv + nh {
                    // 横条：y 钉在 32 的整数倍，横贯可视区全宽。
                    let y = (h0 + (i - nv) as i64) as f32 * GRID_SPACING;
                    let _ = tree.set_prop(bar, PROP_CONTROL_OFFSET,
                        Value::Vec2(nes_scene::Vec2::new(vx0, y)));
                    let _ = tree.set_prop(bar, PROP_CONTROL_SIZE,
                        Value::Vec2(nes_scene::Vec2::new(vx1 - vx0, 1.0)));
                    let _ = tree.set_prop(bar, "visible", Value::Bool(true));
                } else {
                    // 池内备用条带：熄灭（投影无状态，每帧重写一遍口径）。
                    let _ = tree.set_prop(bar, "visible", Value::Bool(false));
                }
            }

            // 2D 标尺布线（S12-6，Godot CanvasItemEditor::_draw_rulers
            // 的自绘版）：顶横条带 + 左竖条带（panel 槽铺底）+ 角块
            //（border 槽），刻度 64px 一根 1px 细条（整 128 的主刻度
            // 全高、次刻度半高贴视口缘 —— Godot graduation 的层级观
            // 感），数字 128px 一个。刻度与世界原点对齐：相机恒等映
            // 射下世界 x=k*64 就落在屏幕 x=k*64，世界 (0,0) 对齐刻度
            // 0。条带/标签数按视口尺寸算、池上限封顶（RULER_TICKS_*
            // / RULER_LABELS_* 注释），落不进的熄灭/置空。
            let _ = tree.set_prop(ruler_h, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(gx0, ruler_y)));
            let _ = tree.set_prop(ruler_h, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(vx1 - gx0, RULER_W)));
            let _ = tree.set_prop(ruler_v, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(gx0, ruler_y)));
            let _ = tree.set_prop(ruler_v, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(RULER_W, vy1 - ruler_y)));
            let _ = tree.set_prop(ruler_corner, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(gx0, ruler_y)));
            let _ = tree.set_prop(ruler_corner, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(RULER_W, RULER_W)));
            let half = RULER_W * 0.5;
            // 顶横刻度：世界 x = k*64 ∈ [vx0, vx1)。
            let mut used = 0usize;
            if vx1 > vx0 {
                let k0 = (vx0 / RULER_TICK).ceil() as i64;
                let k1 = ((vx1 - 1.0) / RULER_TICK).floor() as i64;
                for k in k0..=k1 {
                    if used >= RULER_TICKS_H {
                        break;
                    }
                    let x = k as f32 * RULER_TICK;
                    let major = k % 2 == 0; // 主刻度 = 整 128（64×2）
                    let (ty, th) = if major { (ruler_y, RULER_W) } else { (ruler_y + half, half) };
                    let tick = ruler_ticks[used];
                    let _ = tree.set_prop(tick, PROP_CONTROL_OFFSET,
                        Value::Vec2(nes_scene::Vec2::new(x, ty)));
                    let _ = tree.set_prop(tick, PROP_CONTROL_SIZE,
                        Value::Vec2(nes_scene::Vec2::new(1.0, th)));
                    let _ = tree.set_prop(tick, "visible", Value::Bool(true));
                    used += 1;
                }
            }
            // 左竖刻度：世界 y = k*64 ∈ [vy0, vy1)，池接在顶横之后。
            let h_used = used;
            if vy1 > vy0 {
                let k0 = (vy0 / RULER_TICK).ceil() as i64;
                let k1 = ((vy1 - 1.0) / RULER_TICK).floor() as i64;
                for k in k0..=k1 {
                    if used >= h_used + RULER_TICKS_V {
                        break;
                    }
                    let y = k as f32 * RULER_TICK;
                    let major = k % 2 == 0;
                    let (tx, tw) = if major { (gx0, RULER_W) } else { (gx0 + half, half) };
                    let tick = ruler_ticks[used];
                    let _ = tree.set_prop(tick, PROP_CONTROL_OFFSET,
                        Value::Vec2(nes_scene::Vec2::new(tx, y)));
                    let _ = tree.set_prop(tick, PROP_CONTROL_SIZE,
                        Value::Vec2(nes_scene::Vec2::new(tw, 1.0)));
                    let _ = tree.set_prop(tick, "visible", Value::Bool(true));
                    used += 1;
                }
            }
            // 余量熄灭（投影无状态，每帧重写一遍口径）。
            for tick in &ruler_ticks[used..] {
                let _ = tree.set_prop(*tick, "visible", Value::Bool(false));
            }
            // 顶横数字：x = k*128 ∈ [vx0, vx1)，文本 16px 高正好嵌进
            // 条带（y = 条带顶）。竖标尺数字横排贴条带左缘（Godot 是
            // 旋转排版，取舍见装配注释）。
            let mut lab_used = 0usize;
            if vx1 > vx0 {
                let k0 = (vx0 / RULER_TICK / 2.0).ceil() as i64;
                let k1 = ((vx1 - 1.0) / RULER_TICK / 2.0).floor() as i64;
                for k in k0..=k1 {
                    if lab_used >= RULER_LABELS_H {
                        break;
                    }
                    let x = k as f32 * RULER_TICK * 2.0;
                    let lab = ruler_labels[lab_used];
                    tree.set_local(lab, Transform2D::from_pos(x + 2.0, ruler_y));
                    let _ = tree.set_prop(lab, PROP_LABEL_TEXT, Value::Str(x.to_string()));
                    lab_used += 1;
                }
            }
            if vy1 > vy0 {
                let k0 = (vy0 / RULER_TICK / 2.0).ceil() as i64;
                let k1 = ((vy1 - 1.0) / RULER_TICK / 2.0).floor() as i64;
                for k in k0..=k1 {
                    if lab_used >= RULER_LABELS_H + RULER_LABELS_V {
                        break;
                    }
                    let y = k as f32 * RULER_TICK * 2.0;
                    let lab = ruler_labels[lab_used];
                    tree.set_local(lab, Transform2D::from_pos(gx0 + 1.0, y));
                    let _ = tree.set_prop(lab, PROP_LABEL_TEXT, Value::Str(y.to_string()));
                    lab_used += 1;
                }
            }
            // 余量置空文本（提取层判空不上屏）。
            for lab in &ruler_labels[lab_used..] {
                let _ = tree.set_prop(*lab, PROP_LABEL_TEXT, Value::Str(String::new()));
            }

            // Output dock 布线（S12-6）：全宽 panel 铺底 + "Output"
            // 标题 + 日志 ListView（高 = dock - 标题行 - 底缝）。行文
            // 本 = 环形缓冲最近几行（新行在下）：可见行数按列表高算
            //（行 y = 列表顶 +4 + i×18 → (76-4)/18 = 4 行），环形保
            // 留 8 行、可见窗只放最新能放下的几行 —— ListView 滚动
            // 偏移是 UiVm 瞬态、宿主没有"钉底"通道，宁可少显示也不
            // 把最新行藏进滚动区外（Godot Output 自动钉底的直感）。
            let dock_y = viewport.1 - STATUS_BAND - DOCK_H;
            let _ = tree.set_prop(dock_bg, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(MARGIN, dock_y)));
            let _ = tree.set_prop(dock_bg, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(viewport.0 - 2.0 * MARGIN, DOCK_H)));
            tree.set_local(dock_title, Transform2D::from_pos(MARGIN + 2.0, dock_y + 1.0));
            let dock_list_h = DOCK_H - DOCK_TITLE_H - 2.0;
            let _ = tree.set_prop(hud_dock, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(MARGIN + 2.0, dock_y + DOCK_TITLE_H)));
            let _ = tree.set_prop(hud_dock, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(viewport.0 - 2.0 * MARGIN - 4.0, dock_list_h)));
            let dock_fit = (((dock_list_h - 4.0) / DOCK_ROW_H).floor() as usize).max(1);
            let dock_rows: Vec<String> = editor_log
                .borrow()
                .iter()
                .rev()
                .take(dock_fit)
                .rev()
                .map(|l| l.chars().take(DOCK_LINE_CHARS).collect())
                .collect();
            let _ = tree.set_prop(hud_dock, "rows", Value::Str(dock_rows.join("\n")));

            // 时间轴 dock 布线（S18.1，Output 上方的全宽面板）：九宫格
            // 铺底 + "TIMELINE" 标题 + 补间行区 + 进度条池 + 创建控制行。
            // 每帧投影（投影无状态口径）—— 补间行从 tween_rows(primary)
            // 现算（Selection 主选中驱动 —— 照 Inspector 同款纪律）。
            let tl_y = dock_y - TIMELINE_H;
            let tl_w = viewport.0 - 2.0 * MARGIN;
            let _ = tree.set_prop(tl_bg, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(MARGIN, tl_y)));
            let _ = tree.set_prop(tl_bg, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(tl_w, TIMELINE_H)));
            tree.set_local(tl_title, Transform2D::from_pos(MARGIN + 2.0, tl_y + 1.0));
            let _ = tree.set_prop(hud_tl, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(MARGIN + 2.0, tl_y + TL_TITLE_H)));
            let _ = tree.set_prop(hud_tl, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(tl_w - 4.0, TL_LIST_H)));
            // 行投影：主选中的活动补间（行序 = 注册序 —— tween_rows 单点
            // 保证）；无选中/无补间 = 单行说明（冻结文案，冒烟可断言）。
            let tl_primary = sel.primary(tree);
            let tl_rows: Vec<nes_scene::TweenRow> = match tl_primary {
                Some(p) => tree.tween_rows(p),
                None => Vec::new(),
            };
            let tl_rows_text = if tl_rows.is_empty() {
                if tl_primary.is_some() {
                    "(no tweens on selection)".to_string()
                } else {
                    "(no selection)".to_string()
                }
            } else {
                tl_rows.iter().map(tl_row_text).collect::<Vec<_>>().join("\n")
            };
            let _ = tree.set_prop(hud_tl, "rows", Value::Str(tl_rows_text));
            // 进度条池：可见行（3 行）每行一条行下沿 2px 细条，宽 = 行宽
            // × progress（fill_slot selected 色 —— 照网格条带池先例的
            // Control 池路线，非文本进度条）。行超出可见窗的不画（列表
            // 滚动是 UiVm 瞬态，宿主无钉底通道 —— 宁可少画不画到窗外）。
            let tl_bar_fit = (((TL_LIST_H - 4.0) / DOCK_ROW_H).floor() as usize).min(TL_BARS);
            for (i, &bar) in tl_bars.iter().enumerate() {
                if i < tl_rows.len() && i < tl_bar_fit {
                    let bw = ((tl_w - 4.0) * tl_rows[i].progress.clamp(0.0, 1.0)).max(0.0);
                    let _ = tree.set_prop(bar, PROP_CONTROL_OFFSET,
                        Value::Vec2(nes_scene::Vec2::new(
                            MARGIN + 2.0,
                            tl_y + TL_TITLE_H + 4.0 + i as f32 * DOCK_ROW_H + DOCK_ROW_H - 2.0,
                        )));
                    let _ = tree.set_prop(bar, PROP_CONTROL_SIZE,
                        Value::Vec2(nes_scene::Vec2::new(bw, 2.0)));
                    let _ = tree.set_prop(bar, "visible", Value::Bool(true));
                } else {
                    // 池内备用条带：熄灭（投影无状态，每帧重写一遍口径）。
                    let _ = tree.set_prop(bar, "visible", Value::Bool(false));
                }
            }
            // 创建控制行：说明标签 + 六按钮（九宫格底板随行布线）+ 三输
            // 入框。行 y 每帧重写；x 来自 TL_CTL_LAYOUT 单点表（恒定）。
            // 通道按钮 * 后缀 = 当前选中通道（工具栏 SEL/SNAP 同款口径）；
            // 缓动/模式按钮文本 = 当前档名（循环点按换档，落账段翻下标）。
            let tl_ctl_y = tl_y + TL_TITLE_H + TL_LIST_H + SPACE_S;
            tree.set_local(tl_new_label,
                Transform2D::from_pos(MARGIN + TL_CTL_X, tl_ctl_y + 3.0));
            tree.set_local(tl_to_label,
                Transform2D::from_pos(MARGIN + TL_CTL_X + 194.0, tl_ctl_y + 3.0));
            tree.set_local(tl_ms_label,
                Transform2D::from_pos(MARGIN + TL_CTL_X + 306.0, tl_ctl_y + 3.0));
            const TL_BTN_LAYOUT_IDX: [usize; 6] = [0, 1, 2, 6, 7, 8];
            let ch = |i: usize, name: &str| {
                if tl_channel == i {
                    format!("{name}*")
                } else {
                    name.to_string()
                }
            };
            let tl_btns: [(nes_scene::NodeId, usize, String); 6] = [
                (tl_pos, 0, ch(0, "POS")),
                (tl_scale, 1, ch(1, "SCALE")),
                (tl_alpha, 2, ch(2, "ALPHA")),
                (tl_ease, 6, TL_EASINGS[tl_ease_idx].to_string()),
                (tl_mode, 7, TL_MODES[tl_mode_idx].to_string()),
                (tl_apply, 8, "APPLY".to_string()),
            ];
            for (i, &plate) in tl_plates.iter().enumerate() {
                let (px, pw) = TL_CTL_LAYOUT[TL_BTN_LAYOUT_IDX[i]];
                let _ = tree.set_prop(plate, PROP_CONTROL_OFFSET,
                    Value::Vec2(nes_scene::Vec2::new(MARGIN + TL_CTL_X + px, tl_ctl_y)));
                let _ = tree.set_prop(plate, PROP_CONTROL_SIZE,
                    Value::Vec2(nes_scene::Vec2::new(pw, TOOLBAR_BTN_H)));
            }
            for (b, li, text) in tl_btns {
                let (px, pw) = TL_CTL_LAYOUT[li];
                let _ = tree.set_prop(b, PROP_CONTROL_OFFSET,
                    Value::Vec2(nes_scene::Vec2::new(MARGIN + TL_CTL_X + px, tl_ctl_y)));
                let _ = tree.set_prop(b, PROP_CONTROL_SIZE,
                    Value::Vec2(nes_scene::Vec2::new(pw, TOOLBAR_BTN_H)));
                let _ = tree.set_prop(b, "text", Value::Str(text));
            }
            for (n, li) in [(tl_x_in, 3usize), (tl_y_in, 4), (tl_ms_in, 5)] {
                let (px, pw) = TL_CTL_LAYOUT[li];
                let _ = tree.set_prop(n, PROP_CONTROL_OFFSET,
                    Value::Vec2(nes_scene::Vec2::new(MARGIN + TL_CTL_X + px, tl_ctl_y)));
                let _ = tree.set_prop(n, PROP_CONTROL_SIZE,
                    Value::Vec2(nes_scene::Vec2::new(pw, TOOLBAR_BTN_H)));
            }

            // Hierarchy View：树投影 → ListView 行（前序 + 缩进 + 选中
            // 标记 *，缩进用 ASCII 空格 —— 行文本经默认字体等宽渲染）。
            // 行→节点映射平行重建（walk 顺序即行序）：主选中行下标与
            // 行点击回调都按这份映射结算 —— 投影与交互同源。存活节点
            // 必有 uid（add_node 即发、walk 只访问存活节点），行与映射
            // 严格同长同序；无"悬垂行"可言（删除即整行消失）。
            // S12-5：walk 跳过 "grid" 容器整棵子树；S12-6 沿用同一过滤
            // 先例加 "ruler"/"dock" —— 网格/标尺/Output dock 都是观感
            // 节点不是可编辑对象，不进行列表；过滤在 walk 单点做，行
            // 文本与行→uid 映射天然同源（同一次遍历产出，映射不会被
            // 观感节点污染）。
            let mut lines: Vec<String> = Vec::new();
            let mut row_map: Vec<Uid> = Vec::new();
            let sel_uids: Vec<Uid> = sel.uids().to_vec();
            fn walk(
                tree: &nes_scene::SceneTree,
                id: nes_scene::NodeId,
                depth: usize,
                sel: &[Uid],
                out: &mut Vec<String>,
                map: &mut Vec<Uid>,
                skips: &[nes_scene::NodeId],
            ) {
                if skips.contains(&id) {
                    return; // 观感容器（网格/标尺/dock）：整子树不进层级树。
                }
                let name = tree.name(id).unwrap_or("?");
                let uid = tree.uid_of(id);
                let mark = uid.as_ref().map(|u| sel.contains(u)).unwrap_or(false);
                let indent = "  ".repeat(depth);
                out.push(format!("{}{}{}", indent, if mark { "* " } else { "  " }, name));
                if let Some(u) = uid {
                    map.push(u);
                }
                for &c in tree.children(id) {
                    walk(tree, c, depth + 1, sel, out, map, skips);
                }
            }
            // S18 起 skips 加 theme_node：主题节点是皮肤数据不是可编辑
            // 对象，不进层级树行列表（同 grid/ruler/dock 纪律）。
            // S18.1 起 skips 再加 tldock：时间轴是观感/工具（补间可视化 +
            // 创建控制），不是场景对象 —— 整子树不进层级树。
            let skips = [grid, ruler, dock, toolbar, fsdock, theme_node, tldock];
            walk(tree, tree.root(), 0, &sel_uids, &mut lines, &mut row_map, &skips);
            // 行文本不带尾随 '\n'（场景层 rows_count 按分隔符计数会把
            // 尾随空行当成幻影行，行点击回调的行数上限随之失真）。
            let _ = tree.set_prop(hud_tree, "rows", Value::Str(lines.join("\n")));
            // 刷新共享映射（UiVm 行点击回调在帧内按它查 uid —— 借用
            // 只持续到本语句结束，帧内回调不会撞上宿主借用）。
            *row_map_shared.borrow_mut() = row_map.clone();

            // 选中行下标投影：主选中 uid → 行映射查找（找不到 = -1，
            // 即 schema 的无选中缺省）。与 sel_box/z_index 同款纪律：
            // Selection 是唯一语义来源，每帧直写属性。
            let sel_row = sel
                .primary(tree)
                .and_then(|p| tree.uid_of(p))
                .and_then(|u| row_map.iter().position(|m| m == &u))
                .map(|i| i as i64)
                .unwrap_or(-1);
            let _ = tree.set_prop(hud_tree, "selected", Value::I64(sel_row));

            // Inspector View（S12-7 Godot 分区）：标题 + Transform /
            // Script 两组。组标题是独立 Label（text_dim 色），hud_ins
            // 文本给标题让出空行（行序即布局）；折叠 = 该组行不进文本、
            // 后续行上移。分区标题矩形记入 title_rows（下一帧帧首命中
            // —— 一帧滞后与既有 UI 命中同口径）。数值行随选中实时刷新。
            let ins_x = viewport.0 - INSPECTOR_W;
            title_rows.clear();
            let mut ins_text = String::from("Inspector");
            match sel.primary(tree) {
                Some(p) => {
                    let tf_open = group_stage & 1 == 0;
                    let sc_open = group_stage & 2 == 0;
                    // Transform 组标题（面板第 2 行，S12-11 起步进
                    // INS_ROW_H）：前缀 "-" 展开 / "+" 折叠，text_dim 色
                    //（装配期定槽，此处只翻文本）。
                    title_rows.push((ins_x, 12.0 + INS_ROW_H, 0));
                    tree.set_local(ins_tf_title, Transform2D::from_pos(ins_x, 12.0 + INS_ROW_H));
                    let _ = tree.set_prop(ins_tf_title, "visible", Value::Bool(true));
                    let _ = tree.set_prop(ins_tf_title, PROP_LABEL_TEXT,
                        Value::Str(if tf_open { "- Transform" } else { "+ Transform" }.into()));
                    // 属性行随折叠省略（hud_ins 第 2 行留空给标题）。
                    // 名字截 5 字符：行宽 "name " + 5 = 10 字，不超内衬
                    // 宽（11 字上限）。
                    if tf_open {
                        let name: String = tree.name(p).unwrap_or("?").chars().take(5).collect();
                        let local = tree.local(p).unwrap_or_default();
                        // z 显示属性表现值：选中高亮会把选中精灵的 z 写
                        // 成 5（S12-5 机制），显示的是节点当前真实属性。
                        let z = tree
                            .prop(p, "z_index")
                            .and_then(|v| if let Value::I64(i) = v { Some(*i) } else { None })
                            .unwrap_or(0);
                        ins_text.push_str(&format!(
                            "\n\nname {}\nx {:.0}\ny {:.0}\nz {}",
                            name, local.pos.x, local.pos.y, z
                        ));
                    }
                    let _ = tree.set_prop(hud_ins, PROP_LABEL_TEXT, Value::Str(ins_text));
                    // 标题/信息 Label 每帧投影到右面板顶（x = cw-190，
                    // 随面板走 —— S12-6 修"标题被表面边缘裁剪"口径）。
                    tree.set_local(hud_ins, Transform2D::from_pos(ins_x, 12.0));

                    // 改名输入框 = Transform 组成员：折叠即隐藏；展开时
                    // 槽位紧跟组行（行高 INS_ROW_H —— 不再是装配期写死的
                    // 常量，S12-6 ①根修口径延续：每帧重写，窗口一变当帧
                    // 跟上；S12-11 步进 20 见常量注）。
                    let input_y = 12.0
                        + 2.0 * INS_ROW_H
                        + if tf_open { 4.0 * INS_ROW_H } else { 0.0 }
                        + 4.0;
                    let _ = tree.set_prop(name_input, "visible", Value::Bool(tf_open));
                    let _ = tree.set_prop(name_input, PROP_CONTROL_OFFSET,
                        Value::Vec2(nes_scene::Vec2::new(
                            viewport.0 - INSPECTOR_W - 2.0 * MARGIN + INSPECTOR_INSET,
                            input_y,
                        )));
                    let _ = tree.set_prop(name_input, PROP_CONTROL_SIZE,
                        Value::Vec2(nes_scene::Vec2::new(INSPECTOR_W - 2.0 * INSPECTOR_INSET, 20.0)));

                    // Script 分区标题（F-4 挂载流）+ 正文四行：候选轮换
                    //（F6）/ 挂载（Enter）/ 卸载（U）/ enabled（E）。
                    // 行宽 11 字预算内（S12-6 口径），候选文件名超宽截断。
                    let sc_y = input_y + 20.0 + 4.0;
                    title_rows.push((ins_x, sc_y, 1));
                    tree.set_local(ins_sc_title, Transform2D::from_pos(ins_x, sc_y));
                    let _ = tree.set_prop(ins_sc_title, "visible", Value::Bool(true));
                    let _ = tree.set_prop(ins_sc_title, PROP_LABEL_TEXT,
                        Value::Str(if sc_open { "- Script" } else { "+ Script" }.into()));
                    if sc_open {
                        // 挂载目标只读解析（显示现态：enabled 只对已挂
                        // 载脚本有语义 —— 未挂载显示 "-"）。
                        let (mounted, enabled) = match mount_target(tree, p) {
                            Some((_, m, e)) => (m, e),
                            None => (false, false),
                        };
                        let cand_line: String = scripts
                            .get(script_idx)
                            .map(|r| base_name(r).chars().take(INS_LINE_CHARS).collect())
                            .unwrap_or_else(|| "-".into());
                        tree.set_local(ins_script, Transform2D::from_pos(ins_x, sc_y + INS_ROW_H));
                        let _ = tree.set_prop(ins_script, "visible", Value::Bool(true));
                        let _ = tree.set_prop(ins_script, PROP_LABEL_TEXT,
                            Value::Str(format!(
                                "mount: F6\n{}\nunmount U\nenabled: {}",
                                cand_line,
                                if !mounted { "-" } else if enabled { "Y" } else { "N" },
                            )));
                    } else {
                        let _ = tree.set_prop(ins_script, "visible", Value::Bool(false));
                    }
                }
                None => {
                    ins_text.push_str("\n(none)");
                    let _ = tree.set_prop(hud_ins, PROP_LABEL_TEXT, Value::Str(ins_text));
                    tree.set_local(hud_ins, Transform2D::from_pos(ins_x, 12.0));
                    // 无选中：分区/输入框全部隐藏（Godot 空面板直感）。
                    for n in [ins_tf_title, ins_sc_title, ins_script] {
                        let _ = tree.set_prop(n, "visible", Value::Bool(false));
                    }
                    let _ = tree.set_prop(name_input, "visible", Value::Bool(false));
                }
            }

            // 状态栏（S12-7：工具开关态 + F 键挂载流提示；SNAP 开关
            // ON 恒吸附、Ctrl 反转 —— tools 段 S/N/G 即三开关现态。
            // S12-9：运行态提示 + 运行三键口径）。
            let st = if play.playing {
                format!(
                    "st> PLAYING (F5=restart Shift+F5=stop RESET btn reverts scene) tools:{}{}{}",
                    if tool_sel_on { "S" } else { "-" },
                    if tool_snap_on { "N" } else { "-" },
                    if tool_grid_on { "G" } else { "-" },
                )
            } else {
                format!(
                    "st> undo:{} redo:{} sel:{} tools:{}{}{} | Click=sel Drag=box Del=del F5=play F8=scan F6=cand Enter=mount U=unmount E=enable F7=groups F9=split Ctrl+Z/Y=undo",
                    if log.can_undo() { "Y" } else { "-" },
                    if log.can_redo() { "Y" } else { "-" },
                    sel.len(),
                    if tool_sel_on { "S" } else { "-" },
                    if tool_snap_on { "N" } else { "-" },
                    if tool_grid_on { "G" } else { "-" },
                )
            };
            let _ = tree.set_prop(hud_st, PROP_LABEL_TEXT, Value::Str(st));

            // Selection indicator: Control rect follows primary selection.
            match sel.primary(tree) {
                Some(p) => {
                    let w = tree.world(p).unwrap_or_default();
                    let _ = tree.set_prop(sel_box, PROP_CONTROL_OFFSET,
                        Value::Vec2(nes_scene::Vec2::new(w.tx - 2.0, w.ty - 2.0)));
                }
                None => {
                    let _ = tree.set_prop(sel_box, PROP_CONTROL_OFFSET,
                        Value::Vec2(nes_scene::Vec2::new(-100.0, -100.0)));
                }
            }

            // 重命名输入框投影：有选中 → 可见且 text 绑定选中节点名。
            // 换绑**不再等失焦**（S12-4 ②③ —— 旧口径"编辑会话中不换
            // 绑"让 Tab 循环选中后输入框永远停在旧节点名上）：选中一变
            // 即重绑，草稿经 reset_text 拉到新名（持焦中同样刷新）。
            // 顺序即防污染：
            // 1) 排干滞留提交（正常帧此处必空 —— 提交只在帧内 UiVm
            //    产生、帧后即落账；防御性清空，防未来时序改动把旧绑定
            //    残值安到新选中头上）；
            // 2) rename_bound 先行换新 —— 同帧稍后 UiVm 的失焦/回车
            //    提交带着新草稿（= 新名）落到新绑定头上，值相等被落账
            //    面 unchanged 检查自然跳过；
            // 3) text 属性 + 草稿双写（reset_text 在 tree 借用外做）。
            let primary_uid = sel.primary(tree).and_then(|p| tree.uid_of(p));
            // 输入框 visible 已由上方 Inspector 分区投影按组折叠态每帧
            // 重写（Transform 组折叠 = 隐藏），此处不再重复写。
            if primary_uid != bound_sel {
                rename_sink.borrow_mut().clear();
                bound_sel = primary_uid.clone();
                *rename_bound.borrow_mut() = primary_uid.clone();
                if let Some(p) = sel.primary(tree) {
                    let name = tree.name(p).unwrap_or("").to_string();
                    let _ = tree.set_prop(name_input, "text", Value::Str(name.clone()));
                    rebind_name = Some(name);
                }
            }

            // 选中高亮：Viewport 里的 Sprite 的 z_index（*5* 标记）。
            for u in sel.uids().to_vec() {
                if let Some(id) = tree.find_by_uid(&u) {
                    if tree.kind_tag(id) == Some(nes_scene::NodeKindTag::Sprite2D) {
                        let _ = tree.set_prop(id, "z_index", Value::I64(5));
                    }
                }
            }
        }

        // 换绑草稿（tree 借用外 —— ui_vm_mut 与 tree_mut 不共存）：
        // 持焦中的旧草稿即刻作废，输入框显示跟手刷新（提取层有会话
        // 即显示草稿）。不触发 on_commit —— 换绑不是提交。
        if let Some(name) = rebind_name {
            rt.ui_vm_mut().reset_text(name_input, &name);
        }

        // IME 组合窗定位（第 2 期：**真字宽累加**，第 1 期的 10px 平均
        // 步进退役）：改名输入框持焦且可见的帧，把候选窗钉到光标真位
        // （客户区坐标）。x = 输入框视口位 + 4px 内衬 + 草稿前 caret 个
        // 字符的逐字 advance 累加（[`ime_caret_offset`]，与渲染器
        // push_ttf_label 光标条算式同源：TTF 模式 (char,字号) 度量、缺
        // 字形 .notdef 推进；位图回退按默认字体等宽 advance）。字号与
        // 输入框渲染同源（font_size 14 —— 输入框文本经提取层走同一条
        // TTF 路径，同字体同字号 = 累加值与光标条逐位一致）。本壳默认
        // 开窗 768x432 客户区==视口 1:1 直算；窗口缩放后的折算沿第 1 期
        // 口径记为已知限制。失焦/不可见帧**不调** —— IME 窗停在系统默
        // 认位（定位是持焦期间的宿主责任）。
        if rt.ui_vm_mut().focus() == Some(name_input) {
            let (visible, ox, oy) = {
                let tree = rt.tree_mut();
                let visible = tree
                    .prop(name_input, "visible")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let (ox, oy) = match tree.prop(name_input, PROP_CONTROL_OFFSET) {
                    Some(Value::Vec2(v)) => (v.x, v.y),
                    _ => (0.0, 0.0),
                };
                (visible, ox, oy)
            };
            if visible {
                // 编辑会话读口（S12-2 公开面）：草稿 + 光标一次取回
                //（无会话 = 未聚焦过，此处 focus 已保证有会话，防御性
                // 取空草稿 = 锚点回输入框原点）。
                let (draft, caret) = rt
                    .ui_vm_mut()
                    .text_state(name_input)
                    .map(|t| (t.draft, t.caret))
                    .unwrap_or_else(|| (String::new(), 0));
                let x = (ox
                    + IME_CARET_INSET
                    + ime_caret_offset(
                        ime_font.as_ref(),
                        &draft,
                        caret,
                        UI_FONT_SIZE as f32,
                        bitmap_advance,
                    )) as i32;
                let y = (oy + IME_CARET_INSET) as i32;
                rt.imm_set_caret_point(x, y);
            }
        }

        let _ = rt.emit_input_signals(&snap);
        let frame = FrameInfo::new(index, delta, elapsed, Vec2::new(viewport.0, viewport.1));
        // S12-9 帧循环分叉：运行态把观察者从 NoObserver 换成 ScriptVm
        //（原地换观察者 —— 同一运行时、同一条 tick/提取/渲染路径，不是
        // 第二运行时/第二窗口）。vm 是独立值，与 rt 无借用交集。
        let frame_result = if play.playing {
            let vm = play
                .vm
                .as_mut()
                .expect("运行态必有 VM（playing 与 vm 同生命周期）");
            rt.frame_windowed_with(&frame, vm)
        } else {
            rt.frame_windowed_with(&frame, &mut NoObserver)
        };
        match frame_result {
            Ok(Some(stats)) => {
                if stats.driver_errors > 0 {
                    eprintln!("[帧 {index}] driver_errors={}", stats.driver_errors);
                }
                transient = 0;
            }
            Ok(None) => break,
            Err(err) => {
                transient += 1;
                eprintln!("[帧 {index}] 失败（{transient}/{TRANSIENT_LIMIT}）：{err}");
                if transient >= TRANSIENT_LIMIT {
                    std::process::exit(1);
                }
            }
        }
        // S17.4 帧序契约（simulate 之后）：扩展 update 只在运行态推进
        //（play-in-editor 的生态面 —— 编辑态扩展不写树）。诊断逐行上
        // 控制台，不静默。
        if play.playing {
            for line in rt.update_extensions() {
                println!("[帧 {index}] [扩展] {line}");
            }
        }
        // 重命名提交（帧后落账 —— UiVm 钩子回调在帧内只传值）：
        // 一次提交 = 一条 Modified 事务（Inspector::modify_name）。
        // 运行态让路：改名属编辑动作 —— 滞留提交直接丢弃（输入框
        // 提交在运行态本就不该发生，防御性清空防旧草稿落账）。
        if play.playing {
            rename_sink.borrow_mut().clear();
        } else {
        for (uid, new_name) in rename_sink.borrow_mut().drain(..) {
            let tree = rt.tree_mut();
            let unchanged = tree
                .find_by_uid(&uid)
                .and_then(|id| tree.name(id))
                .is_some_and(|n| n == new_name);
            if unchanged {
                continue;
            }
            log.begin().unwrap();
            Inspector::new(tree, &mut log)
                .modify_name(&uid, &new_name)
                .unwrap();
            log.commit().unwrap();
            log_line(&editor_log, format!("rename {new_name}"));
            // 输入框 text 投影跟着落账后的新名走。
            let _ = tree.set_prop(name_input, "text", Value::Str(new_name));
        }
        }

        // 时间轴输入框提交落账（帧后 —— on_commit 钩子帧内只报值）：
        // 会话值（APPLY 落地取这里）+ text 属性双写（失焦后显示已提交
        // 值）。运行态让路（滞留提交直接丢弃 —— 与改名同一护盾口径）。
        if play.playing {
            tl_input_sinks.borrow_mut().clear();
        } else {
            for (field, value) in tl_input_sinks.borrow_mut().drain(..) {
                let node = match field {
                    0 => tl_x_in,
                    1 => tl_y_in,
                    _ => tl_ms_in,
                };
                match field {
                    0 => tl_x = value.clone(),
                    1 => tl_y = value.clone(),
                    _ => tl_ms = value.clone(),
                }
                let _ = rt.tree_mut().set_prop(node, "text", Value::Str(value));
            }
        }

        // 层级树行点击落账（帧后 —— UiVm 钩子回调在帧内只报行下标）：
        // 一次点击 = 一次 Selection::select（与视口点选同款单选替换语义；
        // 选择是会话态，不进事务不落盘）。下一帧的树投影与 selected 行
        // 高亮随之跟上。运行态让路（选择变更属编辑交互）。
        if play.playing {
            row_clicks.borrow_mut().clear();
        } else {
            for uid in row_clicks.borrow_mut().drain(..) {
                sel.select(uid);
            }
        }

        // 工具栏落账（帧后 —— UiVm 激活回调帧内只报名字）。S12-9：
        // play/stop/reset 三键任何状态都受理（PLAY 运行中 = 重启，Godot
        // 同款；STOP/RESET 越界按一行说明处理）；SEL/SNAP/GRID 开关是
        // 编辑动作 —— 运行态静默忽略（按钮可点但无效果，无日志灌水）。
        for name in tool_clicks.borrow_mut().drain(..) {
            // S18.1 时间轴按钮分流（tl_ 前缀）：通道/缓动/模式循环换档 +
            // APPLY 落地。编辑态专属 —— 运行态静默忽略（补间创建是编辑
            // 动作，与 SEL/SNAP/GRID 同一护盾口径）。
            if let Some(tl) = name.strip_prefix("tl_") {
                if play.playing {
                    continue;
                }
                match tl {
                    "pos" | "scale" | "alpha" => {
                        tl_channel = match tl {
                            "pos" => 0,
                            "scale" => 1,
                            _ => 2,
                        };
                        log_line(
                            &editor_log,
                            format!("tween: channel {}", tl),
                        );
                    }
                    "ease" => {
                        tl_ease_idx = (tl_ease_idx + 1) % TL_EASINGS.len();
                        log_line(
                            &editor_log,
                            format!("tween: easing {}", TL_EASINGS[tl_ease_idx]),
                        );
                    }
                    "mode" => {
                        tl_mode_idx = (tl_mode_idx + 1) % TL_MODES.len();
                        log_line(
                            &editor_log,
                            format!("tween: mode {}", TL_MODES[tl_mode_idx]),
                        );
                    }
                    "apply" => {
                        // APPLY 语义（S18.1 冻结）：**从当前值起算** ——
                        // 经树宿主 API register_tween_channel 落地，from
                        // 在登记处按通道采样当前实际值（= "从现在走到目
                        // 标"，与脚本 Cmd 落地逐位同源）。输入非数值 /
                        // 无选中 / 参数拒收 → Output 报一行，不落地。
                        let target = sel.primary(rt.tree_mut());
                        let parsed = (
                            tl_x.trim().parse::<f32>().ok(),
                            tl_y.trim().parse::<f32>().ok(),
                        );
                        let ms = tl_ms.trim().parse::<f64>().ok();
                        let channel = match tl_channel {
                            0 => match parsed {
                                (Some(x), Some(y)) => Some(nes_scene::TweenChannel::Pos {
                                    from: nes_scene::Vec2::ZERO,
                                    to: nes_scene::Vec2::new(x, y),
                                }),
                                _ => None,
                            },
                            1 => match parsed {
                                (Some(x), Some(y)) => Some(nes_scene::TweenChannel::Scale {
                                    from: nes_scene::Vec2::ZERO,
                                    to: nes_scene::Vec2::new(x, y),
                                }),
                                _ => None,
                            },
                            // alpha 单值复用 x 框（y 框无语义，不参与解析）。
                            _ => parsed
                                .0
                                .map(|a| nes_scene::TweenChannel::Alpha { from: 0.0, to: a }),
                        };
                        let easing = nes_scene::TweenEasing::from_str_exact(TL_EASINGS[tl_ease_idx])
                            .expect("TL_EASINGS 与 TweenEasing 合法名表同序");
                        let mode = nes_scene::TweenMode::from_str_exact(TL_MODES[tl_mode_idx])
                            .expect("TL_MODES 与 TweenMode 合法名表同序");
                        let Some(channel) = channel else {
                            log_line(
                                &editor_log,
                                format!(
                                    "tween: bad input ({} x='{}' y='{}')",
                                    TL_CHANNELS[tl_channel].to_ascii_lowercase(),
                                    tl_x,
                                    tl_y
                                ),
                            );
                            continue;
                        };
                        let Some(ms) = ms.filter(|m| m.is_finite() && *m > 0.0) else {
                            log_line(&editor_log, format!("tween: bad ms '{}'", tl_ms));
                            continue;
                        };
                        let Some(node) = target else {
                            log_line(&editor_log, "tween: no selection".into());
                            continue;
                        };
                        let (node_name, ok) = {
                            let tree = rt.tree_mut();
                            let name = tree.name(node).unwrap_or("?").to_string();
                            let ok = tree.register_tween_channel(node, channel, ms, easing, mode);
                            (name, ok)
                        };
                        if ok {
                            // from 不进日志（登记处采样 —— 行投影/时间轴
                            // 现态即真相；to/时长/缓动/模式照输入回显）。
                            let line = if tl_channel == 2 {
                                format!(
                                    "tween alpha {} to={} ms={} {} {}",
                                    node_name,
                                    parsed.0.unwrap_or(0.0),
                                    ms,
                                    TL_EASINGS[tl_ease_idx],
                                    TL_MODES[tl_mode_idx]
                                )
                            } else {
                                format!(
                                    "tween {} {} to=({},{}) ms={} {} {}",
                                    TL_CHANNELS[tl_channel].to_ascii_lowercase(),
                                    node_name,
                                    parsed.0.unwrap_or(0.0),
                                    parsed.1.unwrap_or(0.0),
                                    ms,
                                    TL_EASINGS[tl_ease_idx],
                                    TL_MODES[tl_mode_idx]
                                )
                            };
                            log_line(&editor_log, line);
                        } else {
                            log_line(&editor_log, "tween: rejected (invalid target/params)".into());
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match name.as_str() {
                "play" => {
                    drag_start = None;
                    gizmo = None;
                    play.start(&mut rt, &assets, &editor_log);
                }
                "stop" => {
                    if play.playing {
                        play.stop(&mut rt, &editor_log);
                    } else {
                        log_line(&editor_log, "stop: not playing".into());
                    }
                }
                "reset" => play.reset(&mut rt, &editor_log),
                _ => {
                    if play.playing {
                        continue;
                    }
                    let on = match name.as_str() {
                        "sel" => {
                            tool_sel_on = !tool_sel_on;
                            tool_sel_on
                        }
                        "snap" => {
                            tool_snap_on = !tool_snap_on;
                            tool_snap_on
                        }
                        _ => {
                            tool_grid_on = !tool_grid_on;
                            tool_grid_on
                        }
                    };
                    log_line(
                        &editor_log,
                        format!("tool {} {}", name, if on { "on" } else { "off" }),
                    );
                }
            }
        }

        // 文件系统 dock 行点击落账（帧后 —— UiVm 钩子帧内只报行下
        // 标）：单击 = 选中该行（会话态，下一帧 selected 行高亮跟上）；
        // .nes 顺手指为 F6 候选起点（两处入口同一挂载流，池与 res://
        // 树同源必命中）。同行 30 帧内两次点击沿 = 双击（FS_DBLCLICK_
        // FRAMES 裁决；UiVm 行回调只有单击沿，双击是宿主会话态的边沿
        // 合成）：.ron 场景 = Output 提示（场景打开归后续里程碑，P0
        // 不实现）；.nes = 直接挂载（与 Enter 同一 mount_script 事务，
        // Output 报结果）；目录/其余后缀 = 提示，不落账。运行态让路
        //（挂载/选中都是编辑动作，滞留点击直接丢弃）。
        if play.playing {
            fs_clicks.borrow_mut().clear();
        } else {
        for row in fs_clicks.borrow_mut().drain(..) {
            let Some(entry) = fs_entries.get(row) else {
                continue;
            };
            fs_sel = Some(row);
            let double = fs_last_press.is_some_and(|(f, r)| {
                r == row && index.saturating_sub(f) < FS_DBLCLICK_FRAMES
            });
            if double {
                if entry.is_dir {
                    log_line(
                        &editor_log,
                        format!(
                            "fs: dir {} (flat view)",
                            base_name(entry.rel.trim_end_matches('/')),
                        ),
                    );
                } else if entry.rel.ends_with(".ron") {
                    log_line(
                        &editor_log,
                        format!(
                            "open {} -> play-in-editor milestone",
                            base_name(&entry.rel),
                        ),
                    );
                } else if entry.rel.ends_with(".nes") {
                    log_line(&editor_log, format!("fs open {}", base_name(&entry.rel)));
                    mount_script(rt.tree_mut(), &mut log, &sel, &editor_log, &entry.rel);
                } else {
                    log_line(
                        &editor_log,
                        format!("fs: no action for .{}", extension_suffix(&entry.rel)),
                    );
                }
            } else if !entry.is_dir && entry.rel.ends_with(".nes") {
                if let Some(i) = scripts.iter().position(|s| *s == entry.rel) {
                    script_idx = i;
                }
            }
            fs_last_press = Some((index, row));
        }
        }

        // 帧节拍：无固定 sleep —— present 的 FIFO 队列自节流（vsync），
        // 帧差以 Instant 实测进 FrameInfo（见循环头的 delta/elapsed）。
    }
    // 自动化钩子断言（仅 NES_EDIT_DEMO=1）：Output 日志与树形态双验。
    if demo {
        let lines: Vec<String> = editor_log.borrow().iter().cloned().collect();
        let has = |p: &str| lines.iter().any(|l| l.contains(p));
        assert!(has("cand spin.nes"), "F6 轮换失败：{lines:?}");
        assert!(has("mount obj1 (+script) <- spin.nes"), "挂载失败：{lines:?}");
        assert!(has("script enabled -"), "enabled 切换失败：{lines:?}");
        assert!(has("unmount"), "卸载失败：{lines:?}");
        assert!(
            has("groups stage 1") && has("groups stage 2"),
            "分组折叠失败：{lines:?}"
        );
        assert!(has("scan 2 script(s)"), "候选池刷新失败：{lines:?}");
        // S12-8：FileSystem 双击 —— fs open 分派提示 + 与 Enter 同款
        // 挂载事务（目标 Script 子节点已存在 -> 无 "(+script)" 后缀，
        // 与首挂载日志可区分）；末尾 U 卸载回空 registry_key（上方树
        // 形态断言不受影响）。
        assert!(has("fs open spin.nes"), "fs double-click dispatch failed: {lines:?}");
        assert!(has("mount obj1 <- spin.nes"), "fs double-click mount failed: {lines:?}");
        // S12-9：play-in-editor 全链路 —— 快照提示 + play 行 + stop +
        // reset 日志齐备；运行态取证：spin 脚本在运行态把宿主节点挪了
        // 位（demo_spin_x > 0），工具栏 PLAY 文本带 *（运行中）；RESET
        // 后 spin 位置回 0（快照数据面还原 = 回到运行前；uid 与 NodeId
        // 不动 —— find_by_name 命中的还是同一个节点）。
        assert!(has("snapshot "), "play snapshot hint missing: {lines:?}");
        assert!(has("play (1 scripts)"), "play log missing: {lines:?}");
        // S13 第 2 期：PLAY 会话自动开音频（场景含 Sound 资源）—— Output
        // 必有 "audio on" 行（无设备环境记 "audio: ..." 错误行，钩子按
        // 实际能力断言其一 —— "没有设备"与"接线断了"不许互装）。
        assert!(
            has("audio on") || has("audio: "),
            "PLAY 后无音频日志（audio on / audio: 错误行均缺）：{lines:?}"
        );
        assert!(has("stop"), "stop log missing: {lines:?}");
        assert!(has("reset"), "reset log missing: {lines:?}");
        // S15：视频接入取证（演示 AMV 在场时；用户机器资产，缺失整段
        // 天然跳过）。PLAY 自动起播 + STOP 停播两行齐备，且无 "video:"
        // 错误行（错误行约定 = audio: 同款）。
        if video_present {
            assert!(has("video on"), "video on log missing: {lines:?}");
            assert!(has("video stopped"), "video stopped log missing: {lines:?}");
            assert!(
                !lines.iter().any(|l| l.starts_with("video:")),
                "video error line leaked: {lines:?}"
            );
        }
        assert!(demo_spin_x > 0.0, "脚本未在运行态驱动（spin.x={demo_spin_x}）");
        assert_eq!(demo_play_text, "PLAY*", "运行中 PLAY 文本应为 PLAY*");
        // IME 第 1 期：Char(0x4E2D) 端到端 —— inject_input（WM_CHAR 口径）
        // → 快照 → SnapshotView → UiVm Unicode 泵 → 草稿 "obj1中"（光标
        // 5，不属断言面但同源）。S12-11 复跑：真字体模式下同一链路照旧
        //（TTF 只换排版，不改数据面 —— 改名流程既有断言不动应仍绿）。
        assert_eq!(demo_ime_draft, "obj1中", "IME Char(0x4E2D) 未入改名框草稿：{demo_ime_draft:?}");
        // S12-11：真字体装载取证 —— 探测链必落一行（命中或回退）；真字
        // 体模式下无回退行、无解析失败行（"Output 无字体错误行"契约）；
        // 位图回退模式必有回退行（优雅降级可观察）。
        assert!(has("font: "), "font probe log missing: {lines:?}");
        assert_eq!(
            ttf_active,
            !has("font: bitmap fallback"),
            "ttf state vs fallback log mismatch: {lines:?}"
        );
        if ttf_active {
            assert!(
                !lines.iter().any(|l| l.contains("ttf parse failed")),
                "font parse error leaked: {lines:?}"
            );
        }
        // S14：用户音乐接入取证（音乐在场时；文件不在仓库的机器整段
        // 天然跳过）。三态循环按实际装载的曲目数取证 —— 首曲必然经过、
        // 双曲时第二曲也经过、循环尾必然回停态；行内容全 ASCII
        //（"flac"/"mp3" 标签 —— 中文文件名是用户数据，不进日志/断言）。
        if !music_tracks.is_empty() {
            assert!(has("music loaded"), "music loaded log missing: {lines:?}");
            let (first_key, first_label) = music_tracks[0];
            assert!(
                has(&format!("music: {first_label} (looped)")),
                "music first track ({first_key}) state missing: {lines:?}"
            );
            if music_tracks.len() > 1 {
                assert!(
                    has("music: mp3 (looped)"),
                    "music second track state missing: {lines:?}"
                );
            }
            assert!(has("music: stopped"), "music stop state missing: {lines:?}");
        }
        {
            let tree = rt.tree_mut();
            let spin = tree.find_by_name("spin").expect("spin 节点存活（uid/NodeId 不动）");
            let x = tree.local(spin).unwrap_or_default().pos.x;
            assert_eq!(x, 0.0, "RESET 后 spin 回到运行前位置（实际 {x}）");
        }
        // S18.1：时间轴创建流全链路 —— APPLY 落地日志（输入框提交的
        // x=2/y=4 + 缺省 ms=500）+ 时间轴行投影含 POS 行（选中节点的
        // 活动补间可视化）+ 两个闩锁（活动补间真实入表 → 推进到站自然
        // 移除；帧差毫秒基准，与刷新率无关）。
        assert!(
            has("tween pos obj1 to=(2,4) ms=500 linear once"),
            "timeline APPLY tween log missing: {lines:?}"
        );
        assert!(
            demo_tl_rows.contains("POS"),
            "timeline rows missing POS row: {demo_tl_rows:?}"
        );
        assert!(demo_tl_seen_active, "APPLY 后登记表未见活动补间");
        assert!(
            demo_tl_seen_done,
            "补间未见到站移除（ms=500 应在演示窗内自然完成）"
        );
        // 树形态：挂载 Script 子节点留存（名字 = 脚本基名），registry_key
        // 已回空串（卸载），enabled = false（切换后未回改）。
        let tree = rt.tree_mut();
        let obj1 = tree.find_by_name("obj1").expect("obj1 存在");
        let kids = tree.children(obj1).to_vec();
        assert_eq!(kids.len(), 1, "挂载节点留存");
        assert_eq!(tree.name(kids[0]), Some("spin"));
        assert_eq!(
            tree.prop(kids[0], "registry_key"),
            Some(&Value::Str(String::new())),
            "卸载 = registry_key 回空串"
        );
        assert_eq!(tree.prop(kids[0], "enabled"), Some(&Value::Bool(false)));
        println!("[demo] 挂载/卸载/enabled/折叠/刷新/play/stop/reset/时间轴 APPLY 冒烟断言通过");
    }
    println!("[完成] Editor Shell 退出");
    let _ = (grid, cam, hud_tree, hud_ins_bg, hud_ins, hud_st, sel_box, name_input, hud_scene, tool_bg, tool_sep, theme_node, tool_plates, ins_tf_title, ins_sc_title, fsdock, fs_bg, fs_title, fs_sep, fs_tree, tldock, tl_bg, tl_title, hud_tl, tl_bars, tl_new_label, tl_to_label, tl_ms_label, tl_plates, tl_pos, tl_scale, tl_alpha, tl_ease, tl_mode, tl_apply, tl_x_in, tl_y_in, tl_ms_in);
}
