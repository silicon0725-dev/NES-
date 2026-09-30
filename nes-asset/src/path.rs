//! 资源路径：**规范化 + 校验**的相对路径。
//!
//! 场景存档里资源引用只允许是这种路径（例如 `Textures/player.png`），
//! 不允许绝对路径、盘符、`..` 上跳 —— 否则场景文件一旦被移动或分享就会
//! 指到使用者机器的任意位置，这是编辑器最典型的越界风险。

use core::fmt;
use std::sync::Arc;

/// 资源路径错误。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AssetPathError {
    /// 空路径。
    Empty,
    /// 绝对路径（以 `/` 或 `\` 开头）。
    Absolute,
    /// 含盘符或 URL scheme（`C:`、`http:`）。
    HasScheme,
    /// `..` 上跳越出资源根。
    ParentEscape,
    /// 含控制字符或 Windows 非法字符。
    InvalidChar(char),
}

impl fmt::Display for AssetPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "资源路径为空"),
            Self::Absolute => write!(f, "资源路径不得是绝对路径"),
            Self::HasScheme => write!(f, "资源路径不得含盘符或 URL scheme"),
            Self::ParentEscape => write!(f, "资源路径不得用 `..` 越出资源根"),
            Self::InvalidChar(c) => write!(f, "资源路径含非法字符 {c:?}"),
        }
    }
}

impl std::error::Error for AssetPathError {}

/// 规范化后的资源相对路径。内部恒为 `/` 分隔、无 `.`/`..`/重复分隔符。
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetPath(Arc<str>);

impl AssetPath {
    /// 规范化并校验。反斜杠会被折叠为正斜杠。
    pub fn new(raw: &str) -> Result<Self, AssetPathError> {
        if raw.trim().is_empty() {
            return Err(AssetPathError::Empty);
        }
        if raw.starts_with('/') || raw.starts_with('\\') {
            return Err(AssetPathError::Absolute);
        }
        let mut segs: Vec<&str> = Vec::new();
        for seg in raw.split(['/', '\\']) {
            match seg {
                "" | "." => continue,
                ".." => {
                    if segs.pop().is_none() {
                        return Err(AssetPathError::ParentEscape);
                    }
                }
                s => {
                    if let Some(c) = s.chars().find(|c| c.is_control()) {
                        return Err(AssetPathError::InvalidChar(c));
                    }
                    // `C:` / `http:` 这类带冒号的段一律拒绝（Windows 下冒号也是非法文件名）。
                    if s.contains(':') {
                        return Err(AssetPathError::HasScheme);
                    }
                    segs.push(s);
                }
            }
        }
        if segs.is_empty() {
            return Err(AssetPathError::Empty);
        }
        Ok(Self(Arc::from(segs.join("/"))))
    }

    /// 规范化后的字符串（`/` 分隔）。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 最后一段。
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// 扩展名（不含点），小写比较由调用方决定。
    pub fn extension(&self) -> Option<&str> {
        self.file_name().rsplit_once('.').map(|(_, e)| e)
    }

    /// 父目录路径。
    pub fn parent(&self) -> Option<Self> {
        self.0.rsplit_once('/').map(|(p, _)| Self(Arc::from(p)))
    }

    /// 追加一段，仍走完整校验。
    ///
    /// 这里**不接受** `..`：追加语义下出现上跳几乎总是调用方写错了，与其静默折叠
    /// （`Textures` + `../a.png` 变成 `a.png`，看起来像"成功"），不如直接报错。
    /// [`AssetPath::new`] 仍按标准语义折叠**不越界**的内部 `..`。
    pub fn join(&self, seg: &str) -> Result<Self, AssetPathError> {
        if seg.split(['/', '\\']).any(|s| s == "..") {
            return Err(AssetPathError::ParentEscape);
        }
        let mut s = String::with_capacity(self.0.len() + seg.len() + 1);
        s.push_str(&self.0);
        s.push('/');
        s.push_str(seg);
        Self::new(&s)
    }
}

impl fmt::Display for AssetPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for AssetPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssetPath({:?})", &*self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_separators_and_dots() {
        let p = AssetPath::new("Textures\\ui/./icon.png").unwrap();
        assert_eq!(p.as_str(), "Textures/ui/icon.png");
        assert_eq!(p.file_name(), "icon.png");
        assert_eq!(p.extension(), Some("png"));
        assert_eq!(p.parent().unwrap().as_str(), "Textures/ui");
    }

    #[test]
    fn rejects_escape_and_absolute() {
        assert_eq!(AssetPath::new("../secret.png"), Err(AssetPathError::ParentEscape));
        assert_eq!(AssetPath::new("a/../../b.png"), Err(AssetPathError::ParentEscape));
        assert_eq!(AssetPath::new("/etc/passwd"), Err(AssetPathError::Absolute));
        assert_eq!(AssetPath::new("C:/Windows/x.png"), Err(AssetPathError::HasScheme));
        assert_eq!(AssetPath::new("   "), Err(AssetPathError::Empty));
        assert_eq!(AssetPath::new("./"), Err(AssetPathError::Empty));
    }

    #[test]
    fn inner_parent_is_collapsed() {
        assert_eq!(AssetPath::new("a/b/../c.png").unwrap().as_str(), "a/c.png");
    }

    #[test]
    fn join_is_validated() {
        let p = AssetPath::new("Textures").unwrap();
        assert_eq!(p.join("a.png").unwrap().as_str(), "Textures/a.png");
        assert_eq!(p.join("ui/icon.png").unwrap().as_str(), "Textures/ui/icon.png");
        // join 语义下 `..` 一律拒绝（静默折叠成 `a.png` 才是危险的）。
        assert_eq!(p.join("../a.png"), Err(AssetPathError::ParentEscape));
        assert_eq!(p.join("ui/../a.png"), Err(AssetPathError::ParentEscape));
        // 而 new() 仍按标准语义折叠不越界的内部 `..`。
        assert_eq!(
            AssetPath::new("Textures/ui/../a.png").unwrap().as_str(),
            "Textures/a.png"
        );
    }
}
