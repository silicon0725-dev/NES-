//! 脚本 VM（S6.19）：`Script` 节点的手写栈式字节码解释器。
//!
//! # 定位（零第三方纪律下的诚实形态）
//!
//! 不引入外部语言运行时 —— VM 是**手写的栈式字节码解释器**：脚本是指令
//! 序列（[`Op`]），宿主在 [`ScriptVm`] 注册表里按键登记，`Script` 节点的
//! `registry_key` 属性引用键，[`ScriptVm::attach_all`] 批量装载。
//!
//! # 双入口（全部走既有 substrate，无第二套机制）
//!
//! - **信号入口**（[`ScriptEntry::Signal`]）：VM 在节点处理器表注册解释
//!   闭包（S6.18）并 `connect_signal_to` 连接 —— 信号命中时引擎直接调
//!   闭包。上下文是 [`SignalCtx`]：**可跨节点**读写（引擎级）。
//! - **Process 入口**（[`ScriptEntry::Process`]）：VM 自身实现
//!   [`SceneObserver`]（挂在 attached 节点的 process 上）。上下文是
//!   [`NodeCtx`]：**只能操作自身**（引擎的回调纪律不破）—— 跨节点的事
//!   经 `Emit` 交给信号脚本。
//!
//! # 栈上的节点（不编进 Value）
//!
//! 栈元素是 [`StackVal`]：值（`Value`）或节点句柄（`NodeId` 原生携带，
//! 代际完整）。把节点编进 `Value`（如 Resource 槽位）会丢代际 —— 那是
//! 身份谎言（S6.15 桥信号同裁决）。
//!
//! # 停机保护（三重，全都不崩帧）
//!
//! - 类型/栈/节点寻址错误：停机并把原因写进局部 `__halt`（可经
//!   [`ScriptVm::locals`] 观测）；
//! - 指令步数上限 [`SCRIPT_MAX_STEPS`]：`Jump` 循环不挂起帧；
//! - 缺失属性：`GetProp` 回落 `I64(0)`（与回调静默口径同家法）。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::identity::NodeId;
use crate::node::NodeKindTag;
use crate::transform::Vec2;
use crate::tree::{NodeCtx, SceneObserver, SceneTree, Signal, SignalCtx};
use crate::value::Value;

/// 单次运行的最大指令步数（Jump 循环保护：停机并记 `__halt`，不挂帧）。
pub const SCRIPT_MAX_STEPS: usize = 10_000;

/// 停机原因的局部变量名（可观测，配合 [`ScriptVm::locals`]）。
pub const HALT_LOCAL: &str = "__halt";

/// 一条指令。栈式：大多数指令消费栈顶、产出新值压回。
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// 压常量。
    Const(Value),
    /// 压局部变量（未初始化 -> `I64(0)`）。
    Local(String),
    /// 弹值存局部。
    SetLocal(String),
    /// 压入口参数：process = 本帧 delta（F32）；signal = 信号载荷。
    Arg,
    /// 压脚本宿主节点（attach 的 `Script` 节点）。
    This,
    /// 按名找节点（整树首个命中）压栈；找不到 -> 停机记录。
    NodeByName(String),
    /// 弹节点，压其属性（缺省回落 `I64(0)`）。
    GetProp(String),
    /// 弹值、弹节点，写属性（process 入口只许写自身 —— NodeCtx 纪律）。
    SetProp(String),
    /// 弹节点，压其本地位置（Vec2）。
    GetT,
    /// 弹 Vec2、弹节点，写本地位置（rot/scale 保持；process 入口只许自身）。
    SetT,
    /// 弹 b、a，压 a+b（I64/I64、数值提升 F32、Vec2+Vec2）。
    Add,
    /// 弹 b、a，压 a-b（同上类型规则）。
    Sub,
    /// 弹 b、a，压 a*b（数值提升 F32）。
    Mul,
    /// 弹 b、a，压 a<b（数值比较 -> Bool）。
    Lt,
    /// 弹 b、a，压 a==b（值或节点相等 -> Bool）。
    Eq,
    /// 无条件跳转到指令下标。
    Jump(usize),
    /// 弹 Bool，假则跳转到指令下标。
    JumpIfNot(usize),
    /// 弹载荷，发射命名信号。
    Emit(String),
}

/// 脚本入口。
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptEntry {
    /// 每帧 process（本节点；上下文 NodeCtx，只能操作自身）。
    Process,
    /// 响应命名信号（任意源；上下文 SignalCtx，可跨节点）。
    Signal(String),
}

/// 一个脚本：入口 + 指令序列 + 初始局部。
#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    /// 入口。
    pub entry: ScriptEntry,
    /// 指令序列（`pc` 越界即正常结束）。
    pub ops: Vec<Op>,
    /// 初始局部变量（attach 时注入；重挂载复位）。
    pub locals: BTreeMap<String, Value>,
}

impl Script {
    /// 便捷构造。
    pub fn new(entry: ScriptEntry, ops: Vec<Op>) -> Self {
        Script {
            entry,
            ops,
            locals: BTreeMap::new(),
        }
    }
}

/// 栈元素：值或节点（节点原生携带 NodeId —— 代际完整，见模块文档）。
#[derive(Clone, Debug, PartialEq)]
pub enum StackVal {
    /// 值。
    V(Value),
    /// 节点句柄。
    N(NodeId),
}

/// VM 上下文适配：同一套 [`Op`] 在两种入口下运行，权限按入口收敛。
enum VmCtx<'a, 'b> {
    /// process 入口：本节点的 NodeCtx（只能写自身）。
    Node(&'a mut NodeCtx<'b>),
    /// 信号入口：SignalCtx（可跨节点）。
    Signal(&'a mut SignalCtx<'b>),
}

impl VmCtx<'_, '_> {
    fn tree(&self) -> &SceneTree {
        match self {
            VmCtx::Node(c) => c.tree(),
            VmCtx::Signal(c) => c.tree(),
        }
    }

    fn emit(&mut self, name: &str, payload: Value) {
        match self {
            VmCtx::Node(c) => c.emit(name, payload),
            VmCtx::Signal(c) => c.emit(name, payload),
        }
    }
}

/// 解释执行一次。所有错误路径：停机 + `__halt` 记录，不向上传播（不崩帧）。
fn run<'a, 'b>(
    ops: &[Op],
    host: NodeId,
    locals: &mut BTreeMap<String, Value>,
    ctx: &mut VmCtx<'a, 'b>,
    arg: Value,
) {
    let mut stack: Vec<StackVal> = Vec::new();
    let mut pc = 0usize;
    let mut steps = 0usize;
    macro_rules! halt {
        ($why:expr) => {{
            locals.insert(HALT_LOCAL.to_string(), Value::Str($why.to_string()));
            return;
        }};
    }
    macro_rules! pop_node {
        () => {
            match stack.pop() {
                Some(StackVal::N(n)) => n,
                Some(StackVal::V(_)) => halt!("栈顶不是节点"),
                None => halt!("stack underflow"),
            }
        };
    }
    macro_rules! pop_val {
        () => {
            match stack.pop() {
                Some(StackVal::V(v)) => v,
                Some(StackVal::N(_)) => halt!("栈顶不是值"),
                None => halt!("stack underflow"),
            }
        };
    }
    while pc < ops.len() {
        steps += 1;
        if steps > SCRIPT_MAX_STEPS {
            halt!("step budget exceeded");
        }
        match &ops[pc] {
            Op::Const(v) => stack.push(StackVal::V(v.clone())),
            Op::Local(name) => stack.push(StackVal::V(
                locals.get(name).cloned().unwrap_or(Value::I64(0)),
            )),
            Op::SetLocal(name) => {
                let v = pop_val!();
                locals.insert(name.clone(), v);
            }
            Op::Arg => stack.push(StackVal::V(arg.clone())),
            Op::This => stack.push(StackVal::N(host)),
            Op::NodeByName(name) => match ctx.tree().find_by_name(name) {
                Some(n) => stack.push(StackVal::N(n)),
                None => halt!("node not found"),
            },
            Op::GetProp(name) => {
                let node = pop_node!();
                let v = ctx
                    .tree()
                    .prop(node, name)
                    .cloned()
                    .unwrap_or(Value::I64(0));
                stack.push(StackVal::V(v));
            }
            Op::SetProp(name) => {
                let v = pop_val!();
                let node = pop_node!();
                match ctx {
                    VmCtx::Node(c) => {
                        if node != c.this() {
                            halt!("process 入口只许写自身");
                        }
                        c.set_prop(name, v);
                    }
                    VmCtx::Signal(c) => c.set_prop(node, name, v),
                }
            }
            Op::GetT => {
                let node = pop_node!();
                let t = ctx.tree().local(node).unwrap_or_default();
                stack.push(StackVal::V(Value::Vec2(Vec2::new(t.pos.x, t.pos.y))));
            }
            Op::SetT => {
                let v = pop_val!();
                let node = pop_node!();
                let Value::Vec2(p) = v else {
                    halt!("SetT 需要 Vec2");
                };
                match ctx {
                    VmCtx::Node(c) => {
                        if node != c.this() {
                            halt!("process 入口只许写自身");
                        }
                        let mut t = c.local();
                        t.pos = p;
                        c.set_local(t);
                    }
                    VmCtx::Signal(c) => {
                        let t0 = c.tree().local(node).unwrap_or_default();
                        let mut t = t0;
                        t.pos = p;
                        c.set_local(node, t);
                    }
                }
            }
            Op::Add => {
                let b = pop_val!();
                let a = pop_val!();
                stack.push(StackVal::V(match (a, b) {
                    (Value::I64(x), Value::I64(y)) => Value::I64(x + y),
                    (Value::Vec2(x), Value::Vec2(y)) => Value::Vec2(Vec2::new(x.x + y.x, x.y + y.y)),
                    (a, b) => match (num_of(&a), num_of(&b)) {
                        (Some(x), Some(y)) => Value::F32(x + y),
                        _ => halt!("Add 类型不符"),
                    },
                }));
            }
            Op::Sub => {
                let b = pop_val!();
                let a = pop_val!();
                stack.push(StackVal::V(match (a, b) {
                    (Value::I64(x), Value::I64(y)) => Value::I64(x - y),
                    (Value::Vec2(x), Value::Vec2(y)) => Value::Vec2(Vec2::new(x.x - y.x, x.y - y.y)),
                    (a, b) => match (num_of(&a), num_of(&b)) {
                        (Some(x), Some(y)) => Value::F32(x - y),
                        _ => halt!("Sub 类型不符"),
                    },
                }));
            }
            Op::Mul => {
                let b = pop_val!();
                let a = pop_val!();
                stack.push(StackVal::V(match (num_of(&a), num_of(&b)) {
                    (Some(x), Some(y)) => Value::F32(x * y),
                    _ => halt!("Mul 类型不符"),
                }));
            }
            Op::Lt => {
                let b = pop_val!();
                let a = pop_val!();
                stack.push(StackVal::V(match (num_of(&a), num_of(&b)) {
                    (Some(x), Some(y)) => Value::Bool(x < y),
                    _ => halt!("Lt 类型不符"),
                }));
            }
            Op::Eq => {
                let b = stack.pop();
                let a = stack.pop();
                let eq = match (a, b) {
                    (Some(StackVal::V(x)), Some(StackVal::V(y))) => x == y,
                    (Some(StackVal::N(x)), Some(StackVal::N(y))) => x == y,
                    _ => halt!("Eq 栈不足或混合"),
                };
                stack.push(StackVal::V(Value::Bool(eq)));
            }
            Op::Jump(to) => {
                pc = *to;
                continue;
            }
            Op::JumpIfNot(to) => match pop_val!() {
                Value::Bool(false) => {
                    pc = *to;
                    continue;
                }
                Value::Bool(true) => {}
                _ => halt!("JumpIfNot 需要 Bool"),
            },
            Op::Emit(name) => {
                let payload = pop_val!();
                ctx.emit(name, payload);
            }
        }
        pc += 1;
    }
}

fn num_of(v: &Value) -> Option<f32> {
    match v {
        Value::F32(f) => Some(*f),
        Value::I64(i) => Some(*i as f32),
        _ => None,
    }
}

/// 脚本 VM：注册表 + 每脚本局部状态（跨调用持久 —— 计数器/累积器语义）。
pub struct ScriptVm {
    /// 键 -> 脚本（宿主登记）。
    scripts: BTreeMap<String, Script>,
    /// 节点 -> 局部状态（闭包与观察者路径共享同一份）。
    states: Rc<RefCell<HashMap<NodeId, BTreeMap<String, Value>>>>,
    /// process 入口的（节点, 脚本）表（观察者路径逐帧驱动）。
    process_scripts: Vec<(NodeId, Script)>,
}

impl Default for ScriptVm {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptVm {
    pub fn new() -> Self {
        ScriptVm {
            scripts: BTreeMap::new(),
            states: Rc::new(RefCell::new(HashMap::new())),
            process_scripts: Vec::new(),
        }
    }

    /// 登记一个脚本（键 = `registry_key` 引用值；同名替换）。
    pub fn register(&mut self, key: &str, script: Script) {
        self.scripts.insert(key.to_string(), script);
    }

    /// 查看某节点的脚本局部（含 `__halt` 停机记录 —— 可观测性）。
    pub fn locals(&self, node: NodeId) -> Option<BTreeMap<String, Value>> {
        self.states.borrow().get(&node).cloned()
    }

    /// 装载一个 `Script` 节点（读 `registry_key` -> 注册表查 -> 按入口挂载）。
    /// 返回缺口描述：非 Script 节点、空键、未知键（如实暴露，不静默哑挂）。
    pub fn attach(&mut self, tree: &mut SceneTree, node: NodeId) -> Result<(), String> {
        if tree.kind_tag(node) != Some(NodeKindTag::Script) {
            return Err("节点不是 Script 类型".to_string());
        }
        let key = match tree.prop(node, "registry_key") {
            Some(Value::Str(k)) if !k.is_empty() => k.clone(),
            _ => return Err("registry_key 为空（未绑定）".to_string()),
        };
        let script = self
            .scripts
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("注册表无脚本 `{key}`"))?;

        // 状态初始化（重挂载 = 复位初始局部 + __halt 清除）。
        let mut init = script.locals.clone();
        init.remove(HALT_LOCAL);
        self.states.borrow_mut().insert(node, init);
        // process 入口去重（重挂载替换）。
        self.process_scripts.retain(|(n, _)| *n != node);

        match script.entry.clone() {
            ScriptEntry::Process => {
                self.process_scripts.push((node, script));
                Ok(())
            }
            ScriptEntry::Signal(name) => {
                // 处理器表闭包 + 方法连接（S6.18 substrate）。
                let states = self.states.clone();
                let ok = tree.set_signal_handler(
                    node,
                    "run",
                    Box::new(move |ctx: &mut SignalCtx<'_>, sig: &Signal| {
                        let mut locals = states.borrow_mut().remove(&node).unwrap_or_default();
                        run(
                            &script.ops,
                            node,
                            &mut locals,
                            &mut VmCtx::Signal(ctx),
                            sig.payload.clone(),
                        );
                        states.borrow_mut().insert(node, locals);
                    }),
                );
                if !ok {
                    return Err("处理器注册失败".to_string());
                }
                tree.connect_signal_to(&name, None, node, "run")
                    .map(|_| ())
                    .ok_or_else(|| format!("连接 `{name}` 失败"))
            }
        }
    }

    /// 批量装载：遍历树中全部 `Script` 节点，返回（节点, 缺口）清单 ——
    /// 一个坏键不挡其他节点（部分成功如实上报）。
    pub fn attach_all(&mut self, tree: &mut SceneTree) -> Vec<(NodeId, String)> {
        let nodes: Vec<NodeId> = tree
            .preorder()
            .into_iter()
            .filter(|&n| tree.kind_tag(n) == Some(NodeKindTag::Script))
            .collect();
        let mut issues = Vec::new();
        for n in nodes {
            if let Err(why) = self.attach(tree, n) {
                issues.push((n, why));
            }
        }
        issues
    }
}

impl SceneObserver for ScriptVm {
    /// process 入口：attached 节点的 process 驱动其脚本（NodeCtx 纪律）。
    fn on_process(&mut self, ctx: &mut NodeCtx<'_>, delta: f32) {
        let node = ctx.this();
        let Some(script) = self
            .process_scripts
            .iter()
            .find(|(n, _)| *n == node)
            .map(|(_, s)| s.clone())
        else {
            return;
        };
        let mut locals = self.states.borrow_mut().remove(&node).unwrap_or_default();
        run(&script.ops, node, &mut locals, &mut VmCtx::Node(ctx), Value::F32(delta));
        self.states.borrow_mut().insert(node, locals);
    }
}
