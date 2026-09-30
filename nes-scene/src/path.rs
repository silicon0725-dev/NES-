//! 节点路径。
//!
//! 语法（草案第 9 节）：
//!
//! ```text
//! Player/Sprite2D         相对路径，第一段须匹配根节点名
//! /root/Player/Sprite2D   绝对路径（前导 '/'），语义同上
//! Ghost[2]                取同父下第 3 个名为 Ghost 的节点（0 基）
//! ```
//!
//! `[n]` 只在同名兄弟之间消歧。若某父节点下同名不超过 1 个，索引段与非索引段等价。
//! 这与 [`crate::tree::SceneTree::path_of`] 的生成规则严格互逆 —— 往返测试覆盖这一点。

use core::fmt;

/// 路径段。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PathSeg {
    /// 按名字取（同名多个时取第一个）。
    Named(String),
    /// 按名字 + 同名序号取（0 基）。
    Indexed(String, usize),
}

impl PathSeg {
    /// 段名。
    pub fn name(&self) -> &str {
        match self {
            Self::Named(n) => n,
            Self::Indexed(n, _) => n,
        }
    }
}

/// 解析后的节点路径。
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct NodePath {
    /// 是否带前导 `/`。
    pub absolute: bool,
    /// 段序列，自根向下。
    pub segs: Vec<PathSeg>,
}

/// 路径解析错误。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PathError {
    /// 空路径。
    Empty,
    /// 段非法（空段、空名）。
    BadSegment(String),
    /// 索引语法非法。
    BadIndex(String),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "路径为空"),
            Self::BadSegment(s) => write!(f, "非法路径段: {}", s),
            Self::BadIndex(s) => write!(f, "非法索引段: {}", s),
        }
    }
}

impl std::error::Error for PathError {}

impl NodePath {
    /// 解析。错误一律返回 `Err`，不静默丢弃非法段 —— 静默会让配置错误变成难查的运行时问题。
    pub fn parse(s: &str) -> Result<Self, PathError> {
        if s.is_empty() {
            return Err(PathError::Empty);
        }
        let absolute = s.starts_with('/');
        let body = if absolute { &s[1..] } else { s };
        if body.is_empty() {
            return Err(PathError::Empty);
        }
        let mut segs = Vec::new();
        for raw in body.split('/') {
            if raw.is_empty() {
                return Err(PathError::BadSegment(String::from("<空段>")));
            }
            match raw.find('[') {
                None => segs.push(PathSeg::Named(raw.to_string())),
                Some(open) => {
                    if !raw.ends_with(']') || raw.len() < open + 2 {
                        return Err(PathError::BadIndex(raw.to_string()));
                    }
                    let name = &raw[..open];
                    if name.is_empty() {
                        return Err(PathError::BadSegment(raw.to_string()));
                    }
                    let inner = &raw[open + 1..raw.len() - 1];
                    let idx: usize = inner
                        .parse()
                        .map_err(|_| PathError::BadIndex(raw.to_string()))?;
                    segs.push(PathSeg::Indexed(name.to_string(), idx));
                }
            }
        }
        Ok(Self { absolute, segs })
    }

    /// 段数。
    pub fn len(&self) -> usize {
        self.segs.len()
    }

    /// 是否无段。
    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    /// 第一段的名字（自根向下的首节点名）。
    pub fn head_name(&self) -> Option<&str> {
        self.segs.first().map(PathSeg::name)
    }

    /// 去掉首段，得到相对子路径。
    pub fn tail(&self) -> Self {
        Self {
            absolute: self.absolute,
            segs: self.segs.iter().skip(1).cloned().collect(),
        }
    }

    /// 追接一段。
    pub fn joined_with(&self, seg: PathSeg) -> Self {
        let mut segs = self.segs.clone();
        segs.push(seg);
        Self {
            absolute: self.absolute,
            segs,
        }
    }
}

impl fmt::Display for NodePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.absolute {
            write!(f, "/")?;
        }
        for (i, seg) in self.segs.iter().enumerate() {
            if i > 0 {
                write!(f, "/")?;
            }
            match seg {
                PathSeg::Named(n) => write!(f, "{}", n)?,
                PathSeg::Indexed(n, idx) => write!(f, "{}[{}]", n, idx)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relative_and_absolute() {
        let rel = NodePath::parse("A/B").expect("ok");
        assert!(!rel.absolute);
        assert_eq!(rel.len(), 2);
        assert_eq!(rel.head_name(), Some("A"));

        let abs = NodePath::parse("/root/A").expect("ok");
        assert!(abs.absolute);
        assert_eq!(abs.head_name(), Some("root"));
        assert_eq!(abs.to_string(), "/root/A");
    }

    #[test]
    fn parses_index_segments() {
        let p = NodePath::parse("Ghost[2]/Body").expect("ok");
        assert_eq!(p.segs[0], PathSeg::Indexed("Ghost".into(), 2));
        assert_eq!(p.segs[1], PathSeg::Named("Body".into()));
        assert_eq!(p.to_string(), "Ghost[2]/Body");
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(NodePath::parse(""), Err(PathError::Empty));
        assert_eq!(NodePath::parse("/"), Err(PathError::Empty));
        assert!(matches!(
            NodePath::parse("A//B"),
            Err(PathError::BadSegment(_))
        ));
        assert!(matches!(
            NodePath::parse("A[x]"),
            Err(PathError::BadIndex(_))
        ));
        assert!(matches!(
            NodePath::parse("A[1"),
            Err(PathError::BadIndex(_))
        ));
        assert!(matches!(
            NodePath::parse("[1]"),
            Err(PathError::BadSegment(_))
        ));
    }

    #[test]
    fn tail_and_join() {
        let p = NodePath::parse("/A/B/C").expect("ok");
        assert_eq!(p.tail().to_string(), "/B/C");
        assert_eq!(
            p.tail().joined_with(PathSeg::Named("D".into())).to_string(),
            "/B/C/D"
        );
    }
}
