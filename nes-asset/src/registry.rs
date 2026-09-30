//! [`AssetRegistry`]：全局资源注册表。
//!
//! 四件事，一件不掺：**身份**（key 分配与解析）、**状态**（加载状态机）、
//! **关系**（依赖图）、**通知**（事件队列 + 订阅）。解码、线程、GPU 上传都不在这里。

use core::fmt;
use std::collections::HashMap;
use std::sync::Arc;

use crate::data::{LoadedAsset, Stamp};
use crate::event::{AssetEvent, Delivery, EventScope, SubscriberId};
use crate::key::{AssetKey, AssetKind};
use crate::loader::{AssetLoader, LoadError};
use crate::path::{AssetPath, AssetPathError};
use crate::state::{LoadState, StateTag};

/// 依赖关系错误。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DepError {
    /// 键不存在（或已悬垂）。
    Unknown(AssetKey),
    /// 自己依赖自己。
    SelfDependency(AssetKey),
    /// 依赖成环。
    Cycle(AssetKey, AssetKey),
}

impl fmt::Display for DepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(k) => write!(f, "未知资源键：{k:?}"),
            Self::SelfDependency(k) => write!(f, "资源不能依赖自己：{k:?}"),
            Self::Cycle(a, b) => write!(f, "资源依赖成环：{a:?} → {b:?}"),
        }
    }
}

impl std::error::Error for DepError {}

/// 热重载轮询报告。
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct ReloadReport {
    /// 检查过的已就绪资源数。
    pub checked: usize,
    /// 已重载的 `(key, 新版本)`，按 key 升序。
    pub reloaded: Vec<(AssetKey, u32)>,
    /// 重载失败的 `(key, 原因)`，按 key 升序。
    pub failed: Vec<(AssetKey, Arc<str>)>,
}

impl ReloadReport {
    /// 是否有资源真的变了。
    pub fn is_empty(&self) -> bool {
        self.reloaded.is_empty() && self.failed.is_empty()
    }
}

/// 一条资源记录（只读遍历用）。
#[derive(Clone, Debug)]
pub struct Entry {
    /// 稳定身份。
    pub key: AssetKey,
    /// 源路径。
    pub path: AssetPath,
    /// 版本。首次成功加载为 1，之后每次重载 +1；卸载归 0。
    pub version: u32,
    /// 引用计数。
    pub refs: u32,
    /// 加载状态。
    pub state: LoadState,
    /// 最近一次成功读取的内容戳。
    pub stamp: Option<Stamp>,
    /// 本资源依赖的资源（升序、去重）。
    pub deps: Vec<AssetKey>,
}

struct Slot {
    gen: u32,
    entry: Option<Entry>,
}

struct Subscription {
    id: SubscriberId,
    scope: EventScope,
}

/// 全局资源注册表。
pub struct AssetRegistry {
    loader: Box<dyn AssetLoader>,
    slots: Vec<Slot>,
    by_path: HashMap<(AssetPath, AssetKind), AssetKey>,
    subs: Vec<Subscription>,
    next_sub: u64,
    events: Vec<AssetEvent>,
    pending_unload: Vec<AssetKey>,
}

impl AssetRegistry {
    /// 以指定加载后端构造。槽位 0 保留给 [`AssetKey::NIL`]，因此首个真实资源是 `slot = 1`。
    pub fn new(loader: impl AssetLoader + 'static) -> Self {
        Self {
            loader: Box::new(loader),
            slots: vec![Slot {
                gen: 0,
                entry: None,
            }],
            by_path: HashMap::new(),
            subs: Vec::new(),
            next_sub: 1,
            events: Vec::new(),
            pending_unload: Vec::new(),
        }
    }

    /// 加载后端（可变借用，供外部注入测试行为）。
    pub fn loader_mut(&mut self) -> &mut dyn AssetLoader {
        &mut *self.loader
    }

    // ---------------------------------------------------------------- 身份

    /// 注册资源：同一 `(path, kind)` 重复注册返回同一个 key（幂等）。
    pub fn register(&mut self, path: AssetPath, kind: AssetKind) -> AssetKey {
        if let Some(k) = self.by_path.get(&(path.clone(), kind)) {
            return *k;
        }
        let key = {
            let slot = self.slots.len() as u32;
            self.slots.push(Slot { gen: 0, entry: None });
            AssetKey::new(slot, 0, kind)
        };
        let entry = Entry {
            key,
            path: path.clone(),
            version: 0,
            refs: 0,
            state: LoadState::NotLoaded,
            stamp: None,
            deps: Vec::new(),
        };
        self.slots[key.slot() as usize].entry = Some(entry);
        self.by_path.insert((path, kind), key);
        key
    }

    /// 便捷注册（字符串路径）。
    pub fn register_str(&mut self, path: &str, kind: AssetKind) -> Result<AssetKey, AssetPathError> {
        let p = AssetPath::new(path)?;
        Ok(self.register(p, kind))
    }

    /// 反查已注册的键。
    pub fn key_of(&self, path: &AssetPath, kind: AssetKind) -> Option<AssetKey> {
        self.by_path.get(&(path.clone(), kind)).copied()
    }

    /// 键是否有效（存在且代际/分类匹配）。
    pub fn contains(&self, key: AssetKey) -> bool {
        self.index_of(key).is_some()
    }

    /// 存活资源数（不含保留槽位 0）。
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.entry.is_some()).count()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 全部键，**槽位升序**（确定性，可依赖）。
    pub fn keys(&self) -> Vec<AssetKey> {
        self.slots
            .iter()
            .filter_map(|s| s.entry.as_ref().map(|e| e.key))
            .collect()
    }

    /// 只读遍历，槽位升序。
    pub fn iter(&self) -> RegistryIter<'_> {
        RegistryIter {
            inner: self.slots.iter().enumerate(),
        }
    }

    /// 资源路径。
    pub fn path_of(&self, key: AssetKey) -> Option<&AssetPath> {
        self.entry(key).map(|e| &e.path)
    }

    /// 资源版本。
    pub fn version_of(&self, key: AssetKey) -> Option<u32> {
        self.entry(key).map(|e| e.version)
    }

    /// 引用计数。
    pub fn refs(&self, key: AssetKey) -> Option<u32> {
        self.entry(key).map(|e| e.refs)
    }

    /// 加载状态。
    pub fn state_of(&self, key: AssetKey) -> Option<&LoadState> {
        self.entry(key).map(|e| &e.state)
    }

    /// 已加载数据（若就绪）。
    pub fn loaded(&self, key: AssetKey) -> Option<Arc<LoadedAsset>> {
        self.entry(key).and_then(|e| e.state.loaded().cloned())
    }

    /// 一条记录。
    pub fn entry(&self, key: AssetKey) -> Option<&Entry> {
        self.index_of(key).and_then(|i| self.slots[i].entry.as_ref())
    }

    /// 处理某键的**另一份 key**：仅当 `(slot, gen)` 一致而 `kind` 不同时返回 `false`。
    ///
    /// 这是"分类属于身份"这条纪律的守卫：拿 Texture 键去查 Audio 资源必须落空。
    fn index_of(&self, key: AssetKey) -> Option<usize> {
        let idx = key.slot() as usize;
        let slot = self.slots.get(idx)?;
        let entry = slot.entry.as_ref()?;
        if slot.gen != key.generation() || entry.key.kind() != key.kind() {
            return None;
        }
        Some(idx)
    }

    fn entry_mut(&mut self, key: AssetKey) -> Option<&mut Entry> {
        let idx = self.index_of(key)?;
        self.slots[idx].entry.as_mut()
    }

    // ------------------------------------------------------------ 引用计数

    /// 增加引用。返回新计数。已被排队卸载的资源会**撤出**卸载队列。
    pub fn acquire(&mut self, key: AssetKey) -> Option<u32> {
        let refs = {
            let e = self.entry_mut(key)?;
            e.refs = e.refs.saturating_add(1);
            e.refs
        };
        self.pending_unload.retain(|k| *k != key);
        Some(refs)
    }

    /// 减少引用。返回新计数。
    ///
    /// 计数归零**不等于**立刻卸载：若此刻没有存活依赖者，键会进入卸载队列，
    /// 等 [`Self::unload_tick`]（帧末）统一处理。这是"不做无引用即卸"的落点。
    pub fn release(&mut self, key: AssetKey) -> Option<u32> {
        let refs = {
            let e = self.entry_mut(key)?;
            e.refs = e.refs.saturating_sub(1);
            e.refs
        };
        if refs == 0 {
            self.enqueue_reclaimable_closure(key);
        }
        Some(refs)
    }

    /// 从 `root` 出发，把自身及其依赖子树中**此刻可回收**的键放入卸载队列。
    ///
    /// 为什么是闭包而不是单个键：一个场景被释放时，它独占的纹理/音频也应该
    /// 在**同一次帧末沉降**里一起走，否则要等下一轮才轮到子资源，帧间内存曲线会拖尾。
    /// 显式栈 + 去重集合，依赖图成环也不会失控（成环本已被 `add_dependency` 拒绝）。
    fn enqueue_reclaimable_closure(&mut self, root: AssetKey) {
        let mut stack = vec![root];
        let mut seen: Vec<AssetKey> = Vec::new();
        while let Some(key) = stack.pop() {
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            if self.is_reclaimable(key) && !self.pending_unload.contains(&key) {
                self.pending_unload.push(key);
            }
            for dep in self.dependencies(key) {
                stack.push(dep);
            }
        }
        self.pending_unload.sort();
    }

    // ---------------------------------------------------------------- 加载

    /// 同步加载到就绪。已就绪则直接返回现有数据（不重复读盘）。
    ///
    /// 状态链：`NotLoaded → Queued → Loading → Ready|Failed`。
    /// 成功一次版本 +1；失败则记录原因并保留可重试状态。
    pub fn load(&mut self, key: AssetKey) -> Result<Arc<LoadedAsset>, LoadError> {
        let idx = self.index_of(key).ok_or(LoadError::UnknownKey(key))?;
        let (path, already) = {
            let e = self.slots[idx].entry.as_ref().expect("槽位有键即有记录");
            (e.path.clone(), e.state.loaded().cloned())
        };
        if let Some(a) = already {
            return Ok(a);
        }
        self.drive_to_loading(idx, &path)?;
        match self.loader.read(&path) {
            Ok((bytes, stamp)) => Ok(self.commit_ready(idx, key, path, bytes, stamp, false)),
            Err(e) => {
                self.commit_failed(idx, e.to_string());
                Err(e)
            }
        }
    }

    /// 失败后重试（`Failed → Loading`）。
    pub fn retry(&mut self, key: AssetKey) -> Result<Arc<LoadedAsset>, LoadError> {
        match self.state_of(key).map(|s| s.tag()) {
            Some(StateTag::Failed) | Some(StateTag::NotLoaded) => self.load(key),
            Some(_) => self.load(key),
            None => Err(LoadError::UnknownKey(key)),
        }
    }

    fn drive_to_loading(&mut self, idx: usize, path: &AssetPath) -> Result<(), LoadError> {
        let tag = self.slots[idx]
            .entry
            .as_ref()
            .expect("槽位有键即有记录")
            .state
            .tag();
        if tag == StateTag::NotLoaded {
            self.transition(idx, StateTag::Queued, path)?;
        }
        if tag != StateTag::Loading {
            self.transition(idx, StateTag::Loading, path)?;
        }
        Ok(())
    }

    fn transition(&mut self, idx: usize, next: StateTag, path: &AssetPath) -> Result<(), LoadError> {
        let e = self.slots[idx].entry.as_mut().expect("槽位有键即有记录");
        if let Err(err) = e.state.validate(next) {
            return Err(LoadError::State(path.clone(), Arc::from(err.to_string())));
        }
        e.state = match next {
            StateTag::NotLoaded => LoadState::NotLoaded,
            StateTag::Queued => LoadState::Queued,
            StateTag::Loading => LoadState::Loading,
            // Ready/Failed 必须带数据，走 commit_* 专门入口。
            StateTag::Ready | StateTag::Failed => {
                return Err(LoadError::State(
                    path.clone(),
                    Arc::from("Ready/Failed 必须经 commit_* 提交"),
                ))
            }
        };
        Ok(())
    }

    /// 提交就绪状态。`reload == true` 时广播 `Reloaded`，否则广播 `Loaded`。
    fn commit_ready(
        &mut self,
        idx: usize,
        key: AssetKey,
        path: AssetPath,
        bytes: Arc<[u8]>,
        stamp: Stamp,
        reload: bool,
    ) -> Arc<LoadedAsset> {
        let version = {
            let e = self.slots[idx].entry.as_mut().expect("槽位有键即有记录");
            e.version = e.version.wrapping_add(1).max(1);
            e.stamp = Some(stamp);
            e.version
        };
        let asset = Arc::new(LoadedAsset::new(key, path, version, bytes, stamp));
        {
            let e = self.slots[idx].entry.as_mut().expect("槽位有键即有记录");
            e.state = LoadState::Ready(Arc::clone(&asset));
        }
        self.events.push(if reload {
            AssetEvent::Reloaded { key, version }
        } else {
            AssetEvent::Loaded { key, version }
        });
        asset
    }

    fn commit_failed(&mut self, idx: usize, reason: String) {
        let (key, version) = {
            let e = self.slots[idx].entry.as_mut().expect("槽位有键即有记录");
            e.state = LoadState::Failed(Arc::from(reason.as_str()));
            (e.key, e.version)
        };
        self.events.push(AssetEvent::Failed {
            key,
            version,
            reason,
        });
    }

    // -------------------------------------------------------------- 热重载

    /// 轮询全部已就绪资源，源发生变化即**原地重载同一个 key** 并广播新版本。
    ///
    /// 这是 M3 出口准则 1 的实现：`key` 稳定、`version` 递增、订阅者收到通知。
    pub fn poll_reloads(&mut self) -> ReloadReport {
        let mut report = ReloadReport::default();
        let mut candidates: Vec<(usize, AssetKey, AssetPath, Stamp)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let e = s.entry.as_ref()?;
                let stamp = e.stamp?;
                if e.state.is_ready() {
                    Some((i, e.key, e.path.clone(), stamp))
                } else {
                    None
                }
            })
            .collect();
        candidates.sort_by_key(|(_, k, _, _)| k.slot());

        for (idx, key, path, old) in candidates {
            report.checked += 1;
            let new_stamp = match self.loader.stamp(&path) {
                Ok(s) => s,
                Err(e) => {
                    let reason: Arc<str> = Arc::from(e.to_string());
                    self.commit_failed(idx, reason.to_string());
                    report.failed.push((key, reason));
                    continue;
                }
            };
            // 判定依据是**内容**（len + hash），不是 mtime：编辑器"保存但内容没变"
            // 不该触发一次重载。revision 只用于诊断展示，变了就地记下。
            if new_stamp.len == old.len && new_stamp.hash == old.hash {
                if new_stamp.revision != old.revision {
                    if let Some(e) = self.slots[idx].entry.as_mut() {
                        e.stamp = Some(new_stamp);
                    }
                }
                continue;
            }
            // 内容真变了才读全量。
            self.drive_to_loading_for_reload(idx, &path);
            match self.loader.read(&path) {
                Ok((bytes, stamp)) => {
                    let v = self.commit_ready(idx, key, path.clone(), bytes, stamp, true);
                    report.reloaded.push((key, v.version));
                }
                Err(e) => {
                    let reason: Arc<str> = Arc::from(e.to_string());
                    self.commit_failed(idx, reason.to_string());
                    report.failed.push((key, reason));
                }
            }
        }
        report
    }

    fn drive_to_loading_for_reload(&mut self, idx: usize, path: &AssetPath) {
        let e = self.slots[idx].entry.as_mut().expect("槽位有键即有记录");
        // Ready → Loading 是热重载的合法通道（见 state.rs 的转移表）。
        if e.state.validate(StateTag::Loading).is_ok() {
            e.state = LoadState::Loading;
        } else {
            self.commit_failed(idx, format!("热重载状态转移非法：{}", StateTag::Loading.as_str()));
            let _ = path;
        }
    }

    // ---------------------------------------------------------------- 依赖

    /// 声明 `user` 依赖 `dep`（例如：场景 → 纹理）。
    ///
    /// 拒绝自依赖与成环：依赖图一旦成环，卸载的可达性判定就失去意义。
    pub fn add_dependency(&mut self, user: AssetKey, dep: AssetKey) -> Result<(), DepError> {
        if user == dep {
            return Err(DepError::SelfDependency(user));
        }
        if !self.contains(user) {
            return Err(DepError::Unknown(user));
        }
        if !self.contains(dep) {
            return Err(DepError::Unknown(dep));
        }
        if self.reaches(dep, user) {
            return Err(DepError::Cycle(user, dep));
        }
        let e = self.entry_mut(user).expect("已校验存在");
        if !e.deps.contains(&dep) {
            e.deps.push(dep);
            e.deps.sort();
        }
        Ok(())
    }

    /// 解除依赖。返回是否真的删除了。
    pub fn remove_dependency(&mut self, user: AssetKey, dep: AssetKey) -> bool {
        match self.entry_mut(user) {
            Some(e) => {
                let before = e.deps.len();
                e.deps.retain(|k| *k != dep);
                before != e.deps.len()
            }
            None => false,
        }
    }

    /// `key` 直接依赖的资源（升序）。
    pub fn dependencies(&self, key: AssetKey) -> Vec<AssetKey> {
        self.entry(key).map(|e| e.deps.clone()).unwrap_or_default()
    }

    /// 直接依赖 `key` 的资源（升序）。由 `deps` 扫描得出，不维护镜像集合。
    pub fn dependents(&self, key: AssetKey) -> Vec<AssetKey> {
        let mut out: Vec<AssetKey> = self
            .slots
            .iter()
            .filter_map(|s| s.entry.as_ref())
            .filter(|e| e.deps.contains(&key))
            .map(|e| e.key)
            .collect();
        out.sort();
        out
    }

    /// `from` 沿依赖边能否到达 `target`（显式栈 DFS，防环失控）。
    fn reaches(&self, from: AssetKey, target: AssetKey) -> bool {
        if from == target {
            return true;
        }
        let mut stack = vec![from];
        let mut seen: Vec<AssetKey> = vec![from];
        while let Some(cur) = stack.pop() {
            for next in self.dependencies(cur) {
                if next == target {
                    return true;
                }
                if !seen.contains(&next) {
                    seen.push(next);
                    stack.push(next);
                }
            }
        }
        false
    }

    // ---------------------------------------------------------------- 卸载

    /// 是否可回收：引用计数归零，且**没有任何仍被引用的资源依赖它**。
    ///
    /// 判据只用 `refs > 0` 作为"存活"定义，不做 Ready 也算存活的额外判断——
    /// 否则一个已就绪但无人引用的资源会永久钉住它的整条依赖子树。
    pub fn is_reclaimable(&self, key: AssetKey) -> bool {
        match self.entry(key) {
            Some(e) => {
                e.refs == 0
                    && self
                        .dependents(key)
                        .iter()
                        .all(|d| self.refs(*d).unwrap_or(0) == 0)
            }
            None => false,
        }
    }

    /// 当前排队待卸载的键（只读，槽位升序）。
    pub fn pending_unload(&self) -> &[AssetKey] {
        &self.pending_unload
    }

    /// 执行一次卸载（帧末调用）。返回真正被卸载的键。
    ///
    /// 队列中的键会被**重新复核**：期间若又被 `acquire`，或出现了存活依赖者，
    /// 则本轮不卸载（并撤出队列）。
    pub fn unload_tick(&mut self) -> Vec<AssetKey> {
        let queue = std::mem::take(&mut self.pending_unload);
        let mut unloaded = Vec::new();
        for key in queue {
            if !self.is_reclaimable(key) {
                continue;
            }
            let idx = match self.index_of(key) {
                Some(i) => i,
                None => continue,
            };
            let was_loaded = {
                let e = self.slots[idx].entry.as_mut().expect("槽位有键即有记录");
                let was = !matches!(e.state, LoadState::NotLoaded);
                e.state = LoadState::NotLoaded;
                e.stamp = None;
                e.version = 0;
                was
            };
            if was_loaded {
                self.events.push(AssetEvent::Unloaded { key });
                unloaded.push(key);
            }
        }
        unloaded
    }

    // ---------------------------------------------------------------- 订阅

    /// 订阅事件。返回订阅者身份，供 [`Self::unsubscribe`]。
    pub fn subscribe(&mut self, scope: EventScope) -> SubscriberId {
        let id = SubscriberId::new(self.next_sub);
        self.next_sub += 1;
        self.subs.push(Subscription { id, scope });
        id
    }

    /// 退订。
    pub fn unsubscribe(&mut self, id: SubscriberId) -> bool {
        let before = self.subs.len();
        self.subs.retain(|s| s.id != id);
        before != self.subs.len()
    }

    /// 订阅者数量。
    pub fn subscriber_count(&self) -> usize {
        self.subs.len()
    }

    /// 取出全部待派发事件（清空队列）。
    pub fn drain_events(&mut self) -> Vec<AssetEvent> {
        std::mem::take(&mut self.events)
    }

    /// 待派发事件数（不清空）。
    pub fn pending_events(&self) -> usize {
        self.events.len()
    }

    /// 帧末派发：把已入队事件按订阅范围投递给订阅者，**并清空队列**。
    ///
    /// 返回顺序是 `(事件顺序, 订阅者注册顺序)` 的双重稳定序。
    pub fn dispatch(&mut self) -> Vec<Delivery> {
        let events = std::mem::take(&mut self.events);
        let mut out = Vec::new();
        for ev in &events {
            for sub in &self.subs {
                if sub.scope.matches(ev) {
                    out.push(Delivery {
                        subscriber: sub.id,
                        event: ev.clone(),
                    });
                }
            }
        }
        out
    }
}

/// 只读遍历器（槽位升序）。
pub struct RegistryIter<'a> {
    inner: std::iter::Enumerate<std::slice::Iter<'a, Slot>>,
}

impl<'a> Iterator for RegistryIter<'a> {
    type Item = (AssetKey, &'a Entry);

    fn next(&mut self) -> Option<Self::Item> {
        for (_, slot) in self.inner.by_ref() {
            if let Some(e) = slot.entry.as_ref() {
                return Some((e.key, e));
            }
        }
        None
    }
}

impl fmt::Debug for AssetRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetRegistry")
            .field("assets", &self.len())
            .field("subscribers", &self.subs.len())
            .field("pending_events", &self.events.len())
            .field("pending_unload", &self.pending_unload.len())
            .finish()
    }
}
