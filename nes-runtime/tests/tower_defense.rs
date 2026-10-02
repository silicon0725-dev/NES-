//! T-TD 契约回归：哨塔防线（S11-2 第三完整项目 —— F-1 多脚本协作）。
//!
//! 场景 = `examples/assets/tower_defense.ron`（四具名 Script 节点：
//! game / spawner / tower_ctl / hud_ui），输入轨迹 =
//! `examples/assets/tower_defense_trace.txt`（两次点击建塔）。
//! 与 headless（S7.3）同一管线逐帧推进，断言**信号纪律**：
//! 金币扣除（buy，唯一写者 game）、生命扣减（leak）、波次推进
//!（waveclear）、敌人死亡奖励（reward）全部经信号落在属主局部。
//!
//! | 编号 | 断言 |
//! |---|---|
//! | T-TD-01 | 帧 100：一次建塔已扣 10 金（gold 20 -> 10，HUD 同步显示）|
//! | T-TD-02 | 帧 900：两次建塔共扣 20 金；击杀有奖励（gold = 20 - 20 + 5*kills，kills >= 4）；漏怪扣生命（lives < 10）；波次推进（wave >= 3）|
//! | T-TD-03 | 双跑逐帧指纹全等（确定性）；HUD 与 game 局部一致（F-1 点读 = 局部值）|

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use nes_render_api::input::{parse_trace, InputEvent};
use nes_runtime::NesRuntime;
use nes_scene::ScriptVm;

/// 输入事件队列是进程级静态 —— 用例串行（与 criterion_headless 同款纪律）。
static TD_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    TD_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn boot() -> (NesRuntime, ScriptVm, Vec<(u64, Vec<InputEvent>)>, f32) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    let trace_text =
        std::fs::read_to_string(root.join("tower_defense_trace.txt")).expect("读轨迹");
    let trace = parse_trace(&trace_text).expect("解析轨迹");
    let mut rt = NesRuntime::open_headless(&root).expect("headless 装配");
    rt.load_scene("tower_defense.ron").expect("装载场景");
    let mut vm = ScriptVm::new();
    let table = rt.resources_mut().clone();
    let asset_root = root.clone();
    let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
        std::fs::read_to_string(asset_root.join(rel)).map_err(|e| e.to_string())
    });
    assert!(issues.is_empty(), "脚本装载缺口：{issues:?}");
    rt.mount_input_view(&mut vm);
    (rt, vm, trace.into_iter().map(|t| (t.frame, t.events)).collect(), 1.0 / 60.0)
}

fn run_frames(
    rt: &mut NesRuntime,
    vm: &mut ScriptVm,
    trace: &[(u64, Vec<InputEvent>)],
    frames: u64,
    delta: f32,
) {
    for f in 0..frames {
        for (fr, evs) in trace {
            if *fr == f {
                for ev in evs {
                    nes_render_wgpu::window::inject_input(*ev);
                }
            }
        }
        let _snap = rt.collect_input();
        rt.emit_input_signals(&_snap);
        let _ = rt.step_headless(delta, vm);
    }
}

fn hud_text(rt: &mut NesRuntime) -> String {
    let tree = rt.tree_mut();
    let hud = tree.find_by_name("hud").expect("hud 节点");
    match tree.prop(hud, "text") {
        Some(nes_scene::Value::Str(s)) => s.clone(),
        other => panic!("hud.text 异常：{other:?}"),
    }
}

fn game_local(vm: &ScriptVm, rt: &mut NesRuntime, name: &str) -> i64 {
    let tree = rt.tree_mut();
    let game = tree.find_by_name("game").expect("game 节点");
    match vm.locals(game).and_then(|l| l.get(name).cloned()) {
        Some(nes_scene::Value::I64(v)) => v,
        other => panic!("game.{name} 异常：{other:?}"),
    }
}

/// T-TD-01：一次点击 -> 一次 buy -> 10 金扣除，HUD 同步。
#[test]
fn t_td_01_buy_deducts_gold_once() {
    let _g = lock();
    let (mut rt, mut vm, trace, delta) = boot();
    run_frames(&mut rt, &mut vm, &trace, 100, delta);
    assert_eq!(game_local(&vm, &mut rt, "gold"), 10, "一次建塔扣 10 金");
    assert_eq!(game_local(&vm, &mut rt, "lives"), 10, "尚无漏怪");
    assert_eq!(game_local(&vm, &mut rt, "wave"), 1, "第一波进行中");
    assert_eq!(
        hud_text(&mut rt),
        "G:10 L:10 W:1 K:0",
        "HUD 点读 game.* 与属主局部一致"
    );
}

/// T-TD-02：全程 900 帧 —— 奖励/漏怪/波次推进全按信号纪律落账。
#[test]
fn t_td_02_full_run_signal_discipline() {
    let _g = lock();
    let (mut rt, mut vm, trace, delta) = boot();
    run_frames(&mut rt, &mut vm, &trace, 900, delta);
    let kills = game_local(&vm, &mut rt, "kills");
    let gold = game_local(&vm, &mut rt, "gold");
    let lives = game_local(&vm, &mut rt, "lives");
    let wave = game_local(&vm, &mut rt, "wave");
    assert!(kills >= 4, "两波合计至少 4 杀（实际 {kills}）");
    assert_eq!(
        gold,
        // 期初 20 - 两座塔 2*10 = 0，故账本 = 每杀 5 金奖励。
        5 * kills,
        "金币账本 = 20 - 两次建塔 20 + 每杀 5 奖励（信号唯一写路径）"
    );
    assert!(lives < 10, "有敌人到底扣生命（实际 lives={lives}）");
    assert!(lives > 0, "两塔防线未破（实际 lives={lives}）");
    assert!(wave >= 3, "波次推进至少两轮（实际 wave={wave}）");
}

/// T-TD-03：同场景同轨迹双跑逐帧指纹全等（S7.3 确定性口径）。
#[test]
fn t_td_03_two_runs_identical() {
    let _g = lock();
    let frames = 900;
    let mut hashes = Vec::new();
    for _ in 0..2 {
        let (mut rt, mut vm, trace, delta) = boot();
        for f in 0..frames {
            for (fr, evs) in &trace {
                if *fr == f {
                    for ev in evs {
                        nes_render_wgpu::window::inject_input(*ev);
                    }
                }
            }
            let snap = rt.collect_input();
            rt.emit_input_signals(&snap);
            let _ = rt.step_headless(delta, &mut vm);
            hashes.push(rt.state_fingerprint(Some(&vm)));
        }
    }
    let half = hashes.len() / 2;
    assert_eq!(hashes[..half], hashes[half..], "双跑逐帧指纹全等");
}
