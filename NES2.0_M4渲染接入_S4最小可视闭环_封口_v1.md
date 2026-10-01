# NES 2.0 · M4 渲染接入 S4 最小可视闭环 封口 v1

> 封口日期：2026-09-30　｜　状态：**S4.1 封口（最小可视闭环已打通并实机验证）**
> 前置：S1 契约冻结、S2 提取层封口、S3 四项缺口封口（各文档见本目录）。
> 本文档记录 S4.1 的出口准则对照、源码改动、运行时实证修正、架构裁决与实测基线，
> 供后续里程碑（多纹理、Label/Control 光栅化、headless Linux 认证）直接引用。

---

## 0. 一句话结论

`nes-render-wgpu` 后端在本机（Intel Iris Xe / Vulkan / wgpu-native v29.0.1.1）**实机跑通了
「契约命令流 → 清屏 + 精灵 → 离屏读回 → PNG」的最小可视闭环**：五 crate 全绿
（测试 75 / 34 / 40 / 42 / 15），clippy 零警告，依赖方向守卫扩至 **G1~G10 全过**，
示例 `s41_visual_closure` 逐像素断言 PASS 且经外部解码器交叉验证。

---

## 1. 出口准则对照

| 出口准则（S4.1 立项口径） | 实测 | 判定 |
|---|---|---|
| lib 构建通过（原 2 错误：E0583 缺 renderer.rs、E0164 MapFailed 分支形状） | `cargo build --lib` EXIT 0 | ✅ |
| Rust 侧消费 `RenderCommand`，经 wgpu-native 画出清屏 + 精灵 | `CommandConsumer::consume` 全链路实机出图 | ✅ |
| 离屏纹理 → 读回 → PNG 落盘 | `output/s41_visual_closure.png`（64x64，16,516 字节） | ✅ |
| 与原型 `s41_probe3.log` 锚点比对 | `px(10,10)`=红、`px(13,13)`=近白、背景=深藏青，逐项一致 | ✅ |
| 外部解码器交叉验证（PNG 编码器自证不自洽） | System.Drawing 回读四锚点一致 | ✅ |
| 失败路径如实报告（不伪造截图、不谎报跑通） | `LibraryNotFound` / `MissingSymbol` / `ConfigMismatch` / `MalformedCommandStream` / `driver_errors` 均有测试或证据覆盖 | ✅ |
| 变更验收三联（test 全绿 + clippy 零警告 + 守卫全过） | 五 crate 满足（见 §4 基线） | ✅ |

---

## 2. 源码改动清单（相对归档说明 v1 的快照）

| 文件 | 改动 |
|---|---|
| `nes-render-wgpu/src/renderer.rs` | **新增**（约 900 行）：`WgpuRenderServer`（契约簿记）、`CommandConsumer`（GPU 执行器）、`FrameStats` / `FrameOutcome`（统计与像素结果 + `write_png`）、`SpritePipeline`（WGSL 着色器 + 实例管线）+ 4 条单测 |
| `nes-render-wgpu/src/error.rs` | `MapFailed` 显示分支改结构体模式并输出 `message`；**补上 Display 整条缺失的 `ConfigMismatch` 分支**（既有潜伏 E0004，被 E0583 掩盖） |
| `nes-render-wgpu/src/gpu.rs` | 图集缓冲用法位 `VERTEX` → `UNIFORM\|COPY_DST`（原绑定不合法且不可写）+ 标签更正；`ATLAS_PX` 256 → **64**（原值使 `CELL_PX=64`，与"16px 格"注释自相矛盾）；采样器 `max_anisotropy` 0 → 1；`FrameImage` 通道语义翻正（字段 `bgra` → `rgba`，`pixel()` 去掉交换，`storage_format` = RGBA8Unorm）+ 摘要式 `Debug`；新增 `view_uniform()` / `sampler()` 访问器；3 处 `field_reassign_with_default` clippy 修正 |
| `nes-render-wgpu/src/ffi.rs` | 新增常量 `WGPU_BUFFER_USAGE_UNIFORM`、`WGPU_FRONT_FACE_CCW`（=1）、`WGPU_DEPTH_SLICE_UNDEFINED`（=u32::MAX） |
| `nes-render-wgpu/src/lib.rs` | 更正"本机没有 MSVC 链接器"的过期口径（归档说明 §4.11 记账项） |
| `nes-render-wgpu/examples/s41_visual_closure.rs` | **新增**：最小可视闭环示例（装配报告 + 统计 + 逐像素断言 + PNG + 外部交叉验证指引） |
| `nes-render-wgpu/tests/criterion_backend.rs` | **新增**：7 条出口准则测试（失败路径 ×2 无 GPU 依赖 + GPU 用例 ×5） |
| `check_dependency_direction.py` | 扩展 **G8 / G9 / G10**：后端 crate 依赖边正向钉住、反向围堵、零第三方依赖 + 无 build.rs + 独立工作区根 |
| `wgpu-win/`（本目录内） | 解压 wgpu-native v29.0.1.1 release 资产（`include/` + `lib/wgpu_native.dll` + `wgpu-native-meta/`），来源 `F:\All NGVGE\WGPU\wgpu-windows-x86_64-gnu-release.zip`，落在 `locate_library` 候选 2 的约定路径 |

**未触碰**：`nes-scene` / `nes-asset` / `nes-render-api` / `nes-render-extract` 四个已封口 crate 零改动（冻结面完好，S1 §10 变更纪律无需触发修订小节）。

---

## 3. 运行时实证修正记录（每条：症状 → 证据 → 修法）

前两条是编码期预判，其余均在实机运行中由驱动校验、崩溃或像素证据暴露：

| # | 症状 | 证据 | 修法 |
|---|---|---|---|
| 1 | uniform 里放 `mat3x2<f32>` | WGSL uniform 地址空间要求矩阵列跨度 16 字节对齐 | 拆三列 `vec2` 存储、着色器内拼回矩阵 |
| 2 | 图集把 `VERTEX` 用法缓冲绑成 uniform | wgpu 校验：绑定组要求 `UNIFORM` 位；且无 `COPY_DST` 不可写 | 缓冲改 `UNIFORM\|COPY_DST`，成为名实相符的视图 uniform 源 |
| 3 | 管线创建崩溃 | DLL 内 panic：`invalid front face for primitive state` | `WGPU_FRONT_FACE_CCW` = **1**（原猜 3；对照解出的 webgpu.h 核实，其余常量全部复核无误） |
| 4 | 装配期 3 条未捕获错误 | `max_anisotropy=0`（须 ≥1）→ 采样器无效 → 绑定组级联失败 | 采样器 `max_anisotropy = 1` |
| 5 | 管线布局校验失败 | 顶点着色器查 `textureDimensions`，但图集布局只给片段阶段纹理可见性 | 纹理查询全部移入片段阶段；四边形边长改 WGSL 常量（构建期 `debug_assert` 钉住） |
| 6 | 提交崩溃 | `Depth slice was provided but the color attachment's view is not 3D` | 2D 视图的颜色附件 `depth_slice` 用 `WGPU_DEPTH_SLICE_UNDEFINED`（u32::MAX） |
| 7 | 精灵 64x64、断言失败 | `CELL_PX = 256/4 = 64`，与自身"16px 格、同原型精灵"注释矛盾 | `ATLAS_PX` = **64**（4x4 格 x 16px，所有既有文档声明成立） |
| 8 | 首帧出图红蓝互换 | 红精灵读成蓝 `(0,0,255)`、藏青背景读成暗红 `(25,13,13)` —— R/B 系统性互换 | **裁决：RGBA8Unorm 读回就是 RGBA 字节，`FrameImage` 的 BGRA 假设是错的**；字段更名 `rgba`、`pixel()` 去交换、`write_png` 透传 |
| 9 | PNG 落盘后进程 AV 崩溃 | `CommandConsumer` 字段序使 `ctx` 先析构 → `lib` 字段 `FreeLibrary` 卸载 DLL → 后续子件 `Drop` 经函数指针调用已卸载内存 | 字段声明序倒置（管线 → 图集 → 目标 → 上下文），结构体注释钉住该纪律 |
| 10 | 第二帧 `drawn=0`、锚点全丢 | 契约 I4：`Create`/`Destroy` **一次性**落缓冲，属性流才每帧全量；消费器却每帧重建条目表 → 第二帧属性命令全部命中"未知句柄"被 I1 静默忽略 | 条目表改为**跨帧持有**（`Create` 建、`Destroy` 删）；由 `criterion_backend_sprite_frame_and_png_roundtrip` 的稳态第二帧断言钉住 |
| 11 | 逐用例开/关 GPU 上下文的测试序列稳定崩溃（0xC000041D），并行时序下偶发（139/127） | `NativeLib::drop` 的 `FreeLibrary` 把 wgpu-native 完全卸载，下一用例重载 —— Vulkan 加载器反复初始化/卸载在其自建线程/TLS 上崩溃。这是**真实 API 风险**：任何"重建后端"的长跑进程都会踩中，测试只是把它放大 | 动态库按**进程生命周期**持有（`Drop` 不卸载，引擎侧惯例，见 §5 裁决 9）；GPU 用例另加进程内串行锁消除并发多实例的残余抖动，8 轮复跑全部稳定 |

---

## 4. 实测基线（2026-09-30，本机 Windows 10 / Intel Iris Xe / Vulkan）

| 检查项 | 结果 |
|---|---|
| 依赖方向守卫 | **G1~G10 = 10/10 PASS**（EXIT 0；G8/G9/G10 为本轮新增） |
| `nes-scene` | test **75** 全绿（lib 40 + m1 19 + m2 4 + m3 12）｜ clippy 零警告 |
| `nes-asset` | test **34** 全绿（lib 18 + m3 16）｜ clippy 零警告 |
| `nes-render-api` | test **40** 全绿（lib 8 + criterion_contract 32）｜ clippy 零警告 |
| `nes-render-extract` | test **42** 全绿（lib 6 + criterion_extract 24 + criterion_gaps 12）｜ clippy 零警告 |
| `nes-render-wgpu` | test **15** 全绿（lib 8：png 4 + renderer 4；集成 criterion_backend 7）｜ clippy `--all-targets -D warnings` 零警告｜ GPU 用例串行化 + 动态库进程持有后连续 8 轮复跑无抖动 |
| `cargo run --example s41_visual_closure` | **PASS**（EXIT 0；适配器 Intel Iris Xe / Vulkan；driver_errors=0） |
| PNG 外部交叉验证 | System.Drawing 回读 `px(9,9)/(10,10)/(13,13)/(26,26)` 与断言逐字节一致 |

> 记账更正（归档说明 §3.10）：历史封口文档基线 `nes-render-api 39` / `nes-render-extract 41`
> 为漏记新增用例的旧数，以本表 **40 / 42** 为准（差异来源：`criterion_contract` 32 条、
> `criterion_gaps` 12 条，均实测清点）。

---

## 5. 架构裁决记录（S4.1 新增，后续里程碑沿用）

1. **读回通道序 = RGBA**（§3 #8）：`FrameImage` 存储即 `wgpuBufferGetMappedRange`
   原始字节，语义访问唯一入口是 `pixel()`。写 PNG 直接透传，不再有 BGRA→RGBA 换算层。
2. **消费器条目表跨帧持有**（§3 #10）：与契约 I4 的"一次性生命周期 + 每帧全量属性"
   复合流对齐；"每帧提取"约束场景侧，不是后端失忆。
3. **GPU 部件析构序**：`CommandConsumer` 字段声明序 = 析构序（管线 → 图集 → 目标 → 上下文），
   与创建序相反、与资源所属关系一致。动态库改为进程生命周期持有后（裁决 9），顺序不再是
   "函数指针失效"意义上的硬约束，但作为创建逆序的卫生纪律保留并注释钉住。
4. **相机单位视图口径**：示例与测试用"相机中心 = 视口半尺寸"（`transform = translation(vp/2)`）
   使 `view_matrix()` 恰为单位 —— 既真实走通契约 I9 的视图矩阵路径（`Camera2DState::view_matrix`
   是唯一权威，后端不另行推导），又让世界坐标 == 屏幕像素坐标，与原型日志同口径。
   相机缺位 / 禁用 / 视口非法时，消费器退回单位视图 + 目标尺寸（帧本地，无"上一有效相机"可退）。
5. **图集采样格映射**：`key.slot % (ATLAS_CELLS²)` 确定性选格；格 0 真实图案、
   其余品红哨兵 —— UV 错采样在像素层立即显形（测试全画面扫描钉住）。
6. **`SetText` / `SetRect` 的 S4.1 边界**：确认句柄已知并记账（`stats.updates`），
   不产生像素 —— 排版归属 CPU 侧，字形光栅化属后续里程碑。
7. **清屏色公开为 `renderer::CLEAR_COLOR`**（f64 通道）并与测试侧字节锚点
   `CLEAR_RGBA` 用专门测试互锁，防两侧漂移。
8. **图集尺寸 = 64px**（4x4 格 x 16px）：`64*4=256` 字节/行恰好满足纹理上传的
   256 字节对齐约束，且与原型 16x16 精灵同格。
9. **动态库按进程生命周期持有**（§3 #11）：`NativeLib` 的 `Drop` 刻意不
   `FreeLibrary` —— wgpu-native/Vulkan 加载器自建线程与 TLS，完全卸载后重载
   会以 0xC000041D 崩溃。句柄与线程由操作系统在进程退出时回收；"重复装配后端"
   由此从危险操作变回普通操作。测试侧另以进程内串行锁约束 GPU 用例
   （并发多实例在本机有残余抖动，串行后 8 轮全稳定）。

---

## 6. 复现入口

```powershell
# 依赖方向守卫（期望 10/10 PASS，EXIT 0）
cd "F:\All NGVGE\NES 2.0\Current products"
python check_dependency_direction.py

# 五个 crate（期望分别 75 / 34 / 40 / 42 / 15 全绿，clippy 零警告）
cd .\nes-scene;          cargo test; cargo clippy --all-targets -- -D warnings
cd ..\nes-asset;         cargo test; cargo clippy --all-targets -- -D warnings
cd ..\nes-render-api;    cargo test; cargo clippy --all-targets -- -D warnings
cd ..\nes-render-extract;cargo test; cargo clippy --all-targets -- -D warnings
cd ..\nes-render-wgpu;   cargo test; cargo clippy --all-targets -- -D warnings

# 最小可视闭环（期望 PASS；产物 output\s41_visual_closure.png）
cargo run --example s41_visual_closure
```

> GPU 用例在无 wgpu-native 资产的机器上会**跳过并打印说明**（`NoLibraryCandidates`），
> 但只要库存在，装配或渲染失败即判失败 —— "没有库"与"有库跑不通"不互相伪装。
> 动态库落点：`nes-render-wgpu\..\wgpu-win\lib\wgpu_native.dll`（候选 2），
> 或用环境变量 `NES_RENDER_WGPU_LIB` 显式指定。

---

## 7. 遗留与后续（S4.1 之后）

> **v1.1 注记**：下表前两行已由 S4.2 关闭（2026-09-30，见
> `NES2.0_M4渲染接入_S4批量与控件_v1.md`：扩容路径测试 + Control HUD 光栅化 +
> flip/zoom 像素级实证；测试基线随之升至 19 项）。
> **v1.2 注记**：真实纹理注册表已由 S4.3 关闭（同日，见
> `NES2.0_M4渲染接入_S4纹理注册表_v1.md`：宿主侧 `register_texture` API +
> 两路混画；测试基线升至 23 项）。
> **v1.3 注记**：Label 文本光栅化已由 S4.4 关闭（同日，见
> `NES2.0_M4渲染接入_S4文本光栅化_v1.md`：等宽字形表最小口径 + 混排演示；
> 测试基线升至 26 项）。四类渲染命令（精灵/控件/文本 + 相机）至此全部产生像素。
> **v1.4 注记**：S4.5 文本契约回归补齐 T-Text-01..14（含 Text+Control+Sprite
> 同管线架构回归项），并修复自定义 `font` 键未实现的缺口；测试基线升至 40 项
>（见 `NES2.0_M4渲染接入_S4文本契约回归_v1.md`）。
> **v1.5 注记**：S4.6 渲染契约回归补齐 T-Sprite / T-Control / T-Camera 三矩阵
>（24 项，含 HUD 不随相机的逆视图折算口径、DrawKey 三级全序、相机回退分支），
> 并抓出 `ControlState` offsets 四边偏移语义误用；测试基线升至 64 项
>（见 `NES2.0_M4渲染接入_S4渲染契约回归_v1.md`）。
> **v1.6 注记**：S4.7 契约验证收官 —— T-Registry（8）、T-Stats（6，含 36 命令
> 全量对账）、视觉基线（PNG 哈希锚定 + bless）；`register_texture` 拒绝 NIL 键；
> 测试基线升至 79 项（见 `NES2.0_M4渲染接入_S4契约验证收尾_v1.md`）。
> **v1.7 注记（M4 全线收官）**：新增 `nes-runtime` 引擎组装层（M5 文档见
> `NES2.0_M5引擎组装层_v1.md`）：场景树 + 磁盘资产驱动整条管线，热重载直达
> 像素；守卫扩至 G1~G11；全仓六 crate 测试 75/34/40/42/79/4 全绿。
> **v1.8 注记（S6.1 窗口/Surface）**：帧循环从离屏搬到真实 Win32 窗口
> （手写 FFI，零第三方）：`consume_to_surface` 与 `consume` 共用同一条
> 命令处理/绘制路径（S6 文档见 `NES2.0_S6窗口Surface_v1.md`）；新增
> T-Surf-01..04，wgpu crate 基线升至 83 项。
> **v1.9 注记（S6.2 tick 接线）**：帧内前半程升级为 `SceneTree::tick`
>（enter/ready/process 生命周期 + Cmd 命令缓冲），宿主行为经
> `SceneObserver` 挂入，回调命令本帧直达像素（文档见
> `NES2.0_S6生命周期Tick接线_v1.md`）；新增 T-Tick-01..04，runtime
> 基线升至 8 项。
> **v1.10 注记（S6.3 场景序列化闭环）**：`load_scene` / `save_scene` 接入
> 帧循环，磁盘 RON 场景成为事实来源，`Resource(n)` 槽位身份经磁盘往返
> 三处同源（文档见 `NES2.0_S6场景序列化闭环_v1.md`）；新增
> T-Scene-01..04，runtime 基线升至 12 项。
> **v1.11 注记（S6.4 暂停与时间缩放）**：草案 §9 落地 —— `ProcessMode`
> 五模式 + 继承解析 + `paused`/`time_scale`，派发口径冻结在 tick
>（文档见 `NES2.0_S6暂停与时间缩放_v1.md`）；新增 T-Pause 01..08 与
> T-Pause-R 1..2，scene 基线升至 83、runtime 升至 14。
> **v1.12 注记（S6.5 序列化口径）**：`process_mode` 裁决为 `NodeDoc`
> 一等字段（与 `local` 对称；缺省不写出、未知值语义报错），磁盘场景携带
> 的调度语义经往返仍生效（文档见 `NES2.0_S6序列化口径ProcessMode_v1.md`）；
> 新增 T-PM 01..04 与 T-Pause-R3，scene 基线升至 87、runtime 升至 15。
> **v1.13 注记（S6.6 子场景嵌套）**：`sub_scene` 引用递归展开 + 回写
> 边界（子树归子文件，存盘只留引用），槽位按 (path, kind) 去重合并
>（文档见 `NES2.0_S6子场景嵌套_v1.md`）；新增 T-Sub 01..04 与
> T-Scene-05，scene 基线升至 91、runtime 升至 16。
> **v1.14 注记（S6.7 子场景热重载）**：改子场景文件 -> `poll_scene_reload`
> 从来源整树重载重展开（子场景文件本就是注册表里的 Scene 资产，缺的只是
> 来源跟踪与重载触发；文档见 `NES2.0_S6子场景热重载_v1.md`）；新增
> T-SubR 01..03，runtime 基线升至 19。
> **v1.15 注记（S6.8 实例级属性覆盖）**：包装节点可携带 `InstanceOverride`
> 记录（路径 + local/process_mode/props），实例化时应用到展开子树；记录
> 属父场景文件 —— 热重载后覆盖仍赢、未覆盖字段跟新（文档见
> `NES2.0_S6实例级属性覆盖_v1.md`）；新增 T-Ovr 01..04 与 T-SubR-04，
> scene 基线升至 95、runtime 升至 20。
> **v1.16 注记（S6.9 diff 式回写）**：`sync_overrides` 把运行时对实例
> 内部节点的编辑烘焙成覆盖记录（参照 = 当前磁盘子场景的独立实例化；
> 资源按所指路径比较防误报；保存保持纯读 —— 文档见
> `NES2.0_S6Diff式回写_v1.md`）；新增 T-Ovr-05..06 与 T-Scene-06，
> scene 基线升至 97、runtime 升至 21。
> **v1.17 注记（S6.10 结构性覆盖）**：`InstanceOverride` 扩展
> `add`/`remove`（实例内增删节点成为父文件记录），diff 随之结构化
>（参照独有 -> remove、当前独有 -> add 子树全量导出；文档见
> `NES2.0_S6结构性覆盖_v1.md`）；新增 T-Ovr-07..09 与 T-Scene-07，
> scene 基线升至 100、runtime 升至 22。项目自此入 Git 仓库。
> **v1.18 注记（S6.11 重命名覆盖）**：`rename` 记录落地 —— 改名**保留
> 跟踪**（节点仍属子场景，字段覆盖与热更新继续作用），diff 以同种类
> 贪心配对识别 rename（文档见 `NES2.0_S6重命名覆盖_v1.md`）；新增
> T-Ovr-10..12 与 T-Scene-08，scene 基线升至 103、runtime 升至 23。
> **v1.19 注记（S6.12 兄弟重排）**：`move_to` 记录落地 —— 纯重排按
> 当前序回放（可证明重建），兄弟序即绘制序的像素语义验证；混合
> 结构+重排不表达（口径，文档见 `NES2.0_S6兄弟重排_v1.md`）；新增
> T-Ovr-13..14 与 T-Scene-09，scene 基线升至 105、runtime 升至 24。
> **v1.20 注记（S6.13 相对位置语义）**：`after:`/`before:` 记录落地 ——
> 锚点在结构落地后解析（新增节点可作锚点），混合"新增+重排"缺口解除；
> diff 生成升级为相对链；顺带修复 S6.12 连环下标缺陷（影子序，文档见
> `NES2.0_S6相对位置语义_v1.md`）；新增 T-Ovr-15 与 T-Scene-10（重写
> T-Ovr-14），scene 基线升至 106、runtime 升至 25。
> **v1.21 注记（S6.14 信号总线）**：草案 §12 落地 —— `Signal` + 帧末泵
>（process 后、冲洗前：信号触发的变更**同帧入画**）+ 级联工作队列
>（迭代非递归，上限 1024 丢弃计数）；交付走观察者，订阅册留给脚本 VM
>（文档见 `NES2.0_S6信号总线_v1.md`）；新增 T-Sig-01..05 与 T-Tick-05，
> scene 基线升至 111、runtime 升至 26。
> **v1.22 注记（S6.15 信号桥）**：TreeEvent -> `tree/*` 桥信号 —— 结构
> 事件自动入信号管道（双通道不互斥、事件原文随 `Signal.event`、泵序最前；
> 文档见 `NES2.0_S6信号桥_v1.md`）；新增 T-Sig-06..08（含跨帧回流闭环），
> scene 基线升至 114。
> **v1.23 注记（S6.16 订阅过滤）**：`SceneObserver::signal_filter` 声明式
> 订阅（All/精确名/前缀）—— 未命中不进处理器、不耗上限、不级联，
> `signals_filtered` 记账；NoObserver 缺省 NONE（文档见
> `NES2.0_S6订阅过滤_v1.md`）；新增 T-Sig-09..12，scene 基线升至 118。
> **v1.24 注记（S6.17 订阅册）**：`connect_signal`/`disconnect_signal`
> 路由层 ——（名字+可选源 -> 目标节点），命中连接给观察者带 dst 上下文
> 的额外交付（注册序、同守上限）；节点销毁自动清理（tick 阶段 1 修剪）；
> 方法级分发仍归脚本 VM（文档见 `NES2.0_S6订阅册_v1.md`）；新增
> T-Sig-13..16，scene 基线升至 122。
> **v1.25 注记（S6.18 方法级分发）**：节点处理器表
>（`set_signal_handler` 闭包）+ `connect_signal_to`（connect 四参补全）
> —— 引擎直接调闭包不经观察者；未注册静默跳过、同名替换、销毁清理；
> 脚本 VM 将在此注册解释器闭包（文档见 `NES2.0_S6方法级分发_v1.md`）；
> 新增 T-Sig-17..19，scene 基线升至 125。
> **v1.26 注记（S6.19 脚本 VM）**：手写栈式字节码解释器（18 指令 +
> ScriptVm 注册表/装载器），`Script` 节点 registry_key 挂载点接通；
> 双入口全走 substrate（信号=处理器表闭包跨节点 / process=观察者仅自身）；
> 三重停机保护（`__halt`/步数上限/缺属性回落）；顺带修正 S6.16 泵过滤
> 只管广播（连接不受观察者订阅影响，文档见 `NES2.0_S6脚本VM_v1.md`）；
> 新增 T-VM-01..05 与 T-Script-R1，scene 基线升至 130、runtime 至 27。
> **v1.27 注记（S6.20 文本脚本语法）**：Rust-lite 文法 + 手写编译器到
> S6.19 字节码（on/every 双入口、局部/属性/位置读写、if、emit、四则
> 比较、Vec2/负号字面量）；编译器按构造保证栈序；Mul 保整型、Eq 数值
> 按值比较（探针实证两处修正，文档见 `NES2.0_S6文本脚本语法_v1.md`）；
> 新增 T-Cmp-01..05，scene 基线升至 135。
> **v1.28 注记（S6.21 控制流与逻辑）**：if-else（含 else if 链）、while
>（死循环步数兜底）、`&&`/`||`/`!`（按值 eager，And/Or/Not 指令，集 21）、
> 比较五族补全（四条组合编译零新指令）；调试抓到交换族 take 主缓冲的
> 重排 bug + 实证脚本内写读批次语义（文档见
> `NES2.0_S6控制流与逻辑_v1.md`）；新增 T-Cmp-06..09，scene 升至 140。
> **v1.29 注记（S6.22 循环控制）**：`break`/`continue` —— 编译期循环
> 上下文栈（continue 即时跳顶、break 占位收尾回填，嵌套绑最内层，
> 循环外编译错；`while true`+break 惯用法不依赖步数兜底；文档见
> `NES2.0_S6循环控制_v1.md`）；新增 T-Cmp-10..12，scene 升至 142。
> **v1.30 注记（S6.23 标签跳转）**：`name: while` + `break name`/
> `continue name` —— 标签由内向外匹配同名层（无标签层不参与），跨层
> break 占位登记目标层、外层收尾统一回填；标签只能用于 while（文档见
> `NES2.0_S6标签跳转_v1.md`）；新增 T-Cmp-13..15，scene 升至 145。
> **v1.31 注记（S6.24 区间迭代）**：`for i in a..b` —— 纯糖脱糖 while
>（界活值内联、循环变量是普通局部），**循环旋转**布局使 continue 落在
> 增量上（不吃增量的经典坑反例证明）；标签放宽到 for；词法补 `..` 与
> 数字停扫（文档见 `NES2.0_S6区间迭代_v1.md`）；新增 T-Cmp-16..18，
> scene 升至 148。
> **v1.32 注记（S6.25 步进与闭区间）**：`for i in a..b step e`（正升
> 负降、零恒假零次；字面量编译期定向、表达式编译运行时方向条件）+
> `..=` 闭区间（三字符符号）；调试实证 Rust `f64::signum(+0.0)==1.0`
> 陷阱（零步进误定向升序 -> 769 轮步数耗尽）改显式三分（文档见
> `NES2.0_S6步进与闭区间_v1.md`）；新增 T-Cmp-19..21，scene 升至 151。
> **v1.33 注记（S6.26 取模/位运算/拼接）**：`%`（符号跟随被除数）、
> 位五族 `& | ^ << >>`（I64 严格、移位 wrapping 不崩帧）、`+` 增
> Str+Str 拼接（严格不隐转）；文法四级位阶梯（C/Rust 序），指令集
> 21->27（文档见 `NES2.0_S6取模位运算拼接_v1.md`）；新增 T-Cmp-22..24
>（T-VM-03 随语义演进更新），scene 升至 154。
> **v1.34 注记（S6.27 进制字面量）**：`0x`/`0b`/`0o` 前缀 —— 整型
> 严格（无进制浮点）、`_` 分隔、大小写不敏感；非法进制数字/无数字/
> 溢出在**词法层**当场指名（0b12 的 `2` 不再变成下游误导性语法错；
> 文档见 `NES2.0_S6进制字面量_v1.md`）；新增 T-Cmp-25，scene 升至 155。
> **v1.35 注记（S6.28 除法与复合赋值）**：`/` 补缺（S6.20 写"四则"
> 但实现缺 —— 整型截断除**除零停机**、浮点 IEEE）+ 十种复合赋值
> `OP=`（脱糖与手写同构；节点成员**读侧双压**——写侧压节点会 Set 弹序
> 颠倒停机，调试实证）；指令集 28（文档见
> `NES2.0_S6除法与复合赋值_v1.md`）；新增 T-Cmp-26/27，scene 升至 157。
> **v1.36 注记（S6.29 自增自减）**：`++`/`--`（含 `node.member++`）
> —— **只能作独立语句**、前后缀等价（语句位无值产生，C 求值序坑结构
> 性不存在）；表达式位编译错指名；`pos++` 走 Add 家法停机；实证成员 ++
> 的批次语义（同脚本两连读陈值末写覆盖）；零新指令（文档见
> `NES2.0_S6自增自减_v1.md`）；新增 T-Cmp-29，scene 升至 158。
> **v1.37 注记（S6.30 字符串操作）**：比较（Lt 增 Str 臂 —— 码点字典
> 序，组合编译使六族全通）、`len` 内建（Unicode 标量数，中文 2 非字节
> 6）、`s[i]` 后缀索引（单字符 Str；越界/负停机附下标与长度）；内建
> 调用语法 `len(` 括号消歧不占保留字；指令集 30（文档见
> `NES2.0_S6字符串操作_v1.md`）；新增 T-Cmp-30，scene 升至 159。
> **v1.38 注记（S6.31 脚本进场景文件）**：Script 节点 `source` 属性
> —— 非空即 attach 时编译装载，场景文件成为"结构+资源+行为"自包含
> 单元（宿主零注册）；与 registry_key 互斥；多行源码 RON 转义往返逐字
> 不丢（存量文件不变）；编译错带脚本文本行号入 attach 缺口（文档见
> `NES2.0_S6脚本进场景文件_v1.md`）；新增 T-Cmp-31 与 T-Script-R2
>（磁盘场景->attach_all->信号->像素），scene 升至 160、runtime 至 28。
> **v1.39 注记（S6.32 脚本热重载）**：`vm.poll_reloads` —— source 属性
> 与编译时戳比对，变化即重编译重挂载；**编译失败保留旧行为**（戳不更新、
> 修好即生效）；重挂载换程序不复位外还修**叠加连接**潜伏缺口（先断旧
> 再接新）；attach_all 前置死节点清理；属性流 + 文件流（S6.7 搭配）双
> 到像素（文档见 `NES2.0_S6脚本热重载_v1.md`）；新增 T-VM-06..08 与
> T-Script-R3，scene 升至 163、runtime 至 29。
> **v1.40 注记（S6.33 外置脚本资产）**：Script 节点第三路挂载 `script`
> 属性（Resource 槽位 -> 资源表 kind:Script 的 .nes 文件，与纹理/子场景
> 同构）；三路恰一非空；`attach_all_with_sources`（注入读取器，VM 不碰
> 文件系统）+ `poll_reloads_with_sources`（改 .nes 文件 -> 重编译 ->
> 新像素，**不整树重载**）；同文件多节点共享编译产物（`__file__:{path}`）；
> `.nes` 入提示映射（文档见 `NES2.0_S6外置脚本资产_v1.md`）；新增
> T-VM-09..10 与 T-Script-R4，scene 升至 165、runtime 至 30。
> **v1.41 注记（S6.34 编辑器脚本面板）**：窗口 `wnd_proc` 增 WM_CHAR
> 输入泵（进程级字符队列 + `drain_chars`/`inject_char` 双口，单窗口
> 口径）；示例 `script_panel` 与 T-Panel-R1 组合已有件成编辑闭环 ——
> 键入 → 宿主解释（回车提交/退格/可打印追加，编辑语义全在宿主）→
> `set_prop source` + `poll_reloads` → **下一帧精灵像素换位**；坏脚本
> last-good、状态行如实回显（`src>`/`in>`/`st>` 三行 Label + Control
> 边框，引擎用自己的管线显示自己的编辑器）。实证两缺口：`open_windowed`
> 资产根缺省当前目录（示例改 `open_windowed_with_root` + 绑定断言）、
> 默认字体须宿主登记（无字体 Label 回落图集格）。场景/提取/渲染语义
> 零改动（文档见 `NES2.0_S6编辑器脚本面板_v1.md`）；新增 T-In-01 与
> T-Panel-R1，wgpu 升至 84、runtime 至 31（全仓合计 396）。
> **v1.42 注记（S7 引擎收束）**：不横向加功能，用测试网反向审整个
> Runtime：五项发现全修 —— ①死节点清理五表统一（S6.33 的 file_stamp
> 曾漏进清理表、attach_all_with_sources 曾整入口无清理 → `prune_dead`
> 统一 + `tracked_nodes()` 观测口径）；②表面获取失败命名化分类
>（status=3 实为 wgpu **Timeout** 瞬态 —— 曾被误诊 ConfigMismatch 并
> 实测杀死示例进程；Timeout 归既有瞬态类，OUTDATED/LOST/OOM 带名
> ConfigMismatch）；③两示例帧循环瞬态容忍（连续上限 120 帧，跳帧重试）；
> ④字符队列容量上限 4096（丢新保旧）；⑤BMP 尺寸算术 checked + 容量
> usize 域（u32 域乘法在 debug 下对合法大图 panic）。清洁账单：库代码
> panic 家族零裸用、**帧路径零 HashMap 迭代**（连接注册序/BTreeMap/
> 前序）、NodeId 带代号、unsafe 逐块 SAFETY 注。新增 T-Stab-01..04，
> scene 升至 166、wgpu 至 87（全仓合计 400，文档见
> `NES2.0_S7引擎收束_stabilization_v1.md`）。
> **v1.43 注记（S7.1 运行时语义冻结）**：四题裁决 + 契约测试固化 ——
> ①黄金帧序（结构落地+桥 → enter → ready → process[五模式表] → 信号
> 泵[宿主预发++帧内发射，桥最前] → 变换冲洗；每回调微批次即时落地）；
> ②暂停矩阵补全：路由处理器与 process 同表门控（Disabled 永不调用、
> Pausable 暂停中跳过、Always/WhenPaused 照常；广播不受暂停影响 ——
> 本轮唯一行为变更，新增 `handlers_skipped` 观测）；③`Observers` 组合
> （注册序稳定 Vec、同节点回调内后注册见先注册落地前状态、订阅过滤
> 并集、组合对引擎是一个观察者）；④Cmd 可见性微批次屏障（级联读新值/
> 自读旧值/末写胜/结构[Spawn]下一帧帧首落地/信号驱动 SetLocal 当帧
> 冲洗入画；connect/disconnect 不是 Cmd —— 回调不可改接线）。
> 新增 T-RS-01..04（含 02b/03b）与 T-RS-R1，scene 升至 172、runtime
> 至 32（全仓合计 407，文档见 `NES2.0_S7.1运行时语义冻结_v1.md`）。
> **v1.44 注记（S7.2 输入系统）**：四层管线落地 —— 平台（Win32 消息
> → 中性 `InputEvent` 队列，容量 1024，VK→`Key` 映射）/ 契约
>（render-api `input` 模块：`InputCollector` 事件流 → 帧快照，**闩锁
> 边缘**）/ 运行时（`collect_input` + `emit_input_signals` 标准
> `input/*` 信号 + `mount_key_probe`）/ 消费（事件式 `on
> "input/key_down"`（arg=键名）与轮询式 `key("名")` 探针 —— 未接
> 探针停机）。Keyboard/Mouse/TextInput/Window 四路分开；**WM_CHAR
> 从引擎 API 退役**（`drain_chars`/`inject_char` 删除，面板迁移到
> 快照 text）。实证三修：泵只翻译按下族（KEYUP 异常 lparam 的幻影
> 字符）、闩锁边缘（同帧按下+抬起不再不可见）、首帧鼠标增量只建基准。
> 新增 T-In-C01..03 / T-In-VM-01..02 / T-In-01..02（重写）/ T-In-R1..02，
> scene 174、api 43、wgpu 88、runtime 34（全仓合计 415，文档见
> `NES2.0_S7.2输入系统_v1.md`）。
> **v1.45 注记（S7.3 Headless 确定性运行时）**：`open_headless`
>（GPU 端 `Option` 化 —— 装配缺席而非 `if headless` 语义分支，tick/
> 输入/装载/指纹与窗口模式逐字节同路径）+ `InputTrace`/`parse_trace`
>（纯文本帧事件轨迹）+ `scene_fingerprint`（**语义状态白名单**哈希：
> 前序结构/变换 f32 位形/属性/脚本局部；绝不取句柄/指针/HashMap
> 布局）+ 逐帧指纹 → `trace_hash`（差分定位到帧）+ `nes` CLI
>（`headless::run` 薄壳）。T-HR-01..08 **全部无 GPU 依赖**；CLI 实测
> 跨进程确定。新增 T-In-C04 与 T-HR-01..08，api 升至 44、runtime 至
> 42（全仓合计 424，文档见 `NES2.0_S7.3Headless确定性运行时_v1.md`）。
> **v1.46 注记（S7.4 首个真实项目·压力图）**：真实游戏 Dodge（单文件
> first_game.ron + 全脚本行为；窗口宿主 + `nes --headless` 确定性双跑）
> 压 S7.0~S7.3 冻结语义 —— 玩法闭环（追踪/碰撞/受击/胜负）由 T-GP-01
> 断言。命中两真阻塞按"只修真阻塞"最小解锁：`xy(e1,e2)` 构造
>（Op::Pack）与 `node.pos.x/.y` 分量读（Op::GetX/GetY，T-Cmp-32）。
> 结构性发现六项记入压力图（逐帧 AI 只能走宿主 tick 信号的合取效应 /
> 局部无初始化 / 属性表封闭无通用状态容器 / 数字不能转字符串 /
> 无集合迭代 / 无 sqrt）—— 不修，攒作 S8 输入。scene 升至 175、
> runtime 至 43（全仓合计 426，文档见
> `NES2.0_S7.4首个真实项目_压力图_v1.md`）。
> **v1.47 注记（S8.0 脚本生命周期状态）**：`init` 块（可选、入口前、
> 每脚本一个；**挂载后首次派发前**执行一次；重挂载 = 局部复位 +
> 重跑；哨兵局部 `__initialized` 可观测且进语义指纹）+ `num_to_str()`
> 内建（I64/F32→十进制 Str，最短往返口径；Script→Text 闭环 —— Dodge
> HUD 显示 `HP: n/3`，`s == 0` 手同步全部删除）+ **Dodge 兼容压力
> 基线**（examples/regression/dodge/ 三件套入库，T-ABI-01 每次改
> VM/Scene/Signal 自动比对 600 帧指纹 —— 游戏级 ABI，压合取语义）。
> S7.4 压力图对账：#2/#4 已解除。新增 T-LC-01..05 与 T-ABI-01，
> scene 升至 180、runtime 至 44（全仓合计 432，文档见
> `NES2.0_S8.0脚本生命周期状态_v1.md`）。

| 事项 | 状态 |
|---|---|
| Label / Control 光栅化（字形图集、断行、锚点矩形） | Control ✅ HUD 口径（S4.2）；Label ✅ 等宽字形表最小口径（S4.4，对齐/换行/字号待 Q-S3-2 裁决） |
| 真实纹理注册表（当前为内建 16 格图集，`key.slot % 16` 选格） | ✅ 宿主侧 API 落地（S4.3）；场景侧从 `nes-asset` 接真实像素属提取层后续接线 |
| 实例缓冲倍增重建路径（`ensure_capacity` 增长分支）的专项测试 | ✅ 已覆盖（S4.2） |
| headless Linux x86_64 + Mesa llvmpipe 认证移植（cfg 分支 / 库定位候选） | 未启动 |
| M5 Scratch 兼容层 / 编辑器可视化脚本 | 未启动 |
| Q-S3-2 / Q-S3-3 / Q-S3-4（Label 排版参数、相机 `limits` 场景入口、`order` 语义耦合） | 仍待裁决（见归档说明 §6，本轮未动） |
| 窗口 / surface 接入（当前恒离屏） | ✅ S6.1（Win32 FFI 窗口 + wgpu surface，离屏与窗口共用同一绘制路径） |

---

## 8. 与归档说明 v1 的衔接（计划事项完成状态）

| 计划项（归档说明 §4） | 状态 |
|---|---|
| 5.1 补 `src/renderer.rs` | ✅ 完成（本轮） |
| 5.2 修 `error.rs:102` | ✅ 完成（连带补上 `ConfigMismatch` 分支） |
| 5.3 写 `examples/s41_visual_closure.rs` 并实机出图 | ✅ 完成（PASS + 外部交叉验证） |
| 5.4 守卫扩 G8 / G9 / G10 | ✅ 完成（10/10） |
| 5.5 补出口准则测试 | ✅ 完成（`criterion_backend` 7 条，覆盖计划列举的全部六类） |
| 5.6 S4 封口并回写文档 | ✅ 本文档即封口记录；归档说明已附 §11 回写 |
| 5.7 测试基线对齐 | ✅ 见 §4 记账更正（40 / 42 / 15 + 守卫 10/10） |

*（内容由AI生成，仅供参考）*
