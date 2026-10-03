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
//! 运行：`cargo run --example editor_shell`

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::{PROP_CONTROL_ANCHOR, PROP_CONTROL_OFFSET, PROP_CONTROL_SIZE, PROP_LABEL_TEXT, PROP_TEXTURE};
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
/// 编辑器日志环形保留行数（新行在下，满 8 丢最旧 —— Godot Output
/// 的最小语义；可见窗只放最新能放下的几行，最新行永远可见）。
const EDITOR_LOG_KEEP: usize = 8;
/// dock 行显示截宽（字符数）：40 字 × 16px advance = 640px，最小窗
/// 768 下 dock 内衬（≈748px）也放得下，行尾不裁字。
const DOCK_LINE_CHARS: usize = 40;

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

/// Inspector 改名输入框的固定槽位 y（面板顶标题 + 属性行 5 行 16px
/// 文本之下：12 + 5×16 = 92，留 4px 缝）—— 每帧投影，不再写死。
const INSPECTOR_INPUT_Y: f32 = 96.0;
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
    let (grid, grid_bars, ruler, ruler_h, ruler_v, ruler_corner, ruler_ticks, ruler_labels, dock, dock_bg, dock_title, hud_dock, cam, obj1, obj2, obj3, hud_tree, hud_ins_bg, hud_ins, hud_st, sel_box, name_input, hud_scene) = {
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
        tree.set_prop(name_input, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(576.0, INSPECTOR_INPUT_Y))).unwrap();
        tree.set_prop(name_input, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(INSPECTOR_W - 2.0 * INSPECTOR_INSET, 20.0))).unwrap();
        tree.set_prop(name_input, "text", Value::Str(String::new())).unwrap();
        tree.set_prop(name_input, "visible", Value::Bool(false)).unwrap();
        tree.apply_pending();
        (grid, grid_bars, ruler, ruler_h, ruler_v, ruler_corner, ruler_ticks, ruler_labels, dock, dock_bg, dock_title, hud_dock, cam, obj1, obj2, obj3, hud_tree, hud_ins_bg, hud_ins, hud_st, sel_box, name_input, hud_scene)
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
    {
        let clicks = row_clicks.clone();
        let map = row_map_shared.clone();
        rt.ui_vm_mut().on_row_activate(move |node, row| {
            if node != hud_tree {
                return; // 只认层级树列表（当前全场景仅此一个 ListView）。
            }
            if let Some(uid) = map.borrow().get(row as usize) {
                clicks.borrow_mut().push(uid.clone());
            }
        });
    }

    let total: u64 = std::env::var("NES_GAME_FRAMES")
        .or_else(|_| std::env::var("NES_EDIT_FRAMES"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
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

        let snap = rt.collect_input();

        // ---- 编辑器命令（消费输入快照 —— 与游戏脚本同一读面）----
        let (z_now, y_now, del_now, tab_now) = (
            snap.is_down("LCtrl") && snap.is_down("Z"),
            snap.is_down("LCtrl") && snap.is_down("Y"),
            snap.is_down("Delete"),
            snap.pressed.contains(&nes_render_api::input::Key::Tab),
        );
        let _ = (z_now, y_now);
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
        if mouse_left_held && !prev_click {
            // hit 在脚本中做；宿主侧直接查树（与 hit 同逻辑的 Rust 版）。
            // 压在编辑器 UI（改名输入框 / 层级树 / Output dock / 标尺
            // 条带）上 = 面板交互：护住选中（不清空、不框选）。输入框
            // 与层级树的点击让给 UiVm 的夺焦/行点击路径；标尺与 dock
            // 照 Godot 口径不属于可编辑区 —— 点上去既不清选中也不框选。
            let over_ui = {
                let tree = rt.tree_mut();
                [name_input, hud_tree, hud_dock, ruler_h, ruler_v, ruler_corner]
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
        }
        // Gizmo 拖拽：鼠标移动 → 选中对象跟随（preview 直写，不入账）；
        // 松开 → Inspector.modify_local 一次事务。按住 Ctrl 吸附 8px 栅格
        //（S12-5：Godot 2D 的 Ctrl 拖动直感，状态栏 Ctrl=snap）—— 目标
        // 位置取整到 GRID_SNAP 的整数倍，松开提交的也是已取整的终值。
        if let Some((ref uid, ox, oy)) = gizmo {
            if mouse_left_held {
                // preview：直写树位置（会话态，微批次之外）。
                let (tx, ty) = (mx - ox, my - oy);
                let (tx, ty) = if snap.is_down("LCtrl") {
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
            let vy0 = TOP_BAND + RULER_W;
            let vx1 = gx1;
            let vy1 = gy1;
            // 左层级面板：宽恒 180，高度到 dock 上缘。
            let _ = tree.set_prop(hud_tree, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(LEFT_PANEL_W, viewport.1 - TOP_BAND - STATUS_BAND - DOCK_H)));
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
            if vx1 > vx0 && vy1 > vy0 {
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
                Value::Vec2(nes_scene::Vec2::new(gx0, TOP_BAND)));
            let _ = tree.set_prop(ruler_h, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(vx1 - gx0, RULER_W)));
            let _ = tree.set_prop(ruler_v, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(gx0, TOP_BAND)));
            let _ = tree.set_prop(ruler_v, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(RULER_W, vy1 - TOP_BAND)));
            let _ = tree.set_prop(ruler_corner, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(gx0, TOP_BAND)));
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
                    let (ty, th) = if major { (TOP_BAND, RULER_W) } else { (TOP_BAND + half, half) };
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
                    tree.set_local(lab, Transform2D::from_pos(x + 2.0, TOP_BAND));
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
            let skips = [grid, ruler, dock];
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

            // Inspector View：选中节点数据投影（S12-6 Godot inspector
            // 分行模式：一行一属性短标签 —— "name obj3" / "x 312"，
            // 16px 等宽 advance=16、面板内衬宽 178px ≈11 字/行，短行
            // 绝不超面板宽；数值行随选中实时刷新）。标题一行 + 属性行
            // 各一行，"(none)" = 无选中。
            let mut ins_text = String::from("Inspector\n");
            match sel.primary(tree) {
                Some(p) => {
                    // 名字截 5 字符：行宽 "name " + 5 = 10 字 = 160px，
                    // 不超内衬宽（11 字上限）。
                    let name: String = tree.name(p).unwrap_or("?").chars().take(5).collect();
                    let local = tree.local(p).unwrap_or_default();
                    // z 显示属性表现值：选中高亮会把选中精灵的 z 写成
                    // 5（S12-5 机制），显示的是节点当前真实属性。
                    let z = tree
                        .prop(p, "z_index")
                        .and_then(|v| if let Value::I64(i) = v { Some(*i) } else { None })
                        .unwrap_or(0);
                    ins_text.push_str(&format!(
                        "name {}\nx {:.0}\ny {:.0}\nz {}",
                        name, local.pos.x, local.pos.y, z
                    ));
                }
                None => ins_text.push_str("(none)"),
            }
            let _ = tree.set_prop(hud_ins, PROP_LABEL_TEXT, Value::Str(ins_text));
            // 标题/信息 Label 每帧投影到右面板顶（x = cw-190, y = 12，
            // 随面板走 —— 修"标题被表面边缘裁剪"）。
            tree.set_local(hud_ins, Transform2D::from_pos(viewport.0 - INSPECTOR_W, 12.0));

            // 改名输入框布局投影（S12-6 ①根修）：位置 = 右面板内、
            // Inspector 标题 + 属性行（5 行 × 16px）下方的固定槽位，
            // 宽度 = 面板宽 - 双边内衬 —— 窗口一变当帧跟上，不再有
            // 装配期写死后漂移的坐标。
            let _ = tree.set_prop(name_input, PROP_CONTROL_OFFSET,
                Value::Vec2(nes_scene::Vec2::new(
                    viewport.0 - INSPECTOR_W - 2.0 * MARGIN + INSPECTOR_INSET,
                    INSPECTOR_INPUT_Y,
                )));
            let _ = tree.set_prop(name_input, PROP_CONTROL_SIZE,
                Value::Vec2(nes_scene::Vec2::new(INSPECTOR_W - 2.0 * INSPECTOR_INSET, 20.0)));

            // 状态栏（S12-5：文案补 Ctrl=snap —— Gizmo 拖拽的吸附提示）。
            let st = format!(
                "st> undo:{} redo:{} sel:{} | Click=sel Shift+Click=multi Drag=box Arrows=move Del=del Ctrl+Z/Y=undo/redo Ctrl=snap",
                if log.can_undo() { "Y" } else { "-" },
                if log.can_redo() { "Y" } else { "-" },
                sel.len(),
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
            let _ = tree.set_prop(name_input, "visible", Value::Bool(primary_uid.is_some()));
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

        // 帧节拍：无固定 sleep —— present 的 FIFO 队列自节流（vsync），
        // 帧差以 Instant 实测进 FrameInfo（见循环头的 delta/elapsed）。
    }
    println!("[完成] Editor Shell 退出");
    let _ = (grid, cam, hud_tree, hud_ins_bg, hud_ins, hud_st, sel_box, name_input, hud_scene);
}
