//! S9-3b **Editor Shell**：建立在已验证状态模型上的编辑器 UI。
//!
//! 架构（评审冻结）：**UI 只消费状态模型，不成为语义来源** ——
//! Hierarchy View 是 SceneTree 的投影（Label 文本），Inspector 是
//! 选择节点数据的投影，Viewport 高亮是 Selection 的投影。一切修改
//! 经 Inspector/Hierarchy 适配器 → TransactionLog。ui 零自有状态
//!（除面板滚动等会话态）。
//!
//! 布局（768x432）：
//! - 左侧 180px：Hierarchy 面板（树投影）
//! - 右侧 160px：Inspector 面板（选中节点属性）
//! - 中间：Viewport（场景 + 选中高亮 z_index=5）
//! - 底部：状态栏（undo/redo 可用性、操作提示）
//!
//! 操作：Tab 循环选择；方向键移动选中；Delete 删除子树；
//! Ctrl+Z undo；Ctrl+Y redo。
//!
//! 运行：`cargo run --example editor_shell`

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

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

/// 视口尺寸（与帧 `FrameInfo::viewport` 同源 —— 控件锚定该视口解析）。
const VIEWPORT: (f32, f32) = (768.0, 432.0);

/// 按压点（**视图空间**）是否落在重命名输入框矩形内 —— 与 UiVm 命中
/// 同一口径（`anchor * viewport + offset` + `size`，不可见即不参与
/// 命中）。宿主用它护住检查器面板：压在输入框上的点击不清选中、
/// 不启动框选，把交互让给 UiVm 的点击夺焦路径（S12-2 验收：
/// 点击改名输入框 -> 夺焦 -> 输入 -> Enter 提交）。
fn press_in_name_input(
    tree: &nes_scene::SceneTree,
    input: nes_scene::NodeId,
    view_pos: (f32, f32),
) -> bool {
    let visible = tree
        .prop(input, "visible")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !visible {
        return false;
    }
    let vec2 = |name: &str| match tree.prop(input, name) {
        Some(Value::Vec2(v)) => *v,
        _ => nes_scene::Vec2::ZERO,
    };
    let anchor = vec2(PROP_CONTROL_ANCHOR);
    let offset = vec2(PROP_CONTROL_OFFSET);
    let size = vec2(PROP_CONTROL_SIZE);
    let (x, y) = (
        anchor.x * VIEWPORT.0 + offset.x,
        anchor.y * VIEWPORT.1 + offset.y,
    );
    view_pos.0 >= x && view_pos.0 < x + size.x && view_pos.1 >= y && view_pos.1 < y + size.y
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
    let (cam, obj1, obj2, obj3, hud_tree, hud_ins, hud_st, sel_box, name_input) = {
        let tree = rt.tree_mut();
        let root = tree.root();
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
        // Hierarchy 面板背景。
        let hud_tree = tree.add_node(root, "hud_tree", NodeKind::Label);
        tree.set_local(hud_tree, Transform2D::from_pos(8.0, 40.0));
        tree.set_prop(hud_tree, PROP_LABEL_TEXT, Value::Str(String::new())).unwrap();
        // Inspector 面板背景。
        let hud_ins = tree.add_node(root, "hud_ins", NodeKind::Label);
        tree.set_local(hud_ins, Transform2D::from_pos(612.0, 40.0));
        tree.set_prop(hud_ins, PROP_LABEL_TEXT, Value::Str(String::new())).unwrap();
        // 状态栏。
        // Selection indicator (Control border following primary selection).
        let sel_box = tree.add_node(root, "sel_box", NodeKind::Control);
        tree.set_prop(sel_box, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::new(0.0, 0.0))).unwrap();
        tree.set_prop(sel_box, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(-100.0, -100.0))).unwrap();
        tree.set_prop(sel_box, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(20.0, 20.0))).unwrap();

        let hud_st = tree.add_node(root, "hud_st", NodeKind::Label);
        tree.set_local(hud_st, Transform2D::from_pos(8.0, 410.0));
        tree.set_prop(hud_st, PROP_LABEL_TEXT, Value::Str(String::new())).unwrap();
        // Inspector 的节点重命名输入框（S12-2 TextInput —— 视口锚定，
        // 与 UiVm 命中/焦点路由同一口径）。选中节点时显示并绑定其名字。
        let name_input = tree.add_node(root, "name_input", NodeKind::TextInput);
        tree.set_prop(name_input, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::new(0.0, 0.0))).unwrap();
        tree.set_prop(name_input, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(616.0, 76.0))).unwrap();
        tree.set_prop(name_input, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(148.0, 20.0))).unwrap();
        tree.set_prop(name_input, "text", Value::Str(String::new())).unwrap();
        tree.set_prop(name_input, "visible", Value::Bool(false)).unwrap();
        tree.apply_pending();
        (cam, obj1, obj2, obj3, hud_tree, hud_ins, hud_st, sel_box, name_input)
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

    let total: u64 = std::env::var("NES_GAME_FRAMES")
        .or_else(|_| std::env::var("NES_EDIT_FRAMES"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
    let mut transient = 0u64;
    const TRANSIENT_LIMIT: u64 = 120;

    for index in 0..total {
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
        let mouse_left_held = snap.is_down("left");
        let mouse_shift = snap.is_down("LShift");
        if mouse_left_held && !prev_click {
            // hit 在脚本中做；宿主侧直接查树（与 hit 同逻辑的 Rust 版）。
            let (mx, my) = (snap.mouse.x, snap.mouse.y);
            // 压在重命名输入框上 = 检查器面板的 UI 交互：护住选中
            //（不清空、不框选），点击让给 UiVm 的夺焦路径。鼠标按
            // 视图/客户区 折算到视图空间 —— 与 UiVm 命中同口径，窗口
            // 缩放后仍准。
            let over_name_input = {
                let (sx, sy) = rt.mouse_view_scale(VIEWPORT);
                press_in_name_input(
                    rt.tree_mut(),
                    name_input,
                    (mx * sx, my * sy),
                )
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
                    sel.toggle(uid);
                } else {
                    sel.select(uid);
                }
                drag_start = None; // 点击命中：不是框选
            } else if !mouse_shift && !over_name_input {
                // 空白处按下：开始框选（拖拽矩形）。压在重命名输入框上
                // 的除外（上方护住 —— 清了选中输入框即隐藏，UiVm 的
                // 点击夺焦就永远够不着它了）。
                drag_start = Some((mx, my));
                sel.clear(); // 框选重置（Shift 保留已有选择）
            }
        }
        // Gizmo 拖拽：鼠标移动 → 选中对象跟随（preview 直写，不入账）；
        // 松开 → Inspector.modify_local 一次事务。
        if let Some((ref uid, ox, oy)) = gizmo {
            if mouse_left_held {
                // preview：直写树位置（会话态，微批次之外）。
                let tree = rt.tree_mut();
                if let Some(id) = tree.find_by_uid(uid) {
                    let cur = tree.local(id).unwrap_or_default();
                    tree.set_local(id, Transform2D::from_pos(snap.mouse.x - ox, snap.mouse.y - oy));
                    let _ = cur;
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
                }
                gizmo = None;
            }
        }

        // 框选拖拽中：mouse up → 选中矩形内全部 Sprite。
        if let Some((sx, sy)) = drag_start {
            if !mouse_left_held {
                // 松开：框选完成。
                let (ex, ey) = (snap.mouse.x, snap.mouse.y);
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
                for uid in in_rect {
                    sel.select(uid);
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
                        log.begin().unwrap();
                        Hierarchy::new(rt.tree_mut(), &mut log)
                            .delete_subtree(&uid)
                            .unwrap();
                        log.commit().unwrap();
                    }
                }
            }
        }
        // Ctrl+Z / Ctrl+Y：undo / redo（直接消费事务历史）。
        if z_now && !prev_z {
            let _ = log.undo(rt.tree_mut());
        }
        if y_now && !prev_y {
            let _ = log.redo(rt.tree_mut());
        }
        prev_z = z_now;
        prev_y = y_now;
        prev_del = del_now;
        prev_tab = tab_now;

        // ---- UI 投影（每帧从状态模型重算，零自有状态）----
        // 输入框是否在编辑会话中（持焦点）：会话期间不换绑定 ——
        // 换选中触发的失焦提交要落到**开会话时**绑定的节点头上
        //（提交回调读 rename_bound，此刻换绑会把旧草稿安到新选中
        // 节点头上），失焦落账后下一帧再重绑新选中。
        let editing = rt.ui_vm_mut().focus() == Some(name_input);
        {
            let tree = rt.tree_mut();
            // Hierarchy View：树投影（前序 + 缩进 + 选中标记 *）。
            let mut lines = String::from("HIERARCHY\n");
            let sel_uids: Vec<Uid> = sel.uids().to_vec();
            fn walk(
                tree: &nes_scene::SceneTree,
                id: nes_scene::NodeId,
                depth: usize,
                sel: &[Uid],
                out: &mut String,
            ) {
                let name = tree.name(id).unwrap_or("?");
                let mark = tree
                    .uid_of(id)
                    .map(|u| sel.contains(&u))
                    .unwrap_or(false);
                let indent = "  ".repeat(depth);
                out.push_str(&format!("{}{}{}\n", indent, if mark { "* " } else { "  " }, name));
                for &c in tree.children(id) {
                    walk(tree, c, depth + 1, sel, out);
                }
            }
            walk(tree, tree.root(), 0, &sel_uids, &mut lines);
            let _ = tree.set_prop(hud_tree, PROP_LABEL_TEXT, Value::Str(lines));

            // Inspector View：选中节点数据投影。
            let mut ins_text = String::from("INSPECTOR\n");
            match sel.primary(tree) {
                Some(p) => {
                    let name = tree.name(p).unwrap_or("?");
                    let local = tree.local(p).unwrap_or_default();
                    let uid_hex = tree.uid_of(p).map(|u| u.to_hex()).unwrap_or_default();
                    ins_text.push_str(&format!("name: {}\npos: ({:.0}, {:.0})\nuid: {}...\n", name, local.pos.x, local.pos.y, &uid_hex[..8]));
                    for (k, v) in tree.props(p).unwrap().iter().take(4) {
                        ins_text.push_str(&format!("{}: {:?}\n", k, v));
                    }
                }
                None => ins_text.push_str("(no selection)"),
            }
            let _ = tree.set_prop(hud_ins, PROP_LABEL_TEXT, Value::Str(ins_text));

            // 状态栏。
            let st = format!(
                "st> undo:{} redo:{} sel:{} | Click=sel Shift+Click=multi Drag=box Arrows=move Del=del Ctrl+Z/Y=undo/redo",
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

            // 重命名输入框投影：有选中 → 可见且 text 绑定选中节点名
            //（换选中才重绑 —— 编辑会话中（editing）不换绑：会话的
            // 失焦提交归旧绑定，失焦后下一帧再绑新选中）。
            let primary_uid = sel.primary(tree).and_then(|p| tree.uid_of(p));
            let _ = tree.set_prop(name_input, "visible", Value::Bool(primary_uid.is_some()));
            if primary_uid != bound_sel && !editing {
                bound_sel = primary_uid.clone();
                *rename_bound.borrow_mut() = primary_uid.clone();
                if let Some(p) = sel.primary(tree) {
                    let name = tree.name(p).unwrap_or("").to_string();
                    let _ = tree.set_prop(name_input, "text", Value::Str(name));
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

        let _ = rt.emit_input_signals(&snap);
        let frame = FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(VIEWPORT.0, VIEWPORT.1));
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
            // 输入框 text 投影跟着落账后的新名走。
            let _ = tree.set_prop(name_input, "text", Value::Str(new_name));
        }

        std::thread::sleep(Duration::from_millis(16));
    }
    println!("[完成] Editor Shell 退出");
    let _ = (cam, hud_tree, hud_ins, hud_st, sel_box, name_input);
}
