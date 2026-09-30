//! M3 出口准则验收：属性里的资源引用接入全局注册表（`nes-asset::AssetRegistry`）。
//!
//! M2 里 `Sprite2D.texture = Resource(7)` 只是一个场景内占位号；M3 之后这个号
//! 经 `ResourceTable` 变成"有路径、有类别、有加载状态、有运行时身份"的真引用。
//!
//! 四条准则：
//! 1. **Resource 键接真注册表**：声明 → 绑定 → 就绪，槽位号 / `AssetKey` /
//!    渲染侧身份三层可互查；未声明的号降级为悬垂项而不是报错。
//! 2. **热重载**：改文件后同槽位版本递增、`AssetKey` 不换、只有真变过的槽位脏；
//!    只 touch 不改内容不算重载；源消失转 `Failed` 而不是 panic。
//! 3. **依赖与引用计数闭环**：场景 → 纹理的依赖边可查，释放到 0 只入队，
//!    帧末才卸载，且任何仍被存活者依赖的资源不卸载；成环被点名而非致命。
//! 4. **资源声明随场景往返**：写盘 → 读回 → 逐槽位复原（一个不漂）；
//!    旧场景（无 `resources` 段）照样能读。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use nes_asset::{
    AssetKind, AssetLoader, AssetPath, AssetRegistry, LoadError, MemoryLoader, Stamp, StateTag,
};
use nes_scene::resources::{ResId, ResourceTable};
use nes_scene::scene_io::{instantiate_doc_with_resources, parse_ron, PackOptions, PackedScene};
use nes_scene::{NodeKind, SceneTree, Value};

// ---------- 测试专用：可在注册表持有加载器的同时改写内容 ----------
//
// `AssetRegistry::new` 会拿走加载器的所有权，测试就再也改不了文件了。
// 注册表只要求 `impl AssetLoader + 'static`，于是这里包一层共享句柄：
// 加载器仍是内存的（`revision` 由写入显式递增，不依赖时钟，热重载可在同一
// 毫秒内完成），测试线程留着 `Rc<RefCell<..>>` 就能模拟"用户在编辑器外改了资源"。
#[derive(Clone)]
struct SharedLoader(Rc<RefCell<MemoryLoader>>);

impl SharedLoader {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(MemoryLoader::new())))
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        self.0.borrow_mut().write(path, bytes.to_vec());
    }

    fn touch(&self, path: &str) {
        assert!(self.0.borrow_mut().touch(path), "touch 的文件必须已存在");
    }

    fn remove(&self, path: &str) {
        assert!(self.0.borrow_mut().remove(path), "删除的文件必须已存在");
    }
}

impl AssetLoader for SharedLoader {
    fn stamp(&mut self, path: &AssetPath) -> Result<Stamp, LoadError> {
        self.0.borrow_mut().stamp(path)
    }

    fn read(&mut self, path: &AssetPath) -> Result<(Arc<[u8]>, Stamp), LoadError> {
        self.0.borrow_mut().read(path)
    }
}

// ---------- 样例场景 ----------
//
// root
// └─ Hero (Sprite2D)  texture = Resource(7)

const HERO_TEXTURE: &str = "Textures/hero.png";

fn hero_scene() -> SceneTree {
    let mut tree = SceneTree::new("root");
    let hero = tree.add_node(tree.root(), "Hero", NodeKind::Sprite2D);
    tree.apply_pending();
    tree.set_prop(hero, "texture", Value::Resource(7)).unwrap();
    tree
}

fn hero_of(tree: &SceneTree) -> Value {
    tree.preorder()
        .into_iter()
        .filter(|n| tree.name(*n) == Some("Hero"))
        .find_map(|n| tree.props(n).and_then(|s| s.get("texture")).cloned())
        .expect("样例场景里有 Hero.texture")
}

// ============================================================ 准则 1

/// 声明过的槽位：绑定后槽位号 → `AssetKey` → 渲染身份三层齐备，且场景持有它。
#[test]
fn criterion_1_declared_texture_reaches_the_registry() {
    let loader = SharedLoader::new();
    loader.write(HERO_TEXTURE, b"png-bytes-v1");

    let mut table = ResourceTable::new();
    let slot = ResId::new(7);
    table
        .declare_at(slot, HERO_TEXTURE, AssetKind::Texture)
        .unwrap();

    let mut reg = AssetRegistry::new(loader.clone());
    let report = table.bind(&mut reg);

    assert!(report.is_clean(), "绑定不该有失败：{report:?}");
    assert_eq!(report.registered, 1);
    assert_eq!(report.loaded, vec![slot]);

    let key = table.key_of(slot).expect("绑定后必有运行时身份");
    assert_eq!(reg.path_of(key).map(|p| p.as_str()), Some(HERO_TEXTURE));
    assert_eq!(reg.state_of(key).map(|s| s.tag()), Some(StateTag::Ready));
    assert_eq!(table.state_of(slot), Some(StateTag::Ready));
    assert_eq!(table.version_of(slot), Some(1), "首次加载版本为 1");
    assert_eq!(reg.refs(key), Some(1), "场景持有 = 引用计数 1");
    assert!(table.entry(slot).unwrap().is_held());

    // 渲染/音频侧拿到的是一份自洽快照，不需要同时持有表与注册表。
    let view = table.view(slot).expect("视图");
    assert_eq!(view.path.as_ref().map(|p| p.as_str()), Some(HERO_TEXTURE));
    assert_eq!(view.kind, Some(AssetKind::Texture));
    assert_eq!(view.key, Some(key));
    assert_eq!(view.render_key, key.as_render_key());
    assert!(view.is_ready() && view.held);

    // 未绑定的 0 号不是引用，不会进表。
    assert!(table.entry(ResId::UNBOUND).is_none());

    // 重复绑定幂等：不重新注册、不叠加引用计数。
    let again = table.bind(&mut reg);
    assert_eq!(again.registered, 0, "重复绑定不该新增键");
    assert_eq!(reg.refs(key), Some(1), "重复绑定不该叠加引用计数");
}

/// 被引用但没声明的槽位：降级为悬垂项，不报错、不丢数据、不拖累别的槽位。
#[test]
fn criterion_1_undeclared_reference_degrades_to_dangling() {
    let mut tree = hero_scene();
    let gun = tree.add_node(tree.root(), "Gun", NodeKind::Sprite2D);
    tree.apply_pending();
    tree.set_prop(gun, "texture", Value::Resource(11)).unwrap();

    let mut table = ResourceTable::new();
    table
        .declare_at(ResId::new(7), HERO_TEXTURE, AssetKind::Texture)
        .unwrap();

    let report = table.adopt_tree(&tree);
    assert_eq!(report.adopted, vec![ResId::new(11)]);
    assert_eq!(report.undeclared.len(), 1, "{report:?}");
    assert_eq!(report.undeclared[0].0, ResId::new(11));
    assert_eq!(report.undeclared[0].1, AssetKind::Texture);
    assert_eq!(report.undeclared[0].2, "Gun");
    assert_eq!(report.undeclared[0].3, "texture");
    assert!(table.entry(ResId::new(11)).unwrap().is_dangling());
    assert!(table.entry(ResId::UNBOUND).is_none(), "Resource(0) 不是引用");

    // 悬垂项不参与绑定：它没有路径，绑定不该为它去注册，也不该因此失败。
    let loader = SharedLoader::new();
    loader.write(HERO_TEXTURE, b"png-bytes-v1");
    let mut reg = AssetRegistry::new(loader);
    let bound = table.bind(&mut reg);
    assert!(bound.is_clean(), "{bound:?}");
    assert_eq!(bound.registered, 1, "只注册声明过的那一条");
    assert!(!table.entry(ResId::new(11)).unwrap().is_bound());
}

/// 把音频塞进 Sprite2D 的纹理槽：场景与表自相矛盾，体检必须点名。
#[test]
fn criterion_1_kind_mismatch_is_reported_by_adopt() {
    let tree = hero_scene();
    let mut table = ResourceTable::new();
    table
        .declare_at(ResId::new(7), "Audio/hit.ogg", AssetKind::Audio)
        .unwrap();

    let report = table.adopt_tree(&tree);
    assert_eq!(
        report.mismatches,
        vec![(
            ResId::new(7),
            AssetKind::Texture,
            AssetKind::Audio,
            "Hero".to_string()
        )]
    );
    assert!(report.undeclared.is_empty(), "类别对不上不等于没声明");
    assert!(!report.is_clean(), "冲突场景必须被识别为不干净");
}

// ============================================================ 准则 2

/// 改文件 → 同槽位版本 +1，`AssetKey` 不换，只有这一个槽位脏。
#[test]
fn criterion_2_hot_reload_bumps_version_on_the_same_slot() {
    let loader = SharedLoader::new();
    loader.write(HERO_TEXTURE, b"v1");
    loader.write("Textures/other.png", b"other");

    let mut table = ResourceTable::new();
    table
        .declare_at(ResId::new(1), HERO_TEXTURE, AssetKind::Texture)
        .unwrap();
    table
        .declare_at(ResId::new(2), "Textures/other.png", AssetKind::Texture)
        .unwrap();
    let mut reg = AssetRegistry::new(loader.clone());
    table.bind(&mut reg);
    let _ = table.take_dirty();

    let key = table.key_of(ResId::new(1)).unwrap();
    assert_eq!(table.version_of(ResId::new(1)), Some(1));

    // 没人动文件 → 什么都没发生。
    let quiet = table.poll(&mut reg);
    assert!(quiet.reloaded.is_empty(), "{quiet:?}");
    assert!(!table.has_dirty());

    // 只有 hero 被改写 → 只有 1 号脏。
    loader.write(HERO_TEXTURE, b"v2-with-more-bytes");
    let hot = table.poll(&mut reg);
    assert_eq!(hot.reloaded, vec![(key, 2)], "同 key、新版本");
    assert_eq!(table.take_dirty(), vec![ResId::new(1)]);
    assert_eq!(table.version_of(ResId::new(1)), Some(2));
    assert_eq!(table.state_of(ResId::new(1)), Some(StateTag::Ready));
    assert_eq!(reg.version_of(key), Some(2));
    assert_eq!(table.key_of(ResId::new(1)), Some(key), "热重载不换身份");
    assert_eq!(table.version_of(ResId::new(2)), Some(1), "没变的槽位不动");
    assert!(!table.has_dirty(), "取走后不再脏");
}

/// 只 touch（mtime 变、内容不变）不算重载 —— 否则编辑器一保存就整树抖动。
#[test]
fn criterion_2_touch_without_content_change_is_not_a_reload() {
    let loader = SharedLoader::new();
    loader.write(HERO_TEXTURE, b"v1");

    let mut table = ResourceTable::new();
    table
        .declare_at(ResId::new(1), HERO_TEXTURE, AssetKind::Texture)
        .unwrap();
    let mut reg = AssetRegistry::new(loader.clone());
    table.bind(&mut reg);
    let _ = table.take_dirty();

    loader.touch(HERO_TEXTURE);
    let report = table.poll(&mut reg);
    assert!(report.reloaded.is_empty(), "内容哈希没变就不该重载：{report:?}");
    assert!(report.failed.is_empty());
    assert!(!table.has_dirty());
    assert_eq!(table.version_of(ResId::new(1)), Some(1));
}

/// 源文件消失：槽位转 `Failed`（编辑器可重试），而不是 panic 或静默成功。
#[test]
fn criterion_2_missing_source_marks_the_slot_failed() {
    let loader = SharedLoader::new();
    loader.write(HERO_TEXTURE, b"v1");

    let mut table = ResourceTable::new();
    table
        .declare_at(ResId::new(1), HERO_TEXTURE, AssetKind::Texture)
        .unwrap();
    let mut reg = AssetRegistry::new(loader.clone());
    table.bind(&mut reg);
    let _ = table.take_dirty();

    loader.remove(HERO_TEXTURE);
    let report = table.poll(&mut reg);
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].0, table.key_of(ResId::new(1)).unwrap());
    assert_eq!(table.state_of(ResId::new(1)), Some(StateTag::Failed));
    assert!(!table.has_dirty(), "失败不是可用变更，不该触发重建");
}

// ============================================================ 准则 3

/// 依赖边可查；释放到 0 只入队，帧末一起卸载；表项同步回未加载。
#[test]
fn criterion_3_dependency_edges_and_refcounts_close_the_loop() {
    let loader = SharedLoader::new();
    loader.write(HERO_TEXTURE, b"png");
    loader.write("Scenes/level.ron", b"scene");

    let mut table = ResourceTable::new();
    let tex = table
        .declare_at(ResId::new(1), HERO_TEXTURE, AssetKind::Texture)
        .unwrap();
    let scene = table
        .declare_at(ResId::new(2), "Scenes/level.ron", AssetKind::Scene)
        .unwrap();
    table.set_deps(scene, vec![tex]).unwrap();

    let mut reg = AssetRegistry::new(loader);
    let report = table.bind(&mut reg);
    assert!(report.is_clean(), "{report:?}");

    let tex_key = table.key_of(tex).unwrap();
    let scene_key = table.key_of(scene).unwrap();
    assert_eq!(reg.dependencies(scene_key), vec![tex_key]);
    assert_eq!(reg.dependents(tex_key), vec![scene_key]);

    // 释放：两条都归零，但此刻都还活着（只是进了队列）。
    let released = table.release_all(&mut reg);
    assert_eq!(released.len(), 2);
    assert_eq!(reg.refs(tex_key), Some(0));
    assert_eq!(reg.refs(scene_key), Some(0));
    assert!(reg.pending_unload().contains(&tex_key));
    assert!(reg.pending_unload().contains(&scene_key), "同一帧末尾一起走");
    assert_eq!(
        reg.state_of(tex_key).map(|s| s.tag()),
        Some(StateTag::Ready),
        "入队不等于卸载"
    );
    assert_eq!(table.version_of(tex), Some(0), "表侧立即反映不再持有");
    assert!(!table.entry(tex).unwrap().is_held());

    // 帧末沉降真正卸载，并同步表项。
    let unloaded = table.reclaim(&mut reg);
    assert!(unloaded.contains(&tex_key));
    assert!(unloaded.contains(&scene_key));
    assert_eq!(reg.state_of(tex_key).map(|s| s.tag()), Some(StateTag::NotLoaded));
    assert_eq!(table.state_of(tex), Some(StateTag::NotLoaded));
    assert_eq!(table.version_of(tex), Some(0));
}

/// 仍被存活者依赖的资源不卸载：那是依赖图存在的意义。
#[test]
fn criterion_3_dependency_keeps_a_referenced_asset_alive() {
    let loader = SharedLoader::new();
    loader.write(HERO_TEXTURE, b"png");
    loader.write("Scenes/level.ron", b"scene");

    let mut table = ResourceTable::new();
    let tex = table
        .declare_at(ResId::new(1), HERO_TEXTURE, AssetKind::Texture)
        .unwrap();
    let scene = table
        .declare_at(ResId::new(2), "Scenes/level.ron", AssetKind::Scene)
        .unwrap();
    table.set_deps(scene, vec![tex]).unwrap();

    let mut reg = AssetRegistry::new(loader);
    table.bind(&mut reg);
    let tex_key = table.key_of(tex).unwrap();
    let scene_key = table.key_of(scene).unwrap();

    // 只放掉纹理：它被仍存活的场景依赖着，不该进卸载队列。
    assert_eq!(reg.release(tex_key), Some(0));
    assert!(!reg.pending_unload().contains(&tex_key));
    assert!(reg.unload_tick().is_empty(), "存活依赖者钉住整条子树");
    assert_eq!(reg.state_of(tex_key).map(|s| s.tag()), Some(StateTag::Ready));

    // 放掉场景后，整条闭包一起可回收。
    assert_eq!(reg.release(scene_key), Some(0));
    let unloaded = reg.unload_tick();
    assert!(unloaded.contains(&tex_key) && unloaded.contains(&scene_key));
}

/// 成环依赖被点名，但不致命：资源照样加载，依赖错误留在报告里给编辑器修。
#[test]
fn criterion_3_cyclic_dependency_is_reported_not_fatal() {
    let loader = SharedLoader::new();
    loader.write("Data/a.json", b"a");
    loader.write("Data/b.json", b"b");

    let mut table = ResourceTable::new();
    let a = table.declare_at(ResId::new(1), "Data/a.json", AssetKind::Data).unwrap();
    let b = table.declare_at(ResId::new(2), "Data/b.json", AssetKind::Data).unwrap();
    table.set_deps(a, vec![b]).unwrap();
    table.set_deps(b, vec![a]).unwrap();

    let mut reg = AssetRegistry::new(loader);
    let report = table.bind(&mut reg);

    assert_eq!(report.dep_errors.len(), 1, "成环必须被点名：{report:?}");
    assert!(report.failed.is_empty(), "依赖成环不该把加载一起拖死");
    assert!(table.entry(a).unwrap().is_bound());
    assert!(table.entry(b).unwrap().is_bound());
}

// ============================================================ 准则 4

/// 资源声明随场景写盘、读回：逐槽位复原，重新绑定拿到同一个运行时身份。
#[test]
fn criterion_4_resource_declarations_travel_with_the_scene() {
    let tree = hero_scene();
    let mut table = ResourceTable::new();
    table
        .declare_at(ResId::new(7), HERO_TEXTURE, AssetKind::Texture)
        .unwrap();
    table
        .declare_at(ResId::new(9), "Audio/hit.ogg", AssetKind::Audio)
        .unwrap();

    let opts = PackOptions::verbose();
    let packed = PackedScene::pack_with_resources(&tree, &table, &opts);
    let ron = packed.ron().to_string();
    assert!(ron.contains("resources: ["), "声明段必须写出去：\n{ron}");
    assert!(
        ron.contains(r#"Res(id: 7, path: "Textures/hero.png", kind: "Texture")"#),
        "\n{ron}"
    );
    assert_eq!(
        PackedScene::pack_with_resources(&tree, &table, &opts).ron(),
        ron,
        "同一树 + 同一表 → 逐字节相同"
    );

    let (tree2, mut table2, report) = packed.instantiate_with_resources().unwrap();
    assert!(report.is_clean(), "读回即自洽：{report:?}");
    assert_eq!(hero_of(&tree2), Value::Resource(7), "属性里的号还是那个号");
    assert_eq!(
        table2.path_of(ResId::new(7)).map(|p| p.as_str()),
        Some(HERO_TEXTURE)
    );
    assert_eq!(table2.entry(ResId::new(7)).unwrap().kind(), Some(AssetKind::Texture));
    assert_eq!(
        table2.id_of_path(&AssetPath::new("Audio/hit.ogg").unwrap(), AssetKind::Audio),
        Some(ResId::new(9)),
        "槽位号不能重排"
    );

    // 写盘前绑过一次的身份，与读回后重新绑定的身份必须一致。
    let loader = SharedLoader::new();
    loader.write(HERO_TEXTURE, b"png");
    loader.write("Audio/hit.ogg", b"ogg");
    let mut reg_a = AssetRegistry::new(loader.clone());
    table.bind(&mut reg_a);
    let key_before = table.key_of(ResId::new(7)).unwrap();

    let mut reg_b = AssetRegistry::new(loader);
    let bound = table2.bind(&mut reg_b);
    assert!(bound.is_clean(), "{bound:?}");
    assert_eq!(
        table2.key_of(ResId::new(7)),
        Some(key_before),
        "同一个文件 + 同一类别 → 同一个身份，往返不漂"
    );
}

/// 空资源表不写 `resources` 段：M2 时代产出的场景文件在本期逐字节不变。
#[test]
fn criterion_4_empty_table_writes_no_resources_field() {
    let tree = hero_scene();
    let opts = PackOptions::verbose();
    let packed = PackedScene::pack(&tree, &opts);
    assert!(!packed.ron().contains("resources"), "\n{}", packed.ron());

    let (_, table, report) = packed.instantiate_with_resources().unwrap();
    assert_eq!(report.adopted, vec![ResId::new(7)], "悬垂引用照样入表");
    assert!(table.entry(ResId::new(7)).unwrap().is_dangling());
}

/// 旧场景（无 `resources` 段、属性里裸写 `Resource(7)`）照样能读：
/// 号码先以悬垂项留在表里，由编辑器补声明，而不是报错或丢数据。
#[test]
fn criterion_4_legacy_scene_without_resources_section_still_reads() {
    let legacy = r#"Scene(
    version: 1,
    root: Node(
        name: "Hero",
        kind: Sprite2D,
        props: { "texture": Resource(7), },
        children: [],
    ),
)"#;
    let doc = parse_ron(legacy).expect("旧场景必须能解析");
    assert!(doc.resources.is_empty());

    let (tree, table, report) = instantiate_doc_with_resources(&doc).unwrap();
    assert_eq!(hero_of(&tree), Value::Resource(7));
    assert_eq!(table.len(), 1);
    assert!(table.entry(ResId::new(7)).unwrap().is_dangling());
    assert_eq!(report.undeclared.len(), 1, "体检报告要指出缺口：{report:?}");
    assert!(report.mismatches.is_empty());
}
