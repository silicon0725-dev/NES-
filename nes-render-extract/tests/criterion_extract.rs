//! S2「提取层最小闭环」出口准则集成测试。
//!
//! 只依赖两件事：`nes-render-extract` 的公开 API、`nes-render-api` 的 `NullRenderServer`。
//! 不引入任何第三方 crate（G6 的纪律同样适用于测试侧：本机工具链不完整，
//! 任何带构建脚本的 dev-dependency 都会让"能不能编译"变成不确定事件）。
//!
//! # 三类出口准则 ↔ 测试分组
//!
//! | 出口准则 | 分组 | 这类测试在钉什么 |
//! |---|---|---|
//! | 变换传播一致 | `transform` | 推出去的世界矩阵必须**逐位等于**场景层 `world` 缓存；父改则子下一帧必变；`flip` 不折叠进变换 |
//! | z 序稳定 | `z_order` | 绘制次序必须**只**由 `(z, NodeData::order, handle)` 全序决定；空闲帧不变、换位次帧生效、新会话可复现 |
//! | 节点增删无泄漏 | `lifecycle` | 增/删/摘子树/保子删父/资源消失/资源换代/长时间 churn 下，映射表与后端存活集合**不许分叉**，也不许悬垂 |
//!
//! 另有 `bookkeeping` 组钉住记账自洽、哨兵计数、缓冲复用、以及生产路径
//! `ResourceTable` 作为资源键来源的接法。
//!
//! # 共用地基断言
//!
//! 每个测试的每一帧都过 [`assert_synced`]，它同时检查四件事：
//!
//! 1. `stats.map_len == ex.map().len()`（统计与映射表自洽）；
//! 2. `ex.map().len() == srv.len()`（映射表与后端句柄集合**不分叉** —— 泄漏与悬垂都
//!    会在这一步炸出来）；
//! 3. 映射表里每个句柄在后端都还存在，且挂着同一个资源键（无悬垂）；
//! 4. `counters.ignored_ops == 0`（契约层对空句柄 / 未知句柄的操作计数：非零即说明
//!    本层发过"打向后端不认识的东西"的命令，或重复销毁过）。

use std::cell::RefCell;
use std::collections::BTreeMap;

use nes_render_api::{
    Affine2, Flip, FrameInfo, ItemHandle, NullRenderServer, RenderAssetKey, RenderCommand, Vec2,
};
use nes_render_extract::{
    affine2_of, ExtractStats, RenderExtractor, RenderKeySource, PROP_FLIP_H, PROP_FLIP_V,
    PROP_TEXTURE, PROP_VISIBLE, PROP_Z_INDEX,
};
use nes_scene::{Affine, NodeId, NodeKind, ResId, ResourceTable, SceneTree, Transform2D, Value};

// ---------------------------------------------------------------- 公共替身与工具

/// 资源键替身：想在测试里演"资源消失 / 资源换代"，不必去真建资源表。
///
/// 用 `RefCell<BTreeMap>` 而非 `HashMap`：迭代顺序确定性是 `nes-scene` / 本层的一贯要求，
/// 测试替身也不做例外（虽然本层其实只做点查）。
#[derive(Default)]
struct KeyMap(RefCell<BTreeMap<ResId, RenderAssetKey>>);

impl KeyMap {
    fn new() -> Self {
        Self::default()
    }

    /// 绑定 / 换代：同一个 `ResId` 换成别的键（或重新绑回旧键）。
    fn set(&self, id: ResId, key: RenderAssetKey) {
        self.0.borrow_mut().insert(id, key);
    }

    /// 资源消失：槽位被回收，之后再查就是"没有可渲染资源"。
    fn remove(&self, id: ResId) -> bool {
        self.0.borrow_mut().remove(&id).is_some()
    }
}

impl RenderKeySource for KeyMap {
    fn render_key(&self, id: ResId) -> Option<RenderAssetKey> {
        self.0.borrow().get(&id).copied()
    }
}

/// 造一个非空资源键（`slot == 0` 或 `gen == 0` 的都是空键，本层视为不可渲染）。
fn key(slot: u32, gen: u32) -> RenderAssetKey {
    RenderAssetKey::from_parts(slot, gen)
}

/// 帧上下文（`viewport` 固定，本层不看它，只做透传）。
fn frame_info(index: u64) -> FrameInfo {
    FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(640.0, 360.0))
}

/// 世界矩阵推进：落地结构变更 + 冲洗变换缓存（正常帧首的顺序）。
fn advance(tree: &mut SceneTree) {
    tree.apply_pending();
    tree.refresh_transforms();
}

/// 写属性并断言 schema 接受它（拒绝说明契约拼写与场景层 schema 不合，属于硬错误）。
fn set_prop(tree: &mut SceneTree, node: NodeId, name: &str, value: Value) {
    tree.set_prop(node, name, value)
        .unwrap_or_else(|err| panic!("写入属性 {name} 被 schema 拒绝：{err:?}"));
}

/// 加一个已绑定纹理的 `Sprite2D`（最常用的可渲染物）。
fn add_sprite(tree: &mut SceneTree, parent: NodeId, name: &str, res: ResId) -> NodeId {
    let node = tree.add_node(parent, name, NodeKind::Sprite2D);
    set_prop(tree, node, PROP_TEXTURE, res.to_value());
    advance(tree);
    node
}

/// 加一个不可渲染的容器节点（`Node2D` 只有变换，没有纹理）。
fn add_container(tree: &mut SceneTree, parent: NodeId, name: &str) -> NodeId {
    let node = tree.add_node(parent, name, NodeKind::Node2D);
    advance(tree);
    node
}

/// 挂到场景根下的可渲染物（先取出 `root()` 再借 `&mut`，避开同表达式双重借用）。
fn add_root_sprite(tree: &mut SceneTree, name: &str, res: ResId) -> NodeId {
    let root = tree.root();
    add_sprite(tree, root, name, res)
}

/// 挂到场景根下的容器节点。
fn add_root_container(tree: &mut SceneTree, name: &str) -> NodeId {
    let root = tree.root();
    add_container(tree, root, name)
}

/// 提取一帧。
fn step(
    ex: &mut RenderExtractor,
    tree: &mut SceneTree,
    source: &dyn RenderKeySource,
    srv: &mut NullRenderServer,
    out: &mut Vec<RenderCommand>,
    index: u64,
) -> ExtractStats {
    let info = frame_info(index);
    ex.extract_into(tree, source, srv, &info, out)
}

/// 每帧必查的地基一致性（见模块文档）。
fn assert_synced(ex: &RenderExtractor, srv: &NullRenderServer, stats: &ExtractStats) {
    assert_eq!(
        stats.map_len,
        ex.map().len(),
        "统计里的 map_len 与映射表实际长度不一致"
    );
    assert_eq!(
        ex.map().len(),
        srv.len(),
        "映射表与后端存活渲染物数量分叉：映射表 {} / 后端 {}",
        ex.map().len(),
        srv.len()
    );
    for slot in ex.map().iter() {
        let item = srv
            .item(slot.handle)
            .unwrap_or_else(|| panic!("映射表里的句柄 {:?} 在后端已不存在（悬垂）", slot.handle));
        assert_eq!(
            item.key, slot.key,
            "句柄 {:?} 挂的资源键与映射表记录不符",
            slot.handle
        );
        assert!(
            slot.seen_frame() > 0,
            "句柄 {:?} 的 seen_frame 仍为 0（从未被遍历到却活在表里）",
            slot.handle
        );
    }
    assert_eq!(
        srv.counters().ignored_ops,
        0,
        "契约层记录了空句柄 / 未知句柄操作：本层对后端发了它不认识的东西"
    );
}

/// **独立**重算期望绘制次序：只读场景树 + 映射表，不看后端内部结构。
///
/// 依据就是本层承诺的全序：`(z_index, NodeData::order)` —— 同一场景里 `order`
/// 全局唯一，所以这两项已经构成全序，`handle` 只是契约层排序的第三决断项。
fn expected_draw_order(tree: &SceneTree, ex: &RenderExtractor) -> Vec<ItemHandle> {
    let mut items: Vec<(i32, u64, ItemHandle)> = Vec::new();
    for node in tree.preorder() {
        let Some(handle) = ex.handle_of(node) else {
            continue;
        };
        let z = tree
            .prop(node, PROP_Z_INDEX)
            .and_then(|value| value.as_i64())
            .unwrap_or(0) as i32;
        let order = tree.get(node).map_or(0, |data| data.order);
        items.push((z, order, handle));
    }
    items.sort();
    items.into_iter().map(|(_, _, handle)| handle).collect()
}

/// 矩阵全分量有限（NaN / Inf 会在后端里静默变成"什么都不画"，测试必须当场拦下）。
fn all_finite(a: Affine2) -> bool {
    a.to_array().iter().all(|value| value.is_finite())
}

fn count_creates(out: &[RenderCommand]) -> usize {
    out.iter()
        .filter(|cmd| matches!(cmd, RenderCommand::CreateItem { .. }))
        .count()
}

fn count_destroys(out: &[RenderCommand]) -> usize {
    out.iter()
        .filter(|cmd| matches!(cmd, RenderCommand::DestroyItem { .. }))
        .count()
}

fn count_transforms(out: &[RenderCommand]) -> usize {
    out.iter()
        .filter(|cmd| matches!(cmd, RenderCommand::SetTransform { .. }))
        .count()
}

// ---------------------------------------------------------------- 准则一：变换传播一致

mod transform {
    use super::*;

    /// 推送的世界矩阵与场景层 `world` 缓存逐位相同（含平移与旋转复合）。
    #[test]
    fn criterion_extract_transform_matches_scene_world_cache() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let container = add_root_container(&mut tree, "container");
        tree.set_local(container, Transform2D::from_pos(10.0, -4.0));
        let sprite = add_sprite(&mut tree, container, "sprite", ResId::new(1));
        tree.set_local(sprite, Transform2D::from_scale(2.0, 3.0));
        advance(&mut tree);

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.fresh, 1, "只应新建 1 个渲染物");

        let handle = ex.handle_of(sprite).expect("sprite 应当已上屏");
        let pushed = srv.item(handle).expect("后端应当有该渲染物").transform;

        // 真值来自场景层唯一权威算式，本层只做逐字段搬运。
        let world = tree.world(sprite).expect("sprite 在世界里");
        assert_eq!(pushed, affine2_of(world), "推送值必须逐位等于 world 缓存");
        assert_eq!(pushed.tx, 10.0);
        assert_eq!(pushed.ty, -4.0);
        assert_eq!(pushed.a, 2.0);
        assert_eq!(pushed.d, 3.0);
        assert!(all_finite(pushed));
    }

    /// 父节点这一帧改变换 → 子节点**下一帧**推送的值必变，且与场景层一致。
    #[test]
    fn criterion_extract_transform_follows_parent_change_next_frame() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let parent = add_root_container(&mut tree, "parent");
        let child = add_sprite(&mut tree, parent, "child", ResId::new(1));
        tree.set_local(child, Transform2D::from_pos(1.0, 0.0));
        advance(&mut tree);

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();

        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        let handle = ex.handle_of(child).expect("child 应当已上屏");
        let before = srv.item(handle).expect("有渲染物").transform;

        // 只动父节点；子的 local 一个字节都没改。
        tree.set_local(parent, Transform2D::from_pos(0.0, 25.0));
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);

        let after = srv.item(handle).expect("有渲染物").transform;
        assert_ne!(before, after, "父变了，子下一帧的推送值必须跟着变");
        assert_eq!(after.ty, before.ty + 25.0, "平移增量应当落在世界矩阵上");
        assert_eq!(
            after,
            affine2_of(tree.world(child).expect("child 在世界里")),
            "推送值仍须逐位等于场景层 world 缓存"
        );
    }

    /// 推送值与场景层"自己乘父子链"的结果一致（本层不另算一套矩阵）。
    #[test]
    fn criterion_extract_transform_matches_scene_recompute() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let a = add_root_container(&mut tree, "a");
        tree.set_local(a, Transform2D::from_rot(0.5));
        let b = add_sprite(&mut tree, a, "b", ResId::new(1));
        tree.set_local(b, Transform2D::from_pos(3.0, 4.0));
        advance(&mut tree);

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);

        let local_b = tree.local(b).expect("b 的 local").to_affine();
        let world_a: Affine = tree.world(a).expect("a 的世界矩阵");
        let recomputed = world_a.mul(&local_b);
        assert_eq!(tree.world(b).expect("b 的世界矩阵"), recomputed);

        let handle = ex.handle_of(b).expect("b 应当已上屏");
        assert_eq!(
            srv.item(handle).expect("有渲染物").transform,
            affine2_of(recomputed),
            "推送值 = affine2_of(父世界矩阵 × 子局部矩阵)"
        );
    }

    /// `flip` 不折叠进世界变换：翻转节点上的矩阵必须还是单位矩阵。
    ///
    /// 契约层的 `RenderItem::world_transform` 才负责把 flip 作为子局部**后乘**，
    /// 提取层若提前折进去，后端再乘一次就成了翻两次（等于没翻）——
    /// 这条测试是那个 bug 的哨兵。
    #[test]
    fn criterion_extract_flip_not_folded_into_transform() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let sprite = add_root_sprite(&mut tree, "sprite", ResId::new(1));
        set_prop(&mut tree, sprite, PROP_FLIP_H, Value::Bool(true));
        set_prop(&mut tree, sprite, PROP_FLIP_V, Value::Bool(false));
        set_prop(&mut tree, sprite, PROP_VISIBLE, Value::Bool(false));
        advance(&mut tree);

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);

        let handle = ex.handle_of(sprite).expect("sprite 应当已上屏");
        let item = srv.item(handle).expect("有渲染物");
        assert_eq!(item.flip, Flip::new(true, false), "flip_h 应当推成水平翻转");
        assert_eq!(item.transform, Affine2::IDENTITY, "flip 不得折进世界变换");
        assert!(!item.visible, "visible=false 只关绘制，不销毁渲染物");
        assert!(
            srv.item(handle).is_some(),
            "不可见不等于不存在：渲染物仍须留在后端"
        );
    }

    /// 不可渲染的节点（容器 / 文字 / 相机）不推变换，其变换变化不产生任何渲染物。
    #[test]
    fn criterion_extract_transform_ignores_non_renderable_nodes() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let container = add_root_container(&mut tree, "container");
        let label = tree.add_node(container, "label", NodeKind::Label);
        advance(&mut tree);

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();

        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.pushed, 0, "容器与文字都不该上屏");
        assert!(srv.is_empty());

        // 只改不可渲染节点的变换：不应让任何渲染物出现 / 消失。
        tree.set_local(container, Transform2D::from_pos(100.0, 100.0));
        tree.set_local(label, Transform2D::from_pos(5.0, 5.0));
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.pushed, 0);
        assert_eq!(stats.created, 0);
        assert_eq!(stats.destroyed, 0);
        assert!(srv.is_empty());
        assert_eq!(ex.handle_of(container), None);
        assert_eq!(ex.handle_of(label), None);
    }
}

// ---------------------------------------------------------------- 准则二：z 序稳定

mod z_order {
    use super::*;

    /// 三兄弟精灵的固定场景（`handle` 由遍历序决定，与 `NodeId` 槽位同序）。
    fn fixture(keys: &KeyMap) -> (SceneTree, NodeId, NodeId, NodeId) {
        keys.set(ResId::new(1), key(1, 1));
        keys.set(ResId::new(2), key(2, 1));
        keys.set(ResId::new(3), key(3, 1));

        let mut tree = SceneTree::new("root");
        let s1 = add_root_sprite(&mut tree, "s1", ResId::new(1));
        let s2 = add_root_sprite(&mut tree, "s2", ResId::new(2));
        let s3 = add_root_sprite(&mut tree, "s3", ResId::new(3));
        (tree, s1, s2, s3)
    }

    /// 绘制次序 = 场景树全序 `(z_index, NodeData::order)` 的投影，且与独立重算一致。
    #[test]
    fn criterion_extract_draw_order_matches_scene_key_order() {
        let keys = KeyMap::new();
        let (mut tree, s1, s2, s3) = fixture(&keys);

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);

        let h1 = ex.handle_of(s1).expect("s1 上屏");
        let h2 = ex.handle_of(s2).expect("s2 上屏");
        let h3 = ex.handle_of(s3).expect("s3 上屏");
        assert_eq!(srv.draw_order(), vec![h1, h2, h3]);

        // 全 z 相同 → 次序只由遍历序决定。
        for node in [s1, s2, s3] {
            let handle = ex.handle_of(node).expect("上屏");
            let item = srv.item(handle).expect("有渲染物");
            assert_eq!(item.z, 0);
            assert_eq!(item.order, tree.get(node).expect("节点在树里").order);
        }
        assert_eq!(srv.draw_order(), expected_draw_order(&tree, &ex));
    }

    /// 空闲帧（树上什么都没有改）：绘制次序逐帧完全相同，且不重建任何渲染物。
    #[test]
    fn criterion_extract_draw_order_idle_frames_unchanged() {
        let keys = KeyMap::new();
        let (mut tree, _s1, _s2, _s3) = fixture(&keys);

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();

        let first = {
            let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
            assert_synced(&ex, &srv, &stats);
            assert_eq!(stats.created, 3);
            assert_eq!(stats.destroyed, 0);
            srv.draw_order()
        };

        for index in 2..=5 {
            advance(&mut tree);
            let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, index);
            assert_synced(&ex, &srv, &stats);
            assert_eq!(stats.created, 0, "空闲帧不该新建渲染物");
            assert_eq!(stats.destroyed, 0, "空闲帧不该销毁渲染物");
            assert_eq!(stats.reused, 3, "空闲帧应当复用全部句柄");
            assert_eq!(stats.swept, 0);
            assert_eq!(srv.draw_order(), first, "空闲帧绘制次序必须逐帧相同");
        }
        assert_eq!(srv.counters().created, 3, "5 帧下来只应创建 3 个渲染物");
    }

    /// 同一个兄弟组内用 `move_child` 换位：**下一帧**次序生效，且句柄不换代。
    #[test]
    fn criterion_extract_draw_order_move_child_takes_effect_without_recreate() {
        let keys = KeyMap::new();
        let (mut tree, s1, s2, _s3) = fixture(&keys);

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();

        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        let h1 = ex.handle_of(s1).expect("s1 上屏");
        let h2 = ex.handle_of(s2).expect("s2 上屏");
        let h3 = ex.handle_of(_s3).expect("s3 上屏");
        assert_eq!(srv.draw_order(), vec![h1, h2, h3]);

        // 把 s2 挪到第一位（结构变更，下一帧落地）。
        tree.move_child(s2, 0);
        advance(&mut tree);
        // 换位的语义不是"order 变小"，而是"兄弟数组顺序 == order 升序"：
        // 场景层会把整个兄弟组的排序键重写一遍。
        let o1 = tree.get(s1).expect("s1 在树里").order;
        let o2 = tree.get(s2).expect("s2 在树里").order;
        let o3 = tree.get(_s3).expect("s3 在树里").order;
        assert!(
            o2 < o1 && o1 < o3,
            "换位后兄弟排序键必须与数组顺序一致：s2={o2} s1={o1} s3={o3}"
        );

        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.created, 0, "换位不该重建渲染物");
        assert_eq!(stats.destroyed, 0);
        assert_eq!(ex.handle_of(s2), Some(h2), "换位不该换句柄");
        assert_eq!(ex.handle_of(s1), Some(h1));
        assert_eq!(srv.draw_order(), vec![h2, h1, h3]);
        assert_eq!(srv.draw_order(), expected_draw_order(&tree, &ex));
    }

    /// 同结构的新会话（新提取器 + 新后端）能复现同一绘制次序。
    #[test]
    fn criterion_extract_draw_order_reproducible_across_sessions() {
        let mut runs: Vec<Vec<ItemHandle>> = Vec::new();
        for _ in 0..2 {
            let keys = KeyMap::new();
            let (mut tree, _s1, _s2, _s3) = fixture(&keys);

            let mut ex = RenderExtractor::new();
            let mut srv = NullRenderServer::new();
            let mut out = Vec::new();
            for index in 1..=3 {
                let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, index);
                assert_synced(&ex, &srv, &stats);
                advance(&mut tree);
            }
            assert_eq!(srv.counters().created, 3);
            runs.push(srv.draw_order());
        }
        assert_eq!(runs[0], runs[1], "同结构场景跨会话必须得到同一绘制次序");
    }

    /// `order` 取的是场景层**兄弟排序键**，不是"第几个被遍历到"的序号。
    ///
    /// 两个场景的遍历位置相同、父节点相同，只是**插入先后**不同：
    /// 若本层把 `order` 当成前序下标，两个场景会给出同一个 order（都取 1），
    /// 这一条就会失败。
    #[test]
    fn criterion_extract_order_semantics_is_scene_sibling_key() {
        let order_of = |tree: &SceneTree, node: NodeId| tree.get(node).expect("节点在树里").order;

        // 场景甲：先建 A，再建 A 的子，再建 B。
        let keys_a = KeyMap::new();
        keys_a.set(ResId::new(1), key(1, 1));
        let mut tree_a = SceneTree::new("root");
        let a = add_root_container(&mut tree_a, "a");
        let a_child = add_sprite(&mut tree_a, a, "a_child", ResId::new(1));
        let b = add_root_sprite(&mut tree_a, "b", ResId::new(1));
        assert!(order_of(&tree_a, a_child) < order_of(&tree_a, b));

        let mut ex_a = RenderExtractor::new();
        let mut srv_a = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex_a, &mut tree_a, &keys_a, &mut srv_a, &mut out, 1);
        assert_synced(&ex_a, &srv_a, &stats);
        assert_eq!(
            srv_a.item(ex_a.handle_of(a_child).unwrap()).unwrap().order,
            order_of(&tree_a, a_child)
        );

        // 场景乙：先建 A 与 B，最后才建 A 的子 —— 遍历位置同为第 2 个。
        let keys_b = KeyMap::new();
        keys_b.set(ResId::new(1), key(1, 1));
        let mut tree_b = SceneTree::new("root");
        let a2 = add_root_container(&mut tree_b, "a");
        let b2 = add_root_sprite(&mut tree_b, "b", ResId::new(1));
        let a2_child = add_sprite(&mut tree_b, a2, "a_child", ResId::new(1));
        assert!(order_of(&tree_b, b2) < order_of(&tree_b, a2_child));

        let mut ex_b = RenderExtractor::new();
        let mut srv_b = NullRenderServer::new();
        let stats = step(&mut ex_b, &mut tree_b, &keys_b, &mut srv_b, &mut out, 1);
        assert_synced(&ex_b, &srv_b, &stats);

        let order_a = srv_a.item(ex_a.handle_of(a_child).unwrap()).unwrap().order;
        let order_b = srv_b.item(ex_b.handle_of(a2_child).unwrap()).unwrap().order;
        assert_eq!(order_a, order_of(&tree_a, a_child));
        assert_eq!(order_b, order_of(&tree_b, a2_child));
        assert_ne!(
            order_a, order_b,
            "同一遍历位置上，order 应当反映各场景自己的兄弟排序键"
        );
    }

    /// `z_index` 决定分组：小 z 在前，同 z 内仍按 `order` 排。
    #[test]
    fn criterion_extract_draw_order_z_index_grouping() {
        let keys = KeyMap::new();
        let (mut tree, s1, s2, s3) = fixture(&keys);
        // 遍历序：s1 → s2 → s3；层号：s1=7、s2=-1、s3=-1。
        set_prop(&mut tree, s1, PROP_Z_INDEX, Value::I64(7));
        set_prop(&mut tree, s2, PROP_Z_INDEX, Value::I64(-1));
        set_prop(&mut tree, s3, PROP_Z_INDEX, Value::I64(-1));
        advance(&mut tree);

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);

        let h1 = ex.handle_of(s1).expect("s1 上屏");
        let h2 = ex.handle_of(s2).expect("s2 上屏");
        let h3 = ex.handle_of(s3).expect("s3 上屏");
        assert_eq!(
            srv.draw_order(),
            vec![h2, h3, h1],
            "z=-1 的两个在前（按 order 排），z=7 的在后"
        );
        assert_eq!(srv.item(h1).unwrap().z, 7);
        assert_eq!(srv.draw_order(), expected_draw_order(&tree, &ex));
    }
}

// ---------------------------------------------------------------- 准则三：节点增删无泄漏

mod lifecycle {
    use super::*;

    /// 新增节点：下一帧建渲染物，映射表与后端同时 +1。
    #[test]
    fn criterion_extract_add_node_creates_item_next_frame() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let s1 = add_root_sprite(&mut tree, "s1", ResId::new(1));

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        assert_eq!((stats.fresh, srv.len()), (1, 1));

        let s2 = add_root_sprite(&mut tree, "s2", ResId::new(1));
        assert_eq!(ex.handle_of(s2), None, "结构变更要到下一帧才可遍历");
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.fresh, 1);
        assert_eq!(stats.reused, 1);
        assert_eq!(srv.len(), 2);
        assert!(ex.handle_of(s1).is_some() && ex.handle_of(s2).is_some());
        assert_ne!(ex.handle_of(s1), ex.handle_of(s2), "两个节点不得共用一个句柄");
    }

    /// 删节点：条目被销毁，映射表与后端同时归零（无泄漏、无悬垂）。
    #[test]
    fn criterion_extract_remove_node_destroys_item_no_leak() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let keeper = add_root_sprite(&mut tree, "keeper", ResId::new(1));
        let doomed = add_root_sprite(&mut tree, "doomed", ResId::new(1));

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        let doomed_handle = ex.handle_of(doomed).expect("doomed 上屏");

        tree.remove_node(doomed, false);
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.swept, 1, "被删节点的条目应由清扫路径摘掉");
        assert_eq!(stats.destroyed, 1);
        assert_eq!(ex.handle_of(doomed), None);
        assert_eq!(srv.item(doomed_handle), None, "后端里不得留下孤儿渲染物");
        assert_eq!(srv.len(), 1, "只剩 keeper");
        assert!(ex.handle_of(keeper).is_some(), "无关节点不该被误伤");
        assert_eq!(srv.counters().destroyed, 1);
    }

    /// 整棵子树被摘：子孙的条目全部被清扫，一个都不许留在后端。
    #[test]
    fn criterion_extract_remove_subtree_sweeps_descendants() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let outside = add_root_sprite(&mut tree, "outside", ResId::new(1));
        let branch = add_root_container(&mut tree, "branch");
        let leaf_a = add_sprite(&mut tree, branch, "leaf_a", ResId::new(1));
        let leaf_b = add_sprite(&mut tree, branch, "leaf_b", ResId::new(1));

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(srv.len(), 3);

        tree.remove_node(branch, false);
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.swept, 2, "枝叶两个渲染物应当被一次清扫掉");
        assert_eq!(stats.destroyed, 2);
        assert_eq!(ex.handle_of(leaf_a), None);
        assert_eq!(ex.handle_of(leaf_b), None);
        assert_eq!(srv.len(), 1, "只该剩 outside");
        assert!(ex.handle_of(outside).is_some());
        assert_eq!(srv.counters().created, 3);
        assert_eq!(srv.counters().destroyed, 2);
    }

    /// `keep_children = true`：只有被删节点自己该消失，孩子被提升到祖父下继续渲染。
    ///
    /// 这条专治"清扫范围划错"：把整棵子树都当死条目扫掉，或反过来漏扫被删节点。
    #[test]
    fn criterion_extract_keep_children_removal_only_sweeps_removed_node() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        // parent 自身也是可渲染物，这样"被删节点自己"与"它的孩子"都要区分。
        let parent = add_root_sprite(&mut tree, "parent", ResId::new(1));
        let child = add_sprite(&mut tree, parent, "child", ResId::new(1));

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(srv.len(), 2);
        let child_handle = ex.handle_of(child).expect("child 上屏");

        tree.remove_node(parent, true);
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);

        assert_eq!(stats.swept, 1, "只有被删的 parent 该被清扫");
        assert_eq!(stats.destroyed, 1);
        assert_eq!(ex.handle_of(parent), None);
        assert_eq!(
            ex.handle_of(child),
            Some(child_handle),
            "被提升的孩子应当沿用原句柄（不是销毁重建）"
        );
        assert_eq!(srv.len(), 1);
        assert_eq!(srv.counters().created, 2, "孩子不得被重建");
    }

    /// 资源消失：节点还在树里，但解析不出资源键 → 条目销毁、且**不算泄漏**。
    #[test]
    fn criterion_extract_resource_disappear_destroys_item() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let sprite = add_root_sprite(&mut tree, "sprite", ResId::new(1));

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        let handle = ex.handle_of(sprite).expect("sprite 上屏");
        assert_eq!(srv.len(), 1);

        // 资源槽位被回收（或纹理被解绑）：节点还在，但已不可渲染。
        assert!(keys.remove(ResId::new(1)));
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.dropped, 1, "走的是'本帧不再可渲染'这条路径，不是清扫");
        assert_eq!(stats.swept, 0);
        assert_eq!(stats.destroyed, 1);
        assert_eq!(ex.handle_of(sprite), None);
        assert_eq!(srv.item(handle), None);
        assert!(srv.is_empty());

        // 解绑纹理属性：同一后果（不可渲染就是不可渲染，不看原因）。
        set_prop(&mut tree, sprite, PROP_TEXTURE, ResId::UNBOUND.to_value());
        keys.set(ResId::new(1), key(1, 1));
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 3);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.created, 0, "texture=0 是未绑定，不该建渲染物");
        assert!(srv.is_empty());
    }

    /// 资源换代：同一个节点换到别的资源键 → 显式销毁旧句柄 + 建新句柄，且不串味。
    #[test]
    fn criterion_extract_resource_rebind_creates_new_handle_no_alias() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let sprite = add_root_sprite(&mut tree, "sprite", ResId::new(1));

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        let old = ex.handle_of(sprite).expect("sprite 上屏");

        // 资源换代：同一个 ResId 换成另一代键（旧键作废）。
        keys.set(ResId::new(1), key(1, 2));
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);

        let new = ex.handle_of(sprite).expect("换代后仍是上屏状态");
        assert_eq!(stats.rebound, 1, "换代应当记一次 rebound");
        assert_eq!(stats.created, 1);
        assert_eq!(stats.destroyed, 1);
        assert_ne!(new, old, "换代必须换句柄：旧句柄永不复活");
        assert_eq!(srv.item(old), None, "旧句柄必须已销毁");
        assert_eq!(
            srv.item(new).expect("新渲染物在").key,
            key(1, 2),
            "新句柄必须挂新资源键"
        );
        assert_eq!(srv.len(), 1, "换代不是复制：任何时刻只有一个渲染物");
        assert_eq!(srv.counters().created, 2);
        assert_eq!(srv.counters().destroyed, 1);

        // 再绑回旧键：仍须换代（键不等就重建），不能复用任何历史句柄。
        keys.set(ResId::new(1), key(1, 3));
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 3);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.rebound, 1);
        let third = ex.handle_of(sprite).expect("仍是上屏状态");
        assert_ne!(third, old);
        assert_ne!(third, new);
        assert_eq!(srv.len(), 1);
    }

    /// 长时间 churn（增 / 删 / 摘子树 / 资源消失与换代交替）：
    /// 每一帧映射表与后端都必须**逐条对齐**，绝不累积僵尸条目。
    #[test]
    fn criterion_extract_long_churn_keeps_map_and_server_in_sync() {
        let keys = KeyMap::new();
        for slot in 1..=4 {
            keys.set(ResId::new(slot), key(slot, 1));
        }

        let mut tree = SceneTree::new("root");
        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();

        // 帧 1：4 个精灵（1 个挂在可摘的子树下）。
        let branch = add_root_container(&mut tree, "branch");
        let sprites: Vec<NodeId> = (1..=4)
            .map(|slot| {
                let parent = if slot == 4 { branch } else { tree.root() };
                add_sprite(&mut tree, parent, &format!("s{slot}"), ResId::new(slot))
            })
            .collect();
        let mut frame = 1;
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, frame);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(srv.len(), 4);

        // 帧 2：摘掉整棵 branch（含 s4）。
        tree.remove_node(branch, false);
        advance(&mut tree);
        frame += 1;
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, frame);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(srv.len(), 3);

        // 帧 3：删 s1、加 s5。
        tree.remove_node(sprites[0], false);
        add_root_sprite(&mut tree, "s5", ResId::new(2));
        advance(&mut tree);
        frame += 1;
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, frame);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(srv.len(), 3);
        assert_eq!(stats.fresh, 1, "只有 s5 是新建的");
        assert_eq!(stats.swept, 1, "只有 s1 是被清扫的");

        // 帧 4：s2 的资源消失。
        keys.remove(ResId::new(2));
        advance(&mut tree);
        frame += 1;
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, frame);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(
            srv.len(),
            1,
            "s2 与 s5 共用 ResId(2)：该键一消失，两个渲染物同时下场"
        );

        // 帧 5：资源回来 + s3 换代。
        keys.set(ResId::new(2), key(2, 1));
        keys.set(ResId::new(3), key(3, 2));
        advance(&mut tree);
        frame += 1;
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, frame);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(srv.len(), 3, "s2/s5 回归 + s3 换代：仍是 3 个");
        assert_eq!(stats.rebound, 1);

        // 帧 6~12：静止抖动（每帧都在跑提取，但树与资源都不变）。
        for _ in 6..=12 {
            advance(&mut tree);
            let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, frame + 1);
            frame += 1;
            assert_synced(&ex, &srv, &stats);
            assert_eq!(stats.created, 0);
            assert_eq!(stats.destroyed, 0);
            assert_eq!(stats.swept, 0);
            assert_eq!(stats.dropped, 0);
        }

        // 全程结束后：后端里的每一个渲染物都在映射表里（反向也不缺）。
        assert_eq!(ex.map().len(), srv.len());
        assert_eq!(ex.map().len(), 3, "churn 结束后只该剩 s2 / s3 / s5");
        assert_eq!(srv.counters().ignored_ops, 0);
        assert_eq!(
            srv.len(),
            srv.counters().created as usize - srv.counters().destroyed as usize,
            "后端存活数必须等于 累计创建 - 累计销毁（生命周期账要对得上）"
        );
    }
}

// ---------------------------------------------------------------- 记账、缓冲与生产资源源

mod bookkeeping {
    use super::*;

    /// 记账自洽：每一帧 `created == fresh + rebound`、`pushed == created + reused`、
    /// `destroyed == rebound + dropped + swept`。
    #[test]
    fn criterion_extract_stats_consistent_every_frame() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let container = add_root_container(&mut tree, "container");
        add_sprite(&mut tree, container, "a", ResId::new(1));
        add_sprite(&mut tree, container, "b", ResId::new(1));

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();

        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        assert!(stats.is_consistent(), "第 1 帧记账不自洽：{stats:?}");
        assert_eq!(
            stats.nodes_visited,
            tree.len(),
            "遍历应覆盖整棵树（含根节点）"
        );
        assert_eq!(stats.nodes_visited, 4, "根 + 容器 + 两个精灵");
        assert_eq!(stats.pushed, 2);
        assert_eq!((stats.created, stats.fresh, stats.reused), (2, 2, 0));
        assert_eq!((stats.destroyed, stats.dropped, stats.swept), (0, 0, 0));
        assert_eq!(stats.map_len, 2);
        assert_eq!(srv.counters().frames, 1, "提交过一帧");

        // 第 2 帧：全复用、零生命周期动作。
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);
        assert!(stats.is_consistent(), "第 2 帧记账不自洽：{stats:?}");
        assert_eq!((stats.created, stats.destroyed), (0, 0));
        assert_eq!(stats.reused, 2);
        assert_eq!(stats.map_len, 2);
        assert_eq!(srv.counters().frames, 2, "两帧应当提交两次");
    }

    /// 稳定树上不重建：累计 `created` 恒等于首次的渲染物数，句柄逐帧不变。
    #[test]
    fn criterion_extract_no_recreate_on_stable_tree() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let sprite = add_root_sprite(&mut tree, "sprite", ResId::new(1));

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();

        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        let handle = ex.handle_of(sprite).expect("上屏");

        for index in 2..=6 {
            advance(&mut tree);
            let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, index);
            assert_synced(&ex, &srv, &stats);
            assert_eq!(stats.created, 0);
            assert_eq!(stats.destroyed, 0);
            assert_eq!(ex.handle_of(sprite), Some(handle), "句柄必须稳定");
            assert_eq!(ex.frames(), index, "提取帧序号应当逐帧递增");
        }
        assert_eq!(srv.counters().created, 1);
        assert_eq!(srv.counters().destroyed, 0);
    }

    /// 输出缓冲里是本帧**全量快照**：每个渲染物恰好一条属性命令，末条是 `Submit`。
    #[test]
    fn criterion_extract_out_buffer_holds_full_snapshot() {
        let keys = KeyMap::new();
        for slot in 1..=3 {
            keys.set(ResId::new(slot), key(slot, 1));
        }

        let mut tree = SceneTree::new("root");
        for slot in 1..=3 {
            add_root_sprite(&mut tree, &format!("s{slot}"), ResId::new(slot));
        }

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);

        // 3 条 CreateItem + 3×4 条属性 + 1 条 Submit。
        assert_eq!(count_creates(&out), 3);
        assert_eq!(count_transforms(&out), 3);
        assert_eq!(out.len(), 3 + 3 * 4 + 1);
        assert!(
            matches!(out.last(), Some(RenderCommand::Submit { .. })),
            "命令流必须以 Submit 收尾"
        );

        // 每条 SetTransform 都指向一个**不同**的存活句柄（没有重复推送 / 没有多推）。
        let mut pushed: Vec<ItemHandle> = Vec::new();
        for cmd in &out {
            if let RenderCommand::SetTransform { handle, .. } = cmd {
                pushed.push(*handle);
            }
        }
        pushed.sort();
        pushed.dedup();
        assert_eq!(pushed.len(), 3, "每个渲染物恰好推一条变换");
        for handle in pushed {
            assert!(srv.item(handle).is_some());
        }
    }

    /// 输出缓冲跨帧复用，且 `submit_into` **先清空**：上一帧的生命周期命令不得残留。
    #[test]
    fn criterion_extract_out_buffer_reused_across_frames() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));
        keys.set(ResId::new(2), key(2, 1));

        let mut tree = SceneTree::new("root");
        add_root_sprite(&mut tree, "s1", ResId::new(1));
        add_root_sprite(&mut tree, "s2", ResId::new(2));

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(count_creates(&out), 2);

        // 第 2 帧：两个资源都消失 → 两条 DestroyItem；不带任何 Create 与属性。
        keys.remove(ResId::new(1));
        keys.remove(ResId::new(2));
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(count_creates(&out), 0);
        assert_eq!(count_destroys(&out), 2);
        assert_eq!(count_transforms(&out), 0);
        assert_eq!(out.len(), 2 + 1, "两条销毁 + 一条 Submit");
        assert!(matches!(out.last(), Some(RenderCommand::Submit { .. })));
        let capacity_after_second = out.capacity();

        // 第 3 帧：资源回来了 → 只该有本帧的 Create；上一帧的两条 Destroy 不得残留。
        keys.set(ResId::new(1), key(1, 1));
        keys.set(ResId::new(2), key(2, 1));
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 3);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(count_destroys(&out), 0, "上一帧的命令必须已被清空");
        assert_eq!(count_creates(&out), 2);
        assert_eq!(count_transforms(&out), 2);
        assert!(
            out.capacity() >= capacity_after_second,
            "输出缓冲应当跨帧复用（容量不缩水）"
        );
    }

    /// 遍历缓冲跨帧复用：稳定树上第 8 帧的容量与第 2 帧相同（热路径不分配）。
    #[test]
    fn criterion_extract_scratch_buffers_do_not_grow() {
        let keys = KeyMap::new();
        keys.set(ResId::new(1), key(1, 1));

        let mut tree = SceneTree::new("root");
        let branch = add_root_container(&mut tree, "branch");
        add_root_sprite(&mut tree, "s1", ResId::new(1));
        add_sprite(&mut tree, branch, "s2", ResId::new(1));
        add_sprite(&mut tree, branch, "s3", ResId::new(1));

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();

        let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        let warm = ex.scratch();
        assert!(warm.order_capacity >= tree.len());
        assert!(warm.stack_capacity >= 1);

        for index in 2..=8 {
            advance(&mut tree);
            let stats = step(&mut ex, &mut tree, &keys, &mut srv, &mut out, index);
            assert_synced(&ex, &srv, &stats);
        }
        let steady = ex.scratch();
        assert_eq!(
            (steady.order_capacity, steady.stack_capacity),
            (warm.order_capacity, warm.stack_capacity),
            "稳定树上遍历缓冲容量不该增长（热路径每帧分配是纪律问题）"
        );
    }

    /// 生产路径：`ResourceTable` 直接作为资源键来源。
    ///
    /// 钉住两件事：①提取层只通过 `RenderKeySource` 认识资源系统（不引 `nes-asset`）；
    /// ②"未绑定 / 已回收槽位 / 空键"三种情形一律**不建渲染物**，而不是报错。
    #[test]
    fn criterion_extract_production_source_reads_resource_table() {
        let mut table = ResourceTable::new();
        let dangling = ResId::new(7);
        assert_eq!(
            table.ensure_dangling(dangling),
            Some(dangling),
            "悬垂槽位应当能被创建"
        );
        // 槽位存在但没有可渲染资源。
        assert!(table.entry(dangling).is_some());
        assert_eq!(table.render_key(dangling), None);
        // 从未声明的槽位：条目都没有。
        assert!(table.entry(ResId::new(9)).is_none());
        assert_eq!(table.render_key(ResId::new(9)), None);
        // 未绑定（Resource(0)）与"空键"后果一致：不可渲染。
        assert_eq!(table.renderable_key(ResId::UNBOUND), None);

        let mut tree = SceneTree::new("root");
        let sprite = add_root_sprite(&mut tree, "sprite", dangling);

        let mut ex = RenderExtractor::new();
        let mut srv = NullRenderServer::new();
        let mut out = Vec::new();
        // 注意：这里传的是 `&table` —— 生产路径的替身是真实资源表。
        let stats = step(&mut ex, &mut tree, &table, &mut srv, &mut out, 1);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(
            stats.created, 0,
            "悬垂纹理槽位不该建渲染物（不可渲染不是错误）"
        );
        assert!(srv.is_empty());
        assert_eq!(ex.handle_of(sprite), None);

        // 同一次提取里，即便树里混了个容器节点，也照样什么都不建。
        add_root_container(&mut tree, "container");
        advance(&mut tree);
        let stats = step(&mut ex, &mut tree, &table, &mut srv, &mut out, 2);
        assert_synced(&ex, &srv, &stats);
        assert_eq!(stats.pushed, 0);
        assert_eq!((stats.created, stats.destroyed), (0, 0));
    }
}
