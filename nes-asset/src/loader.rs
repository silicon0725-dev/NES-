//! 加载后端：把"字节从哪来"与"身份/状态怎么管"彻底分开。
//!
//! [`AssetRegistry`](crate::AssetRegistry) 只认 [`AssetLoader`] trait，
//! 因此 M4 接真实解码器、或将来接异步线程池，都不需要动注册表一行代码。
//!
//! 刻意**只做 stamp + read 两个动作**：热重载轮询先问 `stamp`（廉价），
//! 只有真变了才 `read` 全量字节。否则大资源目录每轮都要重读一遍磁盘。

use core::fmt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::data::{fnv1a64, Stamp};
use crate::key::AssetKey;
use crate::path::AssetPath;

/// 加载错误。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LoadError {
    /// 源不存在。
    NotFound(AssetPath),
    /// IO 失败。
    Io(AssetPath, Arc<str>),
    /// 解码/格式失败（M3 只透传，不实现解码）。
    Decode(AssetPath, Arc<str>),
    /// 键不在注册表里（或已悬垂）。
    UnknownKey(AssetKey),
    /// 非法状态转移（正常流程不会出现，出现即为实现缺陷，却在运行期可观测）。
    State(AssetPath, Arc<str>),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "资源不存在：{p}"),
            Self::Io(p, e) => write!(f, "资源读取失败：{p}（{e}）"),
            Self::Decode(p, e) => write!(f, "资源解码失败：{p}（{e}）"),
            Self::UnknownKey(k) => write!(f, "未知资源键：{k:?}"),
            Self::State(p, e) => write!(f, "资源状态异常：{p}（{e}）"),
        }
    }
}

impl std::error::Error for LoadError {}

/// 加载后端。
pub trait AssetLoader {
    /// 廉价地取内容戳（不读全量内容，若后端没得更廉价的判定就退化为读取）。
    fn stamp(&mut self, path: &AssetPath) -> Result<Stamp, LoadError>;

    /// 读取全量字节与内容戳。
    fn read(&mut self, path: &AssetPath) -> Result<(Arc<[u8]>, Stamp), LoadError>;
}

/// 文件系统加载器：`root` 为资源根，`AssetPath` 相对它解析。
///
/// 路径已在 [`AssetPath`] 层拒绝 `..` / 绝对路径 / 盘符，因此这里不存在
/// 逃出 `root` 的可能。
pub struct FsLoader {
    root: PathBuf,
}

impl FsLoader {
    /// 以资源根构造。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 资源根。
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    fn resolve(&self, path: &AssetPath) -> PathBuf {
        self.root.join(path.as_str())
    }
}

fn mtime_ms(path: &std::path::Path) -> std::io::Result<u64> {
    let meta = std::fs::metadata(path)?;
    let modified = meta.modified()?;
    Ok(modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0))
}

impl AssetLoader for FsLoader {
    fn stamp(&mut self, path: &AssetPath) -> Result<Stamp, LoadError> {
        let full = self.resolve(path);
        if !full.exists() {
            return Err(LoadError::NotFound(path.clone()));
        }
        let revision = mtime_ms(&full).map_err(|e| LoadError::Io(path.clone(), Arc::from(e.to_string())))?;
        // mtime 精度在部分文件系统上只有秒级，为稳妥仍读一次内容算哈希：
        // 这是"正确优先于极限性能"的取舍，M4 若需要可换成后端提供的 stamp。
        let bytes = std::fs::read(&full).map_err(|e| LoadError::Io(path.clone(), Arc::from(e.to_string())))?;
        Ok(Stamp::of(revision, &bytes))
    }

    fn read(&mut self, path: &AssetPath) -> Result<(Arc<[u8]>, Stamp), LoadError> {
        let full = self.resolve(path);
        let bytes = std::fs::read(&full).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LoadError::NotFound(path.clone())
            } else {
                LoadError::Io(path.clone(), Arc::from(e.to_string()))
            }
        })?;
        let revision = mtime_ms(&full).unwrap_or(0);
        let stamp = Stamp::of(revision, &bytes);
        Ok((Arc::from(bytes.into_boxed_slice()), stamp))
    }
}

/// 内存加载器：测试与嵌入式资源用。
///
/// `revision` 由写入显式递增，因此**不需要**依赖真实时钟，
/// 热重载测试可以在同一毫秒内完成。
pub struct MemoryLoader {
    files: HashMap<String, (Arc<[u8]>, u64)>,
    clock: u64,
}

impl MemoryLoader {
    /// 空加载器。
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            clock: 0,
        }
    }

    /// 写入或覆盖（`revision` 递增）。
    pub fn write(&mut self, path: &str, bytes: impl Into<Vec<u8>>) {
        self.clock += 1;
        let rev = self.clock;
        self.files
            .insert(path.to_string(), (Arc::from(bytes.into().into_boxed_slice()), rev));
    }

    /// 只改 revision、内容不变（模拟"touch"）。
    pub fn touch(&mut self, path: &str) -> bool {
        self.clock += 1;
        match self.files.get_mut(path) {
            Some(slot) => {
                slot.1 = self.clock;
                true
            }
            None => false,
        }
    }

    /// 删除。
    pub fn remove(&mut self, path: &str) -> bool {
        self.clock += 1;
        self.files.remove(path).is_some()
    }

    /// 当前文件数。
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl Default for MemoryLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetLoader for MemoryLoader {
    fn stamp(&mut self, path: &AssetPath) -> Result<Stamp, LoadError> {
        match self.files.get(path.as_str()) {
            Some((bytes, rev)) => Ok(Stamp::of(*rev, bytes)),
            None => Err(LoadError::NotFound(path.clone())),
        }
    }

    fn read(&mut self, path: &AssetPath) -> Result<(Arc<[u8]>, Stamp), LoadError> {
        match self.files.get(path.as_str()) {
            Some((bytes, rev)) => Ok((Arc::clone(bytes), Stamp::of(*rev, bytes))),
            None => Err(LoadError::NotFound(path.clone())),
        }
    }
}

/// 便捷：内容哈希。
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    fnv1a64(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_loader_revision_changes_on_write_only() {
        let mut l = MemoryLoader::new();
        l.write("a.png", b"one".to_vec());
        let p = AssetPath::new("a.png").unwrap();
        let s1 = l.stamp(&p).unwrap();
        let s2 = l.stamp(&p).unwrap();
        assert_eq!(s1, s2, "读取不应改变戳");
        l.write("a.png", b"one".to_vec());
        let s3 = l.stamp(&p).unwrap();
        assert_ne!(s1.revision, s3.revision, "写入必须换戳");
        assert_eq!(s1.hash, s3.hash, "内容相同则哈希相同");
    }

    #[test]
    fn memory_loader_missing_is_not_found() {
        let mut l = MemoryLoader::new();
        let p = AssetPath::new("nope.png").unwrap();
        assert_eq!(l.stamp(&p), Err(LoadError::NotFound(p.clone())));
        assert_eq!(l.read(&p), Err(LoadError::NotFound(p)));
    }
}
