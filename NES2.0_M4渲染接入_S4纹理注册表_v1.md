# NES 2.0 · M4 渲染接入 S4.3 纹理注册表 v1

> 交付日期：2026-09-30　｜　状态：**S4.3 完成（S4.2 后的增量，未动任何上游 crate）**
> 前置：`NES2.0_M4渲染接入_S4最小可视闭环_封口_v1.md`（S4.1）、
> `NES2.0_M4渲染接入_S4批量与控件_v1.md`（S4.2）。
> 本轮关闭封口文档 §7 的"真实纹理注册表"项：精灵可以采样宿主侧上传的真实纹理。

---

## 0. 一句话结论

`CommandConsumer` 新增宿主侧 **`register_texture`** API：按 `RenderAssetKey` 上传任意
RGBA8 纹理（≤256px，NES 级素材口径），注册过的键采样真实贴图、未注册的仍走内建
图集格，**同一次 draw 内两路混画**。`nes-render-wgpu` 测试增至 **23 项全绿**
（lib 8 + 集成 15），clippy 零警告，S4.1/S4.2 全部用例与示例复跑不回归。

---

## 1. 形态与裁决

### 1.1 注册表 = 单张平铺大纹理（不是数组纹理）

初版实现用 2D 数组纹理（每张上传一个图层），被实测否决：**本版 naga（wgpu-native
v29 资产）对数组纹理采样内建的重载解析与标准 WGSL 不符**——`textureSampleLevel`
的四参形式报"expected 5, found 4"，补偏移参又报类型错，`textureLoad` 三参形式同样
报错；而 `texture_2d` 的四参显式 LOD 形式自 S4.1 起实证可用。于是注册表改为
**一张 2D 大纹理**：256px 瓦片平铺，2x2（512px）起步、边长翻倍扩容（上限 8192px
= 1024 瓦片），采样与内建图集**完全同构**。位置信息全部并入实例的 UV 矩形，
着色器两路只差一个纹理绑定。

### 1.2 实例数据扩到 12 个 `f32`（48 字节）

`[世界矩阵 x6, UV 矩形 x4 (u0,v0,us,vs), 采样来源 x2 (瓦片号, 类型)]`。
类型 0 = 内建图集（UV = 格子矩形）、1 = 注册表瓦片（UV = 大纹理坐标系子矩形，
按上传尺寸裁剪）；瓦片号仅供帧对账留档。管线布局改为自持的
`[图集绑定组, 注册表绑定组]`（group 0 / group 1）。

### 1.3 上传 = `queueWriteTexture` + CPU 侧行补齐

源行补齐到 256 字节倍数（`bytes_per_row` 的设计用途），**不需要** 新增
`copyBufferToTexture` 符号或暂存缓冲。2x1 这类窄纹理的补齐路径有专项测试。

### 1.4 语义口径

- 同键重复注册 = **原瓦片覆写**（瓦片号不变，热重载语义）；
- 尺寸超限（>256px）/ 字节数不符 / 零尺寸 → `ConfigMismatch` / `PixelBufferSize` /
  `InvalidImageSize`，**被拒的注册不留痕**；
- 扩容重建后既有瓦片逐张重传（CPU 侧留档），绑定组换绑新视图；
- 未写到的纹素保持透明（WebGPU 零初始化保证），经 alpha 丢弃透出下层；
- 绘制尺寸仍是 16px 格（纹理按 UV 拉伸，如 2x1 纹素各占半格）——按纹理原始
  尺寸绘制属后续（需实例数据再扩或 CPU 侧换算）。

## 2. 新增用例（4 条，累计 23）

| 用例 | 验证什么 |
|---|---|
| `texture_registry_sampling` | 16x16 四象限纹理逐象限像素正确；与未注册键（内建格）**同帧两路混画**；`stats.from_registry` 计数；同键覆写下一帧生效且瓦片号不变 |
| `texture_registry_padded_upload` | 2x1 纹理（行字节 8 → 补齐到 256）上传正确：左半第一纹素、右半第二纹素 |
| `texture_registry_growth` | 5 张纯色纹理（初始容量 4）触发边长翻倍扩容 + 既有瓦片重传，5 个精灵各采对各的颜色 |
| `texture_registry_rejects` | 超限 / 字节数不符 / 零尺寸三类拒绝路径，且被拒注册不留痕 |

## 3. 本轮改动清单

| 文件 | 改动 |
|---|---|
| `gpu.rs` | 新增 [`TextureRegistry`]（平铺大纹理 + 采样器 + 绑定组布局/绑定组 + 瓦片分配/覆写/扩容重传 + `sample_info`） |
| `renderer.rs` | WGSL 两路同构采样（`texture_2d` 四参显式 LOD）；实例数据 12 浮点（新 UV 矩形 + 采样来源）；管线布局自持（双绑定组）；`render` 绑定 group 1；`CommandConsumer` 持注册表并提供 `register_texture` / `registry()`；`FrameStats.from_registry` |
| `ffi.rs` | 新增 `WGPU_VERTEX_FORMAT_FLOAT32X4`（UV 矩形属性） |
| `tests/criterion_backend.rs` | 4 条注册表用例 |

## 4. 实测基线（更新）

| 检查项 | 结果 |
|---|---|
| `nes-render-wgpu` | test **23** 全绿（lib 8 + criterion_backend 15）｜ clippy `--all-targets -D warnings` 零警告 ｜ `s41_visual_closure` 复跑 PASS ｜ 连续 3 轮复跑无抖动 |
| 其余四 crate + 守卫 | 未触碰（75 / 34 / 40 / 42 / 10-10） |

## 4.1 实图验证（`examples/s43_real_textures.rs`）

用真实世界图片走完整链路（外部工具解码 → 32bpp BMP → `register_texture` →
GPU 瓦片上传 → 采样渲染 → PNG）：

| 源文件 | 格式与尺寸 | 转换后 | 结果 |
|---|---|---|---|
| `25E7...4624B.jpg` | JPEG 624x624（动漫插画） | 256x256 | 瓦片 0，像素校验过 |
| `555D...DE2DE.gif` | GIF 320x207（取首帧） | 256x166 | 瓦片 1，像素校验过 |
| `text.png` | PNG 7016x4961 RGBA（彩色涂鸦扫描） | 256x181 | 瓦片 2，像素校验过 |
| `a-arrow-up.svg` | SVG（矢量） | —— | **跳过**（GDI+ 不光栅化矢量，如实记录） |

- 预处理：System.Drawing 统一转 32bpp BMP（解码 JPEG/PNG/GIF 属外部工具职责，
  crate 保持零依赖；BMP 是唯一无熵编码、可安全手写解析的格式，示例内含 40 行解析器）；
- 渲染：512x768 离屏画布货架布局，每图按转换后尺寸绘制（16px 基准四边形 x 尺寸缩放，
  即"按纹理尺寸绘制"的实例侧实现路径），`from_registry=3`、`driver_errors=0`、
  去重颜色 54,363 种；
- 验证：每图 3 个内部采样点输出像素 == 源纹素（透明纹素透出清屏色），
  产物 PNG（1.5 MB）另经目视确认无错位/变形/通道互换。

## 5. 遗留更新（对 S4.1 封口文档 §7）

| 事项 | S4.2 后 | S4.3 后 |
|---|---|---|
| 真实纹理注册表 | 未启动 | ✅ 宿主侧 API 落地（`register_texture`；场景侧从 `nes-asset` 接真实像素属提取层后续接线） |
| Label 光栅化 | 未启动 | 未启动（下一增量候选：内嵌位图字库） |
| 按纹理原始尺寸绘制 | —— | 新增遗留：当前绘制尺寸恒为 16px 格 |

*（内容由AI生成，仅供参考）*
