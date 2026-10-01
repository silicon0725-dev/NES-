# NES 2.0 · S6.34 编辑器脚本面板 v1

> 交付日期：2026-10-01　｜　状态：**引擎窗口里的实时脚本编辑器 —— 键入、提交、热重载、状态回显全在引擎自己的渲染管线里**
> 前置：S6.31/32 内嵌脚本与热重载；S4.4 Label 文本光栅化；S4.2 Control 边框；窗口模式（M4 S4 最小可视闭环）。

---

## 0. 一句话结论

三层拼合出**编辑器最小闭环**：① 窗口输入泵（`wnd_proc` 的 WM_CHAR →
进程级字符队列，`drain_chars`/`inject_char` 双口）；② 宿主编辑解释
（回车提交 / 退格 / 可打印追加 —— `Panel::feed`，纯宿主逻辑不进引擎）；
③ 引擎已有件不动：`set_prop source` + `vm.poll_reloads`（S6.32 热重载）
+ Label/Control（S4.4/S4.2）。示例 `script_panel`（768x432：上半精灵
演示、下半三行面板 `src>`/`in>`/`st>`）与 T-Panel-R1 端到端钉死：
**键入 → 提交 → 重编译 → 下一帧精灵换位像素**；坏脚本 last-good。
出口准则 T-In-01 + T-Panel-R1 全过。全仓测试 34 / 165 / 40 / 42 /
**84** / **31** 全绿（合计 396），守卫 11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 输入泵（窗口 crate 的最小职责）

```text
WM_CHAR(0x0102) ──wnd_proc──> TYPED: Mutex<Vec<u32>>（进程级静态队列）
                                    │
              drain_chars() 取走清空 │  inject_char() 测试/自动化注入
                                    ▼
                        宿主编辑解释（Panel::feed）
```

- **单窗口口径**：静态队列不区分窗口（当前一进程一窗口；多窗口属
  编辑器壳层的后续裁决，队列按 HWND 分流是扩展点不是缺口）；
- **字符码不译码**：队列存 `u32` 原码；可打印范围（32..127）由宿主
  定（与字形表覆盖一致），控制字符（退格 0x08 / 回车 0x0D）语义由
  宿主解释 —— 引擎只管"字符到了"；
- `inject_char` 与 `PostMessageW(WM_CHAR)` 落进**同一队列**（T-In-01
  用真窗口消息验证生产路径，T-Panel-R1 用注入验证内容路径）。

### 1.2 编辑解释在宿主（引擎不学编辑器）

`Panel::feed(code) -> bool`（回车返回 true = 提交）：退格弹尾、
可打印追加（上限 120 字符）、其余忽略（非 ASCII 码点不进缓冲 ——
字形表只覆盖 32..127，如实不假装支持）。提交动作是宿主惯用法：

```text
set_prop(brain, "source", 缓冲文本) → vm.poll_reloads(tree)
  ├─ Ok   → st> OK reloaded N（行为已更新）
  └─ Err  → st> ERR <编译错误>（kept last-good —— S6.32 语义）
```

### 1.3 面板即场景（零新渲染件）

三行 Label + 一个 Control 边框 —— S4.4/S4.2 的既有件。`src>` 行显示
**当前生效**源码（`brain.source` 属性，wrap 46 列手动折行）、`in>` 行
显示编辑缓冲 + 光标 `_`、`st>` 行显示提交结果。**引擎用自己的渲染
管线显示自己的编辑器**。

### 1.4 两个实证缺口（本轮修掉，记录在案）

| 症状 | 根因 | 修法 |
|---|---|---|
| 精灵渲染**内建控件格**（纯绿空心框）而非注册纹理 | `open_windowed` 资产根缺省 `.`（当前目录）——示例把纹理写进临时根，`bind_assets` 静默找不到 | 示例改 `open_windowed_with_root` + **绑定/上传断言**（loaded==1、uploaded==1，这类错不再可静默） |
| Label 渲染成**实心色块** | 默认字体未登记（字形表是宿主侧登记件，S4.4 口径；无字体时文本回落图集格） | 示例/测试装载烘焙字形表（`nes-render-wgpu/examples/assets/font_atlas.bmp` + metrics），经 `rt.consumer_mut().set_default_font` |

附带口径：面板显示文本用 **ASCII**（字形表覆盖 32..127；CJK 显示为
空白不是错误，编辑器状态行如实用 ASCII）。

## 2. 实现落点

| 位置 | 改动 |
|---|---|
| `nes-render-wgpu/src/window.rs` | WM_CHAR 分支入 `wnd_proc`；`static TYPED: Mutex<Vec<u32>>`；`drain_chars()`（取走清空）/ `inject_char()` |
| `nes-render-wgpu/tests/criterion_window_input.rs` | T-In-01（新文件：真窗口 PostMessageW → pump → drain 顺序/清空/控制字符） |
| `nes-runtime/examples/script_panel.rs` | 面板示例（场景搭建 + Panel 编辑解释 + 提交热重载 + `NES_PANEL_FRAMES`/`NES_PANEL_TYPE` 自动化钩子：第 80 帧注入键入 + 回车） |
| `nes-runtime/tests/criterion_panel.rs` | T-Panel-R1（新文件：窗口运行时端到端） |

引擎语义件（scene/script/extract）**零改动** —— 面板是已有件的组合。

## 3. 出口准则

### 3.1 窗口输入（`criterion_window_input.rs`，1/1）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-In-01 | `inject_char` + `PostMessageW(WM_CHAR)` 同队列；pump 后 drain **按序**、**取走清空**；控制字符（0x08/0x0D）原样可达 | ✅ |

### 3.2 运行时端到端（`criterion_panel.rs`，1/1）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Panel-R1 | 窗口运行时：键入（字符队列）→ 宿主解释 → 回车提交 → `poll_reloads` 恰命中 brain → **精灵像素 A 位 → B 位**（棋盘色逐像素断言，A 位同时清空）；坏脚本 `)` 编译错误**如实进清单不装载**、精灵保持 B（last-good）；面板 Label 字形有墨（≥8 px）；driver_errors 0 | ✅ |

### 3.3 演示验证（`script_panel`，截图像素扫描）

- 注入前（t≈1s）：棋盘精灵在 INITIAL 脚本驱动下逐帧漂移（client
  (52,48)，+2/帧实证）；
- 注入后（t≈4s，第 80 帧键入 `on "step" { sprite.pos = (40.0, 120.0) }`
  + 回车）：精灵**恰在 client (40..55, 120..135)**（16x16 棋盘 128 亮格）；
- 三行文本可读（PrintWindow 截图 3x 放大目检）：
  `src> on "step" { sprite.pos = (40.0, 120.0) }` /
  `in> …_` / `st> OK reloaded 1 (behavior updated)`。

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 光标移动/选区/多行编辑（←→↑↓、Home/End、Insert） | 未启动（当前单行追加/退格口径） |
| 非 ASCII 输入（IME / 字形表扩覆盖） | 未启动（队列存 u32 已兼容，字形表是缺口） |
| 多窗口输入分流（队列按 HWND） | 单窗口口径（§1.1） |
| 面板行数超界滚动 / 长脚本折叠 | 未启动 |
| headless Linux / WM_SIZE / DPI | 沿各文档遗留表 |

## 5. 记账

- 测试基线：nes-asset 34 / nes-scene 165 / nes-render-api 40 /
  nes-render-extract 42 / **nes-render-wgpu 84**（83 -> 84，+T-In-01）/
  **nes-runtime 31**（30 -> 31，+T-Panel-R1）—— 全绿，合计 **396**；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-render-wgpu（window.rs + 新测试）与 nes-runtime
  （新示例 + 新测试）；**场景/提取/渲染语义零改动**。

*（内容由AI生成，仅供参考）*
