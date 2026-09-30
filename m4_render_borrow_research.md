---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 19886e2d8bf8a6bde5831a704b5a86f4_a768e208b8df11f189c8525400393706
    ReservedCode1: BF/QJjlTmSUX336+awgKkB1WjIME7/Tg5kReWZFLPT7Csa8sTMUI/syxsXxz3IOqhThBLaNAj3+0a/RbWXL4VkpTRteSvbyRQqW3SICGYcTfaoRuXoDnEvlKayw0vPthGDZTTmF/VGCE9OQJO2gyRPTwsUi73uP5Yv2/6i/qFrjdnPGnKxAyw1kVXfg=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 19886e2d8bf8a6bde5831a704b5a86f4_a768e208b8df11f189c8525400393706
    ReservedCode2: BF/QJjlTmSUX336+awgKkB1WjIME7/Tg5kReWZFLPT7Csa8sTMUI/syxsXxz3IOqhThBLaNAj3+0a/RbWXL4VkpTRteSvbyRQqW3SICGYcTfaoRuXoDnEvlKayw0vPthGDZTTmF/VGCE9OQJO2gyRPTwsUi73uP5Yv2/6i/qFrjdnPGnKxAyw1kVXfg=
---

# NES 2.0 · M4 渲染接入 外部项目借鉴调研报告

- 日期：2026-09-25
- 检索方式：multi-search-engine skill（16 引擎：Google/Bing/DuckDuckGo/Baidu 等）+ GitHub / Gitee 站点定向检索 + 官方架构文档直读（Fyrox ARCHITECTURE.md、Godot RenderingServer 文档）
- 任务目标：为 M4「让节点/场景树成为渲染原生输入」找一个比既有 A（vendor-fork）/ B（薄适配层伪造 Scratch 快照）/ C（自研后端）更优或更强的做法
- 证据强度说明：本报告结论均来自公开 README / 架构文档 / 官方 API 文档，未逐一 clone 源码验证；来源已在第 6 节列明

---

## 0. 结论速览

有更优做法，且不是「三选一」，而是把 A 的方向**具体化**为一种被三家主流引擎同时验证过的范式，记为**方案 D：渲染服务端化 + 单向依赖 + 每帧提取**。

三条已被交叉验证的硬事实：

1. **渲染层的原生输入是「渲染物属性集合」，不是上层语义快照。**
   - Godot：整个场景系统「mount」在 RenderingServer 之上，server 是完全 opaque 的 API 后端，资源以 create_* 返回的 RID（不透明指针）表示。
   - scratch-render：渲染层只认 Drawable 的属性级更新接口（`updatePosition / updateDirection / updateScale / updateVisible / updateEffect`），不认积木语义。
   - 结论：M4 需要的是一份「节点属性 → 渲染物」的**属性级推送契约**，而不是去伪造 MotionRuntimeState/LooksRuntimeState。

2. **依赖方向必须单向：renderer → scene，scene 不得知道 renderer。**
   - Fyrox ARCHITECTURE.md 原文：*"a renderer **is** dependent on a scene, but scene does **not** know anything about the renderer"*。
   - 结论：B 方案（nes-scene 侧去构造 Scratch 快照）在依赖图上就是把 renderer 的语义要求倒灌进 scene，属于架构倒置，且会引入「双写语义」——与先前判断一致，本次检索进一步证伪。

3. **语义 → 渲染物的转换只允许发生一次，且发生在提取/推送那一步。**
   - Bevy：`ExtractSchedule` 只做「读主世界、写渲染世界」的数据搬运（ExtractComponent / ExtractResource），渲染世界不回头读主世界。
   - Realism（Rust crate）：`scene.extract(Some(view_proj))` 一次性吐出线性 `RenderCommand` 缓冲，渲染后端（wgpu/Vulkan/任意）自行消费，crate 本身 renderer agnostic。
   - 结论：M4 应是「节点树 → RenderItem/RenderCommand」单向提取；Scratch 语义只允许出现在 M5 兼容层，且位于渲染路径**之外**。

---

## 1. 可借鉴项目清单

| 项目 | 来源/平台 | 与 M4 的关系 | 可直接借鉴的点 |
|---|---|---|---|
| Godot RenderingServer（RID / canvas item） | 官方 API 文档 | 「场景树 ↔ 渲染后端」边界的成熟定义 | 场景系统挂载在 server 上；server 完全 opaque；资源用 `create_*` 返回 RID（不透明指针，非对象引用）；**可完全绕过场景系统直接用**；2D 走 canvas / canvas item |
| Fyrox（原 rg3d）ARCHITECTURE.md | GitHub | 依赖方向 + 命令队列范式 | 单向依赖（renderer→scene）；**不用 ECS**，用 generational arena/pool + handle —— 与 nes-scene 的 NodeId 模型天然同构；fyrox-ui 声明「does not render anything」，只产出 drawing commands，可接任意渲染器 |
| Bevy RenderWorld Extract | 官方博客 0.8 + 文档 | 提取式数据流的工业级实现 | `ExtractSchedule` / `ExtractComponent` / `ExtractResource`；`SyncToRenderWorld` + `RenderEntity↔MainEntity` 双向映射（等价于 NodeId↔DrawableHandle 绑定）；渲染世界与主世界可并行 |
| Realism（Rust crate，scene management） | GitHub | 与 NES 定位最接近：场景层可独立、再喂渲染器 | 「sits between your game logic and your renderer」；`Scene::extract()` → 线性 `RenderCommand`；renderer agnostic；generational index + 稀疏集；父链变换传播带 dirty flag；JSON 场景序列化 |
| scenix / scenix-scene | crates.io | 分层切分节奏 | 先做 **GPU-free scene graph**（节点层级/变换传播/遍历），渲染器与材质放到后续里程碑 —— 与 nes-scene 先于 nes-render 的排期一致 |
| scratch-render Drawable System | DeepWiki（源码级） | 渲染层「原生输入」的最好反例/正例 | Drawable 持有 position/direction/scale/visible/effects/skin，对外只有属性级 update API；渲染器输入是属性集合而非脚本语义 |
| TurboWarp scratch-vm | GitHub | M5 兼容层的对照物 | JIT 编译器；VM 与渲染完全分离，public API 与上游兼容 —— Scratch 语义的正确归属就是这一类「VM crate」，而不是渲染路径 |
| Myth（Gitee，wgpu 渲染引擎） | Gitee | 后端内部阶段编排 | 严格 SSA 的 RenderGraph：声明拓扑 → 自动拓扑排序、死阶段消除、激进临时内存别名、**每帧零分配重建**、可 headless（无窗口表面运行，利于 CI 回归） |
| Vello | 技术演讲 + GitHub | 2D 场景 → GPU 管线的现代做法 | compute shader 管线；刻意支持**多线程生成 2D 场景图**（场景准备与 GPU 提交解耦） |
| benzene（GitHub，Vulkan） | GitHub | crate / 目录切分命名参考 | domain-first 布局：`scene / assets / render / engine` 四层，与 nes-scene / nes-asset / nes-render 的切法一致 |
| Geb Engine（Gitee） | Gitee | 中文生态样本（弱相关） | ECS + 场景管理 + 资源管理 + 渲染系统的分层；仅作生态参考，架构无直接借鉴价值 |

---

## 2. 五种可借鉴架构模式（附最小接口骨架）

### 模式 1｜渲染服务端化（Godot RenderingServer / RID）

Godot 的做法是：场景树（Node/CanvasItem）与渲染后端之间隔着一个**服务端 API**，节点把自己的状态**推送**给 server，server 用不透明 RID 管理渲染侧对象；server 内部实现完全不可见，且允许绕过场景系统直接使用。

对 M4 的映射：不要「让 stage 去消费节点树的借用视图」，而是让一份 **RenderServer / ItemRegistry** 承接节点推送：

```rust
// nes-render-api（后端无关，scene 与 backend 都不依赖对方内部）
pub struct ItemHandle(u64);                      // 对应 Godot 的 RID / Bevy 的 RenderEntity

pub trait RenderServer {
    fn create_item(&mut self, key: RenderAssetKey) -> ItemHandle;
    fn destroy_item(&mut self, h: ItemHandle);
    fn set_visible(&mut self, h: ItemHandle, v: bool);
    fn set_transform(&mut self, h: ItemHandle, m: Affine);
    fn set_z(&mut self, h: ItemHandle, z: i32);
    fn set_flip(&mut self, h: ItemHandle, flip_h: bool, flip_v: bool);   // 补缺口：flip 合成
    fn set_camera(&mut self, cam: &Camera2DState);                      // 补缺口：视图矩阵
    fn set_text(&mut self, h: ItemHandle, text: &LabelState);           // 补缺口：Label
    fn set_rect(&mut self, h: ItemHandle, ctrl: &ControlState);         // 补缺口：Control 布局
    fn submit(&mut self, frame: &FrameInfo) -> Vec<RenderCommand>;
}
```

关键收益：Camera2D / Label / Control / flip 四个缺口落在**服务端契约**里，而不是散落在 stage 内部或 scene 内部。

### 模式 2｜单向依赖（Fyrox）

```text
nes-scene  ──✗──▶  nes-render-*      （scene 不得依赖任何渲染 crate）
nes-render-extract ──▶ nes-scene     （提取层依赖 scene，合法）
nes-render-backend ──▶ nes-render-api（后端依赖契约，不依赖 scene）
```

落地保障：CI 里加一条 `cargo metadata` 反向依赖检查（scene crate 的依赖树中出现 `nes-render-*` 即失败），把「架构不倒退」变成可自动验证的约束。

### 模式 3｜提取式适配（Bevy Extract / Realism extract）

```rust
// 每帧一次：读节点树，写渲染侧数据。渲染侧不回头读 scene。
pub fn extract(world: &SceneTree, reg: &AssetRegistry,
               server: &mut dyn RenderServer, map: &mut NodeItemMap) {
    for node in world.iter_depth_first_deterministic() {   // 复用 M1 的确定性遍历
        let Some(world_m) = node.world_affine_cached() else { continue };  // 复用 M1 的 Affine 缓存
        let item = map.get_or_create(node.id(), |k| {
            server.create_item(reg.render_key_of(k))       // M3 的 AssetKey → RenderAssetKey 桥
        });
        server.set_visible(item, node.visible);
        server.set_z(item, node.z_index);
        server.set_transform(item, world_m);
        // ...属性级推送
    }
}
```

要点：`NodeItemMap` 承载 NodeId↔ItemHandle 绑定，等价于 Bevy 的 `RenderEntity/MainEntity` 与 Godot 的 RID 生命周期管理。

### 模式 4｜Drawable 属性级注册表（scratch-render）

渲染层只暴露属性级 API（`updateVisible/updateEffect/...`），谁调用它不关心。这条直接支撑「B 不必存在」：如果渲染层的输入契约是属性级，那么 M5 的 Scratch 兼容层只需把自己翻译成节点树或 RenderItem，**不需要渲染层为 Scratch 语义开洞**。

### 模式 5｜RenderGraph（Myth SSA / Bevy render graph）——与接入方式正交

Myth 把渲染过程当编译问题（声明拓扑 → 自动排序 / 死阶段消除 / 零分配每帧重建）。这部分只影响**后端内部**阶段编排，不决定接入方式；可在 S4 后端替换时作为「自研后端」的性能与可测性参照（headless 可运行 → CI 回归友好）。

---

## 3. 对 M4 的建议：方案 D（A 的接口重塑 + 缺口补齐）

### 3.1 分层与依赖

| 层 | crate | 职责 | 依赖 |
|---|---|---|---|
| 场景层 | nes-scene（已封口 M1~M3） | 节点/场景树、变换、属性、AssetRegistry | 无渲染依赖 |
| 提取层 | nes-render-extract（新） | 每帧遍历 → 属性级推送；NodeId↔ItemHandle、AssetKey↔RenderAssetKey 桥 | → nes-scene, nes-render-api |
| 契约层 | nes-render-api（新） | RenderServer trait、RenderItem、RenderCommand、FrameInfo | 无 |
| 后端层 | nes-render-backend（A 的 fork 产物简化版） | 消费 Vec<RenderCommand>，负责 GPU 阶段编排 | → nes-render-api |

### 3.2 与既有三方案对照

| 维度 | A vendor-fork | B 薄适配层 | C 自研 | **D（建议）** |
|---|---|---|---|---|
| 方向正确性 | 正确，但「改输入源」表述含糊 | 倒置（scene 侧造快照） | 正确但重复劳动 | 正确且边界显式 |
| 语义来源 | 节点树 | 伪造 Scratch 快照 | 节点树 | 节点树（唯一来源） |
| 依赖方向 | 需人工守 | 被打破 | 需人工守 | 单向 + CI 可验证 |
| 缺口（Camera/Label/Control/flip） | 需在 fork 内改 | 更糟（快照里根本没有） | 需自研 | 落在契约层，可单测 |
| 工时（相对 A） | 1.0 | 0.7（但返工风险高） | ~2.0 | 1.3~1.5 |
| 可逆性 | 中 | 低（双写语义） | 高 | 高（后端可替换） |

### 3.3 M5 Scratch 兼容层的正确位置

TurboWarp/scratch-vm 的对照说明：Scratch 语义应留在独立的 VM 层，把自己翻译成节点树或 RenderItem，**不得进入渲染路径**。即：M5 = `Scratch VM → nes-scene 节点/属性`，而不是 `Scratch VM → renderer` 或 `renderer ← Scratch 快照`。

---

## 4. 分阶段落地（供 review，未开工）

| 阶段 | 内容 | 出口准则 |
|---|---|---|
| S1 | 冻结契约：`RenderServer` trait、`RenderItem`、`RenderCommand`、`FrameInfo` 定稿并回写设计文档 | 契约文档 + 空实现编译通过；依赖方向检查脚本就绪 |
| S2 | 提取层最小闭环：遍历 → create_item/set_transform/set_z/set_visible，NodeId↔ItemHandle 生命周期（含节点删除/资源消失） | 集成测试 `criterion_*`：变换传播一致、z 序稳定、节点增删无泄漏 |
| S3 | 缺口补齐：Camera2D 视图矩阵、Label 文本布局、Control 锚点布局、flip_h/flip_v 合成 | 四项各自独立测试 + 与 twn-render-stage 现有输出逐帧比对 |
| S4 | 后端替换：fork 产物瘦身为「消费 RenderCommand 的后端」，6C5 恢复逻辑保留；RenderGraph 内部编排可选 | 视觉回归（截图 diff）+ headless 跑通；clippy 0 警告 |
| S5 | 文档回写：实测偏差逐条成「v1.1 修订」小节（沿用 M1~M3 封口方法） | 里程碑表标注封口日期、测试计数、当轮真缺陷 |

---

## 5. 风险与验证点

1. **每帧分配**：提取层若每帧 clon 属性会退化。参照 Realism 的「线性 RenderCommand 缓冲」与 Myth 的「零分配每帧重建」，提取层应复用预分配缓冲与 ItemHandle 映射表。
2. **Label 归属**：文本布局属 CPU 侧（Fyrox 的 `formatted_text` 明确只做排版、不渲染），应落在提取层或契约层，不得下沉进 GPU 后端。
3. **不变量自动化**：`nes-scene` 依赖树中禁止出现 `nes-render-*`，用 CI 检查而非口头约定。
4. **B 的残余风险**：一旦允许 scene 侧构造 Scratch 快照，就会出现「节点属性」与「快照」双写，后续 M5 又会引入第三套语义，必须避免。
5. **证据边界**：本报告未逐仓 clone 验证实现细节，S1 前建议对 Realism / scenix / Fyrox 的提取层各抽一次源码细读（重点：句柄生命周期与资源失效处理）。

---

## 6. 来源

- Godot RenderingServer 文档：https://docs.godotengine.org/en/4.4/classes/class_renderingserver.html
- Godot Rendering Architecture 综述：https://deepwiki.com/godotengine/godot-docs/6.3.2-rendering-architecture
- Fyrox ARCHITECTURE.md：https://raw.githubusercontent.com/FyroxEngine/Fyrox/master/ARCHITECTURE.md ；主页 https://fyrox.rs/
- Bevy 0.8 Render World Extract：https://bevy.org/news/bevy-0-8/ ；渲染管线架构：https://deepwiki.com/bevyengine/bevy/5.1-render-pipeline-architecture
- Realism（scene management crate）：https://github.com/saptak7777/realism
- scenix / scenix-scene：https://crates.io/crates/scenix-scene/0.2.0
- scratch-render Drawable System：https://deepwiki.com/scratchfoundation/scratch-render/2.2-drawable-system
- TurboWarp/scratch-vm：https://github.com/TurboWarp/scratch-vm
- Myth（Gitee）：https://gitee.com/chong00li/myth
- Vello 技术演讲：https://rustlab.it/talks/vello-high-performance-2d-graphics
- benzene：https://github.com/sauravniraula/benzene
- Geb Engine（Gitee）：https://gitee.com/byusi/geb_engine
- arewegameyet 2D 渲染生态：https://arewegameyet.rs/ecosystem/2drendering/
*（内容由AI生成，仅供参考）*
