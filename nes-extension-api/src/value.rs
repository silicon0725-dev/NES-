//! 引擎 <-> 脚本的值边界（自有类型）。
//!
//! `NesValue` 是扩展 ABI 唯一的值通货：JS 引擎值（QuickJS 的 `Value` 等）
//! 与引擎值（`nes_scene::Value` 等）都**不得**直接跨越这条边界 —— 双向
//! 转换发生在绑定实现里（`nes-extension-js`），上层对引擎选型零感知。
//!
//! P0 覆盖面：Null / Bool / F64 / Str / Array / Object。整数归一为 F64
//!（JS 侧数值本就是 double）；二进制（字节串）留给后续期次按需扩条
//!（枚举扩条 = 加变体，非破坏性）。

/// 引擎 <-> 脚本的值边界类型。
///
/// 克隆便宜（短字符串/浅表为主），不携带任何第三方类型。
#[derive(Clone, Debug, PartialEq)]
pub enum NesValue {
    /// 空值（JS `null` / `undefined` 归一于此）。
    Null,
    /// 布尔。
    Bool(bool),
    /// 双精度浮点（JS 数值的唯一形态；整数字面量也落在这里）。
    F64(f64),
    /// 字符串。
    Str(String),
    /// 数组。
    Array(Vec<NesValue>),
    /// 对象（键保序 —— 确定性口径：转换序即键序）。
    Object(Vec<(String, NesValue)>),
}

impl NesValue {
    /// 便捷构造：字符串。
    pub fn str(s: impl Into<String>) -> Self {
        NesValue::Str(s.into())
    }

    /// 便捷构造：数组。
    pub fn arr(items: impl IntoIterator<Item = NesValue>) -> Self {
        NesValue::Array(items.into_iter().collect())
    }

    /// 便捷构造：对象（键值对按给定顺序保留）。
    pub fn obj(pairs: impl IntoIterator<Item = (&'static str, NesValue)>) -> Self {
        NesValue::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// 读布尔。
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            NesValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// 读数值。
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            NesValue::F64(x) => Some(*x),
            _ => None,
        }
    }

    /// 读字符串。
    pub fn as_str(&self) -> Option<&str> {
        match self {
            NesValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// 是否为空值。
    pub fn is_null(&self) -> bool {
        matches!(self, NesValue::Null)
    }
}

impl From<bool> for NesValue {
    fn from(v: bool) -> Self {
        NesValue::Bool(v)
    }
}

impl From<f64> for NesValue {
    fn from(v: f64) -> Self {
        NesValue::F64(v)
    }
}

impl From<&str> for NesValue {
    fn from(v: &str) -> Self {
        NesValue::Str(v.to_string())
    }
}

impl From<String> for NesValue {
    fn from(v: String) -> Self {
        NesValue::Str(v)
    }
}

#[cfg(test)]
mod tests {
    // 测试字面量全 ASCII（纪律）。
    use super::NesValue;

    #[test]
    fn accessors_read_only_matching_variant() {
        assert_eq!(NesValue::Bool(true).as_bool(), Some(true));
        assert_eq!(NesValue::F64(1.5).as_f64(), Some(1.5));
        assert_eq!(NesValue::str("txt").as_str(), Some("txt"));
        assert!(NesValue::Null.is_null());
        // 类型不匹配一律 None（不做隐式转换 —— 边界上宁可显式）。
        assert_eq!(NesValue::F64(1.0).as_bool(), None);
        assert_eq!(NesValue::Bool(false).as_f64(), None);
        assert_eq!(NesValue::str("x").as_f64(), None);
    }

    #[test]
    fn convenience_builders_keep_order() {
        let v = NesValue::obj([("a", NesValue::F64(1.0)), ("b", NesValue::str("s"))]);
        match v {
            NesValue::Object(pairs) => {
                assert_eq!(pairs.len(), 2);
                assert_eq!(pairs[0].0, "a");
                assert_eq!(pairs[1].0, "b");
            }
            other => panic!("expected object, got {other:?}"),
        }
        let a = NesValue::arr([NesValue::F64(1.0), NesValue::str("x"), NesValue::Null]);
        assert_eq!(a, NesValue::Array(vec![
            NesValue::F64(1.0),
            NesValue::Str("x".into()),
            NesValue::Null
        ]));
    }
}
