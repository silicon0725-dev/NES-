//! T-Play 契约回归：play-in-editor 的最小事实（S12-9）。
//!
//! 全部用例无 GPU 依赖（headless 装配 —— 与 criterion_headless 同款纪律：
//! 确定性事实不靠渲染端背书）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Play-1 | **换观察者即运行**：同一棵树同一条 tick 路径，NoObserver 跑
//!   3 帧脚本节点的每帧翻转不发生；把观察者换成装载了脚本的 ScriptVm 再跑
//!   3 帧，布尔逐帧翻转 —— 运行态不是第二运行时，只是帧循环的观察者参数
//!   换人。翻转对象用 `enabled`（Script kind 是无基类根类型，schema 里没有
//!   `visible`；`enabled` 是 Script 节点上同形的每帧布尔翻转，且落属性表
//!   —— 与 play 运行期改数据 / RESET 还原属性是同一张表） |
//! | T-Play-2 | **RESET 数据还原基元**：SubtreeSnapshot 快照 -> 运行期改数据
//!   （换名/挪位/改键/裸通道新增键）-> 按 uid 写回 + 摘掉快照没有的键 ->
//!   名字/变换/属性集全部回到运行前，且 uid 与 NodeId 都不变（`restore`
//!   的删了重加会换 NodeId，编辑器壳层手柄经不起 —— 数据面还原是
//!   play-in-editor 的裁决口径） |

use nes_runtime::NesRuntime;
use nes_scene::transaction::SubtreeSnapshot;
use nes_scene::{NodeKind, NoObserver, ScriptVm, Transform2D, Value};

/// 测试根目录（每用例独立子目录 —— M5 §3.1 测试层隔离口径）。
fn root(tag: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_play")
        .join(tag)
}

/// T-Play-1：编辑器帧循环里的脚本驱动 = **观察者参数**。blink 类脚本
///（every 每帧翻转布尔）在 NoObserver 下三帧无声，换 ScriptVm 观察者后
/// 三帧翻三次 —— Godot F5 的全部机制就是这一个参数。
#[test]
fn t_play_1_swap_observer_runs_scripts() {
    let dir = root("t1_observer_swap");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut rt = NesRuntime::open_headless(&dir).expect("headless 装配");

    // 场景：一个挂了 blink 脚本的 Script 节点（内嵌 source，attach_all
    // 直装 —— 与编辑器挂载流的装载语义同一条路径）。
    let brain = {
        let tree = rt.tree_mut();
        let brain = tree.add_node(tree.root(), "brain", NodeKind::Script);
        tree.set_prop(
            brain,
            "source",
            Value::Str("every { this.enabled = !this.enabled }".into()),
        )
        .unwrap();
        brain
    };
    rt.tree_mut().apply_pending();

    let enabled_of = |rt: &mut NesRuntime| -> Option<bool> {
        rt.tree_mut()
            .prop(brain, "enabled")
            .and_then(Value::as_bool)
    };

    // 编辑态：NoObserver 三帧 —— 没人听 process，翻转不发生（schema
    // 缺省真原样不动）。
    for _ in 0..3 {
        rt.step_headless(1.0 / 60.0, &mut NoObserver);
    }
    assert_eq!(
        enabled_of(&mut rt),
        Some(true),
        "NoObserver 下脚本不驱动（缺省值未被翻转）"
    );

    // PLAY：装载全部挂载脚本（零缺口）+ 观察者换成 VM。同一棵树、同一条
    // tick 路径 —— 唯一的差别是帧循环的观察者参数。
    let mut vm = ScriptVm::new();
    let issues = vm.attach_all(rt.tree_mut());
    assert!(issues.is_empty(), "装载无缺口：{issues:?}");
    for i in 0..3u64 {
        rt.step_headless(1.0 / 60.0, &mut vm);
        // 初值 true，每帧翻一次：帧 0/1/2 后 = false/true/false。
        let expect = i % 2 == 1;
        assert_eq!(
            enabled_of(&mut rt),
            Some(expect),
            "第 {i} 帧脚本应把 enabled 翻到 {expect}"
        );
    }
    assert!(
        vm.locals(brain).is_none_or(|l| !l.contains_key(nes_scene::HALT_LOCAL)),
        "脚本无停机记录"
    );
}

/// T-Play-2：RESET 的还原基元 —— 快照（SubtreeSnapshot，uid 锚定）->
/// 运行期任意数据改动 -> 按 uid 写回 + 摘掉快照没有的键。uid 与 NodeId
/// 双双不动（`restore` 的删了重加会换 NodeId，编辑器壳层手柄经不起 ——
/// 数据面还原是 play-in-editor 的裁决口径）。
#[test]
fn t_play_2_snapshot_restore_keeps_identity() {
    let dir = root("t2_snapshot_restore");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut rt = NesRuntime::open_headless(&dir).expect("headless 装配");

    // 场景：一个精灵（局部 (5,5)，visible 走 schema 缺省真）。
    let sprite = {
        let tree = rt.tree_mut();
        let sprite = tree.add_node(tree.root(), "sprite", NodeKind::Sprite2D);
        tree.set_local(sprite, Transform2D::from_pos(5.0, 5.0));
        sprite
    };
    rt.tree_mut().apply_pending();

    // PLAY 前快照：树全量前序（与 editor_shell 的 PLAY 同一口径 —— 每个
    // 节点一份子树快照，数据面含属性表全集）。
    let snapshot: Vec<SubtreeSnapshot> = {
        let tree = rt.tree_mut();
        let root_uid = tree.uid_of(tree.root()).unwrap();
        tree.preorder()
            .into_iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let parent = tree
                    .parent(n)
                    .and_then(|p| tree.uid_of(p))
                    .unwrap_or_else(|| root_uid.clone());
                SubtreeSnapshot::capture(tree, n, parent, i)
            })
            .collect()
    };
    assert!(!snapshot.is_empty());

    // 运行期改动：挪位 + 换名 + 改键 + 裸通道新增键（set_prop 只认 schema
    // 键且出生即满配 —— 新增键只可能来自 set_prop_raw 前向兼容通道；
    // 还原必须把这种键摘掉才算"回到运行前"）。
    {
        let tree = rt.tree_mut();
        tree.set_local(sprite, Transform2D::from_pos(9.0, 9.0));
        tree.rename(sprite, "moved");
        tree.set_prop(sprite, "visible", Value::Bool(false)).unwrap();
        tree.set_prop_raw(sprite, "debug_tag", Value::Str("play".into()));
    }
    let uid_before = rt.tree_mut().uid_of(sprite).unwrap();

    // RESET：按 uid 寻回 -> 摘快照没有的键 -> apply_data 整体写回。
    {
        let tree = rt.tree_mut();
        for snap in &snapshot {
            let Some(id) = tree.find_by_uid(&snap.data.uid) else {
                continue; // 运行期结构不变（编辑交互禁用 + 脚本无结构指令），
                          // 寻不回只可能是脏快照 —— 如实跳过。
            };
            // 快照没有的键 = 运行期新增 —— 摘掉（apply_data 只覆盖快照键）。
            let extra: Vec<String> = tree
                .props(id)
                .map(|p| {
                    p.iter()
                        .map(|(k, _)| k.to_string())
                        .filter(|k| snap.data.props.get(k).is_none())
                        .collect()
                })
                .unwrap_or_default();
            for k in extra {
                tree.remove_prop(id, &k);
            }
            SubtreeSnapshot::apply_data(tree, id, &snap.data).unwrap();
        }
        tree.apply_pending();
    }

    // 全部回到运行前：位置、名字、属性值与属性集；uid 与 NodeId 双双不动。
    let tree = rt.tree_mut();
    assert_eq!(tree.local(sprite).unwrap().pos.x, 5.0, "位置回到运行前");
    assert_eq!(tree.name(sprite), Some("sprite"), "名字回到运行前");
    assert_eq!(
        tree.prop(sprite, "visible"),
        Some(&Value::Bool(true)),
        "visible 回到运行前（schema 缺省真）"
    );
    assert_eq!(
        tree.prop(sprite, "debug_tag"),
        None,
        "运行期经裸通道新增的键已被摘掉"
    );
    assert_eq!(tree.uid_of(sprite), Some(uid_before), "uid 不变");
}
