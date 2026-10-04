//! M4 / S1 契约冻结验收测试。
//!
//! 命名沿用 `nes-scene` / `nes-asset` 的既有惯例：每条测试都以 `criterion_` 开头，
//! 对应调研报告 S1 出口准则或本契约的一条不变式。测试**只依赖本 crate**
//! （零依赖契约层的直接体现：连测试都无法引用 `nes-scene`）。

use std::f32::consts::PI;
use std::sync::Arc;

use nes_render_api::*;

/// 浮点近似断言（精度 1e-4，与 f32 在本场景下的有效位匹配）。
fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn approx_vec(a: Vec2, b: Vec2) -> bool {
    approx(a.x, b.x) && approx(a.y, b.y)
}

// ------------------------------------------------------------ 句柄与身份

/// 不变式：空句柄 / 未绑定键的语义唯一且稳定（`NIL` 就是 0）。
#[test]
fn criterion_contract_nil_handle_and_key_are_zero() {
    assert_eq!(ItemHandle::NIL.raw(), 0);
    assert!(ItemHandle::NIL.is_nil());
    assert_eq!(ItemHandle::default(), ItemHandle::NIL);
    assert_eq!(RenderAssetKey::NIL.to_bits(), 0);
    assert!(RenderAssetKey::NIL.is_nil());
    assert_eq!(RenderAssetKey::default(), RenderAssetKey::NIL);
}

/// 不变式：句柄位编码是 `(slot, gen)`，与 M3 的 `RenderAssetKeyView` 同构且互逆。
#[test]
fn criterion_contract_handle_bits_roundtrip() {
    let handle = ItemHandle::from_parts(7, 3);
    assert_eq!(handle.slot(), 7);
    assert_eq!(handle.generation(), 3);
    assert_eq!(ItemHandle::from_raw(handle.raw()), handle);
    assert!(!handle.is_nil());

    let key = RenderAssetKey::from_parts(0xDEAD_BEEF, 5);
    assert_eq!(key.slot(), 0xDEAD_BEEF);
    assert_eq!(key.generation(), 5);
    assert_eq!(RenderAssetKey::from_bits(key.to_bits()), key);
}

/// 不变式：稳定身份 vs 易变句柄分离 —— 同一 `(slot, gen)` 编出的句柄相等，
/// 但槽位相同、代际不同的两个句柄**必须不等**（否则销毁后会撞上悬垂引用）。
#[test]
fn criterion_contract_slot_reuse_differs_by_generation() {
    let a = ItemHandle::from_parts(1, 1);
    let b = ItemHandle::from_parts(1, 2);
    assert_eq!(a.slot(), b.slot());
    assert_ne!(a, b);
    // 次序全序可用于兜底排序。
    assert!(a < b);
}

/// 不变式：`ItemHandle` / `RenderAssetKey` 可作映射键（`Copy + Ord + Hash`）。
#[test]
fn criterion_contract_handle_is_usable_as_map_key() {
    use std::collections::{BTreeMap, HashMap};

    let mut btree: BTreeMap<ItemHandle, u32> = BTreeMap::new();
    btree.insert(ItemHandle::from_parts(2, 0), 20);
    btree.insert(ItemHandle::from_parts(1, 0), 10);
    let keys: Vec<u32> = btree.values().copied().collect();
    assert_eq!(keys, vec![10, 20], "BTreeMap 必须按句柄值有序");

    let mut hash: HashMap<RenderAssetKey, &str> = HashMap::new();
    hash.insert(RenderAssetKey::from_parts(3, 1), "tex");
    assert_eq!(hash.get(&RenderAssetKey::from_parts(3, 1)), Some(&"tex"));
}

// ------------------------------------------------------------ 绘制次序

/// 不变式：绘制次序是 `z` → `order` → `handle` 的全序，**没有并列**。
#[test]
fn criterion_contract_draw_key_is_total_order() {
    let base = DrawKey {
        z: 0,
        order: 0,
        handle: 0,
    };
    let same = DrawKey { ..base };
    let bigger_order = DrawKey { order: 1, ..base };
    let bigger_z = DrawKey { z: 1, ..base };
    let same_pos_other_handle = DrawKey { handle: 1, ..base };

    assert_eq!(base.cmp(&same), std::cmp::Ordering::Equal);
    assert!(bigger_order > base, "同层内 order 升序");
    assert!(bigger_z > bigger_order, "z 优先于 order");
    assert!(
        same_pos_other_handle > base,
        "完全同位置时以句柄兜底，保证全序无并列"
    );
}

/// 出口准则（S2 预告）：同一份属性集合的绘制次序不依赖插入顺序。
#[test]
fn criterion_contract_draw_order_is_insertion_independent() {
    let mut forward = NullRenderServer::new();
    let mut backward = NullRenderServer::new();

    let handles_f: Vec<ItemHandle> = (0..3).map(|_| forward.create_item(RenderAssetKey::NIL)).collect();
    let handles_b: Vec<ItemHandle> = (0..3).map(|_| backward.create_item(RenderAssetKey::NIL)).collect();

    // 三者的 (z, order) 成对交换：正序 z=0,1,2；逆序 2,1,0。
    for (i, h) in handles_f.iter().enumerate() {
        forward.set_z(*h, i as i32, 0);
    }
    for (i, h) in handles_b.iter().enumerate() {
        backward.set_z(*h, (2 - i) as i32, 0);
    }

    let order_f = forward.draw_order();
    let order_b = backward.draw_order();

    // 正序：索引 0 的 z=0 最前；逆序：索引 2 的 z=0 最前。
    assert_eq!(order_f, vec![handles_f[0], handles_f[1], handles_f[2]]);
    assert_eq!(order_b, vec![handles_b[2], handles_b[1], handles_b[0]]);
}

// ------------------------------------------------------------ 翻转（缺口 1）

/// 缺口契约 1：翻转合成**不得改变世界变换的平移分量**。
#[test]
fn criterion_contract_flip_is_child_local_post_multiply() {
    let transform = Affine2::translation(10.0, 20.0);
    let flipped = Flip::new(true, false);
    let world = flipped.compose(transform);

    assert!(approx(world.tx, 10.0), "flip 不得搬走节点位置 (tx)");
    assert!(approx(world.ty, 20.0), "flip 不得搬走节点位置 (ty)");
    // 绕自身原点镜像：局部 (1, 0) 变成 (9, 20)。
    assert!(approx_vec(world.apply(Vec2::new(1.0, 0.0)), Vec2::new(9.0, 20.0)));
}

/// 缺口契约 1：四种翻转组合互不相同，且与缩放矩阵等价。
#[test]
fn criterion_contract_flip_four_combinations() {
    let identity = Flip::IDENTITY;
    let h = Flip::new(true, false);
    let v = Flip::new(false, true);
    let hv = Flip::new(true, true);

    assert!(identity.is_identity());
    assert!(!h.is_identity());
    assert!(h.any() && v.any() && hv.any());

    assert_eq!(h.to_affine(), Affine2::scale(-1.0, 1.0));
    assert_eq!(v.to_affine(), Affine2::scale(1.0, -1.0));
    assert_eq!(hv.to_affine(), Affine2::scale(-1.0, -1.0));

    // 两次同向翻转 = 恒等（幂等回原点）。
    let back = h.compose(h.to_affine());
    assert!(approx(back.a, 1.0) && approx(back.d, 1.0));
    assert!(approx(back.b, 0.0) && approx(back.c, 0.0));
}

/// 缺口契约 1：`RenderItem::world_transform` = `transform ∘ flip`（后乘）。
#[test]
fn criterion_contract_render_item_world_transform_includes_flip() {
    let item = RenderItem {
        flip: Flip::new(true, true),
        ..RenderItem::new(
            ItemHandle::from_raw(1),
            RenderAssetKey::NIL,
            Affine2::translation(5.0, 6.0),
        )
    };
    let world = item.world_transform();
    assert!(approx(world.tx, 5.0) && approx(world.ty, 6.0));
    assert!(approx(world.a, -1.0) && approx(world.d, -1.0));
    // 无翻转时与原始变换逐位相同。
    let plain = RenderItem::new(ItemHandle::from_raw(2), RenderAssetKey::NIL, Affine2::translation(5.0, 6.0));
    assert_eq!(plain.world_transform().to_array(), plain.transform.to_array());
}

// ------------------------------------------------------------ 相机（缺口 2）

/// 缺口契约 2：单位相机把世界原点映射到视口中心。
#[test]
fn criterion_contract_camera_identity_maps_origin_to_viewport_center() {
    let cam = Camera2DState::new(Vec2::new(800.0, 600.0));
    let view = cam.view_matrix().expect("启用的相机必须有视图矩阵");
    assert!(approx_vec(view.apply(Vec2::ZERO), Vec2::new(400.0, 300.0)));
    assert!(approx_vec(cam.center(), Vec2::ZERO));
    assert!(approx(cam.rotation(), 0.0));
}

/// 缺口契约 2：相机注视点始终落在视口中心（平移 + 缩放无关）。
#[test]
fn criterion_contract_camera_center_always_at_viewport_center() {
    let cam = Camera2DState {
        transform: Affine2::translation(0.0, -50.0),
        zoom: Vec2::new(2.0, 2.0),
        ..Camera2DState::new(Vec2::new(800.0, 600.0))
    };
    let view = cam.view_matrix().unwrap();
    assert!(approx_vec(view.apply(cam.center()), Vec2::new(400.0, 300.0)));

    // 缩放参与视图矩阵：世界 100 单位在 2x 下 = 视口 200 像素。
    let world_point = Vec2::new(cam.center().x + 100.0, cam.center().y);
    assert!(approx_vec(view.apply(world_point), Vec2::new(600.0, 300.0)));
}

/// 缺口契约 2：`offset` 在相机局部坐标系，先按相机旋转再叠加。
#[test]
fn criterion_contract_camera_offset_follows_rotation() {
    let cam = Camera2DState {
        transform: Affine2::rotation(PI / 2.0),
        offset: Vec2::new(10.0, 0.0),
        ..Camera2DState::new(Vec2::new(800.0, 600.0))
    };
    assert!(approx(cam.rotation(), PI / 2.0));
    // 相机旋转 90° 后，局部 (10,0) 指向世界 (0,10)。
    assert!(approx_vec(cam.center(), Vec2::new(0.0, 10.0)), "实际 {:?}", cam.center());
}

/// 缺口契约 2：`limits` 夹紧相机中心；可视区比限制还宽时取限制中心（不抖动）。
#[test]
fn criterion_contract_camera_limits_clamp_center() {
    let lim = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    let cam = Camera2DState {
        transform: Affine2::translation(5000.0, -5000.0),
        limits: Some(lim),
        ..Camera2DState::new(Vec2::new(200.0, 200.0))
    };
    // 可视半尺寸 100 → 中心可夹到 [100, 900]。
    assert!(approx_vec(cam.clamped_center(), Vec2::new(900.0, 100.0)));
    let rect = cam.visible_world_rect();
    assert!(approx(rect.w, 200.0) && approx(rect.h, 200.0));
    assert!(lim.contains(rect.min()) && lim.contains(rect.max()), "可视矩形必须落在限制内");

    // 退化：限制比可视区还小 → 取限制中心，且不触发 NaN。
    let narrow = Camera2DState {
        limits: Some(Rect::new(0.0, 0.0, 50.0, 50.0)),
        transform: Affine2::translation(999.0, 999.0),
        ..Camera2DState::new(Vec2::new(200.0, 200.0))
    };
    assert!(approx_vec(narrow.clamped_center(), Vec2::new(25.0, 25.0)));
}

/// 缺口契约 2：禁用相机返回 `None`；非法缩放按 1 处理（不得把场景压成一点）。
#[test]
fn criterion_contract_camera_disabled_and_zoom_normalization() {
    let disabled = Camera2DState {
        enabled: false,
        ..Camera2DState::new(Vec2::new(800.0, 600.0))
    };
    assert!(disabled.view_matrix().is_none());

    let bad_zoom = Camera2DState {
        zoom: Vec2::new(0.0, -2.0),
        ..Camera2DState::new(Vec2::new(800.0, 600.0))
    };
    assert_eq!(bad_zoom.effective_zoom(), Vec2::ONE);
    let view = bad_zoom.view_matrix().unwrap();
    assert!(view.apply(Vec2::ZERO).x.is_finite());
}

/// 缺口契约 2：旋转相机的可视半尺寸按 AABB 扩张（45° 时边长 √2 倍）。
#[test]
fn criterion_contract_camera_visible_extents_under_rotation() {
    let cam = Camera2DState {
        transform: Affine2::rotation(PI / 4.0),
        ..Camera2DState::new(Vec2::new(200.0, 200.0))
    };
    let half = cam.visible_half_extents();
    let expected = 100.0 * (2.0f32).sqrt();
    assert!(approx(half.x, expected) && approx(half.y, expected));
}

// ------------------------------------------------------------ Label（缺口 3）

/// 缺口契约 3：文本用 `Arc<str>` 共享，克隆不复制字节（每帧零分配的前提）。
#[test]
fn criterion_contract_label_text_is_shared_not_copied() {
    let label = LabelState::new("hello nes", 16.0);
    let cloned = label.clone();
    assert!(
        Arc::ptr_eq(&label.text, &cloned.text),
        "克隆 LabelState 必须共享同一份文本，而不是复制字节"
    );
    assert_eq!(cloned, label);

    let other = LabelState::new("hello nes", 16.0);
    assert_eq!(other, label);
    assert!(!Arc::ptr_eq(&other.text, &label.text), "独立构造是两份数据，但内容相等");
}

/// 缺口契约 3：默认参数明确（默认字体、无额外行距、左上对齐、不换行）。
#[test]
fn criterion_contract_label_defaults_are_explicit() {
    let label = LabelState::new(String::from("x"), 12.0);
    assert_eq!(label.font, RenderAssetKey::NIL);
    assert_eq!(label.font_size, 12.0);
    assert_eq!(label.line_spacing, 0.0);
    assert_eq!(label.align_h, HAlign::Left);
    assert_eq!(label.align_v, VAlign::Top);
    assert_eq!(label.wrap_width, None);
    assert_eq!(HAlign::default(), HAlign::Left);
    assert_eq!(VAlign::default(), VAlign::Top);
}

// ------------------------------------------------------------ Control（缺口 4）

/// 缺口契约 4：锚点布局算式 = `anchor * parent + offset`（逐边独立）。
#[test]
fn criterion_contract_control_resolve_anchor_formula() {
    let pins = ControlState::new([0.0, 0.0, 0.0, 0.0], [10.0, 20.0, 110.0, 70.0]);
    let rect = pins.resolve(Vec2::new(800.0, 600.0));
    // 四锚点为 0：矩形与父尺寸无关（左上角固定尺寸）。
    assert!(approx(rect.x, 10.0) && approx(rect.y, 20.0));
    assert!(approx(rect.w, 100.0) && approx(rect.h, 50.0));
    let rect_other_parent = pins.resolve(Vec2::new(200.0, 100.0));
    assert_eq!(rect, rect_other_parent, "全 0 锚点必须与父尺寸无关");

    // 全 1 锚点：贴着父右下角。
    let right_bottom = ControlState::new([1.0, 1.0, 1.0, 1.0], [-10.0, -20.0, 0.0, 0.0]);
    let rect = right_bottom.resolve(Vec2::new(800.0, 600.0));
    assert!(approx(rect.x, 790.0) && approx(rect.y, 580.0));
    assert!(approx(rect.w, 10.0) && approx(rect.h, 20.0));
}

/// 缺口契约 4：`FULL_RECT` 铺满父容器；跨轴锚点（0 → 1）随父尺寸伸缩。
#[test]
fn criterion_contract_control_full_rect_and_stretch() {
    let full = ControlState::FULL_RECT;
    for parent in [Vec2::new(800.0, 600.0), Vec2::new(100.0, 50.0)] {
        let rect = full.resolve(parent);
        assert!(approx(rect.x, 0.0) && approx(rect.y, 0.0));
        assert!(approx(rect.w, parent.x) && approx(rect.h, parent.y));
    }

    // 水平拉伸（0→1）+ 固定 10px 内边距；垂直方向固定 30px 高。
    let bar = ControlState::new([0.0, 0.0, 1.0, 0.0], [10.0, 0.0, -10.0, 30.0]);
    let rect = bar.resolve(Vec2::new(800.0, 600.0));
    assert!(approx(rect.x, 10.0) && approx(rect.w, 780.0));
    assert!(approx(rect.h, 30.0));
}

/// 缺口契约 4：显式 `Some(min_size)` 只把右下边推出去，**不移动左上角**（避免布局抖动）。
#[test]
fn criterion_contract_control_min_size_grows_bottom_right_only() {
    let ctrl = ControlState {
        min_size: Some(Vec2::new(120.0, 90.0)),
        ..ControlState::new([0.0, 0.0, 0.0, 0.0], [10.0, 20.0, 60.0, 55.0])
    };
    let rect = ctrl.resolve(Vec2::new(800.0, 600.0));
    assert!(approx(rect.x, 10.0) && approx(rect.y, 20.0), "左上角不得被 min_size 搬动");
    assert!(approx(rect.w, 120.0) && approx(rect.h, 90.0));
}

/// 缺口契约 4（v1.1 修订，S3 遗留缺陷 D-S3-1 / Q-S3-1 裁决 A）：
/// `min_size` 缺省为 `None` = **无下界**，负宽高必须**原样透传**；
/// 只有显式 `Some(min)` 才扩张右下边，且任何取值下都**不存在**把负值钳到 0 的分支。
#[test]
fn criterion_contract_control_no_lower_bound_keeps_negative_size() {
    // 缺省即无下界：`new()` 与 `FULL_RECT` 都不得是"零下界"。
    let bare = ControlState::new([0.0, 0.0, 0.0, 0.0], [5.0, 5.0, -25.0, -35.0]);
    assert_eq!(bare.min_size, None, "缺省必须是 None（无下界），不得退化成 ZERO 下界");
    assert_eq!(ControlState::FULL_RECT.min_size, None);

    // 无下界：负宽高原样保留，左上角不受影响。
    let rect = bare.resolve(Vec2::new(400.0, 200.0));
    assert!(approx(rect.x, 5.0) && approx(rect.y, 5.0));
    assert!(
        approx(rect.w, -30.0) && approx(rect.h, -40.0),
        "无下界时负宽高必须原样透传（不得压成 0）"
    );

    // 单边为负（正宽负高）同样不钳制：证明没有"负值专用钳制分支"。
    let mixed = ControlState::new([0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 50.0, -20.0]);
    let m = mixed.resolve(Vec2::new(400.0, 200.0));
    assert!(approx(m.w, 50.0) && approx(m.h, -20.0), "只钳制负值的分支不存在");

    // 归因对照：即使显式给出**负下界**，出口也逐位等于算式结果。
    let with_negative_floor = ControlState {
        min_size: Some(Vec2::new(-100.0, -100.0)),
        ..bare
    };
    let r = with_negative_floor.resolve(Vec2::new(400.0, 200.0));
    assert!(approx(r.w, -30.0) && approx(r.h, -40.0));

    // 对照：显式 `Some(正下界)` 仍只推右下边（既有用例语义不变）。
    let with_floor = ControlState {
        min_size: Some(Vec2::new(120.0, 90.0)),
        ..bare
    };
    let f = with_floor.resolve(Vec2::new(400.0, 200.0));
    assert!(approx(f.x, 5.0) && approx(f.y, 5.0), "左上角不得被 min_size 搬动");
    assert!(approx(f.w, 120.0) && approx(f.h, 90.0));
}

// ------------------------------------------------------------ 服务端不变式

/// 不变式 1：空句柄 / 未知句柄的操作被**静默忽略**且可计数，不得 panic。
#[test]
fn criterion_contract_server_ignores_nil_and_unknown_handles() {
    let mut server = NullRenderServer::new();
    let key = RenderAssetKey::from_parts(1, 1);
    let live = server.create_item(key);
    let dead = server.create_item(key);
    server.destroy_item(dead);

    server.set_visible(ItemHandle::NIL, false);
    server.set_transform(ItemHandle::NIL, Affine2::IDENTITY);
    server.set_z(ItemHandle::NIL, 1, 1);
    server.set_flip(ItemHandle::NIL, Flip::IDENTITY);
    server.set_text(ItemHandle::NIL, &LabelState::new("x", 10.0));
    server.set_rect(ItemHandle::NIL, &ControlState::FULL_RECT);
    server.destroy_item(ItemHandle::NIL);

    server.set_visible(dead, false);
    server.set_text(dead, &LabelState::new("x", 10.0));
    server.set_rect(dead, &ControlState::FULL_RECT);

    assert_eq!(server.counters().ignored_ops, 10, "每次无效操作都必须被记账");
    assert_eq!(server.len(), 1);
    assert!(server.item(live).is_some());
    assert!(server.item(dead).is_none());
}

/// 不变式 2：句柄**不复用** —— 销毁后再创建必须拿到新句柄。
#[test]
fn criterion_contract_server_never_reuses_handles() {
    let mut server = NullRenderServer::new();
    let key = RenderAssetKey::from_parts(2, 1);
    let first = server.create_item(key);
    server.destroy_item(first);
    let second = server.create_item(key);

    assert_ne!(first, second, "销毁过的句柄不得重新分配");
    assert!(server.item(first).is_none());
    assert!(server.item(second).is_some());
    assert_eq!(server.counters().created, 2);
    assert_eq!(server.counters().destroyed, 1);
}

/// 不变式 3 + 顺序：命令流 = 生命周期 → SetCamera → 各渲染物属性 → Submit。
#[test]
fn criterion_contract_submit_command_layout_is_frozen() {
    let mut server = NullRenderServer::new();
    let key_a = RenderAssetKey::from_parts(1, 1);
    let key_b = RenderAssetKey::from_parts(2, 1);
    let a = server.create_item(key_a);
    let b = server.create_item(key_b);
    let camera = Camera2DState::new(Vec2::new(320.0, 240.0));
    server.set_camera(&camera);
    server.set_z(a, 1, 0);
    server.set_z(b, 0, 0);
    server.set_text(a, &LabelState::new("hi", 14.0));
    server.set_rect(b, &ControlState::FULL_RECT);

    let frame = FrameInfo::new(1, 1.0 / 60.0, 0.016, Vec2::new(320.0, 240.0));
    let commands = server.submit(&frame);

    // 生命周期 2 条 + 相机 1 条 + 每物 4 条基础属性 + Label/Control 各 1 条 + Submit 1 条。
    assert_eq!(commands.len(), 2 + 1 + 4 * 2 + 2 + 1);
    assert!(matches!(commands[0], RenderCommand::CreateItem { handle, .. } if handle == a));
    assert!(matches!(commands[1], RenderCommand::CreateItem { handle, .. } if handle == b));
    assert!(matches!(commands[2], RenderCommand::SetCamera { .. }));

    // 属性流按绘制次序：b（z=0）先于 a（z=1）；每物基础四属性顺序固定。
    assert!(matches!(commands[3], RenderCommand::SetTransform { handle, .. } if handle == b));
    assert!(matches!(commands[6], RenderCommand::SetVisible { handle, visible: true } if handle == b));
    assert!(matches!(commands[7], RenderCommand::SetRect { handle, .. } if handle == b));
    assert!(matches!(commands[8], RenderCommand::SetTransform { handle, .. } if handle == a));
    assert!(matches!(commands[12], RenderCommand::SetText { handle, .. } if handle == a));
    assert!(matches!(commands.last().unwrap(), RenderCommand::Submit { .. }));
    assert_eq!(commands.iter().filter(|c| c.is_lifecycle()).count(), 2);
    assert_eq!(commands[3].handle(), Some(b));
    assert_eq!(commands[2].handle(), None);
}

/// 不变式 3：`submit_into` **先清空**缓冲，可跨帧复用同一个 `Vec`。
#[test]
fn criterion_contract_submit_into_clears_reused_buffer() {
    let mut server = NullRenderServer::new();
    let handle = server.create_item(RenderAssetKey::NIL);
    server.set_visible(handle, true);
    let frame = FrameInfo::new(0, 0.0, 0.0, Vec2::new(64.0, 64.0));

    let mut buffer: Vec<RenderCommand> = vec![
        RenderCommand::DestroyItem { handle },
        RenderCommand::DestroyItem { handle },
        RenderCommand::DestroyItem { handle },
    ];
    server.submit_into(&frame, &mut buffer);

    assert_eq!(
        buffer.len(),
        6,
        "1 Create + 4 属性 + 1 Submit = 6（3 条旧 DestroyItem 必须被清空）"
    );
    assert!(matches!(buffer[0], RenderCommand::CreateItem { .. }));
    assert_eq!(
        buffer.iter().filter(|c| c.is_lifecycle() && matches!(c, RenderCommand::DestroyItem { .. })).count(),
        0,
        "上一帧残留的 DestroyItem 不得留在缓冲里"
    );
}

/// 不变式 4（帧命令二分语义）：`submit` 输出 = **一次性事件** + **每帧全量快照**。
///
/// - 事件（`CreateItem` / `DestroyItem`）只在发生的那一帧出现一次，不重放；
/// - 快照（相机 + 各渲染物属性 + `Submit`）在状态不变时逐帧逐条重现 —— 这才是
///   后端可以做无状态消费的依据。
#[test]
fn criterion_contract_submit_is_deterministic() {
    let mut server = NullRenderServer::new();
    let key = RenderAssetKey::from_parts(9, 4);
    let a = server.create_item(key);
    let b = server.create_item(key);
    let c = server.create_item(key);
    server.set_camera(&Camera2DState::new(Vec2::new(1280.0, 720.0)));
    server.set_transform(a, Affine2::translation(1.0, 2.0));
    server.set_transform(b, Affine2::rotation(0.5));
    server.set_z(c, -1, 7);
    server.set_flip(b, Flip::new(false, true));
    server.set_text(c, &LabelState::new("determinism", 20.0));
    let frame = FrameInfo::new(42, 0.016, 0.7, Vec2::new(1280.0, 720.0));

    let first = server.submit(&frame);
    let second = server.submit(&frame);

    // 一次性事件：仅第一帧出现。
    assert_eq!(first.iter().filter(|c| c.is_lifecycle()).count(), 3);
    assert_eq!(
        second.iter().filter(|c| c.is_lifecycle()).count(),
        0,
        "Create/Destroy 是事件，不得逐帧重放"
    );

    // 全量快照：状态不变则逐条相同。
    let snapshot = |cmds: Vec<RenderCommand>| -> Vec<RenderCommand> {
        cmds.into_iter().filter(|c| !c.is_lifecycle()).collect()
    };
    let first_snapshot = snapshot(first);
    let second_snapshot = snapshot(second);
    assert_eq!(first_snapshot, second_snapshot, "快照部分必须逐条可重现");
    assert!(matches!(first_snapshot[0], RenderCommand::SetCamera { .. }));
    assert!(matches!(second_snapshot.last().unwrap(), RenderCommand::Submit { .. }));
    assert_eq!(server.counters().frames, 2);
}

/// 不变式 5：属性流是**全量快照** —— 改一次属性，之后每帧都完整重现。
#[test]
fn criterion_contract_submit_is_full_snapshot_not_delta() {
    let mut server = NullRenderServer::new();
    let handle = server.create_item(RenderAssetKey::from_parts(1, 1));
    server.set_transform(handle, Affine2::translation(3.0, 4.0));
    let frame = FrameInfo::new(0, 0.0, 0.0, Vec2::ZERO);

    let first = server.submit(&frame);
    let second = server.submit(&frame);

    let attr_count = |cmds: &[RenderCommand]| {
        cmds.iter()
            .filter(|c| matches!(c, RenderCommand::SetTransform { .. } | RenderCommand::SetFlip { .. }
                | RenderCommand::SetZ { .. } | RenderCommand::SetVisible { .. }))
            .count()
    };
    assert_eq!(attr_count(&first), 4, "首帧即输出四条基础属性，不等 diff");
    assert_eq!(attr_count(&second), 4, "无变化也必须完整重现，后端无需维护跨帧 diff");
}

/// 不变式：`destroy_item` 即时入队，下次 `submit` 落到缓冲（Create/Destroy 不丢）。
#[test]
fn criterion_contract_destroy_is_enqueued_until_next_submit() {
    let mut server = NullRenderServer::new();
    let handle = server.create_item(RenderAssetKey::NIL);
    server.destroy_item(handle);
    assert!(server.is_empty());

    let frame = FrameInfo::default();
    let commands = server.submit(&frame);
    assert_eq!(commands.len(), 3, "Create + Destroy + Submit");
    assert!(matches!(commands[0], RenderCommand::CreateItem { .. }));
    assert!(matches!(commands[1], RenderCommand::DestroyItem { .. }));
    assert!(matches!(commands[2], RenderCommand::Submit { .. }));

    // 生命周期动作不重复投递。
    let again = server.submit(&frame);
    assert_eq!(again.len(), 1, "Create/Destroy 只投递一次，之后仅剩 Submit");
    assert!(matches!(again[0], RenderCommand::Submit { .. }));
}

/// 不变式：`apply_item` 整块推送与逐属性推送在契约上等价。
#[test]
fn criterion_contract_apply_item_matches_individual_setters() {
    let key = RenderAssetKey::from_parts(5, 2);
    let item = RenderItem {
        z: -3,
        order: 8,
        visible: false,
        flip: Flip::new(true, true),
        ..RenderItem::new(ItemHandle::from_raw(1), key, Affine2::translation(7.0, 8.0))
    };
    let frame = FrameInfo::default();

    let mut bulk = NullRenderServer::new();
    let bulk_handle = bulk.create_item(key);
    bulk.apply_item(&RenderItem {
        handle: bulk_handle,
        ..item
    });
    let bulk_commands = bulk.submit(&frame);

    let mut granular = NullRenderServer::new();
    let h = granular.create_item(key);
    granular.set_transform(h, item.transform);
    granular.set_flip(h, item.flip);
    granular.set_z(h, item.z, item.order);
    granular.set_visible(h, item.visible);
    let granular_commands = granular.submit(&frame);

    assert_eq!(bulk_commands, granular_commands);
    assert_eq!(bulk.item(bulk_handle).unwrap().draw_key(), item.draw_key());
}

/// 不变式：服务端保存的是相机**最后推送值**（含禁用状态），由后端决定是否应用。
#[test]
fn criterion_contract_camera_last_write_wins_including_disabled() {
    let mut server = NullRenderServer::new();
    let enabled = Camera2DState::new(Vec2::new(100.0, 100.0));
    server.set_camera(&enabled);
    assert_eq!(server.camera(), Some(&enabled));

    let disabled = Camera2DState {
        enabled: false,
        ..enabled
    };
    server.set_camera(&disabled);
    assert_eq!(server.camera(), Some(&disabled));
    let commands = server.submit(&FrameInfo::default());
    assert!(matches!(commands[0], RenderCommand::SetCamera { camera } if !camera.enabled));
}

/// 出口准则（对象安全）：`&mut dyn RenderServer` 可用，且能跨三种方向调用。
#[test]
fn criterion_contract_server_is_object_safe_and_usable_as_dyn() {
    fn drive(server: &mut dyn RenderServer) -> Vec<RenderCommand> {
        let key = RenderAssetKey::from_parts(4, 1);
        let handle = server.create_item(key);
        server.set_transform(handle, Affine2::scale(2.0, 3.0));
        server.set_flip(handle, Flip::new(false, true));
        server.set_camera(&Camera2DState::new(Vec2::new(64.0, 64.0)));
        let mut buffer = Vec::new();
        server.submit_into(&FrameInfo::default(), &mut buffer);
        buffer
    }

    let mut server = NullRenderServer::new();
    let commands = drive(&mut server);
    assert!(matches!(commands[0], RenderCommand::CreateItem { .. }));
    assert_eq!(server.len(), 1);
}

/// 边界：无渲染物、无相机的空帧也必须产出合法的单条 `Submit` 尾命令。
#[test]
fn criterion_contract_empty_frame_still_terminates_with_submit() {
    let mut server = NullRenderServer::new();
    let commands = server.submit(&FrameInfo::new(0, 0.0, 0.0, Vec2::ZERO));
    assert_eq!(commands.len(), 1);
    assert!(matches!(commands[0], RenderCommand::Submit { frame } if frame.frame_index == 0));
    assert_eq!(server.counters().frames, 1);
}

/// S16.2（图集帧动画）：`set_uv` 的 null 簿记 —— 同键覆写、未知句柄静默
/// 忽略、销毁随条目清理、输出序恒在 `SetTint` 之后（与 wgpu 后端严格同序）。
#[test]
fn criterion_contract_set_uv_bookkeeping_and_order() {
    let mut server = NullRenderServer::new();
    let handle = server.create_item(RenderAssetKey::from_parts(7, 1));

    // 同键覆写：后写者生效；未知句柄静默忽略（计数器可观测）。
    server.set_uv(handle, [0.0, 0.0, 0.5, 0.5]);
    server.set_uv(handle, [0.5, 0.5, 0.5, 0.5]);
    server.set_uv(ItemHandle::from_raw(999), [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(server.uv_of(handle), Some(&[0.5, 0.5, 0.5, 0.5]));
    assert!(server.uv_of(ItemHandle::from_raw(999)).is_none());

    // 输出序：SetUv 恒在 SetTint 之后、Submit 之前（每渲染物属性流序）。
    server.set_tint(handle, [255, 255, 255, 255]);
    let commands = server.submit(&FrameInfo::default());
    let tint_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetTint { .. }));
    let uv_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetUv { .. }));
    assert!(tint_at.is_some() && uv_at.is_some());
    assert!(uv_at > tint_at, "SetUv 恒在 SetTint 之后");

    // 销毁：uv 随条目消亡（命令流里不再出现）。
    server.destroy_item(handle);
    let commands = server.submit(&FrameInfo::default());
    assert!(
        commands.iter().all(|c| !matches!(c, RenderCommand::SetUv { .. })),
        "销毁后命令流不再出现 SetUv"
    );
    assert!(server.uv_of(handle).is_none());
}

/// S16.3（精灵锚点）：`set_pivot` 的 null 簿记 —— 同键覆写、未知句柄静默
/// 忽略、销毁随条目清理、输出序恒在 `SetUv` 之后（与 wgpu 后端严格同序；
/// 照 SetTint/SetUv 先例三同构的契约侧锚点）。
#[test]
fn criterion_contract_set_pivot_bookkeeping_and_order() {
    let mut server = NullRenderServer::new();
    let handle = server.create_item(RenderAssetKey::from_parts(7, 1));

    // 同键覆写：后写者生效；未知句柄静默忽略（计数器可观测）。
    server.set_pivot(handle, [0.5, 0.5]);
    server.set_pivot(handle, [0.0, 1.0]);
    server.set_pivot(ItemHandle::from_raw(999), [1.0, 1.0]);
    assert_eq!(server.pivot_of(handle), Some(&[0.0, 1.0]));
    assert!(server.pivot_of(ItemHandle::from_raw(999)).is_none());
    let ignored_before = server.counters().ignored_ops;
    assert_eq!(ignored_before, 1, "未知句柄恰好被计一次忽略");

    // 无簿记时不产生命令：先在另一条目上验证"无记录 = 无平移"的命令流面。
    let commands = server.submit(&FrameInfo::default());
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, RenderCommand::SetPivot { .. })),
        "有 pivot 簿记的条目按快照重发 SetPivot"
    );

    // 输出序：SetPivot 恒在 SetUv 之后、SetUv 恒在 SetTint 之后、Submit 收尾
    //（每渲染物属性流序的冻结口径）。
    server.set_tint(handle, [255, 255, 255, 255]);
    server.set_uv(handle, [0.0, 0.0, 1.0, 1.0]);
    let commands = server.submit(&FrameInfo::default());
    let tint_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetTint { .. }));
    let uv_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetUv { .. }));
    let pivot_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetPivot { .. }));
    assert!(tint_at.is_some() && uv_at.is_some() && pivot_at.is_some());
    assert!(uv_at > tint_at, "SetUv 恒在 SetTint 之后");
    assert!(pivot_at > uv_at, "SetPivot 恒在 SetUv 之后");

    // 销毁：pivot 随条目消亡（命令流里不再出现）。
    server.destroy_item(handle);
    let commands = server.submit(&FrameInfo::default());
    assert!(
        commands
            .iter()
            .all(|c| !matches!(c, RenderCommand::SetPivot { .. })),
        "销毁后命令流不再出现 SetPivot"
    );
    assert!(server.pivot_of(handle).is_none());
}

/// S16.6（九宫格）：`set_nine_slice` 的 null 簿记 —— 同键覆写、NIL 恒等
/// 记录照存照发、未知句柄静默忽略、销毁随条目清理；输出序 SetNineSlice
/// 恒在 SetPivot 之后（与 wgpu 后端严格同构的命令流面抽查）。S16.7：
/// modulate / tiling 两开关随载荷同行（覆写后写者生效，快照逐帧重发）。
#[test]
fn criterion_contract_set_nine_slice_bookkeeping_and_order() {
    let mut server = NullRenderServer::new();
    let handle = server.create_item(RenderAssetKey::from_parts(7, 1));
    let tex = RenderAssetKey::from_parts(9, 1);

    // 同键覆写：后写者生效；未知句柄静默忽略（计数器可观测）。
    server.set_nine_slice(handle, tex, 16.0, 16.0, 16.0, 16.0, false, false);
    server.set_nine_slice(handle, tex, 8.0, 0.0, 12.0, 0.0, true, true);
    server.set_nine_slice(ItemHandle::from_raw(999), tex, 1.0, 1.0, 1.0, 1.0, true, false);
    assert_eq!(
        server.nine_slice_of(handle),
        Some(&NineSliceState {
            texture: tex,
            margins: [8.0, 0.0, 12.0, 0.0],
            modulate: true,
            tiling: true,
        }),
        "覆写后写者生效（边距四元组 + 两开关逐位）"
    );
    assert!(server.nine_slice_of(ItemHandle::from_raw(999)).is_none());
    assert_eq!(server.counters().ignored_ops, 1, "未知句柄恰好被计一次忽略");

    // 输出序：SetNineSlice 恒在 SetPivot 之后（S16.6 冻结的链尾位置）。
    server.set_pivot(handle, [0.5, 0.5]);
    let commands = server.submit(&FrameInfo::default());
    let pivot_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetPivot { .. }));
    let nine_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetNineSlice { .. }));
    assert!(pivot_at.is_some() && nine_at.is_some(), "两命令都在流里");
    assert!(nine_at > pivot_at, "SetNineSlice 恒在 SetPivot 之后");
    assert!(
        commands
            .iter()
            .all(|c| c.handle() != Some(ItemHandle::from_raw(999))),
        "未知句柄不产生命令"
    );
    // 载荷面：两开关随全量快照重发（逐帧义务）。
    assert!(matches!(
        commands
            .iter()
            .find(|c| matches!(c, RenderCommand::SetNineSlice { .. })),
        Some(RenderCommand::SetNineSlice {
            modulate: true,
            tiling: true,
            ..
        })
    ));

    // NIL 键 = 恒等记录（照 set_pivot([0,0]) 零向量先例）：照存照发 ——
    // 消费端据此清除跨帧簿记，fill/border 照旧。
    server.set_nine_slice(handle, RenderAssetKey::NIL, 0.0, 0.0, 0.0, 0.0, false, false);
    assert_eq!(
        server.nine_slice_of(handle),
        Some(&NineSliceState::IDENTITY),
        "NIL 恒等记录照存（清除必须可在命令流里承载）"
    );
    let commands = server.submit(&FrameInfo::default());
    let nil_clears = commands
        .iter()
        .filter(|c| matches!(c, RenderCommand::SetNineSlice { texture, .. } if texture.is_nil()))
        .count();
    assert_eq!(nil_clears, 1, "恒等记录随快照重发（消费端的清除载体）");

    // 销毁：九宫格随条目消亡（恒等记录一并消失）。
    server.set_nine_slice(handle, tex, 16.0, 16.0, 16.0, 16.0, false, false);
    server.destroy_item(handle);
    assert!(server.nine_slice_of(handle).is_none(), "销毁随条目清理");
    let commands = server.submit(&FrameInfo::default());
    assert!(
        commands
            .iter()
            .all(|c| !matches!(c, RenderCommand::SetNineSlice { .. })),
        "销毁后命令流不再出现 SetNineSlice"
    );
}
