//! M1 出口准则验收测试。
//!
//! 覆盖草案给出的四条 M1 出口：
//! 1. 遍历顺序稳定可复现（`traversal_order_is_deterministic`）；
//! 2. `Reparent` 拒绝成环（`reparent_rejects_cycles`）；
//! 3. 遍历中发起结构变更不破坏本次遍历（`spawning_during_process_is_deferred`）；
//! 4. 变换脏传播正确（`transform_propagates_downward_only` 等）。
//!
//! 另附身份失效、路径往返、自动改名、换位等不变式测试。

use nes_scene::*;

/// 记录型观察者：把四个阶段的派发序列原样记下来，便于逐项断言。
#[derive(Default)]
struct Log {
    enter: Vec<String>,
    ready: Vec<String>,
    process: Vec<String>,
    tree_events: Vec<TreeEvent>,
}

impl SceneObserver for Log {
    fn on_tree_event(&mut self, _tree: &SceneTree, ev: &TreeEvent) {
        self.tree_events.push(ev.clone());
    }
    fn on_enter_tree(&mut self, ctx: &mut NodeCtx<'_>) {
        self.enter.push(ctx.name().to_string());
    }
    fn on_ready(&mut self, ctx: &mut NodeCtx<'_>) {
        self.ready.push(ctx.name().to_string());
    }
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        self.process.push(ctx.name().to_string());
    }
}

/// 标准测试树：
/// ```text
/// root
/// ├─ A
/// │  ├─ A1
/// │  └─ A2
/// └─ B
/// ```
fn build() -> (SceneTree, NodeId, NodeId, NodeId) {
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "A", NodeKind::Node2D);
    let a1 = t.add_node(a, "A1", NodeKind::Sprite2D);
    let _a2 = t.add_node(a, "A2", NodeKind::Node2D);
    let b = t.add_node(t.root(), "B", NodeKind::Node2D);
    let mut log = Log::default();
    t.tick(1.0 / 60.0, &mut log);
    (t, a, a1, b)
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

// ---------------------------------------------------------------- 出口 1：确定性

#[test]
fn traversal_order_is_deterministic() {
    let (mut t, ..) = build();

    let mut first = Log::default();
    t.tick(0.016, &mut first);
    let mut second = Log::default();
    t.tick(0.016, &mut second);

    // 同一棵静止的树，两次遍历必须逐项相同。
    assert_eq!(first.process, second.process);
    // 且必须是前序：父在子前，兄弟按插入序。
    assert_eq!(first.process, vec!["root", "A", "A1", "A2", "B"]);
}

#[test]
fn traversal_is_stable_after_reparent() {
    let (mut t, a, a1, b) = build();
    // 把 A1 从 A 挪到 B 下。
    t.reparent(a1, b, None);
    let mut log = Log::default();
    t.tick(0.016, &mut log);

    // A 下只剩 A2；A1 出现在 B 之后。
    assert_eq!(log.process, vec!["root", "A", "A2", "B", "A1"]);
    assert_eq!(t.parent(a1), Some(b));
    assert_eq!(t.children(a), &[t.find_str("root/A/A2").unwrap()][..]);
}

#[test]
fn enter_is_top_down_and_ready_is_bottom_up() {
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "A", NodeKind::Node2D);
    let _a1 = t.add_node(a, "A1", NodeKind::Node2D);

    let mut log = Log::default();
    t.tick(0.016, &mut log);

    // enter_tree：自顶向下
    assert_eq!(log.enter, vec!["root", "A", "A1"]);
    // ready：自底向上（子先于父），这对"子节点在 ready 里访问父"的常见写法是关键保证
    assert_eq!(log.ready, vec!["A1", "A", "root"]);

    // 第二次 tick 不应重复派发
    let mut log2 = Log::default();
    t.tick(0.016, &mut log2);
    assert!(log2.enter.is_empty());
    assert!(log2.ready.is_empty());
}

// ---------------------------------------------------------------- 出口 2：无环

#[test]
fn reparent_rejects_cycles() {
    let (mut t, a, a1, _b) = build();

    // 祖先挂到自己的后代下 → 必须拒绝
    t.reparent(a, a1, None);
    // 自己挂到自己下 → 必须拒绝
    t.reparent(a, a, None);
    // 挂到根下（合法）
    t.reparent(a1, t.root(), None);

    let mut log = Log::default();
    t.tick(0.016, &mut log);

    // a 的父仍是 root（未被破坏）
    assert_eq!(t.parent(a), Some(t.root()));
    // a1 未被 a 抢走：它按第三条合法指令挂到了 root 下
    assert_eq!(t.parent(a1), Some(t.root()));

    let rejected = log
        .tree_events
        .iter()
        .filter(|e| matches!(e, TreeEvent::Rejected { op: "Reparent", .. }))
        .count();
    assert_eq!(rejected, 2, "两次成环请求都应被拒绝");
}

#[test]
fn ancestors_and_depth_are_consistent() {
    let (t, a, a1, _b) = build();
    assert_eq!(t.depth(t.root()), 0);
    assert_eq!(t.depth(a), 1);
    assert_eq!(t.depth(a1), 2);
    assert_eq!(t.ancestors(a1), vec![a, t.root()]);
    assert!(t.is_ancestor_of(t.root(), a1));
    assert!(!t.is_ancestor_of(a1, a));
}

// ---------------------------------------------------------------- 出口 3：遍历中变更

struct Spawner {
    seen: Vec<String>,
    spawned: bool,
}

impl SceneObserver for Spawner {
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        let name = ctx.name().to_string();
        self.seen.push(name.clone());
        if name == "A" && !self.spawned {
            self.spawned = true;
            ctx.spawn_child("Late", NodeKind::Node2D);
        }
    }
}

#[test]
fn spawning_during_process_is_deferred() {
    let (mut t, ..) = build();

    let mut sp = Spawner {
        seen: Vec::new(),
        spawned: false,
    };
    t.tick(0.016, &mut sp);

    // 本次遍历序列完全不受回调影响
    assert_eq!(sp.seen, vec!["root", "A", "A1", "A2", "B"]);
    // 新节点此刻还没挂树
    assert!(t.find_by_name("Late").is_none());

    // 下一帧才出现，且位置正确（A 的第一个子节点之后按 order 排在末尾）
    let mut log = Log::default();
    t.tick(0.016, &mut log);
    assert!(log.process.contains(&"Late".to_string()));
    let late = t.find_by_name("Late").expect("Late 应已入树");
    assert_eq!(t.parent(late).and_then(|p| t.name(p)), Some("A"));
}

#[test]
fn removal_during_traversal_does_not_break_frame() {
    struct Killer {
        seen: Vec<String>,
        done: bool,
    }
    impl SceneObserver for Killer {
        fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
            let name = ctx.name().to_string();
            self.seen.push(name.clone());
            if name == "A" && !self.done {
                self.done = true;
                let target = ctx.tree().find_str("root/A/A1").expect("exists");
                ctx.queue(TreeOp::Remove {
                    node: target,
                    keep_children: false,
                });
            }
        }
    }

    let (mut t, ..) = build();
    let mut k = Killer {
        seen: Vec::new(),
        done: false,
    };
    t.tick(0.016, &mut k);
    // 删除请求在本帧不生效，遍历照旧走完
    assert_eq!(k.seen, vec!["root", "A", "A1", "A2", "B"]);

    let mut log = Log::default();
    t.tick(0.016, &mut log);
    assert_eq!(log.process, vec!["root", "A", "A2", "B"]);
    assert!(t.find_str("root/A/A1").is_none());
}

// ---------------------------------------------------------------- 出口 4：变换传播

#[test]
fn transform_propagates_downward_only() {
    let (mut t, a, a1, _b) = build();

    t.set_local(a, Transform2D::from_pos(10.0, 0.0));
    t.refresh_transforms();
    assert!(approx(t.world(a1).unwrap().tx, 10.0), "子应跟随父平移");

    t.set_local(a1, Transform2D::from_pos(1.0, 2.0));
    t.refresh_transforms();
    assert!(approx(t.world(a1).unwrap().tx, 11.0), "子的世界位置应叠加父");
    assert!(approx(t.world(a1).unwrap().ty, 2.0));
    // 改子不得污染父
    assert!(approx(t.world(a).unwrap().tx, 10.0));
}

#[test]
fn parent_rotation_moves_child_world_position() {
    let (mut t, a, a1, _b) = build();
    t.set_local(a1, Transform2D::from_pos(0.0, 1.0));
    t.set_local(a, Transform2D::from_rot(std::f32::consts::FRAC_PI_2));
    t.refresh_transforms();

    // 子在父本地 (0,1)，父旋转 90° → 世界 (-1,0)
    let w = t.world(a1).unwrap();
    assert!(approx(w.tx, -1.0), "tx={}", w.tx);
    assert!(approx(w.ty, 0.0), "ty={}", w.ty);
}

#[test]
fn dirty_flush_count_reflects_affected_subtree() {
    let (mut t, a, _a1, _b) = build();
    // 全树已冲洗干净
    let clean = t.refresh_transforms();
    assert_eq!(clean, 0, "无脏节点时不应重算");

    // 改 A 只应重算 A 及其子树（A1、A2）= 3 个。
    // root 不在内（root.local 没变，它只是需要下探），B 也不在内（与 A 无关的兄弟）。
    t.set_local(a, Transform2D::from_pos(1.0, 1.0));
    assert_eq!(t.refresh_transforms(), 3, "只有 A 子树需要重算");
    // 再冲洗一次应无事可做：脏标记必须被清干净
    assert_eq!(t.refresh_transforms(), 0);
    assert_eq!(t.refresh_transforms(), 0, "二次冲洗应为空");
}

#[test]
fn nonuniform_parent_scale_is_carried_into_world() {
    let (mut t, a, a1, _b) = build();
    t.set_local(a1, Transform2D::from_pos(1.0, 1.0));
    t.set_local(a, Transform2D::from_scale(2.0, 3.0));
    t.refresh_transforms();

    let w = t.world(a1).unwrap();
    assert!(approx(w.tx, 2.0), "tx={}", w.tx);
    assert!(approx(w.ty, 3.0), "ty={}", w.ty);
}

// ---------------------------------------------------------------- 身份与不变式

#[test]
fn removed_subtree_ids_are_invalidated() {
    let (mut t, a, a1, _b) = build();
    t.remove_node(a, false);
    let mut log = Log::default();
    t.tick(0.016, &mut log);

    assert!(t.get(a).is_none(), "被删节点身份必须失效");
    assert!(t.get(a1).is_none(), "整棵子树一并失效");
    assert!(!t.children(t.root()).contains(&a));
    assert!(t.find_str("root/A").is_none());
}

#[test]
fn remove_keeping_children_reparents_them() {
    let (mut t, a, a1, _b) = build();
    t.remove_node(a, true);
    let mut log = Log::default();
    t.tick(0.016, &mut log);

    assert!(t.get(a).is_none());
    assert_eq!(t.parent(a1), Some(t.root()), "孙节点应被提升到祖父下");
}

#[test]
fn duplicate_names_are_auto_suffixed() {
    let mut t = SceneTree::new("root");
    let _x = t.add_node(t.root(), "X", NodeKind::Node2D);
    let _y = t.add_node(t.root(), "X", NodeKind::Node2D);
    let _z = t.add_node(t.root(), "X", NodeKind::Node2D);
    let mut log = Log::default();
    t.tick(0.016, &mut log);

    let names: Vec<String> = t
        .children(t.root())
        .iter()
        .filter_map(|c| t.name(*c).map(|s| s.to_string()))
        .collect();
    assert_eq!(names, vec!["X", "X2", "X3"]);
    assert_eq!(
        log.tree_events
            .iter()
            .filter(|e| matches!(e, TreeEvent::NameAdjusted { .. }))
            .count(),
        2
    );
}

#[test]
fn children_stay_sorted_after_move() {
    let (mut t, a, _a1, b) = build();
    t.move_child(a, 1);
    let mut log = Log::default();
    t.tick(0.016, &mut log);

    assert_eq!(t.children(t.root()), &[b, a][..]);
    // 交换后遍历序同步变化，且仍然确定
    assert_eq!(log.process, vec!["root", "B", "A", "A1", "A2"]);
}

// ---------------------------------------------------------------- 路径

#[test]
fn path_roundtrip() {
    let (t, _a, a1, _b) = build();
    let p = t.path_of(a1).expect("路径可生成");
    assert_eq!(p.to_string(), "root/A/A1");
    assert_eq!(t.find(&p), Some(a1));
    assert_eq!(t.find_str("root/A/A1"), Some(a1));
    assert_eq!(t.find_str("/root/A/A1"), Some(a1));

    // 不存在的路径返回 None，不 panic
    assert_eq!(t.find_str("root/A/Nope"), None);
    assert_eq!(t.find_str("Other/A"), None);
    assert_eq!(t.find_str("root/A/A1/TooDeep"), None);
}

#[test]
fn duplicate_names_are_renamed_so_generated_paths_stay_unique() {
    let mut t = SceneTree::new("root");
    let x0 = t.add_node(t.root(), "Ghost", NodeKind::Node2D);
    let x1 = t.add_node(t.root(), "Ghost", NodeKind::Node2D);
    let mut log = Log::default();
    t.tick(0.016, &mut log);

    // 自动改名保证"一父一名"，所以**生成的**路径天然唯一，不带索引段。
    assert_eq!(t.path_of(x0).unwrap().to_string(), "root/Ghost");
    assert_eq!(t.path_of(x1).unwrap().to_string(), "root/Ghost2");

    // 索引段仍被解析与查找支持 —— 宽容输入、严格输出。
    // 保留它的两个理由：外部手工编写的场景文件可能带 `[n]`；
    // 将来若放开"允许同名兄弟"开关，它可以立刻生效，无需改语法。
    assert_eq!(t.find_str("root/Ghost"), Some(x0));
    assert_eq!(t.find_str("root/Ghost[0]"), Some(x0));
    assert_eq!(t.find_str("root/Ghost[1]"), None);
    assert_eq!(t.find_str("root/Ghost2"), Some(x1));
}

// ---------------------------------------------------------------- 组

#[test]
fn group_members_follow_traversal_order() {
    let (mut t, _a, a1, b) = build();
    t.set_group(b, "enemies", true);
    t.set_group(a1, "enemies", true);
    // 故意按"逆序"加入，输出仍应是前序序
    assert_eq!(t.group_members("enemies"), vec![a1, b]);
    assert!(t.groups_of(a1).contains(&"enemies"));
    t.set_group(b, "enemies", false);
    assert_eq!(t.group_members("enemies"), vec![a1]);
}

// ---------------------------------------------------------------- 帧统计

#[test]
fn tick_stats_report_work_done() {
    let mut t = SceneTree::new("root");
    let _a = t.add_node(t.root(), "A", NodeKind::Node2D);

    let stats = t.tick(0.016, &mut Log::default());
    assert_eq!(stats.frame, 0);
    assert_eq!(stats.entered, 2, "root + A");
    assert_eq!(stats.readied, 2);
    assert_eq!(stats.processed, 2);
    assert_eq!(stats.events, 1, "只有 A 是新增");
    assert!(stats.dirty_flushed >= 2);

    let stats2 = t.tick(0.016, &mut Log::default());
    assert_eq!(stats2.frame, 1);
    assert_eq!(stats2.entered, 0);
    assert_eq!(stats2.readied, 0);
    assert_eq!(stats2.processed, 2);
}
