//! S3「四项渲染缺口补齐」出口准则集成测试。
//!
//! 本文件只覆盖 S3 新增的四项缺口 + 一条与 `twn-render-stage` 既有输出的对齐比对：
//!
//! | 缺口 | 分组 | 这类测试在钉什么 |
//! |---|---|---|
//! | 相机视图矩阵 | `camera` | `Camera2D` 节点必须照实推送 `Camera2DState`（含 `active=false`），单槽**后写覆盖**；`view_matrix` 把注视点映射到视口中心、缩放按比例放大位移；相机**不**建渲染物 |
//! | Label 文本布局 | `label` | 非空文本必须带着完整排版参数抵达后端（文本 / 字号 / 字体键 / 对齐 / 换行），空文本不准入；清空文本要把既有渲染物销毁而不是留在后端 |
//! | Control 锚点布局 | `control` | 锚点 + 偏移 + 尺寸必须按契约算式解析后推送；**负尺寸不钳制**（v1.1 起 `min_size` 缺省为 `None` = 无下界，负宽高原样透传）；显式 `Some(min_size)` 只扩张右下边 |
//! | flip 合成 | `flip` | flip 只作**子局部后乘** `world ∘ scale(±1,±1)`：平移分量逐位不变、四类组合齐备，且**绝不**折进节点世界变换 |
//! | 与 TWN 对齐 | `twn_alignment` | 左右朝向的镜像语义必须与只读参照目录 `twn-render-stage` 记录的三元组 `(scale_x, scale_y, rotation_degrees)` 逐项对上 |
//!
//! # 依赖纪律（与 S2 测试一致）
//!
//! 只依赖两件事：`nes-render-extract` 的公开 API、`nes-render-api` 的 `NullRenderServer`。
//! 不引入任何第三方 crate，也不引入 `twn-render-stage` —— 该目录**只读**，
//! 比对测试用的是从它既有用例中**转录**下来的参照数值（见文件末尾 `twn_alignment` 分组）。

use std::cell::RefCell;
use std::collections::BTreeMap;

use nes_render_api::{
    Affine2, Flip, FrameInfo, ItemHandle, NullRenderServer, RenderAssetKey, RenderCommand, Vec2,
};
use nes_render_extract::{
    affine2_of, camera_state_of, compose_flip, control_state_of, label_state_of,
    ExtractStats, RenderExtractor, RenderKeySource, DEFAULT_LABEL_FONT_SIZE, PROP_CAMERA_ACTIVE,
    PROP_CAMERA_ZOOM, PROP_CONTROL_ANCHOR, PROP_CONTROL_OFFSET, PROP_CONTROL_SIZE, PROP_FLIP_H,
    PROP_FLIP_V, PROP_LABEL_FONT_SIZE, PROP_LABEL_TEXT, PROP_TEXTURE, PROP_VISIBLE, PROP_Z_INDEX,
};
use nes_scene::{NodeId, NodeKind, ResId, SceneTree, Transform2D, Value, Vec2 as SceneVec2};

// ---------------------------------------------------------------- 公共替身与工具

/// 帧上下文：S3 的相机缺口要看 `viewport`（相机状态里带着它），其余测试只做透传。
const VIEWPORT: Vec2 = Vec2::new(640.0, 360.0);

/// 资源键替身（与 S2 测试同构）：本文件只用来给精灵造一个**非空**纹理键，
/// 以证明 Label / Control 的身份键与资源类键不会撞车。
#[derive(Default)]
struct KeyMap(RefCell<BTreeMap<ResId, RenderAssetKey>>);

impl KeyMap {
    fn new() -> Self {
        Self::default()
    }

    fn set(&self, id: ResId, key: RenderAssetKey) {
        self.0.borrow_mut().insert(id, key);
    }
}

impl RenderKeySource for KeyMap {
    fn render_key(&self, id: ResId) -> Option<RenderAssetKey> {
        self.0.borrow().get(&id).copied()
    }
}

/// 造一个非空资源键（`slot == 0` 或 `gen == 0` 的都是空键）。
fn key(slot: u32, gen: u32) -> RenderAssetKey {
    RenderAssetKey::from_parts(slot, gen)
}

fn frame_info(index: u64) -> FrameInfo {
    FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, VIEWPORT)
}

/// 世界矩阵推进：落地结构变更 + 冲洗变换缓存。
fn advance(tree: &mut SceneTree) {
    tree.apply_pending();
    tree.refresh_transforms();
}

/// 写属性并断言 schema 接受它。
fn set_prop(tree: &mut SceneTree, node: NodeId, name: &str, value: Value) {
    tree.set_prop(node, name, value)
        .unwrap_or_else(|err| panic!("写入属性 {name} 被 schema 拒绝：{err:?}"));
}

fn add_sprite(tree: &mut SceneTree, parent: NodeId, name: &str, res: ResId) -> NodeId {
    let node = tree.add_node(parent, name, NodeKind::Sprite2D);
    set_prop(tree, node, PROP_TEXTURE, res.to_value());
    advance(tree);
    node
}

fn add_container(tree: &mut SceneTree, parent: NodeId, name: &str) -> NodeId {
    let node = tree.add_node(parent, name, NodeKind::Node2D);
    advance(tree);
    node
}

fn add_label(tree: &mut SceneTree, parent: NodeId, name: &str, text: &str, font_size: i64) -> NodeId {
    let node = tree.add_node(parent, name, NodeKind::Label);
    set_prop(tree, node, PROP_LABEL_TEXT, Value::Str(text.to_string()));
    set_prop(tree, node, PROP_LABEL_FONT_SIZE, Value::I64(font_size));
    advance(tree);
    node
}

fn add_control(
    tree: &mut SceneTree,
    parent: NodeId,
    name: &str,
    anchor: SceneVec2,
    offset: SceneVec2,
    size: SceneVec2,
) -> NodeId {
    let node = tree.add_node(parent, name, NodeKind::Control);
    set_prop(tree, node, PROP_CONTROL_ANCHOR, Value::Vec2(anchor));
    set_prop(tree, node, PROP_CONTROL_OFFSET, Value::Vec2(offset));
    set_prop(tree, node, PROP_CONTROL_SIZE, Value::Vec2(size));
    advance(tree);
    node
}

fn add_camera(
    tree: &mut SceneTree,
    parent: NodeId,
    name: &str,
    pos: (f32, f32),
    zoom: f32,
    active: bool,
) -> NodeId {
    let node = tree.add_node(parent, name, NodeKind::Camera2D);
    tree.set_local(node, Transform2D::from_pos(pos.0, pos.1));
    set_prop(tree, node, PROP_CAMERA_ZOOM, Value::F32(zoom));
    set_prop(tree, node, PROP_CAMERA_ACTIVE, Value::Bool(active));
    advance(tree);
    node
}

/// 提取一帧。
fn step(
    ex: &mut RenderExtractor,
    tree: &mut SceneTree,
    source: &dyn RenderKeySource,
    srv: &mut NullRenderServer,
    out: &mut Vec<RenderCommand>,
    index: u64,
) -> ExtractStats {
    let info = frame_info(index);
    ex.extract_into(tree, source, srv, &info, out)
}

/// 每帧必查的地基一致性：统计↔映射表↔后端三方不分叉，且契约层没记过无效操作。
fn assert_synced(ex: &RenderExtractor, srv: &NullRenderServer, stats: &ExtractStats) {
    assert_eq!(stats.map_len, ex.map().len(), "统计里的 map_len 与映射表不一致");
    assert_eq!(
        ex.map().len(),
        srv.len(),
        "映射表与后端存活渲染物数量分叉：映射表 {} / 后端 {}",
        ex.map().len(),
        srv.len()
    );
    for slot in ex.map().iter() {
        let item = srv
            .item(slot.handle)
            .unwrap_or_else(|| panic!("映射表里的句柄 {:?} 在后端已不存在（悬垂）", slot.handle));
        assert_eq!(item.key, slot.key, "句柄 {:?} 挂的资源键与映射表不符", slot.handle);
    }
    assert_eq!(
        srv.counters().ignored_ops,
        0,
        "契约层记录了空句柄 / 未知句柄操作：本层对后端发了它不认识的东西"
    );
}

/// 场景层 `Transform2D` → 契约层 `Affine2`（走唯一的桥接入口）。
fn affine_of(t: Transform2D) -> Affine2 {
    affine2_of(t.to_affine())
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5
}

fn assert_close(actual: f32, expected: f32, what: &str) {
    assert!(close(actual, expected), "{what}：期望 {expected}，实际 {actual}");
}

/// 独立重算期望绘制次序：`(z, NodeData::order, handle)` 全序 —— 与后端排序键无关地算一遍。
fn expected_draw_order(tree: &SceneTree, ex: &RenderExtractor) -> Vec<ItemHandle> {
    let mut items: Vec<(i32, u64, ItemHandle)> = Vec::new();
    for node in tree.preorder() {
        let Some(handle) = ex.handle_of(node) else {
            continue;
        };
        let z = tree
            .prop(node, PROP_Z_INDEX)
            .and_then(|value| value.as_i64())
            .unwrap_or(0) as i32;
        let order = tree.get(node).map_or(0, |data| data.order);
        items.push((z, order, handle));
    }
    items.sort();
    items.into_iter().map(|(_, _, handle)| handle).collect()
}

/// 命令种类名（用于断言命令流里的相对位置）。
fn kind_of(cmd: &RenderCommand) -> &'static str {
    match cmd {
        RenderCommand::CreateItem { .. } => "create",
        RenderCommand::DestroyItem { .. } => "destroy",
        RenderCommand::SetVisible { .. } => "set_visible",
        RenderCommand::SetTransform { .. } => "set_transform",
        RenderCommand::SetZ { .. } => "set_z",
        RenderCommand::SetFlip { .. } => "set_flip",
        RenderCommand::SetCamera { .. } => "set_camera",
        RenderCommand::SetText { .. } => "set_text",
        RenderCommand::SetRect { .. } => "set_rect",
        RenderCommand::Submit { .. } => "submit",
    }
}

fn count_of(out: &[RenderCommand], kind: &str) -> usize {
    out.iter().filter(|cmd| kind_of(cmd) == kind).count()
}

/// 某句柄的某类命令在本帧命令流里的位置（`None` = 本帧没有该命令）。
fn index_of(out: &[RenderCommand], kind: &str, handle: ItemHandle) -> Option<usize> {
    out.iter()
        .position(|cmd| kind_of(cmd) == kind && cmd.handle() == Some(handle))
}

/// 断言某句柄的某类命令存在且排在另一类之后。
fn assert_after(out: &[RenderCommand], handle: ItemHandle, later: &str, earlier: &str) {
    let a = index_of(out, earlier, handle)
        .unwrap_or_else(|| panic!("句柄 {handle:?} 本帧缺少 {earlier} 命令"));
    let b = index_of(out, later, handle)
        .unwrap_or_else(|| panic!("句柄 {handle:?} 本帧缺少 {later} 命令"));
    assert!(a < b, "句柄 {handle:?} 的 {later} 没有排在 {earlier} 之后（{a} vs {b}）");
}

fn first_index_of_kind(out: &[RenderCommand], kind: &str) -> Option<usize> {
    out.iter().position(|cmd| kind_of(cmd) == kind)
}

// ---------------------------------------------------------------- 相机（缺口一）

/// 相机单槽**后写覆盖**：前序序里最后写入的那台生效；`active=false` 也**照实推送**，
/// 是否可用由契约层 `view_matrix()` 决定。相机永远不建渲染物。
#[test]
fn criterion_gaps_camera_last_write_wins_and_disabled_state_still_pushed() {
    let mut tree = SceneTree::new("root");
    let keys = KeyMap::new();
    keys.set(ResId::new(1), key(1, 1));
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    let root = tree.root();
    let sprite = add_sprite(&mut tree, root, "bg", ResId::new(1));
    let cam_a = add_camera(&mut tree, root, "cam_a", (10.0, 20.0), 2.0, true);
    let cam_b = add_camera(&mut tree, root, "cam_b", (-5.0, 0.0), 0.25, false);

    // 第 1 帧：两台相机都推过，但只有最后写入的那台留在单槽里 —— cam_b。
    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 0);
    assert_synced(&ex, &srv, &stats);
    assert_eq!(count_of(&out, "set_camera"), 1, "每帧最多一条相机命令");

    let cam = *srv.camera().expect("相机必须落到单槽里");
    assert_eq!(
        cam.transform.to_array(),
        affine_of(Transform2D::from_pos(-5.0, 0.0)).to_array(),
        "后写覆盖：应当是 cam_b 的世界变换"
    );
    assert_eq!(cam.zoom, Vec2::new(0.25, 0.25), "相机缩放必须逐位抵达后端");
    assert_eq!(cam.viewport, VIEWPORT, "视口尺寸（设备像素）必须透传");
    assert_eq!(cam.offset, Vec2::ZERO, "无 offset 属性时偏移必须为零");
    assert!(cam.limits.is_none(), "无 limits 属性时提取层不得凭空造限制");
    assert!(!cam.enabled, "cam_b 的 active=false 必须落成 enabled=false");
    assert!(
        cam.view_matrix().is_none(),
        "未启用相机的 view_matrix 必须是 None（后端据此退回上一有效相机）"
    );
    assert_eq!(
        camera_state_of(&tree, cam_b, VIEWPORT),
        cam,
        "单槽内容必须等于 cam_b 的独立重算结果"
    );

    // 相机不参与渲染物生命周期。
    assert!(ex.handle_of(cam_a).is_none(), "相机不得被分配句柄");
    assert!(ex.handle_of(cam_b).is_none(), "相机不得被分配句柄");
    assert_eq!(stats.created, 1, "本帧只应创建精灵那一个渲染物");
    assert!(ex.handle_of(sprite).is_some(), "精灵必须有渲染物");

    // 第 2 帧：把 cam_b 也启用，单槽内容不变（顺序没变），但 enabled 要翻过来。
    set_prop(&mut tree, cam_b, PROP_CAMERA_ACTIVE, Value::Bool(true));
    advance(&mut tree);
    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
    assert_synced(&ex, &srv, &stats);
    let cam = *srv.camera().expect("相机必须落到单槽里");
    assert!(cam.enabled, "active=true 必须落成 enabled=true");
    assert!(cam.view_matrix().is_some(), "启用相机的 view_matrix 必须存在");

    // 第 3 帧：再挂一台相机（前序序更靠后）⇒ 换成它，照实推送 enabled=false。
    let cam_c = add_camera(&mut tree, root, "cam_c", (7.0, -7.0), 4.0, false);
    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
    assert_synced(&ex, &srv, &stats);
    let cam = *srv.camera().expect("相机必须落到单槽里");
    assert_eq!(
        cam.transform.to_array(),
        affine_of(Transform2D::from_pos(7.0, -7.0)).to_array(),
        "新增相机排在前序序更后位置 ⇒ 覆盖单槽"
    );
    assert_eq!(cam.zoom, Vec2::new(4.0, 4.0));
    assert!(!cam.enabled, "cam_c 的 active=false 必须落成 enabled=false");
    assert_eq!(camera_state_of(&tree, cam_c, VIEWPORT), cam);
}

/// 相机状态 = 场景层 `world` 缓存 + 缩放/视口的逐位映射；`view_matrix` 把注视点
/// 映射到视口中心，并按 `zoom` 成比例放大位移。
#[test]
fn criterion_gaps_camera_view_matrix_maps_center_and_scales_deltas() {
    let mut tree = SceneTree::new("root");
    let keys = KeyMap::new();
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    let root = tree.root();
    let _cam = add_camera(&mut tree, root, "cam", (100.0, 50.0), 2.0, true);

    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 0);
    assert_synced(&ex, &srv, &stats);

    let cam = *srv.camera().expect("相机必须落到单槽里");
    let expected = camera_state_of(&tree, _cam, VIEWPORT);
    assert_eq!(cam, expected, "推送的相机状态必须等于独立重算结果");
    assert_eq!(cam.transform.to_array(), affine_of(Transform2D::from_pos(100.0, 50.0)).to_array());
    assert_eq!(cam.zoom, Vec2::new(2.0, 2.0));
    assert!(cam.enabled);

    let view = cam.view_matrix().expect("启用相机必须给视图矩阵");
    let center = cam.center();
    assert_eq!(center.to_array(), [100.0, 50.0], "无 offset 时注视点就是相机位置");

    let mapped = view.apply(center);
    assert_close(mapped.x, VIEWPORT.x * 0.5, "注视点的视图 x 必须落在视口中心");
    assert_close(mapped.y, VIEWPORT.y * 0.5, "注视点的视图 y 必须落在视口中心");

    // zoom = 2 ⇒ 世界 +10 单位在视图里是 +20 像素；方向不变、比例准确。
    let offset_world = Vec2::new(center.x + 10.0, center.y);
    let mapped_offset = view.apply(offset_world);
    assert_close(mapped_offset.x, VIEWPORT.x * 0.5 + 20.0, "zoom 必须按比例放大位移");
    assert_close(mapped_offset.y, VIEWPORT.y * 0.5, "横向位移不得串到纵向");

    // 负方向同样成立（视图是仿射，不是"只能放大右半边"）。
    let negative = view.apply(Vec2::new(center.x - 10.0, center.y - 5.0));
    assert_close(negative.x, VIEWPORT.x * 0.5 - 20.0, "负方向位移同样按 zoom 缩放");
    assert_close(negative.y, VIEWPORT.y * 0.5 - 10.0, "负方向位移同样按 zoom 缩放");
}

/// 相机节点不产生渲染物，但仍要吃到父链的世界变换。
#[test]
fn criterion_gaps_camera_node_yields_no_render_item_but_gets_world_transform() {
    let mut tree = SceneTree::new("root");
    let keys = KeyMap::new();
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    let root = tree.root();
    let rig = add_container(&mut tree, root, "rig");
    tree.set_local(rig, Transform2D::from_pos(30.0, -12.0));
    advance(&mut tree);
    let cam = add_camera(&mut tree, rig, "cam", (0.0, 0.0), 3.0, true);

    for frame in 0..2 {
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, frame);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.created, 0, "相机不是渲染物，不该创建句柄");
        assert_eq!(stats.pushed, 0, "相机不该进推送计数");
        assert_eq!(stats.nodes_visited, 3, "根 + rig + 相机都应被遍历到");
        assert_eq!(count_of(&out, "create"), 0, "命令流里不该有渲染物创建");
        assert!(
            first_index_of_kind(&out, "set_transform").is_none(),
            "没有任何渲染物时不该有变换命令"
        );
        let cam_state = *srv.camera().expect("相机必须落到单槽里");
        assert_eq!(
            cam_state.transform.to_array(),
            affine_of(Transform2D::from_pos(30.0, -12.0)).to_array(),
            "相机状态必须取父链复合后的世界变换"
        );
        assert_eq!(cam_state.zoom, Vec2::new(3.0, 3.0));
    }

    assert!(ex.handle_of(cam).is_none());
    assert!(srv.is_empty(), "后端不该有任何渲染物");
}

// ---------------------------------------------------------------- Label（缺口二）

/// Label 的文本状态必须**带着完整排版参数**抵达后端，且身份键稳定、与资源键不撞车。
#[test]
fn criterion_gaps_label_state_pushed_with_full_layout_payload() {
    let mut tree = SceneTree::new("root");
    let keys = KeyMap::new();
    keys.set(ResId::new(1), key(1, 1));
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    let root = tree.root();
    let sprite = add_sprite(&mut tree, root, "bg", ResId::new(1));
    let label = add_label(&mut tree, root, "title", "hello", 24);
    // 第二个 Label 不写字号：必须落到契约层默认字号，而不是 0 / 未初始化。
    let plain = tree.add_node(root, "plain", NodeKind::Label);
    set_prop(&mut tree, plain, PROP_LABEL_TEXT, Value::Str("plain".to_string()));
    advance(&mut tree);

    let stats0 = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 0);
    assert_synced(&ex, &srv, &stats0);
    let handle = ex.handle_of(label).expect("非空文本的 Label 必须有渲染物");
    let sprite_handle = ex.handle_of(sprite).expect("精灵必须有渲染物");

    let state = srv.label_of(handle).expect("后端必须收到文本状态").clone();
    assert_eq!(label_state_of(&tree, label).as_ref(), Some(&state), "推送内容 = 独立重算结果");
    assert_eq!(&*state.text, "hello", "文本内容必须逐字抵达");
    assert_eq!(state.font_size, 24.0, "字号必须抵达（I64 属性按 f32 出口）");
    assert!(state.font.is_nil(), "未绑字体资源时必须是空键（用后端默认字体）");
    assert_eq!(state.align_h, nes_render_api::HAlign::Left, "默认左上对齐");
    assert_eq!(state.align_v, nes_render_api::VAlign::Top, "默认左上对齐");
    assert_eq!(state.line_spacing, 0.0, "无额外行距时必须归零，不得留脏值");
    assert!(state.wrap_width.is_none(), "未给换行宽度时必须是 None");

    let plain_handle = ex.handle_of(plain).expect("默认字号的 Label 也必须有渲染物");
    let plain_state = srv.label_of(plain_handle).expect("后端必须收到文本状态");
    assert_eq!(plain_state.font_size, DEFAULT_LABEL_FONT_SIZE, "缺省字号按契约常量");

    // 身份键：非空、稳定、与资源类键不撞车、两个 Label 各不相同。
    let item = srv.item(handle).expect("Label 必须有渲染物");
    assert!(!item.key.is_nil(), "Label 的身份键不能是空键");
    assert_ne!(item.key, srv.item(sprite_handle).unwrap().key, "节点身份键不许与纹理键撞车");
    assert_ne!(item.key, srv.item(plain_handle).unwrap().key, "不同 Label 的身份键必须不同");
    assert!(index_of(&out, "set_text", handle).is_some(), "文本必须落到命令流里");
    assert!(
        index_of(&out, "set_text", sprite_handle).is_none(),
        "精灵不该收到文本命令"
    );
    assert_eq!(count_of(&out, "set_text"), 2, "两个 Label 各一条文本命令");

    // 第 2 帧：句柄复用（不重建），状态继续照实推送。
    let stats1 = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
    assert_synced(&ex, &srv, &stats1);
    assert_eq!(stats1.created, 0, "空闲帧不得重建渲染物");
    assert_eq!(stats1.destroyed, 0);
    assert_eq!(ex.handle_of(label), Some(handle), "句柄必须跨帧稳定");
    assert_eq!(srv.label_of(handle), Some(&state), "空闲帧文本状态不变");
}

/// 空文本不准入；把已成条目的 Label 文本清空 ⇒ 必须销毁后端渲染物（而不是留悬垂）。
#[test]
fn criterion_gaps_label_empty_text_not_admitted_and_cleared_label_destroyed() {
    let mut tree = SceneTree::new("root");
    let keys = KeyMap::new();
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    let root = tree.root();
    // 从一开始就是空文本的 Label：永远不生成渲染物。
    let empty = add_label(&mut tree, root, "empty", "", 16);
    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 0);
    assert_synced(&ex, &srv, &stats);
    assert_eq!(stats.created, 0, "空文本 Label 不准入");
    assert!(ex.handle_of(empty).is_none());
    assert_eq!(srv.len(), 0);
    assert_eq!(count_of(&out, "set_text"), 0, "没有渲染物就不该有文本命令");

    // 有文本 ⇒ 建条目；文本清空 ⇒ 下一帧销毁。
    let label = add_label(&mut tree, root, "t", "x", 16);
    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
    assert_synced(&ex, &srv, &stats);
    let old = ex.handle_of(label).expect("非空文本必须有渲染物");
    assert_eq!(stats.created, 1);
    assert_eq!(stats.fresh, 1);

    set_prop(&mut tree, label, PROP_LABEL_TEXT, Value::Str(String::new()));
    advance(&mut tree);
    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
    assert_synced(&ex, &srv, &stats);
    assert_eq!(stats.destroyed, 1, "清空文本必须销毁既有渲染物");
    assert_eq!(stats.dropped, 1, "这一帧的销毁应当是「不再准入」路径");
    assert!(ex.handle_of(label).is_none(), "映射表不得留下已不存在的条目");
    assert!(srv.item(old).is_none(), "后端不得留下悬垂渲染物");
    assert!(srv.label_of(old).is_none(), "文本状态必须随渲染物一并清掉");
    assert_eq!(srv.len(), 0);
    assert_eq!(index_of(&out, "set_text", old), None, "被销毁的句柄不该再收文本命令");

    // 文本恢复 ⇒ 新句柄（旧句柄此后永不分配），且是全新创建。
    set_prop(&mut tree, label, PROP_LABEL_TEXT, Value::Str("y".to_string()));
    advance(&mut tree);
    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 3);
    assert_synced(&ex, &srv, &stats);
    let fresh = ex.handle_of(label).expect("文本恢复后必须重新建条目");
    assert_ne!(fresh, old, "旧句柄销毁后不得被复用");
    assert_eq!(stats.created, 1);
    assert_eq!(stats.fresh, 1);
    assert_eq!(srv.label_of(fresh).map(|s| s.text.to_string()), Some("y".to_string()));
}

// ---------------------------------------------------------------- Control（缺口三）

/// Control 恒准入（即使 `visible=false`），布局状态必须按锚点算式解析后推送；
/// `min_size` 缺省为 `None`（无下界），显式 `Some(min)` 时只扩张右下边。
#[test]
fn criterion_gaps_control_layout_pushed_and_resolves_anchors() {
    let mut tree = SceneTree::new("root");
    let keys = KeyMap::new();
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    let root = tree.root();
    let control = add_control(
        &mut tree,
        root,
        "panel",
        SceneVec2::new(0.5, 0.5),
        SceneVec2::new(10.0, 20.0),
        SceneVec2::new(100.0, 200.0),
    );
    set_prop(&mut tree, control, PROP_VISIBLE, Value::Bool(false));
    advance(&mut tree);

    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 0);
    assert_synced(&ex, &srv, &stats);
    assert_eq!(stats.created, 1, "Control 恒准入：不可见也建渲染物");

    let handle = ex.handle_of(control).expect("Control 必须有渲染物");
    let item = srv.item(handle).expect("Control 必须有渲染物");
    assert!(!item.visible, "visible=false 必须照实推送（只跳过绘制，不销毁）");

    let layout = *srv.rect_of(handle).expect("后端必须收到控件布局");
    assert_eq!(layout, control_state_of(&tree, control), "推送内容 = 独立重算结果");
    assert_eq!(layout.anchor_left, 0.5);
    assert_eq!(layout.anchor_bottom, 0.5, "四锚点按 Vec2 广播");
    assert_eq!(layout.offset_left, 10.0);
    assert_eq!(layout.offset_top, 20.0);
    assert_eq!(layout.offset_right, 110.0, "右偏移 = 左偏移 + 宽度");
    assert_eq!(layout.offset_bottom, 220.0, "下偏移 = 上偏移 + 高度");
    assert_eq!(layout.min_size, None, "提取层不得凭空造最小尺寸：缺省即无下界");

    // 解析：父 400x200，锚点 0.5/0.5 ⇒ 左上落在父中心 + 偏移，宽高即 size。
    let resolved = layout.resolve(Vec2::new(400.0, 200.0));
    assert_eq!(resolved.to_array(), [210.0, 120.0, 100.0, 200.0]);

    // 显式 `Some(min_size)` 时只把右下边推出去，左上角不动。
    let mut with_min = layout;
    with_min.min_size = Some(Vec2::new(200.0, 300.0));
    assert_eq!(with_min.resolve(Vec2::new(400.0, 200.0)).to_array(), [210.0, 120.0, 200.0, 300.0]);

    // 铺满父容器：四锚点 0/1 + 零偏移 ⇒ 与父同尺寸。
    let full = nes_render_api::ControlState::FULL_RECT;
    assert_eq!(full.resolve(Vec2::new(320.0, 180.0)).to_array(), [0.0, 0.0, 320.0, 180.0]);

    assert_eq!(count_of(&out, "set_rect"), 1, "布局必须落到命令流里");
    assert_after(&out, handle, "set_rect", "set_visible");
}

/// 负尺寸在**提取层**不钳制（负偏移逐位透传），在**契约层**同样不钳制：
/// v1.1 起 `ControlState::min_size` 缺省为 `None`（无下界），`resolve` 出口的负宽高
/// 逐位等于算式结果 —— 真缺陷 D-S3-1 已按裁决 A 修复（见 S3 封口文档缺陷清单
/// 与 S1 文档 v1.1 修订小节）。
///
/// 本用例原先钉死的是"契约层把负宽高压成 0"的**缺陷现状**；缺陷按裁决修复后，
/// 这里改为钉死**修复后语义**（用例保留、断言未放宽，只是把预期换成裁决要求的行为），
/// 并保留"负下界 → 原样返回"的归因对照，防止将来有人把钳制分支重新搬回来。
#[test]
fn criterion_gaps_control_negative_size_passes_through_and_is_not_clamped() {
    let mut tree = SceneTree::new("root");
    let keys = KeyMap::new();
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    let root = tree.root();
    let control = add_control(
        &mut tree,
        root,
        "flipped_panel",
        SceneVec2::ZERO,
        SceneVec2::new(5.0, 5.0),
        SceneVec2::new(-30.0, -40.0),
    );

    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 0);
    assert_synced(&ex, &srv, &stats);
    let handle = ex.handle_of(control).expect("Control 必须有渲染物");
    let layout = *srv.rect_of(handle).expect("后端必须收到控件布局");

    // 提取层：不钳制。负尺寸与由它派生的负偏移逐位透传，信息不在本层丢。
    assert_eq!(layout.offset_right, -25.0, "右偏移 = 5 + (-30)，提取层不得钳到 0");
    assert_eq!(layout.offset_bottom, -35.0, "下偏移 = 5 + (-40)，提取层不得钳到 0");
    assert_eq!(layout.min_size, None, "提取层不得凭空造最小尺寸：缺省即无下界");

    // 契约层（v1.1 修复后）：无下界 ⇒ 负宽高原样到出口，不再被压成 0。
    assert_eq!(
        layout.resolve(Vec2::new(100.0, 100.0)).to_array(),
        [5.0, 5.0, -30.0, -40.0],
        "契约层 resolve 必须原样透传负宽高（D-S3-1 已按裁决修复）"
    );

    // 归因：出口不是"另有钳制分支"顶回来的 —— 即使显式给一个**负下界**，
    // 负宽高也逐位原样返回（说明算式本身没丢信息，钳制只可能来自显式下界）。
    let mut negative_floor = layout;
    negative_floor.min_size = Some(Vec2::new(-100.0, -100.0));
    assert_eq!(
        negative_floor.resolve(Vec2::new(100.0, 100.0)).to_array(),
        [5.0, 5.0, -30.0, -40.0],
        "负下界下负宽高同样原样保留"
    );

    // 对照（旧语义未放宽）：显式 `Some(正下界)` 仍只把右下边推出去，左上角不动；
    // 本例宽高 20x10 小于下界 30x30 ⇒ 被抬到下界，而不是被抬到 0。
    let mut positive_floor = layout;
    positive_floor.min_size = Some(Vec2::new(30.0, 30.0));
    assert_eq!(
        positive_floor.resolve(Vec2::new(100.0, 100.0)).to_array(),
        [5.0, 5.0, 30.0, 30.0],
        "显式下界只推右下边：左上角不动，且不出现在 0 上"
    );
}

/// 端到端钉死（v1.1 修订新增用例）：负宽高从场景属性一路到 `resolve` 出口都不得被钳制，
/// 且提取层下界恒为 `None`（无下界），不得退化成"零下界"。
///
/// 与上一条的区别：这里不看归因/对照，只看**一条完整链路**的逐位数值 ——
/// 场景 `size = (-50, -60)` ⇒ 推送的四边偏移 ⇒ `resolve(parent)` 的出口矩形，
/// 任何一环把负值改成 0 都会红。
#[test]
fn criterion_gaps_control_negative_size_reaches_resolve_without_lower_bound() {
    let mut tree = SceneTree::new("root");
    let keys = KeyMap::new();
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    let root = tree.root();
    let control = add_control(
        &mut tree,
        root,
        "negative_stretch",
        SceneVec2::new(0.5, 0.5),
        SceneVec2::new(10.0, 20.0),
        SceneVec2::new(-50.0, -60.0),
    );

    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 0);
    assert_synced(&ex, &srv, &stats);
    let handle = ex.handle_of(control).expect("Control 必须有渲染物");
    let layout = *srv.rect_of(handle).expect("后端必须收到控件布局");

    // 提取层：下界必须是"无下界"（None），负尺寸派生的负偏移逐位保留。
    assert_eq!(layout.min_size, None, "提取层下界必须是 None（无下界），不得造零下界");
    assert_eq!(layout.offset_right, -40.0, "右偏移 = 10 + (-50)");
    assert_eq!(layout.offset_bottom, -40.0, "下偏移 = 20 + (-60)");

    // 契约层出口：父 400x200 + 锚点 0.5 ⇒ 左上 (210,120)，宽高逐位等于 size（含负号）。
    assert_eq!(
        layout.resolve(Vec2::new(400.0, 200.0)).to_array(),
        [210.0, 120.0, -50.0, -60.0],
        "负宽高必须原样到出口，不得压成 0"
    );
}

// ---------------------------------------------------------------- flip（缺口四）

/// flip 是**子局部后乘** `world ∘ scale(±1,±1)`：平移分量逐位不变，四类组合齐备，
/// 且点映射等价于"先镜像局部坐标、再走世界变换"。
#[test]
fn criterion_gaps_flip_compose_is_child_local_post_multiply() {
    let world = affine_of(Transform2D {
        pos: SceneVec2::new(30.0, -12.0),
        rot: 0.7,
        scale: SceneVec2::new(2.0, 3.0),
        skew: 0.0,
    });
    let base = world.to_array();

    // 四类组合：列的符号按方向取反（第 0 列随 h，第 1 列随 v）。
    for (h, v) in [(false, false), (true, false), (false, true), (true, true)] {
        let flip = Flip::new(h, v);
        let composed = compose_flip(world, flip);
        let got = composed.to_array();
        let sx = if h { -1.0 } else { 1.0 };
        let sy = if v { -1.0 } else { 1.0 };
        assert_eq!(
            got,
            [
                base[0] * sx,
                base[1] * sx,
                base[2] * sy,
                base[3] * sy,
                base[4],
                base[5]
            ],
            "flip(h={h}, v={v}) 的合成结果不对"
        );
        assert_eq!(got[4], base[4], "flip(h={h}, v={v}) 后平移 x 必须逐位不变");
        assert_eq!(got[5], base[5], "flip(h={h}, v={v}) 后平移 y 必须逐位不变");

        // 点语义：合成矩阵作用 = 先把局部点镜像、再走世界变换。
        let p = Vec2::new(3.0, 7.0);
        let mirrored = Vec2::new(if h { -p.x } else { p.x }, if v { -p.y } else { p.y });
        let via_compose = composed.apply(p);
        let via_world = world.apply(mirrored);
        assert_close(via_compose.x, via_world.x, "合成矩阵的点映射必须等于「先镜像再变换」");
        assert_close(via_compose.y, via_world.y, "合成矩阵的点映射必须等于「先镜像再变换」");
    }

    // 不翻转时必须是**逐位**原样（合成不得引入任何浮点扰动）。
    assert_eq!(compose_flip(world, Flip::IDENTITY).to_array(), base);
    assert_eq!(Flip::IDENTITY.to_affine().to_array(), Affine2::IDENTITY.to_array());
}

/// flip 从节点属性来、**不折进**世界变换：改 flip 不动 `transform`，也不重建句柄。
#[test]
fn criterion_gaps_flip_pushed_from_props_and_never_folded_into_transform() {
    let mut tree = SceneTree::new("root");
    let keys = KeyMap::new();
    keys.set(ResId::new(1), key(1, 1));
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    let root = tree.root();
    let sprite = add_sprite(&mut tree, root, "hero", ResId::new(1));
    tree.set_local(
        sprite,
        Transform2D {
            pos: SceneVec2::new(4.0, 5.0),
            rot: 0.25,
            scale: SceneVec2::new(3.0, 3.0),
            skew: 0.0,
        },
    );
    advance(&mut tree);
    set_prop(&mut tree, sprite, PROP_FLIP_H, Value::Bool(true));
    advance(&mut tree);

    let stats0 = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 0);
    assert_synced(&ex, &srv, &stats0);
    let handle = ex.handle_of(sprite).expect("精灵必须有渲染物");
    let item0 = *srv.item(handle).expect("精灵必须有渲染物");
    let world_now = affine2_of(tree.world(sprite).expect("精灵必须有世界变换"));

    assert_eq!(item0.flip, Flip::new(true, false), "flip_h 必须落成 Flip(h=true, v=false)");
    assert_eq!(item0.transform.to_array(), world_now.to_array(), "transform 只装世界变换");
    assert_eq!(
        item0.world_transform().to_array(),
        compose_flip(item0.transform, item0.flip).to_array(),
        "绘制矩阵必须等于「世界变换 ∘ flip」"
    );
    assert_ne!(
        item0.transform.to_array(),
        item0.world_transform().to_array(),
        "翻转必须体现在绘制矩阵上，而不是被忽略"
    );

    // 第 2 帧：换成垂直翻转 —— 句柄复用、transform 不变、绘制矩阵跟着变。
    set_prop(&mut tree, sprite, PROP_FLIP_H, Value::Bool(false));
    set_prop(&mut tree, sprite, PROP_FLIP_V, Value::Bool(true));
    advance(&mut tree);
    let stats1 = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
    assert_synced(&ex, &srv, &stats1);
    assert_eq!(stats1.created, 0, "改 flip 不得重建渲染物");
    assert_eq!(stats1.destroyed, 0);
    assert_eq!(stats1.reused, 1);
    assert_eq!(ex.handle_of(sprite), Some(handle), "句柄必须跨帧稳定");

    let item1 = *srv.item(handle).expect("精灵必须有渲染物");
    assert_eq!(item1.flip, Flip::new(false, true), "flip 必须被逐帧覆盖（不是累积）");
    assert_eq!(
        item1.transform.to_array(),
        item0.transform.to_array(),
        "改 flip 不许动节点世界变换"
    );
    assert_eq!(
        item1.world_transform().to_array(),
        compose_flip(item1.transform, item1.flip).to_array()
    );
    assert_eq!(count_of(&out, "set_flip"), 1, "每帧每渲染物一条 flip 快照");
    assert_after(&out, handle, "set_z", "set_flip");
    assert_after(&out, handle, "set_visible", "set_z");
}

// ---------------------------------------------------------------- 与 TWN 既有输出对齐

/// 与只读参照目录 `twn-render-stage` 的既有用例对齐：左右朝向的镜像**不靠旋转**实现。
///
/// 参照来源（转录，不依赖、不修改）：
/// `temp/twn5/D0L/crates/twn-render-stage/src/lib.rs` 的
/// `left_right_rotation_flips_x_scale_without_rotating` —— 基准缩放 2.0 时，
/// 朝向 +X 记录 `(scale_x, scale_y, rotation_degrees) = (2.0, 2.0, 0.0)`，
/// 朝向 -X 记录 `(-2.0, 2.0, 0.0)`：**只翻 x 缩放的符号，不引入旋转**。
///
/// 本层的对应表示：`(a, b, c, d) = (scale_x, 0, 0, scale_y)`。
/// 因此"rotation_degrees == 0"在本层的可观测判据是 `b == 0 && c == 0`，
/// 而"镜像 +X / -X"对应 `a` 的符号、`d` 保持正号。
#[test]
fn criterion_gaps_flip_alignment_with_twn_render_stage_reference() {
    let world = affine_of(Transform2D {
        pos: SceneVec2::new(12.0, -4.0),
        rot: 0.0,
        scale: SceneVec2::new(2.0, 2.0),
        skew: 0.0,
    });

    // TWN 朝向 +X：不镜像。
    let facing_right = compose_flip(world, Flip::IDENTITY).to_array();
    assert_eq!(facing_right, [2.0, 0.0, 0.0, 2.0, 12.0, -4.0], "对齐 TWN (2.0, 2.0, 0.0)");

    // TWN 朝向 -X：只翻 x 缩放符号，rotation 仍为 0（b、c 逐位为 0），且位置不动。
    let facing_left = compose_flip(world, Flip::new(true, false)).to_array();
    assert_eq!(facing_left, [-2.0, 0.0, 0.0, 2.0, 12.0, -4.0], "对齐 TWN (-2.0, 2.0, 0.0)");
    assert_eq!(facing_left[1], 0.0, "rotation_degrees == 0 ⇒ 本层 b 逐位为 0");
    assert_eq!(facing_left[2], 0.0, "rotation_degrees == 0 ⇒ 本层 c 逐位为 0");
    assert_eq!(facing_left[3], 2.0, "y 缩放不参与左右镜像，保持 +2.0");
    assert_eq!(&facing_left[4..], &facing_right[4..], "左右镜像不得搬动位置");

    // 反向对照：若把"左右镜像"实现成绕原点旋转 180°，y 缩放会变成 -2.0，
    // 且 b、c 不再是 0 —— 这正是 TWN 用例名字里 "without rotating" 要排除的错法。
    let rotated_instead = Affine2::rotation(std::f32::consts::PI).mul(&Affine2::scale(2.0, 2.0));
    let rotated = rotated_instead.to_array();
    assert_eq!(rotated[3], -2.0, "旋转 180° 的 y 缩放会变号（与 TWN 记录不符）");
    assert_ne!(rotated[1], 0.0, "旋转 180° 会引入非零 b（与 rotation_degrees == 0 不符）");
    assert_ne!(
        rotated, facing_left,
        "本层实现的镜像必须与「旋转 180°」这种错法**可区分**"
    );
}

// ---------------------------------------------------------------- 帧布局（I6 落点）

/// 命令流的帧内布局：生命周期动作 → 相机（每帧最多一条） → 各渲染物按绘制次序
/// 输出 `transform → flip → z → visible → text → rect` 全量快照 → `Submit`。
/// 这是 S1 契约 I6 在 S3 新增三类推送（相机 / 文本 / 布局）后的落点检查。
#[test]
fn criterion_gaps_command_layout_puts_camera_first_and_text_rect_last_per_item() {
    let mut tree = SceneTree::new("root");
    let keys = KeyMap::new();
    keys.set(ResId::new(1), key(1, 1));
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    let root = tree.root();
    let rig = add_container(&mut tree, root, "rig");
    let _cam = add_camera(&mut tree, rig, "cam", (0.0, 0.0), 1.0, true);
    let sprite = add_sprite(&mut tree, root, "bg", ResId::new(1));
    let label = add_label(&mut tree, root, "title", "hi", 20);
    let control = add_control(
        &mut tree,
        root,
        "panel",
        SceneVec2::new(0.0, 0.0),
        SceneVec2::new(1.0, 2.0),
        SceneVec2::new(30.0, 40.0),
    );
    // `z_index` 只属于 Node2D 家族，Label / Control 不接收该属性 ⇒ 它们停在 z = 0，
    // 次序由场景层兄弟序决定；精灵给一个更大的 z，用来证明 z 压过兄弟序。
    set_prop(&mut tree, sprite, PROP_Z_INDEX, Value::I64(9));
    advance(&mut tree);

    let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 0);
    assert_synced(&ex, &srv, &stats);

    // 帧尾标记。
    match out.last().expect("命令流不得为空") {
        RenderCommand::Submit { frame } => assert_eq!(frame.frame_index, 0),
        other => panic!("帧尾必须是 Submit，实际是 {}", kind_of(other)),
    }

    // 相机：每帧最多一条，且排在任何渲染物命令之前。
    assert_eq!(count_of(&out, "set_camera"), 1);
    let camera_at = first_index_of_kind(&out, "set_camera").expect("必须推相机");
    let first_item_at = first_index_of_kind(&out, "set_transform").expect("必须推渲染物");
    assert!(camera_at < first_item_at, "相机命令必须排在渲染物命令之前");

    // 每个渲染物的属性顺序：transform → flip → z → visible → text/rect。
    let sprite_handle = ex.handle_of(sprite).expect("精灵必须有条目");
    let label_handle = ex.handle_of(label).expect("Label 必须有条目");
    let control_handle = ex.handle_of(control).expect("Control 必须有条目");
    for handle in [sprite_handle, label_handle, control_handle] {
        assert_after(&out, handle, "set_flip", "set_transform");
        assert_after(&out, handle, "set_z", "set_flip");
        assert_after(&out, handle, "set_visible", "set_z");
    }
    assert_after(&out, label_handle, "set_text", "set_visible");
    assert_after(&out, control_handle, "set_rect", "set_visible");
    assert_eq!(count_of(&out, "set_text"), 1, "只有 Label 收文本命令");
    assert_eq!(count_of(&out, "set_rect"), 1, "只有 Control 收布局命令");

    // 绘制次序：独立重算的 (z, order, handle) 全序必须与后端排序一致。
    assert_eq!(srv.draw_order(), expected_draw_order(&tree, &ex));
    let order = srv.draw_order();
    assert_eq!(order.len(), 3, "精灵 / Label / Control 三个渲染物");
    assert_eq!(
        Some(label_handle),
        order.first().copied(),
        "同为 z = 0 时，兄弟序更早的 Label 必须排在 Control 之前"
    );
    assert_eq!(Some(control_handle), order.get(1).copied(), "z = 0 的两项按兄弟序排列");
    assert_eq!(
        Some(sprite_handle),
        order.last().copied(),
        "z_index = 9 必须压过更早的兄弟序，排到最后"
    );
}
