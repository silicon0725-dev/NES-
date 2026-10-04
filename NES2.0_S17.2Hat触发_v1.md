# NES 2.0 · S17.2 hat 触发（事件重入：扩展从"每帧轮询"升级为"事件处理器"）v1

## §0 结论

**S17.2 达成：JS 扩展有了 Scratch 语义的 hat —— 事件发生 -> 重入扩展代码，
双向桥全通，S17.1 三道防线全数继承。**

- 游戏脚本 `emit "enemy-died" 42` -> JS 扩展 `nes.onSignal("enemy-died", fn)`
  同帧收到（载荷 42 保真）；JS 扩展 `nes.emitSignal("x", v)` -> 游戏/Cmd
  侧收到（反向桥）。实现载体是**既有确定性信号总线**（S6 契约：FIFO 泵、
  级联、1024 上限、订阅过滤）—— 全部白得，零新机制。
- **裁决 A（in-tick 重入）落地**：hat 在信号泵内同步调用（与游戏观察者
  同一 tick 同一时序）—— hat 里的 `setPos` 当步落地、同帧可见（T-HAT-01
  实证）；**QuickJS 重入实测结论：序贯重入成立**（每次派发是独立的一次
  `rt.call`，hat 的发射经缓冲在两次派发之间取走，不存在嵌套 QuickJS 栈帧
  —— 深度 3 级联 T-HAT-03 钉住），裁决 B（延迟批量）无需启用。
- 查证结论两则：① nes-scene **已有** `Observers`（拥有式 Vec 组合、过滤
  并集）—— 'static 场合复用它，不重复造轮子；帧路径的"宿主借用观察者 +
  运行时内部扩展观察者"生命周期不齐，故补**借用形态** `TeeObserver`
  （additive，两副面孔同一裁决）。② ExtensionManager 对树**没有**直接
  通道（读快照/写队列桥）—— `emitSignal` 走同一款桥：新增
  `SignalCapability`（照 InputCapability 模式），树侧落地 = 泵内经
  `SignalCtx::emit` 同泵级联、update 期经 `SceneTree::emit_signal` 下帧泵。
- 十 crate `cargo test --release` 全绿（基线 695 + 新增 13 = **708**，
  其中 T-HAT-01..06 全过），clippy 0 ×10，守卫 15/15（零新依赖，G14/G15
  不动），ext_demo 冒烟终态与 S17.1 基线**逐位一致**（146.41873,
  147.01727 —— hello.js 无 hat，闸关闭，路径逐位同基线的直接证据）。

git：单提交（本文件随提交入库）；不 push。

---

## §1 设计（两个关键裁决的落地细节）

### 1.1 裁决 A：in-tick 重入（hat 在信号泵内同步调用）

```text
帧路径（唯一咽喉 NesRuntime::tick_tree）：
  有扩展订阅信号 hat？
  ├─ 否 -> tree.tick(delta, 宿主观察者)            —— 零开销，与 S17.1 逐位同基线
  └─ 是 -> TeeObserver::new(宿主观察者, ExtensionSignalObserver)
          -> tree.tick(delta, &mut tee)
              泵阶段 5：命中订阅的信号 -> tee.on_signal
                  1) 宿主观察者（游戏侧，先）
                  2) ExtensionSignalObserver（后）：
                     dispatch_signal -> 各扩展 __nes_signal_dispatch 蹦床
                     （S17.1 隔离继承：throw/死循环/超内存 = fault 计数，泵不炸）
                     取走 pending_emits -> ctx.emit -> 同泵级联（1024 上限内）
  每步 tick 后：apply_extension_ops（泵内 hat 写当步落地）
```

- **写树同帧落地**：hat 在泵内 `nes.node.setPos` 进既有写队列，
  `simulate` 每步 tick 后即 `apply_writes`（与 `consume_played_sounds`
  同一消费点家法）—— 同帧后续阶段与下一帧都看得见，无滞后；
  下一帧 `refresh` 不会把它当残留清掉。
- **零开销闸（T-HAT-06）**：`has_signal_hats()` = 能力桥订阅名表非空。
  hello.js 这类无 hat 扩展：闸关闭，不组装扩展观察者、泵不派发 ——
  ext_demo 冒烟终态逐位同 S17.1 基线即为实证。
- **订阅过滤**：扩展观察者的 `signal_filter` = 订阅名集（`Select`）；
  与宿主观察者的过滤取**并集**（`TeeObserver`/`Observers` 同一条裁决）
  —— 一侧的过滤器不能掐掉另一侧的邮件。未命中任何一侧的信号不进泵
  处理器（`signals_filtered` 记账，不耗上限）。
- **裁决 B 未启用**：原案"若 QuickJS 不支持重入，则 hat 调用改队到
  update 前批量执行"——实测无需：重入是**序贯**的（`rt.call` 返回后才
  drain 发射缓冲、才可能有下一次 `rt.call`），QuickJS 栈从不嵌套；
  rquickjs 的 `Rc<RefCell>` 能力闭包在序贯重入下无借用冲突（闭包端
  `try_borrow_mut`，冲突静默让路的既有口径不变）。

### 1.2 裁决落地的另一半：扩展 -> 树的 emit 通道（查证后选型）

- **查证**：ExtensionManager 对树没有 `&mut`（QuickJS 闭包 'static，
  桥模式 = 读快照 + 写队列）—— `emitSignal` 不能直接调 `tree.emit_signal`。
- **选型**：照 InputCapability 模式加 `SignalCapability` trait
  （nes-extension-api，additive；`on_signal(name)` = 订阅声明、
  `emit(name, payload)` = 反向发射，都取 `&mut self` 与写能力同一裁决）。
  引擎实现在 `ExtCapsState`（与四个既有能力同一家）：订阅名表 +
  待发射缓冲（提交序 = 落地序，容量 4096，超限丢弃并如实计数）。
- **两种落地时序（都成立、都有测试）**：
  - **泵内**（hat 重入期间发射）：派发后即刻取走 -> `SignalCtx::emit`
    -> **同泵级联**（SignalCtx 语义，受 1024 交付上限约束）；
  - **update 期**（onUpdate 里发射）：`update_extensions` 尾步取走 ->
    `SceneTree::emit_signal` -> **下一帧的泵**（宿主预发时序）；
    `refresh` 清残留缓冲（半途炸掉的帧不落地 —— 与写队列同一条防线）。
- **TeeObserver vs Observers（查证）**：`Observers` 已存在（拥有式、
  'static、过滤并集、派发序 = 注册序）—— 本期**复用其裁决**、新增
  `TeeObserver<'a>`（借用形态，`&mut dyn` × 2）：帧路径的宿主观察者是
  外部借用、扩展观察者是运行时内部借用，生命周期装不进 `Box<dyn>`。
  两者对引擎都是一个观察者（`TickStats` 不按成员数放大）。

### 1.3 载荷映射（NesValue <-> 树 Value，nes-runtime 侧实现）

- **出界**（树 -> JS）：`F32`/`I64` -> `F64`（JS 数值唯一形态）、
  `Bool`/`Str` 直映、`Vec2` -> `{x, y}` 对象、`Resource` 槽位号 ->
  不透明 number、`Node` 句柄 -> `Null`（**句柄不过界** —— 运行时身份
  不跨上下文，防悬垂与身份谎言）、`Array` 递归。
- **入界**（JS -> 树），与脚本字面量类型对齐（脚本整数走 I64、小数走
  F32）：`F64` 整数值（且在 i64 范围）-> `I64`、其余（含 NaN/Inf）->
  `F32`（f64->f32 精度收缩，如实文档）；`Null` -> `Bool(true)`（无载荷
  占位 —— 与树桥信号的占位约定同一口径；游戏脚本本就产不出 null，映射
  对脚本面不可见）；`Array` 递归；**`Object` 仅 `{x, y}` 数值对映射
  `Vec2`，其余对象 P0 拒绝**（返回 None，落地处丢弃并计数
  `dropped_signal_payloads` —— 树 Value 没有可落地的对象形态，宁可丢弃
  不编造语义）。
- 防爆面：订阅名容量 256（防订阅风暴）、待发射缓冲 4096（50ms 预算内
  发射风暴的损失上界）、级联 1024（S6 既有）—— 三层都是"如实截断 +
  计数"，不挂起帧循环。

### 1.4 帧序与统计口径（不变式）

- 泵交付统计不变：组合对引擎是一个观察者，`signals_delivered` 不放大；
- 派发序确定性：广播 -> tee（宿主先、扩展后）；扩展间按装载序；同名
  多 handler 按注册序（JS 侧数组）；跨扩展发射按提交序入泵 —— 全链
  无 HashSet/数值优先级（T-HAT-03 次序断言钉住）；
- hat 派发失败**不中断**派发循环：单扩展异常/中断后其余扩展照常收到
  同一条信号（T-HAT-04/05 实证）。

---

## §2 JS 面（全文，照抄实现）

新增两个入口（nes-extension-js `NES_BOOTSTRAP_JS`，JS 值全部留在 JS 堆，
Rust 侧零句柄 —— 与 onUpdate 同一形态纪律）：

```js
// 注册 hat：同名多个 handler = 都调，注册序。非函数 handler 抛错
//（装载期即暴露）。首个同名注册时经 __nes_signal_subscribe 向宿主
// 能力桥声明订阅一次（去重由 JS 侧保证，宿主兜底）。
nes.onSignal("enemy-died", function (payload) {
  // payload 已过 NesValue 边界：数字/字符串/布尔/数组/{x,y} 对象
  nes.emitSignal("hat-seen", payload);   // 反向发射（见下）
});

// 反向发射：扩展 -> 引擎信号队列。落地时序由宿主取走时机决定：
// hat 重入期间调用 = 同泵级联；onUpdate 里调用 = 下一帧的泵。
nes.emitSignal("player-scored", 100);
```

派发蹦床（每上下文一份，宿主经 `__nes_signal_dispatch(name, payload)`
进入）：

```js
globalThis.__nes_signal_dispatch = function (name, payload) {
  var list = globalThis.__nes_signal_handlers[name];
  if (list === undefined) { return 0; }        // 未订阅：零调用早退
  var called = 0; var firstError = null;
  for (var i = 0; i < list.length; i++) {
    try { list[i](payload); called = called + 1; }  // 逐 handler try/catch
    catch (e) { if (firstError === null) { firstError = e; } }
  }
  if (firstError !== null) { throw firstError; }     // 循环后重抛 -> fault
  return called;
};
```

设计要点：**逐 handler try/catch + 循环后重抛** —— 单个 handler 抛错不
殃及同表后续 handler（隔离粒度 = handler），错误文本仍沿标准 `rt.call`
错误路径浮出成 fault（S17.1 语义不变：异常必须可见、必须计数）。预算
中断（`"interrupted"`，uncatchable）直接浮出，同一条路径。

---

## §3 隔离继承（S17.1 三道防线在 hat 路径的语义）

| 防线 | hat 路径行为 | 测试 |
|---|---|---|
| 异常隔离 | handler throw -> 蹦床重抛 -> `ExtError::CallFailed` -> `record_failure`（累计 + 连续计数、`last_fault`、诊断消息）；**同表后续 handler 已跑、其余扩展照常派发、泵不炸** | T-HAT-04 |
| 死循环中断 | 50ms `EXEC_BUDGET`（每次 `rt.call` 重新武装 —— 每次派发一份预算）-> QuickJS 中断处理器 -> uncatchable 异常浮出；派发循环继续其余扩展；泵下一帧照常 | T-HAT-05 |
| 自动停用 | 与 update 共用同一套簿记（`record_failure`）：连续失败满 60 次停用该扩展（宣告一次）；成功一次即清连续计数 | （复用 S17.1 语义） |

- hat 故障的诊断面：`extension_faults()` / `last_fault()` / 新增
  `dropped_signal_payloads()`（发射缓冲超限 + 不可落地载荷）；泵内无
  消息通道，故障经诊断面可见、不重复冒泡。
- 无订阅零开销是**结构性**的：闸关闭时扩展观察者根本不组装
  （`tick_tree` 分支），非"组装了再空转"。

---

## §4 门禁（全部实测）

- 十 crate `cargo test --release` 全绿：nes-asset 34、nes-audio 52、
  nes-extension-api 7、nes-extension-js 12、nes-media 27、
  nes-render-api 45、nes-render-extract 56、nes-render-wgpu 123、
  nes-scene 247、nes-runtime 105 = **708 passed / 0 failed**
  （基线 695 + 新增 13：nes-scene TeeObserver 3、nes-extension-api
  SignalCapability 1、nes-extension-js hat 绑定 2、nes-runtime
  T-HAT-01..06 + 载荷映射 7）。
- clippy 0 ×10（全 crate `--all-targets`）。
- 守卫 `check_dependency_direction.py` **15/15**（零新依赖 —— G14/G15
  白名单未动；nes-extension-api 仍零依赖，nes-extension-js 仍只有
  rquickjs 家族 + nes-extension-api）。
- ext_demo 冒烟：headless 与窗口 300 帧全过，终态 **(146.41873,
  147.01727)** 与 S17.1 基线逐位一致（无 hat = 零开销路径的实证），
  JS 异常帧 0。
- editor_shell 冒烟：`NES_GAME_FRAMES=120` 正常退出（exit 0）。
  附注：`NES_EDIT_DEMO=1` 自动化钩子的 FileSystem 双击断言在本机
  **基线提交上同样失败**（`git stash` 后实测复现 —— 资产目录多出的
  blink.nes 改变了文件面板行序，自动化双击落错行），与本期改动无关
  （editor_shell 不装载扩展，tick 路径未变）。
- 契约测试落点：`nes-runtime/tests/s17_2_hats.rs`（T-HAT-01..06 +
  载荷映射边界）、`nes-scene/tests/s17_tee_observer.rs`（组合器三契约）、
  `nes-extension-js/tests/binding_mock.rs`（绑定层 hat 派发/反向发射/
  handler 抛错，无引擎在场）。

---

## §5 遗留（按裁决口径写明）

1. **startHats 重入多线程语义**：Scratch 的 startHat-and-wait（hat 栈、
   并发纱线）未做 —— 本期 hat 是同步单发（事件 -> handler 跑完返回），
   无并发纱线、无 yield。QuickJS 单线程纪律下做多 hat 并发需要协程
   调度（rquickjs 无内建；未来方案 = 宿主侧生成器驱动或分时切片）。
2. **hat 参数过滤**：Scratch hat 可按载荷字段过滤（`when I receive
   [msg v]` 无参；自定义 hat 有谓词参数）。本期订阅只按**信号名**，
   载荷过滤留给 JS handler 体内自判（`if (payload > 3) ...`）——
   语义等价、少一层机制；若未来要做声明式过滤，扩 `SignalFilter`
   的谓词形态即可（enum 扩条，非破坏）。
3. **权限模型**（S17.1 遗留的延续）：任意扩展可订阅任意信号名、可反向
   发射任意名 —— 无每扩展权限声明、无命名空间隔离（如 `ext-id/*` 前缀
   强制）。订阅容量 256 / 发射缓冲 4096 / 级联 1024 三层上限是失控
   损失的界，不是权限。
4. **载荷形态**：`Node` 句柄不出界（本期裁决）；树 `Value::Resource`
   以槽位号 number 出界（不透明，不可悬挂 —— 槽位号跨场景加载无意义）。
   二进制载荷（字节串）仍归 NesValue 扩条留待按需。
5. **hat 里的读快照是一帧前的**：泵内 `nes.scene.find/getPos` 读的是上
   次 `update_extensions` 刷新的快照（本帧 tick 前状态）—— 写同帧落地、
   读一帧旧，与 update 期"读当 tick 后"不同，如实文档；要当帧读需把
   快照刷新挪进 tick 前帧首（成本 = 每帧多一次全树遍历，本期不做）。
6. **`nes.onSignal` 无取消**：注册即终身（无 offSignal；订阅名表本期
   无删除通道）。扩展卸载/热重载整体归 S17.1 遗留的"扩展热重载"项
   （届时卸载 = 丢弃 Context + 清订阅名 + 移除 hat 表）。
