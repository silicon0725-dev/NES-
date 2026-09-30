# NES 2.0 · M4 渲染接入 S4.4 文本光栅化 v1

> 交付日期：2026-09-30　｜　状态：**S4.4 完成（S4.3 后的增量，未动任何上游 crate）**
> 前置：S4.1 封口、S4.2 批量与控件、S4.3 纹理注册表（各文档见本目录）。
> 本轮关闭封口文档 §7 的最后一项绘制缺口：`SetText` 从"只记账"变为**产生像素**。

---

## 0. 一句话结论

`CommandConsumer` 新增 `set_default_font`：登记外部烘焙的字形表后，`SetText` 的文本
被展开成"每字形一个四边形"的实例（采样注册表里的字形表纹理，与精灵/控件共用同一条
管线），混排场景（清屏 + 精灵 + 控件边框 + 四段 Consolas 文本）实机出图且**逐字可读**
（目视转述核对）。`nes-render-wgpu` 测试增至 **26 项全绿**，S4.1~S4.3 全部用例与示例
复跑不回归。

---

## 1. 形态与裁决

### 1.1 字形栅格化在外部，排版在 CPU 侧（契约纪律不变）

- **烘焙**：System.Drawing 把系统字体（Consolas 12px，回退 Courier New）逐字符画进
  16x16 字格（16 列 x 6 行，覆盖 ASCII 32..126），二值渲染提示（无抗锯齿 fringe），
  产出 `examples/assets/font_atlas.bmp` + `font_metrics.txt`；
- **装载**：新增库模块 [`bmp`]（极简 BMP 解析：24/32bpp、BI_RGB、自底上行序 + 通道
  交换；无熵编码格式才手写，与手写 PNG 编码器同一条纪律），两个示例共用；
- **登记**：`set_default_font(FontParams, rgba)` —— 字形表作为一张纹理注册进
  [`TextureRegistry`]（保留键 `slot = u32::MAX`），`FontParams` 携带字格/列数/
  首字符/字符数/字距/行高。

### 1.2 布局口径（S4.4 最小，如实声明）

- 渲染物的世界变换 = **笔起点**（首行首字格左上角）；每字形一个四边形，
  落位 `T(char_index * advance, line * (line_height + line_spacing)) ∘ S(cell/16)`；
- **等宽字距**（advance 恒定）、`\n` 多行、行高 = 基准 + `LabelState::line_spacing`；
- 空格与表外字符**只推进笔位不画**（空格无墨是字体事实，表外字符无字形可采）；
- `font_size` 缩放、`align_h/align_v`、`wrap_width` **记账不参与布局** —— 对齐与
  自动换行需要排版框语义（关联归档说明 Q-S3-2 家族的场景属性入口裁决），属后续；
- `LabelState.font != NIL` 时按该键查注册表（自定义字体表），查不到退回默认字体；
  未设默认字体时文本静默不画（记账不变，不误报）。

### 1.3 文本渲染物不依赖纹理键

与控件同理（S4.2 先例）：有 `SetText` 状态的渲染物按文本对待，`RenderAssetKey::NIL`
即可绘制；条目表/文本表跨帧持有（`Create` 建、`Destroy` 删，契约 I4）。

## 2. 新增用例与演示

| 项 | 验证什么 |
|---|---|
| `criterion_backend_label_text_raster` | 程序化字形表（每字符一格纯色，无需外部资产）：字距落位、多行（行高 + line_spacing）、空格/表外字符只推进笔位、NIL 键文本照常绘制、未设字体时静默不画、跨帧条目持续重画 |
| `bmp::tests`（2 条） | 自底上行序 + BGR(A)→RGBA 通道交换、畸形拒绝（魔数/压缩/截断） |
| `examples/s44_label_text.rs` | 真 Consolas 字形表 + 混排一帧：精灵 + 控件边框 + 四段文本（"NES 2.0" / "wgpu-native" / "S4.4 text" / "clear + sprite + control + text"）；断言字形计数、文本行带有墨、行带外背景；产物 PNG 经目视逐字转述核对 |

## 3. 本轮改动清单

| 文件 | 改动 |
|---|---|
| `src/bmp.rs` | **新增**：极简 BMP 装载器（+2 单测） |
| `src/renderer.rs` | `texts` 登记表 + `DefaultFont` + `set_default_font(FontParams, rgba)` + 字形展开（含空格/表外跳过）+ `FrameStats.glyphs` |
| `src/lib.rs` | 模块表与再导出（`bmp`、`FontParams`） |
| `examples/assets/` | `font_atlas.bmp` + `font_metrics.txt`（外部烘焙产物） |
| `examples/s44_label_text.rs` | **新增**：混排演示 |
| `tests/criterion_backend.rs` | 文本光栅化用例（含程序化字形表辅助函数） |

## 4. 实测基线（更新）

| 检查项 | 结果 |
|---|---|
| `nes-render-wgpu` | test **26** 全绿（lib 10：png 4 + bmp 2 + renderer 4；集成 criterion_backend 16）｜ clippy `--all-targets -D warnings` 零警告 ｜ 连续 3 轮复跑无抖动 ｜ 三个示例（s41/s43/s44）全部 PASS |
| 其余四 crate + 守卫 | 未触碰（75 / 34 / 40 / 42 / 10-10） |

## 5. 遗留更新（对 S4.1 封口文档 §7）

| 事项 | S4.3 后 | S4.4 后 |
|---|---|---|
| Label 光栅化 | 未启动 | ✅ 等宽字形表最小口径（对齐/换行/字号缩放待 Q-S3-2 家族裁决） |
| 按纹理原始尺寸绘制 | 遗留 | 实例侧路径已在 s43 演示（`transform ∘ scale`），引擎侧封装待提取层接线 |
| 窗口/surface、headless Linux、M5、编辑器 | 未启动 | 未启动 |

*（内容由AI生成，仅供参考）*
