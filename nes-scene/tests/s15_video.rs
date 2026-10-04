//! S15 视频资产面 · 场景层契约回归：`video_play` / `video_stop` 语法、
//! Cmd 流落地与确定性边界。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Vid-S-01 | `video_play "key"` / `video_stop "key"` 编译为 `Op::VideoPlay` / `Op::VideoStop`（零栈交互，照 `play` 同款） |
//! | T-Vid-S-02 | Cmd 流落地：树不解释视频，键按**发射序**进 `take_video_cmds` 取走缓冲（play/stop 混排时序如实保留；取走幂等） |
//! | T-Vid-S-03 | **视频命令不进语义指纹**：同轨迹消费与否 trace 指纹逐位相同（T-Aud-04 / T-Cmp-34 同一条确定性纪律） |
//! | T-Vid-S-04 | process 与信号**两入口同权**（照 `play` / `emit` 口径） |
//! | T-Vid-S-05 | `video_play` / `video_stop` 是保留字（不得作变量名）；缺字符串字面量如实报错 |
//! | T-Vid-S-06 | Sprite2D.texture 提示放宽：Video 资源经 texture 属性引用**不产生**类别 mismatch；真不匹配（如 Script 类）仍如实报 |
//!
//! 视频的真实解码/换页/音轨在 nes-runtime 侧（`tests/s15_video.rs`）——
//! 树只搬运键名，本文件只钉场景层契约。

use nes_scene::{
    compile_script, instantiate_doc_with_resources, parse_ron, NodeKind, Op, SceneTree,
    ScriptEntry, ScriptVm, Value,
};

fn tree2d() -> (SceneTree, nes_scene::NodeId, nes_scene::NodeId) {
    let mut t = SceneTree::new("root");
    let sprite = t.add_node(t.root(), "sprite", NodeKind::Node2D);
    let brain = t.add_node(t.root(), "brain", NodeKind::Script);
    t.apply_pending();
    (t, sprite, brain)
}

/// T-Vid-S-01：编译产物逐指令相等 —— `video_play` / `video_stop` 是
/// 语句级关键字 + 字符串字面量，零栈交互（与 `play` 同一形态）。
#[test]
fn t_vid_s_01_video_statements_compile_to_ops() {
    let script = compile_script(r#"every { video_play "Media/spider" }"#).expect("编译");
    assert_eq!(script.entry, ScriptEntry::Process);
    assert_eq!(
        script.ops,
        vec![Op::VideoPlay { key: "Media/spider".into() }]
    );

    let script = compile_script(r#"every { video_stop "Media/spider" }"#).expect("编译");
    assert_eq!(
        script.ops,
        vec![Op::VideoStop { key: "Media/spider".into() }]
    );

    // 同帧混排：编译序 = 语句序（发射序的前提）。
    let script = compile_script(
        "every {\n  video_play \"a\"\n  video_stop \"a\"\n  video_play \"b\"\n}",
    )
    .expect("编译");
    assert_eq!(
        script.ops,
        vec![
            Op::VideoPlay { key: "a".into() },
            Op::VideoStop { key: "a".into() },
            Op::VideoPlay { key: "b".into() },
        ]
    );
}

/// T-Vid-S-02：Cmd 流落地 —— 树不解释视频，键按发射序进取走缓冲；
/// play/stop 混排时序如实保留；取走幂等。
#[test]
fn t_vid_s_02_video_cmds_flow_through_buffer_in_order() {
    let (mut t, _sprite, brain) = tree2d();
    t.set_prop(
        brain,
        "source",
        Value::Str("every { if n < 1 { n = n + 1\n  video_play \"a\"\n  video_stop \"a\"\n  video_play \"b\" } }".into()),
    )
    .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(
        t.take_video_cmds(),
        vec![
            nes_scene::VideoCmd::Play { key: "a".into() },
            nes_scene::VideoCmd::Stop { key: "a".into() },
            nes_scene::VideoCmd::Play { key: "b".into() },
        ],
        "play/stop 混排按发射序落地"
    );
    // 取走即清空：不重复消费。
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert!(t.take_video_cmds().is_empty(), "条件只发一轮，第二帧无命令");
    assert!(t.take_video_cmds().is_empty(), "取走幂等（无人再发则空）");
}

/// T-Vid-S-03：视频命令**不是语义状态** —— 消费（取走）与否不影响语义
/// 指纹，同一轨迹跑两遍指纹逐位相同（照 T-Cmp-34 的 played_sounds 家法；
/// uid 钉成确定性派生身份，两遍只考视频面）。
#[test]
fn t_vid_s_03_video_cmds_are_not_semantic_state() {
    let run_once = || {
        let (mut t, _sprite, brain) = tree2d();
        for (i, n) in t.preorder().into_iter().enumerate() {
            t.set_uid(n, nes_scene::Uid::derive_legacy(&format!("/p{i}")))
                .unwrap();
        }
        t.set_prop(
            brain,
            "source",
            Value::Str("every { if n < 2 { n = n + 1\n  video_play \"s\" } }".into()),
        )
        .unwrap();
        let mut vm = ScriptVm::new();
        assert!(vm.attach_all(&mut t).is_empty());
        let mut hashes = Vec::new();
        for _ in 0..3 {
            let _ = t.tick(1.0 / 60.0, &mut vm);
            hashes.push(nes_scene::scene_fingerprint(&t, Some(&vm)));
            // 每帧取走并丢弃 —— 消费本身不得改变下一帧的指纹。
            let _ = t.take_video_cmds();
        }
        hashes
    };
    let consumed = run_once();
    // 对照组：同一轨迹、从不取走（命令在缓冲里堆积）—— 指纹必须逐位同。
    let unconsumed = {
        let (mut t, _sprite, brain) = tree2d();
        for (i, n) in t.preorder().into_iter().enumerate() {
            t.set_uid(n, nes_scene::Uid::derive_legacy(&format!("/p{i}")))
                .unwrap();
        }
        t.set_prop(
            brain,
            "source",
            Value::Str("every { if n < 2 { n = n + 1\n  video_play \"s\" } }".into()),
        )
        .unwrap();
        let mut vm = ScriptVm::new();
        assert!(vm.attach_all(&mut t).is_empty());
        let mut hashes = Vec::new();
        for _ in 0..3 {
            let _ = t.tick(1.0 / 60.0, &mut vm);
            hashes.push(nes_scene::scene_fingerprint(&t, Some(&vm)));
            // 不取走。
        }
        hashes
    };
    assert_eq!(consumed, unconsumed, "消费与否不影响语义指纹（逐位相同）");
    assert_eq!(consumed, run_once(), "同轨迹两遍指纹逐位相同");
}

/// T-Vid-S-04：信号入口同权 —— `on "go" { video_play "s" }` 与 process
/// 入口走同一条 Cmd 通道。
#[test]
fn t_vid_s_04_signal_entry_same_channel() {
    let (mut t, _sprite, brain) = tree2d();
    t.set_prop(brain, "source", Value::Str(r#"on "go" { video_play "s" }"#.into()))
        .unwrap();
    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(&mut t).is_empty());
    t.emit_signal("go", Value::I64(0));
    let _ = t.tick(1.0 / 60.0, &mut vm);
    assert_eq!(
        t.take_video_cmds(),
        vec![nes_scene::VideoCmd::Play { key: "s".into() }],
        "信号入口同权"
    );
}

/// T-Vid-S-05：语法错误口径 —— 语句后必须跟字符串字面量；关键字是
/// 保留字（不得作变量名）。
#[test]
fn t_vid_s_05_syntax_errors_are_honest() {
    assert!(compile_script(r#"every { video_play 42 }"#).is_err());
    assert!(compile_script(r#"every { video_stop 42 }"#).is_err());
    assert!(compile_script(r#"every { video_play = 1 }"#).is_err());
    assert!(compile_script(r#"every { video_stop = 1 }"#).is_err());
    // 保留字：标识符位出现即语法错（与 play 同款）。
    assert!(compile_script(r#"every { video_play = 1 }"#).is_err());
}

/// T-Vid-S-06：Sprite2D.texture 提示放宽（additive 最小面）—— Video 资源
/// 经 texture 属性引用**不产生**类别 mismatch；表外类别（Script）仍如实报。
#[test]
fn t_vid_s_06_texture_hint_accepts_video_not_anything() {
    // Video 资源 + Sprite2D.texture 引用：体检必须干净。
    let doc = parse_ron(
        r#"Scene(
    version: 1,
    resources: [ Res(id: 1, path: "Media/spider.amv", kind: "Video") ],
    root: Node(
        name: "main",
        kind: "Node",
        children: [
            Node(name: "tv", kind: "Sprite2D", props: { "texture": Resource(1) }, children: [],),
        ],
    ),
)
"#,
    )
    .expect("解析");
    let (_tree, _table, report) = instantiate_doc_with_resources(&doc).expect("实例化");
    assert!(
        report.mismatches.is_empty(),
        "Video 资源经 texture 属性引用不该报类别冲突：{:?}",
        report.mismatches
    );
    assert!(report.undeclared.is_empty());

    // 对照组：Script 类资源经 texture 属性引用 —— 真不匹配仍如实报
    //（放宽面只有 texture|video，不开放任意 kind）。
    let doc = parse_ron(
        r#"Scene(
    version: 1,
    resources: [ Res(id: 1, path: "Scripts/x.nes", kind: "Script") ],
    root: Node(
        name: "main",
        kind: "Node",
        children: [
            Node(name: "tv", kind: "Sprite2D", props: { "texture": Resource(1) }, children: [],),
        ],
    ),
)
"#,
    )
    .expect("解析");
    let (_tree, _table, report) = instantiate_doc_with_resources(&doc).expect("实例化");
    assert_eq!(
        report.mismatches.len(),
        1,
        "Script 资源经 texture 引用仍应报 mismatch：{:?}",
        report.mismatches
    );
}
