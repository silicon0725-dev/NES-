//! NES 2.0 全局资源注册表 —— **M3**。
//!
//! 对应设计文档：`NES2.0_节点场景树_接口草案_v1.md` 第 11 节。
//!
//! # M3 出口准则（可验证）
//!
//! 1. **改文件后订阅者收到同 key 新版本**：`AssetRegistry::poll_reloads` 检出源变化
//!    → 重新加载**同一个** [`AssetKey`] → `version` 递增 → `AssetEvent::Reloaded`
//!    投递给订阅者（见 `tests/m3.rs::reload_broadcasts_same_key_new_version`）。
//! 2. **引用计数与依赖卸载判定正确**：`acquire` / `release` 维护计数；`release` 到 0
//!    只是**入队**，真正卸载发生在 `unload_tick`，且**任何仍被存活者依赖的资源不卸载**
//!    （见 `tests/m3.rs` 的依赖与卸载三例）。
//!
//! # 三条纪律（与 TWN 既有资产同构）
//!
//! 1. **双身份**：[`AssetKey`] 是稳定身份（进存档、进脚本引用、进依赖图），
//!    [`LoadedAsset`] 里的字节是易变内容；热重载只让 `version` 递增，key 永不变。
//!    `RenderAssetKeyView` 是 `twn-render-resources::RenderAssetKey` 的**镜像视图**，
//!    二者共用同一份 `(slot, gen)` 位编码，避免出现两套资源身份。
//! 2. **状态机单向**：[`LoadState`] 只允许
//!    `NotLoaded → Queued → Loading → Ready|Failed`，`Failed` 可重试回 `Loading`，
//!    `Ready` 可因热重载回到 `Loading`；其余转移一律报 [`StateError`]。
//! 3. **不无引用即卸**：`refs == 0` 只是候选资格，卸载统一延迟到 `unload_tick`，
//!    避免加载/卸载在同一帧反复抖动。
//!
//! # 未做（有意留白）
//!
//! - 不做异步加载线程（`Queued` / `Loading` 已就位，将来接入线程池只需实现
//!   [`AssetLoader`] 的非阻塞版本，状态机不变）；
//! - 不做解码（png/ogg 解码属 M4 后端范畴，本 crate 只负责字节与身份）；
//! - 不建反向依赖索引（`dependents` 由 `deps` 扫描得出），规模小时确定性优先，
//!   避免两份事实来源漂移。

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

pub mod data;
pub mod event;
pub mod key;
pub mod loader;
pub mod path;
pub mod registry;
pub mod state;

pub use data::{fnv1a64, LoadedAsset, Stamp};
pub use event::{AssetEvent, Delivery, EventScope, SubscriberId};
pub use key::{AssetKey, AssetKind, RenderAssetKeyView};
pub use loader::{AssetLoader, FsLoader, LoadError, MemoryLoader};
pub use path::{AssetPath, AssetPathError};
pub use registry::{AssetRegistry, DepError, RegistryIter, ReloadReport};
pub use state::{LoadState, StateError, StateTag};
