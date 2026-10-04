//! 引擎能力面（P0 最小集）：宿主实现、JS 侧绑定。
//!
//! 这是"JS 扩展永远不直接碰 SceneTree/Renderer/WGPU"那条裁决的**正面**：
//! 扩展能做什么，完全由宿主把这组 traits 桥出多少来决定。P0 冻结四个
//! 能力（Scene / Node / Input / Audio）+ 一个生命周期（register / update）。
//!
//! 与用户草图的两处 ergonomics 微调（均为防实现方被迫上内部可变性）：
//! * `NodeCapability` 的写方法（`set_pos` / `set_visible`）取 `&mut self`
//!   而非草图中的 `&self` —— 读方法保持 `&self`；
//! * 能力方法的 `NodeRef` 参数按值传入（`Copy` 句柄，草图原为 `&NodeRef`）。

/// 扩展侧的节点引用（不透明句柄）。
///
/// P0 表示：`u64` 位形由宿主定义（nes-runtime 用 `NodeId::to_bits`，
/// slot+generation —— generation 位防"悬垂句柄撞上复用槽位"）。JS 侧
/// 就是一个 number（P0 范围内位形 < 2^53，double 精确承载；越界风险
/// 由宿主文档声明）。
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NodeRef(pub u64);

/// 场景查询能力：按名找节点。
pub trait SceneCapability {
    /// 前序第一个匹配名字的节点（找不到返回 `None`）。
    fn find(&self, name: &str) -> Option<NodeRef>;
}

/// 节点读写能力：位置 / 可见性 / 名字。
pub trait NodeCapability {
    /// 读节点位置（本地变换的平移分量）。
    fn get_pos(&self, r: NodeRef) -> Option<(f32, f32)>;
    /// 写节点位置（只改平移分量，保留旋转/缩放/斜切）。
    fn set_pos(&mut self, r: NodeRef, x: f32, y: f32);
    /// 写节点可见性。
    fn set_visible(&mut self, r: NodeRef, v: bool);
    /// 读节点名字。
    fn get_name(&self, r: NodeRef) -> Option<String>;
}

/// 输入查询能力：按键按住态（快照语义 —— 扩展看到的是本帧输入快照）。
pub trait InputCapability {
    /// 指定逻辑键（如 "Space" / "Left" / "A"）当前是否按住。
    fn is_pressed(&self, name: &str) -> bool;
}

/// 音频触发能力：按键引用播放已注册的声音。
pub trait AudioCapability {
    /// 播放指定键的声音（音量 0.0~1.0；键未注册时静默丢弃，不炸帧）。
    fn play(&self, key: &str, volume: f32);
}

/// 扩展生命周期：注册 + 每帧 update 钩子。
///
/// JS 侧由 `nes.registerExtension(id)` / `nes.onUpdate(fn)` 承接；
/// 本 trait 是宿主面向"已装载扩展"的统一句柄（JS 实现在
/// `nes-extension-js`，未来原生扩展实现自己的版本）。
pub trait ExtensionLifecycle {
    /// 注册扩展（宿主在装载完成后调用；扩展侧记录自己的 id）。
    fn register(&mut self, id: &str);
    /// 每帧钩子（宿主在 simulate 之后调用 —— 扩展看到的是当 tick 后状态）。
    fn update(&mut self);
}

#[cfg(test)]
mod tests {
    use super::{AudioCapability, ExtensionLifecycle, InputCapability, NodeCapability, NodeRef, SceneCapability};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    // mock 宿主（不需要引擎在场 —— API 冻结的价值）。
    struct MockHost {
        found: RefCell<Vec<String>>,
        set_pos: RefCell<Vec<(u64, f32, f32)>>,
        set_vis: RefCell<Vec<(u64, bool)>>,
        played: RefCell<Vec<(String, f32)>>,
        pressed_query: RefCell<Vec<String>>,
    }

    impl MockHost {
        fn new() -> Self {
            Self {
                found: RefCell::new(Vec::new()),
                set_pos: RefCell::new(Vec::new()),
                set_vis: RefCell::new(Vec::new()),
                played: RefCell::new(Vec::new()),
                pressed_query: RefCell::new(Vec::new()),
            }
        }
    }

    impl SceneCapability for MockHost {
        fn find(&self, name: &str) -> Option<NodeRef> {
            self.found.borrow_mut().push(name.to_string());
            Some(NodeRef(11))
        }
    }

    impl NodeCapability for MockHost {
        fn get_pos(&self, _r: NodeRef) -> Option<(f32, f32)> {
            Some((1.0, 2.0))
        }
        fn set_pos(&mut self, r: NodeRef, x: f32, y: f32) {
            self.set_pos.borrow_mut().push((r.0, x, y));
        }
        fn set_visible(&mut self, r: NodeRef, v: bool) {
            self.set_vis.borrow_mut().push((r.0, v));
        }
        fn get_name(&self, _r: NodeRef) -> Option<String> {
            Some("obj1".into())
        }
    }

    impl InputCapability for MockHost {
        fn is_pressed(&self, name: &str) -> bool {
            self.pressed_query.borrow_mut().push(name.to_string());
            name == "Space"
        }
    }

    // AudioCapability 写方法以 &self 为准（实现方用 RefCell 记账 —— 草图
    // 签名的直接后果：触发型能力天然适合内部可变性）。
    impl AudioCapability for MockHost {
        fn play(&self, key: &str, volume: f32) {
            self.played.borrow_mut().push((key.to_string(), volume));
        }
    }

    #[test]
    fn capability_traits_are_host_implementable_without_engine() {
        let mut host = MockHost::new();
        // 读能力（&self）在共享借阅下可用。
        assert_eq!(SceneCapability::find(&host, "obj1"), Some(NodeRef(11)));
        assert_eq!(host.get_pos(NodeRef(11)), Some((1.0, 2.0)));
        assert_eq!(host.get_name(NodeRef(11)).as_deref(), Some("obj1"));
        assert!(InputCapability::is_pressed(&host, "Space"));
        assert!(!host.is_pressed("KeyX"));
        // 写能力（&mut self）独占借阅。
        host.set_pos(NodeRef(11), 3.0, 4.0);
        host.set_visible(NodeRef(11), false);
        AudioCapability::play(&host, "beep", 0.5);
        assert_eq!(host.set_pos.borrow().as_slice(), &[(11, 3.0, 4.0)]);
        assert_eq!(host.set_vis.borrow().as_slice(), &[(11, false)]);
        assert_eq!(host.played.borrow().as_slice(), &[("beep".to_string(), 0.5)]);
        assert_eq!(host.found.borrow().as_slice(), &["obj1".to_string()]);
        assert_eq!(
            host.pressed_query.borrow().as_slice(),
            &["Space".to_string(), "KeyX".to_string()]
        );
    }

    #[test]
    fn lifecycle_records_register_and_updates() {
        // 经 trait object 驱动（宿主统一句柄形态）：用共享计数器观测
        // Box<dyn ExtensionLifecycle> 分派确实到达实现。
        let updates = Rc::new(Cell::new(0u32));
        let lifecycle = CountingLifecycle { registered: RefCell::new(Vec::new()), updates: Rc::clone(&updates) };
        let mut boxed: Box<dyn ExtensionLifecycle> = Box::new(lifecycle);
        boxed.register("hello");
        boxed.update();
        boxed.update();
        assert_eq!(updates.get(), 2);
    }

    struct CountingLifecycle {
        registered: RefCell<Vec<String>>,
        updates: Rc<Cell<u32>>,
    }

    impl ExtensionLifecycle for CountingLifecycle {
        fn register(&mut self, id: &str) {
            self.registered.borrow_mut().push(id.to_string());
        }
        fn update(&mut self) {
            self.updates.set(self.updates.get() + 1);
        }
    }

    #[test]
    fn lifecycle_impl_records() {
        let mut m = MockLifecycle::default();
        ExtensionLifecycle::register(&mut m, "hello");
        ExtensionLifecycle::update(&mut m);
        ExtensionLifecycle::update(&mut m);
        assert_eq!(m.registered, vec!["hello".to_string()]);
        assert_eq!(m.updates, 2);
    }

    #[derive(Default)]
    struct MockLifecycle {
        registered: Vec<String>,
        updates: usize,
    }

    impl ExtensionLifecycle for MockLifecycle {
        fn register(&mut self, id: &str) {
            self.registered.push(id.to_string());
        }
        fn update(&mut self) {
            self.updates += 1;
        }
    }
}
