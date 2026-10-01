//! 反射值：场景属性的统一运行时表示。
//!
//! M2 的关键决策：**所有设计时属性都存进 `PropStore`，`NodeKind` 只保留类型身份**。
//! 好处有三：
//!
//! 1. 新增节点类型（M5 的脚本节点）不需要动 `NodeKind` 的枚举结构；
//! 2. 属性面板、RON 读写、热重载、撤销栈共用同一套 [`Value`] 通路；
//! 3. 属性的类型与默认值集中由 [`NodeSchema`](crate::schema::NodeSchema) 描述，
//!    不存在"两处各写一份默认值"的漂移风险。
//!
//! 类型纪律是**宽容输入、严格输出**：写入时可做同族数值转换（见
//! [`Value::coerce_to`]），读出的类型始终是 schema 声明的那一种。

use std::fmt;

use crate::transform::Vec2;

/// 值类型标签。与 [`Value`] 的变体一一对应。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ValueType {
    /// 32 位浮点。
    F32,
    /// 64 位整数。
    I64,
    /// 布尔。
    Bool,
    /// 字符串。
    Str,
    /// 二维向量。
    Vec2,
    /// 资源键（指向 `nes-asset` 注册表，M3 接入）。
    Resource,
    /// 节点句柄（运行时引用；S8.2b-1）。
    Node,
    /// 数组（S8.2b-2）。
    Array,
}

impl ValueType {
    /// 全部类型，顺序与 [`ValueType::as_str`] 的声明顺序一致。
    pub const ALL: [ValueType; 6] = [
        Self::F32,
        Self::I64,
        Self::Bool,
        Self::Str,
        Self::Vec2,
        Self::Resource,
    ];

    /// 稳定字符串名。用于序列化与编辑器，**不得**随重构改名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::F32 => "F32",
            Self::I64 => "I64",
            Self::Bool => "Bool",
            Self::Str => "Str",
            Self::Vec2 => "Vec2",
            Self::Resource => "Resource",
            Self::Node => "Node",
            Self::Array => "Array",
        }
    }

    /// 从稳定字符串名还原。
    pub fn from_str_exact(s: &str) -> Option<Self> {
        Some(match s {
            "F32" => Self::F32,
            "I64" => Self::I64,
            "Bool" => Self::Bool,
            "Str" => Self::Str,
            "Vec2" => Self::Vec2,
            "Resource" => Self::Resource,
            _ => return None,
        })
    }

    /// 是否为同族数值类型（可互相转换的那两种）。
    pub const fn is_numeric(self) -> bool {
        matches!(self, Self::F32 | Self::I64)
    }
}

/// 属性值。
///
/// 刻意保持小而封闭：新增类型要同时改 schema、序列化、编辑器三处，
/// 因此只有真正需要落到磁盘上的类型才允许进来。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// 浮点。
    F32(f32),
    /// 整数。
    I64(i64),
    /// 布尔。
    Bool(bool),
    /// 字符串。
    Str(String),
    /// 向量。
    Vec2(Vec2),
    /// 资源键。
    Resource(u64),
    /// 节点句柄（S8.2b-1）：运行时实体引用 —— 可持有/可比较（引用等式），
    /// 不序列化、不进属性表（schema 按 ValueType 校验，Node 不在设计时
    /// 类型集）。语义身份见 resolve 口径（S8.2b v1.1）。
    Node(crate::identity::NodeHandle),
    /// 数组（S8.2b-2）：纯拥有式元素表 —— **赋值 = 深拷贝**（局部绑定
    /// 语义，v1.1 冻结：`a2 = a` 后互不影响，无共享引用容器）；元素可含
    /// 句柄。`push/pop` 变异的是**局部绑定**（读-改-写回写编译）。不序列化。
    Array(Vec<Value>),
}

impl Value {
    /// 值的类型标签。
    pub const fn type_of(&self) -> ValueType {
        match self {
            Self::F32(_) => ValueType::F32,
            Self::I64(_) => ValueType::I64,
            Self::Bool(_) => ValueType::Bool,
            Self::Str(_) => ValueType::Str,
            Self::Vec2(_) => ValueType::Vec2,
            Self::Resource(_) => ValueType::Resource,
            Self::Node(_) => ValueType::Node,
            Self::Array(_) => ValueType::Array,
        }
    }

    /// 便捷构造：字符串。
    pub fn str(s: impl Into<String>) -> Self {
        Self::Str(s.into())
    }

    /// 便捷构造：向量。
    pub const fn vec2(x: f32, y: f32) -> Self {
        Self::Vec2(Vec2::new(x, y))
    }

    /// 取 `f32`。类型不符返回 `None`。
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::F32(v) => Some(*v),
            _ => None,
        }
    }

    /// 取 `i64`。类型不符返回 `None`。
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::I64(v) => Some(*v),
            _ => None,
        }
    }

    /// 取 `bool`。类型不符返回 `None`。
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    /// 取 `&str`。类型不符返回 `None`。
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(v) => Some(v.as_str()),
            _ => None,
        }
    }

    /// 取 [`Vec2`]。类型不符返回 `None`。
    pub fn as_vec2(&self) -> Option<Vec2> {
        match self {
            Self::Vec2(v) => Some(*v),
            _ => None,
        }
    }

    /// 取资源键。类型不符返回 `None`。
    pub fn as_resource(&self) -> Option<u64> {
        match self {
            Self::Resource(v) => Some(*v),
            _ => None,
        }
    }

    /// 转换到目标类型。
    ///
    /// 允许的转换只有同族数值：`I64 → F32`（无损），以及整数取值的
    /// `F32 → I64`（`3.0 → 3`）。`3.5` 这种带小数的不做静默截断，返回 `None`
    /// —— 截断是丢信息，宁可让调用方显式取整。
    pub fn coerce_to(&self, ty: ValueType) -> Option<Value> {
        if self.type_of() == ty {
            return Some(self.clone());
        }
        match (self, ty) {
            (Self::I64(i), ValueType::F32) => Some(Self::F32(*i as f32)),
            (Self::F32(f), ValueType::I64) => {
                // 范围与整数性都要检查：`as` 转换在越界时是饱和的，会静默骗人。
                if !f.is_finite() || f.fract() != 0.0 {
                    return None;
                }
                if *f < i64::MIN as f32 || *f > i64::MAX as f32 {
                    return None;
                }
                Some(Self::I64(*f as i64))
            }
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    /// 人类可读形式（编辑器与日志用）。**不保证**可被 RON 解析器读回，
    /// 需要机器可读请用 [`crate::scene_io`] 里的字面量输出。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::F32(v) => write!(f, "{v}"),
            Self::I64(v) => write!(f, "{v}"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Str(v) => write!(f, "{v}"),
            Self::Vec2(v) => write!(f, "({}, {})", v.x, v.y),
            Self::Resource(v) => write!(f, "resource#{v}"),
            Self::Node(h) => write!(f, "node{h:?}"),
            Self::Array(items) => write!(f, "array({})", items.len()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names_roundtrip() {
        for ty in ValueType::ALL {
            assert_eq!(ValueType::from_str_exact(ty.as_str()), Some(ty));
        }
        assert_eq!(ValueType::from_str_exact("i64"), None);
    }

    #[test]
    fn accessors_are_strict() {
        let v = Value::I64(3);
        assert_eq!(v.as_i64(), Some(3));
        assert_eq!(v.as_f32(), None);
        assert_eq!(Value::str("x").as_str(), Some("x"));
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
        assert_eq!(Value::Resource(7).as_resource(), Some(7));
        assert_eq!(Value::vec2(1.0, 2.0).as_vec2(), Some(Vec2::new(1.0, 2.0)));
    }

    #[test]
    fn coerce_allows_commensurate_numbers_only() {
        assert_eq!(Value::I64(3).coerce_to(ValueType::F32), Some(Value::F32(3.0)));
        assert_eq!(Value::F32(3.0).coerce_to(ValueType::I64), Some(Value::I64(3)));
        assert_eq!(Value::F32(3.5).coerce_to(ValueType::I64), None);
        assert_eq!(Value::F32(f32::NAN).coerce_to(ValueType::I64), None);
        assert_eq!(Value::F32(1e30).coerce_to(ValueType::I64), None);
        assert_eq!(Value::Bool(true).coerce_to(ValueType::F32), None);
        assert_eq!(Value::I64(1).coerce_to(ValueType::Str), None);
    }

    #[test]
    fn display_is_human_readable() {
        assert_eq!(Value::F32(1.5).to_string(), "1.5");
        assert_eq!(Value::I64(-2).to_string(), "-2");
        assert_eq!(Value::Bool(false).to_string(), "false");
        assert_eq!(Value::str("hi").to_string(), "hi");
        assert_eq!(Value::vec2(1.0, 2.0).to_string(), "(1, 2)");
        assert_eq!(Value::Resource(9).to_string(), "resource#9");
    }
}
