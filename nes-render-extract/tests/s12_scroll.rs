//! T-SCL 契约回归：S12-3 任务 4 —— 提取与渲染摊平（NullRenderServer 断言；
//! GPU 像素路径由 nes-render-wgpu 的 criterion_clip_contract 追加用例负责）。
//!
//! 坐标口径提醒：控件是**单级视口锚定**（S3 契约，`anchor * viewport +
//! offset`），树层级不参与矩形解析 —— 嵌套用例里的"交集"都是视口空间
//! 矩形的交，与 nes_scene::ui 的命中测算同一口径。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-SCL-01 | ListView 摊平三件套 SetList + SetRect + SetClip（同句柄，输出序冻结 SetList → SetRect → SetClip）；载荷 = rows/row_h/axis/selected/scroll（UiStates 当帧值）+ 双主题槽 |
//! | T-SCL-02 | 滚动烘焙：ScrollView 后代的 ControlState 四边 offset 烘焙量 = −scroll（scrolls 预置值经 attach_ui 注入）；ScrollView 自身不烘自己的滚动 |
//! | T-SCL-03 | scroll_bar 算式：extent = 自身高 + scroll_max，frac = 自身高/extent，pos = scroll/(extent−身高)；色 = border 槽 |
//! | T-SCL-04 | 嵌套 ScrollView：后代 clip = 自身矩形 ∩ 两层滚动矩形交集（结构性判定，与当前偏移值无关）；交空 = 零矩形仍推（全裁）；内层 ScrollView 自身 clip = 内 ∩ 外 |
//! | T-SCL-05 | 基线不变：无滚动祖先的 Button 恒推 SetClip（= 自身矩形，D6 —— 像素路径与任务 1 基线一致：scissor 覆盖全控件矩形时不可见）；裸 Control / 纯 Label 不推 SetClip |
//! | T-SCL-06 | Tabs 摊平：axis=Horizontal、tabs/tab_w/active 属性名、忽略自身 scroll、clip = 自身矩形 |
//! | T-SCL-08 | 装得下不发滚动条（S12-4）：extent <= 自身高（含恰好装下的边界）→ scroll_bar 为 None；溢出哪怕 1px → Some（滑块才可画） |

use std::cell::RefCell;
use std::rc::Rc;

use nes_render_api::{FrameInfo, ListAxis, NullRenderServer, Rect, RenderCommand, Vec2};
use nes_render_extract::{RenderExtractor, RenderKeySource};
use nes_scene::ui::{ThemeColors, UiStates};
use nes_scene::{NodeKind, SceneTree, Value, Vec2 as SVec2};

/// 空资源键源（滚动族控件不需要资源）。
struct NoAssets;
impl RenderKeySource for NoAssets {
    fn render_key(&self, _id: nes_scene::ResId) -> Option<nes_render_api::RenderAssetKey> {
        None
    }
}

fn frame(vw: f32, vh: f32) -> FrameInfo {
    FrameInfo::new(0, 1.0 / 60.0, 0.0, Vec2::new(vw, vh))
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-4
}

/// 提取一帧（可选挂 UiStates 共享面），返回（服务端，提取器，命令流）。
fn extract(
    t: &mut SceneTree,
    ui: Option<Rc<RefCell<UiStates>>>,
) -> (NullRenderServer, RenderExtractor, Vec<RenderCommand>) {
    let mut ex = RenderExtractor::new();
    if let Some(states) = ui {
        ex.attach_ui(states);
    }
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();
    let _ = ex.extract_into(t, &NoAssets, &mut srv, &frame(512.0, 288.0), &mut out);
    (srv, ex, out)
}

/// 某句柄命中谓词的首条命令位置。
fn pos_of(
    stream: &[RenderCommand],
    h: nes_render_api::ItemHandle,
    pred: &dyn Fn(&RenderCommand) -> bool,
) -> Option<usize> {
    stream.iter().position(|c| c.handle() == Some(h) && pred(c))
}

/// T-SCL-01：ListView 摊平三件套 + 载荷字段。
#[test]
fn t_scl_01_list_view_flattens_list_rect_clip() {
    let mut t = SceneTree::new("root");
    let lv = t.add_node(t.root(), "list", NodeKind::ListView);
    t.set_prop(lv, "offset", Value::Vec2(SVec2::new(20.0, 30.0))).unwrap();
    t.set_prop(lv, "size", Value::Vec2(SVec2::new(120.0, 98.0))).unwrap();
    t.set_prop(lv, "rows", Value::Str("alpha\nbeta\ngamma".into())).unwrap();
    t.set_prop(lv, "row_h", Value::I64(18)).unwrap();
    t.set_prop(lv, "selected", Value::I64(1)).unwrap();
    t.apply_pending();

    // 预置滚动瞬态（滚轮当帧值）—— 摊平载荷必须带出。
    let mut ui = UiStates::new();
    ui.scrolls.insert(lv, 27.0);
    let (srv, ex, out) = extract(&mut t, Some(Rc::new(RefCell::new(ui))));
    let h = ex.handle_of(lv).expect("ListView 恒准入（空列表也有框可点）");

    // SetList 载荷：rows 文本 / 行高 / 轴 / 选中 / 滚动当帧值 / 主题槽。
    let rows = srv.list_of(h).expect("同句柄收到 SetList").clone();
    assert_eq!(&*rows.text, "alpha\nbeta\ngamma", "'\\n' 分隔行原样到达");
    assert_eq!(rows.axis, ListAxis::Vertical, "ListView = 垂直轴");
    assert_eq!(rows.row_h, 18.0, "row_h 属性（I64）到达");
    assert_eq!(rows.selected, Some(1), "selected >= 0 才 Some");
    assert_eq!(rows.scroll, 27.0, "scroll = UiStates.scrolls 当帧值");
    let dark = ThemeColors::DEFAULT_DARK;
    assert_eq!(rows.text_color, dark.slot("text").unwrap(), "text_slot 缺省 = text 槽");
    assert_eq!(rows.sel_fill, dark.slot("selected").unwrap(), "sel_fill_slot 缺省 = selected 槽");

    // 输出序冻结：SetList → SetRect → SetClip（同句柄；任务 1 序里
    // SetList 插在 SetText 之后 —— ListView 无 SetText，直接领头）。
    let list_idx = pos_of(&out, h, &|c| matches!(c, RenderCommand::SetList { .. }))
        .expect("命令流里有 SetList");
    let rect_idx = pos_of(&out, h, &|c| matches!(c, RenderCommand::SetRect { .. }))
        .expect("命令流里有 SetRect");
    let clip_idx = pos_of(&out, h, &|c| matches!(c, RenderCommand::SetClip { rect: Some(_), .. }))
        .expect("命令流里有 SetClip");
    assert!(list_idx < rect_idx && rect_idx < clip_idx, "输出序 SetList → SetRect → SetClip");

    // SetClip = 自身 resolve 后矩形（无滚动祖先 + 恒裁剪类型，D6）。
    assert_eq!(
        srv.clip_of(h).copied().expect("恒裁剪类型必有 SetClip"),
        Rect::new(20.0, 30.0, 120.0, 98.0),
        "clip = 自身矩形（含边框）"
    );
}

/// T-SCL-02：滚动烘焙 —— 后代 offset 折进 −scroll；ScrollView 自身不动。
#[test]
fn t_scl_02_descendant_offsets_bake_minus_scroll() {
    let mut t = SceneTree::new("root");
    let sv = t.add_node(t.root(), "scroll", NodeKind::ScrollView);
    t.set_prop(sv, "size", Value::Vec2(SVec2::new(200.0, 100.0))).unwrap();
    let btn = t.add_node(sv, "btn", NodeKind::Button);
    t.set_prop(btn, "offset", Value::Vec2(SVec2::new(8.0, 70.0))).unwrap();
    t.set_prop(btn, "size", Value::Vec2(SVec2::new(100.0, 28.0))).unwrap();
    t.set_prop(btn, "text", Value::Str("HI".into())).unwrap();
    t.apply_pending();

    let mut ui = UiStates::new();
    ui.scrolls.insert(sv, 40.0);
    let (srv, ex, _out) = extract(&mut t, Some(Rc::new(RefCell::new(ui))));

    // 后代按钮：offset_top/offset_bottom 各减偏移和（左右不动）。
    let bh = ex.handle_of(btn).expect("按钮渲染物");
    let rect = *srv.rect_of(bh).expect("SetRect");
    assert_eq!(rect.offset_top, 30.0, "offset_top 烘焙 70 - 40");
    assert_eq!(rect.offset_bottom, 58.0, "offset_bottom 烘焙 98 - 40");
    assert_eq!(rect.offset_left, 8.0, "水平不受垂直滚动影响");
    // 烘焙后的 clip = 烘焙矩形 ∩ 容器矩形：烘焙矩形 (8,30,100,28) ⊂ 容器
    // (0,0,200,100) → clip 恰为烘焙矩形（P1 冻结：契约矩形不动，渲染读到
    // 的值随 offset 平移）。
    assert_eq!(
        srv.clip_of(bh).copied().expect("按钮恒有 clip"),
        Rect::new(8.0, 30.0, 100.0, 28.0),
    );

    // ScrollView 自身：scroll_context_of 只沿**祖先**链 —— 自己的滚动
    // 不烘进自己的矩形（它是视口，不是内容）。
    let sh = ex.handle_of(sv).expect("ScrollView 渲染物");
    let own = *srv.rect_of(sh).expect("SetRect");
    assert_eq!(own.offset_top, 0.0, "ScrollView 自身矩形不随自身滚动平移");
    assert_eq!(own.offset_bottom, 100.0);
}

/// T-SCL-03：scroll_bar frac/pos 算式（构造已知 extent）。
#[test]
fn t_scl_03_scroll_bar_frac_pos_formula() {
    let mut t = SceneTree::new("root");
    let sv = t.add_node(t.root(), "scroll", NodeKind::ScrollView);
    t.set_prop(sv, "size", Value::Vec2(SVec2::new(200.0, 100.0))).unwrap();
    let btn = t.add_node(sv, "btn", NodeKind::Button);
    t.set_prop(btn, "offset", Value::Vec2(SVec2::new(8.0, 150.0))).unwrap();
    t.set_prop(btn, "size", Value::Vec2(SVec2::new(100.0, 28.0))).unwrap();
    t.apply_pending();

    // extent = 自身高(100) + scroll_max(后代底缘 178 − 自身底 100 = 78) = 178。
    // frac = 100/178；scroll 预置 39 → pos = 39/78 = 0.5。
    let mut ui = UiStates::new();
    ui.scrolls.insert(sv, 39.0);
    let (srv, ex, _out) = extract(&mut t, Some(Rc::new(RefCell::new(ui))));
    let sh = ex.handle_of(sv).expect("ScrollView 渲染物");
    let rect = srv.rect_of(sh).expect("SetRect");
    let bar = rect.scroll_bar.expect("ScrollView 自身必发滚动条视觉状态");
    assert!(
        approx(bar.frac, 100.0 / 178.0),
        "frac = 自身高/extent：{} vs {}",
        bar.frac,
        100.0 / 178.0
    );
    assert!(approx(bar.pos, 0.5), "pos = scroll/(extent-身高)：{} vs 0.5", bar.pos);
    assert_eq!(
        bar.color,
        ThemeColors::DEFAULT_DARK.slot("border").unwrap(),
        "滑块色 = border 槽解析色"
    );

    // 装得下（无后代溢出，scroll_max == 0）→ 不发滚动条（S12-4：满长
    // 滑块没有行程可言，画出来纯属视觉噪声 —— 与滚轮路由的 scroll_max=0
    // 预期不滚同一条设计判据）。
    let mut t2 = SceneTree::new("root");
    let sv2 = t2.add_node(t2.root(), "scroll", NodeKind::ScrollView);
    t2.set_prop(sv2, "size", Value::Vec2(SVec2::new(200.0, 100.0))).unwrap();
    t2.apply_pending();
    let (srv2, ex2, _out2) = extract(&mut t2, None);
    let sh2 = ex2.handle_of(sv2).unwrap();
    assert!(
        srv2.rect_of(sh2).unwrap().scroll_bar.is_none(),
        "装得下 = scroll_bar 为 None（不画滑块）"
    );
}

/// T-SCL-08（S12-4）：装得下不发滚动条的边界 —— 溢出哪怕 1px 才 Some，
/// 恰好装下（后代底缘 == 自身底缘）为 None；与 T-SCL-03 的算式分支同一
/// 判据（extent > 自身高）。
#[test]
fn t_scl_08_scroll_bar_only_when_overflowing() {
    let build = |child_h: f32| -> (SceneTree, nes_scene::NodeId) {
        let mut t = SceneTree::new("root");
        let sv = t.add_node(t.root(), "scroll", NodeKind::ScrollView);
        t.set_prop(sv, "size", Value::Vec2(SVec2::new(200.0, 100.0))).unwrap();
        let btn = t.add_node(sv, "btn", NodeKind::Button);
        t.set_prop(btn, "offset", Value::Vec2(SVec2::new(0.0, 0.0))).unwrap();
        t.set_prop(btn, "size", Value::Vec2(SVec2::new(100.0, child_h))).unwrap();
        t.apply_pending();
        (t, sv)
    };

    // 恰好装下：后代底缘 100 == 自身底缘 100 → scroll_max = 0 → None。
    let (mut fits, sv) = build(100.0);
    let (srv, ex, _out) = extract(&mut fits, None);
    let h = ex.handle_of(sv).expect("ScrollView 渲染物");
    assert!(
        srv.rect_of(h).unwrap().scroll_bar.is_none(),
        "恰好装下（边界）= scroll_bar 为 None"
    );

    // 溢出 1px：后代底缘 101 → scroll_max = 1 → Some（滑块才有意义）。
    let (mut over, sv2) = build(101.0);
    let (srv2, ex2, _out2) = extract(&mut over, None);
    let h2 = ex2.handle_of(sv2).expect("ScrollView 渲染物");
    let bar = srv2
        .rect_of(h2)
        .unwrap()
        .scroll_bar
        .expect("溢出 1px 也发滚动条状态");
    assert!(approx(bar.frac, 100.0 / 101.0), "frac = 100/101：{}", bar.frac);
    assert!(approx(bar.pos, 0.0), "未滚动 = pos 0");
}

/// T-SCL-04：嵌套 ScrollView —— 后代 clip = 自身 ∩ 内层 ∩ 外层；交空 = 零矩形。
#[test]
fn t_scl_04_nested_scroll_view_intersection() {
    let mut t = SceneTree::new("root");
    let outer = t.add_node(t.root(), "outer", NodeKind::ScrollView);
    t.set_prop(outer, "size", Value::Vec2(SVec2::new(200.0, 200.0))).unwrap();
    let inner = t.add_node(outer, "inner", NodeKind::ScrollView);
    t.set_prop(inner, "offset", Value::Vec2(SVec2::new(150.0, 150.0))).unwrap();
    t.set_prop(inner, "size", Value::Vec2(SVec2::new(100.0, 100.0))).unwrap();
    // 按钮（视口矩形 100..220）：左/上被内层（150 起）收口，右/下被外层
    //（200 止）收口 —— 两层滚动矩形都参与定界。
    let btn = t.add_node(inner, "btn", NodeKind::Button);
    t.set_prop(btn, "offset", Value::Vec2(SVec2::new(100.0, 100.0))).unwrap();
    t.set_prop(btn, "size", Value::Vec2(SVec2::new(120.0, 120.0))).unwrap();
    // 第二个按钮完全落在祖先交集（150..250 ∩ 0..200）之外 → 空交。
    let out_btn = t.add_node(inner, "out_btn", NodeKind::Button);
    t.set_prop(out_btn, "offset", Value::Vec2(SVec2::new(0.0, 0.0))).unwrap();
    t.set_prop(out_btn, "size", Value::Vec2(SVec2::new(40.0, 40.0))).unwrap();
    t.apply_pending();

    // 无滚动（结构性判定与当前偏移值无关 —— 交集照裁）。
    let (srv, ex, _out) = extract(&mut t, None);

    // 按钮：矩形 (100,100,120,120) ∩ 内层 (150,150,100,100) ∩ 外层
    // (0,0,200,200) → (150,150,50,50)。
    let bh = ex.handle_of(btn).expect("按钮渲染物");
    assert_eq!(
        srv.clip_of(bh).copied().expect("按钮 clip"),
        Rect::new(150.0, 150.0, 50.0, 50.0),
        "clip = 自身 ∩ 两层滚动矩形交集（左/上由内层收口，右/下由外层收口）"
    );

    // 交空：按钮 (0,0,40,40) 与祖先交集 (150,150,50,50) 无重叠 → 零矩形
    // 仍推（全裁 —— wgpu 侧零尺寸裁剪条目整条省略实例）。
    let oh = ex.handle_of(out_btn).expect("按钮渲染物");
    assert_eq!(
        srv.clip_of(oh).copied().expect("空交也推 SetClip"),
        Rect::new(0.0, 0.0, 0.0, 0.0),
        "交空 = 零矩形仍推（全裁）"
    );

    // 内层 ScrollView 自身：恒裁剪类型推自身矩形，再与外层交集。
    let ih = ex.handle_of(inner).expect("内层渲染物");
    assert_eq!(
        srv.clip_of(ih).copied().expect("内层 clip"),
        Rect::new(150.0, 150.0, 50.0, 50.0),
        "内层 clip = 内层矩形 ∩ 外层矩形"
    );
}

/// T-SCL-05：基线 —— 无滚动祖先场景的裁剪契约（D6 演进后的形状）。
#[test]
fn t_scl_05_baseline_clip_shape_unchanged_pixel_path() {
    let mut t = SceneTree::new("root");
    let btn = t.add_node(t.root(), "ok", NodeKind::Button);
    t.set_prop(btn, "offset", Value::Vec2(SVec2::new(32.0, 120.0))).unwrap();
    t.set_prop(btn, "size", Value::Vec2(SVec2::new(140.0, 28.0))).unwrap();
    t.set_prop(btn, "text", Value::Str("OK".into())).unwrap();
    let panel = t.add_node(t.root(), "panel", NodeKind::Control);
    t.set_prop(panel, "size", Value::Vec2(SVec2::new(480.0, 256.0))).unwrap();
    let lbl = t.add_node(t.root(), "lbl", NodeKind::Label);
    t.set_prop(lbl, "text", Value::Str("hi".into())).unwrap();
    t.apply_pending();

    let (srv, ex, out) = extract(&mut t, None);

    // Button：D6 后恒推 SetClip（= 自身矩形）。像素路径与任务 1 基线
    // 逐位一致 —— 文本 + 4 内衬本来在界内，全矩形 scissor 不可见。
    let bh = ex.handle_of(btn).unwrap();
    assert_eq!(
        srv.clip_of(bh).copied().expect("Button 恒有 SetClip（D6 根修）"),
        Rect::new(32.0, 120.0, 140.0, 28.0),
        "clip = 按钮自身矩形"
    );
    let clip_count = out
        .iter()
        .filter(|c| c.handle() == Some(bh) && matches!(c, RenderCommand::SetClip { rect: Some(_), .. }))
        .count();
    assert_eq!(clip_count, 1, "每帧每条目至多一条 SetClip");

    // 裸 Control（无滚动祖先、非摊平四类）：不推 SetClip —— 既有输出逐位不变。
    let ph = ex.handle_of(panel).unwrap();
    assert!(srv.clip_of(ph).is_none(), "裸 Control 无 SetClip（基线不变）");

    // 纯 Label：自由文本不裁 —— 既有观感不变。
    let lh = ex.handle_of(lbl).unwrap();
    assert!(srv.clip_of(lh).is_none(), "纯 Label 无 SetClip");
}

/// T-SCL-06：Tabs 摊平 —— 水平轴、tabs/tab_w/active 属性名、忽略自身 scroll。
#[test]
fn t_scl_06_tabs_flatten_horizontal() {
    let mut t = SceneTree::new("root");
    let tabs = t.add_node(t.root(), "tabs", NodeKind::Tabs);
    t.set_prop(tabs, "offset", Value::Vec2(SVec2::new(10.0, 20.0))).unwrap();
    t.set_prop(tabs, "size", Value::Vec2(SVec2::new(300.0, 24.0))).unwrap();
    t.set_prop(tabs, "tabs", Value::Str("File\nEdit\nView".into())).unwrap();
    t.set_prop(tabs, "tab_w", Value::I64(64)).unwrap();
    t.set_prop(tabs, "active", Value::I64(1)).unwrap();
    t.apply_pending();

    // Tabs 自身的 scroll 瞬态对 Tabs 无效（水平轴忽略 scroll —— 冻结口径）。
    let mut ui = UiStates::new();
    ui.scrolls.insert(tabs, 33.0);
    let (srv, ex, _out) = extract(&mut t, Some(Rc::new(RefCell::new(ui))));
    let h = ex.handle_of(tabs).expect("Tabs 恒准入");
    let rows = srv.list_of(h).expect("同句柄收到 SetList").clone();
    assert_eq!(rows.axis, ListAxis::Horizontal, "Tabs = 水平轴");
    assert_eq!(&*rows.text, "File\nEdit\nView", "tabs 属性到达");
    assert_eq!(rows.tab_w, 64.0, "tab_w 属性到达");
    assert_eq!(rows.row_h, 18.0, "行带高 = 缺省 18（Tabs 无 row_h 属性）");
    assert_eq!(rows.selected, Some(1), "active 属性 → selected");
    assert_eq!(rows.scroll, 33.0, "scroll 照实带出（当帧值）；水平轴由渲染侧忽略");

    // clip = 自身矩形（无滚动祖先 + 恒裁剪类型）。
    assert_eq!(
        srv.clip_of(h).copied().expect("Tabs clip"),
        Rect::new(10.0, 20.0, 300.0, 24.0),
    );
}

/// T-SCL-07（S12-3 评审 [medium] 修复）：Some→None 迁移帧补推清除 ——
/// 控件先在 ScrollView 内（得交集裁剪），随后移出（repaint 到根），
/// 下一帧必须显式 set_clip(None)，跨帧簿记的陈旧裁剪不得残留。
#[test]
fn t_scl_07_clip_cleared_on_structural_exit() {
    let mut t = SceneTree::new("root");
    let sv = t.add_node(t.root(), "sv", NodeKind::ScrollView);
    t.set_prop(sv, "offset", Value::Vec2(SVec2::new(0.0, 0.0))).unwrap();
    t.set_prop(sv, "size", Value::Vec2(SVec2::new(200.0, 100.0))).unwrap();
    // panel 故意超出容器（10+300 > 200），交集才真正收窄。
    let panel = t.add_node(sv, "panel", NodeKind::Control);
    t.set_prop(panel, "offset", Value::Vec2(SVec2::new(10.0, 10.0))).unwrap();
    t.set_prop(panel, "size", Value::Vec2(SVec2::new(300.0, 200.0))).unwrap();
    t.apply_pending();

    // 帧 1：panel 在 ScrollView 内 → clip = 自身 ∩ 容器（Some，收窄）。
    let (srv, _ex, _out) = extract(&mut t, None);
    let h = _ex.handle_of(panel).expect("panel 渲染物");
    let clip1 = srv.clip_of(h).copied().expect("帧 1 必有交集裁剪");
    assert!(
        approx(clip1.w, 190.0) && approx(clip1.h, 90.0),
        "交集应被容器收窄到 190x90：{clip1:?}"
    );

    // 结构变迁：panel 移出 ScrollView（挂到根、位置不变）。
    t.reparent(panel, t.root(), None);
    t.apply_pending();

    // 帧 2：裸 Control 不再命中任何裁剪来源 → 必须补推 None 清除。
    let (srv2, _ex2, _out2) = extract(&mut t, None);
    let h2 = _ex2.handle_of(panel).expect("panel 渲染物（复用句柄）");
    assert_eq!(
        srv2.clip_of(h2).copied(),
        None,
        "Some→None 迁移帧必须清除陈旧裁剪（评审 [medium]）"
    );
}
