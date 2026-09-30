# NES 2.0 · S6.5 序列化口径：ProcessMode v1

> 交付日期：2026-10-01　｜　状态：**开放裁决已拍板并落地（NodeDoc 一等字段）**
> 前置：S6.4 暂停与时间缩放（其 §1.3 留下的开放裁决）。
> 本轮很小但口径要紧：`process_mode` 如何进场景文件。

---

## 0. 一句话结论

裁决：**`NodeDoc` 一等字段**（`NodeData.process_mode` ↔ `NodeDoc.process_mode`
对称，与 `local` 变换同一裁决的延伸）。缺省 `Inherit` 不写出（存量文件逐
字节不变），枚举以稳定字符串名编码（`"Always"` / `"WhenPaused"` …），
未知值**报语义错误**（不静默回落）。属性表方案否决。出口准则 T-PM-01..04
（场景层）+ T-Pause-R3（运行时端到端：磁盘场景携带 `Always` 的精灵，加载后
暂停期间照常被行为回调驱动）全过。全仓测试 **87** / 34 / 42 / 40 / 83 /
**15** 全绿，守卫 11/11，clippy 零警告。

---

## 1. 裁决记录

### 1.1 为什么是一等字段

1. **对称性**：S6.4 已裁决 `process_mode` 是 `NodeData` 的固有调度字段
   （先例：M2 的 `local`——"固有的一等字段，属于空间数据，不进属性表"）。
   存储与序列化同构（`NodeData.local` ↔ `NodeDoc.local` 已是先例），
   `process_mode` 跟随，不产生第二种记忆。
2. **类型安全**：枚举直存，免去 enum 进 `Value` 的两难 —— Str 编码松校验
   （schema 对字符串无闭集校验，拼错静默通过）或 I64 编码不可读。
   未知值的处理权收归解析层：语义错误、指名原值（T-PM-04）。
3. **性能口径**：调度数据每帧每节点都要读（含父链解析），不值得为
   序列化把它搬进属性表再让 tick 付 Value 解析的账。

### 1.2 为什么不是属性表

属性表方案的唯一优势是编辑器面板顺带获得 —— 但编辑器数据模型可以为一等
字段开面板（`local` 变换同样要进编辑器，并未因此入属性表）。为面板便利
破坏存储对称性，不划算。**M2 的"一切设计时数据都在 PropStore"教义维持
原意**：process_mode 是调度数据，与空间数据同级，本就在教义的边界之外。

### 1.3 前向兼容口径

- **缺省不写出**：`compact` 模式下 `Inherit` 省略（与 `local == 单位变换`
  省略同口径）；`verbose` 全量写出。存量文件（S6.4 之前）解析为 Inherit，
  逐字节不变（T-PM-02/03）。
- **未知值不容忍**：与"属性表容忍未知属性"的前向兼容规则**刻意不同** ——
  未知属性是扩展数据（丢了可惜），未知调度模式是引擎语义（静默回落成
  Inherit 会让"暂停菜单失灵"变成无声 bug）。新增模式须随 `FORMAT_VERSION`
  门槛一起走。

## 2. 实现（全部在 nes-scene）

| 位置 | 改动 |
|---|---|
| `tree.rs` | `ProcessMode::as_str` / `from_str_exact`（稳定名，不得随重构改名） |
| `scene_io.rs` `NodeDoc` | `process_mode: ProcessMode` 字段 |
| `scene_io.rs` 写出 | `node_to_doc` 填充；`write_node` 在非 Inherit（或 verbose）时写 `process_mode: "..."` |
| `scene_io.rs` 解析 | 节点字段 `process_mode` 可缺（缺省 Inherit），接受带引号/裸标识符两种写法（与 `kind` 同口径），未知值语义报错 |
| `scene_io.rs` 实例化 | `build_tree` / `build_child` 对根与每个子节点 `set_process_mode` |

运行时（nes-runtime）零改动：`save_scene` / `load_scene` 经
`to_doc_with_resources` / `instantiate_doc_with_resources` 自动携带。

## 3. 出口准则

### 3.1 场景层（`nes-scene/tests/s6_process.rs` 追加，4/4）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-PM-01 | 往返：树上设置的模式经 写出->解析->实例化 原样复原；Inherit 子的生效模式继承自复原后的父 | ✅ |
| T-PM-02 | 存量兼容：不含字段的场景解析为 Inherit | ✅ |
| T-PM-03 | compact 省略缺省、必写非缺省；verbose 全写 | ✅ |
| T-PM-04 | 未知值（`"Sometimes"`）语义报错且指名原值 | ✅ |

### 3.2 运行时端到端（`nes-runtime/tests/criterion_pause.rs` 追加，1/1）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Pause-R3 | 磁盘场景 `process_mode: "Always"` 的精灵：加载后模式复原、暂停期间照常被行为回调驱动（像素继续移动）、渲染照常 | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 编辑器面板为一等字段（local / process_mode）的展示通道 | 未启动（编辑器侧） |
| 信号总线（SignalBus） | 未启动 |
| 子场景嵌套 / WM_SIZE / DPI / headless Linux | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 87**（83 -> 87，+T-PM 4）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  **nes-runtime 15**（14 -> 15，+T-Pause-R3）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（tree.rs 稳定名 + scene_io.rs 一等字段）与新测试；
  nes-runtime 仅新测试。

*（内容由AI生成，仅供参考）*
