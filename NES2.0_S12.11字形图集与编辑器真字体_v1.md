# NES 2.0 · S12.11 字形图集与编辑器真字体 v1

> 交付日期：2026-10-03　｜　状态：**TTF 渲染集成（动态字形图集）+ 壳层真字体切换 + IME 锚点精确化落地**
> 前置：S12.10 第 1 期（TTF 解析/光栅化 + IME Unicode 泵，5b1add6）；S12.9 play-in-editor（e48dfff）。

---

## 0. 一句话结论

第 2 期三块同轮收口：**① 渲染集成**——`nes-render-wgpu/src/glyph.rs`
动态字形图集（256x256 页贪心 shelf 装箱 + (char, 字号) 进程内缓存 +
`0x7000_0000` 页键命名空间），`CommandConsumer::set_ttf_default` 装载后
`font == NIL` 文本自动改走真字体比例排版（任意字号 clamp 8..128、CJK
上屏、光标条随比例字宽）；**② 壳层接入**——editor_shell 启动字体探测
链（msyh.ttc → simhei.ttf → segoeui.ttf，本机命中 msyh，全部缺失优雅
回退位图 + Output 一行）、全部 UI 文本统一字号 14；**③ IME 锚点精确
化**——组合窗锚点从 10px 平均步进升级为真字宽逐字累加（与渲染器光标
条算式同源）。六 crate **561 全绿**（基线 561）、clippy 0、守卫 11/11、
editor_shell 120 帧冒烟干净、NES_EDIT_DEMO 全断言（新增真字体取证）。

## 1. 渲染集成（nes-render-wgpu，第 2 期主体）

- **页命名空间**：字形第 n 页键 = `RenderAssetKey::from_parts(0x7000_0000 | n, 1)`。
  与资源 arena 键（slot 自 0 增长，到不了 2^30）、非资源键（0x8000_0000
  段）、后端保留键（`DEFAULT_FONT_KEY` slot 全 1）位空间互斥；页号上限
  2^28 远超注册表容量天花板（8192px 边长 = 1024 页），gen 固定 1（页内
  容只增不改，不存在换代）。
- **装箱**：贪心 shelf、只在最后一页顺序装箱——行满换行（行高 = 本行
  最高字形）、竖满开新页（旧页不迁移）、单边超 256px 不装箱（只推笔位）。
  编辑器字形"先冷后稳"：头几帧装箱 + 整页上传，稳态零分配零上传。页
  内容只增不改 ⇒ 同键重复注册整页是安全覆写（注册表语义即"同键覆写 +
  立即 writeTexture"，装箱当帧可采样，无需 flush）。
- **排版算式**（`push_ttf_label`）：字号 clamp 8..128 取整作缓存键；第 i
  行基线 = `i * (line_height + line_spacing) + ascent`（line_spacing 逐行
  叠加，T-Text-13 在 TTF 路径同样成立）；字形四边形左/顶 = 笔位 +
  bearing；UV 按页内矩形折算（S8.2 修复口径）；缺字形跳过不画、笔位走
  `.notdef` advance（一排 notdef 方块比安静留白更吵，排版节奏仍在）。
- **光标**：`caret Some(n)` 竖条 1px x line_height，x = 前 n 字符 advance
  之和（'\n' 归零换行）——IME 锚点（§3）与之同源。
- **第 1 期两处解码缺陷修复**（真实字体集成期抓到，合成最小字体夹具钉
  死回归 `minimal_font_negative_short_vectors_and_range_offset`）：
  ① short 向量负增量：`X/Y_SAME_OR_POSITIVE` 标志位**清位即 -d**（曾误
  作 `d - 256`，SimHei 实测轮廓整体错位/放大 6~27 倍）；② cmap format 4
  idRangeOffset 间接寻址：glyphIdArray 位置 = **本条目位置 + 偏移**（曾
  漏加偏移，msyh.ttc 全部间接段查 None/读垃圾）。
- **T-GT 契约套件**（tests/criterion_ttf_text.rs，5 例）：装载后 NIL 文本
  产像素且两帧逐位确定（T-GT-01）、多字号缩放（02）、光标随比例 advance
  （03，容差 2px）、**位图基线逐位不变 + 显式 font 键不受影响**（04）、
  CJK 上屏（05）。缺 wgpu-native 库/缺系统字体 skip，不伪造失败。

## 2. 壳层接入（editor_shell）

- **字体探测链**（`FONT_CANDIDATES`，按优先级）：`msyh.ttc`（CJK+拉丁
  全覆盖现代 UI 字体）→ `simhei.ttf`（CJK 兜底）→ `segoeui.ttf`（纯拉丁
  兜底）；第一个**可读且可解析**的经 `consumer_mut().set_ttf_default(data)`
  装载。本机命中 **msyh.ttc**（upem=2048，14px 行高 18.48、'中' advance
  14.0px）。单文件损坏/截断 = 记行后继续下一候选（探测链的意义就是单
  点失败不致命）；**全部缺失 = 位图回退**：位图默认字体本就先登记，回
  退路径永远可用，Output 记 `font: bitmap fallback` 一行，不 panic。每次
  尝试都落一行（命中/解析失败/回退）——冒烟断言按行取证。
- **统一字号 14**（`UI_FONT_SIZE`，逐处决定清单）：
  | 文本面 | 字号 | 通道/理由 |
  |---|---|---|
  | 全部 Label（面板标题 Scene/Inspector/Output/res:、Inspector 分区标题与属性行、状态栏） | 14 | schema 键 `font_size` set_prop |
  | 标尺数字 | 14 | 观感：14px 行高 ≈18.5px 贴 16px 条带；16px 下探 5px 进视口 |
  | 改名输入框（TextInput） | 14 | schema 无该键 → `set_prop_raw` 前向通道（z_index/border_w 先例）；与面板同字号，且 IME 锚点与光标条渲染同源 |
  | 工具栏六按钮（Button） | 14 | 同前向通道；msyh 实测 16px "RESET" advance 和 ≈46px + 4 内衬越过 48px 按钮右缘，14px 实测 40.6px（墨迹 ≈39px）贴边装得下 |
  提取层配套（nes-render-extract，**additive、缺省逐位不变**）：
  TextInput/Button 的 LabelState 字号改读 `font_size` 属性，缺省
  `DEFAULT_LABEL_FONT_SIZE=16`。
- **竖向行步进 16 → `INS_ROW_H`=20**：msyh 14px 行高 ≈18.5px，16px 步
  进下 Inspector 属性行顶进改名框；20px 给足余量，位图回退（行高恒
  16px）只是行距略宽（降级观感，不破相）。分区标题命中带同步 20px。
- **布局口径复核清单**（等宽 16px 假设逐处过）：
  - `INS_LINE_CHARS=11`（候选文件名截断）：**保持**——真字体是可选增强，
    预算必须按回退位图模式取界（11x16=176 ≤ 178 内衬宽）；真字体下同
    px 容字 ≈2 倍，只会更宽松；
  - `DOCK_LINE_CHARS=40`：**不变**——Output dock 是 ListView（ListState
    位图路径，本里程碑不接真字体），advance=16 未变；
  - 标尺数字密度（128px 一个）：**不变**——14px 下 3 位数 ≈21px、位图
    48px，余量都巨大；
  - 状态栏长文本（≈150 字符）：位图下本就超屏被裁，真字体 14px 收窄约
    一半，768 窗仍超（已知观感，非回归；溢出只会变少）。
- ListView 行文本（层级树/Output/FileSystem）维持位图 16px 等宽——列
  表行接真字体归后续里程碑（§5）。

## 3. IME 锚点精确化（第 1 期 10px 平均步进退役）

- **算式**：`caret_x = 输入框视口位 + 4px 内衬 + Σ advance(draft.chars[..caret])`
  （`ime_caret_offset`，editor_shell 内纯函数）。TTF 模式按 (char, 字号)
  查 hmtx 度量、缺字形走 `.notdef` advance、'\n' 归零——与渲染器
  `push_ttf_label` 光标条算式**同源**；位图回退按默认字体等宽 advance
  （16px，也准于旧 10px）。字号 = `UI_FONT_SIZE`，与输入框渲染字号同源
  （提取层读同一属性）——同字体同字号下累加值与光标条逐位一致，这才
  是"精确"的判据。
- **字体实例**：壳层持同一份字体数据的**独立 TtfFont**（重复解析一次可
  接受——解析器纯 CPU 无共享状态；不共享是刻意的：消费器在 rt 内部，
  取实例要跨所有权，抄字节重 parse 最省事）。
- **读口**：`UiVm::text_state(node) -> Option<TextState>` **S12-2 已公开**
  （draft + caret，s12_ui 9 组断言在钉）——无需 additive，任务条件分支
  不触发。
- 失焦/不可见帧不调（第 1 期口径延续）；窗口缩放折算（客户区==视口
  1:1 直算）仍是已知限制。demo 冒烟：真字体模式下 Char(0x4E2D) 端到端
  草稿断言照旧绿（TTF 只换排版不改数据面），新增字体取证断言（§2）。

## 4. 门禁计数

| 套件 | 计数 |
|---|---|
| nes-asset | 34 |
| nes-scene | 231 |
| nes-render-api | 45 |
| nes-render-extract | 56 |
| nes-render-wgpu | **123**（单元 24：ttf 18 + glyph 图集 6；T-GT 契约 +5；文本/注册表/精灵等既有契约 94） |
| nes-runtime | 72 |
| **六 crate 总计** | **561 全绿**（基线 561） |

clippy 0（六 crate --all-targets）、守卫 11/11、editor_shell 120 帧冒烟
干净、NES_EDIT_DEMO=1 全断言（挂载/卸载/enabled/折叠/刷新/play/stop/
reset/IME 草稿 + 新增字体装载取证：命中行存在、真字体模式无回退行无
解析失败行、回退模式必有回退行）。

## 5. 遗留（按优先级）

1. **多页大字符集性能**：图集进程内不淘汰（内存上限 = 页数 x 256KiB，
   规模可控）；极端场景（整本字典逐字轮播）会持续开页 + 重复整页上传
   ——出现真实需求再做 LRU/页回收（需处理 GPU 页引用失效同步）。
2. **粗细体/字族**：set_ttf_default 单槽位（默认字体）；粗体/斜体/多字
   族需要字体槽位表与 Label 侧 font 家族属性。
3. **字体选择 UI**：编辑器偏好里选 UI 字体（探测链是缺省序，不是配置面）。
4. **ListView 行接真字体**：层级树/Output/FileSystem 行仍是位图 16px
   等宽（ListState 路径未接字形图集）——接通后 dock 字数预算按新步进
   重推。
5. **IME 窗口缩放折算**：锚点按客户区==视口 1:1 直算，缩放窗口后有偏差
   （第 1 期已知限制延续，P0 客户区即视口）。
