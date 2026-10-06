//! 扩展运行时接线（S17 第 2 期）：[`ExtensionManager`] + 能力桥。
//!
//! # 架构口径
//!
//! JS 扩展跑在 **Extension Execution Runtime**（QuickJS，`nes-extension-js`）
//! 里，**不是 NES 的第二 Runtime**：扩展只能经注入的 `nes` 能力对象操作
//! 引擎，永远不直接碰 SceneTree/Renderer/WGPU。本模块是能力桥的引擎侧：
//!
//! ```text
//! SceneTree ──快照──▶ ExtCapsState（只读面）──▶ nes.scene.find / nes.node.getPos|getName
//!   ▲                                              nes.input.isPressed（输入快照）
//!   └──写队列── ExtCapsState.apply ◀── nes.node.setPos|setVisible / nes.audio.play
//! ```
//!
//! # 读快照 / 写队列（安全借用的确定性解法）
//!
//! QuickJS 闭包是 'static 的，没法安全持有 `&mut SceneTree`；本桥的口径：
//! * **读**（find / getPos / getName / isPressed）：每帧从 tick 后的树
//!   **快照**取答 —— 扩展看到的是当 tick 后状态，帧内自洽；
//! * **写**（setPos / setVisible）：进**操作队列**，update 阶段结束的同一
//!   帧内按提交序落地（`set_local` 只改平移分量，保留旋转/缩放/斜切；
//!   可见性走 `visible` 属性）—— 扩展写树 = 游戏状态（进指纹）。
//! * **音频**（play）：直通混音器（`Arc<Mutex>` 克隆，不借树）—— 音频
//!   不进语义状态（S13 裁决），键未注册静默丢弃。
//!
//! # 确定性论断（如实写明前提）
//!
//! headless 下 JS 执行是确定性的：同源码同输入 → 同字节码 → 同求值序
//! → 同写队列 → 同树状态 → 同指纹。**前提**：脚本不依赖宿主注入的
//! 不确定源 —— `Date` 与 `Math.random` 在 P0 **不禁用但文档警告**
//!（沙箱分区 / 内建裁剪是后续期次的事）；时间也是输入（固定 delta）。
//!
//! # 帧序契约
//!
//! `NesRuntime::update_extensions` 在 **simulate 之后**调用（窗口模式下
//! 即 `frame*` 返回后）—— 扩展看到的是当 tick 后状态；本帧写落地在
//! 渲染之后，下一帧呈现（一帧延迟，如实写明）。
//!
//! # 信号 hat（S17.2：事件重入）
//!
//! Scratch 语义的 hat = 事件处理器：事件发生 -> 重入扩展代码。载体是既有
//! 的确定性信号总线（S6 契约：FIFO 泵、级联、1024 上限、订阅过滤）：
//!
//! * **入**（游戏 -> 扩展）：`nes.onSignal(name, fn)` 注册 hat；帧路径
//!   [`crate::NesRuntime::tick_tree`] 发现有订阅时组装
//!   [`TeeObserver`](nes_scene::TeeObserver)(宿主观察者, [`ExtensionSignalObserver`])
//!   —— 泵把命中信号送进 [`SceneObserver::on_signal`]，管理器同步重入各
//!   扩展的 JS 蹦床（**裁决 A：in-tick 重入**，写树同帧落地，无滞后）。
//!   S17.1 三道防线全部继承：hat 里 throw / 死循环 / 内存超限 = 故障计数
//!   + 诊断，泵不炸、其余 handler 继续。
//! * **出**（扩展 -> 游戏）：`nes.emitSignal(name, payload)` 经
//!   `SignalCapability` 进待发射缓冲 —— 泵内派发期间取走 = **同泵级联**
//!   （`SignalCtx::emit` 语义，受 1024 上限约束）；update 期取走 =
//!   **下一帧的泵**（`emit_signal` 预发时序）。
//! * **QuickJS 重入实测结论**：泵 -> JS -> 泵 -> JS 的重入是**序贯**的
//!   （每次派发都是独立的一次 `rt.call`，发射经缓冲在两次派发之间取走，
//!   不存在嵌套 QuickJS 栈帧）—— T-HAT-02/03 以深度 3 级联实测钉住。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use nes_extension_api::{
    AudioCapability, ExtError, ExtensionLifecycle, InputCapability, JsRuntime, NesValue,
    NodeCapability, NodeRef, SceneCapability, SignalCapability,
};
use nes_extension_js::{CapabilityBinding, JsExtension, RquickjsRuntime};
use nes_render_extract::PROP_VISIBLE;
use nes_scene::{
    NodeId, SceneObserver, SceneTree, Signal, SignalCtx, SignalFilter, Transform2D, Value,
};

use crate::NesRuntime;

/// 扩展 hat 订阅名的容量上界（防订阅风暴；超限的声明如实忽略）。
const SIGNAL_SUBSCRIPTION_CAP: usize = 256;
/// 扩展待发射缓冲的容量上界（单次 50ms 预算内的发射风暴损失上界；
/// 超限丢弃并计数 —— 与信号泵 1024 上限同一条"如实截断"纪律）。
const PENDING_EMIT_CAP: usize = 4096;

/// 扩展侧能力状态：读快照 + 写队列（四个能力 traits 的引擎实现）。
///
/// 全部为**自有数据**（无引擎借用）—— 因此能以 `'static` trait object
/// 形态装进 QuickJS 闭包，这是"安全借用" puzzle 的解。
#[derive(Default)]
pub struct ExtCapsState {
    /// 名字 -> NodeRef 位形（前序第一个匹配，与 `SceneTree::find_by_name` 同口径）。
    names: BTreeMap<String, u64>,
    /// NodeRef 位形 -> (位置, 名字)（读面）。
    nodes: BTreeMap<u64, ((f32, f32), String)>,
    /// 本帧按住的键名（输入快照的投影）。
    held: Vec<String>,
    /// 混音器（`open_audio` 后存在；音频直通，不进写队列）。
    mixer: Option<Arc<Mutex<nes_audio::Mixer>>>,
    /// 写队列（提交序 = 落地序；apply 取走即清）。
    ops: Vec<ExtOp>,
    /// 信号 hat 订阅名（S17.2；声明序去重 —— 泵订阅过滤的装配面）。
    /// **跨帧留存**（hat 是注册态，不是快照态 —— 与上面四个读面字段不同家）。
    subscribed: Vec<String>,
    /// 扩展待发射信号（S17.2 `emitSignal` 的落点；提交序 = 落地序）。
    /// 取走点：泵内派发期间 = [`ExtensionSignalObserver::on_signal`]（同泵
    /// 级联）；update 期 = `update_extensions` 尾步（下帧泵）。
    pending_emits: Vec<(String, NesValue)>,
    /// 因发射缓冲超限被丢弃的载荷数（如实计数，诊断面）。
    dropped_emits: u64,
}

/// 一条扩展写操作。
#[derive(Clone, Debug, PartialEq)]
enum ExtOp {
    /// 写节点位置（只改平移分量）。
    SetPos(u64, f32, f32),
    /// 写节点可见性（`visible` 属性）。
    SetVisible(u64, bool),
}

impl ExtCapsState {
    /// 用 tick 后的引擎状态刷新读面 + 清残留写队列（帧首调用）。
    ///
    /// 正常路径队列在 `apply` 已清空；这里再清一次是防御——扩展 update
    /// 半途炸掉时，残留写不得跨帧泄漏（确定性口径：半途炸掉的帧不落地）。
    pub fn refresh(
        &mut self,
        tree: &SceneTree,
        held: &[String],
        mixer: Option<Arc<Mutex<nes_audio::Mixer>>>,
    ) {
        self.names.clear();
        self.nodes.clear();
        for id in tree.preorder() {
            let Some(name) = tree.name(id) else { continue };
            let bits = id.to_bits();
            self.names.entry(name.to_string()).or_insert(bits);
            let pos = tree
                .local(id)
                .map(|t| (t.pos.x, t.pos.y))
                .unwrap_or((0.0, 0.0));
            self.nodes.insert(bits, (pos, name.to_string()));
        }
        self.held = held.to_vec();
        self.mixer = mixer;
        self.ops.clear();
        // 待发射缓冲一并清空（防线一致性：扩展半途炸掉的帧，残留发射不得
        // 跨帧泄漏 —— 与写队列同一条纪律）。订阅名**不清**：hat 是注册态。
        self.pending_emits.clear();
    }

    /// 把写队列按提交序落进树（update 阶段结束后调用；音频已直通无需落地）。
    pub fn apply(&mut self, tree: &mut SceneTree) {
        let ops = std::mem::take(&mut self.ops);
        for op in ops {
            match op {
                ExtOp::SetPos(bits, x, y) => {
                    let id = NodeId::from_bits(bits);
                    // 死节点 set_local 内部是 no-op；只改平移分量（保留旋转/缩放/斜切）。
                    let mut t = tree.local(id).unwrap_or(Transform2D::IDENTITY);
                    t.pos = nes_scene::Vec2::new(x, y);
                    tree.set_local(id, t);
                }
                ExtOp::SetVisible(bits, v) => {
                    let id = NodeId::from_bits(bits);
                    // 与 `NodeCtx::set_prop` 同口径：写失败不炸帧。
                    let _ = tree.set_prop(id, PROP_VISIBLE, Value::Bool(v));
                }
            }
        }
    }

    /// 落地混音器播放（`play` 直通路径）。
    fn mixer_play(&self, key: &str, volume: f32) {
        let Some(mixer) = &self.mixer else { return };
        if let Ok(mut m) = mixer.lock() {
            // 键未注册/参数越界：静默丢弃（trait 契约 —— 音频不炸帧）。
            let _ = m.play(key, volume, false);
        }
    }

    /// 取走扩展待发射信号（提交序 = 落地序；取走即清）。
    pub fn take_pending_emits(&mut self) -> Vec<(String, NesValue)> {
        std::mem::take(&mut self.pending_emits)
    }

    /// 订阅名（声明序去重；宿主装配泵过滤器用）。
    pub fn subscriptions(&self) -> &[String] {
        &self.subscribed
    }

    /// 因发射缓冲超限被丢弃的载荷数（诊断面）。
    pub fn dropped_emits(&self) -> u64 {
        self.dropped_emits
    }
}

impl SceneCapability for ExtCapsState {
    fn find(&self, name: &str) -> Option<NodeRef> {
        self.names.get(name).map(|bits| NodeRef(*bits))
    }
}

impl NodeCapability for ExtCapsState {
    fn get_pos(&self, r: NodeRef) -> Option<(f32, f32)> {
        self.nodes.get(&r.0).map(|(pos, _)| *pos)
    }

    fn set_pos(&mut self, r: NodeRef, x: f32, y: f32) {
        self.ops.push(ExtOp::SetPos(r.0, x, y));
    }

    fn set_visible(&mut self, r: NodeRef, v: bool) {
        self.ops.push(ExtOp::SetVisible(r.0, v));
    }

    fn get_name(&self, r: NodeRef) -> Option<String> {
        self.nodes.get(&r.0).map(|(_, name)| name.clone())
    }
}

impl InputCapability for ExtCapsState {
    fn is_pressed(&self, name: &str) -> bool {
        self.held.iter().any(|k| k == name)
    }
}

impl AudioCapability for ExtCapsState {
    fn play(&self, key: &str, volume: f32) {
        self.mixer_play(key, volume);
    }
}

impl SignalCapability for ExtCapsState {
    fn on_signal(&mut self, name: &str) {
        // 声明序去重（同名多个 handler 只声明一次 —— JS 侧已保证，这里兜底）；
        // 容量上界防订阅风暴，超限如实忽略。
        if self.subscribed.iter().any(|n| n == name) {
            return;
        }
        if self.subscribed.len() >= SIGNAL_SUBSCRIPTION_CAP {
            return;
        }
        self.subscribed.push(name.to_string());
    }

    fn emit(&mut self, name: &str, payload: NesValue) {
        if self.pending_emits.len() >= PENDING_EMIT_CAP {
            // 发射风暴如实截断（与信号泵 1024 上限同一条纪律：不挂起、计数）。
            self.dropped_emits += 1;
            return;
        }
        self.pending_emits.push((name.to_string(), payload));
    }
}

/// 树 Value -> NesValue（S17.2 信号载荷出界；树 -> JS 方向）。
///
/// 映射：F32/I64 -> F64（JS 数值唯一形态）、Bool/Str 直映、Vec2 ->
/// `{x, y}` 对象、Resource 槽位号 -> 不透明 number、Node 句柄 ->
/// `Null`（**句柄不过界** —— 运行时身份不跨上下文，防悬垂与身份谎言）、
/// Array 递归。
pub fn value_to_nes(v: &Value) -> NesValue {
    match v {
        Value::F32(x) => NesValue::F64(*x as f64),
        Value::I64(i) => NesValue::F64(*i as f64),
        Value::Bool(b) => NesValue::Bool(*b),
        Value::Str(s) => NesValue::Str(s.clone()),
        Value::Vec2(p) => NesValue::obj([
            ("x", NesValue::F64(p.x as f64)),
            ("y", NesValue::F64(p.y as f64)),
        ]),
        Value::Resource(n) => NesValue::F64(*n as f64),
        Value::Node(_) => NesValue::Null,
        Value::Array(items) => NesValue::Array(items.iter().map(value_to_nes).collect()),
    }
}

/// NesValue -> 树 Value（S17.2 信号载荷入界；JS -> 树方向）。
///
/// 映射与脚本字面量类型对齐（脚本整数走 `I64`、小数走 `F32`）：
/// * `Null` -> `Bool(true)`（无载荷占位 —— 与树桥信号的占位约定同一口径，
///   游戏脚本本就产不出 null，映射对脚本面不可见）；
/// * F64 整数值（且在 i64 范围）-> `I64`，其余（含 NaN/Inf）-> `F32`
///   （f64 -> f32 精度收缩，如实文档）；
/// * Array 递归；`Object` 仅 `{x, y}` 数值对映射 `Vec2`，**其余对象 P0
///   拒绝**（返回 `None` —— 树 Value 没有可落地的对象形态，宁可丢弃并
///   计数，不编造语义）。
pub fn nes_to_value(v: &NesValue) -> Option<Value> {
    match v {
        NesValue::Null => Some(Value::Bool(true)),
        NesValue::Bool(b) => Some(Value::Bool(*b)),
        NesValue::F64(x) => {
            let integral = x.is_finite() && x.fract() == 0.0 && *x >= i64::MIN as f64;
            Some(if integral && *x <= i64::MAX as f64 {
                Value::I64(*x as i64)
            } else {
                Value::F32(*x as f32)
            })
        }
        NesValue::Str(s) => Some(Value::Str(s.clone())),
        NesValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(nes_to_value(item)?);
            }
            Some(Value::Array(out))
        }
        NesValue::Object(pairs) => {
            if pairs.len() == 2 {
                let mut point = (None, None);
                for (key, val) in pairs {
                    match (key.as_str(), val) {
                        ("x", NesValue::F64(v)) => point.0 = Some(*v as f32),
                        ("y", NesValue::F64(v)) => point.1 = Some(*v as f32),
                        _ => {
                            point = (None, None);
                            break;
                        }
                    }
                }
                if let (Some(x), Some(y)) = point {
                    return Some(Value::Vec2(nes_scene::Vec2::new(x, y)));
                }
            }
            None // P0 拒绝：非 {x, y} 数值对的对象不落树
        }
    }
}

/// 扩展管理器：QuickJS 运行时 + 已装载扩展 + 共享能力桥。
///
/// **不是** `NesRuntime` 的第二实例 —— 它只持 JS 侧（运行时/上下文/回调
/// 槽）与快照/队列；树的真身仍在 [`NesRuntime`]，经 `update_extensions`
/// 的字段级借用拆分桥接（见模块文档帧序契约）。
///
/// # 健壮性三道防线（S17.1）
///
/// 1. **异常隔离**（本模块）：扩展 update 抛错（JS 异常 / 预算中断 / 内存
///    超限，三者同形态）不炸帧 —— 计数 + 写诊断，其余扩展与引擎照常；
/// 2. **死循环中断**（`nes-extension-js` 的执行预算闸）：单次执行超
///    [`nes_extension_js::EXEC_BUDGET`] 即被 QuickJS 中断处理器转成 JS
///    异常，浮出形态与 1 相同；
/// 3. **自动停用**（本模块）：连续失败满 [`FAULT_DISABLE_THRESHOLD`] 帧
///    （60 帧 ≈ 60Hz 下 1 秒）不再调它的 update —— 失控扩展每帧只损耗
///    一个日志槽，健康扩展不受牵连。
pub struct ExtensionManager {
    runtime: Rc<RefCell<RquickjsRuntime>>,
    caps: Rc<RefCell<ExtCapsState>>,
    extensions: Vec<ManagedExt>,
    /// 故障累计计数（全部扩展、含已停用者；诊断面）。
    total_faults: u64,
    /// 最近一次故障（含扩展 id + JS 异常文本；诊断/Output 用）。
    last_fault: Option<String>,
    /// 不可落地信号载荷计数（P0 对象拒绝等；诊断面 —— 与发射缓冲超限
    /// 计数分家：前者在 [`ExtCapsState::dropped_emits`]）。
    dropped_payloads: u64,
}

/// 一个已装载扩展 + 宿主侧的健壮性簿记。
struct ManagedExt {
    ext: JsExtension,
    /// 连续失败计数（成功一帧即清零；满阈值停用）。
    consecutive_faults: u32,
    /// 停用后不再进帧（停用宣告只发一次）。
    disabled: bool,
}

/// 连续失败自动停用阈值（update 或 hat 派发连续失败次数；60 次 ≈ 60Hz
/// 下 1 秒 —— hat 高频信号下同一口径如实适用）。
const FAULT_DISABLE_THRESHOLD: u32 = 60;

impl ExtensionManager {
    /// 构造（QuickJS 运行时初始化失败即 Err）。
    pub fn new() -> Result<Self, ExtError> {
        Ok(Self {
            runtime: Rc::new(RefCell::new(RquickjsRuntime::new()?)),
            caps: Rc::new(RefCell::new(ExtCapsState::default())),
            extensions: Vec::new(),
            total_faults: 0,
            last_fault: None,
            dropped_payloads: 0,
        })
    }

    /// 装载一份扩展源码（独立上下文 + 能力注入 + 顶层求值 + 注册）。
    ///
    /// 注册口径：扩展在顶层调 `nes.registerExtension(id)` 自报身份；
    /// 未自报时用 `fallback_id`（文件名）。返回生效 id。顶层求值期间
    /// 扩展即可调 `nes.onSignal` 注册 hat（订阅名当场进能力桥，下一帧
    /// 泵即生效）。
    pub fn load_source(&mut self, source: &str, fallback_id: &str) -> Result<String, ExtError> {
        // 每个扩展一个上下文（互相隔离）；能力桥按扩展共享（同一引擎状态面）。
        let ctx = self.runtime.borrow_mut().create_context()?;
        let binding = CapabilityBinding::new(
            Rc::clone(&self.caps) as Rc<RefCell<dyn SceneCapability>>,
            Rc::clone(&self.caps) as Rc<RefCell<dyn NodeCapability>>,
            Rc::clone(&self.caps) as Rc<RefCell<dyn InputCapability>>,
            Rc::clone(&self.caps) as Rc<RefCell<dyn AudioCapability>>,
            Rc::clone(&self.caps) as Rc<RefCell<dyn SignalCapability>>,
        );
        binding.install(&mut self.runtime.borrow_mut(), ctx)?;
        {
            let mut rt = self.runtime.borrow_mut();
            rt.load_module(ctx, source)?;
            rt.collect(); // 顶层求值后的首轮回收
        }
        let mut ext = JsExtension::new(Rc::clone(&self.runtime), ctx);
        let id = ext
            .extension_id_from_js()
            .unwrap_or_else(|| fallback_id.to_string());
        ext.register(&id);
        if let Some(e) = ext.take_last_error() {
            return Err(ExtError::CallFailed(e));
        }
        self.extensions.push(ManagedExt { ext, consecutive_faults: 0, disabled: false });
        Ok(id)
    }

    /// 逐扩展调 update 钩子（防线 1 + 3）。
    ///
    /// JS 异常（含预算中断 / 内存超限 —— 三者同形态）**不炸帧**：错误信息
    /// （扩展 id + 异常文本）进返回清单与 [`Self::last_fault`]、故障计数
    /// +1，循环继续跑其余扩展；连续失败满 [`FAULT_DISABLE_THRESHOLD`] 次
    /// 自动停用该扩展（停用宣告只发一次，此后不再调它的 update）。
    pub fn update(&mut self) -> Vec<String> {
        let mut messages = Vec::new();
        for index in 0..self.extensions.len() {
            let failed = {
                let managed = &mut self.extensions[index];
                if managed.disabled {
                    continue; // 已停用：不进帧（宣告早已发过）。
                }
                managed.ext.update();
                match managed.ext.take_last_error() {
                    None => {
                        managed.consecutive_faults = 0; // 成功一帧即清零（"连续"口径）。
                        None
                    }
                    Some(err) => Some(err),
                }
            };
            if let Some(err) = failed {
                // 失败簿记（与 hat 派发共用同一套）。
                self.record_failure(index, &err, &mut messages);
            }
        }
        messages
    }

    /// 单次扩展调用失败的统一簿记（S17.1 防线 1/3 的引擎半边，update 与
    /// hat 派发共用）：累计 + 连续故障计数、最近故障（含扩展 id + 异常
    /// 文本）、诊断消息；连续失败满阈值自动停用（宣告只发一次）。
    fn record_failure(&mut self, index: usize, err: &str, messages: &mut Vec<String>) {
        // 扩展簿记收在一个块里（字段级拆借：extensions 与 last_fault 不相交）。
        let (id, reason, disable) = {
            self.total_faults += 1;
            let managed = &mut self.extensions[index];
            managed.consecutive_faults = managed.consecutive_faults.saturating_add(1);
            let interrupted = err.contains(nes_extension_js::INTERRUPTED_MARK);
            let reason = if interrupted {
                format!(
                    "执行超预算被中断（单次 {:?}）",
                    nes_extension_js::EXEC_BUDGET
                )
            } else {
                "JS 异常".to_string()
            };
            let disable = managed.consecutive_faults >= FAULT_DISABLE_THRESHOLD;
            if disable {
                managed.disabled = true; // 停用宣告只发一次（此后不进帧）。
            }
            (managed.ext.id().to_string(), reason, disable)
        };
        let fault = format!("[扩展 {id}] {err}（{reason}）");
        self.last_fault = Some(fault.clone());
        messages.push(fault);
        if disable {
            messages.push(format!(
                "[扩展 {id}] 连续 {} 次失败，已自动停用（其余扩展不受影响）",
                FAULT_DISABLE_THRESHOLD
            ));
        }
    }

    // ---------- 信号 hat（S17.2） ----------

    /// 是否有扩展订阅过信号 hat（帧路径零开销闸：无订阅不组装扩展观察者，
    /// tick 路径与 S17.1 基线逐位一致）。
    pub fn has_signal_hats(&self) -> bool {
        !self.caps.borrow().subscriptions().is_empty()
    }

    /// 订阅名快照（声明序去重；泵订阅过滤的装配面）。
    pub fn signal_subscriptions(&self) -> Vec<String> {
        self.caps.borrow().subscriptions().to_vec()
    }

    /// 向全部活扩展派发一条信号（hat 重入；`__nes_signal_dispatch` 蹦床）。
    ///
    /// 隔离继承（S17.1）：单扩展的 handler 异常 / 预算中断 / 内存超限 =
    /// 故障簿记 + 诊断（返回清单与 `last_fault`），**循环继续** —— 其余
    /// 扩展照常收到派发；连续失败满阈值自动停用（与 update 同一套簿记，
    /// 成功一次即清连续计数）。
    pub fn dispatch_signal(&mut self, name: &str, payload: NesValue) -> Vec<String> {
        let mut messages = Vec::new();
        for index in 0..self.extensions.len() {
            let outcome = {
                let managed = &mut self.extensions[index];
                if managed.disabled {
                    continue;
                }
                match managed.ext.dispatch_signal(name, &payload) {
                    Ok(_called) => {
                        managed.consecutive_faults = 0;
                        None
                    }
                    Err(e) => Some(e.to_string()),
                }
            };
            if let Some(err) = outcome {
                self.record_failure(index, &err, &mut messages);
            }
        }
        messages
    }

    /// 取走扩展待发射信号（提交序 = 落地序；泵内级联 / update 期落树共用）。
    pub fn take_pending_emits(&self) -> Vec<(String, NesValue)> {
        self.caps.borrow_mut().take_pending_emits()
    }

    /// 写队列落地（提交序；泵内 hat 写的当步落地与 update 期落地共用 ——
    /// 同一条提交序通道，落地函数唯一）。
    pub fn apply_writes(&self, tree: &mut SceneTree) {
        self.caps.borrow_mut().apply(tree);
    }

    /// 记一笔不可落地载荷（P0 对象拒绝；诊断面）。
    pub fn note_dropped_payload(&mut self) {
        self.dropped_payloads += 1;
    }

    /// 被丢弃的扩展信号载荷总数（发射缓冲超限 + 不可落地形态；诊断面）。
    pub fn dropped_signal_payloads(&self) -> u64 {
        self.dropped_payloads + self.caps.borrow().dropped_emits()
    }

    /// 已装载扩展数（含已停用者 —— 停用不是卸载）。
    pub fn extension_count(&self) -> usize {
        self.extensions.len()
    }

    /// 扩展故障累计计数（诊断面；未开扩展 = 0）。
    pub fn extension_faults(&self) -> u64 {
        self.total_faults
    }

    /// 最近一次扩展故障（含扩展 id + JS 异常文本；诊断/Output 用）。
    pub fn last_fault(&self) -> Option<String> {
        self.last_fault.clone()
    }

    /// 能力桥共享句柄（宿主每帧刷新快照 / 落地写队列用）。
    pub fn caps(&self) -> Rc<RefCell<ExtCapsState>> {
        Rc::clone(&self.caps)
    }
}

impl NesRuntime {
    /// 装载一个 `.js` 扩展文件（读盘 -> QuickJS 装载 -> 注册）。
    ///
    /// 首次调用惰性构造 [`ExtensionManager`]（不开扩展的路径零开销，
    /// 与音频面同一纪律）。
    pub fn load_extension_file(&mut self, path: impl AsRef<Path>) -> Result<String, String> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("扩展文件读取失败（{}）：{e}", path.display()))?;
        let fallback = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("extension")
            .to_string();
        if self.extensions.is_none() {
            self.extensions = Some(
                ExtensionManager::new().map_err(|e| format!("扩展运行时初始化失败：{e}"))?,
            );
        }
        self.extensions
            .as_mut()
            .expect("上面刚构造")
            .load_source(&source, &fallback)
            .map_err(|e| format!("扩展装载失败：{e}"))
    }

    /// 推进扩展一帧：快照（tick 后状态）-> update 钩子 -> 写队列落地。
    ///
    /// 返回本帧诊断清单（扩展 JS 异常 / 预算中断 / 停用宣告；空 = 干净）。
    /// 异常不炸帧 —— 引擎照常、其余扩展照常（防线口径见
    /// [`ExtensionManager`] 文档）。**在 simulate 之后调用**（帧序契约见
    /// 模块文档）；写队列当帧落地、下一帧呈现。
    pub fn update_extensions(&mut self) -> Vec<String> {
        let Some(mgr) = &mut self.extensions else {
            return Vec::new();
        };
        // 1) 读面刷新：输入快照投影 + 树快照 + 混音器句柄（字段级借用拆分，
        //    与 mgr 持有的 &mut self.extensions 不相交）。残留待发射缓冲
        //    一并清空（半途炸掉的帧不落地 —— 与写队列同一条防线）。
        {
            let held: Vec<String> = self
                .input_state
                .borrow()
                .held
                .iter()
                .map(|k| k.name())
                .collect();
            let mixer = self.mixer.clone();
            mgr.caps.borrow_mut().refresh(&self.tree, &held, mixer);
        }
        // 2) JS update 钩子（只碰快照/队列/混音器 —— 引擎借用已全部归还）。
        let messages = mgr.update();
        // 3) 写队列落地（扩展写树 = 游戏状态，进指纹）。
        {
            let mut caps = mgr.caps.borrow_mut();
            caps.apply(&mut self.tree);
        }
        // 4) 扩展反向发射（update 期 `emitSignal`）落树：入**下一帧**的
        //    信号泵（宿主预发时序）。泵内 hat 的发射不经过这里 —— 已在
        //    [`ExtensionSignalObserver::on_signal`] 经 `SignalCtx::emit`
        //    同泵级联。不可落地形态（P0 对象拒绝）如实计数不静默编造。
        let pending = mgr.take_pending_emits();
        for (name, payload) in pending {
            match nes_to_value(&payload) {
                Some(v) => self.tree.emit_signal(&name, v),
                None => mgr.note_dropped_payload(),
            }
        }
        messages
    }

    /// 已装载扩展数（未开扩展 = 0）。
    pub fn extension_count(&self) -> usize {
        self.extensions
            .as_ref()
            .map(|m| m.extension_count())
            .unwrap_or(0)
    }

    /// 扩展故障累计计数（诊断面；未开扩展 = 0）。
    pub fn extension_faults(&self) -> u64 {
        self.extensions
            .as_ref()
            .map(|m| m.extension_faults())
            .unwrap_or(0)
    }

    /// 最近一次扩展故障（含扩展 id + JS 异常文本；未开扩展或尚无故障 =
    /// `None`）。
    pub fn last_fault(&self) -> Option<String> {
        self.extensions.as_ref().and_then(|m| m.last_fault())
    }

    /// 被丢弃的扩展信号载荷总数（发射缓冲超限 + 不可落地形态；诊断面，
    /// 未开扩展 = 0）。
    pub fn dropped_signal_payloads(&self) -> u64 {
        self.extensions
            .as_ref()
            .map(|m| m.dropped_signal_payloads())
            .unwrap_or(0)
    }

    /// 是否有扩展订阅过信号 hat（诊断/测试面 —— 帧路径零开销闸的读数：
    /// `false` 时 tick 不组装扩展观察者，路径与 S17.1 基线逐位一致）。
    pub fn extension_has_signal_hats(&self) -> bool {
        self.extensions
            .as_ref()
            .map(|m| m.has_signal_hats())
            .unwrap_or(false)
    }

    /// 扩展已注册的信号名快照（S19.3 SIGNALS 面板数据面；只读转发
    /// [`ExtensionManager::signal_subscriptions`] —— 声明序去重的订阅名；
    /// 未开扩展 = 空表）。壳层静态聚合用它补"扩展 onSignal 注册名"来源。
    pub fn extension_signal_subscriptions(&self) -> Vec<String> {
        self.extensions
            .as_ref()
            .map(|m| m.signal_subscriptions())
            .unwrap_or_default()
    }
}

/// S17.2 扩展信号观察者：hat 的引擎半边。
///
/// 帧路径在发现扩展订阅过信号时，把它与宿主观察者组合进
/// [`nes_scene::TeeObserver`]（宿主在前、扩展在后 —— 派发序即注册序）：
/// 信号泵把命中订阅的信号送进 [`SceneObserver::on_signal`]，本观察者
/// 同步重入各扩展的 JS 蹦床（**in-tick，与游戏观察者同一 tick 同一时序**
/// —— hat 里的写命令当步落地，无滞后），并把 hat 的反向发射经
/// `SignalCtx::emit` 落回**同一泵**（级联，受 1024 交付上限约束）。
///
/// 订阅过滤：只声明扩展订阅过的名字 —— 未订阅信号不进本观察者，
/// 无谓的 JS 调用为零。
///
/// 生命周期：借用 [`ExtensionManager`]（`&'a mut`），每次 tick 就地组装 ——
/// 与宿主观察者的借用互不干扰（`NesRuntime` 的字段级拆借）。
pub struct ExtensionSignalObserver<'a> {
    mgr: &'a mut ExtensionManager,
}

impl<'a> ExtensionSignalObserver<'a> {
    /// 由扩展管理器组装（同一 tick 内派发 + 取走发射都用它）。
    pub fn new(mgr: &'a mut ExtensionManager) -> Self {
        Self { mgr }
    }
}

impl SceneObserver for ExtensionSignalObserver<'_> {
    /// 只订阅扩展声明过的信号名（无订阅 = 泵根本不进本观察者 —— T-HAT-06
    /// 的结构性保证；实际上帧路径在无订阅时根本不组装本观察者）。
    fn signal_filter(&self) -> SignalFilter {
        let names = self.mgr.signal_subscriptions();
        if names.is_empty() {
            return SignalFilter::NONE;
        }
        SignalFilter::Select {
            names,
            prefixes: Vec::new(),
        }
    }

    fn on_signal(&mut self, ctx: &mut SignalCtx<'_>, sig: &Signal) {
        // 1) 派发：树载荷出界 -> 各扩展 JS 蹦床（ExecBudget 预算 + 异常
        //    隔离继承 S17.1 —— hat 里 throw/死循环 = 故障计数，泵不炸）。
        //    诊断已在管理器侧记账（extension_faults / last_fault）；泵内
        //    无消息通道，不重复冒泡。
        let payload = value_to_nes(&sig.payload);
        let _fault_messages = self.mgr.dispatch_signal(&sig.name, payload);
        // 2) 泵内级联：hat 的反向发射落回同一泵（SignalCtx::emit 语义 ——
        //    入队、同泵继续交付；提交序 = 跨扩展发射序，确定性口径）。
        for (name, value) in self.mgr.take_pending_emits() {
            match nes_to_value(&value) {
                Some(v) => ctx.emit(&name, v),
                // P0 对象拒绝：不可落地形态丢弃并如实计数（不编造语义）。
                None => self.mgr.note_dropped_payload(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nes_extension_api::{
        AudioCapability, InputCapability, NodeCapability, NodeRef, SceneCapability,
    };
    use nes_scene::{NodeKind, SceneTree, Transform2D};

    use super::ExtCapsState;

    /// 搭一棵最小树：root + obj1（Sprite2D 位形无所谓，测试只看名字/变换）。
    ///
    /// `add_node` 是 pending 意图 —— `apply_pending` 落地（真引擎在 tick
    /// 内做同一件事；测试不走 tick，就地落地）。
    fn tree_with_obj1() -> SceneTree {
        let mut tree = SceneTree::new("root");
        let root = tree.root();
        let obj = tree.add_node(root, "obj1", NodeKind::Sprite2D);
        tree.set_local(obj, Transform2D::from_pos(10.0, 20.0));
        tree.apply_pending();
        tree
    }

    #[test]
    fn snapshot_reads_reflect_tick_end_tree() {
        let tree = tree_with_obj1();
        let mut caps = ExtCapsState::default();
        caps.refresh(&tree, &[], None);

        let r = SceneCapability::find(&caps, "obj1").expect("obj1 应可找到");
        assert_eq!(caps.get_name(r).as_deref(), Some("obj1"));
        assert_eq!(caps.get_pos(r), Some((10.0, 20.0)));
        assert!(SceneCapability::find(&caps, "missing").is_none());
        // 同名先序第一（与 find_by_name 同口径）：这里只有一个 obj1。
        assert_eq!(
            SceneCapability::find(&caps, "obj1"),
            SceneCapability::find(&caps, "obj1")
        );
    }

    #[test]
    fn queued_writes_land_in_submission_order_and_preserve_other_components() {
        let mut tree = tree_with_obj1();
        let obj = tree.find_by_name("obj1").expect("obj1");
        // 旋转/缩放先行设置 —— setPos 只许改平移分量。
        let tilted = Transform2D { rot: 0.5, scale: nes_scene::Vec2::new(2.0, 3.0), ..Transform2D::from_pos(10.0, 20.0) };
        tree.set_local(obj, tilted);

        let mut caps = ExtCapsState::default();
        caps.refresh(&tree, &[], None);
        let r = SceneCapability::find(&caps, "obj1").unwrap();
        caps.set_pos(r, 30.0, 40.0);
        caps.set_visible(r, false);
        caps.set_pos(r, 50.0, 60.0); // 同帧两写：提交序落地，后者生效
        caps.apply(&mut tree);

        let local = tree.local(obj).unwrap();
        assert_eq!((local.pos.x, local.pos.y), (50.0, 60.0));
        assert_eq!(local.rot, 0.5, "rot 必须保留");
        assert_eq!((local.scale.x, local.scale.y), (2.0, 3.0), "scale 必须保留");
        assert_eq!(tree.prop(obj, nes_render_extract::PROP_VISIBLE), Some(&nes_scene::Value::Bool(false)));
    }

    #[test]
    fn input_snapshot_projection_and_missing_audio_are_silent() {
        let tree = tree_with_obj1();
        let mut caps = ExtCapsState::default();
        caps.refresh(&tree, &["Space".to_string(), "ArrowLeft".to_string()], None);
        assert!(InputCapability::is_pressed(&caps, "Space"));
        assert!(InputCapability::is_pressed(&caps, "ArrowLeft"));
        assert!(!InputCapability::is_pressed(&caps, "KeyX"));
        // 无混音器：play 静默丢弃（trait 契约），不 panic。
        AudioCapability::play(&caps, "Audio/beep", 0.5);
    }

    #[test]
    fn extension_loop_moves_the_tree_without_engine_present() {
        // 端到端（无 NesRuntime）：manager + hello 形 JS + 真 SceneTree。
        // JS 字面量全 ASCII（纪律）。
        let source = r#"
nes.registerExtension("spin");
nes.onUpdate(function () {
  var ref = nes.scene.find("obj1");
  if (ref === null) { return; }
  var p = nes.node.getPos(ref);
  nes.node.setPos(ref, p[0] + 1, p[1] + 2);
});
"#;
        let mut mgr = super::ExtensionManager::new().unwrap();
        let id = mgr.load_source(source, "fallback").unwrap();
        assert_eq!(id, "spin");
        assert_eq!(mgr.extension_count(), 1);

        let mut tree = tree_with_obj1();
        let obj = tree.find_by_name("obj1").unwrap();
        let caps = mgr.caps();
        for frame in 0..5u32 {
            caps.borrow_mut().refresh(&tree, &[], None);
            let errors = mgr.update();
            assert!(errors.is_empty(), "frame {frame}: {errors:?}");
            caps.borrow_mut().apply(&mut tree);
        }
        let local = tree.local(obj).unwrap();
        assert_eq!((local.pos.x, local.pos.y), (15.0, 30.0), "5 帧累计位移");
    }

    #[test]
    fn dangling_node_ref_is_inert() {
        // 位形带 generation：悬垂句柄不得撞上复用槽位的另一个节点。
        let mut tree = tree_with_obj1();
        let obj = tree.find_by_name("obj1").unwrap();
        let mut caps = super::ExtCapsState::default();
        caps.refresh(&tree, &[], None);
        let r = SceneCapability::find(&caps, "obj1").unwrap();
        assert_eq!(r, NodeRef(obj.to_bits()));
        // 伪造代际（gen+1）的句柄：读不到、写落地为 no-op（NodeId 不匹配树内节点）。
        let stale = NodeRef(nes_scene::NodeId::from_bits(r.0 + (1u64 << 32)).to_bits());
        assert_eq!(caps.get_pos(stale), None);
        caps.set_pos(stale, 99.0, 99.0);
        caps.apply(&mut tree);
        assert_eq!(tree.local(obj).unwrap().pos.x, 10.0, "悬垂写不得生效");
    }
}

