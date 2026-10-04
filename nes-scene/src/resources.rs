//! 场景资源表（M3）：把节点属性里的 [`Value::Resource`] 从"占位数字"接到
//! [`nes_asset`] 的全局注册表。
//!
//! # 为什么属性里存的不是 `AssetKey`
//!
//! M2 的属性值是 `Value::Resource(u64)`。若这个 `u64` 直接等于 [`AssetKey`]，
//! 场景文件就写进了一个**进程内 arena 槽位 + 代际**：重启后槽位分配可能不同，
//! 同一份场景在不同会话里指向不同资源，进版本控制更是毫无意义。
//!
//! M3 的解法是把"引用"拆成两层：
//!
//! - 属性里存的是**场景内资源槽位** [`ResId`]（`u32`，只在本场景内有意义，
//!   随场景文件一起保存，因此稳定、可 diff）；
//! - [`ResourceTable`] 把 `ResId` 解析为 `(路径, 类别)`，再由 [`AssetRegistry`]
//!   给出运行时身份 [`AssetKey`]。
//!
//! `ResId(0)` 保留为"未绑定"，与 M2 里 `texture = Resource(0)` 的默认值语义一致 ——
//! 因此 M2 的存量场景文件不需要任何迁移就能读进来（表里表现为悬垂项）。
//!
//! # 三层身份的职责
//!
//! | 身份 | 生命周期 | 谁在用 |
//! |------|----------|--------|
//! | [`ResId`] | 场景文件内稳定 | 场景属性、编辑器面板 |
//! | [`AssetPath`] + [`AssetKind`] | 项目内稳定 | 场景文件、包管理、资源浏览器 |
//! | [`AssetKey`] | 进程内稳定（热重载不变） | 注册表、依赖图、渲染/音频后端 |
//!
//! 渲染侧只认 [`RenderAssetKeyView`]，由 [`ResEntry::render_key`] 从 `AssetKey`
//! 导出（`AssetKind::is_render_facing` 决定哪些类别有渲染视图），这就是"上层统一"：
//! 场景层与渲染层看同一个身份的两个投影，不需要各自维护一套编号。
//!
//! # 与现实文件的关系
//!
//! 表只负责"声明 → 注册 → 加载 → 持有 → 热重载广播"。解引用（把就绪字节
//! 变成 GPU 纹理 / 音频流）是 M4 的事；本模块保证的是：任何时刻都能回答
//! "这个 `ResId` 现在是什么状态、版本几、能不能画"。

use std::collections::BTreeMap;
use std::fmt;

use nes_asset::{
    AssetKey, AssetKind, AssetPath, AssetPathError, AssetRegistry, RenderAssetKeyView, ReloadReport,
    StateTag,
};

use crate::schema::EditorHint;
use crate::tree::SceneTree;
use crate::value::Value;

// ---------- 场景内资源槽位 ----------

/// 场景内资源槽位。`0` 保留为"未绑定"。
///
/// 只保证**同一场景文件内**稳定：它随场景一起序列化，所以换台机器、
/// 换个会话读同一个场景，同一个 `ResId` 仍指向同一份资源。
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ResId(u32);

impl ResId {
    /// 未绑定。属性写 `Resource(0)` 即等于"这个槽位是空的"。
    pub const UNBOUND: Self = Self(0);

    /// 从裸值构造（不校验，`0` 即未绑定）。
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// 裸值。
    pub const fn get(self) -> u32 {
        self.0
    }

    /// 是否为"未绑定"。
    pub const fn is_unbound(self) -> bool {
        self.0 == 0
    }

    /// 转成属性值。
    pub const fn to_value(self) -> Value {
        Value::Resource(self.0 as u64)
    }

    /// 从裸 `u64` 还原（`0` 与超出 `u32` 的引用都视为无效引用）。
    pub fn from_raw(raw: u64) -> Option<Self> {
        if raw == 0 || raw > u32::MAX as u64 {
            None
        } else {
            Some(Self(raw as u32))
        }
    }

    /// 从属性值还原。非 `Resource` 或未绑定都返回 `None`。
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Resource(raw) => Self::from_raw(*raw),
            _ => None,
        }
    }
}

impl fmt::Display for ResId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "res#{}", self.0)
    }
}

// ---------- 错误 ----------

/// 资源表操作错误。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TableError {
    /// 路径不合法（见 [`AssetPath`] 的规则）。
    Path(AssetPathError),
    /// 槽位 `0` 是保留值，不能被声明。
    ReservedId,
    /// 同一槽位被两条不同的声明占用（场景文件自相矛盾）。
    Conflict {
        /// 冲突的槽位。
        id: ResId,
        /// 先来的声明。
        existing: String,
        /// 后来的声明。
        incoming: String,
    },
    /// 依赖指向了一个不存在的槽位。
    UnknownDep(ResId),
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(e) => write!(f, "非法资源路径：{e}"),
            Self::ReservedId => write!(f, "槽位 0 是保留的\"未绑定\"标记，不能被声明"),
            Self::Conflict {
                id,
                existing,
                incoming,
            } => write!(f, "槽位 {id} 已被 {existing} 占用，无法再声明为 {incoming}"),
            Self::UnknownDep(dep) => write!(f, "依赖了未声明的槽位 {dep}"),
        }
    }
}

impl std::error::Error for TableError {}

// ---------- 表项 ----------

/// 表里的一条资源记录。
#[derive(Clone, Debug)]
pub struct ResEntry {
    id: ResId,
    path: Option<AssetPath>,
    kind: Option<AssetKind>,
    key: Option<AssetKey>,
    state: StateTag,
    version: u32,
    held: bool,
    deps: Vec<ResId>,
}

impl ResEntry {
    fn declared(id: ResId, path: AssetPath, kind: AssetKind) -> Self {
        Self {
            id,
            path: Some(path),
            kind: Some(kind),
            key: None,
            state: StateTag::NotLoaded,
            version: 0,
            held: false,
            deps: Vec::new(),
        }
    }

    /// 悬垂项：场景引用了这个槽位，但没人声明过它指向哪个文件。
    fn dangling(id: ResId) -> Self {
        Self {
            id,
            path: None,
            kind: None,
            key: None,
            state: StateTag::NotLoaded,
            version: 0,
            held: false,
            deps: Vec::new(),
        }
    }

    /// 槽位号。
    pub fn id(&self) -> ResId {
        self.id
    }

    /// 源路径（悬垂项为 `None`）。
    pub fn path(&self) -> Option<&AssetPath> {
        self.path.as_ref()
    }

    /// 资源类别（悬垂项为 `None`）。
    pub fn kind(&self) -> Option<AssetKind> {
        self.kind
    }

    /// 运行时身份。绑定过注册表才有值，且**热重载后不变**。
    pub fn key(&self) -> Option<AssetKey> {
        self.key
    }

    /// 加载状态。
    pub fn state(&self) -> StateTag {
        self.state
    }

    /// 版本。就绪为 1 起，每次成功重载 +1，卸载归 0。
    pub fn version(&self) -> u32 {
        self.version
    }

    /// 场景是否持有它（持有即引用计数 ≥ 1，不会被回收）。
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// 是否已接到注册表。
    pub fn is_bound(&self) -> bool {
        self.key.is_some()
    }

    /// 是否为悬垂项（被引用但未声明）。
    pub fn is_dangling(&self) -> bool {
        self.path.is_none()
    }

    /// 就绪且可画/可播。
    pub fn is_ready(&self) -> bool {
        self.state == StateTag::Ready
    }

    /// 依赖的槽位（升序）。
    pub fn deps(&self) -> &[ResId] {
        &self.deps
    }

    /// 渲染侧视图。只有渲染相关类别（纹理/着色器/字体/场景/脚本）才有。
    pub fn render_key(&self) -> Option<RenderAssetKeyView> {
        self.key.and_then(AssetKey::as_render_key)
    }
}

/// 跨层统一视图：把一条引用的三层身份与状态打包成可跨帧持有的快照。
///
/// 消费侧（渲染、音频、编辑器面板）拿这个结构，不需要同时持有表与注册表。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResourceView {
    /// 场景内槽位。
    pub id: ResId,
    /// 源路径。
    pub path: Option<AssetPath>,
    /// 资源类别。
    pub kind: Option<AssetKind>,
    /// 运行时身份。
    pub key: Option<AssetKey>,
    /// 渲染侧身份。
    pub render_key: Option<RenderAssetKeyView>,
    /// 加载状态。
    pub state: StateTag,
    /// 版本。
    pub version: u32,
    /// 是否被场景持有。
    pub held: bool,
}

impl ResourceView {
    /// 是否为悬垂引用。
    pub fn is_dangling(&self) -> bool {
        self.path.is_none()
    }

    /// 是否就绪。
    pub fn is_ready(&self) -> bool {
        self.state == StateTag::Ready
    }
}

// ---------- 副作用报告 ----------

/// [`ResourceTable::bind`] 的结果。
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct BindReport {
    /// 注册成功且内容已就绪的槽位（升序）。
    pub loaded: Vec<ResId>,
    /// 注册成功但内容加载失败：`(槽位, 原因)`。编辑器里显示为红条，不阻塞场景。
    pub failed: Vec<(ResId, String)>,
    /// 依赖图建立失败（成环、悬垂依赖）：`(槽位, 原因)`。
    pub dep_errors: Vec<(ResId, String)>,
    /// 注册表里为本次绑定新增的键数（重复绑定为 0）。
    pub registered: usize,
}

impl BindReport {
    /// 是否全部就绪且无依赖错误。
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty() && self.dep_errors.is_empty()
    }
}

/// [`ResourceTable::adopt_tree`] 的结果。
///
/// 这是**场景与表的一致性体检**：属性指向了没声明的槽位、或者把音频塞进纹理槽，
/// 都会在这里暴露，而不是等到渲染时才发现画不出来。
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct AdoptReport {
    /// 本次新登记的槽位（此前表里没有）。
    pub adopted: Vec<ResId>,
    /// 悬垂引用：`(槽位, 期望类别, 节点名, 属性名)`。
    pub undeclared: Vec<(ResId, AssetKind, String, String)>,
    /// 类别冲突：`(槽位, 期望类别, 实际类别, 节点名)`。
    pub mismatches: Vec<(ResId, AssetKind, AssetKind, String)>,
}

impl AdoptReport {
    /// 无悬垂引用、无类别冲突。
    pub fn is_clean(&self) -> bool {
        self.undeclared.is_empty() && self.mismatches.is_empty()
    }
}

// ---------- 资源表 ----------

/// 场景级资源表。
///
/// 用 `BTreeMap` 而不是 `Vec`：槽位号允许稀疏（M2 的存量场景里有 `Resource(7)`
/// 这种手写编号），且遍历顺序必须确定，否则序列化不稳定。
#[derive(Clone, Default, Debug)]
pub struct ResourceTable {
    entries: BTreeMap<u32, ResEntry>,
    next: u32,
    dirty: Vec<ResId>,
}

impl ResourceTable {
    /// 空表。
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next: 1,
            dirty: Vec::new(),
        }
    }

    /// 槽位数量。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 全部槽位（升序）。
    pub fn ids(&self) -> Vec<ResId> {
        self.entries.values().map(|e| e.id).collect()
    }

    /// 按槽位升序遍历。
    pub fn iter(&self) -> impl Iterator<Item = &ResEntry> + '_ {
        self.entries.values()
    }

    /// 查表项。
    pub fn entry(&self, id: ResId) -> Option<&ResEntry> {
        if id.is_unbound() {
            return None;
        }
        self.entries.get(&id.0)
    }

    /// 取跨层视图。
    pub fn view(&self, id: ResId) -> Option<ResourceView> {
        let e = self.entry(id)?;
        Some(ResourceView {
            id: e.id,
            path: e.path.clone(),
            kind: e.kind,
            key: e.key,
            render_key: e.render_key(),
            state: e.state,
            version: e.version,
            held: e.held,
        })
    }

    /// 运行时身份。
    pub fn key_of(&self, id: ResId) -> Option<AssetKey> {
        self.entry(id).and_then(|e| e.key)
    }

    /// 加载状态。
    pub fn state_of(&self, id: ResId) -> Option<StateTag> {
        self.entry(id).map(|e| e.state)
    }

    /// 版本。
    pub fn version_of(&self, id: ResId) -> Option<u32> {
        self.entry(id).map(|e| e.version)
    }

    /// 源路径。
    pub fn path_of(&self, id: ResId) -> Option<&AssetPath> {
        self.entry(id).and_then(|e| e.path.as_ref())
    }

    /// 反查：同一 `(路径, 类别)` 已经声明过的槽位。
    pub fn id_of_path(&self, path: &AssetPath, kind: AssetKind) -> Option<ResId> {
        self.entries
            .values()
            .find(|e| e.kind == Some(kind) && e.path.as_ref() == Some(path))
            .map(|e| e.id)
    }

    /// 便捷反查（字符串路径）。
    pub fn find(&self, path: &str, kind: AssetKind) -> Option<ResId> {
        let p = AssetPath::new(path).ok()?;
        self.id_of_path(&p, kind)
    }

    /// 声明一条资源，自动分配槽位。同一 `(路径, 类别)` 重复声明是幂等的。
    pub fn declare(&mut self, path: &str, kind: AssetKind) -> Result<ResId, TableError> {
        let p = AssetPath::new(path).map_err(TableError::Path)?;
        Ok(self.declare_path(p, kind))
    }

    /// 声明一条资源（已校验路径）。
    pub fn declare_path(&mut self, path: AssetPath, kind: AssetKind) -> ResId {
        if let Some(id) = self.id_of_path(&path, kind) {
            return id;
        }
        let id = self.alloc_id();
        self.entries.insert(id.0, ResEntry::declared(id, path, kind));
        id
    }

    /// 在**指定槽位**上声明资源。场景文件回读走这条路：槽位号必须与属性里的
    /// 数字一致，不能重新分配。
    pub fn declare_at(&mut self, id: ResId, path: &str, kind: AssetKind) -> Result<ResId, TableError> {
        if id.is_unbound() {
            return Err(TableError::ReservedId);
        }
        let p = AssetPath::new(path).map_err(TableError::Path)?;
        let incoming = format!("`{p}`（{}）", kind.as_str());
        match self.entries.get(&id.0) {
            Some(e) if e.path.as_ref() == Some(&p) && e.kind == Some(kind) => Ok(id),
            Some(e) => Err(TableError::Conflict {
                id,
                existing: describe(e),
                incoming,
            }),
            None => {
                self.entries.insert(id.0, ResEntry::declared(id, p, kind));
                if id.0 >= self.next {
                    self.next = id.0.saturating_add(1);
                }
                Ok(id)
            }
        }
    }

    /// 登记悬垂槽位（被属性引用但未声明）。已存在则原样返回。
    pub fn ensure_dangling(&mut self, id: ResId) -> Option<ResId> {
        if id.is_unbound() {
            return None;
        }
        if self.entries.contains_key(&id.0) {
            return Some(id);
        }
        self.entries.insert(id.0, ResEntry::dangling(id));
        if id.0 >= self.next {
            self.next = id.0.saturating_add(1);
        }
        Some(id)
    }

    /// 声明依赖（`user` 用到的资源必须先被声明）。
    pub fn set_deps(&mut self, user: ResId, deps: Vec<ResId>) -> Result<(), TableError> {
        for d in &deps {
            if self.entry(*d).is_none() {
                return Err(TableError::UnknownDep(*d));
            }
        }
        let e = self.entries.get_mut(&user.0).ok_or(TableError::UnknownDep(user))?;
        let mut sorted = deps;
        sorted.sort_unstable();
        sorted.dedup();
        e.deps = sorted;
        Ok(())
    }

    /// 依赖列表。
    pub fn deps_of(&self, id: ResId) -> &[ResId] {
        self.entry(id).map(|e| e.deps.as_slice()).unwrap_or(&[])
    }

    /// 扫描一棵树，把属性里出现的每个资源引用登记进表。
    ///
    /// 已经声明过的槽位保持不动（声明是权威），只补悬垂项，并做类别体检。
    pub fn adopt_tree(&mut self, tree: &SceneTree) -> AdoptReport {
        let mut report = AdoptReport::default();
        for node in tree.preorder() {
            let Some(store) = tree.props(node) else {
                continue;
            };
            let node_name = tree.name(node).unwrap_or("?").to_string();
            for (prop, value) in store.iter() {
                let Some(id) = ResId::from_value(value) else {
                    continue;
                };
                // 可接受类别全集（S15）：单类别提示退化为单元素表；
                // ResourceMany（Sprite2D.texture = texture|video）解析出表内
                // 全部已知类别。解析不出任何已知类别 = 该属性没有体检口径
                //（与认不出的单类别提示同一口径 —— 跳过而不是误报）。
                let accepted: Vec<AssetKind> = tree
                    .schema_of(node)
                    .and_then(|s| s.prop(prop))
                    .map(|d| match d.hint() {
                        EditorHint::Resource { kind, .. } => {
                            asset_kind_of_hint(kind).into_iter().collect()
                        }
                        EditorHint::ResourceMany { kinds } => kinds
                            .iter()
                            .filter_map(|k| asset_kind_of_hint(k))
                            .collect(),
                        _ => Vec::new(),
                    })
                    .unwrap_or_default();
                // 报告口径类别 = 候选表首个（mismatch 信息里的"期望"）。
                let expected = accepted.first().copied();

                let is_new = !self.entries.contains_key(&id.0);
                if is_new {
                    self.ensure_dangling(id);
                    report.adopted.push(id);
                }

                let actual = self.entries.get(&id.0).and_then(|e| e.kind);
                if let Some(expect) = expected {
                    match actual {
                        None => report
                            .undeclared
                            .push((id, expect, node_name.clone(), prop.to_string())),
                        // S15：体检按**可接受全集**核对 —— 表内类别即合法
                        //（Sprite2D.texture 声明成 Video 不再误报 mismatch），
                        // 表外类别仍如实报（报告口径类别 = expected）。
                        Some(got) if !accepted.contains(&got) => {
                            report.mismatches.push((id, expect, got, node_name.clone()));
                        }
                        Some(_) => {}
                    }
                }
            }
        }
        report.adopted.sort_unstable();
        report.adopted.dedup();
        report
    }

    /// 把表里的声明接到注册表：注册 → 连依赖边 → 加载 → 场景持有。
    ///
    /// 全过程不 panic、不阻塞：加载失败的资源留在表里、状态为 `Failed`，
    /// 编辑器可以重试（`AssetRegistry::retry`）。重复调用幂等。
    pub fn bind(&mut self, reg: &mut AssetRegistry) -> BindReport {
        let mut report = BindReport::default();
        let ids: Vec<ResId> = self
            .entries
            .values()
            .filter(|e| e.path.is_some())
            .map(|e| e.id)
            .collect();

        // 1. 注册：先全部注册，再连边，避免依赖顺序影响结果。
        for id in &ids {
            let key = {
                let e = self.entries.get_mut(&id.0).expect("上一步已收集");
                if let Some(k) = e.key {
                    Some(k)
                } else {
                    let path = e.path.clone().expect("已过滤悬垂项");
                    let kind = e.kind.expect("声明的槽位必有类别");
                    Some(reg.register(path, kind))
                }
            };
            if let Some(k) = key {
                let e = self.entries.get_mut(&id.0).expect("上一步已收集");
                if e.key.is_none() {
                    e.key = Some(k);
                    report.registered += 1;
                }
            }
        }

        // 2. 依赖边。
        for id in &ids {
            let deps = self.entries.get(&id.0).map(|e| e.deps.clone()).unwrap_or_default();
            let Some(user_key) = self.key_of(*id) else {
                continue;
            };
            for dep in deps {
                match self.key_of(dep) {
                    Some(dep_key) => {
                        if let Err(e) = reg.add_dependency(user_key, dep_key) {
                            report.dep_errors.push((*id, e.to_string()));
                        }
                    }
                    None => report
                        .dep_errors
                        .push((*id, format!("依赖 {dep} 未声明或未绑定"))),
                }
            }
        }

        // 3. 加载 + 持有，并同步状态。
        for id in &ids {
            let Some(key) = self.key_of(*id) else {
                continue;
            };
            match reg.load(key) {
                Ok(_) => report.loaded.push(*id),
                Err(e) => report.failed.push((*id, e.to_string())),
            }
            if !self.entries.get(&id.0).map(|e| e.held).unwrap_or(false) {
                reg.acquire(key);
                if let Some(e) = self.entries.get_mut(&id.0) {
                    e.held = true;
                }
            }
            self.sync_entry(*id, reg);
        }

        report
    }

    /// 把注册表的改动收回来：热重载广播 → 表项版本 +1，并记下需要重建的槽位。
    ///
    /// 返回注册表的原始报告（谁被检查、谁重载、谁失败）。受影响槽位用
    /// [`ResourceTable::take_dirty`] 取。
    pub fn poll(&mut self, reg: &mut AssetRegistry) -> ReloadReport {
        let report = reg.poll_reloads();
        let mut dirty: Vec<ResId> = Vec::new();

        for (key, version) in &report.reloaded {
            for e in self.entries.values_mut() {
                if e.key == Some(*key) {
                    e.version = *version;
                    e.state = StateTag::Ready;
                    dirty.push(e.id);
                }
            }
        }
        for (key, _why) in &report.failed {
            for e in self.entries.values_mut() {
                if e.key == Some(*key) {
                    e.state = StateTag::Failed;
                }
            }
        }

        if !dirty.is_empty() {
            dirty.sort_unstable();
            dirty.dedup();
            self.dirty.extend(dirty);
            self.dirty.sort_unstable();
            self.dirty.dedup();
        }
        report
    }

    /// 取走"自上次取走以来发生过变化"的槽位（升序）。
    ///
    /// 消费侧每帧调一次，只重建真正变过的资源，而不是全场景重传。
    pub fn take_dirty(&mut self) -> Vec<ResId> {
        std::mem::take(&mut self.dirty)
    }

    /// 场景持有的引用是否还存在未处理的变更。
    pub fn has_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// 释放场景持有的全部引用（场景卸载 / 切关卡）。
    ///
    /// 释放不等于立刻卸载：引用计数归零后资源进入待回收队列，由
    /// [`AssetRegistry::unload_tick`] 统一卸载，避免"这一帧释放、下一帧又加载"。
    pub fn release_all(&mut self, reg: &mut AssetRegistry) -> Vec<AssetKey> {
        let mut released = Vec::new();
        for e in self.entries.values_mut() {
            if e.held {
                if let Some(key) = e.key {
                    if reg.release(key).is_some() {
                        released.push(key);
                    }
                }
                e.held = false;
            }
        }
        for e in self.entries.values_mut() {
            if !e.held && e.version > 0 {
                e.version = 0;
                e.state = StateTag::NotLoaded;
            }
        }
        released
    }

    /// 推进注册表的回收队列，并把被卸载的槽位状态同步回来。
    pub fn reclaim(&mut self, reg: &mut AssetRegistry) -> Vec<AssetKey> {
        let unloaded = reg.unload_tick();
        for key in &unloaded {
            for e in self.entries.values_mut() {
                if e.key == Some(*key) {
                    e.version = 0;
                    e.state = StateTag::NotLoaded;
                }
            }
        }
        unloaded
    }

    fn alloc_id(&mut self) -> ResId {
        while self.entries.contains_key(&self.next) {
            self.next = self.next.saturating_add(1);
        }
        let id = ResId(self.next);
        self.next = self.next.saturating_add(1);
        id
    }

    fn sync_entry(&mut self, id: ResId, reg: &AssetRegistry) {
        let Some(key) = self.key_of(id) else {
            return;
        };
        let (state, version) = match reg.entry(key) {
            Some(entry) => (entry.state.tag(), entry.version),
            None => return,
        };
        if let Some(e) = self.entries.get_mut(&id.0) {
            e.state = state;
            e.version = version;
        }
    }
}

fn describe(e: &ResEntry) -> String {
    match (&e.path, e.kind) {
        (Some(p), Some(k)) => format!("`{p}`（{}）", k.as_str()),
        (Some(p), None) => format!("`{p}`"),
        _ => "悬垂槽位".to_string(),
    }
}

/// 把 schema 的编辑器提示（`EditorHint::Resource { kind }`）映射到资源类别。
///
/// 提示串是人写的（`"texture"`、`"sfx"`、`"json"`……），因此这里做宽容匹配：
/// 先按稳定名精确匹配，再按常见别名与扩展名兜底。认不出来返回 `None`，
/// 调用方（[`ResourceTable::adopt_tree`]）就跳过类别体检而不是误报。
pub fn asset_kind_of_hint(hint: &str) -> Option<AssetKind> {
    if let Some(k) = AssetKind::from_str_exact(hint) {
        return Some(k);
    }
    let lower = hint.to_ascii_lowercase();
    if let Some(k) = AssetKind::ALL
        .into_iter()
        .find(|k| k.as_str().to_ascii_lowercase() == lower)
    {
        return Some(k);
    }
    match lower.as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "image" | "images" | "tex" | "textures" => {
            Some(AssetKind::Texture)
        }
        "ogg" | "wav" | "mp3" | "flac" | "sound" | "sounds" | "sfx" => Some(AssetKind::Audio),
        // S15：视频资源（稳定名 "Video" 走 from_str_exact；这里是别名/扩展名兜底）。
        "video" | "videos" | "movie" | "movies" | "amv" | "avi" => Some(AssetKind::Video),
        "ttf" | "otf" | "fonts" => Some(AssetKind::Font),
        "ron" | "scenes" | "prefab" | "level" => Some(AssetKind::Scene),
        "js" | "nes" | "scripts" => Some(AssetKind::Script),
        "wgsl" | "shaders" => Some(AssetKind::Shader),
        "json" | "csv" | "toml" | "config" | "data" => Some(AssetKind::Data),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn res_id_roundtrips_through_value() {
        assert_eq!(ResId::UNBOUND.to_value(), Value::Resource(0));
        assert!(ResId::from_value(&Value::Resource(0)).is_none());
        assert_eq!(ResId::from_value(&Value::Resource(7)), Some(ResId::new(7)));
        assert_eq!(ResId::from_raw(u64::from(u32::MAX) + 5), None);
        assert!(ResId::from_value(&Value::I64(7)).is_none());
    }

    #[test]
    fn declare_is_idempotent_and_paths_are_validated() {
        let mut table = ResourceTable::new();
        let a = table.declare("Textures/hero.png", AssetKind::Texture).unwrap();
        let b = table.declare("Textures/hero.png", AssetKind::Texture).unwrap();
        assert_eq!(a, b, "同一路径重复声明必须复用槽位");
        // 类别不同则是另一份资源。
        let c = table.declare("Textures/hero.png", AssetKind::Data).unwrap();
        assert_ne!(a, c);
        assert_eq!(table.len(), 2);
        assert!(matches!(
            table.declare("../escape.png", AssetKind::Texture),
            Err(TableError::Path(_))
        ));
    }

    #[test]
    fn declare_at_rejects_reserved_and_conflicting_slots() {
        let mut table = ResourceTable::new();
        assert_eq!(
            table.declare_at(ResId::UNBOUND, "Textures/a.png", AssetKind::Texture),
            Err(TableError::ReservedId)
        );
        table.declare_at(ResId::new(7), "Textures/a.png", AssetKind::Texture).unwrap();
        // 同一槽位、同一路径、同一类别 → 幂等。
        table.declare_at(ResId::new(7), "Textures/a.png", AssetKind::Texture).unwrap();
        assert!(matches!(
            table.declare_at(ResId::new(7), "Textures/b.png", AssetKind::Texture),
            Err(TableError::Conflict { .. })
        ));
        // 稀疏槽位之后，新声明的槽位号不能撞车。
        let fresh = table.declare("Textures/c.png", AssetKind::Texture).unwrap();
        assert!(fresh.get() > 7);
    }

    #[test]
    fn set_deps_rejects_unknown_slots_and_sorts() {
        let mut table = ResourceTable::new();
        let a = table.declare("Textures/a.png", AssetKind::Texture).unwrap();
        let b = table.declare("Textures/b.png", AssetKind::Texture).unwrap();
        let user = table.declare("Scenes/l.ron", AssetKind::Scene).unwrap();
        table.set_deps(user, vec![b, a, b]).unwrap();
        assert_eq!(table.deps_of(user), &[a, b]);
        assert_eq!(
            table.set_deps(user, vec![ResId::new(99)]),
            Err(TableError::UnknownDep(ResId::new(99)))
        );
    }

    #[test]
    fn kind_hint_mapping_is_lenient_but_not_guessing() {
        assert_eq!(asset_kind_of_hint("texture"), Some(AssetKind::Texture));
        assert_eq!(asset_kind_of_hint("Texture"), Some(AssetKind::Texture));
        assert_eq!(asset_kind_of_hint("sfx"), Some(AssetKind::Audio));
        assert_eq!(asset_kind_of_hint("wgsl"), Some(AssetKind::Shader));
        assert_eq!(asset_kind_of_hint("wharrgarbl"), None);
    }
}
