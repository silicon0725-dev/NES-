//! [`NullRenderServer`]：headless 空实现 —— S1「空实现编译通过」出口准则的载体。
//!
//! 它做三件事，都是后端契约的可执行参照：
//!
//! 1. 按 [`RenderServer`] 的不变式生成**确定性命令流**（可当"后端该怎么接"的样例）；
//! 2. 把副作用真的落在内存里（`items` / `labels` / `rects` / `clips` /
//!    `tints` / `uvs` / `pivots` / `camera`），
//!    让测试能断言"推送被正确保存"，而不只是"没 panic"；
//! 3. 记计数器（创建/销毁/被忽略的操作/帧数/命令数），把"空句柄被忽略"
//!    这类静默行为变成**可观测**的事实 —— 静默而不可观测的吞错最难查。

use std::collections::BTreeMap;

use crate::command::{FrameInfo, RenderCommand};
use crate::handle::{ItemHandle, RenderAssetKey};
use crate::item::RenderItem;
use crate::math::{Affine2, Rect};
use crate::server::RenderServer;
use crate::state::{Camera2DState, ControlState, Flip, LabelState, ListState};

/// 服务端行为计数器（把静默行为显式化）。
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct ServerCounters {
    /// 成功创建的渲染物数。
    pub created: u64,
    /// 成功销毁的渲染物数。
    pub destroyed: u64,
    /// 因空句柄 / 未知句柄而被忽略的操作数。
    pub ignored_ops: u64,
    /// `submit` 次数。
    pub frames: u64,
    /// 累计产生的命令条数。
    pub commands: u64,
}

/// 无 GPU、无窗口、无依赖的空渲染服务端。
///
/// 用途：① S1 出口准则的"空实现编译通过"；② 提取层（S2）的测试替身；
/// ③ 后端实现者的参照样例（顺序、忽略规则、快照语义都在这里可读可跑）。
#[derive(Debug, Default)]
pub struct NullRenderServer {
    next_handle: u64,
    items: BTreeMap<ItemHandle, RenderItem>,
    labels: BTreeMap<ItemHandle, LabelState>,
    /// 列表/页签簿记（S12-3 任务 4）：`set_list` 存、销毁移除。
    lists: BTreeMap<ItemHandle, ListState>,
    rects: BTreeMap<ItemHandle, ControlState>,
    /// 裁剪簿记（E-2 / D1）：`Some(rect)` 存、`None`/销毁移除。
    /// 有裁剪的条目在 `submit_into` 输出序里于 `SetRect` 之后追加 `SetClip`。
    clips: BTreeMap<ItemHandle, Rect>,
    /// 相乘色簿记（S16.1 alpha 通道）：`set_tint` 存（同键覆写）、销毁移除。
    /// 有 tint 的条目在 `submit_into` 输出序里于 `SetClip` 之后追加 `SetTint`。
    tints: BTreeMap<ItemHandle, [u8; 4]>,
    /// 子矩形采样簿记（S16.2 图集帧动画）：`set_uv` 存（同键覆写）、销毁移除。
    /// 有 uv 的条目在 `submit_into` 输出序里于 `SetTint` 之后追加 `SetUv`。
    uvs: BTreeMap<ItemHandle, [f32; 4]>,
    /// 精灵锚点簿记（S16.3）：`set_pivot` 存（同键覆写）、销毁移除。
    /// 有 pivot 的条目在 `submit_into` 输出序里于 `SetUv` 之后追加 `SetPivot`。
    pivots: BTreeMap<ItemHandle, [f32; 2]>,
    camera: Option<Camera2DState>,
    lifecycle: Vec<RenderCommand>,
    counters: ServerCounters,
}

impl NullRenderServer {
    /// 新建（无渲染物、无相机）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 全部存活渲染物（按句柄有序）。
    pub fn items(&self) -> &BTreeMap<ItemHandle, RenderItem> {
        &self.items
    }

    /// 存活渲染物数量。
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否没有任何渲染物。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 取单个渲染物。
    pub fn item(&self, handle: ItemHandle) -> Option<&RenderItem> {
        self.items.get(&handle)
    }

    /// 取渲染物的文本状态（非 Label 类返回 `None`）。
    pub fn label_of(&self, handle: ItemHandle) -> Option<&LabelState> {
        self.labels.get(&handle)
    }

    /// 取渲染物的列表/页签状态（非 List 类返回 `None`）。
    pub fn list_of(&self, handle: ItemHandle) -> Option<&ListState> {
        self.lists.get(&handle)
    }

    /// 取渲染物的控件布局状态。
    pub fn rect_of(&self, handle: ItemHandle) -> Option<&ControlState> {
        self.rects.get(&handle)
    }

    /// 取渲染物的裁剪矩形（未设置返回 `None`）。
    pub fn clip_of(&self, handle: ItemHandle) -> Option<&Rect> {
        self.clips.get(&handle)
    }

    /// 取渲染物的相乘色（S16.1；未设置返回 `None` —— 无记录 = 中性恒等）。
    pub fn tint_of(&self, handle: ItemHandle) -> Option<&[u8; 4]> {
        self.tints.get(&handle)
    }

    /// 取渲染物的子矩形采样（S16.2；未设置返回 `None` —— 无记录 = 整瓦片）。
    pub fn uv_of(&self, handle: ItemHandle) -> Option<&[f32; 4]> {
        self.uvs.get(&handle)
    }

    /// 取渲染物的精灵锚点（S16.3；未设置返回 `None` —— 无记录 = 无平移）。
    pub fn pivot_of(&self, handle: ItemHandle) -> Option<&[f32; 2]> {
        self.pivots.get(&handle)
    }

    /// 当前相机。
    pub fn camera(&self) -> Option<&Camera2DState> {
        self.camera.as_ref()
    }

    /// 计数器快照。
    pub fn counters(&self) -> ServerCounters {
        self.counters
    }

    /// 按 [`DrawKey`](crate::DrawKey) 升序的句柄序列 —— 即后端应当采纳的绘制次序。
    pub fn draw_order(&self) -> Vec<ItemHandle> {
        let mut items: Vec<&RenderItem> = self.items.values().collect();
        items.sort_by_key(|item| item.draw_key());
        items.into_iter().map(|item| item.handle).collect()
    }
}

impl RenderServer for NullRenderServer {
    fn create_item(&mut self, key: RenderAssetKey) -> ItemHandle {
        self.next_handle += 1;
        let handle = ItemHandle::from_raw(self.next_handle);
        self.items
            .insert(handle, RenderItem::new(handle, key, Affine2::IDENTITY));
        self.lifecycle.push(RenderCommand::CreateItem { handle, key });
        self.counters.created += 1;
        handle
    }

    fn destroy_item(&mut self, handle: ItemHandle) {
        if handle.is_nil() || self.items.remove(&handle).is_none() {
            self.counters.ignored_ops += 1;
            return;
        }
        self.labels.remove(&handle);
        self.lists.remove(&handle);
        self.rects.remove(&handle);
        self.clips.remove(&handle);
        self.tints.remove(&handle);
        self.uvs.remove(&handle);
        self.pivots.remove(&handle);
        self.lifecycle.push(RenderCommand::DestroyItem { handle });
        self.counters.destroyed += 1;
    }

    fn set_visible(&mut self, handle: ItemHandle, visible: bool) {
        match self.items.get_mut(&handle) {
            Some(item) => item.visible = visible,
            None => self.counters.ignored_ops += 1,
        }
    }

    fn set_transform(&mut self, handle: ItemHandle, transform: Affine2) {
        match self.items.get_mut(&handle) {
            Some(item) => item.transform = transform,
            None => self.counters.ignored_ops += 1,
        }
    }

    fn set_z(&mut self, handle: ItemHandle, z: i32, order: u64) {
        match self.items.get_mut(&handle) {
            Some(item) => {
                item.z = z;
                item.order = order;
            }
            None => self.counters.ignored_ops += 1,
        }
    }

    fn set_flip(&mut self, handle: ItemHandle, flip: Flip) {
        match self.items.get_mut(&handle) {
            Some(item) => item.flip = flip,
            None => self.counters.ignored_ops += 1,
        }
    }

    fn set_camera(&mut self, camera: &Camera2DState) {
        self.camera = Some(*camera);
    }

    fn set_text(&mut self, handle: ItemHandle, text: &LabelState) {
        if self.items.contains_key(&handle) {
            self.labels.insert(handle, text.clone());
        } else {
            self.counters.ignored_ops += 1;
        }
    }

    fn set_list(&mut self, handle: ItemHandle, rows: &ListState) {
        if self.items.contains_key(&handle) {
            self.lists.insert(handle, rows.clone());
        } else {
            self.counters.ignored_ops += 1;
        }
    }

    fn set_rect(&mut self, handle: ItemHandle, rect: &ControlState) {
        if self.items.contains_key(&handle) {
            self.rects.insert(handle, *rect);
        } else {
            self.counters.ignored_ops += 1;
        }
    }

    fn set_clip(&mut self, handle: ItemHandle, rect: Option<Rect>) {
        if !self.items.contains_key(&handle) {
            // 空句柄 / 未知句柄：静默忽略（契约 I1），计数器使其可观测。
            self.counters.ignored_ops += 1;
            return;
        }
        match rect {
            Some(rect) => {
                self.clips.insert(handle, rect);
            }
            None => {
                // `None` = 清除裁剪（本来就没有也是合法的清除）。
                self.clips.remove(&handle);
            }
        }
    }

    fn set_tint(&mut self, handle: ItemHandle, rgba: [u8; 4]) {
        if !self.items.contains_key(&handle) {
            // 空句柄 / 未知句柄：静默忽略（契约 I1），计数器使其可观测。
            self.counters.ignored_ops += 1;
            return;
        }
        // 同键覆写（全量快照语义）。
        self.tints.insert(handle, rgba);
    }

    fn set_uv(&mut self, handle: ItemHandle, rect: [f32; 4]) {
        if !self.items.contains_key(&handle) {
            // 空句柄 / 未知句柄：静默忽略（契约 I1），计数器使其可观测。
            self.counters.ignored_ops += 1;
            return;
        }
        // 同键覆写（全量快照语义）。
        self.uvs.insert(handle, rect);
    }

    fn set_pivot(&mut self, handle: ItemHandle, pivot: [f32; 2]) {
        if !self.items.contains_key(&handle) {
            // 空句柄 / 未知句柄：静默忽略（契约 I1），计数器使其可观测。
            self.counters.ignored_ops += 1;
            return;
        }
        // 同键覆写（全量快照语义；`[0,0]` 也照存 —— 零平移 = 恒等，
        // 提取层的迁移帧"补推清除"走的就是它）。
        self.pivots.insert(handle, pivot);
    }

    fn submit_into(&mut self, frame: &FrameInfo, out: &mut Vec<RenderCommand>) {
        // 1) 先清空：缓冲跨帧复用，绝不留上一帧的残留。
        out.clear();

        // 2) 生命周期动作：即时入队、submit 时按发生顺序落缓冲。
        out.append(&mut self.lifecycle);

        // 3) 相机（每帧最多一条）。
        if let Some(camera) = self.camera {
            out.push(RenderCommand::SetCamera { camera });
        }

        // 4) 各渲染物：按 DrawKey 升序，输出全量属性快照。
        let mut ordered: Vec<&RenderItem> = self.items.values().collect();
        ordered.sort_by_key(|item| item.draw_key());
        for item in ordered {
            out.push(RenderCommand::SetTransform {
                handle: item.handle,
                transform: item.transform,
            });
            out.push(RenderCommand::SetFlip {
                handle: item.handle,
                flip: item.flip,
            });
            out.push(RenderCommand::SetZ {
                handle: item.handle,
                z: item.z,
                order: item.order,
            });
            out.push(RenderCommand::SetVisible {
                handle: item.handle,
                visible: item.visible,
            });
            if let Some(text) = self.labels.get(&item.handle) {
                out.push(RenderCommand::SetText {
                    handle: item.handle,
                    text: text.clone(),
                });
            }
            // 列表/页签（S12-3 任务 4）：输出序冻结 SetText → SetList →
            // SetRect → SetClip（与 wgpu 后端严格同序）。
            if let Some(rows) = self.lists.get(&item.handle) {
                out.push(RenderCommand::SetList {
                    handle: item.handle,
                    rows: rows.clone(),
                });
            }
            if let Some(rect) = self.rects.get(&item.handle) {
                out.push(RenderCommand::SetRect {
                    handle: item.handle,
                    rect: *rect,
                });
            }
            // 裁剪恒在 SetRect 之后（D1 推送序）；仅当该条目存在裁剪时追加。
            if let Some(clip) = self.clips.get(&item.handle) {
                out.push(RenderCommand::SetClip {
                    handle: item.handle,
                    rect: Some(*clip),
                });
            }
            // 相乘色（S16.1）：恒在 SetClip 之后（契约 I5 顺序冻结；
            // 与 wgpu 后端严格同序）。仅当该条目存在 tint 簿记时追加。
            if let Some(rgba) = self.tints.get(&item.handle) {
                out.push(RenderCommand::SetTint {
                    handle: item.handle,
                    rgba: *rgba,
                });
            }
            // 子矩形采样（S16.2）：恒在 SetTint 之后（契约 I5 顺序冻结；
            // 与 wgpu 后端严格同序）。仅当该条目存在 uv 簿记时追加 ——
            // 无记录 = 整瓦片采样，命令流与既有路径逐条相同。
            if let Some(rect) = self.uvs.get(&item.handle) {
                out.push(RenderCommand::SetUv {
                    handle: item.handle,
                    rect: *rect,
                });
            }
            // 精灵锚点（S16.3）：恒在 SetUv 之后（契约 I5 顺序冻结；与
            // wgpu 后端严格同序）。仅当该条目存在 pivot 簿记时追加 ——
            // 无记录 = 无平移，命令流与既有路径逐条相同。
            if let Some(pivot) = self.pivots.get(&item.handle) {
                out.push(RenderCommand::SetPivot {
                    handle: item.handle,
                    pivot: *pivot,
                });
            }
        }

        // 5) 帧结束标记。
        out.push(RenderCommand::Submit { frame: *frame });

        self.counters.frames += 1;
        self.counters.commands += out.len() as u64;
    }
}
