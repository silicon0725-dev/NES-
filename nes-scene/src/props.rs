//! 属性存储：一个节点上全部属性值的容器。
//!
//! 刻意**不认识** `NodeSchema`：校验与默认值填充由 schema 层驱动，
//! 这里只负责"存、取、比、算差异"。这样属性存储可以被单测直接构造，
//! 不需要先搭一棵树。

use std::collections::BTreeMap;
use std::fmt;

use crate::value::{Value, ValueType};

/// 属性写入失败的原因。
#[derive(Clone, Debug, PartialEq)]
pub enum PropError {
    /// 目标节点不在树里（已删除，或 `NodeId` 过期）。
    ///
    /// 严格说这不算"属性存储"的错误，但编辑器点面板时它是最常见的一种，
    /// 让调用方只处理一个错误类型比拆两个更实用。
    NoSuchNode,
    /// 该节点类型没有这个属性。拼错属性名会在这里被挡住。
    UnknownProp(String),
    /// 类型不兼容，且不构成合法转换。
    TypeMismatch {
        /// 属性名。
        name: String,
        /// schema 声明的类型。
        expected: ValueType,
        /// 实际拿到的类型。
        got: ValueType,
    },
}

impl fmt::Display for PropError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchNode => write!(f, "目标节点不存在"),
            Self::UnknownProp(name) => write!(f, "未知属性 `{name}`"),
            Self::TypeMismatch {
                name,
                expected,
                got,
            } => write!(
                f,
                "属性 `{name}` 期望 {}，实际得到 {}",
                expected.as_str(),
                got.as_str()
            ),
        }
    }
}

impl std::error::Error for PropError {}

/// 两个属性存储之间的单条差异。
#[derive(Clone, Debug, PartialEq)]
pub struct PropDiff<'a> {
    /// 属性名。
    pub name: &'a str,
    /// 旧值。`None` 表示新增。
    pub old: Option<&'a Value>,
    /// 新值。`None` 表示删除。
    pub new: Option<&'a Value>,
}

/// 节点属性表。
///
/// 内部用 [`BTreeMap`]：迭代顺序按属性名升序，**确定性**是序列化可复现的前提。
/// 顺序不是编辑器显示顺序 —— 显示顺序由 `NodeSchema` 决定。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PropStore {
    entries: BTreeMap<String, Value>,
    version: u64,
}

impl PropStore {
    /// 空表。
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            version: 0,
        }
    }

    /// 变更版本号。
    ///
    /// 契约（M2 热重载的接口）：**属性名集合不变时，值每变一次 `version` 递增一次**；
    /// 写入相同值不递增。消费者拿 `version` 当缓存失效键。
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 读属性。
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.entries.get(name)
    }

    /// 写属性。返回是否真的发生了变化。
    ///
    /// **不做**类型校验：校验入口是 [`crate::schema::NodeSchema::validate`]。
    pub fn set(&mut self, name: &str, value: Value) -> bool {
        match self.entries.get(name) {
            Some(old) if *old == value => false,
            _ => {
                self.entries.insert(name.to_string(), value);
                self.version = self.version.wrapping_add(1);
                true
            }
        }
    }

    /// 删属性，返回旧值。
    pub fn remove(&mut self, name: &str) -> Option<Value> {
        let old = self.entries.remove(name);
        if old.is_some() {
            self.version = self.version.wrapping_add(1);
        }
        old
    }

    /// 是否含该属性。
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// 属性个数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否没有任何属性。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 清空。
    pub fn clear(&mut self) {
        if !self.entries.is_empty() {
            self.entries.clear();
            self.version = self.version.wrapping_add(1);
        }
    }

    /// 按属性名升序迭代。
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> + '_ {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// 按属性名升序导出，便于序列化。
    pub fn to_pairs(&self) -> Vec<(String, Value)> {
        self.entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// 与另一份存储比较，返回全部差异（按属性名升序）。
    ///
    /// 这是"场景热重载只应用变化项"与"撤销栈记录最小变更"的共同基础。
    pub fn diff<'a>(&'a self, other: &'a PropStore) -> Vec<PropDiff<'a>> {
        let mut out = Vec::new();
        for (name, old) in self.iter() {
            match other.get(name) {
                Some(new) if new == old => {}
                Some(new) => out.push(PropDiff {
                    name,
                    old: Some(old),
                    new: Some(new),
                }),
                None => out.push(PropDiff {
                    name,
                    old: Some(old),
                    new: None,
                }),
            }
        }
        for (name, new) in other.iter() {
            if !self.contains(name) {
                out.push(PropDiff {
                    name,
                    old: None,
                    new: Some(new),
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(b.name));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_remove_roundtrip() {
        let mut p = PropStore::new();
        assert!(p.is_empty());
        assert!(p.set("visible", Value::Bool(true)));
        assert_eq!(p.get("visible"), Some(&Value::Bool(true)));
        assert!(p.contains("visible"));
        assert_eq!(p.len(), 1);
        assert_eq!(p.remove("visible"), Some(Value::Bool(true)));
        assert_eq!(p.remove("visible"), None);
        assert!(p.is_empty());
    }

    #[test]
    fn version_bumps_only_on_real_change() {
        let mut p = PropStore::new();
        let v0 = p.version();
        assert!(p.set("a", Value::I64(1)));
        assert_eq!(p.version(), v0 + 1);
        assert!(!p.set("a", Value::I64(1)));
        assert_eq!(p.version(), v0 + 1);
        assert!(p.set("a", Value::I64(2)));
        assert_eq!(p.version(), v0 + 2);
        p.remove("a");
        assert_eq!(p.version(), v0 + 3);
        p.remove("a");
        assert_eq!(p.version(), v0 + 3);
    }

    #[test]
    fn iteration_is_name_sorted() {
        let mut p = PropStore::new();
        p.set("z", Value::I64(1));
        p.set("a", Value::I64(2));
        p.set("m", Value::I64(3));
        let names: Vec<&str> = p.iter().map(|(k, _)| k).collect();
        assert_eq!(names, vec!["a", "m", "z"]);
    }

    #[test]
    fn diff_reports_added_removed_changed() {
        let mut a = PropStore::new();
        a.set("same", Value::I64(1));
        a.set("changed", Value::I64(1));
        a.set("removed", Value::I64(1));
        let mut b = PropStore::new();
        b.set("same", Value::I64(1));
        b.set("changed", Value::I64(2));
        b.set("added", Value::I64(1));
        let d = a.diff(&b);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].name, "added");
        assert_eq!(d[0].old, None);
        assert_eq!(d[1].name, "changed");
        assert_eq!(d[1].old, Some(&Value::I64(1)));
        assert_eq!(d[1].new, Some(&Value::I64(2)));
        assert_eq!(d[2].name, "removed");
        assert_eq!(d[2].new, None);
        assert!(PropStore::new().diff(&PropStore::new()).is_empty());
    }

    #[test]
    fn error_display_names_the_property() {
        let e = PropError::UnknownProp("nope".into());
        assert!(e.to_string().contains("nope"));
        let e = PropError::TypeMismatch {
            name: "zoom".into(),
            expected: ValueType::F32,
            got: ValueType::Str,
        };
        let s = e.to_string();
        assert!(s.contains("zoom") && s.contains("F32") && s.contains("Str"));
    }
}
