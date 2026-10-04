//! S17.1 扩展健壮性硬化 —— 三道防线的契约测试。
//!
//! * 防线 1（异常隔离）：update 抛 JS 异常 -> 计数 + 诊断，引擎与其余
//!   扩展照常（`throwing_extension_is_isolated_and_counted`）；
//! * 自动停用：连续 60 帧失败后该扩展不再进帧，其余扩展不受影响
//!   （`sixty_consecutive_faults_disable_the_extension`）；
//! * 防线 2（死循环中断，核心验收）：`while(true)` 在执行预算内被 QuickJS
//!   中断处理器转成 JS 异常，墙钟上界 2s，引擎后续帧照常
//!   （`deadloop_extension_is_interrupted_within_budget_and_engine_continues`）；
//! * 防线 3（内存超限）：hog 脚本不炸进程，行为按实测钉住（"out of memory"
//!   或预算中断 "interrupted"，二者同形态：JS 异常走隔离路径）
//!   （`memory_hog_extension_does_not_kill_engine`）。
//!
//! 正常扩展回归：既有 `extension_loop_moves_the_tree_without_engine_present`
//! （src/extension.rs 内）原样不动；hello.js 语义由 ext_demo 冒烟覆盖。
//! JS 字面量全 ASCII（仓库纪律）。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use nes_runtime::{ExtCapsState, ExtensionManager, NesRuntime};
use nes_scene::{NodeKind, SceneTree, ScriptVm, Transform2D};

/// 每帧抛错的扩展（防线 1 的靶子）。
const THROWER: &str = r#"
nes.registerExtension("thrower");
nes.onUpdate(function () { throw new Error("boom"); });
"#;

/// 健康扩展：每帧把 obj1 平移 (+1, +2)（同轴对照 —— 隔离的正确性证据）。
const SPINNER: &str = r#"
nes.registerExtension("spin");
nes.onUpdate(function () {
  var ref = nes.scene.find("obj1");
  if (ref === null) { return; }
  var p = nes.node.getPos(ref);
  nes.node.setPos(ref, p[0] + 1, p[1] + 2);
});
"#;

/// 死循环扩展（防线 2 的靶子 —— 引擎冻结事故的本体）。
const DEADLOOP_JS: &str = r#"
nes.registerExtension("deadloop");
nes.onUpdate(function () { while (true) { } });
"#;

/// 内存超限扩展（防线 3 的靶子：64 MiB 上限的观测口）。
const HOG: &str = r#"
nes.registerExtension("hog");
var hog = [];
nes.onUpdate(function () { while (true) { hog.push(new Array(1024)); } });
"#;

/// 搭一棵最小树：root + obj1（与 src/extension.rs 内测试同一骨架）。
fn tree_with_obj1() -> SceneTree {
    let mut tree = SceneTree::new("root");
    let root = tree.root();
    let obj = tree.add_node(root, "obj1", NodeKind::Sprite2D);
    tree.set_local(obj, Transform2D::from_pos(10.0, 20.0));
    tree.apply_pending();
    tree
}

/// 推进一帧（快照 -> update -> 写落地；与 NesRuntime::update_extensions 同序）。
fn frame(
    mgr: &mut ExtensionManager,
    caps: &Rc<RefCell<ExtCapsState>>,
    tree: &mut SceneTree,
) -> Vec<String> {
    caps.borrow_mut().refresh(tree, &[], None);
    let messages = mgr.update();
    caps.borrow_mut().apply(tree);
    messages
}

/// 防线 1：异常扩展每帧抛错，50 帧（停用阈值之下 —— 停用语义归下一测试）
/// 后引擎活（健康扩展照常驱动树）、faults 逐帧计满、last_fault 含扩展 id
/// 与异常文本。
#[test]
fn throwing_extension_is_isolated_and_counted() {
    let mut mgr = ExtensionManager::new().unwrap();
    let thrown = mgr.load_source(THROWER, "thrower").unwrap();
    assert_eq!(thrown, "thrower");
    let spun = mgr.load_source(SPINNER, "spin").unwrap();
    assert_eq!(spun, "spin");
    assert_eq!(mgr.extension_count(), 2);

    let mut tree = tree_with_obj1();
    let obj = tree.find_by_name("obj1").unwrap();
    let caps = mgr.caps();
    for frame_no in 0..50 {
        let messages = frame(&mut mgr, &caps, &mut tree);
        assert!(
            !messages.is_empty(),
            "frame {frame_no}: thrower must fault every frame"
        );
        assert!(
            messages.iter().any(|m| m.contains("thrower") && m.contains("boom")),
            "fault message must carry ext id + exception text: {messages:?}"
        );
    }
    assert_eq!(mgr.extension_faults(), 50, "one fault per frame");
    let last = mgr.last_fault().expect("fault recorded");
    assert!(last.contains("thrower"), "ext id missing: {last}");
    assert!(last.contains("boom"), "exception text missing: {last}");

    // 引擎照常：健康扩展 50 帧累计位移 (50, 100)，不受同伴拖累。
    let local = tree.local(obj).unwrap();
    assert_eq!((local.pos.x, local.pos.y), (60.0, 120.0), "50 frames of (+1, +2)");
}

/// 防线 3（自动停用）：连续 60 帧失败 -> 第 60 帧发出停用宣告（faults 行
/// + 停用行两条），此后该扩展不再进帧（零新故障）；共 100 帧后引擎照活、
///   健康扩展全程未停。
#[test]
fn sixty_consecutive_faults_disable_the_extension() {
    let mut mgr = ExtensionManager::new().unwrap();
    mgr.load_source(THROWER, "thrower").unwrap();
    mgr.load_source(SPINNER, "spin").unwrap();

    let mut tree = tree_with_obj1();
    let obj = tree.find_by_name("obj1").unwrap();
    let caps = mgr.caps();
    // 第 1..=59 帧：每帧一条故障行。
    for frame_no in 1..60u32 {
        let messages = frame(&mut mgr, &caps, &mut tree);
        assert_eq!(messages.len(), 1, "frame {frame_no}: {messages:?}");
    }
    // 第 60 帧：故障行 + 停用宣告（恰好两条）。
    let messages = frame(&mut mgr, &caps, &mut tree);
    assert_eq!(messages.len(), 2, "deactivation frame: {messages:?}");
    assert!(
        messages.iter().any(|m| m.contains("thrower") && m.contains("60")),
        "deactivation notice must name the ext and the threshold: {messages:?}"
    );
    assert_eq!(mgr.extension_faults(), 60);

    // 第 61..=100 帧：停用扩展不进帧 —— 零新消息、计数冻结；引擎满 100 帧照活。
    for _ in 0..40 {
        let messages = frame(&mut mgr, &caps, &mut tree);
        assert!(messages.is_empty(), "disabled ext must not run: {messages:?}");
    }
    assert_eq!(mgr.extension_faults(), 60, "frozen after disable");
    // 健康扩展全程未停：100 帧累计位移 (100, 200)。
    let local = tree.local(obj).unwrap();
    assert_eq!((local.pos.x, local.pos.y), (110.0, 220.0));
}

/// 防线 2（核心验收）：死循环扩展在真引擎里被预算中断 —— update 调用墙钟
/// < 2s、fault 计数 > 0、后续帧照常推进。
#[test]
fn deadloop_extension_is_interrupted_within_budget_and_engine_continues() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("s17_ext_hardening_deadloop");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("deadloop.js"), DEADLOOP_JS).unwrap();

    let mut rt = NesRuntime::open_headless(&root).expect("headless engine");
    let id = rt.load_extension_file(root.join("deadloop.js")).unwrap();
    assert_eq!(id, "deadloop");

    // 触发 update：while(true) 必须在预算内被掐断（2s 上界远宽于 50ms 预算）。
    let start = Instant::now();
    let messages = rt.update_extensions();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "deadloop must be cut within budget, took {elapsed:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("deadloop") && m.contains("interrupted")),
        "interrupt must surface as a fault: {messages:?}"
    );
    assert!(rt.extension_faults() >= 1, "fault counter must move");
    assert!(
        rt.last_fault().expect("last_fault").contains("interrupted"),
        "last_fault must carry the interrupt marker"
    );

    // 引擎后续帧照常：simulate + 扩展推进循环无恙，死循环扩展每帧照常被打断。
    let mut vm = ScriptVm::new();
    for _ in 0..3 {
        let _ = rt.step_headless(1.0 / 60.0, &mut vm);
        let messages = rt.update_extensions();
        assert!(
            messages.iter().any(|m| m.contains("interrupted")),
            "engine must keep stepping and interrupting: {messages:?}"
        );
    }
    assert!(rt.extension_faults() >= 4, "each frame counts one fault");
    // 树对象健康（引擎状态可读可写 —— 冻结事故的反面证据）。
    {
        let tree = rt.tree_mut();
        let root_node = tree.root();
        assert_eq!(tree.name(root_node), Some("root"));
    }
}

/// 防线 3：内存超限扩展不炸进程、不挂死 —— 超限以 JS 异常浮出（实测行为：
/// "out of memory"，慢机器上可能是预算中断 "interrupted"；二者都走隔离
/// 路径），引擎（管理器）此后仍可正常工作。
#[test]
fn memory_hog_extension_does_not_kill_engine() {
    let mut mgr = ExtensionManager::new().unwrap();
    let id = mgr.load_source(HOG, "hog").unwrap();
    assert_eq!(id, "hog");

    let mut tree = tree_with_obj1();
    let caps = mgr.caps();
    let mut observed = String::new();
    for frame_no in 0..5 {
        let start = Instant::now();
        let messages = frame(&mut mgr, &caps, &mut tree);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "frame {frame_no}: hog must stop promptly, took {elapsed:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("hog")),
            "frame {frame_no}: hog fault must surface: {messages:?}"
        );
        if observed.is_empty() {
            observed = messages.join(" | ");
        }
    }
    assert!(mgr.extension_faults() >= 5, "every hog frame counts");
    println!("[S17.1 memory-hog observed] {observed}");

    // 同一进程内的全新管理器照常工作（炸进程 = 本测试直接崩）。
    let mut fresh = ExtensionManager::new().unwrap();
    let id = fresh.load_source(SPINNER, "fallback").unwrap();
    assert_eq!(id, "spin");
}
