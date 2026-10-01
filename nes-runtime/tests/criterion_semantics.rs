//! T-RS-R 契约回归：S7.1 运行时语义冻结在**运行时帧循环**里的接线。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-RS-R1 | `Observers` 组合进 `frame_with`：宿主行为观察者与脚本 VM 同帧各驱动各的节点（注册序组合、一条帧路径） |

use nes_render_api::{FrameInfo, Vec2};
use nes_runtime::NesRuntime;
use nes_scene::{NodeCtx, NodeKind, Observers, SceneObserver, SceneTree, Transform2D, Value, ScriptVm};

/// 宿主行为：holder 节点每帧平移 (3,0)。
struct HostDrift {
    holder: nes_scene::NodeId,
}

impl SceneObserver for HostDrift {
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, _delta: f32) {
        if ctx.this() == self.holder {
            ctx.translate(3.0, 0.0);
        }
    }
}

/// T-RS-R1：VM（信号脚本驱动 b）与宿主观察者（process 驱动 a）装进
/// 同一个 `Observers`，`frame_with` 一帧 —— 两边的行为都落地（组合是
/// 注册序 Vec，帧路径只有一条）。
#[test]
fn t_rs_r1_observers_compose_in_frame_loop() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_semantics")
        .join("r1");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let Ok(mut rt) = NesRuntime::open_with_root(&root, 64, 64) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let (a, b) = {
        let tree = rt.tree_mut();
        let cam = tree.add_node(tree.root(), "cam", NodeKind::Camera2D);
        tree.set_local(cam, Transform2D::from_pos(32.0, 32.0));
        let a = tree.add_node(tree.root(), "a", NodeKind::Node2D);
        tree.set_local(a, Transform2D::from_pos(8.0, 8.0));
        let b = tree.add_node(tree.root(), "b", NodeKind::Node2D);
        tree.set_local(b, Transform2D::from_pos(8.0, 8.0));
        let brain = tree.add_node(tree.root(), "brain", NodeKind::Script);
        tree.set_prop(brain, "source", Value::Str("on \"beat\" { b.pos += (0.0, 5.0) }".into()))
            .unwrap();
        tree.apply_pending();
        (a, b)
    };

    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(rt.tree_mut()).is_empty());

    let mut observers = Observers::new();
    assert_eq!(observers.push(Box::new(vm)), 0, "VM 先注册");
    assert_eq!(observers.push(Box::new(HostDrift { holder: a })), 1, "宿主后注册");

    let frame = FrameInfo::new(0, 1.0 / 60.0, 0.0, Vec2::new(64.0, 64.0));
    rt.tree_mut().emit_signal("beat", Value::I64(0));
    let out = rt.frame_with(&frame, &mut observers).expect("帧");
    assert_eq!(out.stats.driver_errors, 0);

    let tree: &SceneTree = rt.tree_mut();
    assert_eq!(tree.local(a).unwrap().pos.x, 11.0, "宿主行为落地（+3）");
    assert_eq!(tree.local(b).unwrap().pos.y, 13.0, "VM 信号脚本落地（+5）");
}
