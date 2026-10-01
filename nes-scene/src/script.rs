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
/// init 已执行哨兵（S8.0）：init 块跑完即写入局部 —— "已初始化"由此
/// 可观测（`vm.locals` / 语义指纹都看得见），且天然随重挂载复位。
pub const INIT_LOCAL: &str = "__initialized";

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
    /// 弹 b、a，压 a*b（I64*I64 -> I64；数值提升 F32）。
    Mul,
    /// 弹 b、a，压 a/b —— I64 截断除（**除零停机**：checked，不 panic
    /// 不静默）；浮点走 IEEE（0 除得 ±inf，是数学事实不是错误）。
    /// S6.20 文档写"四则"但实现缺 `/`—— 本轮（S6.28）补缺。
    Div,
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
    /// 弹 b、a（均须 Bool），压 a && b（**按值 eager** —— 表达式层无副作用，
    /// 短路无可观测收益，见 S6.21 文档 §2.1）。
    And,
    /// 弹 b、a（均须 Bool），压 a || b（eager，同上）。
    Or,
    /// 弹 Bool，压非。
    Not,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    /// 弹 Str，压长度（**Unicode 标量数**，非字节 —— `"你好"` 是 2）。
    StrLen,
    /// 弹下标（I64）、弹 Str，压**单字符 Str**（无 char 类型，一字符串即
    /// 字符的表达）。按字符索引；负数或 >= 长度停机记 `__halt`（附下标）。
    StrIndex,
    /// 弹 Str 键名，问**宿主注入的输入探针**该键是否按住，压 Bool
    ///（S7.2：VM 不碰平台 —— 探针由运行时接 `InputSnapshot::is_down`）。
    /// 未注入探针停机（如实：没接就是没有，不装"恒假"）。
    Key,
    /// 弹 y、x（栈序），压 `Vec2(x, y)`（S7.4 真实项目解锁：任意表达式
    /// 构造向量 —— `xy(px + dx, py)`；数字字面量仍走 Const 折叠）。
    Pack,
    /// 弹 Vec2，压 x 分量（F32）。S7.4：`node.pos.x`。
    GetX,
    /// 弹 Vec2，压 y 分量（F32）。S7.4：`node.pos.y`。
    GetY,
    /// 弹数值（I64/F32），压十进制 Str（S8.2 第一块板：Script→Text
    /// 闭环 —— HUD 显示数值）。F32 用最短往返表示（Rust Display 口径，
    /// 跨版本稳定）。
    NumStr,
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
    /// **init 块**（S8.0）：挂载后**首次派发前**执行一次的指令序列
    ///（与入口共享局部）。重挂载 = 局部复位 + 重跑 init（与 S6.32
    /// "换程序不打补丁"同一条语义）。`None` = 无 init（存量脚本不变）。
    pub init: Option<Vec<Op>>,
}

impl Script {
    /// 便捷构造（无 init 块）。
    pub fn new(entry: ScriptEntry, ops: Vec<Op>) -> Self {
        Script {
            entry,
            ops,
            locals: BTreeMap::new(),
            init: None,
        }
    }

    /// 带 init 块构造（S8.0）。
    pub fn with_init(entry: ScriptEntry, ops: Vec<Op>, init: Vec<Op>) -> Self {
        Script {
            entry,
            ops,
            locals: BTreeMap::new(),
            init: Some(init),
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
    probe: &ProbeSlot,
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
                    // 字符串拼接走 `+`（S6.26；Str+Str 严格 —— 混型不停机
                    // 不转换，数字转字符串宿主侧自查自拼）。
                    (Value::Str(x), Value::Str(y)) => Value::Str(x + &y),
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
                stack.push(StackVal::V(match (a, b) {
                    (Value::I64(x), Value::I64(y)) => Value::I64(x * y),
                    (a, b) => match (num_of(&a), num_of(&b)) {
                        (Some(x), Some(y)) => Value::F32(x * y),
                        _ => halt!("Mul 类型不符"),
                    },
                }));
            }
            Op::Div => {
                let b = pop_val!();
                let a = pop_val!();
                stack.push(StackVal::V(match (a, b) {
                    // 整型截断除：checked —— 除零是停机错误（不 panic 不编造值）。
                    (Value::I64(x), Value::I64(y)) => match x.checked_div(y) {
                        Some(v) => Value::I64(v),
                        None => halt!("整型除零"),
                    },
                    // 浮点走 IEEE：0 除得 ±inf（数学事实，不是错误）。
                    (a, b) => match (num_of(&a), num_of(&b)) {
                        (Some(x), Some(y)) => Value::F32(x / y),
                        _ => halt!("Div 类型不符"),
                    },
                }));
            }
            Op::Lt => {
                let b = pop_val!();
                let a = pop_val!();
                stack.push(StackVal::V(match (a, b) {
                    // 字符串按码点字典序（S6.30）—— Rust String Ord 即此序。
                    (Value::Str(x), Value::Str(y)) => Value::Bool(x < y),
                    (a, b) => match (num_of(&a), num_of(&b)) {
                        (Some(x), Some(y)) => Value::Bool(x < y),
                        _ => halt!("Lt 类型不符"),
                    },
                }));
            }
            Op::Eq => {
                let b = stack.pop();
                let a = stack.pop();
                let eq = match (a, b) {
                    // 数值按值比较（1 == 1.0 为真 —— 数值语言直觉）；其余严格。
                    (Some(StackVal::V(x)), Some(StackVal::V(y))) => match (num_of(&x), num_of(&y)) {
                        (Some(u), Some(v)) => u == v,
                        _ => x == y,
                    },
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
            Op::And => {
                let b = pop_val!();
                let a = pop_val!();
                stack.push(StackVal::V(match (a, b) {
                    (Value::Bool(x), Value::Bool(y)) => Value::Bool(x && y),
                    _ => halt!("And 需要 Bool"),
                }));
            }
            Op::Or => {
                let b = pop_val!();
                let a = pop_val!();
                stack.push(StackVal::V(match (a, b) {
                    (Value::Bool(x), Value::Bool(y)) => Value::Bool(x || y),
                    _ => halt!("Or 需要 Bool"),
                }));
            }
            Op::Not => {
                let a = pop_val!();
                stack.push(StackVal::V(match a {
                    Value::Bool(x) => Value::Bool(!x),
                    _ => halt!("Not 需要 Bool"),
                }));
            }
            Op::Mod => {
                let b = pop_val!();
                let a = pop_val!();
                stack.push(StackVal::V(match (a, b) {
                    (Value::I64(x), Value::I64(y)) => Value::I64(x % y),
                    (a, b) => match (num_of(&a), num_of(&b)) {
                        (Some(x), Some(y)) => Value::F32(x % y),
                        _ => halt!("Mod 类型不符"),
                    },
                }));
            }
            Op::BitAnd | Op::BitOr | Op::BitXor => {
                let b = pop_val!();
                let a = pop_val!();
                let (Value::I64(x), Value::I64(y)) = (a, b) else {
                    halt!("位运算需要 I64");
                };
                let v = match ops[pc] {
                    Op::BitAnd => x & y,
                    Op::BitOr => x | y,
                    _ => x ^ y,
                };
                stack.push(StackVal::V(Value::I64(v)));
            }
            Op::Shl | Op::Shr => {
                let b = pop_val!();
                let a = pop_val!();
                let (Value::I64(x), Value::I64(y)) = (a, b) else {
                    halt!("移位需要 I64");
                };
                // wrapping：移位量按 2^6 取模（x86 语义），大移位量不崩帧。
                let v = match ops[pc] {
                    Op::Shl => x.wrapping_shl(y as u32),
                    _ => x.wrapping_shr(y as u32),
                };
                stack.push(StackVal::V(Value::I64(v)));
            }
            Op::StrLen => {
                let a = pop_val!();
                let Value::Str(s) = a else {
                    halt!("len 需要 Str");
                };
                stack.push(StackVal::V(Value::I64(s.chars().count() as i64)));
            }
            Op::StrIndex => {
                let idx = pop_val!();
                let a = pop_val!();
                let Value::Str(s) = a else {
                    halt!("索引需要 Str");
                };
                let Value::I64(i) = idx else {
                    halt!("下标需要 I64");
                };
                // 负数或 >= 长度：停机附下标（诚实指名，不回绕不编空）。
                if i < 0 || i >= s.chars().count() as i64 {
                    halt!(format!("索引越界 {i}（长度 {}）", s.chars().count()));
                }
                let ch = s.chars().nth(i as usize).expect("已校验范围");
                stack.push(StackVal::V(Value::Str(ch.to_string())));
            }
            Op::Key => {
                let a = pop_val!();
                let Value::Str(name) = a else {
                    halt!("key(..) 需要 Str 键名");
                };
                // 槽内克隆再调用：探针闭包只读快照，不回调进 VM（无重入）。
                let Some(p) = probe.borrow().clone() else {
                    halt!(format!("key(\"{name}\") 未接输入探针（宿主未注入）"));
                };
                stack.push(StackVal::V(Value::Bool(p(&name))));
            }
            Op::Pack => {
                let y = pop_val!();
                let x = pop_val!();
                let (Some(x), Some(y)) = (num_of(&x), num_of(&y)) else {
                    halt!("xy(..) 需要数值分量");
                };
                stack.push(StackVal::V(Value::Vec2(Vec2::new(x, y))));
            }
            Op::GetX => {
                let a = pop_val!();
                let Value::Vec2(v) = a else {
                    halt!(".x 需要 Vec2");
                };
                stack.push(StackVal::V(Value::F32(v.x)));
            }
            Op::GetY => {
                let a = pop_val!();
                let Value::Vec2(v) = a else {
                    halt!(".y 需要 Vec2");
                };
                stack.push(StackVal::V(Value::F32(v.y)));
            }
            Op::NumStr => {
                let a = pop_val!();
                let text = match a {
                    Value::I64(i) => i.to_string(),
                    Value::F32(f) => f.to_string(),
                    _ => halt!("num_to_str 需要数值"),
                };
                stack.push(StackVal::V(Value::Str(text)));
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

/// 键探针（宿主注入的 `key("名") -> Bool` 求值源；VM 不碰平台）。
pub type KeyProbe = Rc<dyn Fn(&str) -> bool>;

/// 探针共享槽：信号处理器闭包（装进树的处理器表）与 process 路径
/// （留在 VM 里）共享同一份 —— attach 后 `set_key_probe` 也立即生效。
type ProbeSlot = Rc<RefCell<Option<KeyProbe>>>;

/// 脚本 VM：注册表 + 每脚本局部状态（跨调用持久 —— 计数器/累积器语义）。
pub struct ScriptVm {    /// 键 -> 脚本（宿主登记 + 内嵌派生键）。
    scripts: BTreeMap<String, Script>,
    /// 节点 -> 局部状态（闭包与观察者路径共享同一份）。
    states: Rc<RefCell<HashMap<NodeId, BTreeMap<String, Value>>>>,
    /// process 入口的（节点, 脚本）表（观察者路径逐帧驱动）。
    process_scripts: Vec<(NodeId, Script)>,
    /// 内嵌路径的**编译时戳**（节点 -> 上次成功编译的 source 文本）——
    /// [`Self::poll_reloads`] 的比对基准（S6.32）。编译失败不更新戳：
    /// 旧行为保留、下次 poll 重试，修好即生效。
    inline_stamp: HashMap<NodeId, String>,
    /// 外置路径的**编译时戳**（节点 -> 上次成功编译的文件文本，S6.33）
    /// —— poll_reloads_with_sources 的比对基准（last-good 同内嵌）。
    file_stamp: HashMap<NodeId, String>,
    /// 节点 -> 该节点的信号连接句柄（S6.32）。重挂载**先断旧再接新**
    /// —— 否则每次 attach 叠加一条连接、信号命中多次（潜伏缺口实证修复）。
    node_conn: HashMap<NodeId, crate::tree::SignalConnectionId>,
    /// 键探针槽（S7.2）：宿主经 [`Self::set_key_probe`] 注入；信号处理器
    /// 闭包与 process 路径共享同一份（attach 后改设也立即生效）。
    probe: ProbeSlot,
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
            inline_stamp: HashMap::new(),
            file_stamp: HashMap::new(),
            node_conn: HashMap::new(),
            probe: Rc::new(RefCell::new(None)),
        }
    }

    /// 注入键探针（S7.2）：`key("名")` 的求值源 —— 运行时把
    /// `InputSnapshot::is_down` 接进来（VM 不碰平台，与文件读取器
    /// 同一注入纪律）。可在 attach 之前或之后设置。
    pub fn set_key_probe(&mut self, probe: KeyProbe) {
        *self.probe.borrow_mut() = Some(probe);
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
    ///
    /// 本方法只解析**内嵌**（source）与 **registry_key** 两路；外置
    /// `script` 属性（.nes 文件）需要文件读取器，走
    /// [`Self::attach_all_with_sources`] / [`Self::attach_external`]。
    pub fn attach(&mut self, tree: &mut SceneTree, node: NodeId) -> Result<(), String> {
        if tree.kind_tag(node) != Some(NodeKindTag::Script) {
            return Err("节点不是 Script 类型".to_string());
        }
        // 三路挂载恰一非空（S6.33 收紧）：source（内嵌）/ script（外置）/
        // registry_key（宿主注册表）。多路同设是接线错误：如实指名拒绝。
        Self::check_exclusive(tree, node)?;
        if matches!(tree.prop(node, "script"), Some(Value::Resource(n)) if *n != 0) {
            return Err("外置脚本（script 属性）需要 attach_all_with_sources".to_string());
        }
        let key_owned;
        let key: &str = if let Some(Value::Str(src)) = tree.prop(node, "source") {
            if !src.is_empty() {
                // 编译即装载：错误带脚本文本的行/列（attach 缺口如实上报）。
                let script = compile_script(src)
                    .map_err(|e| format!("内嵌脚本编译失败：{e}"))?;
                // 注册表键用节点路径无关的稳定派生键（宿主不感知；重挂载
                // 覆盖同键）。源码属节点所有，不占宿主命名空间。
                key_owned = format!("__inline__:{:?}", node);
                self.scripts.insert(key_owned.clone(), script);
                // 戳 = 本次成功编译的源文本（poll_reloads 的比对基准）。
                self.inline_stamp.insert(node, src.clone());
                &key_owned
            } else {
                Self::require_key(tree, node)?
            }
        } else {
            Self::require_key(tree, node)?
        };
        let script = self
            .scripts
            .get(key)
            .cloned()
            .ok_or_else(|| format!("注册表无脚本 `{key}`"))?;
        self.install(tree, node, script)
    }

    /// 外置脚本装载（S6.33）：`script` 属性（资源槽位）-> 表查路径 ->
    /// 调用方已读好的文本 -> 编译 -> 安装。派生键 `__file__:{path}` ——
    /// **同文件多节点共享同一编译产物**（scripts 表自然去重）；戳为文件
    /// 文本（热重载比对基准，last-good 语义同内嵌）。
    pub fn attach_external(
        &mut self,
        tree: &mut SceneTree,
        node: NodeId,
        path: &str,
        text: &str,
    ) -> Result<(), String> {
        if tree.kind_tag(node) != Some(NodeKindTag::Script) {
            return Err("节点不是 Script 类型".to_string());
        }
        Self::check_exclusive(tree, node)?;
        let script = compile_script(text).map_err(|e| format!("外置脚本 {path} 编译失败：{e}"))?;
        let key = format!("__file__:{path}");
        self.scripts.insert(key.clone(), script.clone());
        self.file_stamp.insert(node, text.to_string());
        self.install(tree, node, script)
    }

    /// 三路挂载互斥校验：恰好一路非空。多于一路指名全部违规路。
    fn check_exclusive(tree: &SceneTree, node: NodeId) -> Result<(), String> {
        let mut active: Vec<&str> = Vec::new();
        if matches!(tree.prop(node, "source"), Some(Value::Str(s)) if !s.is_empty()) {
            active.push("source");
        }
        if matches!(tree.prop(node, "script"), Some(Value::Resource(n)) if *n != 0) {
            active.push("script");
        }
        if matches!(tree.prop(node, "registry_key"), Some(Value::Str(k)) if !k.is_empty()) {
            active.push("registry_key");
        }
        match active.len() {
            0 | 1 => Ok(()),
            _ => Err(format!(
                "挂载互斥（恰一非空）：同时设了 {}",
                active.join("、")
            )),
        }
    }

    /// 装载尾段（三路共享）：状态复位 + 入口安装（处理器/连接，
    /// 先断旧再接新）。
    fn install(
        &mut self,
        tree: &mut SceneTree,
        node: NodeId,
        script: Script,
    ) -> Result<(), String> {
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
                let probe = self.probe.clone();
                let ok = tree.set_signal_handler(
                    node,
                    "run",
                    Box::new(move |ctx: &mut SignalCtx<'_>, sig: &Signal| {
                        let mut locals = states.borrow_mut().remove(&node).unwrap_or_default();
                        if let Some(init) = &script.init {
                            if !locals.contains_key(INIT_LOCAL) {
                                run(init, node, &mut locals, &mut VmCtx::Signal(ctx), Value::I64(0), &probe);
                                locals.insert(INIT_LOCAL.to_string(), Value::Bool(true));
                            }
                        }
                        run(
                            &script.ops,
                            node,
                            &mut locals,
                            &mut VmCtx::Signal(ctx),
                            sig.payload.clone(),
                            &probe,
                        );
                        states.borrow_mut().insert(node, locals);
                    }),
                );
                if !ok {
                    return Err("处理器注册失败".to_string());
                }
                // 重挂载不叠加连接：先断旧句柄再接新（S6.32 实证的潜伏缺口
                // —— 否则热重载一次信号命中 N 次）。
                if let Some(old) = self.node_conn.remove(&node) {
                    tree.disconnect_signal(old);
                }
                let conn = tree
                    .connect_signal_to(&name, None, node, "run")
                    .ok_or_else(|| format!("连接 `{name}` 失败"))?;
                self.node_conn.insert(node, conn);
                Ok(())
            }
        }
    }

    /// registry_key 路径的键提取（source 为空时走此路）。
    fn require_key(tree: &SceneTree, node: NodeId) -> Result<&str, String> {
        match tree.prop(node, "registry_key") {
            Some(Value::Str(k)) if !k.is_empty() => Ok(k.as_str()),
            _ => Err("registry_key 为空（未绑定）".to_string()),
        }
    }

    /// 死节点清理（内存卫生口径）：五张节点登记表按 arena 存活裁剪。
    ///
    /// S6.32 引入（attach_all 前置），收束阶段统一：S6.33 新增的
    /// `file_stamp` 当时漏进清理表、`attach_all_with_sources` 整个入口
    /// 没有清理 —— 五个装载/轮询入口现在都走这里。NodeId 带代号
    /// （slot+gen），陈旧键即使残留也不会误交付；清理是卫生措施 +
    /// 防陈旧戳误判，不是正确性防线。
    fn prune_dead(&mut self, tree: &SceneTree) {
        self.states.borrow_mut().retain(|n, _| tree.contains(*n));
        self.process_scripts.retain(|(n, _)| tree.contains(*n));
        self.inline_stamp.retain(|n, _| tree.contains(*n));
        self.file_stamp.retain(|n, _| tree.contains(*n));
        self.node_conn.retain(|n, _| tree.contains(*n));
    }

    /// VM 登记的节点数（五表取最大 —— 观测内存卫生的口径；正常时
    /// 五表同键集）。测试与诊断用：树整体替换 + 重装载后应归零。
    pub fn tracked_nodes(&self) -> usize {
        self.states
            .borrow()
            .len()
            .max(self.process_scripts.len())
            .max(self.inline_stamp.len())
            .max(self.file_stamp.len())
            .max(self.node_conn.len())
    }

    /// 批量装载：遍历树中全部 `Script` 节点，返回（节点, 缺口）清单 ——
    /// 一个坏键不挡其他节点（部分成功如实上报）。
    ///
    /// 前置死节点清理见 [`Self::prune_dead`]。
    pub fn attach_all(&mut self, tree: &mut SceneTree) -> Vec<(NodeId, String)> {
        self.prune_dead(tree);
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

    /// **脚本热重载**（S6.32）：轮询全部内嵌（source 路径）脚本的节点，
    /// `source` 属性与编译时戳不同的 -> 重新编译重挂载。
    ///
    /// 返回 `(重载成功清单, 失败清单(节点, 错误))`。语义口径：
    /// - **编译失败保留旧行为**：戳不更新（仍为上次成功编译的文本），
    ///   旧脚本继续跑、错误进清单 —— 修好源码后下次 poll 即生效；
    /// - 重挂载 = **换程序不打补丁**：局部复位为初始值（attach 既有语义）；
    /// - `registry_key` 路径不经此轮询（宿主改注册表 + 手动 re-attach，
    ///   宿主全权）；死节点不参与（stamp 在 attach_all 已清理，或经
    ///   arena 查无自然跳过）。
    pub fn poll_reloads(
        &mut self,
        tree: &mut SceneTree,
    ) -> (Vec<NodeId>, Vec<(NodeId, String)>) {
        self.prune_dead(tree);
        let mut reloaded = Vec::new();
        let mut failed = Vec::new();
        let nodes: Vec<NodeId> = tree
            .preorder()
            .into_iter()
            .filter(|&n| tree.kind_tag(n) == Some(NodeKindTag::Script))
            .collect();
        for node in nodes {
            let Some(Value::Str(src)) = tree.prop(node, "source") else {
                continue;
            };
            if src.is_empty() || self.inline_stamp.get(&node) == Some(src) {
                continue; // 非内嵌路径 / 未变化
            }
            match self.attach(tree, node) {
                Ok(()) => reloaded.push(node),
                Err(e) => failed.push((node, e)),
            }
        }
        (reloaded, failed)
    }

    /// **全路径装载**（S6.33）：内嵌 + registry_key + **外置 `script` 属性**
    ///（.nes 文件）。外置解析：槽位 -> 资源表查路径 -> `read` 读文本 ->
    /// [`Self::attach_external`]。返回缺口清单（读失败/编译失败如实指名，
    /// 不挡其他节点）。
    ///
    /// `read` 是注入的文件读取器（路径相对资产根）—— 与子场景展开的
    /// `expand_subscenes` 同一注入模式，VM 不碰文件系统。
    pub fn attach_all_with_sources(
        &mut self,
        tree: &mut SceneTree,
        table: &crate::resources::ResourceTable,
        read: &mut dyn FnMut(&str) -> Result<String, String>,
    ) -> Vec<(NodeId, String)> {
        // 前置死节点清理（与其余装载/轮询入口统一，见 prune_dead）。
        self.prune_dead(tree);
        // 外置节点先解析（读文件 + 编译），再统一走 attach。
        let nodes: Vec<NodeId> = tree
            .preorder()
            .into_iter()
            .filter(|&n| tree.kind_tag(n) == Some(NodeKindTag::Script))
            .collect();
        let mut issues = Vec::new();
        for node in nodes {
            if let Some(Value::Resource(slot)) = tree.prop(node, "script") {
                if *slot != 0 {
                    let resolved = Self::external_path(table, *slot)
                        .and_then(|path| read(&path).map(|text| (path, text)));
                    match resolved {
                        Ok((path, text)) => {
                            if let Err(e) = self.attach_external(tree, node, &path, &text) {
                                issues.push((node, e));
                            }
                            continue;
                        }
                        Err(e) => {
                            issues.push((node, e));
                            continue;
                        }
                    }
                }
            }
            if let Err(e) = self.attach(tree, node) {
                issues.push((node, e));
            }
        }
        issues
    }

    /// **全路径热重载轮询**（S6.33）：内嵌戳比对（同 [`Self::poll_reloads`]）
    /// 加外置文件重读比对（读到的文本 != 戳 -> 重编译重挂载，last-good）。
    /// 语义同 S6.32：编译/读失败保留旧行为、错误进清单、修好下次即生效。
    pub fn poll_reloads_with_sources(
        &mut self,
        tree: &mut SceneTree,
        table: &crate::resources::ResourceTable,
        read: &mut dyn FnMut(&str) -> Result<String, String>,
    ) -> (Vec<NodeId>, Vec<(NodeId, String)>) {
        // 先清死节点（树整体替换后的残留，五表统一见 prune_dead）。
        self.prune_dead(tree);
        let mut reloaded = Vec::new();
        let mut failed = Vec::new();
        let nodes: Vec<NodeId> = tree
            .preorder()
            .into_iter()
            .filter(|&n| tree.kind_tag(n) == Some(NodeKindTag::Script))
            .collect();
        for node in nodes {
            if let Some(Value::Resource(slot)) = tree.prop(node, "script") {
                if *slot != 0 {
                    let resolved = Self::external_path(table, *slot)
                        .and_then(|path| read(&path).map(|text| (path, text)));
                    match resolved {
                        Ok((path, text)) => {
                            if self.file_stamp.get(&node) == Some(&text) {
                                continue; // 文件未变
                            }
                            match self.attach_external(tree, node, &path, &text) {
                                Ok(()) => reloaded.push(node),
                                Err(e) => failed.push((node, e)),
                            }
                        }
                        Err(e) => failed.push((node, e)), // 读失败：旧行为保留
                    }
                    continue;
                }
            }
            // 内嵌路径。
            let Some(Value::Str(src)) = tree.prop(node, "source") else {
                continue;
            };
            if src.is_empty() || self.inline_stamp.get(&node) == Some(src) {
                continue;
            }
            match self.attach(tree, node) {
                Ok(()) => reloaded.push(node),
                Err(e) => failed.push((node, e)),
            }
        }
        (reloaded, failed)
    }

    /// 资源槽位 -> 外置脚本路径（表声明缺位/非 Script 类如实报错）。
    fn external_path(
        table: &crate::resources::ResourceTable,
        slot: u64,
    ) -> Result<String, String> {
        use nes_asset::AssetKind;
        let entry = table
            .iter()
            .find(|e| e.id().get() as u64 == slot)
            .ok_or_else(|| format!("外置脚本槽位 {slot} 未在资源表声明"))?;
        if entry.kind() != Some(AssetKind::Script) {
            return Err(format!(
                "外置脚本槽位 {slot} 类别不是 Script（实际 {:?}）",
                entry.kind()
            ));
        }
        let path = entry
            .path()
            .map(|p| p.as_str().to_string())
            .ok_or_else(|| format!("外置脚本槽位 {slot} 无路径"))?;
        Ok(path)
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
        if let Some(init) = &script.init {
            if !locals.contains_key(INIT_LOCAL) {
                run(init, node, &mut locals, &mut VmCtx::Node(ctx), Value::I64(0), &self.probe);
                locals.insert(INIT_LOCAL.to_string(), Value::Bool(true));
            }
        }
        run(
            &script.ops,
            node,
            &mut locals,
            &mut VmCtx::Node(ctx),
            Value::F32(delta),
            &self.probe,
        );
        self.states.borrow_mut().insert(node, locals);
    }
}

// ---------- 文本语法与编译（S6.20） ----------
//
// 手写词法 + 递归下降编译器（与 scene_io 的 RON 解析器同一纪律）。
// 语法（Rust-lite 最小集）：
//
// ```text
// on "step" {                        // 入口：on "信号名" / every（process）
//     n = n + 1                      // 局部赋值（裸标识符）
//     sprite.pos = sprite.pos + arg  // 变换读写（节点.pos，arg=入口参数）
//     sprite.flip_h = true           // 属性读写（节点.属性名）
//     if 2 < n { emit "done" n }     // 条件（无 else；比较单级）
//     emit "tick" (1.0, 0.0)         // 发射（名 + 载荷；Vec2 字面量仅数字）
// }
// ```
//
// 栈序由编译器按构造保证（如 `x.pos = x.pos + d` 自然编译为
// [N,GetT,d...,N,SetT] —— 节点压两次）；保留字：on/every/if/emit/
// arg/this/true/false；`//` 行注释；语句以换行或 `;` 分隔。

use crate::scene_io::ParseError;

/// 编译文本脚本为 [`Script`]。语法/用词错误如实报错（行/列定位）；
/// 类型错误不在此层（运行时停机兜底，见 §1.4）。
pub fn compile_script(src: &str) -> Result<Script, ParseError> {
    let toks = lex(src)?;
    let mut p = TextParser {
        toks,
        pos: 0,
        loops: Vec::new(),
    };
    p.script()
}

// ------------------------------------------------ 词法

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    /// 数值（值，是否整数字面量）。
    Num(f64, bool),
    Str(String),
    /// 单字符符号。
    Sym(char),
    /// 双字符符号（目前只有 `==`）。
    Sym2(String),
    Newline,
    Eof,
}

#[derive(Clone, Debug)]
struct Spanned {
    tok: Tok,
    line: usize,
    col: usize,
}

fn lex(src: &str) -> Result<Vec<Spanned>, ParseError> {
    let mut out = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut col = 1usize;
    let n = chars.len();
    macro_rules! err {
        ($msg:expr) => {
            return Err(ParseError::new(line, col, $msg))
        };
    }
    while i < n {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' => {
                i += 1;
                col += 1;
            }
            '\n' => {
                out.push(Spanned {
                    tok: Tok::Newline,
                    line,
                    col,
                });
                i += 1;
                line += 1;
                col = 1;
            }
            '/' if i + 1 < n && chars[i + 1] == '/' => {
                while i < n && chars[i] != '\n' {
                    i += 1;
                    col += 1;
                }
            }
            '"' => {
                let (s, adv) = lex_string(&chars[i + 1..], line, col)?;
                out.push(Spanned {
                    tok: Tok::Str(s),
                    line,
                    col,
                });
                i += 1 + adv;
                col += 1 + adv;
            }
            '0'..='9' => {
                // 进制字面量（S6.27）：0x/0X 十六进制、0b/0B 二进制、0o/0O 八进制
                // —— **整型严格**（进制浮点不存在，`.` 会停扫交给后续符号）；
                // 允许 `_` 分隔（0xFF_FF）；非法进制数字/前缀后无数字/溢出
                // i64 在词法层如实报错（0b12 的 `2`、0xFFg 的 `g` 都当场指名）。
                let radix = if c == '0' && i + 1 < n {
                    match chars[i + 1] {
                        'x' | 'X' => Some(16u32),
                        'b' | 'B' => Some(2u32),
                        'o' | 'O' => Some(8u32),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(radix) = radix {
                    let mut j = i + 2;
                    let mut digits = String::new();
                    while j < n && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                        let ch = chars[j];
                        if ch != '_' {
                            if ch.to_digit(radix).is_none() {
                                err!(format!("进制字面量含非法数字 `{ch}`（基数 {radix}）"));
                            }
                            digits.push(ch);
                        }
                        j += 1;
                    }
                    if digits.is_empty() {
                        err!("进制前缀后须有数字");
                    }
                    let v: i64 = i64::from_str_radix(&digits, radix)
                        .map_err(|_| ParseError::new(line, col, "进制字面量超出 i64"))?;
                    out.push(Spanned {
                        tok: Tok::Num(v as f64, true),
                        line,
                        col,
                    });
                    col += j - i;
                    i = j;
                } else {
                    // 十进制（含小数；`..` 停扫见下）。
                    let start = i;
                    let mut is_int = true;
                    while i < n && (chars[i].is_ascii_digit() || chars[i] == '.') {
                        if chars[i] == '.' {
                            // 区间分隔符（S6.24）：`0..n` / `1.5..2` —— 数字遇 `..` 停扫。
                            if i + 1 < n && chars[i + 1] == '.' {
                                break;
                            }
                            if !is_int {
                                err!("数字里多余的 `.`");
                            }
                            is_int = false;
                        }
                        i += 1;
                    }
                    let text: String = chars[start..i].iter().collect();
                    let v: f64 =
                        text.parse().map_err(|_| ParseError::new(line, col, "非法数字"))?;
                    out.push(Spanned {
                        tok: Tok::Num(v, is_int),
                        line,
                        col,
                    });
                    col += i - start;
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                out.push(Spanned {
                    tok: Tok::Ident(s),
                    line,
                    col,
                });
                col += i - start;
            }
            // 三字符符号族（S6.25 `..=`；S6.28 复合移位 `<<=` `>>=`）。
            _ if i + 2 < n
                && matches!(
                    (c, chars[i + 1], chars[i + 2]),
                    ('.', '.', '=') | ('<', '<', '=') | ('>', '>', '=')
                ) =>
            {
                let triple: String = [c, chars[i + 1], chars[i + 2]].iter().collect();
                out.push(Spanned {
                    tok: Tok::Sym2(triple),
                    line,
                    col,
                });
                i += 3;
                col += 3;
            }
            // 双字符符号族（S6.21 比较/逻辑；S6.26 移位；S6.28 复合赋值对）。
            _ if i + 1 < n
                && matches!(
                    (c, chars[i + 1]),
                    ('=', '=')
                        | ('<', '=')
                        | ('>', '=')
                        | ('!', '=')
                        | ('&', '&')
                        | ('|', '|')
                        | ('.', '.')
                        | ('<', '<')
                        | ('>', '>')
                        | ('+', '=')
                        | ('-', '=')
                        | ('*', '=')
                        | ('/', '=')
                        | ('%', '=')
                        | ('&', '=')
                        | ('|', '=')
                        | ('^', '=')
                        // ++/--（S6.29，最大匹配如 C：`a--1` 是 (a--)-1 而非 a-(-1)，
                        // 想减负数请加空格 `a - -1`）。
                        | ('+', '+')
                        | ('-', '-')
                ) =>
            {
                let pair: String = [c, chars[i + 1]].iter().collect();
                out.push(Spanned {
                    tok: Tok::Sym2(pair),
                    line,
                    col,
                });
                i += 2;
                col += 2;
            }
            '{' | '}' | '(' | ')' | '[' | ']' | ',' | '.' | ':' | '=' | '+' | '-' | '*' | '<'
                | '>' | '!' | ';' | '&' | '|' | '^' | '%' | '/' => {
                out.push(Spanned {
                    tok: Tok::Sym(c),
                    line,
                    col,
                });
                i += 1;
                col += 1;
            }
            other => err!(format!("非法字符 `{other}`")),
        }
    }
    out.push(Spanned {
        tok: Tok::Eof,
        line,
        col,
    });
    Ok(out)
}

/// 字符串字面量（到收尾引号；支持 `\"` `\` `\n` `\t`）。返回（值, 消耗数）。
fn lex_string(chars: &[char], mut line: usize, mut col: usize) -> Result<(String, usize), ParseError> {
    let mut s = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => return Ok((s, i + 1)),
            '\n' => return Err(ParseError::new(line, col, "字符串没有收尾引号")),
            '\\' => {
                let e = chars.get(i + 1).copied().unwrap_or('?');
                s.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    '"' => '"',
                    '\\' => '\\',
                    other => return Err(ParseError::new(line, col, format!("非法转义 \\{other}"))),
                });
                i += 2;
                col += 2;
            }
            other => {
                s.push(other);
                i += 1;
                col += 1;
                if other == '\n' {
                    line += 1;
                    col = 1;
                }
            }
        }
    }
    Err(ParseError::new(line, col, "字符串没有收尾引号"))
}

// ------------------------------------------------ 语法 -> Op

const RESERVED: [&str; 15] = [
    "on", "every", "if", "else", "while", "for", "in", "step", "break", "continue", "emit",
    "arg", "this", "true", "false",
];

/// 编译期循环上下文（S6.22）：`continue` 的目标（循环顶）即时可知；
/// `break` 的出口下标在循环收尾才确定 —— 占位回填（同 if 的 JumpIfNot）。
struct LoopCtx {
    /// 循环顶（条件求值处）—— `continue` 目标。
    top: usize,
    /// 循环体内 break 的 `Jump(0)` 占位下标清单（含内层带标签登记的跨层占位）。
    breaks: Vec<usize>,
    /// 标签（`name: while`；`None` = 无标签 —— 标签名不与无标签层匹配）。
    label: Option<String>,
}

struct TextParser {
    toks: Vec<Spanned>,
    pos: usize,
    /// 循环栈（嵌套时 `break`/`continue` 绑定最内层）。
    loops: Vec<LoopCtx>,
}

impl TextParser {
    fn peek(&self) -> &Spanned {
        self.toks.get(self.pos).unwrap_or(&Spanned {
            tok: Tok::Eof,
            line: 0,
            col: 0,
        })
    }

    fn peek2(&self) -> &Spanned {
        self.toks.get(self.pos + 1).unwrap_or(&Spanned {
            tok: Tok::Eof,
            line: 0,
            col: 0,
        })
    }

    fn next(&mut self) -> Spanned {
        let t = self.peek().clone();
        self.pos += 1;
        t
    }

    fn err_here(&self, msg: impl Into<String>) -> ParseError {
        let s = self.peek();
        ParseError::new(s.line, s.col, msg)
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek().tok, Tok::Newline | Tok::Sym(';')) {
            self.pos += 1;
        }
    }

    fn expect_sym(&mut self, c: char) -> Result<(), ParseError> {
        if matches!(&self.peek().tok, Tok::Sym(s) if *s == c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err_here(format!("期望 `{c}`")))
        }
    }

    fn expect_str(&mut self) -> Result<String, ParseError> {
        match self.next().tok {
            Tok::Str(s) => Ok(s),
            _ => Err(self.err_here("期望字符串字面量")),
        }
    }

    /// script := ["init" "{" stmts "}"] ("on" STRING | "every") "{" stmts "}"
    ///
    /// init 块（S8.0）：可选、在前、每脚本至多一个 —— 首次派发前执行
    /// 一次（重挂载重跑）。与入口共享局部变量。
    fn script(&mut self) -> Result<Script, ParseError> {
        self.skip_newlines();
        let mut init = None;
        if matches!(self.peek().tok, Tok::Ident(ref k) if k == "init")
            && matches!(self.peek2().tok, Tok::Sym('{'))
        {
            self.pos += 2; // init {
            let mut iops = Vec::new();
            self.stmts(&mut iops)?;
            self.expect_sym('}')?;
            self.skip_newlines();
            init = Some(iops);
        }
        let entry = match self.next().tok {
            Tok::Ident(k) if k == "on" => ScriptEntry::Signal(self.expect_str()?),
            Tok::Ident(k) if k == "every" => ScriptEntry::Process,
            _ => return Err(self.err_here("期望 `init`、`on \"信号名\"` 或 `every`")),
        };
        self.expect_sym('{')?;
        let mut ops = Vec::new();
        self.stmts(&mut ops)?;
        self.expect_sym('}')?;
        self.skip_newlines();
        if !matches!(self.peek().tok, Tok::Eof) {
            return Err(self.err_here("脚本结尾后有多余内容"));
        }
        Ok(match init {
            Some(iops) => Script::with_init(entry, ops, iops),
            None => Script::new(entry, ops),
        })
    }

    /// `break`/`continue` 后的可选标签（后随标识符即视为标签）。
    /// 赋值操作符（S6.28）：消费当前 token —— `=` -> `None`（纯赋值）；
    /// 十种复合 `OP=` -> `Some(二元 Op)`。不认识则报错（在赋值目标之后，
    /// 错误信息指明"期望赋值"）。
    fn assign_op(&mut self) -> Result<Option<Op>, ParseError> {
        let op = match &self.peek().tok {
            Tok::Sym('=') => None,
            Tok::Sym2(s) => match s.as_str() {
                "+=" => Some(Op::Add),
                "-=" => Some(Op::Sub),
                "*=" => Some(Op::Mul),
                "/=" => Some(Op::Div),
                "%=" => Some(Op::Mod),
                "&=" => Some(Op::BitAnd),
                "|=" => Some(Op::BitOr),
                "^=" => Some(Op::BitXor),
                "<<=" => Some(Op::Shl),
                ">>=" => Some(Op::Shr),
                _ => return Err(self.err_here("期望 `=` 赋值或复合赋值")),
            },
            _ => return Err(self.err_here("期望 `=` 赋值或复合赋值")),
        };
        self.pos += 1;
        Ok(op)
    }

    /// ++/-- 语句的目标发射（S6.29）。名字**已消费**；目标 = 局部或
    /// `name.member`（this 须带成员）。恒等脱糖 `target += 1` / `-= 1`：
    /// **语句位无值产生**，前缀后缀语义等价 —— C 的求值序坑在本语言不存在。
    fn incdec_target(
        &mut self,
        ops: &mut Vec<Op>,
        name: String,
        op: Op,
    ) -> Result<(), ParseError> {
        if name == "this" && !matches!(self.peek().tok, Tok::Sym('.')) {
            return Err(self.err_here("this 的 ++/-- 需要成员"));
        }
        if matches!(self.peek().tok, Tok::Sym('.')) {
            self.pos += 1;
            let member = match self.next().tok {
                Tok::Ident(m) => m,
                _ => return Err(self.err_here("期望属性名或 `pos`")),
            };
            self.emit_member_incdec(ops, &name, &member, op);
        } else {
            ops.push(Op::Local(name.clone()));
            ops.push(Op::Const(Value::I64(1)));
            ops.push(op);
            ops.push(Op::SetLocal(name));
        }
        Ok(())
    }

    /// 成员 ++/-- 发射：读侧双压 + Get + Const(1) + op + Set
    ///（与 S6.28 复合成员赋值同构 —— Set 弹值在先，节点必须在值下）。
    fn emit_member_incdec(
        &self,
        ops: &mut Vec<Op>,
        name: &str,
        member: &str,
        op: Op,
    ) {
        for _ in 0..2 {
            if name == "this" {
                ops.push(Op::This);
            } else {
                ops.push(Op::NodeByName(name.to_string()));
            }
        }
        if member == "pos" {
            ops.push(Op::GetT);
        } else {
            ops.push(Op::GetProp(member.to_string()));
        }
        ops.push(Op::Const(Value::I64(1)));
        ops.push(op);
        if member == "pos" {
            ops.push(Op::SetT);
        } else {
            ops.push(Op::SetProp(member.to_string()));
        }
    }

    fn opt_label(&mut self) -> Option<String> {
        match self.peek().tok.clone() {
            Tok::Ident(s) if !RESERVED.contains(&s.as_str()) => {
                self.pos += 1;
                Some(s)
            }
            _ => None,
        }
    }

    /// 按标签解析目标循环层下标：`None` 标签 -> 最内层；`Some(l)` ->
    /// 由内向外（rposition）找**同名标签**层（无标签层不参与匹配）。
    fn loop_by_label(
        &self,
        label: &Option<String>,
        what: &str,
    ) -> Result<usize, ParseError> {
        match label {
            None => {
                if self.loops.is_empty() {
                    Err(self.err_here(format!("{what} 在循环外")))
                } else {
                    Ok(self.loops.len() - 1)
                }
            }
            Some(l) => match self.loops.iter().rposition(|c| c.label.as_deref() == Some(l.as_str()))
            {
                Some(i) => Ok(i),
                None => Err(self.err_here(format!("{what} 未找到标签 `{l}`"))),
            },
        }
    }

    /// for 区间迭代体（S6.24）：`for i in a..b { ... }` —— **纯糖**脱糖为
    /// while 形态，界是**活值**（每次迭代重求值，与手写完全一致）。
    ///
    /// 布局用**循环旋转**（continue 安全的关键）：
    /// ```text
    /// init:  a; SetLocal(i)
    ///        Jump(cond)            // 首轮不增量，直达条件
    /// inc:   i = i + 1             // continue 目标（LoopCtx.top 指此）
    /// cond:  i < b                 // b 的代码内联 —— 活界
    ///        JumpIfNot(end)
    /// body
    ///        Jump(inc)
    /// end:
    /// ```
    /// continue 落在增量上（不吃增量——否则死循环，经典脱糖坑）；break 到 end。
    fn for_body(
        &mut self,
        ops: &mut Vec<Op>,
        label: Option<String>,
    ) -> Result<(), ParseError> {
        // 已消费 for。循环变量（非保留字标识符）。
        let var = match self.next().tok {
            Tok::Ident(v) if !RESERVED.contains(&v.as_str()) => v,
            _ => return Err(self.err_here("for 需要循环变量名")),
        };
        match self.next().tok {
            Tok::Ident(k) if k == "in" => {}
            _ => return Err(self.err_here("期望 `in`")),
        }
        let mut a_code = Vec::new();
        self.expr(&mut a_code)?; // 下界
        // 区间形态：`..`（右开）或 `..=`（右闭，S6.25）。
        let inclusive = match &self.peek().tok {
            Tok::Sym2(s) if s == ".." => {
                self.pos += 1;
                false
            }
            Tok::Sym2(s) if s == "..=" => {
                self.pos += 1;
                true
            }
            _ => return Err(self.err_here("期望 `..` 或 `..=` 区间")),
        };
        let mut b_code = Vec::new();
        self.expr(&mut b_code)?; // 上界（活值：内联进条件）
        // 可选步进：`step expr`（S6.25）。缺省 Const(1)。
        // 字面量符号在**编译期**定向（升/降/零迭代）；一般表达式编译为
        // 运行时方向条件（方向随每次迭代的活值符号）。
        let step_code: Vec<Op> = if matches!(&self.peek().tok, Tok::Ident(k) if k == "step") {
            self.pos += 1;
            let mut c = Vec::new();
            self.expr(&mut c)?;
            c
        } else {
            vec![Op::Const(Value::I64(1))]
        };

        // init
        ops.extend(a_code);
        ops.push(Op::SetLocal(var.clone()));
        let jinit = ops.len();
        ops.push(Op::Jump(0)); // 占位 -> cond（下方回填）
        // inc（continue 目标）：i = i + step（活步进）
        let inc = ops.len();
        ops.push(Op::Local(var.clone()));
        ops.extend(step_code.clone());
        ops.push(Op::Add);
        ops.push(Op::SetLocal(var.clone()));
        // cond：按步进符号定向。
        let cond = ops.len();
        ops[jinit] = Op::Jump(cond);
        // Rust 的 f64::signum(+0.0) == 1.0（IEEE 正号惯例）—— 显式三分：
        // 正 1 / 负 -1 / 零 0（零 -> 恒假零次，见下方 Some(_) 臂）。
        let sign3 = |x: f64| if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 };
        let lit_sign = match step_code.as_slice() {
            [Op::Const(Value::I64(v))] => Some(sign3(*v as f64)),
            [Op::Const(Value::F32(v))] => Some(sign3(*v as f64)),
            _ => None,
        };
        match lit_sign {
            // 编译期定向：升序 i<b / i<=b；降序 i>b / i>=b；零 -> 恒假（零次）。
            Some(s) if s > 0.0 => {
                if inclusive {
                    ops.extend(b_code);
                    ops.push(Op::Local(var.clone()));
                    ops.push(Op::Lt); // b < i
                    ops.push(Op::Not); // !(b<i) = i<=b
                } else {
                    ops.push(Op::Local(var.clone()));
                    ops.extend(b_code);
                    ops.push(Op::Lt); // i < b
                }
            }
            Some(s) if s < 0.0 => {
                if inclusive {
                    ops.push(Op::Local(var.clone()));
                    ops.extend(b_code);
                    ops.push(Op::Lt); // i < b
                    ops.push(Op::Not); // !(i<b) = i>=b
                } else {
                    ops.extend(b_code);
                    ops.push(Op::Local(var.clone()));
                    ops.push(Op::Lt); // b < i = i>b
                }
            }
            Some(_) => {
                ops.push(Op::Const(Value::Bool(false))); // step 0：恒假零次
            }
            // 一般表达式（活方向）：
            //   开区间 (0<s && i<b) || (s<0 && b<i)
            //   闭区间 (0<s && i<=b) || (s<0 && i>=b)
            None => {
                // 0 < s
                ops.push(Op::Const(Value::I64(0)));
                ops.extend(step_code.clone());
                ops.push(Op::Lt);
                if inclusive {
                    ops.extend(b_code.clone());
                    ops.push(Op::Local(var.clone()));
                    ops.push(Op::Lt);
                    ops.push(Op::Not);
                } else {
                    ops.push(Op::Local(var.clone()));
                    ops.extend(b_code.clone());
                    ops.push(Op::Lt);
                }
                ops.push(Op::And);
                // s < 0
                ops.extend(step_code);
                ops.push(Op::Const(Value::I64(0)));
                ops.push(Op::Lt);
                if inclusive {
                    ops.push(Op::Local(var.clone()));
                    ops.extend(b_code);
                    ops.push(Op::Lt);
                    ops.push(Op::Not);
                } else {
                    ops.extend(b_code);
                    ops.push(Op::Local(var.clone()));
                    ops.push(Op::Lt);
                }
                ops.push(Op::And);
                ops.push(Op::Or);
            }
        }
        let jexit = ops.len();
        ops.push(Op::JumpIfNot(0)); // 占位 -> end
        // body（循环栈：continue 目标 = inc）
        self.expect_sym('{')?;
        self.loops.push(LoopCtx {
            top: inc,
            breaks: Vec::new(),
            label,
        });
        let body = self.stmts(ops);
        let ctx = self.loops.pop().expect("循环栈配对");
        body?;
        self.expect_sym('}')?;
        ops.push(Op::Jump(inc)); // 回到增量
        let end = ops.len();
        ops[jexit] = Op::JumpIfNot(end);
        for b in ctx.breaks {
            ops[b] = Op::Jump(end);
        }
        Ok(())
    }

    /// while 体（S6.22 抽取；S6.23 增标签参数）。
    fn while_body(
        &mut self,
        ops: &mut Vec<Op>,
        label: Option<String>,
    ) -> Result<(), ParseError> {
        let top = ops.len();
        self.expr(ops)?; // 条件（Bool）
        let jexit = ops.len();
        ops.push(Op::JumpIfNot(0)); // 占位，循环出口回填
        self.expect_sym('{')?;
        // 循环入栈：体内 break/continue 绑定此层（裸=最内层；带标签=同名层）。
        self.loops.push(LoopCtx {
            top,
            breaks: Vec::new(),
            label,
        });
        let body = self.stmts(ops);
        let ctx = self.loops.pop().expect("循环栈配对");
        body?;
        self.expect_sym('}')?;
        ops.push(Op::Jump(top)); // 回到条件
        let end = ops.len();
        ops[jexit] = Op::JumpIfNot(end);
        // break 占位统一回填到出口（含内层带标签登记的跨层占位）。
        for b in ctx.breaks {
            ops[b] = Op::Jump(end);
        }
        Ok(())
    }

    fn stmts(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        loop {
            self.skip_newlines();
            if matches!(self.peek().tok, Tok::Sym('}') | Tok::Eof) {
                return Ok(());
            }
            self.stmt(ops)?;
            match &self.peek().tok {
                Tok::Newline | Tok::Sym(';') | Tok::Sym('}') | Tok::Eof => {}
                // ++/-- 出现在语句尾部 = 表达式位滥用（`a = i++`）：指名而非误导。
                Tok::Sym2(s) if s == "++" || s == "--" => {
                    return Err(self.err_here("`{s}` 只能作独立语句（不产生值，无求值序）"))
                }
                _ => return Err(self.err_here("语句后期望换行、`;` 或 `}`")),
            }
        }
    }

    fn stmt(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        match self.peek().tok.clone() {
            Tok::Ident(k) if k == "if" => {
                self.pos += 1;
                self.expr(ops)?; // 条件（Bool）
                let jif = ops.len();
                ops.push(Op::JumpIfNot(0)); // 占位，假分支起点回填
                self.expect_sym('{')?;
                self.stmts(ops)?;
                self.expect_sym('}')?;
                if matches!(&self.peek().tok, Tok::Ident(k2) if k2 == "else") {
                    // else / else if（链式：else if 递归为 else 体里的 if 语句）。
                    self.pos += 1;
                    let jend = ops.len();
                    ops.push(Op::Jump(0)); // 真分支跳出，结尾回填
                    let else_start = ops.len();
                    ops[jif] = Op::JumpIfNot(else_start);
                    if matches!(&self.peek().tok, Tok::Ident(k2) if k2 == "if") {
                        self.stmt(ops)?; // else if —— 递归
                    } else {
                        self.expect_sym('{')?;
                        self.stmts(ops)?;
                        self.expect_sym('}')?;
                    }
                    let end = ops.len();
                    ops[jend] = Op::Jump(end);
                } else {
                    let end = ops.len();
                    ops[jif] = Op::JumpIfNot(end);
                }
                Ok(())
            }
            Tok::Ident(k) if k == "while" => {
                self.pos += 1;
                self.while_body(ops, None)
            }
            Tok::Ident(k) if k == "for" => {
                self.pos += 1;
                self.for_body(ops, None)
            }
            // 前缀 ++/-- 语句（S6.29）：`++x` / `--x` —— 语句位与后缀**等价**
            //（无值产生，求值序坑不存在）；目标与复合赋值同族。
            Tok::Sym2(s) if s == "++" || s == "--" => {
                self.pos += 1;
                let op = if s == "++" { Op::Add } else { Op::Sub };
                match self.peek().tok.clone() {
                    Tok::Ident(name) if !RESERVED.contains(&name.as_str()) => {
                        self.pos += 1;
                        self.incdec_target(ops, name, op)
                    }
                    _ => Err(self.err_here("++/-- 目标必须是变量或成员")),
                }
            }
            Tok::Ident(k) if k == "break" => {
                self.pos += 1;
                // 可选标签：`break`（最内层）/ `break name`（由内向外找标签）。
                let label = self.opt_label();
                let pos = self.loop_by_label(&label, "break")?;
                let target = &mut self.loops[pos];
                target.breaks.push(ops.len());
                ops.push(Op::Jump(0)); // 占位，目标循环收尾回填
                Ok(())
            }
            Tok::Ident(k) if k == "continue" => {
                self.pos += 1;
                let label = self.opt_label();
                let pos = self.loop_by_label(&label, "continue")?;
                let top = self.loops[pos].top;
                ops.push(Op::Jump(top)); // 目标循环顶，即时可知
                Ok(())
            }
            Tok::Ident(k) if k == "emit" => {
                self.pos += 1;
                let name = self.expect_str()?;
                self.expr(ops)?; // 载荷
                ops.push(Op::Emit(name));
                Ok(())
            }
            Tok::Ident(name) => {
                // 标签语句（S6.23/24）：`name: while/for ...`。
                if matches!(self.peek2().tok, Tok::Sym(':')) {
                    self.pos += 2; // name ':'
                    match &self.peek().tok {
                        Tok::Ident(k) if k == "while" => {
                            self.pos += 1;
                            self.while_body(ops, Some(name))
                        }
                        Tok::Ident(k) if k == "for" => {
                            self.pos += 1;
                            self.for_body(ops, Some(name))
                        }
                        _ => Err(self.err_here("标签只能用于 while/for")),
                    }
                } else {
                // 保留字里只有 `this` 可作成员赋值目标（this.pos = ...）；
                // 其余（arg = 1 之类）在裸局部路径拒绝。
                let reserved = RESERVED.contains(&name.as_str());
                if reserved && !(name == "this" && matches!(self.peek2().tok, Tok::Sym('.'))) {
                    return Err(self.err_here(format!("`{name}` 是保留字")));
                }
                self.pos += 1;
                if matches!(self.peek().tok, Tok::Sym('.')) {
                    // 节点成员赋值（S6.28 泛化到复合）：node.member OP= expr。
                    // 纯 `=`：节点先压 + expr + Set（值之下）。
                    // 复合 OP=：脱糖 读-算-写（节点压两次：一次给读消费、
                    // 一次留给写回收 —— 与手写双压同构）。
                    self.pos += 1;
                    let member = match self.next().tok {
                        Tok::Ident(m) => m,
                        _ => return Err(self.err_here("期望属性名或 `pos`")),
                    };
                    // 后缀 ++/--（S6.29）：`node.member++` —— 与复合同构（+/- 1）。
                    if let Tok::Sym2(s) = self.peek().tok.clone() {
                        if s == "++" || s == "--" {
                            self.pos += 1;
                            let op = if s == "++" { Op::Add } else { Op::Sub };
                            self.emit_member_incdec(ops, &name, &member, op);
                            return Ok(());
                        }
                    }
                    let bin = self.assign_op()?;
                    match bin {
                        Some(op) => {
                            // 读侧**双压**：一个节点给 GetT/GetProp 消费，
                            // 一个留在栈底给 Set 回收（Set 弹值在先 —— 值必须在顶，
                            // 写侧节点不能压在值后面 —— S6.28 调试实证）。
                            for _ in 0..2 {
                                if name == "this" {
                                    ops.push(Op::This);
                                } else {
                                    ops.push(Op::NodeByName(name.clone()));
                                }
                            }
                            if member == "pos" {
                                ops.push(Op::GetT);
                            } else {
                                ops.push(Op::GetProp(member.clone()));
                            }
                            // 算：读值 OP expr。
                            self.expr(ops)?;
                            ops.push(op);
                        }
                        None => {
                            // 纯赋值：节点先压 + expr（值压在节点上）。
                            if name == "this" {
                                ops.push(Op::This);
                            } else {
                                ops.push(Op::NodeByName(name));
                            }
                            self.expr(ops)?;
                        }
                    }
                    if member == "pos" {
                        ops.push(Op::SetT);
                    } else {
                        ops.push(Op::SetProp(member));
                    }
                    Ok(())
                } else if matches!(self.peek().tok, Tok::Sym('=')) {
                    self.pos += 1;
                    self.expr(ops)?;
                    ops.push(Op::SetLocal(name));
                    Ok(())
                } else if let Tok::Sym2(s) = self.peek().tok.clone() {
                    if s == "++" || s == "--" {
                        // 后缀 ++/--（S6.29）：`x++` —— 与 `x += 1` 同构。
                        self.pos += 1;
                        let op = if s == "++" { Op::Add } else { Op::Sub };
                        ops.push(Op::Local(name.clone()));
                        ops.push(Op::Const(Value::I64(1)));
                        ops.push(op);
                        ops.push(Op::SetLocal(name));
                        return Ok(());
                    }
                    // 复合局部赋值（S6.28）：x OP= e -> Local(x), e, OP, SetLocal(x)。
                    let bin = self.assign_op()?;
                    match bin {
                        Some(op) => {
                            ops.push(Op::Local(name.clone()));
                            self.expr(ops)?;
                            ops.push(op);
                            ops.push(Op::SetLocal(name));
                            Ok(())
                        }
                        None => Err(self.err_here("期望 `=` 赋值或复合赋值")),
                    }
                } else {
                    Err(self.err_here("期望 `=` 赋值"))
                }
                }
            }
            _ => Err(self.err_here("期望语句（赋值 / if / while / for / break / continue / emit）")),
        }
    }

    /// expr := or（逻辑或 <= 逻辑与 <= 比较 <= 加减 <= 乘 <= 一元，S6.21 全链）。
    fn expr(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.or(ops)
    }

    /// or := and ("||" and)*（按值 eager，见 Op::And 文档）。
    fn or(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.and(ops)?;
        while matches!(&self.peek().tok, Tok::Sym2(s) if s == "||") {
            self.pos += 1;
            self.and(ops)?;
            ops.push(Op::Or);
        }
        Ok(())
    }

    /// and := cmp ("&&" cmp)*（eager）。
    fn and(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.cmp(ops)?;
        while matches!(&self.peek().tok, Tok::Sym2(s) if s == "&&") {
            self.pos += 1;
            self.cmp(ops)?;
            ops.push(Op::And);
        }
        Ok(())
    }

    /// cmp := add (OP add)?（单级不可链）。五族里 `<`/`==` 原生；
    /// 其余**组合编译**（零新指令）：`>`＝[b,a,Lt]、`>=`＝[a,b,Lt,Not]、
    /// `<=`＝[b,a,Lt,Not]、`!=`＝[a,b,Eq,Not]。两操作数各编进**临时缓冲**
    /// 再按序拼回 —— 交换族不得 take 主缓冲（会把整个程序前缀卷走重排，
    /// S6.21 调试实证）。
    fn cmp(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        let mut a_code = Vec::new();
        self.bitor(&mut a_code)?; // a（源序，先入临时；经位阶梯到加减）
        let op = match self.peek().tok.clone() {
            Tok::Sym2(s) if s == "==" => Some("=="),
            Tok::Sym2(s) if s == "!=" => Some("!="),
            Tok::Sym2(s) if s == "<=" => Some("<="),
            Tok::Sym2(s) if s == ">=" => Some(">="),
            Tok::Sym('<') => Some("<"),
            Tok::Sym('>') => Some(">"),
            _ => None,
        };
        let Some(op) = op else {
            ops.extend(a_code);
            return Ok(());
        };
        self.pos += 1;
        let mut b_code = Vec::new();
        self.add(&mut b_code)?; // b
        match op {
            "==" => {
                ops.extend(a_code);
                ops.extend(b_code);
                ops.push(Op::Eq);
            }
            "<" => {
                ops.extend(a_code);
                ops.extend(b_code);
                ops.push(Op::Lt);
            }
            ">=" => {
                ops.extend(a_code);
                ops.extend(b_code);
                ops.push(Op::Lt);
                ops.push(Op::Not);
            }
            "!=" => {
                ops.extend(a_code);
                ops.extend(b_code);
                ops.push(Op::Eq);
                ops.push(Op::Not);
            }
            ">" => {
                ops.extend(b_code);
                ops.extend(a_code);
                ops.push(Op::Lt);
            }
            "<=" => {
                ops.extend(b_code);
                ops.extend(a_code);
                ops.push(Op::Lt);
                ops.push(Op::Not);
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn add(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.mul(ops)?;
        loop {
            match &self.peek().tok {
                Tok::Sym('+') => {
                    self.pos += 1;
                    self.mul(ops)?;
                    ops.push(Op::Add);
                }
                Tok::Sym('-') => {
                    self.pos += 1;
                    self.mul(ops)?;
                    ops.push(Op::Sub);
                }
                _ => return Ok(()),
            }
        }
    }

    /// mul := unary ("*" | "%") unary*（S6.26 增 `%`，与乘同级）。
    fn mul(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.primary(ops)?;
        loop {
            match &self.peek().tok {
                Tok::Sym('*') => {
                    self.pos += 1;
                    self.primary(ops)?;
                    ops.push(Op::Mul);
                }
                Tok::Sym('%') => {
                    self.pos += 1;
                    self.primary(ops)?;
                    ops.push(Op::Mod);
                }
                Tok::Sym('/') => {
                    self.pos += 1;
                    self.primary(ops)?;
                    ops.push(Op::Div);
                }
                _ => return Ok(()),
            }
        }
    }

    // ------------------------------------------------ 位阶梯（S6.26，C/Rust 序）
    // bitor <= bitxor <= bitand <= shift <= add —— 全左结合。

    /// bitor := bitxor ("|" bitxor)*。
    fn bitor(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.bitxor(ops)?;
        while matches!(self.peek().tok, Tok::Sym('|')) {
            self.pos += 1;
            self.bitxor(ops)?;
            ops.push(Op::BitOr);
        }
        Ok(())
    }

    /// bitxor := bitand ("^" bitand)*。
    fn bitxor(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.bitand(ops)?;
        while matches!(self.peek().tok, Tok::Sym('^')) {
            self.pos += 1;
            self.bitand(ops)?;
            ops.push(Op::BitXor);
        }
        Ok(())
    }

    /// bitand := shift ("&" shift)*。
    fn bitand(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.shift(ops)?;
        while matches!(self.peek().tok, Tok::Sym('&')) {
            self.pos += 1;
            self.shift(ops)?;
            ops.push(Op::BitAnd);
        }
        Ok(())
    }

    /// shift := add (("<<" | ">>") add)*。
    fn shift(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.add(ops)?;
        loop {
            match &self.peek().tok {
                Tok::Sym2(s) if s == "<<" => {
                    self.pos += 1;
                    self.add(ops)?;
                    ops.push(Op::Shl);
                }
                Tok::Sym2(s) if s == ">>" => {
                    self.pos += 1;
                    self.add(ops)?;
                    ops.push(Op::Shr);
                }
                _ => return Ok(()),
            }
        }
    }

    /// primary := unary（负号/非）→ 基元 → 后缀索引 `s[i]`*（S6.30）。
    /// 索引比一元绑定更紧（C 序：`-s[0]` 是 `-(s[0])`）—— 后缀环在基元
    /// 产出后立即应用。
    fn primary(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.unary(ops)?;
        // 后缀索引环：`expr [ expr ]` -> StrIndex（仅 Str 运行时校验）。
        while matches!(self.peek().tok, Tok::Sym('[')) {
            self.pos += 1;
            self.expr(ops)?; // 下标（完整表达式级）
            self.expect_sym(']')?;
            ops.push(Op::StrIndex);
        }
        Ok(())
    }

    /// `node.pos` 之后的 `.x`/`.y` 分量读（S7.4）：GetT 产物是 Vec2，
    /// 追加 GetX/GetY。仅这一处形态支持（`(expr).x` 不支持 —— 编译器
    /// 无类型推理，静默不支持不如不支持）。
    fn pos_component(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        if matches!(self.peek().tok, Tok::Sym('.')) {
            if let Some(t) = self.toks.get(self.pos + 1) {
                if let Tok::Ident(m) = &t.tok {
                    match m.as_str() {
                        "x" => {
                            self.pos += 2;
                            ops.push(Op::GetX);
                        }
                        "y" => {
                            self.pos += 2;
                            ops.push(Op::GetY);
                        }
                        _ => return Err(self.err_here("pos 后只支持 .x / .y")),
                    }
                }
            }
        }
        Ok(())
    }

    fn unary(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        // 一元负号（最小集）：`-x` -> `0 - x`（先垫 0 再解析操作数，Sub 弹序恰好）。
        if matches!(self.peek().tok, Tok::Sym('-')) {
            self.pos += 1;
            ops.push(Op::Const(Value::I64(0)));
            self.primary(ops)?;
            ops.push(Op::Sub);
            return Ok(());
        }
        // 一元非：`!x` -> [x, Not]。
        if matches!(self.peek().tok, Tok::Sym('!')) {
            self.pos += 1;
            self.primary(ops)?;
            ops.push(Op::Not);
            return Ok(());
        }
        // 内建调用（S6.30 起）：`ident (` —— `len`（长度）、`key`（输入
        // 探针，S7.2）、`xy(e1, e2)`（任意表达式构造 Vec2，S7.4）。
        // 后随括号消歧，不占保留字（局部名不带括号照常是局部）。
        // 未知内建如实报错。
        if let Tok::Ident(name) = self.peek().tok.clone() {
            if name == "len" && matches!(self.peek2().tok, Tok::Sym('(')) {
                self.pos += 2; // len (
                self.expr(ops)?;
                self.expect_sym(')')?;
                ops.push(Op::StrLen);
                return Ok(());
            }
            if name == "key" && matches!(self.peek2().tok, Tok::Sym('(')) {
                self.pos += 2; // key (
                self.expr(ops)?;
                self.expect_sym(')')?;
                ops.push(Op::Key);
                return Ok(());
            }
            if name == "num_to_str" && matches!(self.peek2().tok, Tok::Sym('(')) {
                self.pos += 2; // num_to_str (
                self.expr(ops)?;
                self.expect_sym(')')?;
                ops.push(Op::NumStr);
                return Ok(());
            }
            if name == "xy" && matches!(self.peek2().tok, Tok::Sym('(')) {
                self.pos += 2; // xy (
                self.expr(ops)?;
                self.expect_sym(',')?;
                self.expr(ops)?;
                self.expect_sym(')')?;
                ops.push(Op::Pack);
                return Ok(());
            }
        }
        let sp = self.peek().clone();
        match self.next().tok.clone() {
            Tok::Num(v, true) => ops.push(Op::Const(Value::I64(v as i64))),
            Tok::Num(v, false) => ops.push(Op::Const(Value::F32(v as f32))),
            Tok::Str(s) => ops.push(Op::Const(Value::Str(s))),
            Tok::Ident(k) => match k.as_str() {
                "true" => ops.push(Op::Const(Value::Bool(true))),
                "false" => ops.push(Op::Const(Value::Bool(false))),
                "arg" => ops.push(Op::Arg),
                // `this` / `this.pos`：后随 `.` 走成员访问（节点压栈换成 This）。
                "this" => {
                    if matches!(self.peek().tok, Tok::Sym('.')) {
                        self.pos += 1;
                        let member = match self.next().tok {
                            Tok::Ident(m) => m,
                            _ => return Err(self.err_here("期望属性名或 `pos`")),
                        };
                        ops.push(Op::This);
                        if member == "pos" {
                            ops.push(Op::GetT);
                            self.pos_component(ops)?;
                        } else {
                            ops.push(Op::GetProp(member));
                        }
                    } else {
                        ops.push(Op::This);
                    }
                }
                other => {
                    if RESERVED.contains(&other) {
                        return Err(ParseError::new(sp.line, sp.col, format!("`{other}` 是保留字")));
                    }
                    if matches!(self.peek().tok, Tok::Sym('.')) {
                        self.pos += 1;
                        let member = match self.next().tok {
                            Tok::Ident(m) => m,
                            _ => return Err(self.err_here("期望属性名或 `pos`")),
                        };
                        ops.push(Op::NodeByName(other.to_string()));
                        if member == "pos" {
                            ops.push(Op::GetT);
                            self.pos_component(ops)?;
                        } else {
                            ops.push(Op::GetProp(member));
                        }
                    } else {
                        ops.push(Op::Local(other.to_string()));
                    }
                }
            },
            Tok::Sym('(') => {
                // Vec2 字面量（数字字面量，槽位可带负号）或括号分组。
                let num_ahead = |p: &Self, off: usize| -> Option<f64> {
                    let t = &p.toks.get(p.pos + off)?.tok;
                    match t {
                        Tok::Num(v, _) => Some(*v),
                        _ => None,
                    }
                };
                if let Some(x) = num_ahead(self, 0) {
                    if matches!(self.peek2().tok, Tok::Sym(',')) {
                        self.pos += 2;
                        let y = if let Some(y) = num_ahead(self, 0) {
                            self.pos += 1;
                            y
                        } else if matches!(self.peek().tok, Tok::Sym('-')) {
                            match num_ahead(self, 1) {
                                Some(y) => {
                                    self.pos += 2;
                                    -y
                                }
                                None => return Err(self.err_here("Vec2 字面量第二位须是数字")),
                            }
                        } else {
                            return Err(self.err_here("Vec2 字面量第二位须是数字"));
                        };
                        self.expect_sym(')')?;
                        ops.push(Op::Const(Value::Vec2(Vec2::new(x as f32, y as f32))));
                        return Ok(());
                    }
                }
                self.expr(ops)?;
                self.expect_sym(')')?;
            }
            _ => return Err(self.err_here("期望表达式")),
        }
        Ok(())
    }
}

impl ScriptVm {
    /// 登记（编译文本脚本）：语法错误的行/列经 [`ParseError`] 如实上报。
    pub fn register_text(&mut self, key: &str, src: &str) -> Result<(), ParseError> {
        let script = compile_script(src)?;
        self.register(key, script);
        Ok(())
    }
}