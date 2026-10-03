# NES 2.0 · S12.10 真字体与 IME 第 1 期 v1

> 交付日期：2026-10-03　｜　状态：**TTF 解析/光栅化器 + IME Unicode 输入链落地；渲染集成（动态字形图集）为第 2 期**
> 前置：S12.9 play-in-editor（e48dfff）；S4 文本光栅化（位图字体契约）；S12.2 TextInput。

---

## 0. 一句话结论

真字体路线两块地基同轮落地：**① TTF 子集解析器 + 灰度光栅化器**
（`nes-render-wgpu/src/ttf.rs`，~1300 行零依赖，png/bmp 手写编解码
先例）——sfnt/TTC 头、cmap format 4、glyf 简单+复合字形、nonzero
winding 4x4 超采样光栅化，17 个合成字体单元测试 + 5 个真字体契约
（simhei.ttf / msyh.ttc 实机验证，SimHei upem=256 老_GB 字体实测）；
**② IME 中文输入链第 1 期**——UiVm 草稿泵 Unicode 化（CJK/全角/假名
直入、UTF-16 代理对合成、控制字符照旧丢弃）+ imm32 FFI 组合窗锚定
（跟随改名输入框）。六 crate **535 全绿**、clippy 0、守卫 11/11。
**第 2 期（已排）**：动态字形图集渲染集成——TTF 光栅化结果进 GPU 图
集、比例字宽排版、多字号 Label，替换默认位图字体上屏。

## 1. TTF 子集（解析边界，显式裁剪）

| 表 | 覆盖 |
|---|---|
| sfnt 头 | 0x00010000 与 'ttcf' 集合（取第一个字体） |
| head / maxp / hhea / hmtx | unitsPerEm、indexToLocFormat、numGlyphs、度量 |
| cmap | platform 3/1（回退 3/0、0/x）**format 4**（BMP） |
| loca / glyf | short+long；简单字形 + 复合字形（XY 偏移、scale/2x2） |
| 裁剪 | format 12（增补平面）、hinting、kern——文档记录 |

光栅化：二次贝塞尔隐式中点展平 → nonzero winding 扫线 → 4x4 超采样
（16 级原始计数累加后**一次性**缩放到 0..=255——逐子行覆写会把满覆
盖像素封顶在 63，合成字体三角夹具当场抓到）。

## 2. IME 输入链第 1 期

- **草稿泵 Unicode 化**（契约从"P0 仅 ASCII"升级）：非控制标量全收
  （CJK/全角/假名）；UTF-16 代理对合成（悬挂高代理当帧丢弃，注释
  说明）；Backspace 按字符边界删除。
- **imm32 FFI**（系统 DLL 链接，user32 先例）：ImmGetContext /
  SetCompositionWindow（CFS_POINT 锚到改名输入框光标近似位）/ 
  ImmAssociateContext。组合窗本体仍由系统 IME 绘制（自绘组合串归
  第 2 期后）。
- **测试**：T-UI-12（中文/代理对/控制字符三流）；WM_CHAR 0x4E2D 真
  实消息往返（直接投递——与 IME 布局状态无关，t_in_01 的教训）。

## 3. 契约回归与门禁

| 套件 | 计数 |
|---|---|
| ttf.rs 单元（合成最小字体夹具） | 17 |
| criterion_ttf（真字体，缺字体 skip） | 5 |
| criterion_window_input | +1（CJK 往返）|
| s12_ui T-UI-12 | +1 组 |
| 六 crate 总计 | **535 全绿**（asset 34 / scene 231 / api 45 / extract 56 / wgpu 111 / runtime 72）|

clippy 0、守卫 11/11、editor_shell 120 帧冒烟干净。

## 4. 第 2 期（下一里程碑）

1. 动态字形图集：TTF 光栅化 → GPU 注册表纹理页（(char,size) 缓存、
   页满开新页）；FontParams 扩比例字宽表；渲染器文本分支按字距排布；
   光标位随比例字宽。
2. 多字号 Label：font_size 属性（S4 已冻结 8..128）接真字体光栅化。
3. 编辑器默认字体切真字体（可读性/CJK 显示）；主题字槽位评估。
4. IME 第 2 期：自绘组合串（preedit）内联显示；准确光标位回传
   （替换平均步进近似）。

## 5. 过程记注

- 两个并行子代理均因网络中断遗留半成品（ttf.rs 未接线/未修编译；
  ime 分支近完成）——集成者接手收尾：编译修复 8 处（f32 Eq derive、
  闭包生命周期、MSRV is_none_or）、夹具缺子表追加、**光栅化 63/255
  覆盖缺陷根修**、t_in_01 同款直接投递模式复用。
- SimHei unitsPerEm = 256（老 GB 字体常见），断言白名单据此放宽。
