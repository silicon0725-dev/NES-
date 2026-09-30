//! M3 出口准则与边界的行为测试。
//!
//! 两条出口准则各有一个**同名**测试，可直接 grep：
//! - `reload_broadcasts_same_key_new_version`（改文件 → 订阅者收到同 key 新版本）
//! - `dependents_block_unload`（引用计数与依赖卸载判定）

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use nes_asset::*;

// ------------------------------------------------------------------ 测试用后端

fn tex_reg(reg: &mut AssetRegistry, path: &str) -> AssetKey {
    reg.register_str(path, AssetKind::Texture).expect("路径合法")
}

// ------------------------------------------------------------------ 身份

#[test]
fn register_is_idempotent_and_keys_are_stable() {
    let mut reg = AssetRegistry::new(MemoryLoader::new());
    let a = tex_reg(&mut reg, "Textures/player.png");
    let b = tex_reg(&mut reg, "Textures/player.png");
    assert_eq!(a, b, "同 (path, kind) 必须复用同一 key");
    assert_eq!(reg.len(), 1);

    // 分类属于身份：同路径不同分类 = 不同资源。
    let as_data = reg.register_str("Textures/player.png", AssetKind::Data).unwrap();
    assert_ne!(a, as_data, "分类属于身份，不同分类得到不同 key");
    assert_eq!(reg.len(), 2);

    // 槽位 0 保留给 NIL，真实资源从 1 开始。
    assert!(!a.is_nil());
    assert_eq!(a.slot(), 1);

    // 反查。
    let p = AssetPath::new("Textures/player.png").unwrap();
    assert_eq!(reg.key_of(&p, AssetKind::Texture), Some(a));
    assert_eq!(reg.path_of(a).unwrap().as_str(), "Textures/player.png");
}

#[test]
fn kind_mismatch_and_stale_generation_are_dangling() {
    let mut reg = AssetRegistry::new(MemoryLoader::new());
    let k = tex_reg(&mut reg, "a.png");
    assert!(reg.contains(k));
    // 代际不匹配 = 悬垂。
    assert!(!reg.contains(AssetKey::from_bits((k.generation() as u64 + 7) << 32 | k.slot() as u64, AssetKind::Texture)));
    // 分类不匹配 = 悬垂。
    assert!(!reg.contains(AssetKey::from_bits(k.to_bits(), AssetKind::Audio)));
    // 空键永远不在注册表里。
    assert!(!reg.contains(AssetKey::NIL));
    assert_eq!(reg.refs(AssetKey::NIL), None);
}

#[test]
fn keys_are_slot_ordered() {
    let mut reg = AssetRegistry::new(MemoryLoader::new());
    let a = tex_reg(&mut reg, "a.png");
    let b = tex_reg(&mut reg, "b.png");
    let c = tex_reg(&mut reg, "c.png");
    assert_eq!(reg.keys(), vec![a, b, c], "keys() 必须槽位升序");
    let collected: Vec<AssetKey> = reg.iter().map(|(k, _)| k).collect();
    assert_eq!(collected, vec![a, b, c]);
}

// ------------------------------------------------------------------ 加载

#[test]
fn load_walks_state_machine_and_caches() {
    let mut h = harness();
    let k = tex_reg(&mut h.reg, "a.png");
    assert_eq!(h.reg.state_of(k).unwrap().tag(), StateTag::NotLoaded);
    h.write("a.png", b"hello".to_vec());

    let a1 = h.reg.load(k).expect("应加载成功");
    assert_eq!(a1.version, 1);
    assert_eq!(a1.bytes.as_ref(), b"hello");
    assert_eq!(h.reg.state_of(k).unwrap().tag(), StateTag::Ready);

    // 二次 load：命中缓存，返回同一份 Arc（不重复读盘、不重复发事件）。
    let a2 = h.reg.load(k).expect("应命中缓存");
    assert!(Arc::ptr_eq(&a1, &a2));
    assert_eq!(h.reg.pending_events(), 1, "只有一次 Loaded 事件");

    let delivered = h.reg.dispatch();
    assert!(delivered.is_empty(), "没有订阅者时事件只被清空");
    assert_eq!(h.reg.pending_events(), 0);
}

#[test]
fn missing_source_keeps_key_and_version_zero() {
    let mut h = harness();
    let k = tex_reg(&mut h.reg, "missing.png");
    let err = h.reg.load(k).unwrap_err();
    assert!(matches!(err, LoadError::NotFound(_)), "缺文件应为 NotFound，实际 {err:?}");
    assert_eq!(h.reg.state_of(k).unwrap().tag(), StateTag::Failed);
    assert_eq!(h.reg.version_of(k), Some(0), "从未成功则版本保持 0");
    assert!(h.reg.contains(k), "失败不销毁 key");
    assert_eq!(h.reg.pending_events(), 1, "失败事件已入队");
    let d = h.reg.dispatch();
    assert!(d.is_empty());
}

/// 共享句柄后端：让测试能同时持有 `MemoryLoader` 句柄与注册表。
struct Harness {
    reg: AssetRegistry,
    loader: Rc<RefCell<MemoryLoader>>,
}

/// 无法在注册表内部与外部同时持有加载器，因此提供"共享句柄"后端。
struct SharedMemoryLoader(Rc<RefCell<MemoryLoader>>);

impl AssetLoader for SharedMemoryLoader {
    fn stamp(&mut self, path: &AssetPath) -> Result<Stamp, LoadError> {
        self.0.borrow_mut().stamp(path)
    }
    fn read(&mut self, path: &AssetPath) -> Result<(Arc<[u8]>, Stamp), LoadError> {
        self.0.borrow_mut().read(path)
    }
}

fn harness() -> Harness {
    let loader = Rc::new(RefCell::new(MemoryLoader::new()));
    let reg = AssetRegistry::new(SharedMemoryLoader(Rc::clone(&loader)));
    Harness { reg, loader }
}

impl Harness {
    fn write(&self, path: &str, bytes: impl Into<Vec<u8>>) {
        self.loader.borrow_mut().write(path, bytes);
    }
    fn touch(&self, path: &str) -> bool {
        self.loader.borrow_mut().touch(path)
    }
    fn remove(&self, path: &str) -> bool {
        self.loader.borrow_mut().remove(path)
    }
}

#[test]
fn retry_after_failure_succeeds() {
    let mut h = harness();
    let k = tex_reg(&mut h.reg, "late.png");
    assert!(h.reg.load(k).is_err());
    assert_eq!(h.reg.state_of(k).unwrap().tag(), StateTag::Failed);

    h.write("late.png", b"now here".to_vec());
    let a = h.reg.retry(k).expect("重试应成功");
    assert_eq!(a.version, 1, "首次成功加载版本为 1");
    assert_eq!(a.bytes.as_ref(), b"now here");
    assert_eq!(h.reg.state_of(k).unwrap().tag(), StateTag::Ready);
}

// ------------------------------------------------------------------ 出口准则 1

#[test]
fn reload_broadcasts_same_key_new_version() {
    let mut h = harness();
    let k = tex_reg(&mut h.reg, "Textures/hero.png");
    h.write("Textures/hero.png", b"v1-bytes".to_vec());

    let all = h.reg.subscribe(EventScope::All);
    let only_key = h.reg.subscribe(EventScope::Key(k));

    let first = h.reg.load(k).unwrap();
    assert_eq!(first.version, 1);
    let _ = h.reg.dispatch(); // 清掉 Loaded 事件

    // 改文件。
    h.write("Textures/hero.png", b"v2-bytes-longer".to_vec());
    let report = h.reg.poll_reloads();
    assert_eq!(report.checked, 1);
    assert_eq!(report.reloaded, vec![(k, 2)], "同一 key 版本递增到 2");
    assert!(report.failed.is_empty());

    // key 没变，内容变了。
    assert_eq!(h.reg.version_of(k), Some(2));
    assert_eq!(h.reg.loaded(k).unwrap().bytes.as_ref(), b"v2-bytes-longer");
    assert_eq!(h.reg.path_of(k).unwrap().as_str(), "Textures/hero.png");

    // 订阅者收到新版本。
    let deliveries = h.reg.dispatch();
    let reloaded: Vec<&Delivery> = deliveries
        .iter()
        .filter(|d| d.event.tag() == "Reloaded")
        .collect();
    assert_eq!(reloaded.len(), 2, "两个订阅者各收到一次 Reloaded");
    assert!(reloaded.iter().any(|d| d.subscriber == all));
    assert!(reloaded.iter().any(|d| d.subscriber == only_key));
    for d in &reloaded {
        assert_eq!(d.event.key(), k);
        assert_eq!(d.event.version(), Some(2));
    }
}

#[test]
fn reload_skips_when_content_unchanged() {
    let mut h = harness();
    let k = tex_reg(&mut h.reg, "same.png");
    h.write("same.png", b"content".to_vec());
    h.reg.load(k).unwrap();
    let _ = h.reg.dispatch();

    // 只 touch（mtime 变、内容不变）：不该触发重载。
    assert!(h.touch("same.png"));
    let report = h.reg.poll_reloads();
    assert!(report.is_empty(), "内容未变不得重载，实际 {report:?}");
    assert_eq!(h.reg.version_of(k), Some(1));
}

#[test]
fn reload_failure_marks_failed_and_keeps_key() {
    let mut h = harness();
    let k = tex_reg(&mut h.reg, "gone.png");
    h.write("gone.png", b"data".to_vec());
    h.reg.load(k).unwrap();
    let _ = h.reg.dispatch();

    h.remove("gone.png");
    let report = h.reg.poll_reloads();
    assert_eq!(report.reloaded.len(), 0);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, k);
    assert_eq!(h.reg.state_of(k).unwrap().tag(), StateTag::Failed);
    assert!(h.reg.contains(k), "源消失后 key 仍有效，只是状态变了");

    // 文件回来后可重试。
    h.write("gone.png", b"back".to_vec());
    assert_eq!(h.reg.retry(k).unwrap().version, 2);
}

// ------------------------------------------------------------------ 出口准则 2

#[test]
fn dependents_block_unload() {
    let mut h = harness();
    let scene = h.reg.register_str("Scenes/level.ron", AssetKind::Scene).unwrap();
    let tex = tex_reg(&mut h.reg, "Textures/tile.png");
    h.write("Scenes/level.ron", b"scene".to_vec());
    h.write("Textures/tile.png", b"tile".to_vec());

    h.reg.add_dependency(scene, tex).unwrap();
    assert_eq!(h.reg.dependencies(scene), vec![tex]);
    assert_eq!(h.reg.dependents(tex), vec![scene]);

    // 两者都真正加载起来，才能观测到"卸载"这一步。
    h.reg.load(scene).unwrap();
    h.reg.load(tex).unwrap();
    let _ = h.reg.dispatch();

    // 场景被使用（refs=1），纹理被释放（refs=0）：纹理不得卸载。
    assert_eq!(h.reg.acquire(scene), Some(1));
    assert_eq!(h.reg.acquire(tex), Some(1));
    assert_eq!(h.reg.release(tex), Some(0));
    assert!(!h.reg.is_reclaimable(tex), "仍被存活场景依赖，不可回收");
    assert!(h.reg.pending_unload().is_empty(), "不可回收者不得进队列");
    assert!(h.reg.unload_tick().is_empty());
    assert_eq!(h.reg.state_of(tex).unwrap().tag(), StateTag::Ready, "纹理仍在内存");

    // 场景也释放：依赖子树在同一帧一起沉降（槽位序）。
    assert_eq!(h.reg.release(scene), Some(0));
    assert!(h.reg.is_reclaimable(tex));
    assert_eq!(h.reg.pending_unload(), &[scene, tex]);
    let unloaded = h.reg.unload_tick();
    assert_eq!(unloaded, vec![scene, tex], "场景与其独占纹理同帧卸载");
    assert_eq!(h.reg.state_of(tex).unwrap().tag(), StateTag::NotLoaded);
    assert_eq!(h.reg.version_of(tex), Some(0), "卸载后版本归零");
    assert!(h.reg.unload_tick().is_empty(), "第二次 tick 必须无事发生");
}

#[test]
fn reacquire_cancels_pending_unload() {
    let mut h = harness();
    let k = tex_reg(&mut h.reg, "keep.png");
    h.write("keep.png", b"x".to_vec());
    h.reg.load(k).unwrap();

    h.reg.acquire(k);
    h.reg.release(k);
    assert_eq!(h.reg.pending_unload(), &[k]);

    h.reg.acquire(k);
    assert!(h.reg.pending_unload().is_empty(), "重新引用应撤出卸载队列");
    assert!(h.reg.unload_tick().is_empty());
    assert_eq!(h.reg.state_of(k).unwrap().tag(), StateTag::Ready);
}

#[test]
fn dependencies_reject_cycles_and_self() {
    let mut h = harness();
    let a = h.reg.register_str("a.bin", AssetKind::Data).unwrap();
    let b = h.reg.register_str("b.bin", AssetKind::Data).unwrap();
    assert_eq!(h.reg.add_dependency(a, a), Err(DepError::SelfDependency(a)));
    h.reg.add_dependency(a, b).unwrap();
    assert_eq!(h.reg.add_dependency(b, a), Err(DepError::Cycle(b, a)));
    let unknown = AssetKey::from_bits(9999, AssetKind::Data);
    assert_eq!(h.reg.add_dependency(a, unknown), Err(DepError::Unknown(unknown)));

    // 幂等加入不产生重复边。
    h.reg.add_dependency(a, b).unwrap();
    assert_eq!(h.reg.dependencies(a), vec![b]);
    assert!(h.reg.remove_dependency(a, b));
    assert!(!h.reg.remove_dependency(a, b));
    assert!(h.reg.dependencies(a).is_empty());
}

// ------------------------------------------------------------------ 派发与分类

#[test]
fn subscriber_scopes_filter_correctly() {
    let mut h = harness();
    let tex = tex_reg(&mut h.reg, "t.png");
    let audio = h.reg.register_str("a.ogg", AssetKind::Audio).unwrap();
    h.write("t.png", b"t".to_vec());
    h.write("a.ogg", b"a".to_vec());

    let all = h.reg.subscribe(EventScope::All);
    let kind_tex = h.reg.subscribe(EventScope::Kind(AssetKind::Texture));
    let key_tex = h.reg.subscribe(EventScope::Key(tex));

    h.reg.load(tex).unwrap();
    h.reg.load(audio).unwrap();
    assert_eq!(h.reg.pending_events(), 2);

    let d = h.reg.dispatch();
    // tex 事件命中 3 个订阅者，audio 事件只命中 All。
    assert_eq!(d.len(), 4, "实际 {d:?}");
    assert_eq!(d[0].subscriber, all);
    assert_eq!(d[1].subscriber, kind_tex);
    assert_eq!(d[2].subscriber, key_tex);
    assert_eq!(d[3].subscriber, all);
    assert_eq!(d[3].event.key(), audio);

    // 退订后不再收到。
    assert!(h.reg.unsubscribe(all));
    assert!(!h.reg.unsubscribe(all));
    h.reg.poll_reloads();
    assert!(h.reg.dispatch().is_empty());
}

#[test]
fn render_key_view_shares_identity() {
    let mut reg = AssetRegistry::new(MemoryLoader::new());
    let k = tex_reg(&mut reg, "t.png");
    let view = k.as_render_key().expect("纹理应有渲染视图");
    assert_eq!(view.to_bits(), k.to_bits());
    assert_eq!(AssetKey::from_render_key(view, AssetKind::Texture), Some(k));
    assert_eq!(AssetKey::from_render_key(view, AssetKind::Audio), None);

    let a = reg.register_str("a.ogg", AssetKind::Audio).unwrap();
    assert!(a.as_render_key().is_none(), "非渲染类无渲染视图");
}

#[test]
fn unknown_key_load_is_error() {
    let mut reg = AssetRegistry::new(MemoryLoader::new());
    let ghost = AssetKey::from_bits(42, AssetKind::Texture);
    assert!(matches!(reg.load(ghost), Err(LoadError::UnknownKey(_))));
    assert_eq!(reg.refs(ghost), None);
    assert_eq!(reg.acquire(ghost), None);
    assert_eq!(reg.release(ghost), None);
}

// ------------------------------------------------------------------ 真实文件系统

#[test]
fn fs_loader_roundtrip_and_hot_reload() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("nes_asset_fs");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    let file = root.join("Textures").join("fs.png");
    std::fs::write(&file, b"one").unwrap();

    let mut reg = AssetRegistry::new(FsLoader::new(&root));
    let k = tex_reg(&mut reg, "Textures/fs.png");
    let sub = reg.subscribe(EventScope::All);
    let a = reg.load(k).expect("应能读到真实文件");
    assert_eq!(a.bytes.as_ref(), b"one");
    assert_eq!(a.version, 1);
    let _ = reg.dispatch();

    // 内容改写：poll 应重载同一 key。
    std::fs::write(&file, b"two-longer").unwrap();
    let report = reg.poll_reloads();
    assert_eq!(report.reloaded, vec![(k, 2)], "实际 {report:?}");

    let d = reg.dispatch();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].subscriber, sub);
    assert_eq!(d[0].event.tag(), "Reloaded");
    assert_eq!(reg.loaded(k).unwrap().bytes.as_ref(), b"two-longer");

    // 清理。
    let _ = std::fs::remove_dir_all(&root);
}
