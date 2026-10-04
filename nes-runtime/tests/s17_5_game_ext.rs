//! S17.4 游戏扩展实战 —— 真游戏宿主装载真扩展的自动化验证。
//!
//! 被测物 = **真实交付物**：两个样例游戏的场景（`first_game.ron` /
//! `dungeon.ron`，真资产 skip-if-missing 惯例）+ 入库真扩展
//!（`Extensions/shake.js`，经 `load_extension_file` 真实读盘路径装载）
//! + 宿主帧惯例（simulate 之后 `update_extensions`，诊断逐行收集）。
//!   headless 直驱（`update_extensions` 快照/队列/混音器三面全部 GPU
//!   无关 —— 扩展面在 headless 下与窗口模式同一条路径）。
//!
//! | 编号 | 断言 |
//! |---|---|
//! | T-GE-01 | Dodge：装载计数 == 1；300 帧零诊断、扩展 faults == 0；敌人 AI（e*_brain 追踪脚本）在 headless 下真实逼近玩家（e3 位移 > 50px），最小距离压进 48px 阈值后相机震动真实发生（cam 偏离基准 > 0.5px）且震后逐位归位（cam 回基准 192,108）|
//! | T-GE-02 | Mini Dungeon：同一份 shake.js（同一文件，零改动）在第二个游戏触发震动 —— 跨游戏通用性的正面证据；同样零诊断零故障 |
//! | T-GE-03 | 坏扩展不挡游戏：装载期语法错误 = 报错 + 缺席，同目录好扩展照常装载（计数 1）、游戏帧循环零诊断、好扩展真实驱动树 |
//!
//! JS 字面量全 ASCII（仓库纪律 —— 本文件不内联 JS，真扩展从盘上装载；
//! T-GE-03 落盘的两份 JS 亦全 ASCII）。输入队列是进程级静态 —— 用例
//! 串行（tower_defense 同款锁纪律）。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use nes_runtime::NesRuntime;
use nes_scene::{NodeKind, ScriptVm, Transform2D};

/// 输入事件队列是进程级静态 —— 用例串行（与 tower_defense 同款纪律）。
static GE_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    GE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 冒烟帧数（与 ext_demo 冒烟口径一致；两游戏的敌人 AI 都在 ~150 帧
/// 内把最小距离压进 48px 阈值，300 帧余量充足）。
const FRAME_COUNT: u64 = 300;

/// 资产根（与游戏宿主同一目录 —— 场景/脚本/扩展都在这儿）。
fn assets_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets")
}

/// 真资产缺席 = skip（字体/用户音乐用例惯例 —— CI/他机安全）。
fn assets_present(root: &Path, scene: &str) -> bool {
    root.join(scene).is_file() && root.join("Extensions/shake.js").is_file()
}

/// 游戏宿主的装载段（与 first_game/dungeon_game 逐字同款路径）：
/// load_scene -> attach_all_with_sources（磁盘读闭包）-> mount_input_view。
fn boot_game(rt: &mut NesRuntime, root: &Path, scene: &str) -> ScriptVm {
    rt.load_scene(scene).expect("装载场景");
    let mut vm = ScriptVm::new();
    let table = rt.resources_mut().clone();
    let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
        std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
    });
    assert!(issues.is_empty(), "script mount gaps: {issues:?}");
    rt.mount_input_view(&mut vm);
    vm
}

/// 宿主帧惯例的一帧：输入收集 -> 信号 -> step_headless（simulate）->
/// update_extensions（S17 帧序契约：扩展在 simulate 之后）。返回本帧
/// 扩展诊断清单。
fn host_frame(rt: &mut NesRuntime, vm: &mut ScriptVm) -> Vec<String> {
    let snap = rt.collect_input();
    let _ = rt.emit_input_signals(&snap);
    let _ = rt.step_headless(1.0 / 60.0, vm);
    rt.update_extensions()
}

/// 节点位置读数（断言辅助）。
fn pos_of(rt: &mut NesRuntime, name: &str) -> (f32, f32) {
    let tree = rt.tree_mut();
    let id = tree
        .find_by_name(name)
        .unwrap_or_else(|| panic!("{name} must exist"));
    let t = tree.local(id).expect("local transform");
    (t.pos.x, t.pos.y)
}

/// T-GE-01：Dodge + shake.js —— 自然逼近触发相机震动，震后归位。
#[test]
fn t_ge_01_dodge_extension_loads_once_and_shake_fires_on_natural_approach() {
    let _g = lock();
    let root = assets_root();
    if !assets_present(&root, "first_game.ron") {
        eprintln!("[skip] first_game.ron or Extensions/shake.js not found");
        return;
    }
    let mut rt = NesRuntime::open_headless(&root).expect("headless setup");
    // 宿主惯例：真实读盘装载（load_extension_file 路径，非内联源码）。
    let id = rt
        .load_extension_file(root.join("Extensions/shake.js"))
        .expect("load shake.js");
    assert_eq!(id, "shake", "extension must self-report id=shake");
    assert_eq!(rt.extension_count(), 1, "load count must be 1");

    let mut vm = boot_game(&mut rt, &root, "first_game.ron");
    let player_start = pos_of(&mut rt, "player");
    let e3_start = pos_of(&mut rt, "e3");
    let cam_base = pos_of(&mut rt, "cam"); // 场景基准 192,108（扩展应原样记录）

    let mut diagnostics: Vec<String> = Vec::new();
    let mut max_dev: f32 = 0.0;
    let mut first_shake_frame: Option<u64> = None;
    let mut restored_after_shake = false;
    for frame in 0..FRAME_COUNT {
        let lines = host_frame(&mut rt, &mut vm);
        diagnostics.extend(lines);
        let (cx, cy) = pos_of(&mut rt, "cam");
        let dev = ((cx - cam_base.0).powi(2) + (cy - cam_base.1).powi(2)).sqrt();
        if dev > max_dev {
            max_dev = dev;
        }
        if dev > 0.5 && first_shake_frame.is_none() {
            first_shake_frame = Some(frame); // 首次触发帧（只记第一次）
        }
        if first_shake_frame.is_some() && dev == 0.0 {
            restored_after_shake = true; // 震动结束后曾逐位回基准
        }
    }

    assert!(
        diagnostics.is_empty(),
        "zero diagnostics expected: {diagnostics:?}"
    );
    assert_eq!(rt.extension_faults(), 0, "extension faults must be 0");
    // 敌人 AI 在 headless 下真实逼近（不是测试摆拍）：e3（最快追踪者）
    // 300 帧位移远超一步（碰撞重置会让它往返，位移 >= 50px 即证）。
    let e3_end = pos_of(&mut rt, "e3");
    let e3_travel =
        ((e3_end.0 - e3_start.0).powi(2) + (e3_end.1 - e3_start.1).powi(2)).sqrt();
    assert!(e3_travel > 50.0, "enemy AI must approach headlessly (e3 travel {e3_travel})");
    assert_eq!(pos_of(&mut rt, "player"), player_start, "player must stay put (no input)");
    // 震动真实发生 + 结束归位：jitter 幅度 3px、每次震动 20 次随机写入，
    // 全部写入 |偏移| <= 0.5px 的概率量级 1e-10 —— 断言如实在此。
    let shake_frame = first_shake_frame.expect("camera shake must fire");
    assert!(max_dev > 0.5, "shake offset must be observable (max dev {max_dev})");
    assert!(
        restored_after_shake,
        "camera must land exactly on base after shake"
    );
    println!(
        "[T-GE-01 observed] shake fired at frame {shake_frame}, max dev {max_dev:.3}px, restored-to-base=true"
    );
}

/// T-GE-02：Mini Dungeon + 同一份 shake.js —— 跨游戏通用性（同一文件
/// 零改动在第二个游戏触发震动）。
#[test]
fn t_ge_02_dungeon_runs_the_same_extension_file() {
    let _g = lock();
    let root = assets_root();
    if !assets_present(&root, "dungeon.ron") {
        eprintln!("[skip] dungeon.ron or Extensions/shake.js not found");
        return;
    }
    let mut rt = NesRuntime::open_headless(&root).expect("headless setup");
    let id = rt
        .load_extension_file(root.join("Extensions/shake.js"))
        .expect("load shake.js");
    assert_eq!(id, "shake");
    assert_eq!(rt.extension_count(), 1);

    let mut vm = boot_game(&mut rt, &root, "dungeon.ron");
    let cam_base = pos_of(&mut rt, "cam");

    let mut diagnostics: Vec<String> = Vec::new();
    let mut max_dev: f32 = 0.0;
    let mut restored_after_shake = false;
    let mut shook = false;
    for _ in 0..FRAME_COUNT {
        let lines = host_frame(&mut rt, &mut vm);
        diagnostics.extend(lines);
        let (cx, cy) = pos_of(&mut rt, "cam");
        let dev = ((cx - cam_base.0).powi(2) + (cy - cam_base.1).powi(2)).sqrt();
        if dev > max_dev {
            max_dev = dev;
        }
        if dev > 0.5 {
            shook = true;
        }
        if shook && dev == 0.0 {
            restored_after_shake = true;
        }
    }

    assert!(
        diagnostics.is_empty(),
        "zero diagnostics expected: {diagnostics:?}"
    );
    assert_eq!(rt.extension_faults(), 0);
    assert!(shook, "same shake.js must fire in the second game (max dev {max_dev})");
    assert!(max_dev > 0.5);
    assert!(restored_after_shake, "camera must land exactly on base after shake");
    println!("[T-GE-02 observed] max dev {max_dev:.3}px, restored-to-base=true");
}

/// T-GE-03 的好扩展（ASCII；普通函数 onUpdate —— 最小驱动面）。
const OK_JS: &str = r#"
nes.registerExtension("ok");
nes.onUpdate(function () {
  var ref = nes.scene.find("player");
  if (ref === null) { return; }
  var p = nes.node.getPos(ref);
  nes.node.setPos(ref, p[0] + 1.0, p[1]);
});
"#;

/// T-GE-03：坏扩展不挡游戏 —— 装载期语法错误 = 报错 + 缺席；宿主扫描
/// 惯例（逐个装载、失败记录继续）下同目录好扩展照常装载并真实驱动树，
/// 帧循环零诊断。
#[test]
fn t_ge_03_bad_extension_file_does_not_block_the_game() {
    let _g = lock();
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("s17_5_bad_ext");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("Extensions")).unwrap();
    // 语法错误（缺右括号）—— 装载必报错。
    std::fs::write(tmp.join("Extensions/broken.js"), "nes.registerExtension(\"broken\"")
        .unwrap();
    std::fs::write(tmp.join("Extensions/ok.js"), OK_JS).unwrap();

    let mut rt = NesRuntime::open_headless(&tmp).expect("headless setup");
    // 宿主扫描惯例（与 first_game/dungeon_game 的 load_extensions 同款
    // 循环）：字典序逐个装载，失败只记录、循环继续。
    let mut files: Vec<PathBuf> = std::fs::read_dir(tmp.join("Extensions"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("js"))
        .collect();
    files.sort();
    let mut loaded = 0usize;
    let mut failed = 0usize;
    for f in &files {
        match rt.load_extension_file(f) {
            Ok(_) => loaded += 1,
            Err(_) => failed += 1,
        }
    }
    assert_eq!(failed, 1, "broken.js must fail to load");
    assert_eq!(loaded, 1, "broken.js must not block ok.js");
    assert_eq!(rt.extension_count(), 1, "only the good extension is registered");

    // 最小游戏树（player/cam/e1 —— shake 语义的名字约定不参与本测，
    // ok.js 只动 player）。e1 摆在玩家旁：真扩展生态里同目录多扩展
    // 共存 —— 这里 ok.js 与树共存即证"游戏照常"。
    {
        let tree = rt.tree_mut();
        let r = tree.root();
        let cam = tree.add_node(r, "cam", NodeKind::Camera2D);
        tree.set_local(cam, Transform2D::from_pos(192.0, 108.0));
        let player = tree.add_node(r, "player", NodeKind::Sprite2D);
        tree.set_local(player, Transform2D::from_pos(16.0, 100.0));
        tree.apply_pending();
    }

    let mut vm = ScriptVm::new();
    let player_start = pos_of(&mut rt, "player");
    for _ in 0..30 {
        let lines = host_frame(&mut rt, &mut vm);
        assert!(lines.is_empty(), "zero diagnostics expected: {lines:?}");
    }
    let player_end = pos_of(&mut rt, "player");
    assert_eq!(
        player_end.0,
        player_start.0 + 30.0,
        "good extension must drive the tree every frame (+1/frame)"
    );
    assert_eq!(rt.extension_faults(), 0);
}
