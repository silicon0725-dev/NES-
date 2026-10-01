# NES 2.0 · S6.19 脚本 VM v1

> 交付日期：2026-10-01　｜　状态：**手写栈式字节码解释器落地（registry_key 挂载点接通）**
> 前置：S6.18 方法级分发（处理器表 = 解释器闭包的落点）。`Script` 节点
> 从 M1 预留至今的挂载点正式启用。

---

## 0. 一句话结论

新增 `nes-scene::script` 模块：[`Op`]（18 条栈式指令）+ [`ScriptVm`]
（注册表/每脚本持久局部/装载器）。`Script` 节点的 `registry_key` 属性
引用注册表键，`attach_all` 批量装载。**双入口全走既有 substrate**：
信号入口 = 处理器表闭包 + 方法连接（跨节点，SignalCtx）；process 入口 =
VM 自身实现 SceneObserver（只能操作自身，NodeCtx 纪律）。三重停机保护
（类型/栈/寻址错误写 `__halt`、`Jump` 死循环步数上限、缺属性回落）。
出口准则 T-VM-01..05 + T-Script-R1（脚本驱动精灵到**像素**）全过。
全仓测试 **130** / 34 / 42 / 40 / 83 / **27** 全绿，守卫 11/11，
clippy 零警告。

---

## 1. 语义与裁决

### 1.1 零第三方纪律下的 VM 形态

不引入外部语言运行时 —— 手写**栈式字节码解释器**。脚本 = 指令序列
（宿主侧构造/将来的编译器产出），不是文本语言（文本语法属后续里程碑，
解析器纪律与 RON 手写解析器同源）。Scratch 广播/可视化脚本/JS 扩展
（M5 兼容层）将来编译成 `Op` 或在处理器表注册解释器闭包 —— **机制不再
新增**。

### 1.2 双入口（substrate 复用，无第二套分发）

| 入口 | 挂载 | 上下文 | 权限 |
|---|---|---|---|
| `ScriptEntry::Signal(name)` | `set_signal_handler(node, "run", 解释闭包)` + `connect_signal_to(name, None, node, "run")`（S6.18） | `SignalCtx` | **跨节点**读写（引擎级） |
| `ScriptEntry::Process` | VM 实现 `SceneObserver`，attached 节点的 process 驱动 | `NodeCtx` | **只能操作自身**（回调纪律不可绕，T-VM-05 实证：跨节点写 -> 停机） |

跨节点的正道：process 脚本 `Emit` -> 信号脚本干活。

### 1.3 泵语义修正（S6.16 缺口的实证暴露）

T-VM-01 首跑暴露：S6.16 的订阅过滤把**订阅册路由**也挡了 —— NoObserver
宿主（脚本场景的常态：连接即全部行为）下显式接线静默失效。修正：
`signal_filter` 只管**广播**交付；连接（显式接线）不受观察者订阅声明
影响。S6.16 的"未命中不进处理器"收敛为"不进**广播**"。

### 1.4 停机保护（三重，全不崩帧）

- 类型/栈/节点寻址错误：停机 + 原因写局部 `__halt`（`ScriptVm::locals`
  可观测）；
- `SCRIPT_MAX_STEPS = 10_000`：`Jump` 自环不挂帧（T-VM-03 实证）；
- 缺属性：`GetProp` 回落 `I64(0)`（与回调静默口径同家法）。

### 1.5 栈上的节点（不编进 Value）

栈元素 `StackVal = V(Value) | N(NodeId)` —— 节点原生携带代际。编进
`Value`（如 Resource 槽位）会丢代际，是身份谎言（S6.15 桥信号同裁决）。
**栈序约定**：`GetT`/`GetProp` 消费节点 —— 常见模式是节点压**两次**
（一次留给自己消费、一次留给 `SetT`/`SetProp` 回收），测试实证过写反
的后果（栈下溢/停机），指令文档已注明。

### 1.6 状态与重挂载

- 每脚本局部**跨调用持久**（计数器/累积器语义，T-VM-02 帧间计数实证）；
  闭包与观察者路径共享同一份（`Rc<RefCell<..>>`）；
- 重挂载 = 复位为脚本初始局部 + 清 `__halt` + 替换处理器/去重 process 表。

## 2. API 面

| 成员 | 职责 |
|---|---|
| `Op`（18 指令） | Const/Local/SetLocal/Arg/This/NodeByName/GetProp/SetProp/GetT/SetT/Add/Sub/Mul/Lt/Eq/Jump/JumpIfNot/Emit |
| `ScriptEntry` / `Script` | 入口（Process/Signal）+ 指令 + 初始局部 |
| `ScriptVm::register` / `attach` / `attach_all` | 登记 / 单点装载（registry_key）/ 批量装载（缺口清单如实上报，不挡好键） |
| `ScriptVm::locals` | 局部视图（含 `__halt` —— 可观测性） |
| `impl SceneObserver for ScriptVm` | process 入口驱动 |
| `StackVal` / `SCRIPT_MAX_STEPS` / `HALT_LOCAL` | 栈元素 / 步数上限 / 停机局部名 |

nes-runtime 零改动（`frame_with(&mut vm)` 直接以 VM 为观察者）。

## 3. 出口准则

### 3.1 场景层（`nes-scene/tests/s6_script.rs`，5/5）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-VM-01 | 信号脚本跨节点：命中即运行，SetT 移动其他节点**同帧**生效（local+world）；载荷经 Arg 入栈；重复信号累计 | ✅ |
| T-VM-02 | process 脚本：逐帧驱动；局部跨调用持久（n=2 后第 3 帧分支）；条件跳转 + Emit 入泵 | ✅ |
| T-VM-03 | 停机三例：节点找不到 / Jump 自环步数上限 / Add 类型错 —— 全部 `__halt` 可观测、不崩帧 | ✅ |
| T-VM-04 | attach_all：好键装载工作、坏键/空键进缺口清单；初始局部注入 | ✅ |
| T-VM-05 | process 纪律：跨节点写 -> 停机记录（NodeCtx 纪律不可绕） | ✅ |

### 3.2 运行时端到端（`nes-runtime/tests/criterion_script.rs`，1/1）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Script-R1 | 场景内 Script 节点（registry_key）-> attach_all 装载 -> 宿主每帧预发 -> 脚本平移精灵 -> **像素逐帧右移**（(10,10)->(26,10)->(42,10)）；零停机零驱动错误 | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 文本脚本语法 + 编译到 `Op`（手写解析器纪律） | 未启动 |
| 脚本序列化进场景文件（RON 的 Op 编码） | 未启动 |
| `DUP`/`Swap` 等栈操作指令；字符串/比较族扩展 | 未启动（18 条最小集） |
| process 入口的 Arg=delta（现为 0 占位 —— NodeCtx 不携带 delta） | 待 NodeCtx 携带或改入口签名 |
| 观察者复合（宿主观察者 + VM 并存；`Observers` 转发器） | 未启动 |
| WM_SIZE / DPI / headless Linux | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 130**（125 -> 130，+T-VM 5）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  **nes-runtime 27**（26 -> 27，+T-Script-R1）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（新 `script` 模块 + 泵过滤语义修正）与新测试；
  nes-runtime 零改动（新测试除外）。

*（内容由AI生成，仅供参考）*
