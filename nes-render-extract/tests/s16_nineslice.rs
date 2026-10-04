//! S16.6 提取层契约：`Control` 的九宫格属性（`ns_tex` + `ns_l/t/r/b`）
//! → `SetNineSlice` 的准入 / 覆写 / 清除语义。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-NSX-01 | ns 属性有效的 Control → 命令流携带 SetNineSlice（键 + 边距四元组逐位） |
//! | T-NSX-02 | 缺省 Control / 未绑定纹理 / 全零边距 → 不推 SetNineSlice（缺省路径命令流与既有逐条相同） |
//! | T-NSX-03 | 有效 → 清空迁移帧补推一次 NIL 恒等记录（照 `set_pivot([0,0])` 零向量先例：照存照发，消费端据此摘跨帧簿记）；稳态帧随快照重发 |
//! | T-NSX-04 | 仅裸 Control 生效：Button 即使带 ns 属性也不推（派生控件不面板纹理化，S16.6 冻结口径） |
//!
//! 簿记面（同键覆写 / 销毁清理 / 输出序）由 nes-render-api 的
//! criterion_contract 与 nes-render-wgpu 的 criterion_nineslice 两侧钉住。

use std::cell::RefCell;
use std::collections::BTreeMap;

use nes_render_api::{FrameInfo, NullRenderServer, RenderAssetKey, RenderCommand, Vec2};
use nes_render_extract::RenderExtractor;
use nes_render_extract::RenderKeySource;
use nes_scene::{NodeKind, ResId, SceneTree, Value};

/// 资源键替身（与 criterion_extract 同款：RefCell + BTreeMap 保确定性）。
#[derive(Default)]
struct KeyMap(RefCell<BTreeMap<ResId, RenderAssetKey>>);

impl KeyMap {
    fn bind(&self, id: ResId, key: RenderAssetKey) {
        self.0.borrow_mut().insert(id, key);
    }
}

impl RenderKeySource for KeyMap {
    fn render_key(&self, id: ResId) -> Option<RenderAssetKey> {
        self.0.borrow().get(&id).copied()
    }
}

fn frame() -> FrameInfo {
    FrameInfo::new(0, 1.0 / 60.0, 0.0, Vec2::new(640.0, 360.0))
}

/// 搭一个 Control（可选写 ns 属性），提取一帧返回（服务端、命令流、句柄）。
fn extract_control(fns: &[(&str, Value)]) -> (NullRenderServer, Vec<RenderCommand>, Option<nes_render_api::ItemHandle>, RenderExtractor) {
    let mut tree = SceneTree::new("root");
    let node = tree.add_node(tree.root(), "panel", NodeKind::Control);
    tree.set_prop(node, "offset", Value::Vec2(nes_scene::Vec2::new(8.0, 8.0)))
        .unwrap();
    tree.set_prop(node, "size", Value::Vec2(nes_scene::Vec2::new(96.0, 96.0)))
        .unwrap();
    for (name, value) in fns {
        tree.set_prop(node, name, value.clone())
            .unwrap_or_else(|err| panic!("写入 {name} 被 schema 拒绝：{err:?}"));
    }
    tree.apply_pending();

    let assets = KeyMap::default();
    assets.bind(ResId::new(1), RenderAssetKey::from_parts(9, 1));
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();
    ex.extract_into(&mut tree, &assets, &mut srv, &frame(), &mut out);
    let handle = ex.handle_of(node);
    (srv, out, handle, ex)
}

fn nine_of(out: &[RenderCommand]) -> Option<(RenderAssetKey, [f32; 4])> {
    out.iter().find_map(|c| match c {
        RenderCommand::SetNineSlice {
            handle: _,
            texture,
            l,
            t,
            r,
            b,
        } => Some((*texture, [*l, *t, *r, *b])),
        _ => None,
    })
}

/// T-NSX-01：五属性齐备（纹理键非空 + 至少一边距 > 0）→ SetNineSlice 到达，
/// 载荷逐位等于属性值；负边距照收（钳制权威在渲染侧）。
#[test]
fn t_nsx_01_valid_props_push_nine_slice() {
    let (srv, out, handle, _ex) = extract_control(&[
        ("ns_tex", Value::Resource(1)),
        ("ns_l", Value::I64(16)),
        ("ns_t", Value::I64(16)),
        ("ns_r", Value::I64(16)),
        ("ns_b", Value::I64(16)),
    ]);
    let h = handle.expect("Control 恒准入");
    assert_eq!(
        srv.nine_slice_of(h),
        Some(&(RenderAssetKey::from_parts(9, 1), [16.0, 16.0, 16.0, 16.0])),
        "服务端簿记收到键 + 边距"
    );
    assert_eq!(
        nine_of(&out),
        Some((RenderAssetKey::from_parts(9, 1), [16.0, 16.0, 16.0, 16.0])),
        "命令流携带 SetNineSlice"
    );

    // 部分边距 + 负值：> 0 判据看"至少一条"，负值照传（渲染侧钳制）。
    let (srv, out, _, _) = extract_control(&[
        ("ns_tex", Value::Resource(1)),
        ("ns_l", Value::I64(-4)),
        ("ns_t", Value::I64(8)),
    ]);
    assert_eq!(
        nine_of(&out),
        Some((RenderAssetKey::from_parts(9, 1), [-4.0, 8.0, 0.0, 0.0])),
    );
    assert!(srv.nine_slice_of(*srv.items().keys().next().unwrap()).is_some());
}

/// T-NSX-02：缺省 / 未绑定纹理 / 全零边距 → 不推 SetNineSlice。
#[test]
fn t_nsx_02_inactive_props_push_nothing() {
    // 缺省 Control：命令流里没有任何 SetNineSlice。
    let (_, out, _, _) = extract_control(&[]);
    assert!(
        out.iter().all(|c| !matches!(c, RenderCommand::SetNineSlice { .. })),
        "缺省路径命令流与既有逐条相同"
    );
    // 纹理未绑定（Resource(0)）：同上。
    let (_, out, _, _) = extract_control(&[
        ("ns_tex", Value::Resource(0)),
        ("ns_l", Value::I64(16)),
    ]);
    assert!(out.iter().all(|c| !matches!(c, RenderCommand::SetNineSlice { .. })));
    // 全零边距：同上（没有切割线 = 未启用）。
    let (_, out, _, _) = extract_control(&[
        ("ns_tex", Value::Resource(1)),
        ("ns_l", Value::I64(0)),
        ("ns_t", Value::I64(0)),
        ("ns_r", Value::I64(0)),
        ("ns_b", Value::I64(0)),
    ]);
    assert!(out.iter().all(|c| !matches!(c, RenderCommand::SetNineSlice { .. })));
}

/// T-NSX-03：有效 → 清空迁移帧补推一次 NIL **恒等记录**（照
/// `set_pivot([0,0])` 零向量先例：照存照发 —— 消费端收到后摘跨帧簿记，
/// 面板回 fill/border；清除必须可在命令流里承载，簿记跨帧的后端才能收
/// 到"清掉"）。稳态帧恒等记录随全量快照重发（补推只发生这一次）。
#[test]
fn t_nsx_03_migration_frame_pushes_nil_identity_once() {
    let mut tree = SceneTree::new("root");
    let node = tree.add_node(tree.root(), "panel", NodeKind::Control);
    tree.set_prop(node, "ns_tex", Value::Resource(1)).unwrap();
    tree.set_prop(node, "ns_l", Value::I64(16)).unwrap();
    tree.apply_pending();

    let assets = KeyMap::default();
    assets.bind(ResId::new(1), RenderAssetKey::from_parts(9, 1));
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();

    // 第 1 帧：有效 → 正常推送（簿记在案）。
    ex.extract_into(&mut tree, &assets, &mut srv, &frame(), &mut out);
    let h = ex.handle_of(node).expect("Control 恒准入");
    assert_eq!(
        srv.nine_slice_of(h),
        Some(&(RenderAssetKey::from_parts(9, 1), [16.0, 0.0, 0.0, 0.0])),
        "有效帧簿记在案"
    );

    // 第 2 帧：清空 ns_tex（回到 Resource(0)）→ 迁移帧补推 NIL 恒等记录。
    tree.set_prop(node, "ns_tex", Value::Resource(0)).unwrap();
    tree.apply_pending();
    out.clear();
    ex.extract_into(&mut tree, &assets, &mut srv, &frame(), &mut out);
    assert_eq!(
        nine_of(&out),
        Some((RenderAssetKey::NIL, [0.0, 0.0, 0.0, 0.0])),
        "迁移帧恰好一条 NIL 恒等记录（消费端的清除载体）"
    );

    // 第 3 帧：稳态（仍无九宫格）→ 恒等记录随全量快照重发（不另发新值）。
    out.clear();
    ex.extract_into(&mut tree, &assets, &mut srv, &frame(), &mut out);
    assert_eq!(
        nine_of(&out),
        Some((RenderAssetKey::NIL, [0.0, 0.0, 0.0, 0.0])),
        "恒等记录随快照重发"
    );
}

/// T-NSX-04：仅裸 Control 生效 —— Button 继承了 ns 属性键（schema 链），
/// 但提取层不读：带满配 ns 属性的 Button 不推 SetNineSlice。
#[test]
fn t_nsx_04_button_does_not_read_ns_props() {
    let mut tree = SceneTree::new("root");
    let node = tree.add_node(tree.root(), "ok", NodeKind::Button);
    tree.set_prop(node, "ns_tex", Value::Resource(1)).unwrap();
    tree.set_prop(node, "ns_l", Value::I64(16)).unwrap();
    tree.set_prop(node, "ns_t", Value::I64(16)).unwrap();
    tree.apply_pending();

    let assets = KeyMap::default();
    assets.bind(ResId::new(1), RenderAssetKey::from_parts(9, 1));
    let mut ex = RenderExtractor::new();
    let mut srv = NullRenderServer::new();
    let mut out = Vec::new();
    ex.extract_into(&mut tree, &assets, &mut srv, &frame(), &mut out);
    assert!(out.iter().all(|c| !matches!(c, RenderCommand::SetNineSlice { .. })));
    // 按钮自身照常摊平（SetRect + SetText 同句柄），ns 属性对它 inert。
    assert!(srv.rect_of(ex.handle_of(node).expect("按钮恒准入")).is_some());
}
