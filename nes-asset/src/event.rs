//! 资源事件与订阅。
//!
//! 纪律与 `nes-scene` 的 `SignalBus` 一致：事件**入队**，由调用方在帧末
//! [`AssetRegistry::dispatch`](crate::AssetRegistry::dispatch) 统一派发，
//! 禁止在加载/重载过程中直接回调订阅者（否则可重入，且加载顺序会影响观测顺序）。

use core::fmt;

use crate::key::{AssetKey, AssetKind};

/// 订阅者身份。
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SubscriberId(u64);

impl SubscriberId {
    /// 序号。
    pub const fn index(self) -> u64 {
        self.0
    }

    pub(crate) const fn new(index: u64) -> Self {
        Self(index)
    }
}

/// 订阅范围。
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum EventScope {
    /// 全部资源。
    All,
    /// 指定分类的全部资源（编辑器里"纹理目录变了就刷新预览"用这个）。
    Kind(AssetKind),
    /// 单个资源。
    Key(AssetKey),
}

impl EventScope {
    /// 事件是否命中该范围。
    pub fn matches(&self, event: &AssetEvent) -> bool {
        match self {
            Self::All => true,
            Self::Kind(k) => event.key().kind() == *k,
            Self::Key(k) => event.key() == *k,
        }
    }
}

/// 资源事件。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AssetEvent {
    /// 首次加载完成。
    Loaded {
        /// 资源键。
        key: AssetKey,
        /// 版本（首次为 1）。
        version: u32,
    },
    /// 加载失败。
    Failed {
        /// 资源键。
        key: AssetKey,
        /// 版本（失败时保留原版本号）。
        version: u32,
        /// 原因。
        reason: String,
    },
    /// 热重载完成。**key 不变，version 递增** —— 这是 M3 出口准则 1 的可观测面。
    Reloaded {
        /// 资源键。
        key: AssetKey,
        /// 新版本。
        version: u32,
    },
    /// 已卸载。
    Unloaded {
        /// 资源键。
        key: AssetKey,
    },
}

impl AssetEvent {
    /// 事件涉及的资源键。
    pub fn key(&self) -> AssetKey {
        match self {
            Self::Loaded { key, .. }
            | Self::Failed { key, .. }
            | Self::Reloaded { key, .. }
            | Self::Unloaded { key } => *key,
        }
    }

    /// 事件涉及的版本（`Unloaded` 无版本，返回 `None`）。
    pub fn version(&self) -> Option<u32> {
        match self {
            Self::Loaded { version, .. } | Self::Failed { version, .. } | Self::Reloaded { version, .. } => {
                Some(*version)
            }
            Self::Unloaded { .. } => None,
        }
    }

    /// 稳定标签，用于日志与测试断言。
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Loaded { .. } => "Loaded",
            Self::Failed { .. } => "Failed",
            Self::Reloaded { .. } => "Reloaded",
            Self::Unloaded { .. } => "Unloaded",
        }
    }
}

impl fmt::Display for AssetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loaded { key, version } => write!(f, "Loaded {key:?} v{version}"),
            Self::Failed { key, version, reason } => {
                write!(f, "Failed {key:?} v{version}（{reason}）")
            }
            Self::Reloaded { key, version } => write!(f, "Reloaded {key:?} v{version}"),
            Self::Unloaded { key } => write!(f, "Unloaded {key:?}"),
        }
    }
}

/// 一次派发记录。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Delivery {
    /// 收件订阅者。
    pub subscriber: SubscriberId,
    /// 事件。
    pub event: AssetEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(slot: u32) -> AssetKey {
        crate::key::AssetKey::new(slot, 0, AssetKind::Texture)
    }

    #[test]
    fn scope_matching() {
        let e = AssetEvent::Reloaded {
            key: tex(3),
            version: 2,
        };
        assert!(EventScope::All.matches(&e));
        assert!(EventScope::Kind(AssetKind::Texture).matches(&e));
        assert!(!EventScope::Kind(AssetKind::Audio).matches(&e));
        assert!(EventScope::Key(tex(3)).matches(&e));
        assert!(!EventScope::Key(tex(4)).matches(&e));
    }

    #[test]
    fn event_reports_key_and_version() {
        let e = AssetEvent::Unloaded { key: tex(1) };
        assert_eq!(e.key(), tex(1));
        assert_eq!(e.version(), None);
        assert_eq!(e.tag(), "Unloaded");
    }
}
