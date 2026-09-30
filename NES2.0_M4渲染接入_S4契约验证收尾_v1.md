# NES 2.0 · M4 渲染接入 S4.7 契约验证收尾 v1

> 交付日期：2026-09-30　｜　状态：**契约验证阶段收官（T-Registry + T-Stats + 视觉基线）**
> 前置：S4.5 文本契约、S4.6 渲染契约（T-Sprite/T-Control/T-Camera）。
> 本轮补齐 §5 列出的三个空缺，渲染面的契约回归体系至此完整闭合。

---

## 0. 一句话结论

注册表交叉语义（T-Registry 8 项）、帧统计完备对账（T-Stats 6 项，旗舰用例把
**36 条命令、10 个统计字段**全部与手工账面对齐）、端到端**视觉基线**（复合场景
PNG 字节哈希锚定 + bless 重录模式）全部落地；`nes-render-wgpu` 测试增至
**79 项全绿**，五 crate 基线 75/34/40/42/79，clippy 零警告，三轮无抖动。

---

## 1. 本轮交付

### 1.1 T-Registry（`tests/criterion_registry_contract.rs`，8 项）

键隔离 / 同键覆写热重载（瓦片号不变）/ **覆写换尺寸**（UV 裁剪随新尺寸更新，
32x8 半红半蓝纹理的左右半区断言）/ 扩容保持 / **双次扩容**（17 张，4->16->64，
逐张核对）/ NIL 键注册被拒（不留痕）/ 字体与精灵纹理共存（含扩容互不干扰）/
未注册键回退内建格。

### 1.2 T-Stats（`tests/criterion_stats_contract.rs`，6 项）

空帧全零 / 忽略逐条计数（NIL + 未知句柄）/ **混合帧全量对账**（7 创建 + 1 销毁 +
相机 + 24 属性 + SetRect + SetText + Submit = 36 条命令；updates=26、drawn=5、
controls=1、glyphs=2、from_registry=1、skipped=2 —— 每个字段与手工账面严格一致）/
逐帧重置（生命周期事件不重复计数）/ frame_index 透传 / `drawn==0 ⟺ 画面全清屏`
（计数与像素互证）。

### 1.3 视觉基线（`tests/criterion_visual_baseline.rs`）

- 场景全程序化（清屏 + 内建精灵 + 注册表精灵 + 控件边框 + "NES 2.0" 文本 +
  单位相机，256x128），无外部文件依赖，逐次运行确定；
- 锚定整帧 PNG 字节的 **FNV-1a 64**（本机基线 `0xf6cf1788941760b7`）；
- 与逐像素锚点的分工：锚点钉**契约语义**，基线钉**漂移**（驱动更新、着色器改动、
  打包顺序变化等任何未预期像素变化，即使不违反契约也会显形）；
- **bless 重录**：`NES_RENDER_BLESS_BASELINE=1 cargo test --test criterion_visual_baseline`
  打印新哈希（打印即通过），填回常量完成重录 —— 基线与 GPU/驱动相关，换机重录
  是预期操作而非测试缺陷。

### 1.4 库侧契约收紧

`register_texture(NIL)` 现在被拒（`ConfigMismatch`）：NIL 保留给"未绑定"语义，
NIL 键的渲染物在绘制过滤前就被跳过，注册进去是一张永远采不到的死纹理。
与 `set_custom_font` 拒绝 NIL 键同一口径（T-Reg-06 钉住）。

## 2. 契约回归体系总览（收官状态）

| 矩阵 | 项数 | 载体 |
|---|---|---|
| T-Text | 14 | `criterion_text_contract.rs` |
| T-Sprite | 8 | `criterion_sprite_contract.rs` |
| T-Control | 8 | `criterion_control_contract.rs` |
| T-Camera | 8 | `criterion_camera_contract.rs` |
| T-Registry | 8 | `criterion_registry_contract.rs` |
| T-Stats | 6 | `criterion_stats_contract.rs` |
| 视觉基线 | 1 | `criterion_visual_baseline.rs` |
| 机制测试 | 16 | `criterion_backend.rs`（失败路径/几何/扩容等） |
| **合计** | **69** | + lib 单测 10 = 79 |

三轮契约验证共抓到并修复 **4 个真实问题**（font 键未实现、ControlState offsets
语义误用、NIL 纹理键死条目、若干测试自身的坐标口径错误沉淀为方法论）。

## 3. 实测基线（最终）

| 检查项 | 结果 |
|---|---|
| `nes-render-wgpu` | test **79** 全绿 ｜ clippy `--all-targets -D warnings` 零警告 ｜ 连续 3 轮无抖动 ｜ 三示例 PASS ｜ 视觉基线钉住 |
| 其余四 crate + 守卫 | 未触碰（75 / 34 / 40 / 42 / 10-10） |

## 4. 下一阶段候选（契约验证之外）

1. **窗口 / surface 接入**（Win32 手写 FFI，摆脱"纯离屏"）；
2. **headless Linux 认证**（cfg 分支 + dlopen + WSL 编译验证，原生认证目标）；
3. **M5 Scratch 兼容层**（绕中心翻转补偿等口径已在文档就位）；
4. **提取层 -> 后端接线**（nes-asset 真实像素 -> register_texture 的引擎侧管线，
   当前为宿主侧手动注册）。

*（内容由AI生成，仅供参考）*
