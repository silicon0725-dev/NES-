//! T-TDS 语义回归：哨塔防线（形态参考 criterion_headless.rs 中
//! T-FARM-02 / T-GP-02 的玩法闭环测试）。
//!
//! 装载 `examples/assets/tower_defense.ron` 场景 + 建塔输入轨迹
//! （`tower_defense_trace.txt`，两次点击建塔），逐帧推进并断言
//! **游戏语义不变量**（语义断言，不是指纹比对）：
//!
//!   - 金币扣除与奖励数额（buy = -10 / reward = +5，账本方程精确成立）
//!   - 生命扣减（漏怪 -1，且不破防）
//!   - 波次推进（wave 单调不减，且随清波推进）
//!   - 存活 / 失败条件（双塔存活 lives > 0；零塔必 LOSE）
//!
//! 字符串字面量只用 ASCII（Windows E0765 教训）；注释用中文。

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use nes_render_api::input::{parse_trace, InputEvent};
use nes_runtime::NesRuntime;
use nes_scene::{NodePath, ScriptVm, Value};

/// 输入事件队列是进程级静态 —— 用例串行（与既有测试同款纪律）。
static TDS_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    TDS_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 装配：headless 装载 tower_defense 场景 + 挂脚本 + 输入轨迹。
fn boot() -> (NesRuntime, ScriptVm, Vec<(u64, Vec<InputEvent>)>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    let trace_text =
        std::fs::read_to_string(root.join("tower_defense_trace.txt")).expect("read trace");
    let trace = parse_trace(&trace_text).expect("parse trace");
    let mut rt = NesRuntime::open_headless(&root).expect("headless boot");
    rt.load_scene("tower_defense.ron").expect("load scene");
    let mut vm = ScriptVm::new();
    let table = rt.resources_mut().clone();
    let asset_root = root.clone();
    let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
        std::fs::read_to_string(asset_root.join(rel)).map_err(|e| e.to_string())
    });
    assert!(issues.is_empty(), "script attach issues: {issues:?}");
    rt.mount_input_view(&mut vm);
    (rt, vm, trace.into_iter().map(|t| (t.frame, t.events)).collect())
}

/// 推进 [from, from + frames) 帧：按绝对帧号注入轨迹事件 +
/// 内建 tick（语义主循环）。分段推进时帧号不重置。
fn run_frames(
    rt: &mut NesRuntime,
    vm: &mut ScriptVm,
    trace: &[(u64, Vec<InputEvent>)],
    from: u64,
    frames: u64,
) {
    for f in from..from + frames {
        for (fr, evs) in trace {
            if *fr == f {
                for ev in evs {
                    nes_render_wgpu::window::inject_input(*ev);
                }
            }
        }
        let snap = rt.collect_input();
        let _ = rt.emit_input_signals(&snap);
        let _ = rt.step_headless(1.0 / 60.0, vm);
    }
}

/// 读 game 脚本节点的局部整数（gold/lives/wave/kills 唯一写者）。
fn game_local(vm: &ScriptVm, rt: &mut NesRuntime, name: &str) -> i64 {
    let tree = rt.tree_mut();
    let game = tree.find(&NodePath::parse("/main/game").unwrap()).unwrap();
    match vm.locals(game).and_then(|l| l.get(name).cloned()) {
        Some(Value::I64(v)) => v,
        other => panic!("game.{name} unexpected: {other:?}"),
    }
}

/// 读 HUD 文本（hud_ui 每帧点读 game.* 渲染 —— 双写一致性的观察面）。
fn hud_text(rt: &mut NesRuntime) -> String {
    let tree = rt.tree_mut();
    let hud = tree.find(&NodePath::parse("/main/hud").unwrap()).unwrap();
    match tree.prop(hud, "text") {
        Some(Value::Str(s)) => s.clone(),
        other => panic!("hud.text unexpected: {other:?}"),
    }
}

/// 数可见塔数（点击建塔的实体落点）。
fn visible_towers(rt: &mut NesRuntime) -> usize {
    let tree = rt.tree_mut();
    let towers = tree.find(&NodePath::parse("/main/towers").unwrap()).unwrap();
    tree.children(towers)
        .iter()
        .filter(|&&h| tree.prop(h, "visible") == Some(&Value::Bool(true)))
        .count()
}

/// 逐帧不变量（金币非负、生命区间、波次单调不减）。
fn sample(vm: &ScriptVm, rt: &mut NesRuntime, last_wave: &mut i64) {
    let gold = game_local(vm, rt, "gold");
    let lives = game_local(vm, rt, "lives");
    let wave = game_local(vm, rt, "wave");
    let kills = game_local(vm, rt, "kills");
    assert!(gold >= 0, "gold must never go negative: {gold}");
    assert!((0..=10).contains(&lives), "lives must stay in 0..=10: {lives}");
    assert!(kills >= 0, "kills must never go negative: {kills}");
    assert!(wave >= *last_wave, "wave must be monotonic: {wave} < {last_wave}");
    *last_wave = wave;
}

/// T-TDS-01：初始状态 —— 尚未建塔：期初 20 金 / 10 生命 / 第 1 波 / 0 杀，
/// HUD 与 game 局部逐字一致（点读 = 局部值）。
#[test]
fn t_tds_01_initial_state() {
    let _g = lock();
    let (mut rt, mut vm, trace) = boot();
    run_frames(&mut rt, &mut vm, &trace, 0, 5);
    assert_eq!(game_local(&vm, &mut rt, "gold"), 20, "initial gold");
    assert_eq!(game_local(&vm, &mut rt, "lives"), 10, "initial lives");
    assert_eq!(game_local(&vm, &mut rt, "wave"), 1, "initial wave");
    assert_eq!(game_local(&vm, &mut rt, "kills"), 0, "initial kills");
    assert_eq!(
        hud_text(&mut rt),
        "GOLD:20 LIVES:10 WAVE:1 KILLS:0",
        "HUD mirrors game locals exactly"
    );
    assert_eq!(visible_towers(&mut rt), 0, "no towers before first click");
}

/// T-TDS-02：一次建塔恰好扣 10 金 —— 轨迹首击在帧 30/31（按下+抬起），
/// 帧 100 时 gold = 20 - 10 = 10 且只扣一次；尚无漏怪（lives = 10）。
#[test]
fn t_tds_02_first_tower_costs_exactly_ten() {
    let _g = lock();
    let (mut rt, mut vm, trace) = boot();
    run_frames(&mut rt, &mut vm, &trace, 0, 100);
    assert_eq!(game_local(&vm, &mut rt, "gold"), 10, "one buy = -10 gold");
    assert_eq!(game_local(&vm, &mut rt, "lives"), 10, "no leaks yet with one tower");
    assert_eq!(game_local(&vm, &mut rt, "wave"), 1, "wave 1 in progress");
    assert_eq!(
        hud_text(&mut rt),
        "GOLD:10 LIVES:10 WAVE:1 KILLS:0",
        "HUD mirrors game locals"
    );
    assert_eq!(visible_towers(&mut rt), 1, "exactly one tower placed");
}

/// T-TDS-03：全程 900 帧（两座塔）—— 语义不变量全量受检：
///   - 金币账本方程：gold = 20 - 10*塔数 + 5*击杀（奖励数额精确 +5）
///   - 生命扣减真实发生但防线未破（0 < lives < 10）
///   - 波次单调推进至少两轮（wave >= 3）
///   - HUD 与 game 局部一致；塔池恰两座可见
#[test]
fn t_tds_03_full_run_semantic_invariants() {
    let _g = lock();
    let (mut rt, mut vm, trace) = boot();
    let mut last_wave = 1;
    let mut mid_sampled = false;
    // 分段推进，中途也受不变量约束（不是只看终点）。
    for seg in 0..3u64 {
        run_frames(&mut rt, &mut vm, &trace, seg * 300, 300);
        sample(&vm, &mut rt, &mut last_wave);
        if !mid_sampled {
            mid_sampled = true;
            // 帧 600 前只有第一座塔：账本至多含一次 -10 与奖励。
            let gold = game_local(&vm, &mut rt, "gold");
            let kills = game_local(&vm, &mut rt, "kills");
            assert!(gold <= 20 - 10 + 5 * kills, "accounting must hold mid-run");
        }
    }
    let kills = game_local(&vm, &mut rt, "kills");
    let gold = game_local(&vm, &mut rt, "gold");
    let lives = game_local(&vm, &mut rt, "lives");
    let wave = game_local(&vm, &mut rt, "wave");
    assert!(kills >= 4, "two waves must yield at least 4 kills: {kills}");
    // 账本：期初 20 - 两座塔共扣 2*10 = 0，故余额 = 每杀 5 金奖励。
    assert_eq!(gold, 5 * kills, "ledger: 20 - two towers (2*10) + 5 per kill");
    assert!(lives < 10, "some enemy must leak past two towers: lives={lives}");
    assert!(lives > 0, "two-tower line must survive: lives={lives}");
    assert!(wave >= 3, "wave must advance at least twice: wave={wave}");
    assert_eq!(visible_towers(&mut rt), 2, "exactly two towers placed");
    assert_eq!(
        hud_text(&mut rt),
        format!("GOLD:{gold} LIVES:{lives} WAVE:{wave} KILLS:{kills}"),
        "HUD mirrors game locals"
    );
    // HUD 恒不显示失败态（存活条件）。
    assert!(!hud_text(&mut rt).contains("LOSE"), "must not LOSE with two towers");
}

/// T-TDS-04：失败条件 —— 零塔（空输入）跑 3000 帧：无杀无购（gold = 20、
/// kills = 0），敌人成批漏到底，lives 扣至 0，HUD 转 "LOSE"（存活条件
/// 的反例面：失败语义必须可达）。
#[test]
fn t_tds_04_lose_without_towers() {
    let _g = lock();
    let (mut rt, mut vm, _trace) = boot();
    let mut last_wave = 1;
    let mut lose = false;
    for _ in 0..3000u64 {
        let snap = rt.collect_input();
        let _ = rt.emit_input_signals(&snap);
        let _ = rt.step_headless(1.0 / 60.0, &mut vm);
        if game_local(&vm, &mut rt, "lives") <= 0 {
            // hud_ui 的 tick 处理器在漏怪级联之后才渲染 —— 再跑两帧让 HUD 落笔。
            for _ in 0..2 {
                let snap = rt.collect_input();
                let _ = rt.emit_input_signals(&snap);
                let _ = rt.step_headless(1.0 / 60.0, &mut vm);
            }
            sample(&vm, &mut rt, &mut last_wave);
            lose = true;
            break;
        }
    }
    assert!(lose, "zero-tower defense must reach LOSE within 3000 frames");
    assert_eq!(game_local(&vm, &mut rt, "kills"), 0, "no kills without towers");
    assert_eq!(game_local(&vm, &mut rt, "gold"), 20, "no buy, no reward: gold intact");
    assert!(game_local(&vm, &mut rt, "wave") >= 3, "waves still advance on leaks");
    assert!(hud_text(&mut rt).contains("LOSE"), "HUD must show LOSE");
    assert_eq!(visible_towers(&mut rt), 0, "no towers placed");
}
