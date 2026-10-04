//! S16.5 命中测算与 pivot 联动（T-HP-01..04）：hit 盒跟随锚点。
//!
//! 背景：S16.3 起精灵渲染经 pivot 平移（`world ∘ translate(-pivot * 16px)`，
//! 局部内层平移），而 `Op::Hit` 的命中盒仍锚在 `world.tx/ty` —— 渲染与
//! 命中分叉：pivot (0.5,0.5) 的精灵画在中心锚定位，点它中心却 miss。
//!
//! 修复：命中盒原点改为「世界矩阵映射 pivot 平移后的锚点角」
//! `origin = world.apply((-pivot.x * 16, -pivot.y * 16))`，几何单点在
//! [`nes_scene::SceneTree::sprite_hit_origin`]（脚本 hit 与编辑器宿主
//! 点击/框选共用）；pivot 缺省/类型错/非有限 = (0,0)，无 pivot 精灵与
//! 旧口径逐位相同。旋转下轴对齐盒是既有契约的近似（口径不变）。
//!
//! 本文件字符串字面量全 ASCII（脚本源 / 断言消息）；注释中文。

use nes_scene::{NodeHandle, NodeKind, SceneTree, ScriptVm, Transform2D, Value, Vec2};

/// 建一棵最小树：root 下一个 Sprite2D "sp" 在世界 (32,32)，可选写 pivot
/// 属性（`pivot` = Some((px,py)) 时经 schema 正常属性路径写入）；再挂
/// 一个 Script 节点跑 `src`，发 "go" 信号一帧，返回局部表。
fn run_hit_script(pivot: Option<(f32, f32)>, src: String) -> std::collections::BTreeMap<String, Value> {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let sp = t.add_node(t.root(), "sp", NodeKind::Sprite2D);
    t.set_local(sp, Transform2D::from_pos(32.0, 32.0));
    if let Some((px, py)) = pivot {
        t.set_prop(sp, "pivot", Value::Vec2(Vec2::new(px, py))).unwrap();
    }
    t.apply_pending();
    t.refresh_transforms();

    let sc = t.add_node(t.root(), "hp", NodeKind::Script);
    t.apply_pending();
    t.set_prop(sc, "source", Value::Str(src)).unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    vm.locals(sc).unwrap().clone()
}

/// T-HP-01（现状回归）：无 pivot（缺省）精灵 (32,32) —— 命中盒 = 旧口径
/// [32,48) x [32,48)：hit(33,33) 命中（含节点身份比对），hit(31,33) miss。
#[test]
fn t_hp_01_no_pivot_default_box() {
    // 助手级：pivot 缺省 -> 盒原点 == (tx, ty) 逐位不变（基线哈希前提）。
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let sp = t.add_node(t.root(), "sp", NodeKind::Sprite2D);
    t.set_local(sp, Transform2D::from_pos(32.0, 32.0));
    t.apply_pending();
    t.refresh_transforms();
    assert_eq!(t.sprite_hit_origin(sp), Vec2::new(32.0, 32.0));

    // VM 级：脚本 hit 同几何 + 命中身份 == 该精灵。
    let sc = t.add_node(t.root(), "hp", NodeKind::Script);
    t.apply_pending();
    t.set_prop(
        sc,
        "source",
        Value::Str("on \"go\" { h1 = hit(33.0, 33.0)\n  h2 = hit(31.0, 33.0) }".to_string()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let l = vm.locals(sc).unwrap();
    assert_eq!(
        l.get("h1"),
        Some(&Value::Node(NodeHandle::of(sp))),
        "hit(33,33) must return the sprite itself"
    );
    assert_eq!(l.get("h2"), Some(&Value::Bool(false)), "hit(31,33) left of box must miss");
}

/// T-HP-02：pivot (0.5,0.5) 同精灵 —— 渲染四边形平移到 [24,40) x [24,40)，
/// 命中盒跟随：hit(36,36) 命中；hit(40,40) miss（半开边界；旧口径此点
/// 命中 —— 分叉修复的证据）；hit(25,25) 命中（旧口径 miss 的区域 ——
/// 「点所见即所得」的直接证据）；视觉中心附近 hit(33,33) 命中
///（S16.3 分叉 bug 场景：点画出来的中心必须中）。
#[test]
fn t_hp_02_pivot_half_center_box() {
    // 助手级：origin = apply((-8,-8)) = (24,24)。
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let sp = t.add_node(t.root(), "sp", NodeKind::Sprite2D);
    t.set_local(sp, Transform2D::from_pos(32.0, 32.0));
    t.set_prop(sp, "pivot", Value::Vec2(Vec2::new(0.5, 0.5))).unwrap();
    t.apply_pending();
    t.refresh_transforms();
    assert_eq!(t.sprite_hit_origin(sp), Vec2::new(24.0, 24.0));

    let l = run_hit_script(
        Some((0.5, 0.5)),
        "on \"go\" { a = hit(36.0, 36.0)\n  b = hit(40.0, 40.0)\n  c = hit(25.0, 25.0)\n  d = hit(33.0, 33.0) }"
            .to_string(),
    );
    assert!(matches!(l.get("a"), Some(Value::Node(_))), "hit(36,36) inside shifted box");
    assert_eq!(l.get("b"), Some(&Value::Bool(false)), "hit(40,40) is half-open end (old box hit here)");
    assert!(matches!(l.get("c"), Some(Value::Node(_))), "hit(25,25) old-box miss region now hits");
    assert!(matches!(l.get("d"), Some(Value::Node(_))), "hit(33,33) drawn center region hits");
}

/// T-HP-03：pivot (1,1) —— 盒移到 [16,32) x [16,32)：hit(17,17) 命中；
/// hit(32,32) miss（旧盒的起点、新盒的半开终点）；hit(15,17) miss。
#[test]
fn t_hp_03_pivot_one_full_shift() {
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let sp = t.add_node(t.root(), "sp", NodeKind::Sprite2D);
    t.set_local(sp, Transform2D::from_pos(32.0, 32.0));
    t.set_prop(sp, "pivot", Value::Vec2(Vec2::new(1.0, 1.0))).unwrap();
    t.apply_pending();
    t.refresh_transforms();
    assert_eq!(t.sprite_hit_origin(sp), Vec2::new(16.0, 16.0));

    let l = run_hit_script(
        Some((1.0, 1.0)),
        "on \"go\" { a = hit(17.0, 17.0)\n  b = hit(32.0, 32.0)\n  c = hit(15.0, 17.0) }"
            .to_string(),
    );
    assert!(matches!(l.get("a"), Some(Value::Node(_))), "hit(17,17) inside [16,32) box");
    assert_eq!(l.get("b"), Some(&Value::Bool(false)), "hit(32,32) half-open end of new box");
    assert_eq!(l.get("c"), Some(&Value::Bool(false)), "hit(15,17) left of new box");
}

/// T-HP-04：pivot 属性类型错（set_prop_raw 塞字符串）—— 按缺省 (0,0)
/// 回归 T-HP-01 盒位，不 panic；顺带钉住非有限分量（NaN）同缺省口径
///（与提取层 sprite_pivot 一致 —— NaN 进平移会污染世界矩阵）。
#[test]
fn t_hp_04_bad_pivot_type_falls_back() {
    // 字符串塞 pivot：缺省 (0,0)，盒回 [32,48)。
    let l = run_hit_script(
        None,
        "on \"go\" { a = hit(33.0, 33.0)\n  b = hit(25.0, 25.0) }".to_string(),
    );
    assert!(matches!(l.get("a"), Some(Value::Node(_))), "string pivot falls back to (0,0)");
    assert_eq!(l.get("b"), Some(&Value::Bool(false)), "shifted region must miss on fallback");

    // 直接在树上塞两种坏值再走助手，钉住回退 + 不 panic。
    let mut t = SceneTree::new("root");
    t.apply_pending();
    let sp = t.add_node(t.root(), "sp", NodeKind::Sprite2D);
    t.set_local(sp, Transform2D::from_pos(32.0, 32.0));
    t.apply_pending();
    t.refresh_transforms();
    t.set_prop_raw(sp, "pivot", Value::Str("not-a-vec2".to_string()));
    assert_eq!(t.sprite_hit_origin(sp), Vec2::new(32.0, 32.0), "string pivot -> default origin");
    t.set_prop_raw(sp, "pivot", Value::Vec2(Vec2::new(f32::NAN, 0.0)));
    assert_eq!(t.sprite_hit_origin(sp), Vec2::new(32.0, 32.0), "NaN pivot -> default origin");
    t.set_prop_raw(sp, "pivot", Value::I64(7));
    assert_eq!(t.sprite_hit_origin(sp), Vec2::new(32.0, 32.0), "i64 pivot -> default origin");
}
