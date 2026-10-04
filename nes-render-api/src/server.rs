//! [`RenderServer`]：节点树 → 渲染后端的**属性级推送契约**（S1 核心冻结物）。

use crate::command::{FrameInfo, RenderCommand};
use crate::handle::{ItemHandle, RenderAssetKey};
use crate::item::RenderItem;
use crate::math::{Affine2, Rect};
use crate::state::{Camera2DState, ControlState, Flip, LabelState, ListState};

/// 渲染服务端。
///
/// # 它是什么
///
/// 场景层不"从渲染器里拉数据"，而是把渲染物的**属性**推给它 —— 这正是
/// Godot `RenderingServer` / scratch-render Drawable 的做法，也是方案 D 的
/// 服务端化落点。本 trait 不含任何 GPU / 窗口 / 表面概念，只描述"要画什么"。
///
/// # 实现者必须遵守的不变式（契约冻结）
///
/// 1. **空句柄与未知句柄不 panic**：[`ItemHandle::NIL`] 或已被销毁的句柄发起的
///    任何操作都必须被静默忽略，且不得影响本帧其余命令的生成；
/// 2. **句柄不复用**：`destroy_item` 之后该句柄值不得再分配给新渲染物；
/// 3. **`submit_into` 先清空 `out`**，再写入本帧命令，末尾必定是一条
///    [`RenderCommand::Submit`]（调用方可跨帧复用同一个缓冲，实现"每帧零分配"）；
/// 4. **确定性**：同一份内部状态连续 `submit` 两次，输出必须逐条相同；
///    属性流按 [`DrawKey`](crate::DrawKey)（`z` → `order` → `handle`）升序，
///    禁止依赖哈希迭代顺序；
/// 5. **顺序固定**：生命周期动作（按发生顺序）→ `SetCamera`（若有）→ 各渲染物
///    的 `SetTransform` / `SetFlip` / `SetZ` / `SetVisible` →（Label 则追加
///    `SetText`）→（List 则追加 `SetList`）→（Control 则追加 `SetRect`，随后
///    **按需**追加 `SetClip` —— 裁剪恒在 `SetRect` 之后）→（有 tint 簿记则
///    追加 `SetTint`，S16.1 —— 恒在 `SetClip` 之后）→（有 uv 簿记则追加
///    `SetUv`，S16.2 —— 恒在 `SetTint` 之后）→（有 pivot 簿记则追加
///    `SetPivot`，S16.3 —— 恒在 `SetUv` 之后）→（有九宫格簿记则追加
///    `SetNineSlice`，S16.6 —— 恒在 `SetPivot` 之后）→ `Submit`；
/// 6. **属性流是全量快照**：不做"仅变化时推送"的增量省略，后端无需维护跨帧 diff。
///
/// # 对象安全
///
/// 全部方法都不含泛型参数与 `Self: Sized` 约束，因此 `&mut dyn RenderServer`
/// 是合法的 —— 提取层（S2）正是按 `&mut dyn RenderServer` 持有它的。
pub trait RenderServer {
    /// 新建渲染物，返回其句柄。`key` 可以是未绑定键
    /// （[`RenderAssetKey::NIL`]：先建条目、后补资源）。
    fn create_item(&mut self, key: RenderAssetKey) -> ItemHandle;

    /// 销毁渲染物。空句柄 / 未知句柄被忽略。
    fn destroy_item(&mut self, handle: ItemHandle);

    /// 设置可见性（不可见 ≠ 销毁）。
    fn set_visible(&mut self, handle: ItemHandle, visible: bool);

    /// 设置世界变换。
    fn set_transform(&mut self, handle: ItemHandle, transform: Affine2);

    /// 设置层号与同层次序。
    fn set_z(&mut self, handle: ItemHandle, z: i32, order: u64);

    /// 设置翻转（不改变变换的平移分量）。
    fn set_flip(&mut self, handle: ItemHandle, flip: Flip);

    /// 设置当前相机。契约层只保存"最后一次推送的相机"，
    /// `enabled == false` 也照实保存，由后端决定是否应用。
    fn set_camera(&mut self, camera: &Camera2DState);

    /// 设置文本（Label 类渲染物）。
    fn set_text(&mut self, handle: ItemHandle, text: &LabelState);

    /// 设置列表/页签状态（S12-3 任务 4；ListView / Tabs 摊平载荷）。
    ///
    /// 输出序冻结在对应条目的 `set_text` 之后、`set_rect` 之前
    ///（SetText → SetList → SetRect → SetClip）。
    fn set_list(&mut self, handle: ItemHandle, rows: &ListState);

    /// 设置控件布局（Control 类渲染物）。传入的是**未解析**的锚点状态，
    /// 解析公式见 [`ControlState::resolve`]。
    fn set_rect(&mut self, handle: ItemHandle, rect: &ControlState);

    /// 设置裁剪矩形（E-2 裁剪契约，语义裁决 D1）。
    ///
    /// - 裁剪是**渲染物属性**，不是流式栈：与属性流的全量快照/跨帧幂等一致，
    ///   嵌套裁剪由提取层沿祖先链求交集后以单条 `SetClip` 下发；
    /// - `rect` 是**已解析的视口空间**矩形，后端负责按目标尺寸折算成像素 scissor；
    /// - `None` = 清除该条目的裁剪；条目销毁时裁剪随条目消亡；
    /// - 空句柄 / 未知句柄被静默忽略（契约 I1 口径）；
    /// - 本属性恒在对应条目的 `set_rect` 之后推送。
    fn set_clip(&mut self, handle: ItemHandle, rect: Option<Rect>);

    /// 设置相乘色（S16.1 alpha 通道的契约扩展；RGBA8 直 alpha）。
    ///
    /// - 语义 = E-1 相乘色（采样色 x tint），但作为独立属性命令：精灵没有
    ///   自己的颜色字段，alpha 通道经此进入；
    /// - 同键覆写（全量快照/跨帧幂等）；条目销毁时随条目消亡；
    /// - 空句柄 / 未知句柄被静默忽略（契约 I1 口径）；
    /// - 输出序恒在对应条目的 `set_clip` 之后。
    fn set_tint(&mut self, handle: ItemHandle, rgba: [u8; 4]);

    /// 设置子矩形采样（S16.2 图集帧动画；归一化 UV 矩形 `[u0, v0, us, vs]`）。
    ///
    /// - 语义 = 注册表纹理的归一化子矩形（`[0, 0, 1, 1]` = 恒等 =
    ///   既有整瓦片采样），后端拿注册表实际尺寸折算成采样坐标；
    /// - 同键覆写（全量快照/跨帧幂等）；条目销毁时随条目消亡；
    /// - 空句柄 / 未知句柄被静默忽略（契约 I1 口径）；
    /// - 输出序恒在对应条目的 `set_tint` 之后；
    /// - 后端只在注册表精灵分支消费（图集格 / 字形 / 控件路径不受影响）。
    fn set_uv(&mut self, handle: ItemHandle, rect: [f32; 4]);

    /// 设置精灵锚点（S16.3 精灵锚点；归一化锚点 `[px, py]`，0..1 相对
    /// 精灵矩形）。
    ///
    /// - 语义 = 精灵四边形在**变换前的局部空间**平移 `-pivot × 16px 基准格`
    ///   （`world ∘ translation`），旋转/缩放/位置因此以锚点为基准
    ///   （`(0.5, 0.5)` = 中心锚定）；`[0, 0]` = 零平移 = 既有行为恒等；
    /// - 同键覆写（全量快照/跨帧幂等）；条目销毁时随条目消亡；
    /// - 空句柄 / 未知句柄被静默忽略（契约 I1 口径）；
    /// - 输出序恒在对应条目的 `set_uv` 之后；
    /// - 后端只在注册表精灵分支消费（无记录 = 无平移，逐位同基线）。
    fn set_pivot(&mut self, handle: ItemHandle, pivot: [f32; 2]);

    /// 设置九宫格纹理（S16.6 Control 面板纹理化；源纹理键 + 3x3 切割边距
    /// `[l, t, r, b]`，单位 = 源纹理像素）。
    ///
    /// - 语义 = Control 分支改走九宫格展开（四角 1:1、四边单向拉伸、中心
    ///   双向拉伸），`fill` / `border` 条带不再绘制（纹理自带边）；边距在
    ///   控件边长一半处钳制的折算权威在后端单处；
    /// - 同键覆写（全量快照/跨帧幂等）；条目销毁时随条目消亡；
    /// - `texture == [`RenderAssetKey::NIL`]` = **恒等记录**（照
    ///   `set_pivot([0,0])` 零向量先例）：照存照发、fill/border 照旧 ——
    ///   消费端收到后清除跨帧九宫格簿记（清除必须可在命令流里承载）；
    /// - 空句柄 / 未知句柄被静默忽略（契约 I1 口径）；
    /// - 输出序恒在对应条目的 `set_pivot` 之后。
    fn set_nine_slice(
        &mut self,
        handle: ItemHandle,
        texture: RenderAssetKey,
        l: f32,
        t: f32,
        r: f32,
        b: f32,
    );

    /// 生成本帧命令序列写入 `out`（**先清空** `out`）。
    fn submit_into(&mut self, frame: &FrameInfo, out: &mut Vec<RenderCommand>);

    /// [`RenderServer::submit_into`] 的便利版：返回新缓冲。
    ///
    /// 热路径（每帧调用）应使用 `submit_into` 复用缓冲，本方法只供测试/冷路径使用。
    fn submit(&mut self, frame: &FrameInfo) -> Vec<RenderCommand> {
        let mut out = Vec::new();
        self.submit_into(frame, &mut out);
        out
    }

    /// 按属性集合整块推送（等价于逐项调用 setter，顺序固定为
    /// `set_transform` → `set_flip` → `set_z` → `set_visible`）。
    ///
    /// 默认实现即可，实现者无需覆写：它保证"整块推送"与"逐属性推送"
    /// 在契约上不可区分（S2 提取层两种写法混用也不会产生分歧）。
    fn apply_item(&mut self, item: &RenderItem) {
        self.set_transform(item.handle, item.transform);
        self.set_flip(item.handle, item.flip);
        self.set_z(item.handle, item.z, item.order);
        self.set_visible(item.handle, item.visible);
    }
}

// 编译期守卫：trait 必须保持对象安全（提取层按 &mut dyn RenderServer 持有）。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::null::NullRenderServer;

    fn holds_dyn(_server: &mut dyn RenderServer) {}

    #[test]
    fn trait_is_object_safe() {
        let mut server = NullRenderServer::new();
        holds_dyn(&mut server);
        let key = RenderAssetKey::from_parts(7, 1);
        let handle = server.create_item(key);
        server.apply_item(&RenderItem::new(handle, key, Affine2::translation(1.0, 2.0)));
        assert_eq!(server.len(), 1);
    }
}
