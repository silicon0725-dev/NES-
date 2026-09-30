//! 提取层对资源侧的**唯一**需求：这个东西现在有没有稳定资源键。
//!
//! 提取层不直接写死 `nes-asset`（G6 也把依赖收窄到两条 path 依赖），
//! 因此资源键的读取走这个最小接口：
//!
//! - 生产路径：`nes-scene` 的 [`ResourceTable`]（零分配，逐条目直读）；
//! - 测试路径：内存替身（想验"资源消失 / 换代"时不必真去建资源表）。
//!
//! 换来的好处不只是"好测"：**"资源能不能渲染"这个问题被显式化了**。
//! 在场景层，这个问题散在 `render_key()` 的 `Option` 里；到了这里它就是
//! 提取层的准入条件 —— `None` 一律不建渲染物，而不是"建了再修"。

use nes_render_api::RenderAssetKey;
use nes_scene::{ResId, ResourceTable};

use crate::bridge::render_key_of_bits;

/// 资源键来源（提取层与资源系统之间的最小接口）。
pub trait RenderKeySource {
    /// 该资源槽位当前的渲染侧稳定键。
    ///
    /// `None` 表示当前**没有可渲染的资源**：未绑定、已悬垂、非渲染类别
    /// （非纹理）、或槽位已被回收。提取层的处理是"不渲染"，**不是**报错。
    fn render_key(&self, id: ResId) -> Option<RenderAssetKey>;

    /// 便利版：有键、且键非空（`slot != 0`）才算可渲染。
    ///
    /// 空键（`RenderAssetKey::NIL`）与"没有键"在提取层是同一个后果：
    /// 不建渲染物，已有渲染物按"资源消失"处理。
    fn renderable_key(&self, id: ResId) -> Option<RenderAssetKey> {
        self.render_key(id).filter(|key| !key.is_nil())
    }
}

/// 生产路径：直接读 [`ResourceTable`] 的条目。
///
/// `entry(id)` 返回 `None` 表示该槽位当前不存在（已回收 / 从未声明）；
/// 条目的 `render_key()` 只在资源**已绑定且属于渲染类别**时给出键。
/// 全程只是读引用与复制 `u64`，不在热路径上分配。
impl RenderKeySource for ResourceTable {
    fn render_key(&self, id: ResId) -> Option<RenderAssetKey> {
        let bits = self.entry(id)?.render_key()?.to_bits();
        Some(render_key_of_bits(bits))
    }
}
