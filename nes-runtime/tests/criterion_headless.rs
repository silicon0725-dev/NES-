//! T-HR 契约回归：headless / 确定性运行时（S7.3）。
//!
//! **全部用例无 GPU 依赖**（headless 装配不碰 wgpu-native —— 这是
//! 本里程碑的架构主张本身：确定性不靠渲染端背书）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-HR-01 | 同场景 + 同输入轨迹跑两次：**逐帧指纹 + 轨迹指纹全等** |
//! | T-HR-02 | 空输入：确定（且两次运行跨独立运行时实例） |
//! | T-HR-03 | 键盘轨迹：确定；且**不等于**空输入的指纹（不是常数哈希） |
//! | T-HR-04 | 鼠标轨迹：确定 |
//! | T-HR-05 | 文本轨迹：确定 |
//! | T-HR-06 | 信号级联（链式 emit + 双订阅者）：确定 —— S7.1 冻结的 BFS 泵序是确定性契约 |
//! | T-HR-07 | 状态混合演化（输入驱动局部 + 持续浮点漂移）：确定 |
//! | T-HR-08 | 差分口径：同场景不同轨迹，分歧帧之前全等、第一处差异定位到帧 |

use std::sync::{Mutex, MutexGuard};

use nes_render_api::input::parse_trace;
use nes_runtime::headless::run;
use nes_runtime::HeadlessReport;

/// 输入事件队列是进程级静态 —— 用例串行（与 criterion_input 同款纪律）。
static HR_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    HR_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 测试根目录（每用例独立子目录，M5 §3.1 测试层隔离口径）。
fn root(tag: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_headless")
        .join(tag)
}

/// 写场景文件，返回（资产根, 场景相对路径）。
fn write_scene(tag: &str, scene: &str) -> (std::path::PathBuf, String) {
    let r = root(tag);
    let _ = std::fs::remove_dir_all(&r);
    std::fs::create_dir_all(&r).unwrap();
    std::fs::write(r.join("main.ron"), scene).unwrap();
    (r, "main.ron".to_string())
}

/// 场景骨架 + 节点列表（节点体是 RON 片段）。
fn scene_of(nodes: &[String]) -> String {
    let body = nodes
        .iter()
        .map(|n| format!("            {n},"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Scene(\n    version: 1,\n    resources: [],\n    root: Node(\n        name: \"main\",\n        kind: \"Node\",\n        children: [\n{body}\n        ],\n    ),\n)"
    )
}

/// 普通节点。
fn node(name: &str, kind: &str) -> String {
    format!("Node(name: \"{name}\", kind: \"{kind}\", children: [])")
}

/// 脚本节点（source 单行、`;` 分句 —— 避开 RON 换行转义噪音；
/// `\"` 为 **RON 层**转义，原样落盘）。
fn script(name: &str, src: &str) -> String {
    let escaped = src.replace('"', "\\\"");
    format!(
        "Node(name: \"{name}\", kind: \"Script\", props: {{ \"source\": \"{escaped}\" }}, children: [])"
    )
}

/// 双跑恒等断言。
fn assert_two_runs_identical(
    tag: &str,
    scene: &str,
    trace: &str,
    frames: u64,
) -> (HeadlessReport, HeadlessReport) {
    let (root, rel) = write_scene(tag, scene);
    let trace = parse_trace(trace).expect("轨迹");
    let a = run(&root, &rel, &trace, frames, 1.0 / 60.0).expect("跑 A");
    let b = run(&root, &rel, &trace, frames, 1.0 / 60.0).expect("跑 B");
    assert_eq!(a.frame_hashes, b.frame_hashes, "{tag}：逐帧指纹全等");
    assert_eq!(a.trace_hash, b.trace_hash, "{tag}：轨迹指纹全等");
    (a, b)
}

/// T-HR-01：核心 —— 同场景 + 同轨迹，两次运行逐帧全等。覆盖信号入口、
/// 按键探针、局部计数器（语义状态的主要面）。
#[test]
fn t_hr_01_same_scene_same_trace_identical() {
    let _g = lock();
    let scene = scene_of(&[
        node("sp", "Node2D"),
        script(
            "brain",
            "on \"input/key_down\" { n = n + 1; if arg == \"W\" { sp.pos += (3.0, 0.0) } }",
        ),
        script("hold", "on \"go\" { if key(\"W\") { sp.pos += (0.0, 1.0) } }"),
    ]);
    let (rep, _) = assert_two_runs_identical(
        "hr01",
        &scene,
        "0 key_down W\n5 key_up W\n6 key_down ArrowRight\n9 key_up ArrowRight",
        12,
    );
    assert!(
        rep.frame_hashes.windows(2).any(|w| w[0] != w[1]),
        "状态在动（非常数哈希）"
    );
}

/// T-HR-02：空输入（无轨迹）—— 纯场景驱动确定。
#[test]
fn t_hr_02_empty_input_deterministic() {
    let _g = lock();
    let scene = scene_of(&[
        script("drift", "every { this.pos += (1.0, 2.0) }"),
        script("counter", "every { c = c + 1 }"),
        node("anchor", "Node2D"),
    ]);
    assert_two_runs_identical("hr02", &scene, "", 30);
}

/// T-HR-03：键盘轨迹确定；且与空输入轨迹**不同**（哈希真反映了输入）。
#[test]
fn t_hr_03_keyboard_trace_deterministic_and_distinct() {
    let _g = lock();
    let scene = scene_of(&[
        node("p", "Node2D"),
        script("b", "on \"input/key_down\" { p.pos += (1.0, 1.0) }"),
        script("h", "on \"go\" { if key(\"W\") { p.pos += (0.0, 5.0) } }"),
    ]);
    let trace = "0 key_down W\n4 key_up W\n5 key_down W\n9 key_up W";
    let (with_keys, _) = assert_two_runs_identical("hr03", &scene, trace, 12);
    // 同场景空轨迹：指纹必须不同（按住键进语义状态 + 信号命中不同）。
    let (root, rel) = write_scene("hr03", &scene);
    let empty = run(&root, &rel, &[], 12, 1.0 / 60.0).expect("空轨迹");
    assert_ne!(with_keys.trace_hash, empty.trace_hash, "输入真的影响状态");
}

/// T-HR-04：鼠标轨迹（移动 + 按钮脉冲 + resize）确定。
#[test]
fn t_hr_04_mouse_trace_deterministic() {
    let _g = lock();
    let scene = scene_of(&[
        node("ui", "Node2D"),
        script("b", "on \"input/mouse_down\" { clicks = clicks + 1 }"),
        script("m", "on \"input/mouse_move\" { ui.pos = arg }"),
    ]);
    assert_two_runs_identical(
        "hr04",
        &scene,
        "0 mouse_move 10 20\n1 mouse_move 40 60\n2 mouse_down left mouse_up left\n3 resize 800 600\n6 mouse_move -5 -7",
        10,
    );
}

/// T-HR-05：文本轨迹（char 码点序列）确定。
#[test]
fn t_hr_05_text_trace_deterministic() {
    let _g = lock();
    let scene = scene_of(&[
        node("line", "Node"),
        script("b", "on \"input/text\" { line.note = arg; line.visible = !line.visible }"),
        script("c", "on \"input/text\" { n = len(arg) + n }"),
    ]);
    assert_two_runs_identical(
        "hr05",
        &scene,
        "0 char 104 char 105\n1 char 13\n2 char 97 char 98 char 99",
        6,
    );
}

/// T-HR-06：信号级联确定 —— S7.1 冻结的 BFS 泵序/级联/Cmd 微批次在
/// 确定性口径下的直接兑现（双订阅者 = 路由注册序参与哈希）。
#[test]
fn t_hr_06_signal_cascade_deterministic() {
    let _g = lock();
    let scene = scene_of(&[
        node("a", "Node2D"),
        script("s1", "on \"input/key_down\" { a.pos += (1.0, 0.0); emit \"chain2\" 0 }"),
        script("s2", "on \"chain2\" { a.pos += (0.0, 1.0); emit \"chain3\" 0 }"),
        script("s3", "on \"chain3\" { depth = depth + 1 }"),
        script("s4", "on \"chain3\" { depth2 = depth2 + 2 }"),
    ]);
    let (rep, _) = assert_two_runs_identical(
        "hr06",
        &scene,
        "0 key_down Space\n1 key_down Space\n2 key_down Q",
        6,
    );
    assert!(rep.frame_hashes.windows(2).any(|w| w[0] != w[1]));
}

/// T-HR-07：状态混合演化确定 —— 输入驱动局部 + 持续漂移 + 多脚本并存
///（f32 位形逐帧进哈希：浮点累积的确定性直接受检）。
#[test]
fn t_hr_07_spawn_remove_deterministic() {
    let _g = lock();
    let scene = scene_of(&[
        node("holder", "Node2D"),
        script(
            "spawner",
            "on \"input/key_down\" { holder.pos += (2.0, 0.0); if arg == \"Space\" { spawn = spawn + 1 } }",
        ),
        script("mover", "every { this.pos += (0.5, 0.5) }"),
    ]);
    let (rep, _) = assert_two_runs_identical(
        "hr07",
        &scene,
        "0 key_down Space\n1 key_down Enter\n2 key_down Space\n5 key_down Escape",
        9,
    );
    assert!(rep.frame_hashes.windows(2).any(|w| w[0] != w[1]));
    assert_eq!(rep.frame_hashes.len(), 9);
}

/// T-HR-08：轨迹差分口径 —— 同场景**不同**轨迹，逐帧指纹在分歧帧之前
/// 全等、之后可以分岔（第一处差异定位到帧 —— Scratch 差分执行的同一条
/// 口径）。
#[test]
fn t_hr_08_first_divergence_locates_frame() {
    let _g = lock();
    let scene = scene_of(&[
        node("p", "Node2D"),
        script("b", "on \"input/key_down\" { p.pos += (1.0, 0.0) }"),
        script("h", "on \"go\" { if key(\"W\") { p.pos += (0.0, 2.0) } }"),
    ]);
    let a = parse_trace("0 key_down W\n3 key_up W").unwrap();
    let b = parse_trace("0 key_down W\n4 key_up W").unwrap(); // 只差一帧的松开时机
    let (root, rel) = write_scene("hr08", &scene);
    let ra = run(&root, &rel, &a, 8, 1.0 / 60.0).unwrap();
    let rb = run(&root, &rel, &b, 8, 1.0 / 60.0).unwrap();
    let first = ra
        .frame_hashes
        .iter()
        .zip(&rb.frame_hashes)
        .position(|(x, y)| x != y)
        .expect("必然分岔");
    assert_eq!(first, 3, "松开时机差在第 3 帧首次可见（A 已松、B 仍按）");
    assert!(ra.frame_hashes[..first] == rb.frame_hashes[..first], "分歧前全等");
}

/// T-GP-01（S7.4 首个真实项目）：**Dodge** 游戏在 headless 下的真实
/// 玩法闭环 —— 静止玩家被三台追踪者追上 3 次 → HUD 变 "LOSE"；带轨迹
/// 两次运行逐帧全等（真实项目 = 确定性契约的最大用例）。
#[test]
fn t_gp_01_dodge_gameplay_and_determinism() {
    let _g = lock();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");

    // 玩法闭环：无输入跑 1500 帧 —— 追踪者必然追上静止玩家 3 次。
    let mut rt = nes_runtime::NesRuntime::open_headless(&root).expect("headless 装配");
    rt.load_scene("first_game.ron").expect("加载");
    let mut vm = nes_scene::ScriptVm::new();
    {
        let table = rt.resources_mut().clone();
        let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
            std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
        });
        assert!(issues.is_empty(), "{issues:?}");
    }
    rt.mount_key_probe(&mut vm);
    let mut lose_at = None;
    for f in 0..1500u64 {
        let snap = rt.collect_input();
        let _ = rt.emit_input_signals(&snap);
        let _ = rt.step_headless(1.0 / 60.0, &mut vm); // 内建 tick（S8.1）
        if f % 30 == 0 {
            let hud = {
                let tree = rt.tree_mut();
                tree.find(&nes_scene::NodePath::parse("/main/hud").unwrap())
                    .and_then(|n| tree.prop(n, "text").cloned())
            };
            if let Some(nes_scene::Value::Str(s)) = hud {
                if s.contains("LOSE") {
                    lose_at = Some(f);
                    break;
                }
            }
        }
    }
    assert!(lose_at.is_some(), "静止玩家在 1500 帧内必然 LOSE（追上 3 次）");

    // 确定性：仓库内的游戏 + 仓库内的轨迹，两次运行全等。
    let trace_text =
        std::fs::read_to_string(root.join("first_game_trace.txt")).expect("读轨迹");
    let trace = nes_render_api::input::parse_trace(&trace_text).expect("轨迹");
    let a = nes_runtime::headless::run(&root, "first_game.ron", &trace, 600, 1.0 / 60.0)
        .expect("跑 A");
    let b = nes_runtime::headless::run(&root, "first_game.ron", &trace, 600, 1.0 / 60.0)
        .expect("跑 B");
    assert_eq!(a.frame_hashes, b.frame_hashes, "游戏逐帧全等");
    assert_eq!(a.trace_hash, b.trace_hash);
}

/// T-ABI-01（S8.0 兼容压力基线）：**游戏级 ABI** —— 仓库冻结的
/// Dodge 基线（场景 + 轨迹 + 期望哈希）跑 600 帧比对指纹。VM/Scene/
/// Signal 的任何语义变更都会在此炸响 —— 这是"改引擎必过"的游戏级
/// 回归（区别于单元契约：它压的是**合取语义**）。
///
/// 基线更新流程（有意变更语义时）：重跑 CLI 生成新哈希 -> 人工确认
/// diff 合理（评审对应里程碑文档）-> 更新 expected_hash.txt。
#[test]
fn t_abi_01_dodge_baseline() {
    let _g = lock();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/regression/dodge");
    let trace_text = std::fs::read_to_string(dir.join("trace.txt")).expect("读轨迹");
    let trace = nes_render_api::input::parse_trace(&trace_text).expect("轨迹");
    let expected: String = std::fs::read_to_string(dir.join("expected_hash.txt"))
        .expect("读期望哈希")
        .trim()
        .to_string();
    let report = nes_runtime::headless::run(&dir, "scene.ron", &trace, 600, 1.0 / 60.0)
        .expect("基线运行");
    let actual = format!("trace hash {:016x}", report.trace_hash);
    assert_eq!(
        actual, expected,
        "Dodge 基线指纹漂移 —— VM/Scene/Signal 语义变了？\
         若为有意变更：评审里程碑文档后更新 expected_hash.txt"
    );
    assert_eq!(report.frame_hashes.len(), 600);
}

// ---------------------------------------------------------------- S8.1 游戏节拍

/// T-LP-01：**内建 tick** —— 每帧恰一次、载荷 = 树帧号（arg 可作帧计数，
/// Dodge referee 已用）；宿主零手发。
#[test]
fn t_lp_01_builtin_tick_once_per_frame_with_frame_no() {
    let _g = lock();
    let (root, rel) = write_scene(
        "lp01",
        &scene_of(&[script(
            "clock",
            "init { last = -1; hits = 0 }\non \"tick\" { last = arg; hits = hits + 1 }",
        )]),
    );
    let mut rt = nes_runtime::NesRuntime::open_headless(&root).unwrap();
    rt.load_scene(&rel).unwrap();
    let mut vm = nes_scene::ScriptVm::new();
    let table = rt.resources_mut().clone();
    let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
        std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
    });
    assert!(issues.is_empty(), "{issues:?}");
    for _ in 0..5 {
        let _ = rt.collect_input();
        let _ = rt.step_headless(1.0 / 60.0, &mut vm);
    }
    let clock = vm
        .locals(
            rt.tree_mut()
                .find(&nes_scene::NodePath::parse("/main/clock").unwrap())
                .unwrap(),
        )
        .expect("局部");
    assert_eq!(clock.get("hits"), Some(&nes_scene::Value::I64(5)), "每帧恰一次");
    assert_eq!(clock.get("last"), Some(&nes_scene::Value::I64(4)), "载荷 = 树帧号（0 基）");
}

/// T-LP-02：**固定步长蓄步器** —— 快帧 0 步（余量跨帧）、慢帧补步、
/// `every` 的 arg 恒 = 固定步长（确定性模拟口径）、螺旋钳制如实计数。
#[test]
fn t_lp_02_fixed_step_accumulator() {
    let _g = lock();
    let (root, rel) = write_scene(
        "lp02",
        &scene_of(&[
            node("sp", "Node2D"),
            script("drift", "init { n = 0; d = 0.0 }\nevery { n = n + 1; d = arg; this.pos = xy(0.0, 0.0) }"),
        ]),
    );
    let mut rt = nes_runtime::NesRuntime::open_headless(&root).unwrap();
    rt.load_scene(&rel).unwrap();
    let mut vm = nes_scene::ScriptVm::new();
    let table = rt.resources_mut().clone();
    let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
        std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
    });
    assert!(issues.is_empty(), "{issues:?}");
    let sp = rt
        .tree_mut()
        .find(&nes_scene::NodePath::parse("/main/drift").unwrap())
        .unwrap();

    rt.set_fixed_step(1.0 / 60.0);
    // 帧序列（变帧率）：1/30（=2 步）、1/120（=0 步，余量进位）、
    // 1/120 + 1/120（各 0 或合并出 1 步 —— 蓄步器数学决定）、大帧（钳制）。
    let s1 = rt.step_headless(1.0 / 30.0, &mut vm);
    assert_eq!(s1, 2, "慢帧补两步");
    let s2 = rt.step_headless(1.0 / 120.0, &mut vm);
    assert_eq!(s2, 0, "快帧零步（余量 1/120 进位）");
    let s3 = rt.step_headless(1.0 / 120.0, &mut vm);
    assert_eq!(s3, 1, "进位合并出一步");
    let drift = vm.locals(sp).unwrap();
    assert_eq!(drift.get("n"), Some(&nes_scene::Value::I64(3)), "恰三步");
    assert_eq!(
        drift.get("d"),
        Some(&nes_scene::Value::F32(1.0 / 60.0)),
        "every 的 arg 恒 = 固定步长"
    );

    // 螺旋钳制：一帧 2 秒（步长 0.25 —— 二进制精确整除得 8 步）->
    // 钳到 5，丢弃 3 步并计数。（重设步长 = 清蓄步余量，断言不受
    // 前段进位的浮点尾差影响；1/60 类步长的除法本身有尾差。）
    rt.set_fixed_step(0.25);
    let before = rt.steps_dropped();
    let s4 = rt.step_headless(2.0, &mut vm);
    assert_eq!(s4, 5, "钳制到 5 步");
    assert_eq!(rt.steps_dropped() - before, 3, "丢弃 3 步如实计数");
}

/// T-LP-03：未设固定步长 = 宿主纪律模式（帧 delta 原样一步 —— 既有
/// 语义不变；every 的 arg 跟随帧 delta）。
#[test]
fn t_lp_03_default_host_discipline_unchanged() {
    let _g = lock();
    let (root, rel) = write_scene(
        "lp03",
        &scene_of(&[script("drift", "init { d = 0.0 }\nevery { d = arg; this.pos = xy(0.0, 0.0) }")]),
    );
    let mut rt = nes_runtime::NesRuntime::open_headless(&root).unwrap();
    rt.load_scene(&rel).unwrap();
    let mut vm = nes_scene::ScriptVm::new();
    let table = rt.resources_mut().clone();
    let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
        std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
    });
    assert!(issues.is_empty(), "{issues:?}");
    let drift = rt
        .tree_mut()
        .find(&nes_scene::NodePath::parse("/main/drift").unwrap())
        .unwrap();
    let steps = rt.step_headless(0.25, &mut vm); // 任意帧 delta
    assert_eq!(steps, 1, "原样一步");
    assert_eq!(
        vm.locals(drift).unwrap().get("d"),
        Some(&nes_scene::Value::F32(0.25)),
        "arg = 帧 delta（宿主纪律）"
    );
}

/// T-GP-02（S8.2 移植压力测试）：**Mini Dungeon** 玩法闭环 + 确定性 ——
/// 静止玩家被追满 3 次 → LOSE；带轨迹（走位+射击）双跑全等且 ≠ 静止。
#[test]
fn t_gp_02_dungeon_gameplay_and_determinism() {
    let _g = lock();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");

    // 玩法闭环：无输入 —— 三台追踪者必追满 3 次。
    let mut rt = nes_runtime::NesRuntime::open_headless(&root).unwrap();
    rt.load_scene("dungeon.ron").unwrap();
    let mut vm = nes_scene::ScriptVm::new();
    let table = rt.resources_mut().clone();
    let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
        std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
    });
    assert!(issues.is_empty(), "{issues:?}");
    rt.mount_key_probe(&mut vm);
    let mut lose_at = None;
    for f in 0..2000u64 {
        let _ = rt.collect_input();
        let _ = rt.step_headless(1.0 / 60.0, &mut vm);
        if f % 30 == 0 {
            let hud = {
                let tree = rt.tree_mut();
                tree.find(&nes_scene::NodePath::parse("/main/hud").unwrap())
                    .and_then(|n| tree.prop(n, "text").cloned())
            };
            if let Some(nes_scene::Value::Str(s)) = hud {
                if s.contains("LOSE") {
                    lose_at = Some(f);
                    break;
                }
            }
        }
    }
    assert!(lose_at.is_some(), "静止玩家 2000 帧内必 LOSE");

    // 确定性 + 输入效应：走位射击轨迹双跑全等，且 ≠ 静止。
    let trace_text = std::fs::read_to_string(root.join("dungeon_trace.txt")).expect("读轨迹");
    let trace = nes_render_api::input::parse_trace(&trace_text).expect("轨迹");
    let a = nes_runtime::headless::run(&root, "dungeon.ron", &trace, 900, 1.0 / 60.0).unwrap();
    let b = nes_runtime::headless::run(&root, "dungeon.ron", &trace, 900, 1.0 / 60.0).unwrap();
    assert_eq!(a.frame_hashes, b.frame_hashes, "双跑全等");
    assert_eq!(a.trace_hash, b.trace_hash);
    let static_run =
        nes_runtime::headless::run(&root, "dungeon.ron", &[], 900, 1.0 / 60.0).unwrap();
    assert_ne!(a.trace_hash, static_run.trace_hash, "输入真的影响状态");
}
