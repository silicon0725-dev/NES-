//! 场景序列化 / 实例化 / 打包（RON 文本）。
//!
//! # 为什么是 RON
//!
//! 场景文件必须**人能读、人能改**（零基础用户要能在编辑器之外手改），
//! 同时必须**无歧义、可 diff**。RON 的 `Ident(...)` 结构天然带类型标签，
//! 比 JSON 少一层"值类型靠猜"的麻烦，也不需要引入任何依赖。
//!
//! # 文本形态
//!
//! ```ron
//! // nes-scene packed v1 · 2 nodes
//! Scene(
//!     version: 1,
//!     resources: [
//!         Res(id: 7, path: "Textures/hero.png", kind: "Texture"),
//!     ],
//!     root: Node(
//!         name: "Root",
//!         kind: "Node2D",
//!         props: {
//!             "visible": Bool(true),
//!             "z_index": I64(0),
//!         },
//!         children: [
//!             Node(
//!                 name: "Ghost",
//!                 kind: "Sprite2D",
//!                 props: { "texture": Resource(7), },
//!                 children: [],
//!             ),
//!         ],
//!     ),
//! )
//! ```
//!
//! `resources` 段是 M3 加的自解释层：属性里的 `Resource(7)` 只是一个**场景内
//! 槽位号**，它指向哪个文件由这一段声明（见 [`crate::resources`]）。
//! 旧场景没有这一段照样能读 —— 那些号会先以"悬垂引用"的形式留在资源表里，
//! 由编辑器补声明，而不是报错或丢数据。空资源表**不写出**该字段，
//! 因此 M2 时代产出的场景文件在本期逐字节不变。
//!
//! # 三档宽容度（沿用 M1 的"宽容输入、严格输出"）
//!
//! - **输出严格**：字段顺序固定、缩进固定、属性按 schema 顺序 —— 同一棵树
//!   序列化两次必然逐字节相同，因此可以直接进版本控制。
//! - **输入宽容**：允许注释、尾逗号、字段乱序、缺省 `props`/`children`、
//!   裸数字/裸字符串字面量、`kind` 用裸标识符。
//! - **前向兼容**：schema 里没有的属性**原样保留**，不报错也不丢弃 ——
//!   这样高版本引擎产出的场景在低版本里打开不会把扩展属性吃掉。
//!
//! # 与热重载的关系
//!
//! 反序列化不要求"文件版本 == 引擎版本"，只要求"不高于"。热重载时属性契约不变、
//! 仅 `PropStore::version` 递增（见 [`crate::props`]）。

use std::fmt;
use std::path::Path;

use crate::identity::NodeId;
use crate::node::NodeKindTag;
use crate::props::PropStore;
use crate::resources::{asset_kind_of_hint, AdoptReport, ResId, ResourceTable};
use crate::schema::NodeSchema;
use crate::transform::Vec2;
use crate::transform::Transform2D;
use crate::tree::{ProcessMode, SceneTree};
use crate::value::Value;

/// 当前格式版本。**只增不改**：字段语义变化时递增，解析器要能读旧版本。
pub const FORMAT_VERSION: u32 = 1;

/// 打包选项。
#[derive(Clone, Debug, PartialEq)]
pub struct PackOptions {
    /// 省略等于 schema 默认值的属性。反序列化时默认值会由 schema 补回，
    /// 因此省略不改变语义，只让文件更短、diff 更干净。
    pub omit_defaults: bool,
    /// 每层缩进空格数。
    pub indent: usize,
    /// 是否写文件头注释（版本与节点数）。
    pub header: bool,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            omit_defaults: false,
            indent: 4,
            header: true,
        }
    }
}

impl PackOptions {
    /// 全量写出（含默认值），便于人工核对。
    pub fn verbose() -> Self {
        Self::default()
    }

    /// 省略默认值，便于进版本控制。
    pub fn compact() -> Self {
        Self {
            omit_defaults: true,
            ..Self::default()
        }
    }
}

/// 解析或实例化失败。
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    /// 行号（1 起）。0 表示与位置无关的语义错误。
    pub line: usize,
    /// 列号（1 起）。
    pub col: usize,
    /// 说明。
    pub message: String,
}

impl ParseError {
    /// 带位置的语法错误。
    pub fn new(line: usize, col: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            col,
            message: message.into(),
        }
    }

    /// 与位置无关的语义错误（版本不符、属性类型不符等）。
    pub fn semantic(message: impl Into<String>) -> Self {
        Self {
            line: 0,
            col: 0,
            message: message.into(),
        }
    }

    /// 是否是语义错误（无位置信息）。
    pub fn is_semantic(&self) -> bool {
        self.line == 0
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_semantic() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "第 {} 行第 {} 列：{}", self.line, self.col, self.message)
        }
    }
}

impl std::error::Error for ParseError {}

/// 文档根。
#[derive(Clone, Debug, PartialEq)]
pub struct SceneDoc {
    /// 格式版本。
    pub version: u32,
    /// 资源声明表（M3）。**空表不写出**，因此 M2 的存量场景逐字节不变。
    ///
    /// 属性里的 `Resource(n)` 里的 `n` 是 [`ResId`](crate::resources::ResId)，
    /// 本节把 `n` 解析成 `(路径, 类别)`：`n` 在一个场景内稳定，路径在一个项目内稳定。
    pub resources: Vec<ResourceDoc>,
    /// 根节点。
    pub root: NodeDoc,
}

/// 文档里的一条资源声明。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResourceDoc {
    /// 场景内槽位号（对应属性里的 `Resource(n)`）。
    pub id: u32,
    /// 资源路径（相对资源根，见 [`AssetPath`](nes_asset::AssetPath)）。
    pub path: String,
    /// 资源类别稳定名（`Texture` / `Audio` / `Scene`……）。
    pub kind: String,
}

impl ResourceDoc {
    /// 类别已解析。
    pub fn asset_kind(&self) -> Option<nes_asset::AssetKind> {
        asset_kind_of_hint(&self.kind)
    }
}

/// 文档中的一个节点。
#[derive(Clone, Debug, PartialEq)]
pub struct NodeDoc {
    /// 名字。
    pub name: String,
    /// 类型标签。
    pub kind: NodeKindTag,
    /// 本地变换。**不写出即等于单位变换**（`compact` 模式下省略）。
    ///
    /// 变换刻意不进属性表：它是 `NodeData` 的固有一等字段，属于空间数据；
    /// 属性表装的是设计时数据。两者序列化必须对称 —— 漏掉变换的话，
    /// 打包出来的场景里所有节点都会叠在原点。
    pub local: Transform2D,
    /// 处理模式（调度数据，与 `local` 同一裁决：一等字段不进属性表）。
    /// **不写出即等于 `Inherit`**（`compact` 模式下省略），存量文件不变。
    pub process_mode: ProcessMode,
    /// 实例级覆盖记录（仅当本节点是 `sub_scene` 包装节点时有意义）。
    /// **不写出即等于无覆盖**；实例化时应用到展开子树（S6.8）。
    pub overrides: Vec<InstanceOverride>,
    /// 属性（顺序由写入侧决定，读取侧不依赖顺序）。
    pub props: Vec<(String, Value)>,
    /// 子节点，顺序即场景顺序。
    pub children: Vec<NodeDoc>,
}

/// 一条实例级覆盖：对子场景内某节点（按路径寻址）的覆盖 —— 字段级
///（local / process_mode / props）与**结构性**（add / remove）。
///
/// `None` / `false` / 空 = 不覆盖（跟随子场景文件）。路径相对**子场景根**
///（即展开后包装节点的子节点）：`""` = 根自身，`"sprite"` = 根的直接子节点，
/// `"a/b"` = 逐段下钻（段名按子节点名匹配）。
///
/// 定义在 scene_io（而非 tree）是因为 `add` 装的是 [`NodeDoc`] —— 它本质是
/// 序列化形态的覆盖；`NodeData.overrides` 持有它作为文件事实的运行时镜像。
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceOverride {
    /// 目标节点相对子场景根的路径。
    pub path: String,
    /// 覆盖本地变换。
    pub local: Option<Transform2D>,
    /// 覆盖处理模式。
    pub process_mode: Option<ProcessMode>,
    /// 覆盖属性（名字 -> 值；`Resource(n)` 是**父场景**槽位号）。
    pub props: Vec<(String, Value)>,
    /// 结构性覆盖：在该节点下**追加**的子树（父场景文件拥有的节点，
    /// 含其属性/变换/后代）。与 `remove` 互斥（同一记录不得同时出现）。
    pub add: Vec<NodeDoc>,
    /// 结构性覆盖：从实例中**移除**该节点（整棵子树）。
    pub remove: bool,
}

impl NodeDoc {
    /// 节点总数（含自身）。
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(NodeDoc::count).sum::<usize>()
    }
}

// ---------- 写出 ----------

/// 树 → 文档（全量，不过滤默认值）。资源表为空（属性里的引用退化为裸槽位号）。
pub fn to_doc(tree: &SceneTree) -> SceneDoc {
    to_doc_with_resources(tree, &ResourceTable::new(), false)
}

/// 树 → 文档，且省略等于 schema 默认值的属性。
pub fn to_doc_omitting_defaults(tree: &SceneTree) -> SceneDoc {
    to_doc_with_resources(tree, &ResourceTable::new(), true)
}

/// 树 + 资源表 → 文档。表里已声明的槽位写进 `resources` 段，
/// 悬垂槽位（只被属性引用、没声明）不写 —— 它们本来就是编辑器要修的缺口。
pub fn to_doc_with_resources(
    tree: &SceneTree,
    table: &ResourceTable,
    omit_defaults: bool,
) -> SceneDoc {
    SceneDoc {
        version: FORMAT_VERSION,
        resources: table
            .iter()
            .filter_map(|e| match (e.path(), e.kind()) {
                (Some(p), Some(k)) => Some(ResourceDoc {
                    id: e.id().get(),
                    path: p.as_str().to_string(),
                    kind: k.as_str().to_string(),
                }),
                _ => None,
            })
            .collect(),
        root: node_to_doc(tree, tree.root(), omit_defaults),
    }
}

fn node_to_doc(tree: &SceneTree, id: NodeId, omit_defaults: bool) -> NodeDoc {
    let schema = NodeSchema::of(tree.kind_tag(id).unwrap_or(NodeKindTag::Node));
    let empty = PropStore::new();
    let store = tree.props(id).unwrap_or(&empty);

    let mut props: Vec<(String, Value)> = Vec::new();
    // 1. schema 顺序，保证输出确定性，也保证属性面板顺序与文件顺序一致。
    for desc in schema.props() {
        if let Some(v) = store.get(desc.name()) {
            if omit_defaults && desc.is_default(v) {
                continue;
            }
            props.push((desc.name().to_string(), v.clone()));
        }
    }
    // 2. 扩展属性（本引擎版本不认识的），按名字升序追加，不丢。
    for (name, v) in store.iter() {
        if schema.prop(name).is_none() {
            props.push((name.to_string(), v.clone()));
        }
    }

    NodeDoc {
        name: tree.name(id).unwrap_or("").to_string(),
        kind: tree.kind_tag(id).unwrap_or(NodeKindTag::Node),
        local: tree.local(id).unwrap_or(Transform2D::IDENTITY),
        process_mode: tree.process_mode(id).unwrap_or_default(),
        overrides: tree.instance_overrides(id).unwrap_or_default().to_vec(),
        props,
        // 子场景边界（草案 §10）：绑定了 `sub_scene` 引用的节点，其子树是
        // 加载时展开出来的**派生内容**，归子场景文件所有 —— 回写只留引用，
        // 否则编辑器无法把改动写回子场景文件。未绑定的引用（Resource(0)）
        // 视为已摘除，子树是本地内容，照常写出。
        children: if bound_sub_scene(store).is_some() {
            Vec::new()
        } else {
            tree.children(id)
                .iter()
                .map(|c| node_to_doc(tree, *c, omit_defaults))
                .collect()
        },
    }
}

/// 节点上已绑定的子场景槽位号（`sub_scene: Resource(n)`，n != 0）。
fn bound_sub_scene(store: &PropStore) -> Option<u64> {
    match store.get(PROP_SUB_SCENE) {
        Some(Value::Resource(n)) if *n != 0 => Some(*n),
        _ => None,
    }
}

/// 树 → RON 文本。
pub fn write_ron(tree: &SceneTree, opts: &PackOptions) -> String {
    let doc = if opts.omit_defaults {
        to_doc_omitting_defaults(tree)
    } else {
        to_doc(tree)
    };
    doc_to_ron(&doc, opts)
}

/// 树 + 资源表 → RON 文本。资源声明随场景一起写盘，
/// 因此同一个场景文件读回来仍指向同一批资源。
pub fn write_ron_with_resources(
    tree: &SceneTree,
    table: &ResourceTable,
    opts: &PackOptions,
) -> String {
    let doc = to_doc_with_resources(tree, table, opts.omit_defaults);
    doc_to_ron(&doc, opts)
}

/// 文档 → RON 文本。同一文档 + 同一选项 → 逐字节相同。
pub fn doc_to_ron(doc: &SceneDoc, opts: &PackOptions) -> String {
    let mut out = String::new();
    if opts.header {
        out.push_str(&format!(
            "// nes-scene packed v{} · {} nodes\n",
            doc.version,
            doc.root.count()
        ));
    }
    let ind1 = " ".repeat(opts.indent);
    let ind2 = " ".repeat(opts.indent * 2);
    out.push_str("Scene(\n");
    out.push_str(&format!("{ind1}version: {},\n", doc.version));
    if !doc.resources.is_empty() {
        out.push_str(&format!("{ind1}resources: [\n"));
        for r in &doc.resources {
            out.push_str(&format!(
                "{ind2}Res(id: {}, path: {}, kind: {}),\n",
                r.id,
                quote(&r.path),
                quote(&r.kind)
            ));
        }
        out.push_str(&format!("{ind1}],\n"));
    }
    out.push_str(&format!("{ind1}root: "));
    write_node(&mut out, &doc.root, opts, 1);
    out.push_str(",\n)\n");
    out
}

fn write_node(out: &mut String, doc: &NodeDoc, opts: &PackOptions, level: usize) {
    let ind = " ".repeat(level * opts.indent);
    let ind1 = " ".repeat((level + 1) * opts.indent);
    let ind2 = " ".repeat((level + 2) * opts.indent);

    out.push_str("Node(\n");
    out.push_str(&format!("{ind1}name: {},\n", quote(&doc.name)));
    out.push_str(&format!("{ind1}kind: {},\n", quote(doc.kind.as_str())));
    // 变换只在非单位（或全量模式）时写出：缺省即单位，语义不丢。
    if !opts.omit_defaults || doc.local != Transform2D::IDENTITY {
        out.push_str(&format!("{ind1}local: {},\n", transform_literal(doc.local)));
    }
    // 处理模式只在非 Inherit（或全量模式）时写出：缺省即继承，语义不丢。
    if !opts.omit_defaults || doc.process_mode != ProcessMode::Inherit {
        out.push_str(&format!(
            "{ind1}process_mode: {},\n",
            quote(doc.process_mode.as_str())
        ));
    }
    // 实例级覆盖只在非空时写出（不写出即无覆盖，存量文件不变）。
    // 记录内只写被覆盖的字段 —— 与解析侧"缺 = 不覆盖"对称。
    if !doc.overrides.is_empty() {
        let ind3 = " ".repeat((level + 3) * opts.indent);
        out.push_str(&format!("{ind1}overrides: [\n"));
        for ov in &doc.overrides {
            let mut rec = format!("{}Override(path: {}", ind2, quote(&ov.path));
            if let Some(t) = ov.local {
                rec.push_str(&format!(", local: {}", transform_literal(t)));
            }
            if let Some(m) = ov.process_mode {
                rec.push_str(&format!(", process_mode: {}", quote(m.as_str())));
            }
            if !ov.props.is_empty() {
                rec.push_str(", props: {");
                for (k, v) in &ov.props {
                    rec.push_str(&format!("{}: {}, ", quote(k), value_literal(v)));
                }
                rec.push('}');
            }
            out.push_str(&rec);
            if ov.remove {
                out.push_str(", remove: true");
            }
            if !ov.add.is_empty() {
                // 结构性覆盖：追加的子树按节点层级递归写出（复用 write_node）。
                out.push_str(", add: [\n");
                for added in &ov.add {
                    out.push_str(&ind3);
                    write_node(out, added, opts, level + 3);
                    out.push_str(",\n");
                }
                out.push_str(&format!("{ind2}])"));
            } else {
                out.push(')');
            }
            out.push_str(",\n");
        }
        out.push_str(&format!("{ind1}],\n"));
    }

    if doc.props.is_empty() {
        out.push_str(&format!("{ind1}props: {{}},\n"));
    } else {
        out.push_str(&format!("{ind1}props: {{\n"));
        for (k, v) in &doc.props {
            out.push_str(&format!("{ind2}{}: {},\n", quote(k), value_literal(v)));
        }
        out.push_str(&format!("{ind1}}},\n"));
    }

    if doc.children.is_empty() {
        out.push_str(&format!("{ind1}children: [],\n"));
    } else {
        out.push_str(&format!("{ind1}children: [\n"));
        for c in &doc.children {
            out.push_str(&ind2);
            write_node(out, c, opts, level + 2);
            out.push_str(",\n");
        }
        out.push_str(&format!("{ind1}],\n"));
    }
    out.push_str(&format!("{ind})"));
}

fn transform_literal(t: Transform2D) -> String {
    format!(
        "(x: {}, y: {}, rot: {}, sx: {}, sy: {}, skew: {})",
        f32_literal(t.pos.x),
        f32_literal(t.pos.y),
        f32_literal(t.rot),
        f32_literal(t.scale.x),
        f32_literal(t.scale.y),
        f32_literal(t.skew)
    )
}

fn value_literal(v: &Value) -> String {
    match v {
        Value::F32(f) => format!("F32({})", f32_literal(*f)),
        Value::I64(i) => format!("I64({i})"),
        Value::Bool(b) => format!("Bool({b})"),
        Value::Str(s) => format!("Str({})", quote(s)),
        Value::Vec2(v) => format!("Vec2({}, {})", f32_literal(v.x), f32_literal(v.y)),
        Value::Resource(r) => format!("Resource({r})"),
    }
}

fn f32_literal(f: f32) -> String {
    if f.is_nan() {
        "NaN".to_string()
    } else if f.is_infinite() {
        if f.is_sign_positive() {
            "inf".to_string()
        } else {
            "-inf".to_string()
        }
    } else {
        // `{:?}` 对浮点保证带小数点或指数，读回来不会退化成整数。
        format!("{f:?}")
    }
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------- 读入 ----------

/// RON 文本 → 文档。
pub fn parse_ron(s: &str) -> Result<SceneDoc, ParseError> {
    let mut p = Parser::new(s);
    let doc = p.scene()?;
    p.skip_trivia();
    if p.peek().is_some() {
        return Err(p.error("文档末尾有游离内容"));
    }
    Ok(doc)
}

/// 文档 → 树。
///
/// 属性写入走 schema 校验；schema 不认识的属性原样保留（前向兼容）。
/// 结束后统一落地结构变更并冲洗世界变换，因此返回的树可直接渲染与查询。
///
/// 只关心结构时用这个；要让属性里的资源引用变成真资源，用
/// [`instantiate_doc_with_resources`]。
pub fn instantiate_doc(doc: &SceneDoc) -> Result<SceneTree, ParseError> {
    check_version(doc)?;
    build_tree(doc)
}

/// 文档 → 树 + 资源表 + 一致性体检报告。
///
/// 表按文档里的 `resources` 段**逐槽位**复原（不重新分配编号），因此属性里的
/// `Resource(n)` 读回来仍指向写出去时那个文件。随后扫描全树补齐悬垂引用
/// （M2 存量场景里那些没声明的 `Resource(n)` 会以悬垂项形式留在表里，
/// 不报错、不丢数据），并检查类别是否与 schema 提示冲突。
pub fn instantiate_doc_with_resources(
    doc: &SceneDoc,
) -> Result<(SceneTree, ResourceTable, AdoptReport), ParseError> {
    check_version(doc)?;
    let tree = build_tree(doc)?;
    let mut table = ResourceTable::new();
    for r in &doc.resources {
        let kind = r.asset_kind().ok_or_else(|| {
            ParseError::semantic(format!("资源槽位 {} 的类别 `{}` 无法识别", r.id, r.kind))
        })?;
        table
            .declare_at(ResId::new(r.id), &r.path, kind)
            .map_err(|e| ParseError::semantic(format!("资源槽位 {} 声明失败：{e}", r.id)))?;
    }
    let report = table.adopt_tree(&tree);
    Ok((tree, table, report))
}

/// RON 文本 → 树。
pub fn instantiate(s: &str) -> Result<SceneTree, ParseError> {
    instantiate_doc(&parse_ron(s)?)
}

// ---------- 子场景嵌套（草案 §10） ----------

/// 子场景引用属性名（**扩展属性**，不入 schema）。
///
/// 值为 `Resource(n)`：n 是本场景 `resources` 段里 `kind: "Scene"` 的槽位，
/// 指向另一份场景文件（路径相对资产根）。加载时递归展开：被引场景的根
/// 成为该节点的子节点，子树归子场景文件所有（回写只留引用，见
/// [`node_to_doc`] 的边界规则）。`Resource(0)` = 未绑定，不展开。
pub const PROP_SUB_SCENE: &str = "sub_scene";

/// 把文档里所有 `sub_scene` 引用**递归展开**成完整节点树（纯函数，文件
/// 读取经 `load` 注入 —— 路径相对资产根，返回文档或错误消息）。
///
/// # 槽位重编号（正确性关键）
///
/// 父子文档的资源槽位号各管各的，直接合并会撞号（父槽 1 = 场景文件、
/// 子槽 1 = 纹理）。展开时子文档的全部资源声明按 **(path, kind) 去重复用**
/// 合并进父表（资产身份 = 路径），并把嫁接子树里所有 `Resource(n)` 引用按
/// 映射改写 —— 展开后的文档只有一张表、一套号，且编号在
/// 展开->存盘->再展开 循环里幂等。递归同理（孙辈先展开、再随子辈整体合并）。
///
/// # 循环引用
///
/// 以引用路径链检测（`A -> B -> A` 如实报错并指名链条）。路径是子场景
/// 身份的判定键。
pub fn expand_subscenes(
    doc: &SceneDoc,
    load: &mut dyn FnMut(&str) -> Result<SceneDoc, String>,
) -> Result<SceneDoc, String> {
    let mut out = doc.clone();
    let mut chain: Vec<String> = Vec::new();
    expand_node(&mut out.root, &mut out.resources, load, &mut chain)?;
    Ok(out)
}

/// 展开一个节点（含其子节点里的引用 —— 包装节点可以出现在任何深度）。
fn expand_node(
    node: &mut NodeDoc,
    resources: &mut Vec<ResourceDoc>,
    load: &mut dyn FnMut(&str) -> Result<SceneDoc, String>,
    chain: &mut Vec<String>,
) -> Result<(), String> {
    for child in node.children.iter_mut() {
        expand_node(child, resources, load, chain)?;
    }
    let Some(slot) = bound_sub_scene_doc(node) else {
        return Ok(());
    };
    let path = resources
        .iter()
        .find(|r| r.id as u64 == slot)
        .map(|r| r.path.clone())
        .ok_or_else(|| format!("子场景槽位 {slot} 未声明（sub_scene 引用悬垂）"))?;
    if chain.contains(&path) {
        return Err(format!("子场景循环引用：{} -> {path}", chain.join(" -> ")));
    }
    chain.push(path.clone());
    let mut child = load(&path).map_err(|e| format!("加载子场景 {path} 失败：{e}"))?;
    // 先递归展开子文档内部（孙辈），再整体嫁接。
    let mut child_resources = std::mem::take(&mut child.resources);
    let mut child_root = child.root;
    expand_node(&mut child_root, &mut child_resources, load, chain)?;
    chain.pop();

    // 重编号：子文档资源（含孙辈合并进来的）合并进父表 —— **按 (path, kind)
    // 去重复用**（资产身份 = 路径，不是每次引用一个新槽位）：已存在的映射到
    // 既有槽，没有的追加。这让编号在 展开->存盘->再展开 循环里幂等，也让
    // 同一子场景的多处实例共享资源声明。嫁接子树引用按映射改写。
    let mut next = resources.iter().map(|r| r.id).max().unwrap_or(0) + 1;
    let mut remap: std::collections::BTreeMap<u64, u64> = std::collections::BTreeMap::new();
    for r in child_resources {
        let new_id = resources
            .iter()
            .find(|p| p.path == r.path && p.kind == r.kind)
            .map(|p| p.id as u64)
            .unwrap_or_else(|| {
                let id = next;
                resources.push(ResourceDoc { id, ..r });
                next += 1;
                id as u64
            });
        remap.insert(r.id as u64, new_id);
    }
    remap_resources_in(&mut child_root, &remap);
    // 包装节点的子树是子场景的派生内容：**替换**（父文件里手写在包装节点下的
    // 内容不归它所有 —— 边界规则见 PROP_SUB_SCENE 文档）。
    node.children = vec![child_root];
    Ok(())
}

/// 文档节点上已绑定的子场景槽位号（与树侧 [`bound_sub_scene`] 同判定）。
fn bound_sub_scene_doc(node: &NodeDoc) -> Option<u64> {
    match node.props.iter().find(|(k, _)| k == PROP_SUB_SCENE) {
        Some((_, Value::Resource(n))) if *n != 0 => Some(*n),
        _ => None,
    }
}

/// 按映射改写子树里所有 `Resource(n)` 引用（含本节点与全部后代）。
fn remap_resources_in(node: &mut NodeDoc, remap: &std::collections::BTreeMap<u64, u64>) {
    for (_, v) in node.props.iter_mut() {
        if let Value::Resource(n) = v {
            if let Some(mapped) = remap.get(n) {
                *n = *mapped;
            }
        }
    }
    for child in node.children.iter_mut() {
        remap_resources_in(child, remap);
    }
}

fn check_version(doc: &SceneDoc) -> Result<(), ParseError> {
    if doc.version > FORMAT_VERSION {
        return Err(ParseError::semantic(format!(
            "场景格式版本 {} 高于当前引擎支持的 {}",
            doc.version, FORMAT_VERSION
        )));
    }
    Ok(())
}

fn build_tree(doc: &SceneDoc) -> Result<SceneTree, ParseError> {
    let mut tree = SceneTree::new_with_kind(&doc.root.name, doc.root.kind.kind());
    let root = tree.root();
    tree.set_local(root, doc.root.local);
    tree.set_process_mode(root, doc.root.process_mode);
    tree.set_instance_overrides(root, doc.root.overrides.clone());
    apply_props(&mut tree, root, &doc.root.props)?;
    for child in &doc.root.children {
        build_child(&mut tree, root, child)?;
    }
    // 结构先落地（add_node 是延迟队列操作，构建期间 children 尚未挂接），
    // 再全树应用覆盖记录；结构性覆盖（add/remove）本身也是延迟操作，
    // 应用后再落地一次。
    tree.apply_pending();
    apply_all_overrides(&mut tree)?;
    tree.apply_pending();
    tree.refresh_transforms();
    Ok(tree)
}

fn build_child(tree: &mut SceneTree, parent: NodeId, doc: &NodeDoc) -> Result<(), ParseError> {
    let id = tree.add_node(parent, &doc.name, doc.kind.kind());
    tree.set_local(id, doc.local);
    tree.set_process_mode(id, doc.process_mode);
    apply_props(tree, id, &doc.props)?;
    tree.set_instance_overrides(id, doc.overrides.clone());
    for child in &doc.children {
        build_child(tree, id, child)?;
    }
    Ok(())
}

/// 全树应用实例级覆盖（结构落地后）：逐节点取出记录并应用到其展开子树。
fn apply_all_overrides(tree: &mut SceneTree) -> Result<(), ParseError> {
    let ids = tree.preorder();
    for id in ids {
        let records = tree
            .instance_overrides(id)
            .map(<[InstanceOverride]>::to_vec)
            .unwrap_or_default();
        if records.is_empty() {
            continue;
        }
        apply_overrides(tree, id, &records)?;
    }
    Ok(())
}

/// 把实例级覆盖应用到（已落地的）展开子树。
///
/// 路径相对**子场景根**（包装节点的第一个子节点）；悬垂路径（子场景更新后
/// 节点被改名/删除）如实报语义错误并指名 —— 热重载路径上引擎保持上一棵
/// 好树（`load_scene` 失败不替换），文件作者修复后下一轮轮询恢复。
fn apply_overrides(
    tree: &mut SceneTree,
    wrapper: NodeId,
    overrides: &[InstanceOverride],
) -> Result<(), ParseError> {
    let wrapper_name = tree.name(wrapper).unwrap_or("?").to_string();
    let child_root = tree
        .children(wrapper)
        .first()
        .copied()
        .ok_or_else(|| ParseError::semantic(format!(
            "节点 `{wrapper_name}` 声明了覆盖，但没有展开子树（sub_scene 未绑定或未展开）"
        )))?;
    for ov in overrides {
        let mut target = child_root;
        if !ov.path.is_empty() {
            for seg in ov.path.split('/') {
                let next = tree
                    .children(target)
                    .iter()
                    .copied()
                    .find(|&c| tree.name(c) == Some(seg));
                target = next.ok_or_else(|| ParseError::semantic(format!(
                    "节点 `{wrapper_name}` 的覆盖路径 `{}` 在子场景中未命中（段 `{seg}`）",
                    ov.path
                )))?;
            }
        }
        if ov.remove {
            // 结构性覆盖：移除整棵子树（延迟落地 —— 路径解析基于当前结构）。
            // 移除后字段覆盖无意义，跳过；同一记录的 add 由解析层保证互斥。
            tree.queue(crate::tree::TreeOp::Remove {
                node: target,
                keep_children: false,
            });
            continue;
        }
        if let Some(t) = ov.local {
            tree.set_local(target, t);
        }
        if let Some(m) = ov.process_mode {
            tree.set_process_mode(target, m);
        }
        if !ov.props.is_empty() {
            apply_props(tree, target, &ov.props)?;
        }
        for added in &ov.add {
            // 结构性覆盖：追加父场景拥有的子树（延迟挂接，字段即时写入占位节点）。
            build_child(tree, target, added)?;
        }
    }
    Ok(())
}

// ---------- diff 式回写（S6.9） ----------

/// 以**参照子场景**（当前磁盘文件的独立实例化）为基准，把当前树中实例子树
/// 的差异生成为覆盖记录（路径相对子场景根）。
///
/// - 字段差异（`local` / `process_mode` / `props`）按名配对逐层下钻；
/// - **结构性差异也生成记录**（S6.10）：参照独有的节点 -> `remove`，
///   当前独有的节点 -> 按父路径聚合一条 `add`（子树全量导出）；
///   重命名表现为 remove + add；
/// - `Resource(n)` 属性按**所指路径**比较（当前表 vs 参照表）：展开合并后
///   父子槽位号不同但指向同一文件**不算差异** —— 生成的记录用当前
///   （父场景）编号；
/// - 参照取**当前磁盘内容**：保证 存->载 幂等（重载后应用记录复现当前树）。
///   若磁盘子场景在加载后被他人改动且宿主未重载，烘焙会把"当前所见"固化成
///   覆盖 —— 这是"保存所见即所得"的语义；
/// - 记录是**全量重生成**：已应用过的旧记录若仍与参照不同会被重新捕获，
///   恰好等于参照的会自然消失（自清洁、幂等）。
pub fn diff_instance_overrides(
    current: &SceneTree,
    wrapper: NodeId,
    current_table: &ResourceTable,
    reference: &SceneTree,
    reference_table: &ResourceTable,
) -> Vec<InstanceOverride> {
    let mut out = Vec::new();
    let Some(cur_root) = current.children(wrapper).first().copied() else {
        return out; // 无展开子树：无从而来，也无记录
    };
    let cur_side = DiffSide {
        tree: current,
        table: current_table,
    };
    let ref_side = DiffSide {
        tree: reference,
        table: reference_table,
    };
    diff_override_node(&cur_side, cur_root, &ref_side, reference.root(), String::new(), &mut out);
    out
}

/// diff 的一侧：树 + 它的槽位表（资源按所指路径比较时要用）。
struct DiffSide<'a> {
    tree: &'a SceneTree,
    table: &'a ResourceTable,
}

/// 递归配对比较一对节点（`path` 为当前节点相对子场景根的路径）。
#[allow(clippy::too_many_arguments)]
fn diff_override_node(
    current: &DiffSide<'_>,
    cur: NodeId,
    reference: &DiffSide<'_>,
    r#ref: NodeId,
    path: String,
    out: &mut Vec<InstanceOverride>,
) {
    // 1) 三类字段差异。
    let local = if current.tree.local(cur) != reference.tree.local(r#ref) {
        current.tree.local(cur)
    } else {
        None
    };
    let process_mode =
        if current.tree.process_mode(cur) != reference.tree.process_mode(r#ref) {
            current.tree.process_mode(cur)
        } else {
            None
        };
    let empty = crate::props::PropStore::new();
    let cur_store = current.tree.props(cur).unwrap_or(&empty);
    let ref_store = reference.tree.props(r#ref).unwrap_or(&empty);
    let mut props: Vec<(String, Value)> = Vec::new();
    for (k, v) in cur_store.iter() {
        let rv = ref_store.get(k);
        let differs = match (v, rv) {
            // 资源按所指路径比较（槽位号在两张表里各管各的）。
            (Value::Resource(a), Some(Value::Resource(b))) => {
                !resource_same_pointee(current.table, *a, reference.table, *b)
            }
            _ => Some(v) != rv,
        };
        if differs {
            props.push((k.to_string(), v.clone()));
        }
    }
    if local.is_some() || process_mode.is_some() || !props.is_empty() {
        out.push(InstanceOverride {
            path: path.clone(),
            local,
            process_mode,
            props,
            add: Vec::new(),
            remove: false,
        });
    }

    // 2) 按名配对下钻；**结构性差异生成为覆盖记录**（S6.10）：
    //    参照独有（实例中被删）-> `remove` 记录；当前独有（实例中新增）
    //    -> 该父路径下一条 `add` 记录（子树用 node_to_doc 全量导出）。
    //    重命名表现为 remove + add（节点身份不保留，语义无损）。
    let mut added: Vec<NodeDoc> = Vec::new();
    for rc in reference.tree.children(r#ref) {
        let Some(name) = reference.tree.name(*rc) else { continue };
        let child_path = if path.is_empty() {
            name.to_string()
        } else {
            format!("{path}/{name}")
        };
        let Some(cc) = current
            .tree
            .children(cur)
            .iter()
            .copied()
            .find(|&c| current.tree.name(c) == Some(name))
        else {
            out.push(InstanceOverride {
                path: child_path,
                local: None,
                process_mode: None,
                props: Vec::new(),
                add: Vec::new(),
                remove: true,
            });
            continue;
        };
        diff_override_node(current, cc, reference, *rc, child_path, out);
    }
    for cc in current.tree.children(cur) {
        let Some(name) = current.tree.name(*cc) else { continue };
        let in_reference = reference
            .tree
            .children(r#ref)
            .iter()
            .any(|r| reference.tree.name(*r) == Some(name));
        if !in_reference {
            added.push(node_to_doc(current.tree, *cc, false));
        }
    }
    if !added.is_empty() {
        out.push(InstanceOverride {
            path: path.clone(),
            local: None,
            process_mode: None,
            props: Vec::new(),
            add: added,
            remove: false,
        });
    }
}

/// 两个资源槽位是否指向同一路径（解析失败 = 悬垂，悬垂对悬垂视为相等 ——
/// 都是"没有所指"，不该由 diff 产生覆盖；单侧悬垂算差异）。
fn resource_same_pointee(a_table: &ResourceTable, a: u64, b_table: &ResourceTable, b: u64) -> bool {
    let pa = a_table
        .iter()
        .find(|e| e.id().get() as u64 == a)
        .and_then(|e| e.path().map(|p| p.as_str().to_string()));
    let pb = b_table
        .iter()
        .find(|e| e.id().get() as u64 == b)
        .and_then(|e| e.path().map(|p| p.as_str().to_string()));
    match (pa, pb) {
        (Some(x), Some(y)) => x == y,
        (None, None) => true,
        _ => false,
    }
}

fn apply_props(
    tree: &mut SceneTree,
    id: NodeId,
    props: &[(String, Value)],
) -> Result<(), ParseError> {
    let node_name = tree.name(id).unwrap_or("?").to_string();
    for (name, value) in props {
        let known = tree
            .schema_of(id)
            .map(|s| s.prop(name).is_some())
            .unwrap_or(false);
        if known {
            tree.set_prop(id, name, value.clone()).map_err(|e| {
                ParseError::semantic(format!("节点 `{node_name}` 属性写入失败：{e}"))
            })?;
        } else {
            tree.set_prop_raw(id, name, value.clone());
        }
    }
    Ok(())
}

// ---------- 打包 ----------

/// 已打包场景：自包含的 RON 文本 + 头部元信息。
///
/// 相当于 Godot 的 `PackedScene`：是"可直接存盘、直接实例化"的产物，
/// 与编辑器里的活树分开持有。
#[derive(Clone, Debug, PartialEq)]
pub struct PackedScene {
    ron: String,
    format_version: u32,
    node_count: usize,
}

impl PackedScene {
    /// 把一棵树打包成 RON 文本。
    pub fn pack(tree: &SceneTree, opts: &PackOptions) -> Self {
        let doc = if opts.omit_defaults {
            to_doc_omitting_defaults(tree)
        } else {
            to_doc(tree)
        };
        Self {
            format_version: doc.version,
            node_count: doc.root.count(),
            ron: doc_to_ron(&doc, opts),
        }
    }

    /// 树 + 资源表 → 打包场景。
    ///
    /// 与 [`PackedScene::pack`] 的唯一区别是资源声明会随场景一起写盘，
    /// 因此产物是**自解释**的：拿到这一个文件就能复原"属性里的 n 指向哪个资源"。
    pub fn pack_with_resources(
        tree: &SceneTree,
        table: &ResourceTable,
        opts: &PackOptions,
    ) -> Self {
        let doc = to_doc_with_resources(tree, table, opts.omit_defaults);
        Self {
            format_version: doc.version,
            node_count: doc.root.count(),
            ron: doc_to_ron(&doc, opts),
        }
    }

    /// 从既有 RON 文本装载（顺带校验格式版本）。
    pub fn from_ron(ron: impl Into<String>) -> Result<Self, ParseError> {
        let ron = ron.into();
        let doc = parse_ron(&ron)?;
        if doc.version > FORMAT_VERSION {
            return Err(ParseError::semantic(format!(
                "场景格式版本 {} 高于当前引擎支持的 {}",
                doc.version, FORMAT_VERSION
            )));
        }
        Ok(Self {
            node_count: doc.root.count(),
            format_version: doc.version,
            ron,
        })
    }

    /// 从文件装载。
    pub fn load(path: &Path) -> Result<Self, ParseError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ParseError::semantic(format!("读取 {} 失败：{e}", path.display())))?;
        Self::from_ron(text)
    }

    /// RON 文本。
    pub fn ron(&self) -> &str {
        &self.ron
    }

    /// 节点总数。
    pub fn node_count(&self) -> usize {
        self.node_count
    }

    /// 格式版本。
    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    /// 实例化出一棵全新的树。
    pub fn instantiate(&self) -> Result<SceneTree, ParseError> {
        instantiate(&self.ron)
    }

    /// 实例化出树 + 资源表（含一致性体检报告）。
    pub fn instantiate_with_resources(
        &self,
    ) -> Result<(SceneTree, ResourceTable, AdoptReport), ParseError> {
        instantiate_doc_with_resources(&parse_ron(&self.ron)?)
    }

    /// 存盘（自动创建父目录）。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        std::fs::write(path, &self.ron)
    }
}

// ---------- 解析器 ----------

struct Parser {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Parser {
    fn new(s: &str) -> Self {
        Self {
            chars: s.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError::new(self.line, self.col, message)
    }

    /// 跳过空白与注释（`//` 行注释、`/* */` 块注释）。
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.chars.get(self.pos + 1) == Some(&'/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                Some('/') if self.chars.get(self.pos + 1) == Some(&'*') => {
                    self.bump();
                    self.bump();
                    loop {
                        match self.peek() {
                            None => return,
                            Some('*') if self.chars.get(self.pos + 1) == Some(&'/') => {
                                self.bump();
                                self.bump();
                                break;
                            }
                            _ => {
                                self.bump();
                            }
                        }
                    }
                }
                _ => return,
            }
        }
    }

    fn expect(&mut self, c: char) -> Result<(), ParseError> {
        self.skip_trivia();
        if self.peek() == Some(c) {
            self.bump();
            Ok(())
        } else {
            Err(self.error(format!("期望 `{c}`，实际 {:?}", self.peek())))
        }
    }

    fn ident(&mut self) -> Result<String, ParseError> {
        self.skip_trivia();
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        if s.is_empty() {
            return Err(self.error("期望标识符"));
        }
        Ok(s)
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.skip_trivia();
        if self.peek() != Some('"') {
            return Err(self.error("期望字符串"));
        }
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(self.error("字符串没有收尾引号")),
                Some('"') => return Ok(out),
                Some('\\') => match self.bump() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('0') => out.push('\0'),
                    Some('u') => {
                        self.expect('{')?;
                        let mut hex = String::new();
                        while let Some(c) = self.peek() {
                            if c == '}' {
                                break;
                            }
                            hex.push(c);
                            self.bump();
                        }
                        self.expect('}')?;
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| self.error("非法 Unicode 转义"))?;
                        let ch = char::from_u32(code)
                            .ok_or_else(|| self.error("Unicode 转义不是合法字符"))?;
                        out.push(ch);
                    }
                    other => {
                        return Err(self.error(format!("未知转义 `\\{other:?}`")));
                    }
                },
                Some(c) => out.push(c),
            }
        }
    }

    /// 宽松数值词法：数字、可选符号、可选小数与指数，另外接受 `inf` / `NaN`。
    fn number_token(&mut self) -> Result<String, ParseError> {
        self.skip_trivia();
        let mut s = String::new();
        if matches!(self.peek(), Some('+') | Some('-')) {
            s.push(self.bump().unwrap_or('+'));
        }
        let mut digits = false;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            s.push(self.bump().unwrap_or('0'));
            digits = true;
        }
        if self.peek() == Some('.') {
            s.push('.');
            self.bump();
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                s.push(self.bump().unwrap_or('0'));
                digits = true;
            }
        }
        if digits && matches!(self.peek(), Some('e') | Some('E')) {
            s.push(self.bump().unwrap_or('e'));
            if matches!(self.peek(), Some('+') | Some('-')) {
                s.push(self.bump().unwrap_or('+'));
            }
            let mut exp = false;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                s.push(self.bump().unwrap_or('0'));
                exp = true;
            }
            if !exp {
                return Err(self.error("指数缺少数字"));
            }
        }
        if !digits {
            // `inf` / `NaN` 这类非数字形态，交给上层 `f32` 解析判定。
            if let Some(c) = self.peek() {
                if c.is_alphabetic() {
                    let id = self.ident()?;
                    s.push_str(&id);
                    return Ok(s);
                }
            }
            return Err(self.error("期望数字"));
        }
        Ok(s)
    }

    fn f32_value(&mut self) -> Result<f32, ParseError> {
        let token = self.number_token()?;
        token
            .parse::<f32>()
            .map_err(|_| self.error(format!("非法浮点数 `{token}`")))
    }

    fn i64_value(&mut self) -> Result<i64, ParseError> {
        let token = self.number_token()?;
        token
            .parse::<i64>()
            .map_err(|_| self.error(format!("非法整数 `{token}`")))
    }

    fn bool_value(&mut self) -> Result<bool, ParseError> {
        let id = self.ident()?;
        match id.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(self.error(format!("期望 `true` 或 `false`，实际 `{other}`"))),
        }
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        self.skip_trivia();
        match self.peek() {
            Some('"') => Ok(Value::Str(self.string()?)),
            Some(c) if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' => {
                let token = self.number_token()?;
                if token.contains('.') || token.contains('e') || token.contains('E') {
                    Ok(Value::F32(token.parse::<f32>().map_err(|_| {
                        self.error(format!("非法浮点数 `{token}`"))
                    })?))
                } else {
                    match token.parse::<i64>() {
                        Ok(i) => Ok(Value::I64(i)),
                        Err(_) => Ok(Value::F32(token.parse::<f32>().map_err(|_| {
                            self.error(format!("非法数值 `{token}`"))
                        })?)),
                    }
                }
            }
            Some(c) if c.is_alphabetic() || c == '_' => {
                let name = self.ident()?;
                match name.as_str() {
                    "true" => return Ok(Value::Bool(true)),
                    "false" => return Ok(Value::Bool(false)),
                    "NaN" => return Ok(Value::F32(f32::NAN)),
                    "inf" => return Ok(Value::F32(f32::INFINITY)),
                    _ => {}
                }
                self.expect('(')?;
                let v = match name.as_str() {
                    "F32" => Value::F32(self.f32_value()?),
                    "I64" => Value::I64(self.i64_value()?),
                    "Bool" => Value::Bool(self.bool_value()?),
                    "Str" => Value::Str(self.string()?),
                    "Vec2" => {
                        let x = self.f32_value()?;
                        self.expect(',')?;
                        let y = self.f32_value()?;
                        Value::Vec2(Vec2::new(x, y))
                    }
                    "Resource" => {
                        let token = self.number_token()?;
                        Value::Resource(token.parse::<u64>().map_err(|_| {
                            self.error(format!("非法资源键 `{token}`"))
                        })?)
                    }
                    other => {
                        return Err(self.error(format!("未知值构造器 `{other}`")));
                    }
                };
                self.expect(')')?;
                Ok(v)
            }
            other => Err(self.error(format!("期望一个值，实际 {other:?}"))),
        }
    }

    /// `{ "key": value, ... }`
    /// `(x: .., y: .., rot: .., sx: .., sy: .., skew: ..)`，字段可缺省。
    fn transform_block(&mut self) -> Result<Transform2D, ParseError> {
        self.expect('(')?;
        let mut t = Transform2D::IDENTITY;
        loop {
            self.skip_trivia();
            match self.peek() {
                Some(')') => {
                    self.bump();
                    return Ok(t);
                }
                None => return Err(self.error("变换没有收尾 `)`")),
                _ => {}
            }
            let field = self.ident()?;
            self.expect(':')?;
            let v = self.f32_value()?;
            match field.as_str() {
                "x" => t.pos.x = v,
                "y" => t.pos.y = v,
                "rot" => t.rot = v,
                "sx" => t.scale.x = v,
                "sy" => t.scale.y = v,
                "skew" => t.skew = v,
                other => {
                    return Err(self.error(format!("变换里未知字段 `{other}`")));
                }
            }
            self.skip_trivia();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(')') => {
                    self.bump();
                    return Ok(t);
                }
                other => {
                    return Err(self.error(format!("变换里期望 `,` 或 `)`，实际 {other:?}")));
                }
            }
        }
    }

    fn props_block(&mut self) -> Result<Vec<(String, Value)>, ParseError> {
        self.expect('{')?;
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                Some('}') => {
                    self.bump();
                    return Ok(out);
                }
                None => return Err(self.error("属性表没有收尾 `}`")),
                _ => {}
            }
            let key = self.string()?;
            self.expect(':')?;
            let value = self.value()?;
            out.push((key, value));
            self.skip_trivia();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some('}') => {
                    self.bump();
                    return Ok(out);
                }
                other => {
                    return Err(self.error(format!("属性表里期望 `,` 或 `}}`，实际 {other:?}")));
                }
            }
        }
    }

    /// `[ Node(...), ... ]`
    fn node_list(&mut self) -> Result<Vec<NodeDoc>, ParseError> {
        self.expect('[')?;
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                Some(']') => {
                    self.bump();
                    return Ok(out);
                }
                None => return Err(self.error("子节点表没有收尾 `]`")),
                _ => {}
            }
            out.push(self.node()?);
            self.skip_trivia();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => {
                    self.bump();
                    return Ok(out);
                }
                other => {
                    return Err(self.error(format!("子节点表里期望 `,` 或 `]`，实际 {other:?}")));
                }
            }
        }
    }

    /// 覆盖记录表：`[ Override(...), ... ]`。
    fn override_list(&mut self) -> Result<Vec<InstanceOverride>, ParseError> {
        self.expect('[')?;
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                Some(']') => {
                    self.bump();
                    return Ok(out);
                }
                None => return Err(self.error("覆盖表没有收尾 `]`")),
                _ => {}
            }
            out.push(self.override_record()?);
            self.skip_trivia();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => {
                    self.bump();
                    return Ok(out);
                }
                other => {
                    return Err(self.error(format!("覆盖表里期望 `,` 或 `]`，实际 {other:?}")));
                }
            }
        }
    }

    /// 单条覆盖：`Override(path: "a/b", local: (...), process_mode: "...", props: {...})`。
    /// 除 `path` 外的字段可缺（缺 = 不覆盖该字段）。
    fn override_record(&mut self) -> Result<InstanceOverride, ParseError> {
        self.skip_trivia();
        let head = self.ident()?;
        if head != "Override" {
            return Err(self.error(format!("期望 `Override`，实际 `{head}`")));
        }
        self.expect('(')?;

        let mut path: Option<String> = None;
        let mut local = None;
        let mut process_mode = None;
        let mut props: Vec<(String, Value)> = Vec::new();
        let mut add: Vec<NodeDoc> = Vec::new();
        let mut remove = false;

        loop {
            self.skip_trivia();
            match self.peek() {
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return Err(self.error("覆盖记录没有收尾 `)`")),
                _ => {}
            }
            let field = self.ident()?;
            self.expect(':')?;
            match field.as_str() {
                "path" => path = Some(self.string()?),
                "local" => local = Some(self.transform_block()?),
                "process_mode" => {
                    self.skip_trivia();
                    let text = if self.peek() == Some('"') {
                        self.string()?
                    } else {
                        self.ident()?
                    };
                    process_mode = Some(ProcessMode::from_str_exact(&text).ok_or_else(|| {
                        self.error(format!("未知处理模式 `{text}`"))
                    })?);
                }
                "props" => {
                    self.skip_trivia();
                    if self.peek() == Some('{') {
                        props = self.props_block()?;
                    } else {
                        return Err(self.error("覆盖记录的 `props` 期望 `{`"));
                    }
                }
                "add" => {
                    add = self.node_list()?;
                }
                "remove" => {
                    self.skip_trivia();
                    let text = self.ident()?;
                    remove = match text.as_str() {
                        "true" => true,
                        "false" => false,
                        _ => return Err(self.error(format!("`remove` 期望 true/false，实际 `{text}`"))),
                    };
                }
                other => {
                    return Err(self.error(format!("覆盖记录里未知字段 `{other}`")));
                }
            }
            self.skip_trivia();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return Err(self.error("覆盖记录没有收尾 `)`")),
                other => {
                    return Err(self.error(format!("覆盖记录里期望 `,` 或 `)`，实际 {other:?}")));
                }
            }
        }

        let path = path.ok_or_else(|| ParseError::semantic("覆盖记录缺少 `path` 字段"))?;
        if remove && !add.is_empty() {
            return Err(ParseError::semantic(format!(
                "覆盖记录 `{path}` 同时声明 remove 与 add —— 互斥"
            )));
        }
        Ok(InstanceOverride {
            path,
            local,
            process_mode,
            props,
            add,
            remove,
        })
    }

    fn node(&mut self) -> Result<NodeDoc, ParseError> {
        self.skip_trivia();
        let head = self.ident()?;
        if head != "Node" {
            return Err(self.error(format!("期望 `Node`，实际 `{head}`")));
        }
        self.expect('(')?;

        let mut name = String::new();
        let mut has_name = false;
        let mut kind: Option<NodeKindTag> = None;
        let mut local = Transform2D::IDENTITY;
        let mut process_mode = ProcessMode::Inherit;
        let mut overrides: Vec<InstanceOverride> = Vec::new();
        let mut props: Vec<(String, Value)> = Vec::new();
        let mut children: Vec<NodeDoc> = Vec::new();

        loop {
            self.skip_trivia();
            match self.peek() {
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return Err(self.error("节点没有收尾 `)`")),
                _ => {}
            }
            let field = self.ident()?;
            self.expect(':')?;
            match field.as_str() {
                "name" => {
                    name = self.string()?;
                    has_name = true;
                }
                "kind" => {
                    self.skip_trivia();
                    let text = if self.peek() == Some('"') {
                        self.string()?
                    } else {
                        self.ident()?
                    };
                    let tag = NodeKindTag::from_str_exact(&text).ok_or_else(|| {
                        self.error(format!("未知节点类型 `{text}`"))
                    })?;
                    kind = Some(tag);
                }
                "local" => {
                    local = self.transform_block()?;
                }
                "process_mode" => {
                    self.skip_trivia();
                    let text = if self.peek() == Some('"') {
                        self.string()?
                    } else {
                        self.ident()?
                    };
                    process_mode = ProcessMode::from_str_exact(&text).ok_or_else(|| {
                        self.error(format!("未知处理模式 `{text}`"))
                    })?;
                }
                "overrides" => {
                    overrides = self.override_list()?;
                }
                "props" => {
                    self.skip_trivia();
                    if self.peek() == Some('{') {
                        props = self.props_block()?;
                    } else {
                        // 宽容：显式 `null` 之外的一切都当空表处理不了，直接报错更诚实。
                        return Err(self.error("`props` 期望 `{`"));
                    }
                }
                "children" => {
                    children = self.node_list()?;
                }
                other => {
                    return Err(self.error(format!("节点里未知字段 `{other}`")));
                }
            }
            self.skip_trivia();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return Err(self.error("节点没有收尾 `)`")),
                other => {
                    return Err(self.error(format!("节点里期望 `,` 或 `)`，实际 {other:?}")));
                }
            }
        }

        if !has_name {
            return Err(ParseError::semantic("节点缺少 `name` 字段"));
        }
        let kind = kind.ok_or_else(|| ParseError::semantic("节点缺少 `kind` 字段"))?;
        Ok(NodeDoc {
            name,
            kind,
            local,
            process_mode,
            overrides,
            props,
            children,
        })
    }

    fn scene(&mut self) -> Result<SceneDoc, ParseError> {
        self.skip_trivia();
        let head = self.ident()?;
        if head != "Scene" {
            return Err(self.error(format!("期望 `Scene` 作为文档根，实际 `{head}`")));
        }
        self.expect('(')?;

        let mut version: Option<u32> = None;
        let mut resources: Vec<ResourceDoc> = Vec::new();
        let mut root: Option<NodeDoc> = None;

        loop {
            self.skip_trivia();
            match self.peek() {
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return Err(self.error("文档没有收尾 `)`")),
                _ => {}
            }
            let field = self.ident()?;
            self.expect(':')?;
            match field.as_str() {
                "version" => {
                    let token = self.number_token()?;
                    version = Some(token.parse::<u32>().map_err(|_| {
                        self.error(format!("非法版本号 `{token}`"))
                    })?);
                }
                "resources" => {
                    resources = self.resource_list()?;
                }
                "root" => {
                    root = Some(self.node()?);
                }
                other => {
                    return Err(self.error(format!("文档里未知字段 `{other}`")));
                }
            }
            self.skip_trivia();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return Err(self.error("文档没有收尾 `)`")),
                other => {
                    return Err(self.error(format!("文档里期望 `,` 或 `)`，实际 {other:?}")));
                }
            }
        }

        let version = version.ok_or_else(|| ParseError::semantic("文档缺少 `version` 字段"))?;
        let root = root.ok_or_else(|| ParseError::semantic("文档缺少 `root` 字段"))?;
        Ok(SceneDoc {
            version,
            resources,
            root,
        })
    }

    /// `[ Res(id: 1, path: "...", kind: "Texture"), ... ]`
    fn resource_list(&mut self) -> Result<Vec<ResourceDoc>, ParseError> {
        self.skip_trivia();
        self.expect('[')?;
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                Some(']') => {
                    self.bump();
                    return Ok(out);
                }
                None => return Err(self.error("资源列表没有收尾 `]`")),
                _ => {}
            }
            out.push(self.resource()?);
            self.skip_trivia();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => {
                    self.bump();
                    return Ok(out);
                }
                None => return Err(self.error("资源列表没有收尾 `]`")),
                other => {
                    return Err(self.error(format!("资源列表里期望 `,` 或 `]`，实际 {other:?}")));
                }
            }
        }
    }

    /// `Res(id: 1, path: "...", kind: "Texture")`
    fn resource(&mut self) -> Result<ResourceDoc, ParseError> {
        self.skip_trivia();
        let head = self.ident()?;
        if head != "Res" {
            return Err(self.error(format!("期望 `Res`，实际 `{head}`")));
        }
        self.expect('(')?;

        let mut id: Option<u32> = None;
        let mut path: Option<String> = None;
        let mut kind: Option<String> = None;

        loop {
            self.skip_trivia();
            match self.peek() {
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return Err(self.error("资源声明没有收尾 `)`")),
                _ => {}
            }
            let field = self.ident()?;
            self.expect(':')?;
            match field.as_str() {
                "id" => {
                    let token = self.number_token()?;
                    let raw = token.parse::<u64>().map_err(|_| {
                        self.error(format!("非法资源槽位 `{token}`"))
                    })?;
                    if raw == 0 || raw > u32::MAX as u64 {
                        return Err(ParseError::semantic(format!(
                            "资源槽位 `{raw}` 越界：0 是保留值，上限 {}",
                            u32::MAX
                        )));
                    }
                    id = Some(raw as u32);
                }
                "path" => path = Some(self.string()?),
                "kind" => {
                    // 类别允许写带引号的稳定名，也允许裸标识符（`kind: Texture`）。
                    self.skip_trivia();
                    kind = Some(match self.peek() {
                        Some('"') => self.string()?,
                        _ => self.ident()?,
                    });
                }
                other => {
                    return Err(self.error(format!("资源声明里未知字段 `{other}`")));
                }
            }
            self.skip_trivia();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return Err(self.error("资源声明没有收尾 `)`")),
                other => {
                    return Err(self.error(format!("资源声明里期望 `,` 或 `)`，实际 {other:?}")));
                }
            }
        }

        let id = id.ok_or_else(|| ParseError::semantic("资源声明缺少 `id` 字段"))?;
        let path = path.ok_or_else(|| ParseError::semantic("资源声明缺少 `path` 字段"))?;
        let kind = kind.ok_or_else(|| ParseError::semantic("资源声明缺少 `kind` 字段"))?;
        Ok(ResourceDoc { id, path, kind })
    }
}
