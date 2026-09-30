---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 19886e2d8bf8a6bde5831a704b5a86f4_549237b6b8a311f1b172525400248c00
    ReservedCode1: grhg3yHfZFUJx2UvnP1mKFIKRgVMSrvDMIouDI1NjCv+VY3vYGKI1c665URUKjOHc1rxlOFKE3aUKOcEOfm8qolL6nnkf0MkC9C52aSFneh6tLNVK1107HHl2kq+GcHaiBvwDn5P5isXIz2dCDdsMfCKs/ZKQqSykjcnTXiS78oykq4hxjkKgK4L3MY=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 19886e2d8bf8a6bde5831a704b5a86f4_549237b6b8a311f1b172525400248c00
    ReservedCode2: grhg3yHfZFUJx2UvnP1mKFIKRgVMSrvDMIouDI1NjCv+VY3vYGKI1c665URUKjOHc1rxlOFKE3aUKOcEOfm8qolL6nnkf0MkC9C52aSFneh6tLNVK1107HHl2kq+GcHaiBvwDn5P5isXIz2dCDdsMfCKs/ZKQqSykjcnTXiS78oykq4hxjkKgK4L3MY=
---

# NES 2.0 通用节点 / 场景树 接口草案 v1

- 定位：P0 阶段核心子系统设计，是「类 Godot 节点管理 + 全局资源」的地基
- 前置约束：优先复用既有 TWN 资产；已 DRIVE SEALED 的包（TWN-0~5A.3.20、6A~6C5、6D0A~6D0G）不得改内部成员，本设计一律落在独立工程 `F:\All NGVGE\NES 2.0`
- 硬目标：零代码基础用户可做游戏（编辑器可完整描述与序列化整个场景）
- 留缝：JS 扩展兼容后做，但本设计必须预留与 `twn-shadow`（Scratch 语义层）的映射接口，不得堵死
- 草案日期：2026-09-25

---

## 0. 设计立场

TWN 既有分层已给出三条可复用的纪律，本设计直接继承：

| 既有纪律 | 出处 | 本设计沿用方式 |
|---|---|---|
| 稳定身份与临时句柄分离 | `RenderAssetKey` vs `TextureHandle` | `NodeId`（稳定）vs `NodeHandle`（临时） |
| 后端中立契约 + `forbid(unsafe)` | `twn-backend-contracts` | 场景树核心不含 unsafe，不吃渲染后端细节 |
| 确定性调度与预算 | `twn-scheduler`（warp 500ms / work budget 0.75） | 遍历顺序必须全序确定，可 trace 回放 |

**同时继承一条负面教训**：TWN 现有运行时是 `Scratch Target + Drawable 平铺 + layer`，**没有通用树**。本设计不是改造它，而是**平行新建一层**，靠适配器对接。

---

## 1. 设计原则（五条）

1. **数据与行为分离**：节点数据住 arena，行为由 schema/脚本描述。不用 `Rc<RefCell<dyn Node>>` 满天飞——那会立刻撞上借用检查与序列化双重墙。
2. **结构变更延迟应用**：遍历过程中禁止直接改树，全部进 `pending_ops`，帧首统一落地（等价 Godot `call_deferred`）。这是避免遍历器失效与不确定性的唯一低代价方案。
3. **全序确定**：兄弟节点用显式 `order: u64` 排序键，同键用 `NodeId.slot` 兜底。任何遍历结果可复现。
4. **反射优先**：节点的一切可在编辑器暴露的属性，必须由 `NodeSchema` 描述。没有反射就没有零代码编辑器，也没有可视化脚本。
5. **核心不依赖渲染**：M1/M2 可在无 GPU 前提下全量单元测试通过。

---

## 2. 身份模型

沿用 `RenderAssetKey` 的双身份纪律：

```rust
/// 稳定身份：可序列化、跨帧不变、跨存档不变。存档/引用/脚本里只准出现这个。
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct NodeId { slot: u32, gen: u32 }

/// 临时句柄：运行时缓存用，帧内有效，不序列化。
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct NodeHandle(NonZeroU64);
```

规则（与渲染资产同构，便于统一心智模型）：
- `NodeId` 删除后其 `gen` 失效，永久不复用同一 `(slot, gen)`。
- 任何持久化结构（场景文件、脚本引用、信号连接表）**只准**存 `NodeId`。
- `NodeHandle` 只允许出现在帧内缓存与热路径索引中。

---

## 3. 存储布局

```rust
pub struct SceneTree {
    nodes: SlotMap<NodeId, NodeData>,   // 主 arena
    root: NodeId,
    pending_ops: Vec<TreeOp>,           // 延迟结构变更
    order_seq: u64,                     // 单调递增排序键分配器
    free_handle_map: HashMap<NodeId, NodeHandle>,
    groups: HashMap<InternedString, Vec<NodeId>>,   // 组索引（可重建）
    frame: u64,
}

pub struct NodeData {
    kind: NodeKind,
    name: InternedString,
    parent: Option<NodeId>,
    children: Vec<NodeId>,      // 已按 order 排序（不变式）
    props: PropStore,           // 反射驱动
    local: Transform2D,
    world: Transform2D,         // 惰性求解缓存
    flags: NodeFlags,           // DIRTY_XFORM | ENTERED | READY | VISIBLE | ...
    order: u64,
    script: Option<ScriptSlot>,
    meta: MetaMap,
}
```

**为何不用 `Rc<RefCell>` / 父子双向强引用**：
- 借用地狱 + 运行时 panic 风险；
- 无法可靠序列化；
- 热重载时无法替换节点实现；
- 无法做确定性遍历（指针顺序不稳定）。

**SlotMap 代价**：删除有空洞。可接受——`gen` 校验能兜住悬垂，且 arena 比指针链表缓存友好。

---

## 4. 节点分类：enum dispatch（非 trait object）

Godot 是继承式（`Node` → `Node2D` → `Sprite2D`）。Rust 无继承，两条路线：

| 路线 | 优点 | 缺点 | 结论 |
|---|---|---|---|
| `Box<dyn NodeBehavior>` | 扩展开放 | 无法序列化、无法枚举属性、对象安全限制、动态派发 | ✗ |
| **enum dispatch** | 可序列化、可枚举 schema、内联、编译期穷尽检查 | 加节点类型需改内核 | **✓ 采用** |
| 纯 ECS | 组合灵活 | 零代码用户无法理解「实体/组件」，与 Godot 心智模型冲突 | ✗ |

```rust
pub enum NodeKind {
    Node(Node),             // 纯容器
    Node2D(Node2D),         // 带变换、可渲染基类
    Sprite2D(Sprite2D),     // 贴图
    AnimatedSprite2D(..),
    Camera2D(..),           // 视口
    Control(Control),       // UI 基类：锚点/边距
    Label(Label),
    Button(Button),
    AudioPlayer(..),
    Timer(..),
    Script(ScriptNode),     // 用户脚本节点（挂载脚本到通用容器）
    // 内建类型按需扩充
}
```

**关键取舍**：用户自定义节点不做成新 enum 分支，而是「通用容器 + 挂载脚本」。这样内核枚举保持封闭（可穷尽 match、可序列化），扩展性由脚本层消化——也正好是零代码用户的路径（拖节点 → 挂脚本）。

`ScriptNode` 是通往可视化脚本与（后置的）JS 扩展的同一个挂载点。

---

## 5. 属性反射（零代码地基）

```rust
pub struct NodeSchema {
    pub tag: NodeKindTag,
    pub display: &'static str,
    pub base: Option<NodeKindTag>,
    pub props: &'static [PropSchema],
}

pub struct PropSchema {
    pub name: InternedString,
    pub ty: PropType,
    pub default: Value,
    pub hint: EditorHint,        // 决定编辑器用哪种控件
    pub flags: PropFlags,        // READONLY | EDITOR_ONLY | STORAGE | NET
}

pub enum PropType {
    Bool, Int, Float, Vec2, Rect2, Color, String,
    Enum(&'static [&'static str]),
    Asset(AssetKind),            // 引用全局资源
    NodeRef,                     // 引用树内节点
    Flags(&'static [&'static str]),
    Null,
}

pub enum Value { Null, Bool(bool), Int(i64), Float(f32), Vec2(Vec2), Rect2(Rect2),
                 Color(Color), Str(Arc<str>), EnumIdx(u16), Asset(AssetKey), Node(NodeId) }
```

约束：
- 属性读写统一走 `get_prop(&self, NodeId, PropId) -> Option<Value>` / `set_prop(..) -> Result<(), PropError>`。
- 直接字段访问仅限内核内部；脚本/编辑器/序列化一律走反射通道。
- `EditorHint` 必须能驱动生成控件（Range/Slider/ColorPicker/FilePicker/NodePicker/Dropdown），否则零代码目标不成立。
- `Asset(AssetKind)` 与 `NodeRef` 让编辑器自动出资源选择器与节点选择器——这两项占了编辑器工作量的七成。

---

## 6. 树操作 API

```rust
pub enum TreeOp {
    Add { parent: NodeId, kind: NodeKind, name: String, at: Option<usize> },
    Remove { node: NodeId, keep_children: bool },
    Reparent { node: NodeId, new_parent: NodeId, at: Option<usize> },
    Rename { node: NodeId, name: String },
    Move { node: NodeId, new_index: usize },
}

impl SceneTree {
    // 即时查询（只读，遍历中安全）
    pub fn get(&self, id: NodeId) -> Option<&NodeData>;
    pub fn parent(&self, id: NodeId) -> Option<NodeId>;
    pub fn children(&self, id: NodeId) -> &[NodeId];
    pub fn node_path(&self, id: NodeId) -> NodePath;              // 稳定路径（含 index 去歧义）
    pub fn find(&self, from: NodeId, path: &NodePath) -> Option<NodeId>;
    pub fn find_by_name(&self, from: NodeId, name: &str, recursive: bool) -> Option<NodeId>;
    pub fn group(&self, g: &str) -> &[NodeId];
    pub fn is_ancestor_of(&self, a: NodeId, b: NodeId) -> bool;

    // 结构变更（延迟，遍历中调用安全）
    pub fn queue(&mut self, op: TreeOp) -> NodeId;                // Add/Reparent 返回预留 id
    pub fn apply_pending(&mut self) -> Vec<TreeEvent>;            // 帧首调用，返回结构事件

    // 生命周期驱动（内核调度器调用）
    pub fn begin_frame(&mut self) -> FrameCtx;
    pub fn process_frame(&mut self, ctx: &mut FrameCtx, delta: f32);
    pub fn end_frame(&mut self, ctx: FrameCtx);
}
```

**不变式**（必须有单元测试守住）：
1. `children` 恒按 `order` 升序。
2. `parent` 与 `children` 双向一致。
3. 不存在环（`Reparent` 校验：新父不得是自身后代）。
4. `apply_pending` 幂等性：同一批事件应用一次即稳定。
5. 遍历中调用 `queue` 不得使当前遍历迭代器失效。

**节点路径**：`NodePath` 需支持两种段——按名 `Player`、按位置 `Player:Sprite2D[0]`。重名节点自动改名（`Sprite2D`→`Sprite2D2`）以保路径可用，规则须在文档层固定，否则编辑器与脚本会互相打脸。

---

## 7. 生命周期回调与确定性遍历

参考 Godot 但收敛为最小集：

| 回调 | 时机 | 次数 |
|---|---|---|
| `enter_tree` | 节点首次进入活动树 | 每次入树一次 |
| `ready` | 子树全部进入树后，**自底向上** | 每次入树一次 |
| `process(delta)` | 每帧，**自顶向下** | 每帧 |
| `exit_tree` | 离开活动树 | 每次离树一次 |

顺序规则（写死，不得依赖实现细节）：
- 遍历一律**深度优先、子节点按 `order` 升序**。
- `ready` 自底向上（子先于父），与 Godot 语义一致——允许父在 `ready` 中读到已就绪的子。
- 同一帧内入树的新节点，其 `enter_tree`/`ready` 在该帧 `process` **之前**完成。

---

## 8. 变换与脏传播

```rust
pub struct Transform2D { pub pos: Vec2, pub rot: f32, pub scale: Vec2, pub skew: f32 }
```

- 节点只存 `local`；`world` 是缓存。
- `set_local()` 置 `DIRTY_XFORM` 并**向上传播**标记祖先（用于快速判断整棵子树是否需重算）。
- **帧内集中求解**：`process` 全部结束后统一 flush，自顶向下一次算完；渲染阶段只读 `world`。
- 代价：同一帧内脚本读到的 `world` 是上一帧值。**这是刻意设计**（与 Godot 类似），需在文档显式声明，避免用户以为读到实时值。
- 提供 `world_now(&self, id) -> Transform2D` 显式即时求解接口，供确实需要的场景（如追尾摄像机）。

---

## 9. SceneTree 运行时语义

```rust
pub struct SceneTreeRt {
    tree: SceneTree,
    root: NodeId,                     // 根节点（含 viewport 语义）
    paused: bool,
    time_scale: f32,
    process_mode: ProcessMode,        // 节点级，见下
    signal_bus: SignalBus,
    frame: u64,
}

pub enum ProcessMode { Inherit, Pausable, WhenPaused, Always, Disabled }
```

- `paused` 只影响 `Pausable` 与 `Inherit`（继承父链）节点；`Always` 不受影响（UI、存档点必需）。
- `time_scale` 只缩放 `delta`，不改变遍历次数（确定性优先）。
- **暂停不是停止遍历**，是 `delta = 0` 且跳过 `Pausable` 的 `process`——结构变更与信号仍然工作，否则暂停期间 UI 会僵死。

---

## 10. Scene 与实例化

```rust
pub struct SceneDesc {         // 可序列化模板
    pub version: u16,
    pub root: usize,
    pub nodes: Vec<NodeDesc>,  // 平坦数组 + 索引子表，避免递归深度问题
}

pub struct NodeDesc {
    pub kind: NodeKindTag,
    pub name: String,
    pub props: Vec<(String, Value)>,
    pub children: Vec<usize>,
    pub script: Option<ScriptDesc>,
    pub meta: Vec<(String, Value)>,
}
```

API：

```rust
impl SceneTree {
    pub fn instantiate(&mut self, scene: &SceneDesc, parent: NodeId) -> NodeId;
    pub fn pack(&self, subtree_root: NodeId) -> SceneDesc;   // 反向导出（编辑器保存）
}
```

- **模板与实例分离**：`SceneDesc` 是只读模板，实例化产生全新 `NodeId`，不复用模板内 id。
- 支持嵌套子场景（`sub_scene` 属性持 `AssetKey`），实例化时递归展开——但需保留「子场景根」标记，否则编辑器无法回写。
- **序列化格式**：首发 **RON**（可读、可 diff、Rust 友好），JSON 作为交换格式并行支持，二进制格式后置。理由：当前阶段人可手改场景文件的收益远大于体积。
- 加载必须容忍未知属性（前向兼容）与缺失属性（回落默认值），`version` 用于迁移。

---

## 11. 全局资源：AssetRegistry

```rust
#[derive(Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetKey { slot: u32, gen: u32, kind: AssetKind }

pub enum AssetKind { Texture, Audio, Font, Scene, Script, Shader, Data }

pub enum LoadState {
    NotLoaded,
    Queued,
    Loading,
    Ready(Arc<LoadedAsset>),
    Failed(Arc<str>),
}

pub struct AssetEntry {
    pub key: AssetKey,
    pub path: PathBuf,
    pub version: u32,          // 热重载递增，key 不变
    pub refs: u32,             // 引用计数
    pub state: LoadState,
    pub dependents: Vec<AssetKey>,
}
```

规则：
- **key 不随内容变化**（与 NodeId 同纪律）→ 热重载时 key 稳定，只 `version` 递增。
- 加载状态机单向推进：`NotLoaded → Queued → Loading → Ready|Failed`；`Failed` 可重试回 `Loading`。
- 依赖图显式：场景 → 纹理/音频/脚本。卸载按引用计数 + 依赖可达性判定，**不做无引用即卸**（避免卡顿）。
- 热重载：文件 mtime/hash 变化 → 重新加载同 key → 版本递增 → 广播 `AssetReloaded{key, version}` → 持有者决定是否重建派生数据。
- 与 `twn-render-resources` 的 `RenderAssetKey` 关系：**上层统一**。`RenderAssetKey` 视为 `AssetRegistry` 中 `Texture` 类的一面视图，避免出现两套资源身份。

### 11.1 v1.1 修订（M3 落地实况，实测驱动）

M3 已封口，落地时对上面骨架做了 5 处修订：

1. **身份判据改为「路径 + 类别」**（草案只写了 `(slot, gen, kind)`）：`AssetKey` 实际由
   `AssetPath + AssetKind` 派生，全局稳定、可序列化；`(slot, gen)` 只作为
   `RenderAssetKeyView` 的位编码镜像给渲染侧。存档里写路径而非槽位号，跨会话/跨机不失效。
2. **`LoadState` 拆成 `StateTag` + 注册表持有内容**：草案把 `Arc<LoadedAsset>` 塞进状态枚举，
   会让状态变重载荷、且热重载必须换枚举值；实测拆为 `Copy` 的状态标记
   （`NotLoaded/Queued/Loading/Ready/Failed`，单向推进）与注册表内的 `Arc<LoadedAsset>`，
   热重载只让 `version` 递增。
3. **热重载判据 = 长度 + 内容哈希（fnv1a64），不是 mtime**：编辑器「保存但内容没变」
   不触发重建；mtime 只作廉价预筛。判失败不 panic，槽位转 `Failed` 等重试。
4. **场景侧新增 `ResourceTable`（草案缺这层）**：节点属性里的 `Resource(u64)` 是
   **场景内槽位号**，`ResourceTable` 负责「槽位号 →(声明) 路径/类别 →(绑定) AssetKey」三层映射；
   槽位号随场景序列化（`resources:` 段），**0 保留为未绑定**；旧场景无该段仍可读，
   被引用未声明的号降级为**悬垂项**（可编辑、可补声明，不报错、不丢数据），
   类别对不上进 `mismatches` 体检报告。
5. **不做反向依赖索引**：`dependents` 由 `deps` 扫描得出（两份事实来源易漂移）；
   卸载在 `unload_tick` 帧末批量判定，`refs == 0` 只是候选资格而非卸载命令，
   且**任何仍被存活者依赖的资源不卸载**。

---

## 12. 信号 / 事件

```rust
pub struct SignalBus {
    subscribers: HashMap<SignalId, Vec<Subscriber>>,   // Subscriber = { node: NodeId, method: InternedString }
}

impl SignalBus {
    pub fn connect(&mut self, src: NodeId, sig: SignalId, dst: NodeId, method: InternedString);
    pub fn disconnect(&mut self, ..);
    pub fn emit(&mut self, src: NodeId, sig: SignalId, args: &[Value]);   // 入队，帧末派发
}
```

- 派发**入队**，帧末统一 flush，禁止 emit 中直接递归调用（否则可重入死锁）。
- 连接表只存 `NodeId`，节点销毁时自动清理悬挂连接（否则是经典内存与崩溃来源）。
- 信号是将来桥接两个东西的同一根管道：可视化脚本的事件流、以及 Scratch 的广播/`runtime.on`。

---

## 13. 与既有内核的边界（crate 划分）

新增 crate（均在 `F:\All NGVGE\NES 2.0` 独立工程内，不触碰封签包）：

| crate | 职责 | 依赖 |
|---|---|---|
| `nes-scene` | 身份 / SlotMap / 树操作 / 属性反射 / SceneTree / 信号 | `twn-core`、`nes-asset` |
| `nes-asset` | AssetRegistry / 加载状态机 / 依赖图 / 热重载 | `twn-core`、`twn-render-resources` |
| `nes-scene-compat`（后置） | Node ↔ Scratch Target 映射，JS 扩展兼容用 | `nes-scene`、`twn-shadow` |

依赖方向（单向，不得反流）：

```
nes-app / nes-editor
      ↓
   nes-scene ──→ nes-asset
      ↓              ↓
   twn-core    twn-render-resources
      ↓
twn-runtime-state / twn-scheduler / twn-trace
      ↓
twn-backend-contracts ──→ null | wgpu | audio-miniaudio | platform-winit | asset-decode-resvg
```

**红线**：`nes-scene` 不得依赖任何具体后端（wgpu / miniaudio / winit），否则将来换后端要动核心；渲染接入由 `twn-render-stage` 通过只读遍历接口消费，而非核心反向调用渲染。

---

## 14. 与 Scratch 语义层的映射留缝（现在不动，但现在要留）

JS 扩展兼容整体后置，但以下接口**必须现在预留**，否则将来返工：

| Scratch 概念 | NES 2.0 对应 | 预留接口 |
|---|---|---|
| Target（舞台/角色） | `Node2D` 子树 + `TargetMirror` 标记 | `NodeFlags::SCRATCH_TARGET` |
| 造型 / 背景 | `Sprite2D` 的 `Asset(Texture)` | 已覆盖 |
| 变量 / 列表 | 节点属性 + 全局资源 | `PropStore` 已支持 |
| 广播 / `runtime.on` | `SignalBus` | 同一根管道 |
| `runtime.targets` | 树的目标索引视图 | `SceneTree::targets_view() -> &[NodeId]`（只读） |
| `runtime.ext_*` 跨扩展互访 | 扩展实例注册表 | `ScriptSlot` 预留 `registry_key` |

核心承诺：**兼容层只能通过只读视图 + 命令队列访问 `nes-scene`**，不得让 Scratch 语义侵入核心数据结构。

---

## 15. 确定性保证与 trace 挂钩

- 每帧 `end_frame` 输出结构事件序列（`Add/Remove/Reparent/Rename/Move` + `NodeId` + `order`）到 `twn-trace`，可回放。
- 遍历顺序、信号派发顺序、`ready` 顺序全部由 `order` + `slot` 决定，禁止依赖 `HashMap` 迭代顺序。**任何使用 `HashMap` 参与遍历的代码点视为缺陷**（组索引、信号订阅表均需在遍历前排序）。
- 浮点：`delta` 由调度器统一供给，节点不得自取时间源。

---

## 16. Rust 接口骨架（摘要）

```rust
// ——— 身份 ———
pub struct NodeId { slot: u32, gen: u32 }

// ——— 节点 ———
pub enum NodeKind { Node(Node), Node2D(Node2D), Sprite2D(Sprite2D),
                    Control(Control), Label(Label), Script(ScriptNode) }

// ——— 属性 ———
pub trait Reflect {
    fn schema(&self) -> &'static NodeSchema;
    fn get(&self, p: PropId) -> Option<Value>;
    fn set(&mut self, p: PropId, v: Value) -> Result<(), PropError>;
}

// ——— 行为 ———
pub trait Behavior {
    fn enter_tree(&mut self, ctx: &mut NodeCtx<'_>);
    fn ready(&mut self, ctx: &mut NodeCtx<'_>);
    fn process(&mut self, ctx: &mut NodeCtx<'_>, delta: f32);
    fn exit_tree(&mut self, ctx: &mut NodeCtx<'_>);
}

// NodeCtx 是唯一向行为暴露的写入口（命令队列 + 只读树 + 信号发射），
// 保证行为代码无法绕过不变式直接改树。
pub struct NodeCtx<'a> {
    pub this: NodeId,
    tree: &'a SceneTree,
    cmds: &'a mut CommandBuf,
    signals: &'a mut SignalBus,
}
```

`NodeCtx` 的形态是本设计里**最关键的一条接口纪律**：行为代码只拿到只读树 + 命令缓冲，从类型层面就无法绕过不变式。

---

## 17. 里程碑与出口准则

| 里程碑 | 交付 | 出口准则（可验证） |
|---|---|---|
| **M1 骨架** | 身份 + SlotMap + 树操作 + 确定性遍历 + 变换传播 | 单测证明：遍历顺序稳定复现；`Reparent` 拒绝成环；遍历中 `queue` 不崩；脏传播正确 |
| **M2 反射与场景** | NodeSchema 全量 + 反射读写 + Scene 序列化/实例化/打包 | 场景保存 → 加载 → 实例化 → 再打包，两棵树逐节点属性与顺序完全一致 |
| **M3 全局资源** ✅ 已封口 | AssetRegistry + 状态机 + 依赖图 + 热重载广播 + `ResourceTable`（把节点属性里的 `Resource(u64)` 接真注册表） | 改文件后订阅者收到同 key 新版本；引用计数与依赖卸载判定正确 |
| **M4 渲染接入** | `twn-render-stage` 消费节点树（只读） | 无 GPU 环境下 M1~M3 全绿；有 GPU 时渲染结果与树状态一致 |
| **M5（后置）** | `nes-scene-compat`：Scratch Target 映射 | 单个真实扩展可在节点树上跑通 |

**M3 封口实况（2026-09-25）**：`nes-asset` 34 测试 + `nes-scene` 75 测试全绿
（含验收 `nes-scene/tests/m3.rs` 12 例，覆盖资源绑定三层互查、悬垂降级、类别冲突体检、
热重载版本递增 / touch 不算重载 / 源消失转 `Failed`、依赖边与帧末卸载闭包、成环点名、
声明随场景往返不漂、旧场景无 `resources` 段可读），`cargo clippy --all-targets -D warnings` 0 警告。
封口时修掉一个真缺陷：`Res(...)` 的 `kind:` 字段取值前未跳空白，导致**自己写出的 RON 自己读不回**
（`path:` 因走字符串解析器恰好掩盖了该问题）——已加 `skip_trivia` 并留了往返测试。

**P0 出口定义**：M1 + M2 通过，即「能用编辑器描述一棵树、存盘、读回、原样运行」。
**P1 出口定义（M3 达成）**：树上的资源引用不再是占位号——声明、加载、热重载、卸载全链路可验证。

---

## 18. 待拍板点（需你裁定，我不替你决定）

| # | 议题 | 我的建议 | 影响面 |
|---|---|---|---|
| 1 | 节点分类：enum dispatch vs trait object | **enum dispatch** | 决定可序列化性与编辑器可行性 |
| 2 | 序列化首发格式 | **RON**（JSON 并行） | 决定场景文件可读性与 diff 体验 |
| 3 | 是否引入 ECS | **不引入** | 与零代码用户心智模型直接冲突 |
| 4 | 新 crate 命名（`nes-scene` / `twn-scene`） | `nes-` 前缀，与封签资产显式区隔 | 影响将来审计边界清晰度 |
| 5 | `world` 变换的帧内可见性（上一帧值 vs 即时求解） | **上一帧 + 显式即时接口** | 影响脚本语义与用户预期 |
| 6 | 是否现在定义可视化脚本的数据格式 | **先定属性反射粒度，格式后置** | 反射粒度定错会返工 |

---

## 附：本草案与上一轮扩展扫描的衔接

上一轮扫描确认：JS 扩展生态「后置」不改变架构约束，但**留了三条缝必须现在保留**：
1. `SignalBus` 同时承载可视化脚本事件与 Scratch 广播 —— 已在本设计第 12 节覆盖。
2. `ScriptNode` 作为唯一脚本挂载点，同时服务可视化脚本与将来的 JS 扩展 —— 已覆盖，并预留 `registry_key` 支持 `runtime.ext_*` 跨扩展互访。
3. `AssetRegistry` 与 `RenderAssetKey` 上层统一，避免将来出现两套资源身份 —— 已覆盖。
*（内容由AI生成，仅供参考）*
