//! 已加载资产与内容戳。

use core::fmt;
use std::sync::Arc;

use crate::key::AssetKey;
use crate::path::AssetPath;

/// 内容戳：判定"文件是否变了"的唯一依据。
///
/// - `revision`：文件系统下是 mtime 毫秒；内存加载器下是显式修订号。
/// - `len`：字节数。挡住"mtime 精度不够 + 长度不变"以外的大多数误判。
/// - `hash`：FNV-1a 64 内容哈希。挡住"同秒内改写且长度不变"。
///
/// 三者合起来做**相等比较**，任一不同即视为需要重载。刻意不做"mtime 更新就一定重载"
/// 的粗糙判断，否则编辑器保存一次就会让整棵资源树抖动。
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Stamp {
    /// 修订号 / mtime 毫秒。
    pub revision: u64,
    /// 字节数。
    pub len: u64,
    /// 内容哈希。
    pub hash: u64,
}

impl Stamp {
    /// 由修订号与内容构造。
    pub fn of(revision: u64, bytes: &[u8]) -> Self {
        Self {
            revision,
            len: bytes.len() as u64,
            hash: fnv1a64(bytes),
        }
    }
}

/// FNV-1a 64。选它而不是更强哈希的理由：零依赖、确定性、几行实现，
/// 用于"内容是否变化"的判定足够；它**不是**安全哈希，不得用于防篡改。
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// 已加载资产（不可变）。热重载产生**新的** `LoadedAsset`（新 `version`），
/// 旧 `Arc` 持有者继续用旧数据，直到它自己换成新的——不做原地改写，
/// 这样渲染线程不会被撕裂读。
#[derive(Clone)]
pub struct LoadedAsset {
    /// 稳定身份。
    pub key: AssetKey,
    /// 源路径。
    pub path: AssetPath,
    /// 版本。首次加载为 1，每次热重载 +1。
    pub version: u32,
    /// 原始字节。
    pub bytes: Arc<[u8]>,
    /// 内容戳。
    pub stamp: Stamp,
}

impl LoadedAsset {
    /// 构造。
    pub fn new(key: AssetKey, path: AssetPath, version: u32, bytes: Arc<[u8]>, stamp: Stamp) -> Self {
        Self {
            key,
            path,
            version,
            bytes,
            stamp,
        }
    }

    /// 字节数。
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// 是否空内容。
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// 内容哈希。
    pub fn hash(&self) -> u64 {
        self.stamp.hash
    }
}

impl fmt::Debug for LoadedAsset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadedAsset")
            .field("key", &self.key)
            .field("path", &self.path)
            .field("version", &self.version)
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_deterministic_and_sensitive() {
        assert_eq!(fnv1a64(b"abc"), fnv1a64(b"abc"));
        assert_ne!(fnv1a64(b"abc"), fnv1a64(b"abd"));
        assert_ne!(fnv1a64(b""), fnv1a64(b"x"));
    }

    #[test]
    fn stamp_tracks_len_and_hash() {
        let a = Stamp::of(1, b"ab");
        let b = Stamp::of(1, b"ab");
        let c = Stamp::of(2, b"ab");
        let d = Stamp::of(1, b"abc");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_eq!(a.len, 2);
    }
}
