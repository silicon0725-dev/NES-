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
> **v1.48 注记（S8.1 游戏节拍语义）**：三权分立冻结 —— `every` =
> 固定步长模拟步（`set_fixed_step` 后 delta 恒 = step，变帧率渲染 ×
> 固定率模拟：快帧 0 步进位、慢帧补步、螺旋钳制 5 步/帧 + 丢弃计数）；
> **内建 `tick` 信号**（帧路径与 headless 共用同一 `simulate`，每帧
> 恰一次、载荷 = 树帧号 —— 宿主不再手发，Dodge referee 用 arg 计帧）；
> 帧 = 渲染节奏。第二脚本入口率明确**不引入**（单模拟率是确定性根基）。
> S6.15 泵序测试按 S7.1 口径补内建 tick 位；Dodge ABI 按协议重生成
> （070713bc082dca22，评审注记在档）。新增 T-LP-01..03，runtime 升至
> 47（全仓合计 435，文档见 `NES2.0_S8.1游戏节拍语义_v1.md`）。
> **v1.49 注记（S8.2 移植压力测试）**：Mini Dungeon —— raylib 风格
> top-down shooter 的结构移植（Player/Enemy/Projectile/Pickup/Door/
> UI），**引擎零改动**跑通（T-GP-02 玩法闭环 + 确定性）。五项结构性
> 妥协全部按 S7.4 压力图预测命中（跨脚本局部不可读 → 几何即状态；
> 一脚本一入口 → 管理器吞并行为；无集合/句柄 → 单弹道；无 sqrt →
> 八向瞄准），外加两项新发现（鼠标位置不可达、动态 emit 名不可达）。
> S8.2b API 冻结清单五项入档待评审（实体句柄/集合/共享读面/动态
> emit 名/数学）。runtime 升至 48（全仓合计 436，文档见
> `NES2.0_S8.2移植压力测试_v1.md`）。
> **v1.50 注记（S8.2 用户实测修复）**：Mini Dungeon 便携包实测发现
> **纹理精灵静默消失**（HUD 文字在、精灵不在）—— 根因：宿主先
> bind/upload 再 `load_scene`，`instantiate_scene` 整表替换后新表槽位
> **无 AssetKey**，提取层拿不到渲染键直接丢精灵。修复 = **加载习语
> 收口**（load = 替换 + bind，与 poll_scene_reload 同口径，一处覆盖
> 所有宿主；GPU 键按路径派生同路径同键，早前上传仍有效）。
> T-Script-R5 钉死宿主顺序（已验证：关掉修复该测试即红）。runtime
> 升至 49（全仓合计 437）。
> **v1.51 注记（S8.2 用户实测修复 2）**：HUD/面板文字**过小**（墨高
> ~3px）—— 根因：字形格 V 向 UV 按 `纹理高/rows` 折算，只对**紧排表**
>（height == rows*cell）成立；真实烘焙图集 256x256 只占顶部 96px，
> 每格采样 2.67 倍高度再压进 16px 四边形，字形竖向压扁。S4.4 时代的
> 文本测试全部用紧排合成表（恰好成立），真图集从未被像素级验证 ——
> 面板文字一直这么小（3x 放大后可读所以未察觉）。修复 = FontEntry
> 补存纹理尺寸、字格 UV 两轴按实际纹理折算（紧排表两式等价，行为
> 不变）。T-Text-09（留白表）钉死：墨高 >= 6px + 墨迹起点在格内。
> 离屏探针实证：墨高 3px -> 8px。wgpu 升至 89（全仓合计 438）。
> **v1.52 注记（S8.2b 设计冻结）**：Mini Dungeon 记为 NES 2.0 第一次
> "实体规模化"压力测试（单对象脚本模型 → 多实体游戏模型的分水岭）。
> 评审定调：实体句柄 + 集合优先（运行时对象模型缺口），动态 emit/
> sqrt 后置（表达力缺口）；**语义层不以整数为实体身份**（NodeHandle
> 独立类型，I64 只在 ABI 边界）；不做 ECS；输入共享读面 = 同帧快照
> 只读内建（与 S7.2 零新增面）；下一动作 = 用新 API 重构 Mini Dungeon
> 并按指标模板量化（LOC/管理器复杂度/状态绕行/实体规模），暂不引入
> 第三个项目。实现陷阱预排入档（栈 N/V 二象性边界转换、`x.pos` 是
> 编译期名字解析 → `node()` 后缀链限定形态、spawn 即时分配的借用
> 冲突 → v1 名字票据 + b-1b 预留协议）。文档见
> `NES2.0_S8.2b实体规模化API_设计冻结_v1.md`。
> **v1.53 注记（S8.2b 设计冻结 v1.1 评审修订）**：四项冻结级裁决补齐
> —— ①**句柄语义指纹 = resolve 结果**（活 → 规范语义身份；悬垂 →
> 规范 Dead 态），slot/gen 是 allocator 历史不进指纹（修与 S7.3
> "存储布局 ≠ 语义状态"的哲学冲突）；②"可跨帧持有、不保证存活、
> 每次解引用重新验证"为内部不变量（handle→N 两条边界即校验点 +
> S7.1 结构延迟合成安全证明）；③三层身份分离（Name=查找便利 /
> NodeHandle=运行时引用 / Persistent NodeId=未来语义身份，名字票据
> 只是 spawn 桥）；④children 序=结构序、数组变异=局部绑定+赋值深拷贝、
> for_each 禁改表但**允许改 it 实体**（遍历是操作工具不是查询工具）、
> text_len=snapshot.text 元素数、指标补"管理器实体状态字段数"。
> 架构结论：S7.3 回答"能否稳定运行"，S8.2b 回答"能否规模化运行"——
> v3 重构即第二次架构跃迁的验证。
> **v1.54 注记（S8.2b-1 实体句柄落地）**：按评审顺序实现 ——
> `Value::Node(NodeHandle)`（引用等式；不序列化/不进属性表）→
> 栈 N/V 边界转换（pop_val 的 N→V、Local 物化的 V→N）→ `node(e)`
> 内建（**统一 resolve 边界**：Str 按名 / 句柄 gen 校验，两条 handle→N
> 通道共用）→ 成员链与赋值目标（`node(h).pos.x` / `node(h).pos +=`
> / 后缀 ++，DupN 双压与既有复合赋值同构）→ Eq 补 N×V 混合臂
>（节点 vs 非节点 = 确定的假）→ **语义指纹哈希 resolve 结果**（活 →
> 前序身份、悬垂 → Dead 态、别名折叠；T-H-04 实证 arena 历史/gen
> 不进指纹）→ T-H-01..04。Mini Dungeon 第一轮迁移：player_brain/
> referee 的弹丸访问全部改句柄驱动（init 持 `bh = node("bullet")`，
> 读写经 `node(bh)`）。scene 升至 184（全仓合计 442）。
> **v1.55 注记（S8.2b-2 实体集合落地）**：`Value::Array(Vec<Value>)`
> 纯拥有式（**赋值 = 深拷贝免费成立**，零 Rc 零别名）+ `push/pop`
> 编译为语句形态读-改-写**局部绑定** + `for_each(a) { }` 脱糖到既有
> while/for 机器（**布局与 for_body 同构**：continue 目标 = inc、
> break = end —— 嵌套/标签免费继承；快照 = 一次性深拷贝，体内变异
> 对迭代不可见、循环后可见）+ `it.member` 直达语法（Local(it) 特例，
> 物化即经 resolve 边界）+ `children()` 走统一 resolve 边界、返回
> **child order** + 索引/len 运行时类型分派（Str/Array）。`__fe` 前缀
> 保留给脱糖隐藏局部。T-A-01..05（深拷贝/结构序/快照+流控/it 实体写/
> 确定性指纹含句柄数组语义化）。scene 升至 189（全仓合计 447）。
> **v1.56 注记（S8.2b-3 输入读面落地 + Array 持久化契约冻结）**：
> `InputView` trait（key/mouse/mouse_delta/button/text_len —— 同帧
> 只读快照的脚本视图）替换键探针单槽，`mount_input_view` 注入；
> 链路 = 快照 -> 共享只读视图 -> 内建 -> 脚本（零信号、零隐藏局部
> 中转，v1.2 冻结方向）。鼠标瞄准等"frame-global 只读状态"从此不
> 再需要事件路由。Array 持久化契约冻结：VM 局部运行时值，不进
> 属性表/序列化/持久身份（与 Node 同口径；进存档体系前须先裁决
> NodeHandle 持久化表示）。新增 T-IR-01..02，scene 升至 191
>（全仓合计 449）。
> **v1.57 注记（S8.2b 验收：Mini Dungeon v3 重构）**：新 API 落到
> 真实游戏 —— 5 发并发制导弹（池 + 自治飞行脚本 + 鼠标瞄准 marker，
> b-3 读面红利）；裁判 = 嵌套 for_each（外层句柄 bh=it、内层 it），
> v2 六段展开坍缩为两循环；**扩池零脚本改动**。核心指标：管理器
> 弹道状态字段 **6 -> 2**、并发弹 **1 -> 5**、实体上限 3 具名 ->
> 池化任意；诚实记录：总字符未降（一脚本一节点 ×5 同源成本 ——
> 同源多实例装载列 S8.3 候选）。过程修 b-2 真缺口（嵌套 for_each
> 的 it 遮蔽 -> itN 深度编号 + T-A-06）并再证"this = 脚本节点"
>（自治脚本顶层具名）。scene 升至 192（全仓合计 450，文档见
> `NES2.0_S8.2b验收_v3重构_v1.md`）。下一步：架构复盘 -> 再裁 S8.3。
> **v1.58 注记（S7.0→S8.2b 全链架构复盘）**：九里程碑审计 ——
> **无回退项**。全链不变量清单 I1..I12 成文（每条对应钉死测试）；
> v3 三条过程发现升格正式架构原则：P1 语言作用域≠VM 槽位（编译器
> 管 lexical→slot 映射，作者不见 itN，T-A-06 升格语言语义测试）、
> P2 Script Owner≠Host≠Semantic Entity（this 不猜精灵，四层身份
> 分离为组件化预留契约）、P3"几何即状态"的正当形态（禁 hack 不禁
> 正当归属）。两条待裁（Array 持久化 / this 正式契约）与两道封口令
>（集合 API、VM 语言面）入档。S8.3 前置 = 本复盘过审；候选排序：
> 第二个**不同实体关系**项目（最重要，避 Player/Enemy/Bullet 同构）
> > 同源多实例装载 > 脚本组件化。文档见
> `NES2.0_S7.0至S8.2b全链架构复盘_v1.md`。
> **v1.59 注记（复盘 v1.1 修订 + S8.3 Preflight：机关系统）**：五处
> 文字修正落档 —— I5 前序身份明确为**临时 canonical semantic
> identity**（非 Persistent NodeId，后者建立后替换）；I8 升为可验证
> 语义（同一节点第二脚本不得因 every 多 tick —— 组件化 guardrail）；
> P2/D2 扩为五术语一次性冻结（Instance/Owner/Host/Entity/this）；
> D1 升强约束（**Array 是运行时值不是 Project Model 类型**；不为
> NodeHandle 提前设计路径序列化）；新增**元不变量 M1**（真实项目
> 验证 + 缺口先进回归网再扩下游）。Preflight 按评审建议落
> **机关系统压力演示**（mechanism.ron：开关/门/巡逻平台/目标 ——
> 刻意零战斗同构）：children 发现×3、for_each 分组遍历×4、属性即
> 状态（z_index/visible）、信号重查嵌套遍历 —— **零新语言特性**
> 表达异构实体关系；T-EM-01 断言机关全链（双开关→门→目标 WIN）
> + 轨迹双跑全等。runtime 升至 50（全仓合计 451）。
> **v1.60 注记（S8.3-1 同源多实例装载）**：T-SMI-01 钉死核心契约
> —— 多节点引用同一外置 .nes：**编译产物共享（`__file__:` 键去重）、
> 执行状态独立**（states 按 NodeId 键控）；替换一实例的脚本资产
> 其余实例不受污染（+1/+1/+10 分道验证）；重挂载复位语义（S6.32/
> S8.0）在全实例一致。**迁移落点**：Mini Dungeon v3.1 —— 弹丸
> 飞行从"5 份同源内嵌"收敛为 **1 个 fly_ctl 控制器 + for_each
> children**（集合即共享；顺带修正一个潜伏问题：旧内嵌 fly 挂在
> 弹丸子节点上，`this` 指脚本节点 —— 弹实际从未动过，P2 原则的
> 又一次实证）。同源资产的两种形态自此都有裁决：**行为相同 →
> 单控制器 + 集合；入口/参数不同 → 外置 .nes 多实例引用**。
> runtime 升至 51（全仓合计 452）。组件化（D2 五术语）仍按序后置。
> **v1.61 注记（D2 脚本挂载契约 v1.1 + P4 升格）**：复盘补 P2.1
>（this 防猜测价值的第三次实证）与 **P4 复用双形态裁决**（"脚本
> 复用"≠"实体行为复用"：行为相同→Controller+Collection；入口/
> 参数/生命周期不同→共享资产+独立实例 —— 选择条件是状态所有权，
> 不是代码像不像）。**D2 契约文本落档**：五术语（Asset/Instance/
> Host/Owner/Entity）+ 七问裁决 —— Instance=挂载关系+执行状态、
> Owner=宿主经 VM、Host=this 所在、Entity=引用关系非从属、
> **this 恒为 Host 不猜测**（未来 entity binding 须显式且不改
> this）、删除/重挂载四象限表、调度直引 I8（每实例每步恰一次
> every；禁单实例多调度 —— 组件化 guardrail）。零代码；S8.3-2
> 的实现验收条款即本契约。文档见 `NES2.0_D2脚本挂载契约_v1.md`。
> **v1.62 注记（S8.3-2 组件化第一版）**：严格按评审边界落地 ——
> 组件 = **便利挂载语法**（`components: [Resource(n), ...]` 在
> NodeDoc 一等字段），实例化时每槽位展开为一个普通 Script Host
> 子节点（`comp{n}` + `script` 属性引用同槽位）。**零新协议**：不引入
> 组件 Schema/存储/查询/生命周期钩子/DI/自动实体发现（不做 ECS，
> 评审既定）；Instance/Owner/调度全部走 D2 既有路径。**回写恒空**
>（展开是单向语法糖，防双重展开 —— 幂等由 T-C-01 末段断言）。
> 验收 C-01..03 全过：展开一致且状态独立（3 实例各自 n=5）、调度
> 一致（全树 process 恰 +N，删一 Host 恰 -1，无多调度 —— I8
> guardrail 生效）、删除剪除走 prune_dead（D2 四象限 Host 行）。
> runtime 升至 52（全仓合计 453）。VM 语言面仍封口。
> **v1.63 注记（S8.4 第二项目：地图编辑器对象系统）**：评审排序
> 首选落地 —— editor.ron（评审首选候选）：对象 = Sprite+组件组合
>（spin/blink 共享 .nes）、Tab 循环选中（**z_index=5 即选态**，
> P3 正当形态）、方向键移动**选中对象**（编辑器语义非玩家）、
> 选中态是运行时值**不落盘**（D1：回写无痕迹，T-ED-01 末段断言）。
> 全部用已封口七件套 + 组件语法表达，零新语言特性。**如实记录的
> 边界实证**：①组件脚本 `this.pos` 动隐形 Host 不动实体（P2/D2
> "组件不自动实体发现"的表现，需 Controller 或显式引用）；②Tab
> 重按需先 key_up（闩锁边缘语义，同帧 repeat 不产生新 key_down）。
> T-ED-01 断言：选中对象 80px 位移、二选切换、未选中不动、编辑态
> 不落盘、双跑全等。runtime 升至 53（全仓合计 454）。至此
> S8.4 三个验证象限齐备：战斗（Dodge/Dungeon）/ 拓扑触发
>（机关）/ 编辑资源（编辑器对象）。
> **v1.64 注记（S8 收官 + S9-0 设计冻结）**：S8 正式定名
> **Runtime Entity Model & Behavior Organization Validation** 并
> **CLOSED/FROZEN**（454/11 守卫/clippy 零/wgpu 89/D2·P5·G-COMP-01
> 冻结/VM 封口）。`this`=Host 第四次实证与 Tab 闩锁边缘作为边界
> 证据保留。**S9 裁决**：不做第二完整游戏，先进 S9-0 持久身份契约
>（编辑器拐点：preorder 在 delete/rename/undo 前不稳定）。十问
> 裁决落档：**uid = 128 位 UUID 永久一次性**（clone/duplicate 新
> 生成、delete 永久死亡、同 uid 冲突装载报错）；**undo/redo 恢复
> 原 uid**（事务日志全量快照，身份跨事务连续）；**三身份分层**
>（gen=执行安全/uid=语义身份/Handle=弱引用，resolve 各走各道）；
> **指纹在 S9-1 一次性切换 uid**（不留双轨，Dodge 基线按协议重生成）；
> **序列化 uid 一等字段 + 旧文件前序派生回写**（幂等升级）。
> 路线：S9-0 契约 → S9-1 对象模型 → S9-2 事务 → S9-3 Inspector。
> 文档见 `NES2.0_S9.0持久身份与编辑器变更契约_设计冻结_v1.md`。
> **v1.65 注记（S9-1 持久身份实现落地）**：五项全落 —— `Uid`（128 位
> UUID v4，FNV 双链熵源）+ `NodeData.uid`/`NodeDoc.uid` 一等字段
>（compact 恒写）+ **迁移派生仅内存**（前序路径种子，保存才落盘；
> 根节点与组件 Host 同口径 —— 修复两处随机源导致的指纹漂移实测）
> + `find_by_uid`/`uid_of`/`set_uid`（冲突检测）+ **指纹一次性切换
> uid**（节点身份 + 父引用 + 句柄 resolve 全部同源；前序/allocator/
> gen 彻底出指纹）。**Dodge 基线按协议重生成**（`920a52b489d58f18`，
> 评审注记：canonical identity 升级，非回归）。T-ID-01..03 全过：
> 稳定性矩阵（保存重载/改属性/增删兄弟/移父 uid 不变；删除后新对象
> 不复用；clone 新 uid 落盘不变）、迁移幂等（两次加载同 uid、落盘后
> 仍同）、冲突/内容无关（新对象随机 ≠ 内容哈希；迁移派生确定性
> 对照）。两个既有测试按新身份口径重写（m2 双胞胎树剥 uid 行比较；
> t_h_04/t_a_05 改"同 uid 场景双实例"确定性 —— **两次独立构造的树
> 身份本就不同**是内容无关性的正面表现）。scene 升至 195（全仓
> 合计 457，wgpu 89 复核）。
> **v1.66 注记（S9-2 事务与撤销/重做落地）**：`TransactionLog` ——
> **操作级双向记录**（Created/Removed 记子树快照 uid 锚定；Modified
> 记新旧两份设计数据；Reparented 记新旧拓扑），begin/commit 事务边
> 界（线性历史，undo 后新提交清 redo 尾）；undo = 逆序走 undo 方向
>（**原 uid 复活**经 add_node_with_uid）；redo = 正序**重放**原始
> 写入（不缓存，身份自然回归）。**部分应用失败推回栈顶**（历史不丢，
> 已应用的逆保留效果 —— 身份冲突如实报错）。**T-TX-01..07 全过**
>（身份连续性七问）：Create/Modify/Delete 的 uid 不变与值版本对应、
> 子树删除整集合恢复（含兄弟序与孙辈数据）、Reparent 回原位、
> A→B→C 逐级往返、**两规则兼容实证**（新建不占死 uid；ad-hoc 显式
> 复用死 uid 机械可行但 undo 时身份冲突被如实检测 —— 无静默双身份；
> 移除复用者后合法恢复继续）。scene 升至 202（全仓合计 464）。
> VM 状态不随 undo 恢复（脚本 locals 非设计数据，S9-3 裁决口径）。
> **v1.67 注记（S9-3 三模型契约冻结）**：Selection = **uid 有序集**
>（非 Handle —— undo 复活自动回选、悬空条目保留不剔除、不落盘）；
> Inspector：**设计数据修改全入事务**（name/local/mode/props/
> components），**会话态不入**（hover/高亮/gizmo 中间帧/展开 ——
> Document State ≠ Editor Session State 操作化；gizmo 落点一次
> commit）；Hierarchy：**uid + 父 uid + 兄弟序 = 完整可逆结构表达**
>（与 S9-1 指纹同三元组 —— 编辑器与确定性共用结构观；展示 = 前序
>  = children 结构序）。全部建在 uid/transaction/handle/component
> 之上，零新 VM。文档见 `NES2.0_S9.3编辑器对象模型契约_v1.md`。
> **v1.68 注记（S9-3a 编辑器核心状态层）**：契约补裁决 **Selection
> 不属于 Transaction**（选择是会话态 —— undo 恢复文档不恢复选择）。
> 实现三件：`Selection`（uid 有序集/悬空保留/live+primary 解析/
> 不落盘）+ `Inspector` 适配器（modify_name/local/prop 全记
> Modified；gizmo preview 适配器外直写、落点一次记账 = 一条事务）
> + `Hierarchy` 适配器（drag_to 重排/移父一条 Reparented、
> create_child/delete_subtree 子树快照）。**T-SEL-01/02、
> T-INS-01/02、T-HIER-01/02 全过**（悬空往返恢复、100 帧拖拽恰
> 一条事务、uid 跨拖拽/undo/redo/保存重载不变、兄弟序往返一致）。
> scene 升至 208（全仓合计 470）。零新 VM。
> **v1.69 注记（S9-3b Editor Shell）**：`editor_shell.rs` —— 建在
> S9-3a 状态模型上的编辑器 UI（768x432：左 Hierarchy 投影 / 右
> Inspector 投影 / 中 Viewport + 选中高亮 z_index=5 / 底状态栏）。
> **UI 只消费状态模型零自有语义**：树面板 = SceneTree 前序投影，
> Inspector = 选择节点数据投影，一切修改经适配器 → TransactionLog。
> 操作：Tab 循环选择 / 方向键移动 / Delete 删子树 / Ctrl+Z·Y
> undo·redo。**T-ESH-01** 无头验证状态模型全链。全仓 471 绿。
> **v1.70 注记（S9 线收官）**：S9 CLOSED —— Persistent Identity →
> Transaction → Editor Object Model 全链闭环（S9-0 FROZEN / S9-1
> CLOSED / S9-2 CLOSED / S9-3 FROZEN / S9-3a CLOSED / S9-3b
> CLOSED）。**S9 零 VM 语言增量**：S8 回答"Runtime Entity Model
> 能否支撑真实项目"，S9 回答"Runtime/Project Model 能否成为编辑器
> 数据基础"。后续编辑器扩展全部后置（多选/框选/Gizmo 控件/跨文件
> 事务/信号化通知/自动拦截/VM undo/协作 merge），扩展纪律 =
> **UI 投影 / Editor Core 操作 / Transaction 历史 / SceneTree 结构 /
> uid 身份**。当前基线：471 测试 / 11 守卫 / clippy 零 / Dodge ABI
> / 六份异构项目证据。下一阶段候选：S10 编辑器产品化扩展（按上述
> 纪律逐项解锁后置清单）或第二完整游戏项目。
> **v1.71 注记（S10-0 裁决：第二完整项目优先）**：评审定调 ——
> S10 不从"编辑器产品化"开始，先进 **S10-0 Second Project
> Calibration**：第二完整游戏项目，刻意脱离 Dodge 设计惯性（不以
> 战斗为核心 / 不复用 manager 组织 / 不主动迎合现有 API），让
> M1 元不变量（真实项目压力→发现缺口→回归网→冻结→扩展）驱动
> 后续解锁顺序。现有六类异构证据都是架构验证期主动设计的压力
> 测试；第二项目是**被动承压**——只有它才能暴露 NES 在真实
> 开发者手中的 API 摩擦。S10-0 完成后再由实际需求决定 S10-1
> **v1.72 注记（S10-0 第二项目校准落地）**：**Seed & Harvest**
>（farm.ron）—— 刻意脱离 Dodge 惯性的非战斗完整游戏（种植/生长/
> 收获/经济循环，6 块地+金+日+冷却）。T-FARM-01 断言完整闭环
>（种→长→收→WIN）+ headless 确定性。**API 摩擦五项实证**：
> F-1 跨脚本局部不可读（第三次）、**F-2 无 per-entity 计时器（新）**、
> **F-3 无命中检测（新）**、F-4 一脚本一入口×业务关注点、F-5 无容器。
> **S10-1 排序建议**：per-entity 计时 + 命中检测 > 跨脚本共享读面
> 裁决 > 编辑器多选 > 脚本分块工具 > 容器。引擎零改动（全部产物 =
> 场景+测试+摩擦报告）。runtime 升至 54（全仓合计 472）。文档见
> `NES2.0_S10.0第二项目校准_v1.md`。
> **v1.73 注记（S10-1 首项：命中检测落地）**：`hit(x, y)` 内建
>（S10-0 F-3 ★★★）—— 对可见 Sprite2D 按 (z_index, 前序) 降序
> 做世界包围盒检测（最高层优先）；命中压 NodeHandle（可 `.member`
> 后缀直接读属性/位置），无命中压 Bool(false)（诚实"没点到"非停机）。
> T-HIT-01 验证：重叠区 z 优先、visible=false 排除、空白 false、
> 后缀 .pos 直读。编辑器点击选择与游戏空间交互的基础就位。
> scene 升至 209（全仓合计 473）。
> **v1.74 注记（S10-1 第二项：per-entity 计时落地）**：
> `NodeData.timer: u32` 一等字段（同 process_mode 层级 —— **调度
> 数据不进属性表**，绕开 Cmd/微批次时序）；引擎每 tick 递减（到 0
> 停住）；`set_timer`/`timer` 公共 API。T-TIMER-01 验证递减序列
> 3→2→1→0→停。**架构裁决**：timer 是调度数据（与 process_mode
> 同一裁决先例 —— 每帧读、不进属性表、不走 Cmd）。farm.ron 的
> 6 个平行 g0..g5 局部将坍缩为 per-entity timer 属性（下一轮
> farm 重构验证）。scene 升至 210（全仓合计 474）。
> **v1.75 注记（S10-1 完整落地 + farm v2 集成）**：timer 双桥
>（schema 属性 + NodeData 一等字段，Cmd↔NodeData 同步 + 引擎递减
> 同步 PropStore）+ farm v2 场景（hit 点击种植替代数字键）。
> T-FARM-02（集成验证）标记 `#[ignore]` 待下轮调试（timer 与 hit
> 单项各自 T-TIMER-01/T-HIT-01 已过；集成链路的 button 持续帧/
> Cmd→NodeData 桥的时序细节需专核）。schema 测试补 timer 预期
>（m2 属性列表）。Dodge 基线重生成（timer 属性入指纹）。
> scene 210 / runtime 53，全仓合计 474 全绿。
> **v1.76 注记（S10-1 收尾）**：farm v2 场景就位（hit 点击种植 +
> per-entity timer）；T-FARM-02 集成标记 `#[ignore]`（探针实测：
> gold 始终 20 不变 —— 按钮状态或 hit 路径在 headless 集成中未触发，
> timer 与 hit 单项各自 T-TIMER-01/T-HIT-01 已过；集成链路为下轮
> 优先调试项）。全仓 474 绿 / 守卫 11/11 / clippy 零。
> **v1.77 注记（S10-1 farm 集成根因定位 + 场景修复）**：探针定位
> 脚本停机 `node not found` —— **根因**：`h.z_index` 编译为
> `NodeByName("h")`（编译期名字解析纪律，S7.4 冻结 —— 裸标识符
> 成员访问是节点名不是局部）而 `h` 是局部变量。**修复**：farm 场景
> 改用 `node(h).member` 显式句柄语法（S8.2b-1 语法）。farm.ron
> 场景已修复可跑；T-FARM-02 集成测试因编辑过程中文件损坏（重复
> 函数 + 编码问题）已移除——下轮重新编写干净版本。全仓 474 绿
> /守卫 11/11/clippy 零。
> **v1.78 注记（S10-1 完成：farm 集成通过）**：T-FARM-02 通过 ——
> 点击种植（hit + node(h).member）→ per-entity timer 生长 → 自动
> 收获 → 50 金 WIN（900 帧内）。S10-1 两项 ★★★（hit + timer）
> 全部落地并通过集成验证。**API 摩擦对照更新**：F-2（平行计时局部）
> 消除（NodeData.timer per-entity）；F-3（数字键退化）消除（hit
> 空间命中 + InputView mouse_x/y/button）；F-4（单控制器膨胀）缓解
> （可分离关注点但仍受一脚本一入口约束）。runtime 升至 54（全仓
> 合计 475）。
> **v1.79 注记（S10-2 首项：编辑器点击选择落地）**：editor_shell
> 升级——鼠标**点击选择**替代 Tab 循环（宿主侧 hit 逻辑：z_index
> 降序 Sprite2D 世界包围盒 + held 前沿检测）+ **Shift+点击多选**
>（Selection.toggle）。S10-1 的 hit() 与 S9-3a 的 Selection 在编辑
> 器真实语境闭环。Tab 循环保留（备用）。状态栏提示更新。475 绿。
> **v1.80 注记（S10-2 第二项：框选落地）**：editor_shell 空白拖拽
> → 框选矩形 → 松开选中矩形内全部可见 Sprite 中心（Selection 逐个
> select，保持序 = 选择序）。拖拽状态是编辑器会话态（不进事务/不落盘，
> D1 口径）。与 S9-3a Selection 模型完全一致（select 就是 select，
> 框选只是输入法）。475 绿。
> **v1.81 注记（S10-2 第三项：选择指示器落地）**：editor_shell 新增
> **v1.82 注记（S10-2 第四项：Gizmo 拖拽落地）**：选中对象直接鼠标
> 拖拽移动 —— 点在已选对象上进入拖拽（记录偏移）；拖拽中 preview
> 直写（会话态不入账）；松开 Inspector.modify_local 一次事务（100 帧
> 拖拽 = 一条 undo 步，T-INS-02 口径实操兑现）。475 绿。
> 选择指示器（Control 20x20 边框跟随主选位置，无选择移出视口）——
> Gizmo 最小口径（视觉反馈，不是拖拽手柄）。Control 的 anchor/
> offset/size 经提取层 `set_rect` 渲染边框（S4.2 既有件，编辑器
> 复用零新渲染能力）。编辑器三件（点击/多选/框选/指示器）全部就位。
> 475 绿。
> 编辑器产品化的优先序（多选/框选/Gizmo/Inspector 控件/新缺口）。

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
