# NES 2.0 S19.3 SIGNALS 信号面板（谁在发、谁在听、发了几次）v1

- 分支：`s19-3-signals`（wt-sig 独立 worktree）；基线 HEAD `e2ae11e`（S19.2 Inspector 重组）
- 改动面：`nes-scene/src/tree.rs`（additive 诊断面）、`nes-runtime/src/extension.rs`（additive 只读访问器）、`nes-runtime/examples/editor_shell.rs`（页签 UI + 静态扫描）、`nes-runtime/examples/assets/Scripts/spin.nes`（demo 夹具加一行 emit）、新增 `nes-scene/tests/s19_3_signal_stats.rs`；零新依赖、依赖分层不变
- 蓝图依据：`NES2.0_S19.0编辑器组织蓝图_v1.md` §3.2（SIGNALS 面板 —— 谁在发、谁在听、发了几次）

## 0. 结论

1. **运行计数**：`SceneTree` 新增 `signal_stats: HashMap<String, u64>`（信号名 -> 累计**送达**次数）+ 只读读面 `signal_stats_sorted() -> Vec<(String, u64)>`（计数降序、同计数名字典序）。递增点在 tick 阶段 5 信号泵的两处实际交付位（广播一次计一、路由每条命中连接各计一）；级联再发射回同泵照实各计。**不进语义指纹、不进序列化**（照 `played_sounds` 副作用通道口径；指纹采样面查证见 §1.3）。
2. **静态扫描**：editor_shell 壳层纯函数 `scan_signal_refs(source) -> (Vec<String> emits, Vec<String> ons)` —— 零依赖手写扫描（非 regex crate），`.nes` 形态 `emit "x"` / `on "x"`、`.js` 形态 `emitSignal("x")` / `onSignal("x")`；整行注释（trim 后 `//` 起）剔除，行尾注释**不剔除**（蓝图 Q3 误报容忍口径，如实标注）；去重 + 字典序。5 个单元测试（`cargo test --example editor_shell` 跑）。
3. **页签 UI**：Output dock 标题行内 `[OUTPUT][SIGNALS]` 两枚小页签按钮（工具栏按钮模式：九宫格底板 + 透明底 Button，开在既有 "dock" 容器下 —— walk 整子树跳过）；活动页签文本 `*` 后缀；会话态 `dock_tab` 不进树。SIGNALS 视图 = 复用 hud_dock 的 ListView 行 `<name>  emitted:N  on:<k>  <tags>`（name 字典序），来源拼注 g=游戏脚本 / e=扩展 / s=静态引擎源。**行为零变化**：OUTPUT 视图内容/断言逐位照旧；SIGNALS 纯只读、页签切换不落 Output 行（零日志灌水 —— 环形缓冲既有断言面不动）。
4. **扩展注册名查证结论**：`ExtensionManager::signal_subscriptions()` 已存在（S17.2 订阅名快照，声明序去重）—— **无需**新增 `registered_signal_names()`；缺的是 `NesRuntime` 层的只读转发，补 `extension_signal_subscriptions() -> Vec<String>`（additive，未开扩展 = 空表）。
5. **门禁**：十 crate `cargo test --release` **792/792**（基线 787 + 新增 5：nes-scene 契约 T-SS-01..05）+ `--example editor_shell` 扫描单元测试 **5/5**（cargo 默认不跑 example 内 `#[cfg(test)]`，门禁显式加跑）+ clippy `--all-targets` **0 警告 ×10** + 依赖守卫 **15/15** + editor_shell 冒烟（120 帧干净退出；NES_EDIT_DEMO 420 帧全断言连跑 4 次稳定）+ first_game(Dodge) 240 帧干净退出。

## 1. 运行计数（nes-scene）

### 1.1 口径（冻结）

- **计什么**：`tick` 阶段 5 泵内**实际交付**。两处递增位与 `TickStats::signals_delivered` 的两处自增一一对应：
  - **广播交付**（观察者 `on_signal`，无目标上下文）：一条信号计 1；
  - **路由交付**（订阅册，每条命中连接一次；方法级分发不经观察者同口径）：每条连接各计 1 —— 一条信号两条连接 = 计 2；
  - **级联**：处理器再发射的信号回同泵继续交付，照实各计（a→b→c 级联 = a/b/c 各 1）。
- **不计什么**：订阅过滤未命中（`signals_filtered`，NoObserver 口径）、超 `SIGNAL_DELIVERY_CAP` 丢弃、`ProcessMode` 门控跳过（`handlers_skipped`）、方法级连接目标未注册处理器（静默跳过）—— 四者都不发生交付，与 `signals_delivered` 同判据。
- **计数时机在交付不在发射**：宿主 `emit_signal` 预发后未 tick = 不计数（T-SS-01 钉死）。
- **无清空口**：诊断面只增；宿主重开场景即换新树（RESET 是数据面还原，不动此表 —— 编辑器 demo 的计数跨 RESET 存活，正是 SIGNALS 视图"运行后回看"的用法）。

### 1.2 读面

`pub fn signal_stats_sorted(&self) -> Vec<(String, u64)>`：计数降序、同计数按名字典序（排序稳定 = SIGNALS 面板行序可另排字典序仍确定）。SIGNALS 视图消费时重排为名字典序（§3）。

### 1.3 指纹采样面查证（不进指纹的依据）

`determinism.rs::scene_fingerprint` 采样面 = 帧号 / 暂停位 / time_scale / 前序逐节点（uid/名/类型/父 uid/生命周期位/process_mode/本地变换位形/属性表/脚本局部）+ 补间登记表（条件混入）。`signal_stats` 不在其中 —— 与 `played_sounds`/`video_cmds` 取走缓冲同一条"副作用通道不是状态"纪律（tree.rs 字段注就地写明）。**契约验证**（T-SS-05）：同一场景经 `to_doc -> instantiate_doc` 双实例化（uid 随文档往返、两跑身份同源），同轨迹逐帧 tick —— 跑 A 全收（计数表累积 5 条）、跑 B NoObserver 全滤（计数表恒空），逐帧指纹**逐位相同**。差异的只有计数表本身，即计数不影响指纹。

> 查证过程备注：两棵独立 `SceneTree::new` 实例的指纹天然不同（根节点 uid 是 `Uid::new_v4()` 随机生成，S9-0 持久身份进指纹）—— 所以 T-SS-05 的双跑必须走文档往返同源实例化，不能各自 `new`。这是"同轨迹两跑"在本引擎口径下的正确形态。

## 2. 静态扫描（editor_shell 壳层）

### 2.1 正则面（手写扫描器，零新依赖）

| 形态 | 模式 | 说明 |
|---|---|---|
| `.nes` 发射 | `emit "name"` | 关键字 + 空白（可省）+ 双引号字面量 |
| `.nes` 订阅 | `on "name"` | 同上 |
| `.js` 发射 | `emitSignal("name")` | 括号后跳空白取双引号字面量 |
| `.js` 订阅 | `onSignal("name")` | 同上 |

- **词边界**：关键字前一字符非 `[A-Za-z0-9_]` —— `person "bob"` / `icon "a.png"` 的词内 `on` 不误报；`emitSignal(` 不会被 `emit` 抢走（js 形态先判，且 nes `emit` 形态要求后随空白+引号，`Signal(...` 不命中 —— 双保险）。
- **注释剔除**：逐行 trim 后 `//` 起 = 整行剔除。
- **去重 + 排序**：emits/ons 各自 dedup + 字典序（静态面只答"谁引用了谁"，计数只在运行时送达面）。

### 2.2 误报容忍（蓝图 Q3 如实标注）

**行尾注释不剔除**：`emit "x" // note` 仍命中（clippy `manual_strip` 修整后语义不变）。字符串字面量内容不做词法级跳过：源码里字符串/行尾注释中出现同形文本（如 `println("emit \"x\"")`）会误报命中。**裁决**：静态扫描是诊断面不是编译器，v1 不做字符串状态机 —— 误报如实记入，宁可多一行不少一行（面板行格式含 tags 供人眼判读）。整行注释（最常见形态）已剔除。

### 2.3 聚合（`scan_signal_index`）

信号名 -> `(bool g, bool e, u64 on)` 三源并表（BTreeMap = 行字典序的天然来源）：

1. **Script 节点（g）**：`source` 属性内嵌文本优先（S6.31）；否则 `registry_key` 非空按资产根相对路径读文件（与运行时装载同一相对系）；两者皆空 = 未挂载跳过；读盘失败如实跳过（带病也能跑口径）。
2. **Extensions/*.js（e）**：字典序 = 宿主装载序同源。
3. **扩展运行时注册名（e）**：`NesRuntime::extension_signal_subscriptions()`（本里程碑 additive 只读转发，见 §0.4）；静态已计过的名字不重复加 on 计数（防双计），静态未命中的订阅名补 `on:1`（订阅即听者）。

重扫周期照 `scan_assets` 先例：**每 60 帧**（`SCRIPT_SCAN_EVERY`）+ F8 手动顺带（均不落 Output 行）。`emitted` 是树读面，**每帧现算**不吃缓存。

## 3. 页签 UI（editor_shell）

### 3.1 结构

- 页签行 = dock 标题行内（`DOCK_TITLE_H` 18px 满高）：`[OUTPUT]`(48px) `[SIGNALS]`(64px) 两枚 Button + 各自九宫格底板（工具栏按钮模式逐位同源：底板垫底、按钮 `fill_slot` 置空透明、字号 14）；x 起点 `TAB_BTN_X=72`（"Output" 标题文本右侧）。开在既有 `"dock"` 容器下 —— walk 整子树跳过、z=-80 同 dock、`over_ui` 护盾收录（压上不清选中）。
- 会话态 `dock_tab: u8`（0=OUTPUT 缺省 / 1=SIGNALS）：不进树、不落盘；切换经 UiVm `on_activate` -> `tool_clicks` 落账段翻位，**不落 Output 行**（EDITOR_LOG_KEEP=48 环形缓冲的既有断言面零扰动）。
- 标题文本随页签投影（OUTPUT 时与既有 `"Output"` 逐位同）；活动页签文本 `*` 后缀（工具栏开关同款口径）。

### 3.2 SIGNALS 视图

行格式（全 ASCII）：`<name>  emitted:N  on:<k>  <tags>`，name 字典序，行截宽照 OUTPUT 同一预算（`DOCK_LINE_CHARS`=40）。

- `emitted:N`：`signal_stats_sorted()` 实时读面（本视图重排字典序）；
- `on:<k>`：§2.3 静态聚合；
- `<tags>`：g/e 位来自静态聚合；**s = 仅运行时统计可见**（引擎自发信号：tree/* 桥信号、`tween_done`、input/* —— 这些没有脚本源，编辑态 NoObserver 订阅全滤不交付，故编辑期引擎信号如实不入表；PLAY 期 ScriptVm 缺省全收才计数）。
- 行面按 `dock_tab` 每帧分派：OUTPUT = 既有日志行逐位不动（**行为零变化的回归锚**）。

### 3.3 demo 钩子断言（NES_EDIT_DEMO）

夹具：`spin.nes` 的 every 块加一行 `emit "spun" 1`（spin.nes 仅 editor_shell 使用，位置断言不受影响）。注入流尾部（既有链路全部收尾后）：帧 360-364 点 SIGNALS 页签（(156,321) —— 首版误点 OUTPUT 位的 (96,321) 已修），闩锁窗 366..=378 取 SIGNALS 行面；帧 380-384 点回 OUTPUT。断言：SIGNALS 行含 `spun`（PLAY 期 ScriptVm 广播交付逐帧计数，emitted ≈ 60 —— 真实送达计数链路）+ 行格式两字段；切回 OUTPUT 后 `OUTPUT*` 活动标记闩锁。既有断言（挂载/play/stop/reset/折叠/刷新/时间轴/IME/音乐/字体/菜单/S19.2 三分区）全部保持。

## 4. 门禁

| 项 | 结果 |
|---|---|
| 十 crate `cargo test --release` | **792/792**（= 基线 787 + 新增 5；逐 crate：asset 34 / audio 52 / ext-api 7 / ext-js 29 / media 27 / render-api 48 / render-extract 65 / render-wgpu 143 / runtime 116 / scene 271） |
| `cargo test --release --example editor_shell`（扫描单元测试） | **5/5**（cargo 默认不跑 example 内 `#[cfg(test)]` —— 已实测验证 —— 门禁显式加跑；已实证 plain `cargo test` 对 example 单测零执行） |
| clippy（`--all-targets`，十 crate） | **0 警告 ×10**（修整：扫描器 `strip_prefix` 化 + 模块头 doc list 续行重排） |
| 依赖方向守卫 `check_dependency_direction.py` | **15/15** |
| editor_shell 冒烟 | 120 帧干净退出；`NES_EDIT_DEMO=1 NES_EDIT_FRAMES=420` 全断言通过，**连跑 4 次**稳定 |
| first_game(Dodge) 冒烟 | `NES_GAME_FRAMES=240` 干净退出 |

## 5. 遗留

1. **载荷预览**：SIGNALS 行只有名字/计数，载荷（Value 形态）不展示 —— 需要载荷采样面（最新载荷快照属会话态）归后续里程碑。
2. **信号注入调试**：从面板手工发射一条信号到选中节点（编辑器侧 `emit_signal`）—— 写路径，涉及"编辑器发起的运行期改动"语义裁决，未做。
3. **过滤框**：信号名过滤输入框未做（行数大时需要）；行截宽 40 字符沿用 OUTPUT 预算，超宽截断。
4. **行尾注释误报**：§2.2 容忍口径 —— 词法级字符串跳过归后续（若实战误报扰人再做）。
5. **s 标签细化**：当前"仅运行时统计可见"即引擎源；将来引擎自发信号名单膨胀（如 S13 输入信号族）可再分 `e`（引擎）/`s` 分列。
