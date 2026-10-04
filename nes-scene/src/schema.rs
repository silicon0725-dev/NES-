//! 属性 schema：每个节点类型有哪些属性、什么类型、默认值、怎么编辑。
//!
//! 这是"属性反射"的权威描述，也是**唯一**一份属性默认值来源：
//! [`crate::tree::NodeData`] 建节点时按它填默认值，序列化省略默认值时按它比对，
//! 编辑器属性面板按它出控件，M5 脚本节点按它注册自定义属性。
//!
//! 继承是**聚合**而非覆盖：子类型的属性集 = 祖先属性（基类在前）+ 自身属性。
//! 因此 `Sprite2D` 的 schema 里能同时看到 `visible`（来自 `Node`）与 `texture`
//! （自身）。类型字段的继承关系仍由 [`NodeKindTag::chain`] 定义，schema 只是投影。

use std::sync::OnceLock;

use crate::node::NodeKindTag;
use crate::props::{PropError, PropStore};
use crate::transform::Vec2;
use crate::ui::THEME_SLOTS;
use crate::value::{Value, ValueType};

/// 编辑器提示。只影响属性面板怎么画控件，不影响语义与序列化。
#[derive(Clone, Debug, PartialEq)]
pub enum EditorHint {
    /// 无提示。
    None,
    /// 数值滑杆。
    Number {
        /// 下界（含）。
        min: f32,
        /// 上界（含）。
        max: f32,
        /// 步进。
        step: f32,
    },
    /// 资源引用。`kind` 是资源类别（M3 接入注册表后用于过滤候选）。
    Resource {
        /// 资源类别。
        kind: &'static str,
    },
    /// 资源引用（多类别，S15）：属性可接受**其中任一**类别的资源。
    ///
    /// 只为"同一属性槽位天然承载多种交付格式"的场景存在 —— 现在只有
    /// 一个使用者：`Sprite2D.texture`（纹理资源或视频资源，视频的当前
    /// 帧就是一张纹理）。**不是**任意 kind 放行口：候选表是编译期常量
    /// 白名单，装载体检（`adopt_tree`）仍按表核对实际声明类别。
    /// `kinds[0]` 是报告口径类别（mismatch 信息里用它指名"期望"）。
    ResourceMany {
        /// 可接受的资源类别（提示串，经 `asset_kind_of_hint` 解析）。
        kinds: &'static [&'static str],
    },
    /// 多行文本。
    Multiline,
    /// 枚举取值。
    Enum {
        /// 允许的取值。
        values: &'static [&'static str],
    },
}

/// 单条属性描述。
#[derive(Clone, Debug, PartialEq)]
pub struct PropDesc {
    name: &'static str,
    ty: ValueType,
    default: Value,
    hint: EditorHint,
    doc: &'static str,
}

impl PropDesc {
    /// 构造。`ty` 必须与 `default` 的变体一致 —— 由 [`NodeSchema::build`] 的
    /// 自检断言保证，不一致会在首次取 schema 时直接 panic，而不是悄悄跑歪。
    pub fn new(
        name: &'static str,
        ty: ValueType,
        default: Value,
        hint: EditorHint,
        doc: &'static str,
    ) -> Self {
        Self {
            name,
            ty,
            default,
            hint,
            doc,
        }
    }

    /// 属性名。
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// 声明类型。
    pub fn ty(&self) -> ValueType {
        self.ty
    }

    /// 默认值。
    pub fn default_value(&self) -> &Value {
        &self.default
    }

    /// 编辑器提示。
    pub fn hint(&self) -> &EditorHint {
        &self.hint
    }

    /// 文档串。
    pub fn doc(&self) -> &'static str {
        self.doc
    }

    /// 该值是否等于默认值（序列化省略默认值时用）。
    pub fn is_default(&self, value: &Value) -> bool {
        *value == self.default
    }
}

/// 一个节点类型的完整属性集。
#[derive(Clone, Debug, PartialEq)]
pub struct NodeSchema {
    tag: NodeKindTag,
    chain: Vec<NodeKindTag>,
    own: Vec<PropDesc>,
    all: Vec<PropDesc>,
}

impl NodeSchema {
    /// 取某类型的 schema。首次调用时构造并缓存全部 7 张表。
    pub fn of(tag: NodeKindTag) -> &'static NodeSchema {
        static CACHE: OnceLock<Vec<NodeSchema>> = OnceLock::new();
        let tables = CACHE.get_or_init(|| {
            NodeKindTag::ALL
                .iter()
                .map(|t| NodeSchema::build(*t))
                .collect()
        });
        &tables[tag.index()]
    }

    /// 类型标签。
    pub fn tag(&self) -> NodeKindTag {
        self.tag
    }

    /// 继承链，**基类在前**（`Node, Node2D, Sprite2D`）。
    pub fn chain(&self) -> &[NodeKindTag] {
        &self.chain
    }

    /// 本类型自身的属性（不含继承）。
    pub fn own_props(&self) -> &[PropDesc] {
        &self.own
    }

    /// 完整属性集，继承的在先。
    pub fn props(&self) -> &[PropDesc] {
        &self.all
    }

    /// 属性个数。
    pub fn len(&self) -> usize {
        self.all.len()
    }

    /// 是否无属性。
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// 按名查属性描述。
    pub fn prop(&self, name: &str) -> Option<&PropDesc> {
        self.all.iter().find(|p| p.name == name)
    }

    /// 按名取默认值。
    pub fn default_value(&self, name: &str) -> Option<&Value> {
        self.prop(name).map(|p| p.default_value())
    }

    /// 生成一份填满默认值的属性存储。
    pub fn default_store(&self) -> PropStore {
        let mut s = PropStore::new();
        for p in &self.all {
            s.set(p.name, p.default.clone());
        }
        s
    }

    /// 校验并归一化一次写入。
    ///
    /// 1. 属性必须存在，否则 [`PropError::UnknownProp`]；
    /// 2. 类型必须一致，或构成合法数值转换（见 [`Value::coerce_to`]），
    ///    否则 [`PropError::TypeMismatch`]；
    /// 3. 带 [`EditorHint::Number`] 的属性按上下界 **clamp**，而不是报错 ——
    ///    脚本设超范围的缩放值应当被夹到边界，这是编辑器滑杆的同一语义。
    pub fn validate(&self, name: &str, value: &Value) -> Result<Value, PropError> {
        let desc = self
            .prop(name)
            .ok_or_else(|| PropError::UnknownProp(name.to_string()))?;
        let coerced = value.coerce_to(desc.ty()).ok_or_else(|| PropError::TypeMismatch {
            name: name.to_string(),
            expected: desc.ty(),
            got: value.type_of(),
        })?;
        Ok(clamp_to_hint(coerced, desc.hint()))
    }

    fn build(tag: NodeKindTag) -> NodeSchema {
        let mut chain = tag.chain();
        chain.reverse();
        let own = own_props(tag);
        let mut all = Vec::new();
        for t in &chain {
            all.extend(own_props(*t));
        }
        debug_assert!(all
            .iter()
            .all(|p| p.ty() == p.default_value().type_of()));
        NodeSchema {
            tag,
            chain,
            own,
            all,
        }
    }
}

fn clamp_to_hint(value: Value, hint: &EditorHint) -> Value {
    let (min, max) = match hint {
        EditorHint::Number { min, max, .. } => (*min, *max),
        _ => return value,
    };
    match value {
        Value::F32(f) => {
            if f.is_nan() {
                Value::F32(f)
            } else {
                Value::F32(f.clamp(min, max))
            }
        }
        Value::I64(i) => Value::I64((i as f64).clamp(min as f64, max as f64) as i64),
        other => other,
    }
}

/// 每个类型自身的属性表。命名顺序即编辑器显示顺序。
fn own_props(tag: NodeKindTag) -> Vec<PropDesc> {
    use EditorHint as H;
    match tag {
        NodeKindTag::Node => vec![
            PropDesc::new(
                "visible",
                ValueType::Bool,
                Value::Bool(true),
                H::None,
                "是否参与显示与处理。隐藏节点仍参与变换与脚本逻辑。",
            ),
            PropDesc::new(
                "timer",
                ValueType::I64,
                Value::I64(0),
                H::Number { min: 0.0, max: 3600.0, step: 1.0 },
                "每帧倒计时。引擎每 tick -1 到 0 停住。脚本读写（S10-1/F-2）。",
            ),
        ],
        NodeKindTag::Node2D => vec![PropDesc::new(
            "z_index",
            ValueType::I64,
            Value::I64(0),
            H::Number {
                min: -4096.0,
                max: 4096.0,
                step: 1.0,
            },
            "绘制层级。同层内越大越靠前。",
        )],
        NodeKindTag::Sprite2D => vec![
            PropDesc::new(
                "texture",
                ValueType::Resource,
                Value::Resource(0),
                // S15：纹理**或视频**资源（视频资源运行中经渲染侧同键逐帧
                // 覆写"当前帧"，Sprite 引用它即播画面）。类型校验面不变
                //（仍是 ValueType::Resource），放宽只在提示/体检层。
                H::ResourceMany { kinds: &["texture", "video"] },
                "纹理或视频资源键。0 表示未绑定。",
            ),
            PropDesc::new(
                "flip_h",
                ValueType::Bool,
                Value::Bool(false),
                H::None,
                "水平翻转。",
            ),
            PropDesc::new(
                "flip_v",
                ValueType::Bool,
                Value::Bool(false),
                H::None,
                "垂直翻转。",
            ),
            PropDesc::new(
                "alpha",
                ValueType::F32,
                Value::F32(1.0),
                H::Number { min: 0.0, max: 1.0, step: 0.01 },
                "不透明度（0 透明 .. 1 不透明）。S16.1 加性 schema：缺省 1.0 \
                 —— 渲染侧 tint 乘 255/255 = 恒等，无补间场景的观感与指纹外的\
                 语义逐位不变（指纹采样面含本键，见 S16.1 文档 §2 基线重录）。",
            ),
            PropDesc::new(
                "sheet_cols",
                ValueType::I64,
                Value::I64(0),
                H::Number { min: 0.0, max: 4096.0, step: 1.0 },
                "图集列数（S16.2）。0 = 整图模式 —— 既有整瓦片采样逐位不变；\
                 > 0 时按纹理宽 ÷ 本值切成网格，帧索引走行主序。",
            ),
            PropDesc::new(
                "sheet_rows",
                ValueType::I64,
                Value::I64(0),
                H::Number { min: 0.0, max: 4096.0, step: 1.0 },
                "图集行数（S16.2）。0 = 正方形网格（行数 = 列数）—— 实现简洁\
                 取舍，文档写明；> 0 时按纹理高 ÷ 本值切行。",
            ),
            PropDesc::new(
                "frame",
                ValueType::I64,
                Value::I64(0),
                H::None,
                "图集帧索引（S16.2；行主序 col = frame % cols、row = frame / \
                 cols）。越出 cols*rows 时模运算回绕（负值同样回绕）—— 补间\
                 循环到末帧后回到 0 的正主通道，不设数值钳制。",
            ),
            PropDesc::new(
                "pivot",
                ValueType::Vec2,
                Value::Vec2(Vec2::ZERO),
                H::None,
                "归一化锚点（S16.3；0..1 相对精灵矩形，缺省 (0,0) = 左上角 \
                 = 既有行为逐位不变）。绘制/旋转/缩放的基准点：(0.5,0.5) \
                 = 中心锚定（位置即精灵中心）。越界值照实接受 = 锚点落在 \
                 精灵外（拖尾/关节挂点等合法用途），不设数值钳制。",
            ),
        ],
        NodeKindTag::Camera2D => vec![
            PropDesc::new(
                "zoom",
                ValueType::F32,
                Value::F32(1.0),
                H::Number {
                    min: 0.05,
                    max: 16.0,
                    step: 0.05,
                },
                "缩放倍数。越大画面越近。",
            ),
            PropDesc::new(
                "active",
                ValueType::Bool,
                Value::Bool(true),
                H::None,
                "是否为当前生效的相机。",
            ),
        ],
        NodeKindTag::Control => vec![
            PropDesc::new(
                "anchor",
                ValueType::Vec2,
                Value::Vec2(Vec2::ZERO),
                H::None,
                "归一化锚点（0..1）。",
            ),
            PropDesc::new(
                "offset",
                ValueType::Vec2,
                Value::Vec2(Vec2::ZERO),
                H::None,
                "相对锚点的像素偏移。",
            ),
            PropDesc::new(
                "size",
                ValueType::Vec2,
                Value::Vec2(Vec2::new(100.0, 100.0)),
                H::None,
                "控件尺寸（像素）。",
            ),
            PropDesc::new(
                "fill_slot",
                ValueType::Str,
                Value::Str(String::new()),
                H::None,
                "填充槽位名（S12.1 主题；空 = 透明）。",
            ),
            PropDesc::new(
                "border_slot",
                ValueType::Str,
                Value::Str("border".into()),
                H::None,
                "边框槽位名（S12.1 主题）。",
            ),
        ],
        NodeKindTag::Button => vec![
            PropDesc::new(
                "text",
                ValueType::Str,
                Value::Str(String::new()),
                H::None,
                "按钮文本。",
            ),
            PropDesc::new(
                "fill_slot",
                ValueType::Str,
                Value::Str("panel".into()),
                H::None,
                "填充槽位名（主题解析；悬停/按下自动换档，S12.0 §3.2）。",
            ),
            PropDesc::new(
                "border_slot",
                ValueType::Str,
                Value::Str("border".into()),
                H::None,
                "边框槽位名。",
            ),
            PropDesc::new(
                "text_slot",
                ValueType::Str,
                Value::Str("text".into()),
                H::None,
                "文字槽位名。",
            ),
        ],
        NodeKindTag::TextInput => vec![
            PropDesc::new(
                "text",
                ValueType::Str,
                Value::Str(String::new()),
                H::None,
                "已提交值（也是初始值）。编辑中的草稿是 UiVm 瞬态，不落此属性。",
            ),
            PropDesc::new(
                "fill_slot",
                ValueType::Str,
                Value::Str("panel".into()),
                H::None,
                "填充槽位名（主题解析）。",
            ),
            PropDesc::new(
                "border_slot",
                ValueType::Str,
                Value::Str("border".into()),
                H::None,
                "边框槽位名（获得焦点时换档 accent，S12-2）。",
            ),
            PropDesc::new(
                "text_slot",
                ValueType::Str,
                Value::Str("text".into()),
                H::None,
                "文字槽位名。",
            ),
            PropDesc::new(
                "placeholder",
                ValueType::Str,
                Value::Str(String::new()),
                H::None,
                "占位提示文本（P0 仅存储，不参与渲染）。",
            ),
        ],
        NodeKindTag::ScrollView => vec![PropDesc::new(
            "step",
            ValueType::I64,
            Value::I64(48),
            H::Number {
                min: 1.0,
                max: 512.0,
                step: 1.0,
            },
            "每格滚轮滚动的像素数（S12-3）。",
        )],
        NodeKindTag::ListView => vec![
            PropDesc::new(
                "rows",
                ValueType::Str,
                Value::Str(String::new()),
                H::Multiline,
                "行文本，'\\n' 分隔（空串 = 无行）。",
            ),
            PropDesc::new(
                "row_h",
                ValueType::I64,
                Value::I64(18),
                H::Number {
                    min: 4.0,
                    max: 128.0,
                    step: 1.0,
                },
                "行高（像素）；滚轮步进同此值。",
            ),
            PropDesc::new(
                "selected",
                ValueType::I64,
                Value::I64(-1),
                H::None,
                "选中行下标（-1 = 无选中）；落账由宿主经 on_row_activate 回调自做。",
            ),
            PropDesc::new(
                "text_slot",
                ValueType::Str,
                Value::Str("text".into()),
                H::None,
                "行文字槽位名（主题解析）。",
            ),
            PropDesc::new(
                "sel_fill_slot",
                ValueType::Str,
                Value::Str("selected".into()),
                H::None,
                "选中行填充槽位名（主题解析）。",
            ),
        ],
        NodeKindTag::Tabs => vec![
            PropDesc::new(
                "tabs",
                ValueType::Str,
                Value::Str(String::new()),
                H::Multiline,
                "页签文本，'\\n' 分隔（空串 = 无页签）。",
            ),
            PropDesc::new(
                "tab_w",
                ValueType::I64,
                Value::I64(64),
                H::Number {
                    min: 8.0,
                    max: 256.0,
                    step: 1.0,
                },
                "单个页签宽度（像素）。",
            ),
            PropDesc::new(
                "active",
                ValueType::I64,
                Value::I64(-1),
                H::None,
                "活动页签下标（-1 = 无）；由宿主落账。",
            ),
            PropDesc::new(
                "text_slot",
                ValueType::Str,
                Value::Str("text".into()),
                H::None,
                "页签文字槽位名（主题解析）。",
            ),
            PropDesc::new(
                "sel_fill_slot",
                ValueType::Str,
                Value::Str("selected".into()),
                H::None,
                "活动页签填充槽位名（主题解析）。",
            ),
        ],
        NodeKindTag::Theme => THEME_SLOTS
            .iter()
            .map(|(name, rgba)| {
                let packed = ((rgba[0] as i64) << 24)
                    | ((rgba[1] as i64) << 16)
                    | ((rgba[2] as i64) << 8)
                    | rgba[3] as i64;
                PropDesc::new(
                    name,
                    ValueType::I64,
                    Value::I64(packed),
                    H::None,
                    "语义槽位色（I64 0xRRGGBBAA 打包）。",
                )
            })
            .collect(),
        NodeKindTag::Label => vec![
            PropDesc::new(
                "text",
                ValueType::Str,
                Value::Str(String::new()),
                H::Multiline,
                "显示文本。",
            ),
            PropDesc::new(
                "font_size",
                ValueType::I64,
                Value::I64(16),
                H::Number {
                    min: 8.0,
                    max: 128.0,
                    step: 1.0,
                },
                "字号（像素）。",
            ),
            PropDesc::new(
                "color_slot",
                ValueType::Str,
                Value::Str("text".into()),
                H::None,
                "文字槽位名（S12.1 主题）。",
            ),
        ],
        NodeKindTag::Script => vec![
            PropDesc::new(
                "registry_key",
                ValueType::Str,
                Value::Str(String::new()),
                H::None,
                "脚本注册表键。空串表示未绑定；这是 M5 兼容层的唯一挂载点。",
            ),
            PropDesc::new(
                "source",
                ValueType::Str,
                Value::Str(String::new()),
                H::None,
                "内嵌脚本文本（S6.31）。非空即由 ScriptVm 在 attach 时编译装载；\
                 与 registry_key 互斥（一个节点一个事实来源）。\
                 多行源码经 RON 字符串转义随场景文件往返。",
            ),
            PropDesc::new(
                "script",
                ValueType::Resource,
                Value::Resource(0),
                H::Resource { kind: "script" },
                "外置脚本资产槽位（S6.33）：指向场景资源表 kind:Script 条目\
                 （.nes 纯文本文件，与纹理/子场景的资产路径同构）。\
                 三路挂载（source/script/registry_key）恰一非空。",
            ),
            PropDesc::new(
                "enabled",
                ValueType::Bool,
                Value::Bool(true),
                H::None,
                "是否参与脚本调度。",
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inheritance_aggregates_base_first() {
        let s = NodeSchema::of(NodeKindTag::Sprite2D);
        assert_eq!(
            s.chain(),
            &[
                NodeKindTag::Node,
                NodeKindTag::Node2D,
                NodeKindTag::Sprite2D
            ]
        );
        let names: Vec<&str> = s.props().iter().map(|p| p.name()).collect();
        assert_eq!(
            names,
            vec![
                "visible",
                "timer",
                "z_index",
                "texture",
                "flip_h",
                "flip_v",
                "alpha",
                "sheet_cols",
                "sheet_rows",
                "frame",
                "pivot"
            ]
        );
        assert_eq!(s.len(), 11);
        // S16.2 加性缺省 = 整图现状：三键缺省全 0。
        assert_eq!(s.default_value("sheet_cols"), Some(&Value::I64(0)));
        assert_eq!(s.default_value("sheet_rows"), Some(&Value::I64(0)));
        assert_eq!(s.default_value("frame"), Some(&Value::I64(0)));
        // S16.3 加性缺省 = 左上角锚定（既有行为）：pivot 缺省 (0,0)。
        assert_eq!(
            s.default_value("pivot"),
            Some(&Value::Vec2(crate::transform::Vec2::ZERO))
        );
    }

    #[test]
    fn own_props_hold_only_own_members() {
        let s = NodeSchema::of(NodeKindTag::Node2D);
        assert_eq!(s.own_props().len(), 1);
        assert_eq!(s.own_props()[0].name(), "z_index");
        assert_eq!(NodeSchema::of(NodeKindTag::Node).own_props().len(), 2);
    }

    #[test]
    fn every_tag_has_a_schema_with_consistent_types() {
        for tag in NodeKindTag::ALL {
            let s = NodeSchema::of(tag);
            assert_eq!(s.tag(), tag);
            assert!(!s.is_empty(), "{tag:?} 应当至少继承到 visible");
            for p in s.props() {
                assert_eq!(p.ty(), p.default_value().type_of(), "{:?}", p.name());
            }
        }
    }

    #[test]
    fn default_store_is_filled_from_schema() {
        let s = NodeSchema::of(NodeKindTag::Camera2D);
        let store = s.default_store();
        // visible（Node）+ z_index（Node2D）+ zoom / active（Camera2D）
        assert_eq!(store.len(), 5);
        assert_eq!(store.get("visible"), Some(&Value::Bool(true)));
        assert_eq!(store.get("zoom"), Some(&Value::F32(1.0)));
        assert_eq!(store.get("active"), Some(&Value::Bool(true)));
        assert_eq!(store.get("nope"), None);

        // 默认表必须与 schema 声明的默认值逐项一致（否则实例化出来的树
        // 会与"新建节点"的树不一致，序列化快照对不上）。
        for (name, value) in store.iter() {
            assert_eq!(s.default_value(name), Some(value), "{name}");
        }
    }

    #[test]
    fn validate_rejects_unknown_and_mismatched() {
        let s = NodeSchema::of(NodeKindTag::Node2D);
        assert_eq!(
            s.validate("nope", &Value::I64(1)),
            Err(PropError::UnknownProp("nope".into()))
        );
        assert_eq!(
            s.validate("visible", &Value::str("yes")),
            Err(PropError::TypeMismatch {
                name: "visible".into(),
                expected: ValueType::Bool,
                got: ValueType::Str,
            })
        );
    }

    #[test]
    fn validate_coerces_and_clamps() {
        let n2d = NodeSchema::of(NodeKindTag::Node2D);
        assert_eq!(n2d.validate("z_index", &Value::F32(3.0)), Ok(Value::I64(3)));
        // clamp 而不是报错
        assert_eq!(n2d.validate("z_index", &Value::I64(99999)), Ok(Value::I64(4096)));
        let cam = NodeSchema::of(NodeKindTag::Camera2D);
        assert_eq!(cam.validate("zoom", &Value::F32(100.0)), Ok(Value::F32(16.0)));
        assert_eq!(cam.validate("zoom", &Value::F32(1.5)), Ok(Value::F32(1.5)));
        assert_eq!(
            cam.validate("zoom", &Value::I64(2)),
            Ok(Value::F32(2.0))
        );
    }

    #[test]
    fn is_default_compares_against_declared_default() {
        let p = NodeSchema::of(NodeKindTag::Node)
            .prop("visible")
            .expect("visible 应当存在");
        assert!(p.is_default(&Value::Bool(true)));
        assert!(!p.is_default(&Value::Bool(false)));
    }

    #[test]
    fn numeric_hint_bounds_are_sane() {
        let s = NodeSchema::of(NodeKindTag::Label);
        match s.prop("font_size").map(|p| p.hint()) {
            Some(EditorHint::Number { min, max, step }) => {
                assert!(*min < *max && *step > 0.0);
            }
            other => panic!("font_size 应当带数值提示，实际 {other:?}"),
        }
    }
}
