//! [`NullRenderServer`]：headless 空实现 —— S1「空实现编译通过」出口准则的载体。
//!
//! 它做三件事，都是后端契约的可执行参照：
//!
//! 1. 按 [`RenderServer`] 的不变式生成**确定性命令流**（可当"后端该怎么接"的样例）；
//! 2. 把副作用真的落在内存里（`items` / `labels` / `rects` / `camera`），
//!    让测试能断言"推送被正确保存"，而不只是"没 panic"；
//! 3. 记计数器（创建/销毁/被忽略的操作/帧数/命令数），把"空句柄被忽略"
//!    这类静默行为变成**可观测**的事实 —— 静默而不可观测的吞错最难查。

use std::collections::BTreeMap;

use crate::command::{FrameInfo, RenderCommand};
use crate::handle::{ItemHandle, RenderAssetKey};
use crate::item::RenderItem;
use crate::math::Affine2;
use crate::server::RenderServer;
use crate::state::{Camera2DState, ControlState, Flip, LabelState};

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
    rects: BTreeMap<ItemHandle, ControlState>,
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

    /// 取渲染物的控件布局状态。
    pub fn rect_of(&self, handle: ItemHandle) -> Option<&ControlState> {
        self.rects.get(&handle)
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
        self.rects.remove(&handle);
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

    fn set_rect(&mut self, handle: ItemHandle, rect: &ControlState) {
        if self.items.contains_key(&handle) {
            self.rects.insert(handle, *rect);
        } else {
            self.counters.ignored_ops += 1;
        }
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
            if let Some(rect) = self.rects.get(&item.handle) {
                out.push(RenderCommand::SetRect {
                    handle: item.handle,
                    rect: *rect,
                });
            }
        }

        // 5) 帧结束标记。
        out.push(RenderCommand::Submit { frame: *frame });

        self.counters.frames += 1;
        self.counters.commands += out.len() as u64;
    }
}
