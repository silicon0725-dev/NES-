# NES 2.0 · S7.2 输入系统（Input System）v1

> 交付日期：2026-10-01　｜　状态：**平台输入 → 帧输入快照的四层管线落地；Keyboard / Mouse / TextInput / Window 四路分开；WM_CHAR 从引擎 API 退役**
> 前置：S7.0 收束（队列有界纪律）；S7.1 运行时语义冻结（宿主预发信号 + 注入纪律）。

---

## 0. 一句话结论

输入从"全局 WM_CHAR 字符队列"升格为四层管线：**平台层**（Win32
消息 → 中性 `InputEvent`，VK→`Key` 映射、容量 1024）、**契约层**
（nes-render-api `input` 模块：`InputCollector` 事件流 → 帧快照，
**闩锁边缘**语义）、**运行时层**（`collect_input` 折叠 +
`emit_input_signals` 标准 `input/*` 信号 + `mount_key_probe` 探针）、
**消费层**（事件式脚本 `on "input/key_down"` / 轮询式 `key("W")` /
宿主直接读快照）。`drain_chars`/`inject_char` **整体退役** ——
WM_CHAR 不再是引擎 API。实证修正三处：KEYUP 幻影字符（泵只翻译
按下族）、同帧脉冲不可见（集合差 → 闩锁）、首帧鼠标增量伪影（首次
观测只建基准）。新增 T-In-C01..03 / T-In-VM-01..02 / T-In-01..02（重写）/
T-In-R1..02。全仓测试 **415**（34 / 174 / 43 / 42 / 88 / 34）全绿，
守卫 11/11，clippy 零警告。

---

## 1. 分层与依赖

```text
Platform Input（nes-render-wgpu/window.rs：WM_* → InputEvent，进程级队列）
      ↓ drain_input / inject_input（容量 1024，满丢新保旧）
Input Collector（nes-render-api/input.rs：纯状态机）
      ↓ collect_input()（运行时字段折叠器，每帧一次）
Frame Input Snapshot（InputSnapshot：held/pressed/released/mouse/text/resized）
      ↓ 三条消费路（互不排斥）
   ├─ emit_input_signals：边缘 → 标准 input/* 信号（宿主预发纪律，S7.1）
   ├─ mount_key_probe：按住态 → 脚本 key("名") 探针（共享槽，装一次）
   └─ 宿主直接读快照（编辑器面板读 text 字段）
      ↓
Script / UI / Game
```

四路**分开**（一个事件族一个通道，不互相挤占）：

| 路 | 事件 | 快照字段 | 信号 |
|---|---|---|---|
| Keyboard | WM_KEYDOWN/UP | `held` / `pressed` / `released` | `input/key_down` / `input/key_up`（载荷 Str 键名） |
| Mouse | WM_MOUSEMOVE + 三键 | `mouse` / `mouse_delta` / `buttons_*` | `input/mouse_move`（Vec2 位置）/ `input/mouse_down` / `input/mouse_up`（Str "left"/"right"/"middle"） |
| TextInput | WM_CHAR（合成） | `text: Vec<u32>` | `input/text`（Str 整帧文本） |
| Window | WM_SIZE | `resized: Option<(u32,u32)>` | `input/window/resized`（Vec2） |

**WM_CHAR 不是引擎 API**：字符码到平台层为止（折成 `InputEvent::Char`），
宿主与脚本消费快照 `text` 字段。旧 `drain_chars`/`inject_char` 删除
（S6.34 的 T-In-01 随之重写为事件口径）。

## 2. 语义与裁决

### 2.1 边缘 = 闩锁，不是集合差（T-In-R1 实证修正）

初版用 `curr − prev` 算 pressed —— **同帧按下又抬起（快点击/注入节奏
< 一帧）在 held 和边缘里都不可见**，低帧率下真实点击会凭空消失。修正：

- `pressed` 在 down 事件上**闩锁**（重发的 down —— 键已按住 —— 不重复
  闩锁，自动重发天然幂等）；
- `released` 只记**上帧末按住**的键的抬起（凭空 up 不算）；
- 同帧脉冲 = `pressed=true, released=false, held=false`（干净单帧）；
- `held` 是**帧末状态**。

### 2.2 鼠标增量：首次观测只建基准

窗口收到的第一个移动事件前鼠标"本来就在那"—— 首帧 delta 报 (0,0)
（跳变是伪影，不是输入）。此后 delta = 本帧位置 − 上帧位置；无移动
事件的帧 delta = (0,0)。

### 2.3 键位口径

- 中性 `Key` 枚举（字母/数字/方向/编辑键/修饰），名字口径与
  `from_name` 反解共用（信号载荷、脚本 `key("名")`、探针同一张表）；
  未列举键保留原码（`Other`），`from_name` 不解析 → 探针返回假
  （**未列举 = 未按，不猜**）；
- Win32 `WM_KEYDOWN` 缺省不分左右修饰（`VK_SHIFT` 一个码）—— 统一记
  左变体（`LShift` 等），需要区分走 raw input / scancode（后续）。

### 2.4 平台层两条实证修正

1. **KEYUP 幻影字符**：`TranslateMessage` 对带异常 lparam 的 KEYUP
   （注入/远控工具形态）也会合成 WM_CHAR —— 真实按键会双字符。泵改为
   **只对按下族（WM_KEYDOWN/WM_SYSKEYDOWN）翻译**（游戏循环惯用法）。
   正常形态的 KEYUP（带转换位标志）本就不合成（实证）。
2. **事件不吞**：`wnd_proc` 记录事件后**仍走 DefWindowProcW**（输入
   消息记录与窗口默认处理不互斥，保守口径）。

### 2.5 消费双路（事件式 / 轮询式）

- **事件式**：`emit_input_signals`（宿主在 `frame*` 前调用 —— S7.1
  黄金帧序的宿主预发位）把边缘发成 `input/*` 信号；脚本
  `on "input/key_down" { if arg == "W" { ... } }`（`arg` = 键名 Str）。
  同帧脉冲**有** key_down、**无** key_up（§2.1 口径的自然结果）。
- **轮询式**：`mount_key_probe(&mut vm)` 装一次（共享槽 —— 之后每帧
  `collect_input` 自动刷新读数）；脚本 `key("ArrowRight")` → Bool。
  VM 不碰平台（与文件读取器同一注入纪律）；**未接探针时 `key(..)`
  停机**（`__halt` 指名，不装"恒假"—— 没接就是没有）。

### 2.6 headless / 自动化

`inject_input` 与真实消息同队列；离屏运行时 `collect_input` 照常折叠
（无消息即空快照）—— 输入可合成、可回放，是 S7.3 headless 的前置件。

## 3. 环境备注（如实记录）

本轮窗口**视觉**验证不可用：本会话的 DWM 合成已退化（`PrintWindow`
对 flip-model 表面返回纯白；屏幕拷贝同）—— **上一已知良好提交
（c680405，本轮前用同法截到正确内容）同样全白**，证明是环境不是回归。
地面真值以**离屏像素测试**为准（criterion_panel / criterion_input 全
绿，同一提取/消费路径），窗口路径的视觉验收留到环境恢复后补做。

## 4. 实现落点

| 位置 | 改动 |
|---|---|
| `nes-render-api/src/input.rs` | `Key`/`MouseButton`/`InputEvent`/`InputSnapshot`/`InputCollector`（纯数据层） |
| `nes-render-wgpu/src/window.rs` | `EVENTS` 队列（替换 TYPED，容量 1024）；`vk_to_key`；`input_event_of`；泵只翻译按下族 |
| `nes-scene/src/script.rs` | `Op::Key`（探针求值，未接停机）；`key(name)` 文法；`ScriptVm::set_key_probe`（共享槽） |
| `nes-runtime/src/lib.rs` | `collect_input` / `emit_input_signals` / `mount_key_probe`（+ 折叠器与共享快照字段） |
| `nes-runtime/examples/script_panel.rs` | 面板消费快照 `text` 字段（WM_CHAR 路径退役） |

## 5. 出口准则

| 编号 | 契约 | 结果 |
|---|---|---|
| T-In-C01 | 键边缘闩锁：重发幂等、同帧脉冲可见（pressed 有 / released 无 / held 无）、跨帧抬边 | ✅ |
| T-In-C02 | 鼠标：位置覆盖、首帧建基准 delta=0、按钮三态 | ✅ |
| T-In-C03 | text/resized 一次性（取走即清）；键名/按钮名 round-trip；`is_down` 未列举名 = 假 | ✅ |
| T-In-01（重写） | 真实消息路径：PostMessageW 键/鼠标/尺寸 → 中性事件按序入队；**KEYDOWN 经泵合成字符**（真实打字路径）；KEYUP 不再合成字符 | ✅ |
| T-In-02 | 注入与真实消息同队列；容量 1024 丢新保旧；VK→Key 映射表（含左右修饰统一记左、未列举保原码） | ✅ |
| T-In-VM-01 | `key(..)` 编译为 `Op::Key`；未接探针停机指名（不装恒假） | ✅ |
| T-In-VM-02 | 探针求值驱动脚本；attach 后注入立即生效（共享槽）；未列举键不触发 | ✅ |
| T-In-R1 | 运行时端到端：注入事件 → `collect_input` 快照（边缘/按住/text/resize/首帧基准/一次性清）→ `emit_input_signals` 恰 6 条 → 脚本 `on "input/key_down"` 载荷过滤驱动 | ✅ |
| T-In-R2 | 探针全链：按住两帧持续驱动、松开即停（队列→折叠→共享快照→探针→VM） | ✅ |

## 6. 遗留与后续

| 事项 | 状态 |
|---|---|
| 窗口路径视觉验收（本会话 DWM 退化，环境恢复后补） | 待环境 |
| 鼠标滚轮 / 附加按钮（XBUTTON）/ 键盘扩展（F 键、小键盘、OEM） | 未启动（`Other` 保原码通路已备） |
| UTF-16 代理对重组（BMP 外字符的 text） | 未启动（`Vec<u32>` 原码口径已兼容） |
| IME 组合态（composition 中间态 vs 提交文本） | 未启动（当前只有提交字符） |
| 左右修饰区分（raw input / scancode 路径） | 未启动（统一记左已文档化） |
| 鼠标捕获（capture）/ 拖出窗口 / 多显示器负坐标 | 部分口径已备（i16 解包）；捕获未启动 |
| S7.3 Headless Runtime（`nes --headless scene.nes --frames N` + 状态哈希确定性） | 下一里程碑（输入可合成已就绪） |

## 7. 记账

- 测试基线：nes-asset 34 / **nes-scene 174**（172 -> 174，+T-In-VM-01..02）/
  **nes-render-api 43**（40 -> 43，+T-In-C01..03）/ nes-render-extract 42 /
  **nes-render-wgpu 88**（87 -> 88，T-In-01 重写 + T-In-02）/ **nes-runtime
  34**（32 -> 34，+T-In-R1..02）—— 全绿，合计 **415**；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 破坏性变更：`drain_chars`/`inject_char` 删除（唯一消费者面板/测试
  全部迁移到 `collect_input().text` / `inject_input`）。

*（内容由AI生成，仅供参考）*
