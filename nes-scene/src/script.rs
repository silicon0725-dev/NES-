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
    /// 弹 b、a（均须 Bool），压 a && b（**按值 eager** —— 表达式层无副作用，
    /// 短路无可观测收益，见 S6.21 文档 §2.1）。
    And,
    /// 弹 b、a（均须 Bool），压 a || b（eager，同上）。
    Or,
    /// 弹 Bool，压非。
    Not,
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
                stack.push(StackVal::V(match (a, b) {
                    (Value::I64(x), Value::I64(y)) => Value::I64(x * y),
                    (a, b) => match (num_of(&a), num_of(&b)) {
                        (Some(x), Some(y)) => Value::F32(x * y),
                        _ => halt!("Mul 类型不符"),
                    },
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
                let v: f64 = text.parse().map_err(|_| ParseError::new(line, col, "非法数字"))?;
                out.push(Spanned {
                    tok: Tok::Num(v, is_int),
                    line,
                    col,
                });
                col += i - start;
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
            // 双字符符号族（S6.21 扩到 == <= >= != && ||）。
            _ if i + 2 < n && (c, chars[i + 1], chars[i + 2]) == ('.', '.', '=') => {
                out.push(Spanned {
                    tok: Tok::Sym2("..=".into()),
                    line,
                    col,
                });
                i += 3;
                col += 3;
            }
            _ if i + 1 < n
                && matches!(
                    (c, chars[i + 1]),
                    ('=', '=') | ('<', '=') | ('>', '=') | ('!', '=') | ('&', '&') | ('|', '|')
                        | ('.', '.')
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
            '{' | '}' | '(' | ')' | ',' | '.' | ':' | '=' | '+' | '-' | '*' | '<' | '>' | '!' | ';' => {
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

    /// script := ("on" STRING | "every") "{" stmts "}"
    fn script(&mut self) -> Result<Script, ParseError> {
        self.skip_newlines();
        let entry = match self.next().tok {
            Tok::Ident(k) if k == "on" => ScriptEntry::Signal(self.expect_str()?),
            Tok::Ident(k) if k == "every" => ScriptEntry::Process,
            _ => return Err(self.err_here("期望 `on \"信号名\"` 或 `every`")),
        };
        self.expect_sym('{')?;
        let mut ops = Vec::new();
        self.stmts(&mut ops)?;
        self.expect_sym('}')?;
        self.skip_newlines();
        if !matches!(self.peek().tok, Tok::Eof) {
            return Err(self.err_here("脚本结尾后有多余内容"));
        }
        Ok(Script::new(entry, ops))
    }

    /// `break`/`continue` 后的可选标签（后随标识符即视为标签）。
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
                    // 节点成员赋值：node.member = expr（member 为 pos -> SetT）。
                    // 栈序：SetT/SetProp 弹值再弹节点 —— 节点必须**先压**（值之下）。
                    self.pos += 1;
                    let member = match self.next().tok {
                        Tok::Ident(m) => m,
                        _ => return Err(self.err_here("期望属性名或 `pos`")),
                    };
                    self.expect_sym('=')?;
                    if name == "this" {
                        ops.push(Op::This);
                    } else {
                        ops.push(Op::NodeByName(name));
                    }
                    self.expr(ops)?; // 值压在节点上
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
        self.add(&mut a_code)?; // a（源序，先入临时）
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

    fn mul(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
        self.primary(ops)?;
        while matches!(self.peek().tok, Tok::Sym('*')) {
            self.pos += 1;
            self.primary(ops)?;
            ops.push(Op::Mul);
        }
        Ok(())
    }

    fn primary(&mut self, ops: &mut Vec<Op>) -> Result<(), ParseError> {
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