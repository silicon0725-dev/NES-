//! T-Tick 契约回归：生命周期与 `Cmd` 命令缓冲接入运行时帧循环（S6.2）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Tick-01 | 运行时首帧驱动完整生命周期：enter（自顶向下）/ready（自底向上）各一次、process 每帧；第二帧起 enter/ready 不再触发 |
//! | T-Tick-02 | 回调里的 `SetLocal`（translate）**本帧**直达像素（帧内可见） |
//! | T-Tick-03 | 回调里的结构变更（Remove）**延迟一帧**：本帧仍渲染、下一帧消失且收到 Removed 事件 |
//! | T-Tick-04 | 回调里的 Spawn **延迟一帧**落地：下一帧新节点先 enter 再 process（草案顺序），接上纹理后入画 |
//!
//! 场景层语义本身（遍历确定性、幂等、延迟 Spawn）已由 `nes-scene/tests/m1.rs`
//! 钉死；本组证明的是**运行时帧循环真的在驱动 tick**，以及回调命令直达像素。
//! 资产在独立子目录程序化生成（沿用 M5 §3.1 的隔离裁决）。

use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::PROP_TEXTURE;
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::{
    NodeCtx, NodeId, NodeKind, SceneObserver, SceneTree, Transform2D, TreeEvent, TreeOp,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
/// 四象限纹理：黄 / 青 / 亮灰 / 暗灰。
const V1: [[u8; 4]; 4] = [
    [255, 255, 0, 255],
    [0, 255, 255, 255],
    [200, 200, 200, 255],
    [80, 80, 80, 255],
];

/// 16x16 四象限纹理（象限色按 [TL, TR, BL, BR]）。
fn quadrant_rgba(colors: &[[u8; 4]; 4]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            let color = colors[if y < 8 { 0 } else { 2 } + if x >= 8 { 1 } else { 0 }];
            rgba.extend_from_slice(&color);
        }
    }
    rgba
}

/// 装配运行时 + 一张已上传纹理 + 精灵(10,10) + 单位相机（独立子目录）。
fn assemble_in(dir: &str) -> Option<(NesRuntime, NodeId, nes_scene::ResId)> {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_tick")
        .join(dir);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("demo.bmp"), 16, 16, &quadrant_rgba(&V1))
        .expect("写演示纹理");

    let mut rt = NesRuntime::open_with_root(&root, 64, 64).ok()?;
    let res = rt.declare_texture("Textures/demo.bmp").expect("声明纹理");
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 1, "磁盘纹理应加载成功");
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    let tree = rt.tree_mut();
    let root_node = tree.root();
    let sprite = tree.add_node(root_node, "player", NodeKind::Sprite2D);
    tree.set_prop(sprite, PROP_TEXTURE, res.to_value())
        .expect("绑定纹理属性");
    tree.set_local(sprite, Transform2D::from_pos(10.0, 10.0));
    let camera = tree.add_node(root_node, "cam", NodeKind::Camera2D);
    tree.set_local(camera, Transform2D::from_pos(32.0, 32.0));
    Some((rt, sprite, res))
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(64.0, 64.0))
}

/// 计数观察者：记录 enter/ready 的节点名序与 process 计数。
#[derive(Default)]
struct Counter {
    enter: Vec<&'static str>,
    ready: Vec<&'static str>,
    process: usize,
    events: Vec<&'static str>,
}

impl SceneObserver for Counter {
    fn on_enter_tree(&mut self, ctx: &mut NodeCtx<'_>) {
        self.enter.push(leak_name(ctx));
    }
    fn on_ready(&mut self, ctx: &mut NodeCtx<'_>) {
        self.ready.push(leak_name(ctx));
    }
    fn on_process(&mut self, _ctx: &mut NodeCtx<'_>, _delta: f32) {
        self.process += 1;
    }
    fn on_tree_event(&mut self, _tree: &SceneTree, ev: &TreeEvent) {
        self.events.push(match ev {
            TreeEvent::Added { .. } => "added",
            TreeEvent::Removed { .. } => "removed",
            TreeEvent::Renamed { .. } => "renamed",
            TreeEvent::Reparented { .. } => "reparented",
            TreeEvent::Moved { .. } => "moved",
            TreeEvent::NameAdjusted { .. } => "name_adjusted",
            TreeEvent::Rejected { .. } => "rejected",
        });
    }
}

/// 名字转 'static（仅测试用：树存活贯穿整个用例，名字不会被释放）。
fn leak_name(ctx: &NodeCtx<'_>) -> &'static str {
    Box::leak(ctx.name().to_string().into_boxed_str())
}

/// T-Tick-01：首帧完整生命周期 + 幂等。
#[test]
fn t_tick_01_lifecycle_driven_by_runtime_frame() {
    let Some((mut rt, _sprite, _res)) = assemble_in("t1") else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let mut counter = Counter::default();
    let outcome = rt.frame_with(&frame(0), &mut counter).expect("首帧");
    assert_eq!(outcome.stats.drawn, 1);
    // enter 自顶向下（前序：root -> player -> cam）
    assert_eq!(counter.enter, vec!["root", "player", "cam"]);
    // ready 自底向上（逆前序）
    assert_eq!(counter.ready, vec!["cam", "player", "root"]);
    // process 每节点一次
    assert_eq!(counter.process, 3);

    let mut second = Counter::default();
    rt.frame_with(&frame(1), &mut second).expect("第二帧");
    assert!(second.enter.is_empty(), "第二帧不再 enter");
    assert!(second.ready.is_empty(), "第二帧不再 ready");
    assert_eq!(second.process, 3, "process 每帧照常");
}

/// T-Tick-02：回调里的 translate 本帧直达像素（SetLocal 立即生效）。
#[test]
fn t_tick_02_callback_setlocal_reaches_pixels_same_frame() {
    struct Mover {
        sprite: NodeId,
    }
    impl SceneObserver for Mover {
        fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
            if ctx.this() == self.sprite {
                ctx.translate(16.0, 0.0); // (10,10) -> (26,10)
            }
        }
    }

    let Some((mut rt, sprite, _res)) = assemble_in("t2") else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let mut mover = Mover { sprite };
    let outcome = rt.frame_with(&frame(0), &mut mover).expect("首帧");
    assert_eq!(outcome.stats.drawn, 1, "精灵仍在画");
    let image = &outcome.image;
    assert_eq!(image.pixel(28, 12), Some(V1[0]), "新位置左上象限（黄）");
    assert_eq!(image.pixel(38, 12), Some(V1[1]), "新位置右上象限（青）");
    assert_eq!(image.pixel(12, 12), Some(CLEAR_RGBA), "旧位置已空");
}

/// T-Tick-03：回调里的 Remove 延迟一帧 —— 本帧仍渲染，下一帧消失 + 事件。
#[test]
fn t_tick_03_callback_remove_deferred_one_frame() {
    struct Remover {
        sprite: NodeId,
        fired: bool,
        events: Vec<&'static str>,
    }
    impl SceneObserver for Remover {
        fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
            if !self.fired && ctx.this() == self.sprite {
                self.fired = true;
                ctx.queue(TreeOp::Remove {
                    node: self.sprite,
                    keep_children: false,
                });
            }
        }
        fn on_tree_event(&mut self, _tree: &SceneTree, ev: &TreeEvent) {
            if matches!(ev, TreeEvent::Removed { .. }) {
                self.events.push("removed");
            }
        }
    }

    let Some((mut rt, sprite, _res)) = assemble_in("t3") else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let mut remover = Remover {
        sprite,
        fired: false,
        events: Vec::new(),
    };
    let first = rt.frame_with(&frame(0), &mut remover).expect("首帧");
    assert_eq!(first.stats.drawn, 1, "结构变更延迟：本帧仍渲染");
    assert_eq!(first.image.pixel(12, 12), Some(V1[0]), "本帧像素未变");
    assert!(remover.events.is_empty(), "本帧无结构事件");

    let second = rt.frame_with(&frame(1), &mut remover).expect("第二帧");
    assert_eq!(second.stats.drawn, 0, "下一帧精灵消失");
    assert_eq!(second.image.pixel(12, 12), Some(CLEAR_RGBA), "像素回到背景");
    assert_eq!(remover.events, vec!["removed"], "Removed 事件在落地帧送达");
}

/// T-Tick-04：回调里的 Spawn 延迟一帧落地 —— 下一帧新节点先 enter 再 process
/// （草案顺序：同帧入树者的 enter/ready 在 process 之前），接上纹理后入画。
#[test]
fn t_tick_04_callback_spawn_lands_next_frame() {
    struct Spawner {
        fired: bool,
        enter_after_spawn: Vec<&'static str>,
    }
    impl SceneObserver for Spawner {
        fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
            if !self.fired && ctx.name() == "player" {
                self.fired = true;
                ctx.spawn_child("extra", NodeKind::Sprite2D);
            }
        }
        fn on_enter_tree(&mut self, ctx: &mut NodeCtx<'_>) {
            self.enter_after_spawn.push(leak_name(ctx));
        }
    }

    let Some((mut rt, _sprite, res)) = assemble_in("t4") else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let mut spawner = Spawner {
        fired: false,
        enter_after_spawn: Vec::new(),
    };
    let first = rt.frame_with(&frame(0), &mut spawner).expect("首帧");
    assert_eq!(first.stats.drawn, 1, "本帧只有原精灵");
    assert_eq!(spawner.enter_after_spawn.len(), 3, "本帧 enter 只有原有三节点");
    assert!(rt.tree_mut().find_by_name("extra").is_none(), "本帧结构未落地");

    // 第二帧：extra 落地并 enter（在 process 之前），但还没纹理属性 -> 不入画。
    let second = rt.frame_with(&frame(1), &mut spawner).expect("第二帧");
    // enter 账是累计的：首帧三节点 + 落地帧恰好新增 extra（末位）。
    assert_eq!(spawner.enter_after_spawn.len(), 4, "落地帧恰好新增一次 enter");
    assert_eq!(spawner.enter_after_spawn.last(), Some(&"extra"), "新节点在落地帧 enter");
    assert_eq!(second.stats.drawn, 1, "无纹理不入画（未绑定不渲染契约）");

    // 宿主侧给 extra 绑纹理（合法的帧间编辑），第三帧入画。
    // extra 是 player 的子节点（ctx.spawn_child 挂在发起节点下）：
    // local (22,30) 经父 (10,10) 复合 -> 世界 (32,40) —— 顺带验证
    // 回调 Spawn 落地后的父子变换复合贯通。
    let extra = rt.tree_mut().find_by_name("extra").expect("extra 已在树中");
    rt.tree_mut()
        .set_prop(extra, PROP_TEXTURE, res.to_value())
        .expect("绑定纹理");
    rt.tree_mut()
        .set_local(extra, Transform2D::from_pos(22.0, 30.0));
    let third = rt.frame_with(&frame(2), &mut spawner).expect("第三帧");
    assert_eq!(third.stats.drawn, 2, "两个精灵都在画");
    assert_eq!(third.image.pixel(34, 42), Some(V1[0]), "新精灵像素到位");
}

/// T-Tick-05：信号端到端 —— on_process 发射 "go"，on_signal 处理器把精灵
/// 平移 16px：**同一帧**像素已在新位置（泵先于变换冲洗、Cmd 立即落地、
/// 提取在 tick 之后 —— 全链同帧）。
#[test]
fn t_tick_05_signal_moves_sprite_same_frame() {
    use nes_scene::{Signal, SignalCtx};

    struct SignalMover {
        sprite: NodeId,
        handled: Vec<String>,
    }
    impl nes_scene::SceneObserver for SignalMover {
        fn on_process(&mut self, ctx: &mut nes_scene::NodeCtx<'_>, _delta: f32) {
            if ctx.this() == self.sprite {
                ctx.emit("go", nes_scene::Value::I64(1));
            }
        }
        fn on_signal(&mut self, ctx: &mut SignalCtx<'_>, sig: &Signal) {
            self.handled.push(sig.name.clone());
            if sig.name == "go" {
                // 信号处理器面向任意节点（没有"当前节点"）。
                ctx.set_local(self.sprite, nes_scene::Transform2D::from_pos(26.0, 10.0));
            }
        }
    }

    let Some((mut rt, sprite, _res)) = assemble_in("s5") else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let mut mover = SignalMover {
        sprite,
        handled: Vec::new(),
    };
    let outcome = rt.frame_with(&frame(0), &mut mover).expect("首帧");
    // 首帧泵序（S8.1 起含内建 tick）：内建 tick（宿主预发位）最先，
    // 桥信号其次（首帧落地 -> tree/added x2），用户信号在后。
    assert_eq!(
        mover.handled,
        vec!["tick", "tree/added", "tree/added", "go"],
        "内建 tick -> 桥 -> 用户信号（同帧交付，S8.1 起）"
    );
    assert_eq!(outcome.stats.drawn, 1);
    // 精灵 (10,10) -> (26,10)：信号触发的移动当帧入画。
    assert_eq!(outcome.image.pixel(28, 12), Some(V1[0]), "同帧新位置");
    assert_eq!(outcome.image.pixel(12, 12), Some(CLEAR_RGBA), "旧位置已空");
}
