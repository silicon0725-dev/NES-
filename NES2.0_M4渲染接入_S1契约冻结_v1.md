---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 19886e2d8bf8a6bde5831a704b5a86f4_e41dbe35b9d811f1b172525400248c00
    ReservedCode1: HJHysKaEiAuHUnZ+Wmz/jG9Ze6aw6mT7mohSFnpq8BqmSLFNGRqtP4GgrCP0OgsjxItzuxJojKtmEbnbkwyIiW57RQo+4T8XIZRGk4xi/KveoRAYFYbnw8zS+rRj+HzxT9zWA70oqMZMv13ITDSQB9yNOMthJfR7m97dIaamOJuhljGByyoWAIT1Pdo=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 19886e2d8bf8a6bde5831a704b5a86f4_e41dbe35b9d811f1b172525400248c00
    ReservedCode2: HJHysKaEiAuHUnZ+Wmz/jG9Ze6aw6mT7mohSFnpq8BqmSLFNGRqtP4GgrCP0OgsjxItzuxJojKtmEbnbkwyIiW57RQo+4T8XIZRGk4xi/KveoRAYFYbnw8zS+rRj+HzxT9zWA70oqMZMv13ITDSQB9yNOMthJfR7m97dIaamOJuhljGByyoWAIT1Pdo=
---



# NES 2.0 · M4 渲染接入 S1「契约冻结」文档 v1

- 日期：2026-09-26
- 状态：**Frozen（S1 出口）** —— 本文档列出的类型、方法签名、不变量与算式在 S2/S3 期间不得改动；如需改动走第 10 节变更纪律
- 依据：《`m4_render_borrow_research.md`》方案 D（渲染服务端化 + 单向依赖 + 每帧提取）第 4 节 S1 出口准则
- 交付根目录：`C:\Users\Administrator\AppData\Roaming\Tencent\Marvis\User\90EA32E1CDA20B32D089B53E8D26FB7F\workspace\conv_b3458d8493f34d69b1aa7f1a67fa414c\output`（下文相对路径均以此为根）
- 不包含：GPU / 窗口 / 表面 / 后端实现（S4）、提取层实现（S2）、Scratch 语义（M5）

---

## 0. 一句话结论

M4 的接入面已在契约层定死为**一份属性级推送契约**：场景层不认识渲染层，后端只认识 `RenderServer` + `Vec<RenderCommand>`，Camera2D / Label / Control / flip 四个缺口作为一等契约类型落在本层并有单测看守。契约层零依赖、零 unsafe，`cargo test` 39 项全通过、`cargo clippy -D warnings` 零警告、依赖方向守卫 5/5 通过。

---

## 1. 交付物清单

| # | 交付物 | 路径（相对 output） | 说明 |
|---|---|---|---|
| 1 | 契约层 crate | `nes-render-api/` | 零依赖；`src/` 8 个模块 + `tests/criterion_contract.rs` |
| 1.1 | 契约清单 | `nes-render-api/Cargo.toml` | `[dependencies]` 为空，零依赖理由写在文件内 |
| 1.2 | crate 根 | `nes-render-api/src/lib.rs` | 契约表面总表 + 5 条硬约束；`forbid(unsafe_code)` / `deny(missing_docs)` |
| 1.3 | wire 数学类型 | `nes-render-api/src/math.rs` | `Vec2` / `Affine2` / `Rect`，字段序对齐 `nes-scene` 的 `Affine` |
| 1.4 | 句柄与键 | `nes-render-api/src/handle.rs` | `ItemHandle`（易变）/ `RenderAssetKey`（稳定身份） |
| 1.5 | 渲染物 | `nes-render-api/src/item.rs` | `RenderItem`（`Copy`）/ `DrawKey`（全序） |
| 1.6 | 缺口契约 | `nes-render-api/src/state.rs` | `Flip` / `Camera2DState` / `LabelState` / `ControlState` |
| 1.7 | 命令与帧 | `nes-render-api/src/command.rs` | `RenderCommand` / `FrameInfo` |
| 1.8 | 服务端契约 | `nes-render-api/src/server.rs` | `RenderServer` trait（对象安全）+ 6 条实现者不变式 |
| 1.9 | 空实现 | `nes-render-api/src/null.rs` | `NullRenderServer` + `ServerCounters`（headless，可作 S2 测试替身） |
| 1.10 | 契约不变量单测 | `nes-render-api/tests/criterion_contract.rs` | 31 项 `criterion_contract_*` |
| 2 | 依赖方向检查脚本 | `check_dependency_direction.py` | 基于 `cargo metadata`，5 项守卫 G1~G5，退出码 0/1/2，可直接接 CI |
| 3 | 本文档 | `NES2.0_M4渲染接入_S1契约冻结_v1.md` | 冻结签名 + 出口准则对照 + 实测结果 + 开放问题 |

---

## 2. 冻结签名

以下签名即冻结面：S2/S3 只能**调用**它们，不得改形。

### 2.1 句柄与资源键（`handle.rs`）

```rust
pub struct ItemHandle(u64);                       // 后端侧易变句柄，等价 Godot RID；不得序列化
impl ItemHandle {
    pub const NIL: Self;                          // 0，空句柄
    pub const fn from_raw(raw: u64) -> Self;
    pub const fn raw(self) -> u64;
    pub const fn from_parts(slot: u32, gen: u32) -> Self;   // 高 32 位 gen，低 32 位 slot
    pub const fn slot(self) -> u32;
    pub const fn generation(self) -> u32;
    pub const fn is_nil(self) -> bool;
}

pub struct RenderAssetKey(u64);                   // 稳定资源身份在渲染侧的投影（M3 位编码一致）
impl RenderAssetKey {
    pub const NIL: Self;
    pub const fn from_bits(bits: u64) -> Self;
    pub const fn from_parts(slot: u32, gen: u32) -> Self;
    pub const fn to_bits(self) -> u64;
    pub const fn slot(self) -> u32;
    pub const fn generation(self) -> u32;
    pub const fn is_nil(self) -> bool;
}
```

职责切分（冻结）：`ItemHandle` = **渲染物生命周期**（谁被销毁谁失效）；`RenderAssetKey` = **资源身份**（内容换代、句柄不变）。二者不得互相替代。

### 2.2 渲染物与绘制键（`item.rs`）

```rust
pub struct DrawKey { pub z: i32, pub order: u64, pub handle: u64 }   // Ord 全序，无并列

pub struct RenderItem {                            // Copy：可整块入预分配缓冲，零堆分配
    pub handle: ItemHandle,
    pub key: RenderAssetKey,
    pub visible: bool,
    pub z: i32,
    pub order: u64,
    pub transform: Affine2,                        // 世界变换（已含父链复合）
    pub flip: Flip,                                // 不参与世界变换缓存，绘制期后乘
}
impl RenderItem {
    pub fn new(handle: ItemHandle, key: RenderAssetKey, transform: Affine2) -> Self;
    pub fn draw_key(&self) -> DrawKey;
    pub fn world_transform(&self) -> Affine2;      // = transform ∘ flip
}
```

### 2.3 命令与帧（`command.rs`）

```rust
pub struct FrameInfo {
    pub frame_index: u64, pub delta: f32, pub time: f64,
    pub viewport: Vec2, pub dpi_scale: f32,
}
impl FrameInfo {
    pub const fn new(frame_index: u64, delta: f32, time: f64, viewport: Vec2) -> Self;   // dpi_scale = 1
    pub fn with_dpi_scale(self, dpi_scale: f32) -> Self;
}

pub enum RenderCommand {
    CreateItem { handle: ItemHandle, key: RenderAssetKey },   // 生命周期
    DestroyItem { handle: ItemHandle },                       // 生命周期
    SetVisible { handle: ItemHandle, visible: bool },
    SetTransform { handle: ItemHandle, transform: Affine2 },
    SetZ { handle: ItemHandle, z: i32, order: u64 },
    SetFlip { handle: ItemHandle, flip: Flip },
    SetCamera { camera: Camera2DState },                      // 每帧最多一条，位于属性流之前
    SetText { handle: ItemHandle, text: LabelState },
    SetRect { handle: ItemHandle, rect: ControlState },
    Submit { frame: FrameInfo },                              // 必须位于末尾
}
impl RenderCommand {
    pub fn handle(&self) -> Option<ItemHandle>;               // SetCamera / Submit → None
    pub fn is_lifecycle(&self) -> bool;                       // Create / Destroy
}
```

### 2.4 服务端契约（`server.rs`）—— S1 核心冻结物

```rust
pub trait RenderServer {
    fn create_item(&mut self, key: RenderAssetKey) -> ItemHandle;
    fn destroy_item(&mut self, handle: ItemHandle);
    fn set_visible(&mut self, handle: ItemHandle, visible: bool);
    fn set_transform(&mut self, handle: ItemHandle, transform: Affine2);
    fn set_z(&mut self, handle: ItemHandle, z: i32, order: u64);
    fn set_flip(&mut self, handle: ItemHandle, flip: Flip);
    fn set_camera(&mut self, camera: &Camera2DState);
    fn set_text(&mut self, handle: ItemHandle, text: &LabelState);
    fn set_rect(&mut self, handle: ItemHandle, rect: &ControlState);
    fn submit_into(&mut self, frame: &FrameInfo, out: &mut Vec<RenderCommand>);   // 热路径
    fn submit(&mut self, frame: &FrameInfo) -> Vec<RenderCommand>;               // 便利版（默认实现）
    fn apply_item(&mut self, item: &RenderItem);                                  // 整块推送（默认实现）
}
```

对象安全（冻结）：全部方法无泛型参数、无 `Self: Sized`，因此 `&mut dyn RenderServer` 合法 —— 提取层（S2）正是按此持有。

### 2.5 四项缺口契约（`state.rs`）

缺口 1 —— `Flip`（翻转合成）：

```rust
pub struct Flip { pub h: bool, pub v: bool }
impl Flip {
    pub const IDENTITY: Self;
    pub const fn new(h: bool, v: bool) -> Self;
    pub const fn is_identity(self) -> bool;
    pub const fn any(self) -> bool;
    pub const fn to_affine(self) -> Affine2;              // scale(±1, ±1)
    pub fn compose(self, transform: Affine2) -> Affine2;  // transform ∘ flip（后乘）
}
```

> 冻结语义：`world ∘ scale(±1,±1)`，**平移分量逐位不变**；不折进节点变换（否则污染 `nes-scene` 的世界变换缓存，且"改 flip 不改位置"失去可验证性）。

缺口 2 —— `Camera2DState`（视图矩阵）：

```rust
pub struct Camera2DState {
    pub transform: Affine2, pub offset: Vec2, pub zoom: Vec2,
    pub viewport: Vec2, pub limits: Option<Rect>, pub enabled: bool,
}
impl Camera2DState {
    pub fn new(viewport: Vec2) -> Self;
    pub fn effective_zoom(self) -> Vec2;         // zoom <= 0 视为 1
    pub fn rotation(self) -> f32;                // 取 transform 的旋转分量
    pub fn center(self) -> Vec2;                 // transform 平移 + R(rot)·offset
    pub fn visible_half_extents(self) -> Vec2;   // 世界轴 AABB 半尺寸（旋转下取 |cos|,|sin| 投影）
    pub fn clamped_center(self) -> Vec2;         // limits 夹紧；轴比视口窄时取 limits 中心
    pub fn visible_world_rect(self) -> Rect;
    pub fn view_matrix(self) -> Option<Affine2>; // enabled == false → None
}
```

> 冻结算式（唯一权威，后端不得另行推导）：
> `view = T(viewport/2) ∘ S(zoom) ∘ T(-clamped_center) ∘ R(-rotation)`

缺口 3 —— `LabelState`（文本状态）：

```rust
pub struct LabelState {
    pub text: Arc<str>,                  // 与 nes-scene 的 Value::Str 同表示，克隆不复制字节
    pub font: RenderAssetKey,            // NIL = 后端默认字体
    pub font_size: f32, pub line_spacing: f32,
    pub align_h: HAlign, pub align_v: VAlign,
    pub wrap_width: Option<f32>,
}
impl LabelState { pub fn new(text: impl Into<Arc<str>>, font_size: f32) -> Self; }

pub enum HAlign { Left /*默认*/, Center, Right }
pub enum VAlign { Top /*默认*/, Center, Bottom }
```

> 冻结边界：契约只描述"显示什么"；**排版（断行 / 字形度量 / 图集打包）属 CPU 侧**，落提取层或 S3 文本实现，不得下沉进 GPU 后端（对应调研报告风险点 2）。

缺口 4 —— `ControlState`（锚点布局）：

```rust
pub struct ControlState {
    pub anchor_left: f32, pub anchor_top: f32, pub anchor_right: f32, pub anchor_bottom: f32,
    pub offset_left: f32, pub offset_top: f32, pub offset_right: f32, pub offset_bottom: f32,
    pub min_size: Option<Vec2>,                                 // v1.1：None = 无下界（缺省）
}
impl ControlState {
    pub const FULL_RECT: Self;                                  // 锚点 0,0,1,1 + 零偏移
    pub const fn new(anchors: [f32; 4], offsets: [f32; 4]) -> Self;
    pub fn resolve(&self, parent_size: Vec2) -> Rect;
}
```

> 冻结算式：四边各自 `anchor * parent_size + offset` 得矩形；若 `min_size` 为 `Some(min)` 且宽/高小于 `min`，**只推右/下边**（左上角不动）。**不钳制负尺寸**（负宽高保留原值，见开放问题 Q2）。
>
> _v1.1 修订（2026-09-26）：`min_size` 由 `Vec2`（缺省 `Vec2::ZERO`）改为 `Option<Vec2>`，缺省 `None` = **无下界**，负宽高原样透传 —— 见 §11。原结论"不钳制负尺寸"不变。_

### 2.6 headless 空实现（`null.rs`）

```rust
pub struct ServerCounters { pub created: u64, pub destroyed: u64,
                            pub ignored_ops: u64, pub frames: u64, pub commands: u64 }
pub struct NullRenderServer { /* 内部：BTreeMap 记录 + lifecycle 队列 + 计数器 */ }
impl NullRenderServer {
    pub fn new() -> Self;
    pub fn items(&self) -> &BTreeMap<ItemHandle, RenderItem>;   // 有序，不使用 HashMap
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn item(&self, handle: ItemHandle) -> Option<&RenderItem>;
    pub fn label_of(&self, handle: ItemHandle) -> Option<&LabelState>;
    pub fn rect_of(&self, handle: ItemHandle) -> Option<&ControlState>;
    pub fn camera(&self) -> Option<&Camera2DState>;
    pub fn counters(&self) -> ServerCounters;
    pub fn draw_order(&self) -> Vec<ItemHandle>;                 // 按 DrawKey 升序
}
impl RenderServer for NullRenderServer { /* 全部方法实现，严格遵循第 3 节不变式 */ }
```

用途：① 满足 S1"空实现编译通过"；② S2 提取层的测试替身；③ 后端实现者的可读可跑参照样例。

---

## 3. 契约不变量（冻结条款）

| 编号 | 不变量 | 看守测试 |
|---|---|---|
| I1 | 空句柄（`ItemHandle::NIL`）与未知/已销毁句柄的一切操作被**静默忽略**，不 panic、不影响同帧其余命令 | `criterion_contract_server_ignores_nil_and_unknown_handles` |
| I2 | 句柄**永不复用**：`destroy_item` 后该句柄值不得再分配给新渲染物；slice 复用必须换 generation | `criterion_contract_server_never_reuses_handles`、`criterion_contract_slot_reuse_differs_by_generation` |
| I3 | `submit_into` **先清空 `out`** 再写入本帧命令，末条必为 `Submit`（调用方可跨帧复用缓冲 → 每帧零分配） | `criterion_contract_submit_into_clears_reused_buffer`、`criterion_contract_empty_frame_still_terminates_with_submit` |
| I4 | 命令流二分：**一次事件**（`CreateItem`/`DestroyItem`，按调用顺序，只在发生的那一帧出现一次）+ **每帧全量快照**（相机 + 各渲染物属性 + `Submit`）；状态不变时快照部分逐条可重现 | `criterion_contract_submit_is_deterministic`、`criterion_contract_submit_is_full_snapshot_not_delta`、`criterion_contract_destroy_is_enqueued_until_next_submit` |
| I5 | 属性流按 `DrawKey`（`z` → `order` → `handle`）升序，**禁止**依赖哈希迭代顺序或插入先后 | `criterion_contract_draw_key_is_total_order`、`criterion_contract_draw_order_is_insertion_independent` |
| I6 | 帧内顺序固定：生命周期动作 → `SetCamera`（若有）→ 各渲染物的 `SetTransform`/`SetFlip`/`SetZ`/`SetVisible` →（Label）`SetText` →（Control）`SetRect` → `Submit` | `criterion_contract_submit_command_layout_is_frozen` |
| I7 | `apply_item` 与逐项 setter 在契约上**不可区分** | `criterion_contract_apply_item_matches_individual_setters` |
| I8 | `RenderItem::world_transform() == transform ∘ flip`，flip 不影响平移分量 | `criterion_contract_render_item_world_transform_includes_flip`、`criterion_contract_flip_is_child_local_post_multiply`、`criterion_contract_flip_four_combinations` |
| I9 | 相机注视点在旋转 + 缩放 + 夹紧后仍映射到视口中心；`enabled == false` 时 `view_matrix()` 为 `None` | `criterion_contract_camera_center_always_at_viewport_center`、`criterion_contract_camera_identity_maps_origin_to_viewport_center`、`criterion_contract_camera_visible_extents_under_rotation`、`criterion_contract_camera_offset_follows_rotation`、`criterion_contract_camera_limits_clamp_center`、`criterion_contract_camera_disabled_and_zoom_normalization` |
| I10 | 契约层零依赖、零 `unsafe`，且 trait 保持对象安全 | `criterion_contract_server_is_object_safe_and_usable_as_dyn`、`criterion_contract_nil_handle_and_key_are_zero`、依赖守卫 G3/G4 |

---

## 4. 与调研报告 S1 出口准则对照

调研报告第 4 节原文出口准则：**「契约文档 + 空实现编译通过；依赖方向检查脚本就绪」**。

| 出口准则 | 要求 | 交付 | 实测证据 | 判定 |
|---|---|---|---|---|
| 契约冻结（`RenderServer` trait 定稿） | 方法面定稿、缺口入契约 | `server.rs` §2.4（9 个必需方法 + 2 个默认方法） | `cargo test` 31 项集成测试通过 | ✅ |
| `RenderItem` 定稿 | 属性集合、可零分配入缓冲 | `item.rs` §2.2（`Copy`，字段不含专用状态） | 同上 + clippy 0 警告 | ✅ |
| `RenderCommand` 定稿 | 后端可线性消费 | `command.rs` §2.3（9 变体 + `Submit`） | `criterion_contract_submit_command_layout_is_frozen` | ✅ |
| `FrameInfo` 定稿 | 帧上下文，不参与绘制决策 | `command.rs` §2.3（`frame_index/delta/time/viewport/dpi_scale`） | 编译 + 布局测试 | ✅ |
| 回写设计文档 | 冻结签名落文档 | 本文档 §2 / §3 | — | ✅ |
| 空实现编译通过 | headless 空实现可编译可跑 | `null.rs` 的 `NullRenderServer` | `cargo check`/`build` EXIT 0；`cargo test` EXIT 0（8 单测 + 31 集成） | ✅ |
| 依赖方向检查脚本就绪 | 基于 `cargo metadata` 判定 `nes-scene` 依赖树无 `nes-render-*` | `check_dependency_direction.py`（G1~G5，含 `--json` 输出） | 实测 5/5 PASS，EXIT 0 | ✅ |
| 四项缺口入契约（报告 §3.1 缺口列） | Camera/Label/Control/flip 可单测 | `state.rs` §2.5 | 12 项缺口专项测试（flip 3 / camera 7 / label 2 / control 3） | ✅ |

**超出 S1 出口准则、本轮顺带冻结的部分**（说明动机）：

1. **`DrawKey` 全序键**：报告骨架里 `set_z` 只有 `z`，没定义同层次序。若不给全序键，S2 会自然滑向"按插入顺序画"，与项目的确定性要求冲突 → 提前冻结为 `z → order → handle`。
2. **`RenderAssetKey` 位编码镜像**：报告用 `RenderAssetKey` 占位但未给表示。契约层必须零依赖，不能直接用 `nes-asset::AssetKey`，故冻结为与 M3 相同的 `(slot, gen)` 位编码，S2 做纯位拷贝桥接。
3. **`submit_into` + 缓冲复用**：报告骨架只有返回 `Vec` 的 `submit`；每帧新建 `Vec` 与"每帧零分配"（报告风险点 1）直接冲突 → 补 `submit_into(out)` 为热路径，`submit` 降为便利版。
4. **`NullRenderServer` 行为计数器**：让"忽略规则 / 事件一次性 / 顺序"可被断言，而不是靠人工阅读实现。

---

## 5. 与调研报告接口骨架的差异（逐条冻结理由）

| # | 报告骨架 | 本契约 | 差异性质 | 理由 |
|---|---|---|---|---|
| 1 | `fn set_z(&mut self, h, z: i32)` | `set_z(&mut self, h, z: i32, order: u64)` | **扩展** | 同层稳定次序必须可显式传递，否则绘制次序依赖实现内部顺序 |
| 2 | `fn set_flip(&mut self, h, flip_h: bool, flip_v: bool)` | `set_flip(&mut self, h, flip: Flip)` | **收窄为类型** | 两个 bool 参数易混淆；`Flip` 可承载 `to_affine`/`compose` 算式并单测 |
| 3 | `fn submit(&mut self, frame) -> Vec<RenderCommand>` | 增加 `submit_into(..., out: &mut Vec<...>)` | **扩展** | 每帧零分配（报告风险点 1），`submit` 保留为便利版 |
| 4 | `RenderItem` 未给字段 | 冻结 7 字段（`Copy`） | **补全** | 报告未定义"渲染物属性集合"的具体形状；字段刻意排斥 Label/Control 专用状态，避免大类型污染热缓冲 |
| 5 | 无 `DrawKey` | 冻结 `DrawKey` | **新增** | 定理式排序入口，防止后端/提取层各写一套 |
| 6 | 无 `FrameInfo.dpi_scale` | 冻结 `dpi_scale` | **新增** | 视口单位（设备像素）与逻辑像素的换算必须有单一出处，否则 S3 与 `twn-render-stage` 比对必然出现 1.25/1.5 倍偏差 |
| 7 | 无 `apply_item` | 冻结默认方法 | **新增** | 提取层"整块推送"与"逐属性推送"等价，避免 S2 两种写法产生分歧 |

---

## 6. 实测验证结果（本机 Windows 10 / cargo 1.98.1 / rustc 1.98.1）

### 6.1 `cargo test --all-targets`

- 命令：`cargo test --manifest-path output/nes-render-api/Cargo.toml --all-targets`
- 结果：**EXIT 0**

| 目标 | 结果 |
|---|---|
| `unittests src\lib.rs`（crate 内） | **8 passed; 0 failed** |
| `tests\criterion_contract.rs` | **31 passed; 0 failed** |

crate 内 8 项（数学与对象安全自检）：
`math::tests::{array_roundtrip_keeps_layout_order, inverse_roundtrips_point, composition_order_is_self_after_rhs, degenerate_matrix_has_no_inverse, rotation_of_ignores_scale, identity_is_neutral, rect_contains_is_inclusive_on_all_edges}`、`server::tests::trait_is_object_safe`

31 项契约不变量测试（按主题分组，全部以 `criterion_contract_` 前缀命名，沿用 M1~M3 封口惯例）：

| 主题 | 用例数 | 用例名（节选/全列） |
|---|---|---|
| 句柄与键 | 4 | `nil_handle_and_key_are_zero`、`handle_bits_roundtrip`、`handle_is_usable_as_map_key`、`slot_reuse_differs_by_generation` |
| 绘制次序 | 2 | `draw_key_is_total_order`、`draw_order_is_insertion_independent` |
| 提交语义 | 6 | `submit_command_layout_is_frozen`、`submit_is_full_snapshot_not_delta`、`submit_is_deterministic`、`submit_into_clears_reused_buffer`、`empty_frame_still_terminates_with_submit`、`destroy_is_enqueued_until_next_submit` |
| 服务端不变式 | 4 | `server_ignores_nil_and_unknown_handles`、`server_never_reuses_handles`、`server_is_object_safe_and_usable_as_dyn`、`apply_item_matches_individual_setters` |
| 缺口 1 · flip | 3 | `flip_four_combinations`、`flip_is_child_local_post_multiply`、`render_item_world_transform_includes_flip` |
| 缺口 2 · camera | 7 | `camera_identity_maps_origin_to_viewport_center`、`camera_center_always_at_viewport_center`、`camera_offset_follows_rotation`、`camera_visible_extents_under_rotation`、`camera_limits_clamp_center`、`camera_disabled_and_zoom_normalization`、`camera_last_write_wins_including_disabled` |
| 缺口 3 · label | 2 | `label_defaults_are_explicit`、`label_text_is_shared_not_copied` |
| 缺口 4 · control | 3 | `control_resolve_anchor_formula`、`control_full_rect_and_stretch`、`control_min_size_grows_bottom_right_only` |

### 6.2 `cargo clippy --all-targets -- -D warnings`

- 结果：**EXIT 0**，输出仅 `Checking nes-render-api` + `Finished`，**零警告、零错误**（`-D warnings` 生效下通过 = 无 lint 债务）
- 注：crate 内已开 `#![deny(missing_docs)]`，公共 API 全部带文档注释

### 6.3 依赖方向守卫

- 命令：`python check_dependency_direction.py --root <output> --json <temp>/s1_depcheck.json`
- 结果：**EXIT 0，5/5 PASS**

| 编号 | 检查项 | 实测 |
|---|---|---|
| G1 | `nes-scene` 依赖树（含传递）不含 `nes-render-*` | PASS —— 传递依赖 1 个：`nes-asset` |
| G2 | `nes-asset` 依赖树不含 `nes-render-*` | PASS —— 传递依赖 0 个 |
| G3 | `nes-render-api` 零依赖（normal/dev/build 均为空） | PASS —— 声明依赖 0 条 |
| G4 | `nes-render-api` 不依赖 `nes-scene`/`nes-asset` | PASS —— 传递依赖 0 个 |
| G5 | `nes-scene`/`nes-asset` 源码中不出现 `nes_render` 符号（防注释级/feature 级隐性引用） | PASS —— 扫描 25 个 `.rs`/`.toml`，命中 0 处 |

### 6.4 本轮真实偏差与缺陷（沿用 M1~M3 封口方法：只记真问题）

| # | 现象 | 性质 | 处置 |
|---|---|---|---|
| D1 | `criterion_contract_submit_is_deterministic` 首轮断言"两次 `submit` 输出逐条相同"**失败**：第二次提交不含 `CreateItem`（生命周期事件已消费），快照部分则完全一致 | **契约歧义（真缺陷）** —— "确定性"未写明覆盖范围，容易被误读为"连事件也重放" | 已把二分语义写成显式条款 I4（事件一次性 + 快照逐帧重现），测试改为分别断言两侧；trait 文档同步细化 |
| D2 | 首轮 `cargo check` EXIT 101：测试第 191 行把 `f32` 传给了 `approx_vec(Vec2, Vec2)` | 测试代码笔误（1 处） | 改用标量断言，复跑 EXIT 0 |
| D3 | 验证脚本首版在 600s 级调用中未返回（`Start-Process -RedirectStandardOutput` 与 cargo→rustc 子进程共享句柄时的读流等待）；改用 `cmd /c "... > 日志 2>&1"` 文件重定向后，同一命令 3s 内结束 | **工具链事实**，非契约问题 | 已在验证流程中固化（后续 S2/S3 复用该模式，可避免 CI 上出现同类悬挂） |

> 结论：S1 未见契约级设计缺陷；D1 是**定义缺失**（非实现错误），已在冻结文本中闭合。

### 6.5 日志落盘位置（中间产物，可复核）

均在 `temp/` 下：`s1c_test.out.log`（全量测试输出）、`s1c_clippy.out.log`（clippy 输出）、`s1_depcheck.log` / `s1_depcheck.json`（依赖守卫结果）。

---

## 7. 边界与纪律（本轮遵守情况）

| 约束 | 遵守方式 | 证据 |
|---|---|---|
| 契约层零依赖 | `[dependencies]` 为空；含 dev/build 亦为 0 | 守卫 G3 PASS |
| 不得依赖 `nes-scene` / `nes-asset` / 任何渲染后端 | 未出现任何依赖项 | 守卫 G4 PASS |
| 禁止修改 `nes-scene`、`nes-asset` 既有源码与测试 | 全程只读该两 crate 以确认 `Affine` 字段序与 `AssetKey` 位编码；未写入 | 守卫 G5 PASS（25 个文件 0 命中）；两 crate 目录未出现在任何写入调用中 |
| 禁止改动 `NES2.0_节点场景树_接口草案_v1.md` | 未读取写入；契约层**不要求**该草案改动一行（本层与节点树解耦） | 同上 |
| 编译产物不得污染 output | `CARGO_TARGET_DIR` 指向 `temp/nes-render-api-target` | —— |

---

## 8. 对 S2/S3/S4 的接续约束（本次冻结的直接后果）

1. **S2 提取层的形状已被本契约钉死**：每帧一次 `for node in world.iter_depth_first_deterministic()`，用 `NodeId → ItemHandle` 映射表（替代 Bevy 的 `RenderEntity/MainEntity`、Godot 的 RID 表）做 `get_or_create`，然后逐属性 `set_*`，最后 `submit_into(&mut frame_commands)` 复用同一缓冲（I3）。
2. **`AssetKey → RenderAssetKey` 桥为纯位拷贝**：契约层用与 M3 相同的 `(slot, gen)` 位编码，S2 不得引入新的身份体系（守卫会持续看守）。
3. **flip / Camera2D / Label / Control 的算式已在契约层冻结**：S3 只做「把 `nes-scene` 属性填进这些状态」，不得在 stage 内部另写一份矩阵或布局公式，否则与 `twn-render-stage` 的逐帧比对失去唯一参照。
4. **后端（S4）只允许消费 `Vec<RenderCommand>`**：后端不得读取 `nes-scene`（G1 守卫会失败），也不得依赖 `RenderServer` 之外的任何场景概念。
5. **M5 兼容层位置不变**：Scratch 语义翻译到节点树/`RenderItem`，位于渲染路径之外（报告 §3.3）。

---

## 9. 开放问题清单（需拍板）

以下 8 项**不影响 S1 出口**（契约已可编译、可测试），但会影响 S2/S3 的实现基线；建议在 S2 开工前一次性裁决。

| # | 问题 | 选项 | 影响面 | 我的倾向 |
|---|---|---|---|---|
| Q1 | **相机 `limits` 在相机旋转下的语义**：当前实现按**世界轴 AABB** 夹紧（近似），Godot 按相机轴夹紧 | A. 保持世界轴 AABB；B. 改为相机轴夹紧；C. 本里程碑只支持旋转为 0 的相机 | S3 与 `twn-render-stage` 的逐帧比对基准；旋转 + limits 同时出现时的可见区域差异 | A（保持）—— 与既有 stage 行为对齐优先，旋转+limits 组合极少；若 twn 实现是相机轴，则 S3 直接改这一处算式并加一条 `criterion_*` |
| Q2 | **`ControlState::resolve` 对负宽高的处理**：当前**不钳制**（负值保留），Godot 保留负值表达"反向拉伸" | A. 保留不钳制；B. 钳制到 0；C. 钳制并在 `Rect` 上记录异常标志 | 布局异常时的绘制行为；后端是否需要额外的负尺寸分支 | A（保持）—— 语义留给调用方，契约不替上游做决定 |
| Q3 | **flip 属性的上游来源**：契约层已冻结 flip 语义，但 `nes-scene`（禁改）当前是否暴露可用的 flip/sprite 属性需要确认 | A. 复用 `nes-scene` 既有属性中的 flip 字段（若存在）；B. S2 期在提取层维护一张临时的 `NodeId → Flip` 旁路表；C. 向后续里程碑申请为 `nes-scene` 增补属性（本里程碑不动） | S2 能否直接取到 flip；是否引入旁路状态（旁路状态与"每帧提取"原则有张力） | 需您确认 `nes-scene` 现有属性里 flip 的实际归属；若没有，倾向 B（临时、有截止日）而非提前改 `nes-scene` |
| Q4 | **`set_z(z, order)` 的 `order` 与 `nes-scene` 兄弟序的同源关系**：契约层用 `u64`，需确认 scene 侧现有次序字段的类型/语义（是否包含"插入序"与"显式 z"两套含义） | A. 与 scene 字段一一对应；B. 提取层按稳定遍历序生成；C. 引入 `order = 兄弟序号 * 2^32 + 插入序` 的合成 | S2 的 `DrawKey` 稳定性；同层多节点的绘制次序是否与现有 stage 一致 | A 优先，无法一一对应时用 B（确定性遍历序）—— 但要避免与既有 stage 视觉回归冲突 |
| Q5 | **`ItemHandle` 位宽 64（slot/gen 各 32）是否冻结**：将来若需要 GPU 侧非 CPU 句柄（例如后端想直接塞 wgpu 索引）是否够用 | A. 冻结 64 位；B. 改为 `NonZeroU64` + 保留位；C. 改 u128 | ABI 稳定性；后端能否复用同一句柄空间 | A（冻结）—— 契约层不承诺承载后端内部句柄，后端自建映射表 |
| Q6 | **资源换代（M3 generation）在后端的刷新路径**：`RenderAssetKey` 带 generation，`nes-asset` 支持同 key 新版本；但当前契约**没有**"资源已更新"命令（只有 `CreateItem(key)`） | A. 约定"资源换代 = `destroy_item` + `create_item`"（零新增 API）；B. 新增 `RebindKey { handle, key }`；C. 新增资源级事件 `AssetChanged { key }` | S3/S4 的 GPU 资源刷新成本（是否重建整个渲染物）；未来热重载行为 | A 暂用（S1 不扩面），S2 实测"重载一次纹理"的成本后再决定是否升 B；若您希望 S1 就补，B 是最小增量 |
| Q7 | **相机是单槽还是多槽**：当前 `set_camera` 为 last-write-wins 单槽，`Submit` 前的属性流只带一条 `SetCamera` | A. 单相机冻结（含分屏/多视口不在 M4 范围）；B. 改为按视口 id 多相机；C. 单相机 + 预留 `viewport_id` 字段 | 是否支持分屏/双人游戏；API 一旦扩面，后端实现复杂度上升 | A（冻结）—— M4 目标是"节点树成为渲染原生输入"，多相机属新需求，不应由契约猜测 |
| Q8 | **依赖守卫是否列为 CI 阻塞项，以及调用时机**：脚本已就绪（`python check_dependency_direction.py`，退出码 0/1） | A. CI 阻塞（推荐）+ 提交前本地跑；B. 仅里程碑末跑一次；C. 仅文档约定不自动检查 | 架构不倒退的实际保障强度（报告风险点 3：用 CI 而非口头约定） | A（阻塞）—— 脚本 3s 内跑完，成本可忽略；但 CI 环境需要 Python 3（本机 3.11.8 可用） |

---

## 10. 冻结后的变更纪律

1. 本文档 §2 签名与 §3 不变量属**冻结面**：S2/S3 期间只能新增（向后兼容的默认方法 / 可选字段），不得改形、不得改算式含义。
2. 任何必须改形的变更，须在本文档追加「v1.1 修订」小节，逐条写明：触发原因、改动前后的签名、受影响测试、是否破坏 S2 已落地的代码 —— 沿用 M1~M3 的封口方法。
3. 每次变更后必须同时满足：`cargo test` 全绿、`cargo clippy --all-targets -- -D warnings` 零警告、依赖守卫 5/5 通过。三项任一不过视为未完成变更。
4. 依赖守卫（G1~G5）为本层的**持续不变量**，不随里程碑结束而撤销；新增 crate（`nes-render-extract` / `nes-render-backend`）时，若出现新的依赖边，需在 G 系列中登记预期方向（例如允许 `nes-render-extract → nes-render-api`，仍禁止任何 `nes-render-* → nes-scene` 反向边）。
---

## 11. v1.1 修订（2026-09-26）

> 依 §10.2 追加。本节**只补丁说明**，不推翻 §2 / §3 既有结论；§2.5 的冻结算式仍是唯一权威。

### 11.1 触发原因

S3 封口发现真缺陷 **D-S3-1**（待裁决 **Q-S3-1**）：`ControlState::resolve` 的 `min_size` 兜底写成 `max(size, min_size)` 形态，而 `min_size` 缺省为 `Vec2::ZERO` —— 于是**负宽高被压成 0**，与 §2.5 算式注「**不钳制负尺寸**（负宽高保留原值）」及 Q2 裁决 A 直接矛盾。

定位结论：**契约层语义缺陷，非提取层问题**。`nes-render-extract` 对负 anchor / offset / size 逐位透传、未做任何钳制；把 `min_size` 设为负值即可让负宽高原样返回，证明钳制来自 `min_size` 的"零下界"兜底而非专用分支。

用户裁决：**「修」**（即把契约层补齐到 Q2 的语义，而非改注释或不动）。

### 11.2 改动前后的签名

| 位置 | 改动前（v1.0） | 改动后（v1.1） |
|---|---|---|
| `ControlState::min_size` | `pub min_size: Vec2`（`new()` / `FULL_RECT` 均填 `Vec2::ZERO`） | `pub min_size: Option<Vec2>`（`new()` / `FULL_RECT` 均填 `None`） |
| `ControlState::resolve` | 恒存在下界：`if w < min_size.x { w = min_size.x }`（同 `h`） | 仅显式下界生效：`if let Some(min) = self.min_size { if w < min.x { w = min.x } … }` |
| `state.rs` 字段/函数文档 | 「本函数**不钳制负尺寸**：负宽高保留原值，语义留给调用方」 | 结论不变，补一句归因：「v1.1 起 `min_size` 缺省为 `None` = 无下界，因此缺省路径下负宽高必然原样透传」 |

语义（改后冻结）：

- `None`（缺省）= **无下界**：负宽高**原样透传**，`resolve` 出口逐位等于 `anchor * parent + offset` 算得的矩形；
- `Some(min)`：仅当宽/高小于 `min` 时**只推右/下边**，左上角不动（原冻结语义不动）；
- 任何取值下都**不存在**"把负值钳到 0"的分支。

成本与约束：`Option<Vec2>` 仍为 `Copy`、零堆分配，`ControlState` 保持 `Copy`；契约层依旧零依赖、零 `unsafe`（守卫 G3/G4 复测通过）。

### 11.3 受影响测试

| crate | 文件 | 改动 |
|---|---|---|
| `nes-render-api` | `tests/criterion_contract.rs` | 旧用例 `criterion_contract_control_min_size_grows_bottom_right_only` 的字段改为 `min_size: Some(Vec2::new(120.0, 90.0))`（**断言逐字未动**）；**新增** `criterion_contract_control_no_lower_bound_keeps_negative_size` —— 钉死"缺省 `None` + 负宽高原样透传（含正宽负高的单边为负）+ 负下界不引入钳制 + 显式正下界仍只推右下边" |
| `nes-render-extract` | `tests/criterion_gaps.rs` | 旧用例更名为 `criterion_gaps_control_negative_size_passes_through_and_is_not_clamped`，其中"契约层被压成 0"的缺陷现状断言改为"原样透传 `[5, 5, -30, -40]`"，并保留负下界归因对照 + 显式正下界对照；**新增** `criterion_gaps_control_negative_size_reaches_resolve_without_lower_bound` —— 端到端钉死：场景 `size = (-50, -60)` → 四边偏移 `(-40/-40)` → `resolve` 出口 `[210, 120, -50, -60]`，且提取层下界必须是 `None`；另 2 处 `min_size == Vec2::ZERO` 断言改为 `None`；文件头覆盖矩阵与两处文档注释同步修订 |
| — | 纪律 | **未删除任何旧用例、未放宽任何断言**；旧用例覆盖的"正下界只推右下边"语义仍被两侧各自钉死 |

### 11.4 是否破坏 S2 已落地的代码

**不破坏。** 唯一调用面是 `ControlState::new(anchors, offsets)`（构造签名与语义均未变）与 `resolve(parent_size)`（签名未变，出口只在"缺省下界"这一条路径上由"压 0"变为"保留原值"）。提取层构造 `ControlState` 时本就不设 `min_size`，源码级改动只有 1 处注释 —— 也就是说 S2/S3 的既有 24 项集成测试 + 6 项单测**一行未改地继续通过**。

### 11.5 受影响的不变量核对（§3 I1~I10）

逐条核对结论：**I1~I10 中无一条以 `min_size` 的类型或默认值作为前提**。与 Control 相关的只有 I6（帧内顺序里 `SetRect` 的位置）与 I7（`apply_item` 与逐项 setter 等价），二者只约束命令流形状与等价性，均不受本次改动影响。因此 §3 表**无需改形**；`ControlState` 的语义承载点是 §2.5 的算式条款（已就地加 v1.1 注）。Q2 的裁决结论（A：不钳制）也未变 —— 本次是**把实现补齐到 Q2 的语义**。

### 11.6 变更后实测（2026-09-26，本机 Windows 10 / cargo 1.98.1）

| 检查项 | 命令 | 实测 | 退出码 |
|---|---|---|---|
| 契约层测试 | `cargo test --all-targets`（`nes-render-api`） | 8 单测 + **32** 集成，0 failed（v1.0 为 31，+1 新增） | EXIT 0 |
| 契约层 lint | `cargo clippy --all-targets -- -D warnings` | 零警告零错误 | EXIT 0 |
| 提取层测试 | `cargo test --all-targets`（`nes-render-extract`） | 6 单测 + 24 旧集成 + **12** 缺口集成，0 failed（缺口集成 v1.0 为 11，+1 新增） | EXIT 0 |
| 提取层 lint | `cargo clippy --all-targets -- -D warnings` | 零警告零错误 | EXIT 0 |
| 依赖方向守卫 | `python check_dependency_direction.py` | **7/7 PASS**（G1~G7） | EXIT 0 |

§10.3 的三项要求（测试全绿 / clippy 零警告 / 守卫通过）**同时满足**。

本轮边界：未修改 `nes-scene`、`nes-asset` 任何文件；未触碰 `twn-render-stage` 等 TWN 封签资产（守卫 G5 复核：扫描源码零命中）。

日志落盘（中间产物，可复核）：`temp/v11_api_test.log`、`temp/v11_api_clippy.log`、`temp/v11_ext_test.log`、`temp/v11_ext_clippy.log`、`temp/v11_guard.log`、`temp/v11_exit.log`。

---

*（内容由AI生成，仅供参考）*
*（内容由AI生成，仅供参考）*
