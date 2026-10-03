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
//! ② F-4 挂载流（键盘两步）：F5 扫 `Scripts/*.nes`（资产根相对路径，
//!    另每 60 帧自动刷）→ F6 轮换候选 → Enter 挂载 → U 卸载 → E 切
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
//! 运行：`cargo run --example editor_shell`

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use nes_render_api::input::{InputEvent, Key, MouseButton};
use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::{PROP_CONTROL_ANCHOR, PROP_CONTROL_OFFSET, PROP_CONTROL_SIZE, PROP_LABEL_TEXT, PROP_TEXTURE};
use nes_render_wgpu::window::inject_input;
use nes_render_wgpu::{bmp, FontParams};
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::editor::{Hierarchy, Inspector, Selection};
use nes_scene::transaction::TransactionLog;
use nes_scene::{NodeKind, ScriptVm, Transform2D, Value, Uid};

fn solid_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    [r, g, b, 255].repeat(16 * 16)
}

/// 布局常量（S12-4 冻结、S12-6 扩底部 dock）：面板**恒定宽** —— 最大化
/// 只扩中间世界视口，侧面板不跟着拉伸（消除"整个画面被拉长"观感的关键）。
/// - 左层级面板：x = 8..188（宽 180），y = 40..ch-dock 上缘；
/// - 右检查器面板：x = cw-198..cw-8（宽 190），y = 8..ch-dock 上缘；
/// - 底部 Output dock：高 96，y = ch-dock-状态栏..ch-状态栏，全宽；
/// - 状态栏文本：y = ch-20（底部 16 文本 + 8 边距）；
/// - 视口可编辑区 = 两面板之间再让出顶/左各 16px 标尺（标尺不属于
///   可编辑区，Godot 口径）：视口高 = ch - 40 - (dock 96 + 状态栏 24)。
const MARGIN: f32 = 8.0;
const LEFT_PANEL_W: f32 = 180.0;
const INSPECTOR_W: f32 = 190.0;
const TOP_BAND: f32 = 40.0;
const STATUS_BAND: f32 = 24.0;

/// 底部 Output dock 高度（Godot 底部"输出"面板观感）：标题行 + 日志
/// 行列表；视口与状态栏让出这 96px。
const DOCK_H: f32 = 96.0;
/// dock 日志行行高（与 ListView `row_h` 同值；渲染器行 y = 矩形顶
/// +4 + i*row_h，故可见行数 = (列表高-4) / 18 向下取整 = 4 行）。
const DOCK_ROW_H: f32 = 18.0;
/// 编辑器日志环形保留行数（新行在下，满 N 丢最旧 —— Godot Output
/// 的最小语义；可见窗只放最新能放下的几行，最新行永远可见）。S12-8
/// 起 12 行：冒烟钩子要同时断言 Enter 与 FileSystem 双击**两条挂载
/// 路径**的日志（一轮流程恰好 12 行，早期行不再被新行挤出断言窗；
/// dock 可见窗仍只显最新几行，显示面不变）。
const EDITOR_LOG_KEEP: usize = 12;
/// dock 行显示截宽（字符数）：40 字 × 16px advance = 640px，最小窗
/// 768 下 dock 内衬（≈748px）也放得下，行尾不裁字。
const DOCK_LINE_CHARS: usize = 40;

/// 文件系统 dock（S12-8，Godot 左下 res:// 面板）布局常量：
/// - 分隔条厚度（4px border 槽条）与标题行高（"res:/" 16px 文本行）；
/// - P0 布局裁决：**固定分割 + F9 两档** —— 不做拖拽，F9 在
///   "Scene 55% / FileSystem 40%" 与 "Scene 40% / FileSystem 55%"
///   两档间切换（焦点段占大头）；比例是编辑器会话态，不进树。
const FS_SEP_H: f32 = 4.0;
/// FileSystem 标题行高（"res:/" 一行 16px，与默认文本行高同口径）。
const FS_TITLE_H: f32 = 16.0;
/// Scene / FileSystem 分割比（上段 = Scene；F9 切到 ALT 档）。
const FS_SPLIT_TOP: f32 = 0.55;
const FS_SPLIT_ALT: f32 = 0.40;
/// fs 列表行高（与 dock 行高同值；行 y = 列表顶 +4 + i*row_h）。
const FS_ROW_H: f32 = 18.0;
/// 资产扫描深度（P0 两层条目：根一层 + 子目录一层）。
const FS_SCAN_DEPTH: usize = 2;
/// 资产白名单后缀（`.` 隐藏项与无后缀垃圾一律不进树）。
const FS_EXT_WHITELIST: [&str; 6] = ["nes", "bmp", "png", "ron", "ttf", "txt"];
/// 双击裁决窗（帧）：同行两次行点击报告沿间隔 <30 帧 = 双击。UiVm
/// 行回调只有单击 —— 双击是宿主会话态的边沿合成（60fps 下 <0.5s，
/// 与鼠标双击时长同量级；行回调沿 = 抬键沿，与按下沿间隔至差一帧，
/// 同一裁决口径）。
const FS_DBLCLICK_FRAMES: u64 = 30;

/// 2D 标尺条带厚度（Godot 2D 视口顶横/左竖刻度尺观感）。
const RULER_W: f32 = 16.0;
/// 标尺最小刻度间距（1px 细条）；数字标签每 2 格（=128px）一个
/// —— 渲染器字形只按字体单元一种字号展开（font_size 不参与缩放，
/// 已查证 nes-render-wgpu 展开路径），16px 等宽下 128px 密度放得下。
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

/// 视口工具栏高度（Godot 2D 视口顶部工具条观感）：标尺之上的一条
/// 工具带，panel 槽铺底 + 底缘 1px border 槽分隔线。
const TOOLBAR_H: f32 = 24.0;
/// 工具栏按钮尺寸与步进（48px 宽按钮 + 4px 缝，20px 高贴 24px 带）。
const TOOLBAR_BTN_W: f32 = 48.0;
const TOOLBAR_BTN_H: f32 = 20.0;
const TOOLBAR_BTN_STEP: f32 = 52.0;

/// F5/F6/F7/F9 的 Win32 虚拟键码。Key 契约未列举 F 键 —— 平台层把未列举
/// 虚拟键原样保留为 `Key::Other(原码)`（vk_to_key 兜底分支），边缘
/// 检测直接按 `Key::Other(VK_*)` 比对 pressed 集。选 F 键有个工程
/// 理由：F 键不产生 WM_CHAR 文本 —— 与改名输入框的键入天然无冲突
/// （字母键做不到）。
const VK_F5: u32 = 0x74;
const VK_F6: u32 = 0x75;
const VK_F7: u32 = 0x76;
/// F9：左栏 Scene/FileSystem 分割档切换（S12-8，两档见 FS_SPLIT_*）。
const VK_F9: u32 = 0x78;

/// 脚本候选池自动刷新周期（帧）：std::fs::read_dir 每帧调用 = 每帧
/// 一次目录枚举 + 若干次分配，60fps 下纯属浪费。裁决：**每 60 帧
/// （约 1s）自动刷一次 + F5 手动即时刷** —— 不用每帧（代价无谓），
/// 也不只靠 F5（外部增删 .nes 文件要等按键才可见，观感差）。
const SCRIPT_SCAN_EVERY: u64 = 60;
/// Inspector 行内字符预算（S12-6 口径：面板内衬宽 178px、advance=16
/// ≈ 11 字/行）—— 候选文件名显示按此截断。
const INS_LINE_CHARS: usize = 11;
/// 输入框/面板内容的水平内衬。
const INSPECTOR_INSET: f32 = 6.0;

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
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 5);
    assert_eq!(rt.upload_pending_textures().expect("上传"), 5);
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
        rt.consumer_mut()
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
    let (grid, grid_bars, ruler, ruler_h, ruler_v, ruler_corner, ruler_ticks, ruler_labels, dock, dock_bg, dock_title, hud_dock, toolbar, tool_bg, tool_sep, tool_sel, tool_snap, tool_grid, ins_tf_title, ins_sc_title, ins_script, cam, obj1, obj2, obj3, hud_tree, hud_ins_bg, hud_ins, hud_st, sel_box, name_input, hud_scene, fsdock, fs_bg, fs_title, fs_sep, fs_tree) = {
        let tree = rt.tree_mut();
        let root = tree.root();
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
            let _ = tree.set_prop(bar, "fill_slot", Value::Str("border".into()));
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
            let _ = tree.set_prop(tick, "fill_slot", Value::Str("border".into()));
            let _ = tree.set_prop(tick, "visible", Value::Bool(false));
            tree.set_prop_raw(tick, "z_index", Value::I64(-90));
            ruler_ticks.push(tick);
        }
        // 数字标签池：Label 空文本 = 提取层判空不上屏（免 visible 接
        // 线）。Godot 竖标尺数字是旋转 90° 排版，等宽点阵字体先横排
        // —— 3 位数（48px 宽）会溢出 16px 条带压到视口最左缘，观感
        // 等同刻度注记，取舍记此。
        let mut ruler_labels = Vec::with_capacity(RULER_LABELS_H + RULER_LABELS_V);
        for _ in 0..RULER_LABELS_H + RULER_LABELS_V {
            let lab = tree.add_node(ruler, "ruler_label", NodeKind::Label);
            tree.set_local(lab, Transform2D::from_pos(-1000.0, -1000.0));
            let _ = tree.set_prop(lab, PROP_LABEL_TEXT, Value::Str(String::new()));
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
        let _ = tree.set_prop(dock_bg, "fill_slot", Value::Str("panel".into()));
        tree.set_prop_raw(dock_bg, "z_index", Value::I64(-80));
        let dock_title = tree.add_node(dock, "dock_title", NodeKind::Label);
        tree.set_local(dock_title, Transform2D::from_pos(MARGIN + 2.0, 320.0));
        let _ = tree.set_prop(dock_title, PROP_LABEL_TEXT, Value::Str("Output".into()));
        tree.set_prop_raw(dock_title, "z_index", Value::I64(-80));
        let hud_dock = tree.add_node(dock, "hud_dock", NodeKind::ListView);
        let _ = tree.set_prop(hud_dock, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(hud_dock, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(MARGIN, 320.0)));
        let _ = tree.set_prop(hud_dock, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(600.0, DOCK_H - 18.0 - 2.0)));
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
        let _ = tree.set_prop(fs_bg, "fill_slot", Value::Str("panel".into()));
        tree.set_prop_raw(fs_bg, "z_index", Value::I64(-60));
        let fs_title = tree.add_node(fsdock, "fs_title", NodeKind::Label);
        tree.set_local(fs_title, Transform2D::from_pos(MARGIN + 2.0, 242.0));
        let _ = tree.set_prop(fs_title, PROP_LABEL_TEXT, Value::Str("res:/".into()));
        tree.set_prop_raw(fs_title, "z_index", Value::I64(-60));
        let fs_sep = tree.add_node(fsdock, "fs_sep", NodeKind::Control);
        let _ = tree.set_prop(fs_sep, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(fs_sep, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(MARGIN, 236.0)));
        let _ = tree.set_prop(fs_sep, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, FS_SEP_H)));
        let _ = tree.set_prop(fs_sep, "fill_slot", Value::Str("border".into()));
        tree.set_prop_raw(fs_sep, "z_index", Value::I64(-60));
        let fs_tree = tree.add_node(fsdock, "fs_tree", NodeKind::ListView);
        let _ = tree.set_prop(fs_tree, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(fs_tree, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(MARGIN, 258.0)));
        let _ = tree.set_prop(fs_tree, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, 80.0)));
        let _ = tree.set_prop(fs_tree, "rows", Value::Str(String::new()));
        let _ = tree.set_prop(fs_tree, "row_h", Value::I64(FS_ROW_H as i64));
        let _ = tree.set_prop(fs_tree, "selected", Value::I64(-1));
        tree.set_prop_raw(fs_tree, "z_index", Value::I64(-60));
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
        let _ = tree.set_prop(tool_bg, "fill_slot", Value::Str("panel".into()));
        tree.set_prop_raw(tool_bg, "z_index", Value::I64(-70));
        let tool_sep = tree.add_node(toolbar, "tool_sep", NodeKind::Control);
        let _ = tree.set_prop(tool_sep, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_sep, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(188.0, TOP_BAND + TOOLBAR_H - 1.0)));
        let _ = tree.set_prop(tool_sep, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(380.0, 1.0)));
        let _ = tree.set_prop(tool_sep, "fill_slot", Value::Str("border".into()));
        tree.set_prop_raw(tool_sep, "z_index", Value::I64(-70));
        let tool_sel = tree.add_node(toolbar, "tool_sel", NodeKind::Button);
        let _ = tree.set_prop(tool_sel, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_sel, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(192.0, TOP_BAND + 2.0)));
        let _ = tree.set_prop(tool_sel, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(TOOLBAR_BTN_W, TOOLBAR_BTN_H)));
        let _ = tree.set_prop(tool_sel, "text", Value::Str("SEL".into()));
        let tool_snap = tree.add_node(toolbar, "tool_snap", NodeKind::Button);
        let _ = tree.set_prop(tool_snap, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_snap, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(192.0 + TOOLBAR_BTN_STEP, TOP_BAND + 2.0)));
        let _ = tree.set_prop(tool_snap, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(TOOLBAR_BTN_W, TOOLBAR_BTN_H)));
        let _ = tree.set_prop(tool_snap, "text", Value::Str("SNAP".into()));
        let tool_grid = tree.add_node(toolbar, "tool_grid", NodeKind::Button);
        let _ = tree.set_prop(tool_grid, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::ZERO));
        let _ = tree.set_prop(tool_grid, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(192.0 + 2.0 * TOOLBAR_BTN_STEP, TOP_BAND + 2.0)));
        let _ = tree.set_prop(tool_grid, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(TOOLBAR_BTN_W, TOOLBAR_BTN_H)));
        let _ = tree.set_prop(tool_grid, "text", Value::Str("GRID".into()));
        let cam = tree.add_node(root, "cam", NodeKind::Camera2D);
        tree.set_local(cam, Transform2D::from_pos(384.0, 216.0));
        let obj1 = tree.add_node(root, "obj1", NodeKind::Sprite2D);
        tree.set_prop(obj1, PROP_TEXTURE, Value::Resource(1)).unwrap();
        tree.set_local(obj1, Transform2D::from_pos(280.0, 180.0));
        let obj2 = tree.add_node(root, "obj2", NodeKind::Sprite2D);
        tree.set_prop(obj2, PROP_TEXTURE, Value::Resource(2)).unwrap();
        tree.set_local(obj2, Transform2D::from_pos(380.0, 180.0));
        let obj3 = tree.add_node(root, "obj3", NodeKind::Sprite2D);
        tree.set_prop(obj3, PROP_TEXTURE, Value::Resource(3)).unwrap();
        tree.set_local(obj3, Transform2D::from_pos(480.0, 180.0));
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
        tree.set_prop(hud_ins_bg, "fill_slot", Value::Str("panel".into())).unwrap();
        // Inspector 标题 + 选中信息（短文本：标题一行 + 信息另起，
        // "(none)" = 无选中）。位置每帧投影（x = cw-190, y = 12，跟随
        // 右面板）—— 修 S12-4 ⑤"标题被表面边缘裁剪"。
        let hud_ins = tree.add_node(root, "hud_ins", NodeKind::Label);
        tree.set_local(hud_ins, Transform2D::from_pos(578.0, 12.0));
        tree.set_prop(hud_ins, PROP_LABEL_TEXT, Value::Str(String::new())).unwrap();
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
        tree.set_prop(sel_box, "border_slot", Value::Str("accent".into())).unwrap();
        tree.set_prop_raw(sel_box, "border_w", Value::F32(2.0));
        tree.set_prop_raw(sel_box, "z_index", Value::I64(100));

        // 左面板标题（S12-5 Godot 命名）：与右侧 Inspector 标题同款 Label。
        // 左面板 x 恒定（MARGIN），位置装配期一次写定即可，无需每帧投影。
        let hud_scene = tree.add_node(root, "hud_scene", NodeKind::Label);
        tree.set_local(hud_scene, Transform2D::from_pos(MARGIN + 2.0, 12.0));
        tree.set_prop(hud_scene, PROP_LABEL_TEXT, Value::Str("Scene".into())).unwrap();

        let hud_st = tree.add_node(root, "hud_st", NodeKind::Label);
        tree.set_local(hud_st, Transform2D::from_pos(8.0, 410.0));
        tree.set_prop(hud_st, PROP_LABEL_TEXT, Value::Str(String::new())).unwrap();
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
        // Inspector 分区标题（S12-7 Godot 分组观感）：text_dim 色小节
        // 标题，"+" 折叠 / "-" 展开；点击标题行（宿主矩形命中）或 F7
        // 切换折叠。位置/文本每帧投影（跟随右面板与组布局）。
        // ins_script 是 Script 分区正文（mount/unmount/enabled 行）。
        let ins_tf_title = tree.add_node(root, "ins_tf_title", NodeKind::Label);
        tree.set_local(ins_tf_title, Transform2D::from_pos(-1000.0, -1000.0));
        let _ = tree.set_prop(ins_tf_title, PROP_LABEL_TEXT, Value::Str("- Transform".into()));
        let _ = tree.set_prop(ins_tf_title, "color_slot", Value::Str("text_dim".into()));
        let ins_sc_title = tree.add_node(root, "ins_sc_title", NodeKind::Label);
        tree.set_local(ins_sc_title, Transform2D::from_pos(-1000.0, -1000.0));
        let _ = tree.set_prop(ins_sc_title, PROP_LABEL_TEXT, Value::Str("- Script".into()));
        let _ = tree.set_prop(ins_sc_title, "color_slot", Value::Str("text_dim".into()));
        let ins_script = tree.add_node(root, "ins_script", NodeKind::Label);
        tree.set_local(ins_script, Transform2D::from_pos(-1000.0, -1000.0));
        let _ = tree.set_prop(ins_script, PROP_LABEL_TEXT, Value::Str(String::new()));
        tree.apply_pending();
        (grid, grid_bars, ruler, ruler_h, ruler_v, ruler_corner, ruler_ticks, ruler_labels, dock, dock_bg, dock_title, hud_dock, toolbar, tool_bg, tool_sep, tool_sel, tool_snap, tool_grid, ins_tf_title, ins_sc_title, ins_script, cam, obj1, obj2, obj3, hud_tree, hud_ins_bg, hud_ins, hud_st, sel_box, name_input, hud_scene, fsdock, fs_bg, fs_title, fs_sep, fs_tree)
    };
    let _ = (obj1, obj2, obj3);

    // 编辑器状态（会话态 —— 不进事务、不落盘）。
    let mut sel = Selection::new();
    let mut log = TransactionLog::new();
    let mut vm = ScriptVm::new();
    rt.mount_input_view(&mut vm);
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
    // UiVm 提交钩子的落点（UiVm 零写权 —— 值经共享缓冲传回宿主，
    // 宿主帧后落 Inspector::modify_name 一条 Modified 事务）。
    let rename_sink: Rc<RefCell<Vec<(Uid, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let rename_bound: Rc<RefCell<Option<Uid>>> = Rc::new(RefCell::new(None));
    {
        let sink = rename_sink.clone();
        let bound = rename_bound.clone();
        rt.ui_vm_mut().on_commit(move |_node, value| {
            if let Value::Str(name) = value {
                if let Some(uid) = bound.borrow().clone() {
                    sink.borrow_mut().push((uid, name));
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
    // 工具栏按钮激活落点（UiVm 零写权延续 —— 帧内报按钮名，帧后宿主
    // 翻开关并记日志；与行点击同一共享缓冲模式）。
    let tool_clicks: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let names: std::collections::BTreeMap<nes_scene::NodeId, &str> = [
            (tool_sel, "sel"),
            (tool_snap, "snap"),
            (tool_grid, "grid"),
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
                70 => inject_input(InputEvent::Key { key: Key::Other(VK_F5), down: true }),
                72 => inject_input(InputEvent::Key { key: Key::Other(VK_F5), down: false }),
                // S12-8：FileSystem 双击挂载 —— 鼠标先移到 fs 树
                // spin.nes 行（(60, 256)：768x432 客户区、默认 Scene
                // 55% 档下第 2 行），两次点击沿间隔 10 帧 < 30（双击
                // 裁决窗），挂载后 U 卸载回空 registry_key（树形态
                // 断言兼容）。
                80 => inject_input(InputEvent::MouseMove { x: 60.0, y: 256.0 }),
                82 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
                84 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
                90 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: true }),
                92 => inject_input(InputEvent::MouseButton { button: MouseButton::Left, down: false }),
                100 => inject_input(InputEvent::Key { key: Key::U, down: true }),
                102 => inject_input(InputEvent::Key { key: Key::U, down: false }),
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
        // F5 刷资产扫描 / F6 轮换候选 / F7 循环组折叠 / F9 切左栏分割
        // 档；Enter 挂载、U 卸载、E 切 enabled（后三者有焦点门，见挂
        // 载段）。
        let (f5_now, f6_now, f7_now, f9_now, enter_now, u_now, e_now) = (
            snap.pressed.contains(&Key::Other(VK_F5)),
            snap.pressed.contains(&Key::Other(VK_F6)),
            snap.pressed.contains(&Key::Other(VK_F7)),
            snap.pressed.contains(&Key::Other(VK_F9)),
            snap.pressed.contains(&Key::Enter),
            snap.pressed.contains(&Key::U),
            snap.pressed.contains(&Key::E),
        );
        // 点击选择（hit 命中 + Selection）：左键单选 / Shift+左键多选。
        if tab_now && !prev_tab {
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
        // Label 无 anchor/size —— 矩形 = 标题行整条面板宽 x 16px 行高。
        let title_click: Option<usize> = title_rows
            .iter()
            .find(|(rx, ry, _)| {
                mx >= *rx && mx < *rx + INSPECTOR_W && my >= *ry && my < *ry + 16.0
            })
            .map(|(_, _, gi)| *gi);
        if mouse_left_held && !prev_click && title_click.is_none() && tool_sel_on {
            // hit 在脚本中做；宿主侧直接查树（与 hit 同逻辑的 Rust 版）。
            // 压在编辑器 UI（改名输入框 / 层级树 / Output dock / 标尺
            // 条带）上 = 面板交互：护住选中（不清空、不框选）。输入框
            // 与层级树的点击让给 UiVm 的夺焦/行点击路径；标尺与 dock
            // 照 Godot 口径不属于可编辑区 —— 点上去既不清选中也不框选。
            let over_ui = {
                let tree = rt.tree_mut();
                [
                    name_input, hud_tree, hud_dock, ruler_h, ruler_v, ruler_corner,
                    tool_sel, tool_snap, tool_grid,
                    fs_bg, fs_sep, fs_tree,
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
                    let w = tree.world(n).unwrap_or_default();
                    if mx >= w.tx && mx < w.tx + 16.0 && my >= w.ty && my < w.ty + 16.0 {
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
        } else if mouse_left_held && !prev_click {
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

        // 框选拖拽中：mouse up → 选中矩形内全部 Sprite。
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
                            let w = tree.world(n).unwrap_or_default();
                            let (cx, cy) = (w.tx + 8.0, w.ty + 8.0); // 中心
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
        if dx != 0.0 || dy != 0.0 {
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
        // Delete：删除子树（Hierarchy 事务）。
        if del_now && !prev_del {
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
        // 焦点门：改名输入框持焦时 Enter/字母属于输入框（UiVm 提交
        // 改名）—— 键盘挂载流整体让路。focus 是上一帧 UiVm 更新的
        // 结果（一帧滞后，与既有 UI 命中口径一致）。
        let renaming = rt.ui_vm_mut().focus() == Some(name_input);
        if !renaming {
            // F5：手动刷新资产扫描（带日志；周期刷新不打扰 Output）。
            if f5_now {
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
                    log_line(&editor_log, "cand: none (F5 to scan)".into());
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
                        log_line(&editor_log, "mount: no candidate (F5 to scan)".into());
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
        }
        // Ctrl+Z / Ctrl+Y：undo / redo（直接消费事务历史）。落账后
        // 文档真相可能已变（改名被回滚/重放）—— 输入框投影与草稿
        // 跟随（S12-4 ④）：reset_text 置草稿 = 当前名、不触发
        // on_commit（回滚值不会再记账），持焦中的旧草稿即刻作废。
        let mut doc_changed = false;
        if z_now && !prev_z && log.undo(rt.tree_mut()).unwrap_or(false) {
            doc_changed = true;
            log_line(&editor_log, "undo".into());
        }
        if y_now && !prev_y && log.redo(rt.tree_mut()).unwrap_or(false) {
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
            // 底缘上移到 dock 上缘；可编辑区再让出顶/左各 16px 标尺
            //（标尺不属于可编辑区，Godot 口径 —— 网格/命中/框选只在
            // 可编辑区内）。
            let gx0 = MARGIN + LEFT_PANEL_W;
            let gx1 = viewport.0 - INSPECTOR_W - 2.0 * MARGIN;
            let gy1 = viewport.1 - STATUS_BAND - DOCK_H;
            let vx0 = gx0 + RULER_W;
            // S12-7：标尺整体下移让出视口工具栏（工具带 24px 在标尺之
            // 上 —— Godot 2D 视口顶部工具条的堆叠顺序）。
            let ruler_y = TOP_BAND + TOOLBAR_H;
            let vy0 = ruler_y + RULER_W;
            let vx1 = gx1;
            let vy1 = gy1;

            // 视口工具栏布线（S12-7）：panel 槽铺底 + 底缘 1px border
            // 分隔线 + 三个开关按钮。gx0 恒定（面板恒宽），沿投影纪律
            // 每帧重写；按钮文本后缀 * = ON —— 开关态是编辑器会话态，
            // 每帧重写进文本投影（投影无状态口径）。
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
            ];
            for (i, (b, on, name)) in tool_btns.iter().enumerate() {
                let _ = tree.set_prop(*b, PROP_CONTROL_OFFSET,
                    Value::Vec2(nes_scene::Vec2::new(
                        gx0 + 4.0 + i as f32 * TOOLBAR_BTN_STEP,
                        TOP_BAND + 2.0,
                    )));
                let _ = tree.set_prop(*b, "text",
                    Value::Str(if *on { format!("{name}*") } else { (*name).to_string() }));
            }
            // 左栏两段布线（S12-8，Godot 左栏 Scene + res:// 两段）：
            // 可用高 = 顶带到 dock 上缘；上段 Scene（层级树）+ 4px
            // border 分隔条 + 下段 FileSystem（"res:/" 标题 + 资产树）。
            // 分割比例由 F9 档位推导（fs_focus 会话态 —— 比例本身不进
            // 树，只落在每帧重写的 offset/size 上，焦点段占大头）。
            // 最小窗口下段高钳 0（列表/条带照画零矩形，提取层口径）。
            let avail_h = (viewport.1 - TOP_BAND - STATUS_BAND - DOCK_H).max(0.0);
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
            // 右检查器面板底：x = cw-198（宽 190 + 右缘 8），y = 8..dock 上缘。
            let _ = tree.set_prop(hud_ins_bg, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(viewport.0 - INSPECTOR_W - 2.0 * MARGIN, MARGIN)));
            let _ = tree.set_prop(hud_ins_bg, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(INSPECTOR_W, viewport.1 - MARGIN - STATUS_BAND - DOCK_H)));
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
            let dock_list_h = DOCK_H - 18.0 - 2.0;
            let _ = tree.set_prop(hud_dock, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(MARGIN + 2.0, dock_y + 18.0)));
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
            let skips = [grid, ruler, dock, toolbar, fsdock];
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
                    // Transform 组标题（面板第 2 行）：前缀 "-" 展开 /
                    // "+" 折叠，text_dim 色（装配期定槽，此处只翻文本）。
                    title_rows.push((ins_x, 12.0 + 16.0, 0));
                    tree.set_local(ins_tf_title, Transform2D::from_pos(ins_x, 12.0 + 16.0));
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
                    // 槽位紧跟组行（行高动态 —— 不再是装配期写死的常量，
                    // S12-6 ①根修口径延续：每帧重写，窗口一变当帧跟上）。
                    let input_y =
                        12.0 + 2.0 * 16.0 + if tf_open { 4.0 * 16.0 } else { 0.0 } + 4.0;
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
                        tree.set_local(ins_script, Transform2D::from_pos(ins_x, sc_y + 16.0));
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
            // ON 恒吸附、Ctrl 反转 —— tools 段 S/N/G 即三开关现态）。
            let st = format!(
                "st> undo:{} redo:{} sel:{} tools:{}{}{} | Click=sel Drag=box Del=del F5=scan F6=cand Enter=mount U=unmount E=enable F7=groups F9=split Ctrl+Z/Y=undo",
                if log.can_undo() { "Y" } else { "-" },
                if log.can_redo() { "Y" } else { "-" },
                sel.len(),
                if tool_sel_on { "S" } else { "-" },
                if tool_snap_on { "N" } else { "-" },
                if tool_grid_on { "G" } else { "-" },
            );
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

        let _ = rt.emit_input_signals(&snap);
        let frame = FrameInfo::new(index, delta, elapsed, Vec2::new(viewport.0, viewport.1));
        match rt.frame_windowed_with(&frame, &mut vm) {
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
        // 重命名提交（帧后落账 —— UiVm 钩子回调在帧内只传值）：
        // 一次提交 = 一条 Modified 事务（Inspector::modify_name）。
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

        // 层级树行点击落账（帧后 —— UiVm 钩子回调在帧内只报行下标）：
        // 一次点击 = 一次 Selection::select（与视口点选同款单选替换语义；
        // 选择是会话态，不进事务不落盘）。下一帧的树投影与 selected 行
        // 高亮随之跟上。
        for uid in row_clicks.borrow_mut().drain(..) {
            sel.select(uid);
        }

        // 工具栏开关落账（帧后 —— UiVm 激活回调帧内只报名字）。开关
        // 态是编辑器会话态：只翻本地布尔 + Output 一行，不进树不落账。
        for name in tool_clicks.borrow_mut().drain(..) {
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

        // 文件系统 dock 行点击落账（帧后 —— UiVm 钩子帧内只报行下
        // 标）：单击 = 选中该行（会话态，下一帧 selected 行高亮跟上）；
        // .nes 顺手指为 F6 候选起点（两处入口同一挂载流，池与 res://
        // 树同源必命中）。同行 30 帧内两次点击沿 = 双击（FS_DBLCLICK_
        // FRAMES 裁决；UiVm 行回调只有单击沿，双击是宿主会话态的边沿
        // 合成）：.ron 场景 = Output 提示（场景打开归 play-in-editor
        // 里程碑，P0 不实现）；.nes = 直接挂载（与 Enter 同一
        // mount_script 事务，Output 报结果）；目录/其余后缀 = 提示，
        // 不落账。
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
        println!("[demo] 挂载/卸载/enabled/折叠/刷新 冒烟断言通过");
    }
    println!("[完成] Editor Shell 退出");
    let _ = (grid, cam, hud_tree, hud_ins_bg, hud_ins, hud_st, sel_box, name_input, hud_scene, tool_bg, tool_sep, ins_tf_title, ins_sc_title, fsdock, fs_bg, fs_title, fs_sep, fs_tree);
}
