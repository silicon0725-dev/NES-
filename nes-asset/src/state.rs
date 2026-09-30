//! 加载状态机。
//!
//! ```text
//! NotLoaded → Queued → Loading → Ready
//!                        ↓
//!                     Failed → (retry) → Loading
//! Ready → Loading            （热重载，key 不变、version 递增）
//! 任意态 → NotLoaded          （卸载）
//! ```
//!
//! 其它转移一律 [`StateError`]。把状态机做成显式可校验对象，是为了让
//! "Queued 里直接变 Ready"、"Failed 里直接变 Ready" 这类静默错误在测试里炸出来。

use core::fmt;
use std::sync::Arc;

use crate::data::LoadedAsset;

/// 状态标签（不含数据），用于转移校验与日志。
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum StateTag {
    /// 未加载。
    NotLoaded,
    /// 已排队（将来接异步线程池的挂载点）。
    Queued,
    /// 加载中。
    Loading,
    /// 就绪。
    Ready,
    /// 失败（可重试）。
    Failed,
}

impl StateTag {
    /// 稳定字符串名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotLoaded => "NotLoaded",
            Self::Queued => "Queued",
            Self::Loading => "Loading",
            Self::Ready => "Ready",
            Self::Failed => "Failed",
        }
    }
}

/// 加载状态。
#[derive(Clone, Debug)]
pub enum LoadState {
    /// 未加载。
    NotLoaded,
    /// 已排队。
    Queued,
    /// 加载中。
    Loading,
    /// 就绪。持有不可变资产数据（`Arc` 克隆廉价，可安全外发给渲染/音频侧）。
    Ready(Arc<LoadedAsset>),
    /// 失败。保留原因供编辑器展示，可重试。
    Failed(Arc<str>),
}

impl LoadState {
    /// 状态标签。
    pub fn tag(&self) -> StateTag {
        match self {
            Self::NotLoaded => StateTag::NotLoaded,
            Self::Queued => StateTag::Queued,
            Self::Loading => StateTag::Loading,
            Self::Ready(_) => StateTag::Ready,
            Self::Failed(_) => StateTag::Failed,
        }
    }

    /// 是否就绪。
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    /// 取已加载数据。
    pub fn loaded(&self) -> Option<&Arc<LoadedAsset>> {
        match self {
            Self::Ready(a) => Some(a),
            _ => None,
        }
    }

    /// 失败原因。
    pub fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed(r) => Some(r),
            _ => None,
        }
    }

    /// 转移是否合法。
    pub fn can_advance_to(&self, next: StateTag) -> bool {
        use StateTag::*;
        match (self.tag(), next) {
            // 卸载：任意态都可回到 NotLoaded。
            (_, NotLoaded) => true,
            // 加载主链。
            (NotLoaded, Queued) => true,
            (Queued, Loading) => true,
            (Loading, Ready) | (Loading, Failed) => true,
            // 失败重试。
            (Failed, Loading) => true,
            // 热重载：就绪态回到 Loading（key 不变，version 由注册表递增）。
            (Ready, Loading) => true,
            // 幂等自转移（重复 load 同一状态不应报错）。
            (a, b) if a == b => true,
            _ => false,
        }
    }

    /// 校验并返回 `next` 的标签，非法转移返回 [`StateError`]。
    pub fn validate(&self, next: StateTag) -> Result<StateTag, StateError> {
        if self.can_advance_to(next) {
            Ok(next)
        } else {
            Err(StateError {
                from: self.tag(),
                to: next,
            })
        }
    }
}

/// 非法状态转移。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StateError {
    /// 来源状态。
    pub from: StateTag,
    /// 目标状态。
    pub to: StateTag,
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "非法加载状态转移：{} → {}", self.from.as_str(), self.to.as_str())
    }
}

impl std::error::Error for StateError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_chain_is_legal() {
        let s = LoadState::NotLoaded;
        assert_eq!(s.validate(StateTag::Queued), Ok(StateTag::Queued));
        let s = LoadState::Queued;
        assert_eq!(s.validate(StateTag::Loading), Ok(StateTag::Loading));
        let s = LoadState::Loading;
        assert_eq!(s.validate(StateTag::Ready), Ok(StateTag::Ready));
        assert_eq!(s.validate(StateTag::Failed), Ok(StateTag::Failed));
    }

    #[test]
    fn shortcuts_are_rejected() {
        let s = LoadState::NotLoaded;
        assert!(s.validate(StateTag::Ready).is_err());
        assert!(s.validate(StateTag::Loading).is_err());
        let f = LoadState::Failed(Arc::from("boom"));
        assert!(f.validate(StateTag::Ready).is_err());
        assert_eq!(f.validate(StateTag::Loading), Ok(StateTag::Loading));
        assert_eq!(f.failure(), Some("boom"));
    }

    #[test]
    fn hot_reload_path_is_legal() {
        // Ready → Loading 是热重载的官方通道（key 不变、version 递增）。
        let asset = Arc::new(LoadedAsset::new(
            crate::key::AssetKey::new(1, 0, crate::key::AssetKind::Texture),
            crate::path::AssetPath::new("a.png").unwrap(),
            1,
            Arc::from(Vec::<u8>::new().into_boxed_slice()),
            crate::data::Stamp::default(),
        ));
        let ready = LoadState::Ready(asset);
        assert!(ready.can_advance_to(StateTag::Loading));
        assert!(ready.can_advance_to(StateTag::NotLoaded));
        assert!(!ready.can_advance_to(StateTag::Queued));
        assert!(ready.is_ready());
        assert_eq!(ready.loaded().unwrap().version, 1);
    }

    #[test]
    fn unload_is_always_allowed() {
        // 状态表：任意状态 → NotLoaded（卸载）都为真。
        assert!(LoadState::NotLoaded.can_advance_to(StateTag::NotLoaded));
        assert!(LoadState::Queued.can_advance_to(StateTag::NotLoaded));
        assert!(LoadState::Loading.can_advance_to(StateTag::NotLoaded));
        assert!(LoadState::Failed(Arc::from("x")).can_advance_to(StateTag::NotLoaded));
    }
}
