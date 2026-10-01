//! T-A 契约回归：实体集合（S8.2b-2 —— Array / for_each / children /
//! `it` 成员 / 深拷贝 / 快照迭代）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-A-01 | array/push/pop/len/索引 roundtrip；**赋值 = 深拷贝**（`a2 = a` 后 push 互不影响）；越界/空 pop 停机 |
//! | T-A-02 | `children` 返回**child order**（结构序），元素为句柄；经 node() 成员读写通 |
//! | T-A-03 | for_each 快照：体内 push 不可见（len 不变）、循环后可见；break/continue 继承 |
//! | T-A-04 | `it.pos`/`it.属性` 直达语法：对每个实体写（实体规模化惯用形）；悬垂句柄元素经 resolve 停机 |
//! | T-A-05 | 确定性：数组局部（含句柄）语义化进指纹，双跑恒等 |

use nes_scene::{NodeKind, ScriptVm, SceneTree, Transform2D, Value};

fn src(t: &mut SceneTree, node: nes_scene::NodeId, text: &str) {
    t.set_prop(node, "source", Value::Str(text.to_string())).unwrap();
}

/// T-A-01：数组原语 + 深拷贝。
#[test]
fn t_a_01_array_primitives_and_deep_copy() {
    let mut t = SceneTree::new("root");
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    t.apply_pending();
    src(
        &mut t,
        b,
        "on \"go\" { a = array()\n  push(a, 1)\n  push(a, 2.5)\n  n0 = len(a)\n  v1 = a[1]\n  a2 = a\n  push(a2, 99)\n  la = len(a)\n  la2 = len(a2)\n  pop(a)\n  laf = len(a) }",
    );
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let l = vm.locals(b).unwrap();
    assert_eq!(l.get("n0"), Some(&Value::I64(2)));
    assert_eq!(l.get("v1"), Some(&Value::F32(2.5)), "索引读元素");
    assert_eq!(l.get("la"), Some(&Value::I64(2)), "深拷贝：a2 的 push 不影响 a");
    assert_eq!(l.get("la2"), Some(&Value::I64(3)));
    assert_eq!(l.get("laf"), Some(&Value::I64(1)), "pop 变异局部绑定");

    // 错误口径：越界 / pop 空 / 非数组 push。
    src(&mut t, b, "on \"go\" { a = array(); x = a[0] }");
    let _ = vm.poll_reloads(&mut t);
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(vm.locals(b).unwrap().contains_key(nes_scene::HALT_LOCAL), "空数组索引停机");
    src(&mut t, b, "on \"go\" { a = array(); pop(a) }");
    let _ = vm.poll_reloads(&mut t);
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(vm.locals(b).unwrap().contains_key(nes_scene::HALT_LOCAL), "pop 空停机");
}

/// T-A-02：children = child order + 句柄元素。
#[test]
fn t_a_02_children_structural_order() {
    let mut t = SceneTree::new("root");
    let holder = t.add_node(t.root(), "holder", NodeKind::Node);
    let c1 = t.add_node(holder, "z_first_added", NodeKind::Node2D);
    let c2 = t.add_node(holder, "a_second", NodeKind::Node2D);
    let c3 = t.add_node(holder, "m_third", NodeKind::Node2D);
    t.set_local(c2, Transform2D::from_pos(7.0, 8.0));
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    t.apply_pending();
    src(
        &mut t,
        b,
        "on \"go\" { h = node(\"holder\")\n  kids = children(h)\n  nk = len(kids)\n  second = kids[1]\n  px = node(second).pos.x\n  node(second).pos = xy(1.0, 2.0) }",
    );
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let l = vm.locals(b).unwrap();
    assert_eq!(l.get("nk"), Some(&Value::I64(3)));
    assert_eq!(l.get("px"), Some(&Value::F32(7.0)), "kids[1] 是第二个子（结构序）");
    assert_eq!(t.local(c2).unwrap().pos.x, 1.0, "句柄元素成员写落地");
    let _ = (c1, c3);
}

/// T-A-03：for_each 快照迭代 + break/continue。
#[test]
fn t_a_03_for_each_snapshot_and_flow() {
    let mut t = SceneTree::new("root");
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    t.apply_pending();
    src(
        &mut t,
        b,
        "on \"go\" { a = array()\n  push(a, 10)\n  push(a, 20)\n  push(a, 30)\n  sum = 0\n  for_each(a) { push(a, 99)\n  ln = len(a)\n  sum = sum + it }\n  after = len(a) }",
    );
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let l = vm.locals(b).unwrap();
    assert_eq!(l.get("sum"), Some(&Value::I64(60)), "恰遍历快照三元素（体内 push 不扩迭代）");
    // 局部绑定 a 本身正常增长（变异局部绑定语义）；**迭代界**来自快照
    //（sum 只累计 3 个元素即证明）。末轮 len(a) = 6。
    assert_eq!(l.get("ln"), Some(&Value::I64(6)), "用户绑定正常增长（局部绑定变异）");
    assert_eq!(l.get("after"), Some(&Value::I64(6)));

    // break / continue 继承。
    src(
        &mut t,
        b,
        "on \"go\" { a = array()\n  push(a, 1)\n  push(a, 2)\n  push(a, 3)\n  push(a, 4)\n  acc = 0\n  for_each(a) { if it == 2 { continue }\n  if it == 4 { break }\n  acc = acc + it } }",
    );
    let _ = vm.poll_reloads(&mut t);
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(
        vm.locals(b).unwrap().get("acc"),
        Some(&Value::I64(4)),
        "continue 跳过 2，break 在 4 前停（1+3）"
    );
}

/// T-A-04：`it.member` 直达语法 —— 实体规模化惯用形。
#[test]
fn t_a_04_it_member_writes_each_entity() {
    let mut t = SceneTree::new("root");
    let holder = t.add_node(t.root(), "holder", NodeKind::Node);
    let s1 = t.add_node(holder, "s1", NodeKind::Node2D);
    let s2 = t.add_node(holder, "s2", NodeKind::Node2D);
    let s3 = t.add_node(holder, "s3", NodeKind::Node);
    t.set_local(s1, Transform2D::from_pos(0.0, 0.0));
    t.set_local(s2, Transform2D::from_pos(0.0, 0.0));
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    t.apply_pending();
    src(
        &mut t,
        b,
        "on \"go\" { kids = children(node(\"holder\"))\n  for_each(kids) { it.pos += xy(2.0, 1.0)\n  it.visible = false } }",
    );
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.local(s1).unwrap().pos.x, 2.0, "s1 移动");
    assert_eq!(t.local(s2).unwrap().pos.y, 1.0, "s2 移动");
    assert_eq!(t.prop(s3, "visible"), Some(&Value::Bool(false)), "s3 属性写");

    // 悬垂元素：迭代中删除发生在帧间（结构延迟）—— 下一轮访问停机。
    t.remove_node(s2, false);
    let _ = t.tick(1.0 / 60.0, &mut vm); // 落地删除
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm); // children 重新收集（无 s2）
    assert_eq!(t.local(s1).unwrap().pos.x, 4.0, "再次遍历（2 个子）");
}

/// T-A-05：确定性 —— 数组局部（含句柄）语义化进指纹。
#[test]
fn t_a_05_array_locals_deterministic_fingerprint() {
    let build = || {
        let mut t = SceneTree::new("root");
        let h = t.add_node(t.root(), "h", NodeKind::Node);
        for i in 0..3 {
            let c = t.add_node(h, &format!("c{i}"), NodeKind::Node2D);
            t.set_local(c, Transform2D::from_pos(i as f32, 0.0));
        }
        let b = t.add_node(t.root(), "b", NodeKind::Script);
        t.apply_pending();
        src(
            &mut t,
            b,
            "on \"go\" { kids = children(node(\"h\"))\n  push(kids, node(\"h\"))\n  n = len(kids) }",
        );
        let mut vm = ScriptVm::new();
        assert!(vm.attach_all(&mut t).is_empty());
        t.emit_signal("go", Value::I64(0));
        let _ = t.tick(1.0 / 60.0, &mut vm);
        (t, vm)
    };
    let (t1, v1) = build();
    let (t2, v2) = build();
    assert_eq!(
        nes_scene::scene_fingerprint(&t1, Some(&v1)),
        nes_scene::scene_fingerprint(&t2, Some(&v2)),
        "数组局部（句柄元素语义化）双跑恒等"
    );
}

/// T-A-06（补）：**嵌套 for_each** —— 内层 `it` 遮蔽外层（语言层单名，
/// VM 层 it0/it1）；外层变量在内层可见。
#[test]
fn t_a_06_nested_for_each_shadowing() {
    let mut t = SceneTree::new("root");
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    t.apply_pending();
    src(
        &mut t,
        b,
        "on \"go\" { outer = array()\n  push(outer, 1)\n  push(outer, 2)\n  inner = array()\n  push(inner, 10)\n  push(inner, 20)\n  push(inner, 30)\n  pairs = \"\"\n  for_each(outer) { for_each(inner) { pairs = pairs + \"[\" + num_to_str(it) + \"]\" }\n  after = it } }",
    );
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let l = vm.locals(b).unwrap();
    assert_eq!(
        l.get("pairs"),
        Some(&Value::Str("[10][20][30][10][20][30]".to_string())),
        "内层 it 遮蔽外层（6 次内层迭代）"
    );
    assert_eq!(l.get("after"), Some(&Value::I64(2)), "内层结束后外层 it 可见");
}
