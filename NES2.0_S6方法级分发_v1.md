# NES 2.0 · S6.18 方法级分发 v1

> 交付日期：2026-10-01　｜　状态：**节点处理器表落地（草案 connect 的 method 位补全）**
> 前置：S6.17 订阅册（路由层）。`connect(src, sig, dst, method)` 至此
> 四个参数全部可表达。

---

## 0. 一句话结论

`SceneTree` 新增**节点处理器表**：`set_signal_handler(node, method, 闭包)`
把处理逻辑挂到具体节点；`connect_signal_to(名字, 源, 目标, 方法)` 的连接
命中时**引擎直接调用闭包**（dst 上下文、载荷可读、可再发射、可下 Cmd），
不经观察者分支。未注册处理器的方法连接静默跳过；同名替换后者生效；
节点销毁时处理器表随订阅册一同修剪。出口准则 T-Sig-17..19 全过。全仓
测试 **125** / 34 / 42 / 40 / 83 / **26** 全绿，守卫 11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 落点：Rust 闭包，脚本 VM 的 substrate

草案 `connect(..., method: InternedString)` 的方法分发以脚本 VM 为前提。
本轮的处理器表用**Rust 闭包**填这个位：`SignalHandler =
Box<dyn FnMut(&mut SignalCtx, &Signal)>` —— 与观察者回调同一形状。推论：
脚本 VM 将来在这里注册解释器闭包（`Script` 节点的 `registry_key` 语义
直接承接），**无需第二套机制**；行为代码也可以现在就把逻辑挂到节点上，
观察者不再按 dst 分支。

### 1.2 双路分发（一张册，两种交付）

连接的 `method` 字段决定交付路：
- `None`（`connect_signal`，S6.17）-> 观察者交付（dst 上下文）；
- `Some(m)`（`connect_signal_to`）-> 引擎查目标节点处理器表并调用，
  **观察者不收到该次路由**（T-Sig-17 断言）。

两种连接可在同一册并存、各交付各的（T-Sig-18）。路由交付一律计入
`signals_routed` 并同守 `SIGNAL_DELIVERY_CAP`。

### 1.3 未注册处理器 = 接线期缺口，静默跳过

方法连接的目标节点上没有该方法 -> 该连接跳过（不调用、不计数、不崩帧），
注册后即通。与 `NodeCtx::set_prop` 静默口径同一家法：接线期错误不崩帧，
想可见就在宿主侧对账（册视图 + 注册时序）。不为它发明报警通道。

### 1.4 别名问题的 take/put 模式（实现要点）

处理器存于树内（`HashMap<NodeId, HashMap<String, SignalHandler>>`），调用
时 ctx 需要只读树借用 —— 直接 `get_mut` 闭包与 `&*self` 冲突。解法：
**take（暂离表）-> 调用 -> put（归还）**，Box 移动廉价；处理器在表外期间
树可借出只读引用。处理器表不进 `NodeData`（闭包不可 Clone/Debug/PartialEq，
树数据保持可比较）。

## 2. API 面

| 成员 | 职责 |
|---|---|
| `SignalHandler`（类型别名） | `Box<dyn FnMut(&mut SignalCtx, &Signal)>` |
| `SceneTree::set_signal_handler` | 注册/替换（同名后者生效；节点须存在） |
| `SceneTree::remove_signal_handler` | 显式移除（幂等） |
| `SignalConnection.method` + `connect_signal_to` | 方法级连接（四参 connect 补全） |
| 泵 | 双路分发 + 未注册跳过 + 销毁修剪覆盖处理器表 |

nes-runtime 零改动。

## 3. 出口准则（`s6_signal.rs` 追加，3/3）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Sig-17 | 方法级分发：闭包被调（dst 上下文、载荷可读、可再发射级联入泵）；观察者不收该次路由 | ✅ |
| T-Sig-18 | 未注册处理器静默跳过（无路由计数）；补注册即通；方法连接与观察者连接并存各交付 | ✅ |
| T-Sig-19 | 同名替换后者生效；显式移除幂等；节点销毁 -> 册与处理器表清理、再发射无路由不崩 | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 脚本 VM 解释器闭包注册（`registry_key` -> 处理器表装载） | 未启动（落点已备，§1.1） |
| 处理器表只读视图（册视图的对应物，编辑器检视） | 未启动 |
| 行为代码侧 set_handler/connect（SignalCtx 目前只读） | 未启动 |
| WM_SIZE / DPI / headless Linux | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 125**（122 -> 125，+T-Sig-17..19）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  nes-runtime 26 —— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（处理器表/方法连接/泵双路/修剪扩展 + 导出）与新测试。

*（内容由AI生成，仅供参考）*
