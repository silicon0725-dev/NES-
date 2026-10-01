//! S7.1 运行时语义冻结（Runtime Semantics Freeze）契约：
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-RS-01 | **黄金帧序**：结构落地+桥回调 -> enter（自顶向下）-> ready（自底向上）-> process（自顶向下）-> 信号泵（宿主预发 ++ 帧内发射[桥最前]）-> 变换冲洗 |
//! | T-RS-02 | **暂停矩阵补全**：路由处理器与 process 同表门控（Disabled 永不调用、Pausable 暂停中跳过、Always/WhenPaused 照常）；广播路径不受暂停影响；VM 信号脚本自动同表 |
//! | T-RS-03 | **观察者组合**：注册序稳定派发、同节点回调内后注册成员见先注册成员落地前状态、订阅过滤取并集 |
//! | T-RS-04 | **Cmd 可见性屏障**：级联处理器见发射者已落地的新值；同处理器内自读旧值、同属性末写胜；结构（Spawn）下一帧才入树；信号驱动的 SetLocal 当帧冲洗入画 |

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use nes_scene::{
    NodeCtx, NodeId, NodeKind, Observers, ProcessMode, SceneObserver, SceneTree, Signal,
    SignalCtx, SignalFilter, ScriptVm, Transform2D, TreeEvent, Value};

// ---------------------------------------------------------------- 工具

/// 事件记录仪（黄金序/组合序断言用）。
struct Recorder {
    tag: &'static str,
    log: Rc<RefCell<Vec<String>>>,
    /// process 期对指定节点发射的信号名（Some 时）。
    emit_for: Option<(&'static str, &'static str)>,
}

impl SceneObserver for Recorder {
    fn on_tree_event(&mut self, _t: &SceneTree, ev: &TreeEvent) {
        self.log.borrow_mut().push(format!("{}:ev:{}", self.tag, ev.signal_name()));
    }
    fn on_enter_tree(&mut self, ctx: &mut NodeCtx<'_>) {
        self.log.borrow_mut().push(format!("{}:enter:{}", self.tag, ctx.name()));
    }
    fn on_ready(&mut self, ctx: &mut NodeCtx<'_>) {
        self.log.borrow_mut().push(format!("{}:ready:{}", self.tag, ctx.name()));
    }
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _d: f32) {
        self.log.borrow_mut().push(format!("{}:process:{}", self.tag, ctx.name()));
        if let Some((node, sig)) = self.emit_for {
            if ctx.name() == node {
                ctx.emit(sig, Value::I64(0));
            }
        }
    }
    fn on_signal(&mut self, _ctx: &mut SignalCtx<'_>, sig: &Signal) {
        self.log.borrow_mut().push(format!("{}:signal:{}", self.tag, sig.name));
    }
}

fn take_log(log: &Rc<RefCell<Vec<String>>>) -> Vec<String> {
    std::mem::take(&mut *log.borrow_mut())
}

// ---------------------------------------------------------------- T-RS-01

/// **黄金帧序**：一帧之内各阶段的精确顺序。帧 0 让全部节点完成
/// enter/ready；帧 1 是被审帧 —— 宿主预排队一次结构删除 + 预发一条
/// 信号 + B 在 process 期发射 —— 断言完整日志序。
#[test]
fn t_rs_01_golden_tick_order() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut t = SceneTree::new("root");
    let a = t.add_node(t.root(), "A", NodeKind::Node2D);
    let b = t.add_node(a, "B", NodeKind::Node2D);
    let c = t.add_node(t.root(), "C", NodeKind::Node2D);
    t.apply_pending();
    let _ = b;

    let mut obs = Recorder { tag: "r", log: log.clone(), emit_for: Some(("B", "fromB")) };
    let _ = t.tick(1.0 / 60.0, &mut obs);
    // 帧 0 的序也冻结：enter 自顶向下、ready 自底向上、process 自顶向下，
    // process 期发射的信号当帧入泵交付。
    assert_eq!(
        take_log(&log),
        vec![
            "r:enter:root", "r:enter:A", "r:enter:B", "r:enter:C", //
            "r:ready:C", "r:ready:B", "r:ready:A", "r:ready:root", //
            "r:process:root", "r:process:A", "r:process:B", "r:process:C", //
            "r:signal:fromB",
        ],
        "帧 0：生命周期序 + 当帧泵"
    );

    // 帧 1 材料：预排队删除 C（下一帧帧首落地）、预发 host 信号。
    t.remove_node(c, false);
    t.emit_signal("host", Value::I64(0));
    let stats = t.tick(1.0 / 60.0, &mut obs);
    assert_eq!(
        take_log(&log),
        vec![
            "r:ev:tree/removed", // 阶段 1：结构落地 + 即时事件回调（桥信号入泵）
            "r:process:root", "r:process:A", "r:process:B", // C 已不在遍历序
            "r:signal:host", // 泵序：宿主预发最先
            "r:signal:tree/removed", // 帧内发射按发射期序：桥（阶段 1）在前
            "r:signal:fromB", // process 期发射在后
        ],
        "帧 1：黄金序"
    );
    assert_eq!(stats.signals_delivered, 3);
    assert_eq!(stats.handlers_skipped, 0);
    assert_eq!(stats.events, 1);
}

// ---------------------------------------------------------------- T-RS-02

/// **暂停矩阵补全**：路由处理器（处理器表/观察者路由）按连接目标节点
/// 的**生效模式**门控 —— 与 process 同表、delta 无关：Disabled 永不
/// 调用；Pausable 暂停中跳过（暂停冻结的是 Pausable 族的时间与事件
/// 两者）；Always/WhenPaused 照常。广播（宿主观察者）不受暂停影响。
#[test]
fn t_rs_02_pause_matrix_handler_gating() {
    let mut t = SceneTree::new("root");
    let mk = |t: &mut SceneTree, name: &str, mode: ProcessMode| {
        let n = t.add_node(t.root(), name, NodeKind::Node);
        t.set_process_mode(n, mode);
        n
    };
    let n_pausable = mk(&mut t, "pausable", ProcessMode::Pausable);
    let n_always = mk(&mut t, "always", ProcessMode::Always);
    let n_when = mk(&mut t, "whenpaused", ProcessMode::WhenPaused);
    let n_disabled = mk(&mut t, "disabled", ProcessMode::Disabled);
    let n_inherit = mk(&mut t, "inherit", ProcessMode::Inherit);
    t.apply_pending();

    // 处理器：命中计数（Rc 共享给闭包与断言）。
    let hits: Vec<Rc<Cell<u32>>> = (0..5).map(|_| Rc::new(Cell::new(0))).collect();
    let wire = |t: &mut SceneTree, n: NodeId, hit: &Rc<Cell<u32>>| {
        let h = hit.clone();
        assert!(t.set_signal_handler(
            n,
            "run",
            Box::new(move |_ctx: &mut SignalCtx<'_>, _sig: &Signal| {
                h.set(h.get() + 1);
            }),
        ));
        assert!(t.connect_signal_to("tick", None, n, "run").is_some());
    };
    wire(&mut t, n_pausable, &hits[0]);
    wire(&mut t, n_always, &hits[1]);
    wire(&mut t, n_when, &hits[2]);
    wire(&mut t, n_disabled, &hits[3]);
    wire(&mut t, n_inherit, &hits[4]);

    let log = Rc::new(RefCell::new(Vec::new()));
    let mut obs = Recorder { tag: "r", log: log.clone(), emit_for: None };

    // 未暂停：Pausable/Always/WhenPaused/Inherit->Pausable 命中；Disabled 跳过。
    t.emit_signal("tick", Value::I64(0));
    let st = t.tick(1.0 / 60.0, &mut obs);
    assert_eq!((hits[0].get(), hits[1].get(), hits[2].get(), hits[3].get(), hits[4].get()), (1, 1, 1, 0, 1));
    assert_eq!(st.handlers_skipped, 1, "Disabled 未暂停也跳过");
    assert!(take_log(&log).iter().any(|e| e == "r:signal:tick"));

    // 暂停：Pausable/Inherit 跳过（事件冻结）；Always/WhenPaused 照常；
    // 广播路径不受暂停影响（r:signal:tick 仍达观察者）。
    t.set_paused(true);
    t.emit_signal("tick", Value::I64(0));
    let st = t.tick(1.0 / 60.0, &mut obs);
    assert_eq!((hits[0].get(), hits[1].get(), hits[2].get(), hits[3].get(), hits[4].get()), (1, 2, 2, 0, 1));
    assert_eq!(st.handlers_skipped, 3, "暂停中 Pausable+Inherit+Disabled 跳过");
    assert_eq!(st.processed, 2, "暂停中 Always/WhenPaused 两节点派发 process");
    assert_eq!(st.process_skipped, 4, "root/pausable/inherit（暂停）+ disabled 未派发");
    assert!(
        take_log(&log).iter().any(|e| e == "r:signal:tick"),
        "广播不受暂停影响（泵照常）"
    );
    t.set_paused(false);
}

/// **VM 信号脚本同表**：门控在泵里，处理器表闭包（VM 装载的脚本）
/// 自动继承 —— Disabled 的脚本节点收不到信号；恢复 Inherit 即驱动。
#[test]
fn t_rs_02b_vm_script_respects_mode() {
    let mut t = SceneTree::new("root");
    let sp = t.add_node(t.root(), "sp", NodeKind::Node2D);
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.set_prop(brain, "source", Value::Str("on \"step\" { sp.pos += (4.0, 0.0) }".into()))
        .unwrap();
    t.set_process_mode(brain, ProcessMode::Disabled);
    t.apply_pending();

    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());

    t.emit_signal("step", Value::I64(0));
    let st = t.tick(1.0 / 60.0, &mut nes_scene::NoObserver);
    assert_eq!(st.handlers_skipped, 1, "Disabled 脚本节点处理器被跳过");
    assert_eq!(t.local(sp).unwrap().pos.x, 0.0, "精灵未动");

    t.set_process_mode(brain, ProcessMode::Inherit); // 解析为 Pausable，未暂停
    t.emit_signal("step", Value::I64(0));
    let st = t.tick(1.0 / 60.0, &mut nes_scene::NoObserver);
    assert_eq!(st.handlers_skipped, 0);
    assert_eq!(t.local(sp).unwrap().pos.x, 4.0, "恢复后驱动");
}

// ---------------------------------------------------------------- T-RS-03

/// **观察者组合**：注册序稳定派发；同节点回调内后注册成员见先注册
/// 成员**落地前**状态（Cmd 批次在整组回调返回后落地）。
#[test]
fn t_rs_03_observer_composition_order_and_batch() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut t = SceneTree::new("root");
    let n = t.add_node(t.root(), "n", NodeKind::Node); // visible 缺省 true
    t.apply_pending();

    // 成员 0：process(n) 写 visible=false；成员 1：process(n) 读 visible。
    struct Writer {
        log: Rc<RefCell<Vec<String>>>,
        target: &'static str,
    }
    impl SceneObserver for Writer {
        fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _d: f32) {
            if ctx.name() == self.target {
                ctx.set_prop("visible", Value::Bool(false));
                self.log.borrow_mut().push("w:process".into());
            }
        }
    }
    struct Reader {
        log: Rc<RefCell<Vec<String>>>,
        target: &'static str,
        saw: Rc<Cell<Option<bool>>>,
    }
    impl SceneObserver for Reader {
        fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _d: f32) {
            if ctx.name() == self.target {
                self.saw.set(Some(matches!(ctx.prop("visible"), Some(Value::Bool(true)))));
                self.log.borrow_mut().push("r:process".into());
            }
        }
    }
    let saw = Rc::new(Cell::new(None));
    let mut observers = Observers::new();
    assert_eq!(observers.len(), 0);
    observers.push(Box::new(Recorder { tag: "0", log: log.clone(), emit_for: None }));
    observers.push(Box::new(Writer { log: log.clone(), target: "n" }));
    observers.push(Box::new(Reader { log: log.clone(), target: "n", saw: saw.clone() }));

    let _ = t.tick(1.0 / 60.0, &mut observers); // 帧 0：生命周期
    let frame0 = take_log(&log);
    assert_eq!(
        frame0[frame0.len() - 2..],
        vec!["w:process".to_string(), "r:process".to_string()],
        "process(n)：注册序 w 先于 r"
    );
    assert_eq!(saw.get(), Some(true), "同节点回调内：r 见 w 落地前的旧值（true）");
    assert_eq!(
        t.prop(n, "visible"),
        Some(&Value::Bool(false)),
        "整组回调返回后批次落地"
    );
}

/// **订阅过滤并集**：任一成员订阅即送达组合（各成员自行忽略）；
/// 全员不订阅的才被泵过滤。统计按泵交付计（组合 = 一个观察者）。
#[test]
fn t_rs_03b_filter_union() {
    struct Picky {
        tag: &'static str,
        filter: SignalFilter,
        seen: Rc<RefCell<Vec<String>>>,
    }
    impl SceneObserver for Picky {
        fn on_signal(&mut self, _c: &mut SignalCtx<'_>, s: &Signal) {
            self.seen.borrow_mut().push(format!("{}:{}", self.tag, s.name));
        }
        fn signal_filter(&self) -> SignalFilter {
            self.filter.clone()
        }
    }
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut observers = Observers::new();
    observers.push(Box::new(Picky {
        tag: "a",
        filter: SignalFilter::prefixes(&["ui/"]),
        seen: seen.clone(),
    }));
    observers.push(Box::new(Picky {
        tag: "b",
        filter: SignalFilter::prefixes(&["game/"]),
        seen: seen.clone(),
    }));

    let mut t = SceneTree::new("root");
    t.apply_pending();
    t.emit_signal("ui/x", Value::I64(0));
    t.emit_signal("game/y", Value::I64(0));
    t.emit_signal("other", Value::I64(0));
    let st = t.tick(1.0 / 60.0, &mut observers);
    assert_eq!(
        take_log(&seen),
        vec!["a:ui/x", "b:ui/x", "a:game/y", "b:game/y"],
        "并集：两条成员订阅的信号都转发给全部成员"
    );
    assert_eq!(st.signals_filtered, 1, "无人订阅的 other 被泵过滤");
    assert_eq!(st.signals_delivered, 2, "组合按一个观察者计账");
}

// ---------------------------------------------------------------- T-RS-04

/// **Cmd 可见性屏障**（微批次模型，逐条冻结）：
/// 1. 级联可见：A 处理器写 + 发射 -> B 处理器读到**新值**；
/// 2. 同处理器自读：读到**旧值**（批次未落地）；
/// 3. 同属性末写胜；
/// 4. 结构（Spawn）下一帧帧首才入树（enter/ready 在下下帧... 不，
///    落地帧即完成 enter/ready —— 结构落地的同帧阶段 2/3）；
/// 5. 信号驱动的 SetLocal 当帧冲洗（world 变换已更新）。
#[test]
fn t_rs_04_cmd_visibility_barrier() {
    let mut t = SceneTree::new("root");
    let mk = |t: &mut SceneTree, name: &str| t.add_node(t.root(), name, NodeKind::Node);
    let sa = mk(&mut t, "sa"); // 级联源
    let sb = mk(&mut t, "sb"); // 级联观察者
    let sc_src = mk(&mut t, "sc_src"); // 自读源
    let sc_dst = mk(&mut t, "sc_dst"); // 自读结果
    let sd = mk(&mut t, "sd"); // 末写胜
    let mover = t.add_node(t.root(), "mover", NodeKind::Node2D);
    t.set_local(mover, Transform2D::from_pos(0.0, 0.0));

    let brain = |t: &mut SceneTree, name: &str, src: &str| {
        let b = t.add_node(t.root(), name, NodeKind::Script);
        t.set_prop(b, "source", Value::Str(src.into())).unwrap();
        b
    };
    brain(
        &mut t,
        "brainA",
        "on \"go\" { sa.visible = false\n  emit \"foo\" 0 }",
    );
    brain(&mut t, "brainB", "on \"foo\" { sb.visible = sa.visible }");
    brain(
        &mut t,
        "brainC",
        "on \"go2\" { sc_src.visible = false\n  sc_dst.visible = sc_src.visible }",
    );
    brain(
        &mut t,
        "brainD",
        "on \"go3\" { sd.visible = false\n  sd.visible = true }",
    );
    brain(&mut t, "brainE", "on \"go4\" { mover.pos = (10.0, 20.0) }");
    t.apply_pending();

    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    for name in ["go", "go2", "go3", "go4"] {
        t.emit_signal(name, Value::I64(0));
    }
    let st = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(st.handlers_skipped, 0);

    // 1. 级联可见：B 看到 A 已落地的新值（false）。
    assert_eq!(t.prop(sa, "visible"), Some(&Value::Bool(false)));
    assert_eq!(t.prop(sb, "visible"), Some(&Value::Bool(false)), "级联读新值");
    // 2. 同处理器自读旧值。
    assert_eq!(t.prop(sc_src, "visible"), Some(&Value::Bool(false)));
    assert_eq!(t.prop(sc_dst, "visible"), Some(&Value::Bool(true)), "自读旧值");
    // 3. 同属性末写胜。
    assert_eq!(t.prop(sd, "visible"), Some(&Value::Bool(true)));
    // 5. 信号驱动的 SetLocal 当帧冲洗（泵后阶段 6）。
    assert_eq!(t.world_position(mover), Some(nes_scene::Vec2::new(10.0, 20.0)));

    // 4. 结构下一帧：process 期 spawn 的子节点本帧不入遍历序（帧 0 日志
    //    无 enter:child）；下一帧帧首落地、阶段 2 即 enter（帧 1 日志恰好
    //    只有 enter:child —— 一次性 spawn 排除干扰）。
    let log = Rc::new(RefCell::new(Vec::new()));
    struct Spawner {
        log: Rc<RefCell<Vec<String>>>,
        done: bool,
    }
    impl SceneObserver for Spawner {
        fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _d: f32) {
            if ctx.name() == "mover" && !self.done {
                self.done = true;
                ctx.spawn_child("child", NodeKind::Node);
                self.log.borrow_mut().push("spawned".into());
            }
        }
        fn on_enter_tree(&mut self, ctx: &mut NodeCtx<'_>) {
            self.log.borrow_mut().push(format!("enter:{}", ctx.name()));
        }
    }
    let mut obs = Spawner { log: log.clone(), done: false };
    let mut t2 = SceneTree::new("root");
    let holder = t2.add_node(t2.root(), "mover", NodeKind::Node2D);
    t2.apply_pending();
    let st0 = t2.tick(1.0 / 60.0, &mut obs); // 帧 0：enter mover + spawn 入 pending
    assert_eq!(
        take_log(&log),
        vec!["enter:root", "enter:mover", "spawned"],
        "spawn 当帧：child 不 enter（结构未落地）"
    );
    assert_eq!(st0.events, 0);
    let st1 = t2.tick(1.0 / 60.0, &mut obs); // 帧 1：帧首落地 + 阶段 2 enter
    assert_eq!(take_log(&log), vec!["enter:child"], "下一帧帧首落地即 enter");
    assert_eq!(st1.events, 1, "Add 落地产生事件");
    assert_eq!(t2.children(holder).len(), 1);
}
