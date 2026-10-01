# NES 2.0 · S6.16 订阅过滤 v1

> 交付日期：2026-10-01　｜　状态：**观察者声明式订阅（未命中不进处理器）**
> 前置：S6.14 信号总线、S6.15 信号桥。泵从"全量送 `on_signal`"升级为
> "只送命中订阅的"。

---

## 0. 一句话结论

`SceneObserver` 新增声明法 `signal_filter() -> SignalFilter`（`All` 缺省 /
`Select { names, prefixes }`），泵在交付前过滤：未命中的信号**不进处理器**
—— 不消耗交付上限、不触发级联，计入 `TickStats::signals_filtered`（对账
恒等式：`delivered + filtered + dropped == 总发射`）。`NoObserver` 缺省
`NONE`（无行为代码时泵只记账零回调）。出口准则 T-Sig-09..12 全过。全仓
测试 **118** / 34 / 42 / 40 / 83 / **26** 全绿，守卫 11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 "订阅"在本引擎的形态

**声明式、随观察者走、无注册表状态** —— 与 S6.14"无连接表"的消解口径
一脉相承：过滤条件是观察者自己的回答（`&self`，每帧取一次而非每信号），
不存在 `connect/disconnect` 的悬挂清理问题。节点-方法级的订阅册仍归
脚本 VM 里程碑（`Script` 节点挂载点）。

### 1.2 过滤的三个不（安全语义）

未命中的信号：
1. **不进处理器** —— `on_signal` 不被调用；
   （S6.19 修正：此处收敛为"不进**广播**"—— 订阅册连接是显式接线，
   不受观察者订阅声明影响，见 `NES2.0_S6脚本VM_v1.md` §1.3；）
2. **不消耗上限** —— `SIGNAL_DELIVERY_CAP` 只数真实处理器调用；
3. **不触发级联** —— 被滤信号不可能再发射（T-Sig-11：订阅 "x" 的处理器
   收 x 发 y，y 被滤即链断，交付停在 1）。

### 1.3 对账恒等式与记账

`delivered + filtered + dropped == 总发射`（dropped 仅在上限截断时非零，
此时余量整体计入 —— 含本会被过滤的项，文档口径：截断是对队列的截断）。
`NoObserver` 缺省 `NONE`：`frame()` 无观察者路径零回调开销，全部进
`signals_filtered` 记账（可观测性不丢）。

### 1.4 兼容

缺省 `All` —— 既有观察者（S6.14/15 的全部测试）行为不变，零迁移。

## 2. API 面

| 成员 | 职责 |
|---|---|
| `SignalFilter`（enum） | `All` / `Select { names, prefixes }`；`NONE` 常量 |
| `SignalFilter::names(..)` / `prefixes(..)` | 构造助手（如 `prefixes(&["tree/"])` 收全部桥信号） |
| `SignalFilter::matches(name)` | 命中判定（精确或前缀） |
| `SceneObserver::signal_filter(&self)` | 声明订阅（缺省 `All`；每帧取一次） |
| `TickStats::signals_filtered` | 过滤记账 |

nes-runtime 零改动。

## 3. 出口准则（`s6_signal.rs` 追加，4/4）

| 编号 | 契约 | 结果 |
|---|---|
| T-Sig-09 | 精确名订阅：只 "go" 进处理器；桥与未订阅用户信号被滤；对账恒等式成立 | ✅ |
| T-Sig-10 | 前缀订阅 `tree/`：只收桥信号，用户信号被滤 | ✅ |
| T-Sig-11 | 过滤切断级联：收 x 发 y，y 被滤即链断；被滤信号不耗上限 | ✅ |
| T-Sig-12 | `NoObserver` 缺省 NONE：零回调、全部过滤记账 | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 订阅册 connect/disconnect | ✅ S6.17 路由层（方法级分发仍归脚本 VM，见 `NES2.0_S6订阅册_v1.md`） |
| 多观察者分发（当前单观察者声明法，多行为宿主自行内部分发） | 未启动 |
| 通配/模式订阅（`*` 等） | 未启动（前缀已覆盖桥信号场景） |
| WM_SIZE / DPI / headless Linux | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 118**（114 -> 118，+T-Sig-09..12）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  nes-runtime 26 —— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（SignalFilter/声明法/泵过滤/统计 + 导出）与新测试。

*（内容由AI生成，仅供参考）*
