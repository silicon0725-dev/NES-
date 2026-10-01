//! T-H 契约回归：实体句柄（S8.2b-1 —— `Value::Node` / `node()` /
//! 统一 resolve 边界 / 成员链 / 引用等式 / 语义指纹）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-H-01 | 句柄可存局部、可经 `node()` 取回、成员读写全通（`node(h).pos = ...` / `node(h).pos.x` / 属性 / 复合赋值 / ++） |
//! | T-H-02 | **悬垂句柄**：目标删除后每次访问经 resolve 校验如实停机（不撞复用槽位、不静默变假） |
//! | T-H-03 | 引用等式：`node("a") == node("a")` 真、不同节点假；句柄局部与现取句柄相等 |
//! | T-H-04 | **语义指纹**：句柄哈希 resolve 结果（别名折叠、悬垂规范化）；位形/gen 不进指纹 —— 指纹对 arena 历史不敏感 |

use nes_scene::{NodeKind, ScriptVm, SceneTree, Transform2D, Value};

fn src(t: &mut SceneTree, node: nes_scene::NodeId, text: &str) {
    t.set_prop(node, "source", Value::Str(text.to_string())).unwrap();
}

/// T-H-01：局部持有 + 成员读写全链。
#[test]
fn t_h_01_handle_hold_and_member_access() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    t.set_local(sp, Transform2D::from_pos(10.0, 20.0));
    let _n2 = t.add_node(t.root(), "flag", NodeKind::Node);
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    t.apply_pending();
    src(
        &mut t,
        b,
        "on \"go\" { h = node(\"sp\")\n  node(h).pos = xy(3.0, 4.0)\n  rx = node(h).pos.x\n  ry = node(h).pos.y\n  node(h).visible = false\n  h2 = node(\"flag\")\n  same = node(h) == node(h2) }",
    );
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);

    assert_eq!(t.local(sp).unwrap().pos.x, 3.0, "句柄写 pos 落地");
    assert_eq!(t.local(sp).unwrap().pos.y, 4.0);
    assert_eq!(t.prop(sp, "visible"), Some(&Value::Bool(false)), "句柄写属性");
    // 微批次契约（S7.1，T-Cmp-32 同款）：同处理器内句柄自读**旧值**
    //（写是 Cmd 未落地）；落地后的新值经上方树断言核对。
    let locals = vm.locals(b).unwrap();
    assert_eq!(locals.get("rx"), Some(&Value::F32(10.0)), "同处理器句柄自读旧值");
    assert_eq!(locals.get("ry"), Some(&Value::F32(20.0)));
    assert_eq!(locals.get("same"), Some(&Value::Bool(false)), "不同节点引用不等");

    // 复合赋值 + 后缀 ++（DupN 双压路径）。
    src(
        &mut t,
        b,
        "on \"go\" { h = node(\"sp\")\n  node(h).pos += xy(1.0, 1.0)\n  node(h).z_index += 2 }",
    );
    let _ = vm.poll_reloads(&mut t);
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(t.local(sp).unwrap().pos.x, 4.0, "复合赋值（读-算-写）");
    let sp2 = t.find_by_name("sp").unwrap();
    assert_eq!(t.prop(sp2, "z_index"), Some(&Value::I64(2)), "属性复合赋值");
}

/// T-H-02：悬垂句柄 —— 每次访问经 resolve 校验，如实停机指名。
/// 构造：同入口两次调用（局部跨调用持久）—— 首次拿句柄，帧间删目标，
/// 再次调用走局部物化路径（Op::Local 的 handle->N 校验）。
#[test]
fn t_h_02_stale_handle_halts_on_access() {
    let mut t = SceneTree::new("root");
    let victim = t.add_node(t.root(), "victim", NodeKind::Node2D);
    let b = t.add_node(t.root(), "b", NodeKind::Script);
    t.apply_pending();
    src(
        &mut t,
        b,
        "on \"go\" { if have == 0 { have = 1; h = node(\"victim\") } else { y = h; done = 1 } }",
    );
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());

    // 第一次：拿句柄。
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(vm.locals(b).unwrap().get("have") == Some(&Value::I64(1)));

    // 帧间删除（结构延迟：下一帧帧首落地）。
    t.remove_node(victim, false);
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(!t.contains(victim), "目标已删");

    // 第二次：局部里的悬垂句柄物化 -> resolve 校验失败 -> 停机指名。
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let l = vm.locals(b).unwrap();
    assert_eq!(l.get("done"), None, "未走到 done（停机在物化）");
    assert!(
        l.get(nes_scene::HALT_LOCAL).is_some_and(|v| matches!(v, Value::Str(s) if s.contains("悬垂"))),
        "停机指名悬垂：{:?}",
        l.get(nes_scene::HALT_LOCAL)
    );

    // 名解析通道的停机（node(不存在的名)）。
    let mut t2 = SceneTree::new("root");
    let b2 = t2.add_node(t2.root(), "b", NodeKind::Script);
    t2.apply_pending();
    src(&mut t2, b2, "on \"go\" { x = node(\"missing\") }");
    let mut vm2 = ScriptVm::new();
    assert!(vm2.attach_all(&mut t2).is_empty());
    t2.emit_signal("go", Value::I64(0));
    let _ = t2.tick(1.0 / 60.0, &mut vm2);
    assert!(
        vm2.locals(b2).is_some_and(|l| l
            .get(nes_scene::HALT_LOCAL)
            .is_some_and(|v| matches!(v, Value::Str(s) if s.contains("找不到")))),
        "node(不存在名) 停机指名"
    );
}

/// T-H-03：引用等式（栈上 N×N —— Local 物化后比较）。
#[test]
fn t_h_03_reference_equality() {
    let mut t = SceneTree::new("root");
    t.add_node(t.root(), "a", NodeKind::Node2D);
    t.add_node(t.root(), "b", NodeKind::Node2D);
    let s = t.add_node(t.root(), "s", NodeKind::Script);
    t.apply_pending();
    src(
        &mut t,
        s,
        "on \"go\" { h1 = node(\"a\")\n  h2 = node(\"a\")\n  h3 = node(\"b\")\n  same = h1 == h2\n  diff = h1 == h3\n  fresh = h1 == node(\"a\")\n  mixed = h1 == 5 }",
    );
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    let l = vm.locals(s).unwrap();
    assert_eq!(l.get("same"), Some(&Value::Bool(true)), "别名句柄相等");
    assert_eq!(l.get("diff"), Some(&Value::Bool(false)));
    assert_eq!(l.get("fresh"), Some(&Value::Bool(true)), "局部句柄 == 现取");
    assert_eq!(l.get("mixed"), Some(&Value::Bool(false)), "句柄 != 数值");
}

/// T-H-04：语义指纹 —— 句柄哈希 resolve 结果；arena 历史（gen）不进指纹。
#[test]
fn t_h_04_fingerprint_resolve_semantics() {
    // 两个运行时实例，语义状态相同、**arena 历史不同**（B 先经历一次
    // 生成-删除再重建同名节点 —— slot/gen 不同），指纹必须一致。
    let build = |churn: bool| {
        let mut t = SceneTree::new("root");
        let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
        t.set_local(sp, Transform2D::from_pos(5.0, 6.0));
        // 两分支 tick 次数对齐（帧号是语义状态的一部分，S7.3）——
        // 只比 arena 历史（slot 分配 / gen 递增）的影响。
        if churn {
            // arena 历史：插入再删除（gen 递增、槽位进 free 表）。
            let junk = t.add_node(t.root(), "junk", NodeKind::Node);
            t.apply_pending();
            t.remove_node(junk, false);
            let _ = t.tick(1.0 / 60.0, &mut nes_scene::NoObserver); // 落地删除
        } else {
            let _ = t.tick(1.0 / 60.0, &mut nes_scene::NoObserver); // 对齐帧号
        }
        let b = t.add_node(t.root(), "b", NodeKind::Script);
        t.apply_pending();
        src(&mut t, b, "on \"go\" { h = node(\"sp\")\n  n = 1 }");
        let mut vm = ScriptVm::new();
        assert!(vm.attach_all(&mut t).is_empty());
        t.emit_signal("go", Value::I64(0));
        let _ = t.tick(1.0 / 60.0, &mut vm);
        (t, vm)
    };
    // S9-1 起 canonical 身份 = uid：两次**独立构造**的树身份本就不同
    //（随机 v4）—— 本测试改为"同一语义场景（同 uid）双实例，其中一个
    // 经历 arena churn（插入-删除）"：churn 改变 slot 分配与 gen 序列，
    // 但不改任何存活节点的 uid/内容 → 指纹必须恒等。
    let (t0, _) = build(false);
    let doc = nes_scene::to_doc(&t0);
    let text = nes_scene::doc_to_ron(&doc, &nes_scene::PackOptions::compact());
    let rerun = |churn: bool| {
        let mut t = nes_scene::instantiate(&text).unwrap();
        if churn {
            let junk = t.add_node(t.root(), "junk", NodeKind::Node);
            t.apply_pending();
            t.remove_node(junk, false);
            let _ = t.tick(1.0 / 60.0, &mut nes_scene::NoObserver);
        } else {
            let _ = t.tick(1.0 / 60.0, &mut nes_scene::NoObserver);
        }
        let b = t.find_by_name("b").unwrap();
        let mut vm = ScriptVm::new();
        assert!(vm.attach_all(&mut t).is_empty());
        t.emit_signal("go", Value::I64(0));
        let _ = t.tick(1.0 / 60.0, &mut vm);
        let _ = b;
        (t, vm)
    };
    let (ta, vma) = rerun(false);
    let (tb, vmb) = rerun(true);
    let fa = nes_scene::scene_fingerprint(&ta, Some(&vma));
    let fb = nes_scene::scene_fingerprint(&tb, Some(&vmb));
    assert_eq!(fa, fb, "arena 历史（gen/槽位复用）不进语义指纹（同 uid 场景）");
}
