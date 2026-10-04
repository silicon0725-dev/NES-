# NES 2.0 · S17.3 协程与权限（C4 生成器让出 + B1-B4 收敛裁决：B3 权限模型）v1

## §0 结论

**S17.3 双件达成：JS 扩展有了 Scratch 线程语义的轻量等价物（生成器协程，
帧计数驱动、确定性），且权限模型在 NES 语境收敛为 B3 一件（声明期权限
数组 + 能力注入处逐调用裁决），全部纯 JS 侧实现（bootstrap 一处改），
零 Rust 面新增、零新依赖。**

- **C4 协程让出**：`onUpdate` / `onSignal` 处理器可为**生成器函数**
  （`function*`）—— 调用返回值有 `.next` 即入调度器。`yield`（裸）= 停
  一帧续跑；`yield n` = 停 n 帧续跑（推进周期 n+1 帧）；done = 出表；
  生成器内 throw = 既有 fault 隔离（S17.1）。hat 触发 = **新建实例**
  （Scratch startHats 重入语义），同 hat 并发多实例，每扩展活动实例上限
  32（拒新留旧 + fault 计数）。
- **B 收敛裁决**：rquickjs 上下文**本身就是沙箱**（无 DOM/文件/网络，
  能力面之外一无所有），Web 世界的三档分区（unsandboxed/iframe/worker）
  在 NES 语境**不适用**；B2 DTO 边界已有（`NesValue` 无活对象过界）；
  B4 设备桥即能力面（注入多少 traits，扩展就能做多少事）。三件在既有
  架构里**已天然成立**，真正缺的只有 **B3 权限模型**：
  `nes.registerExtension(id, perms)` 第二参声明权限数组（缺省 = 全授予，
  hello.js 兼容），能力调用逐调用裁决，未授予抛
  `Error("permission denied: <cap>")` → 走既有 fault 隔离路径（不炸不
  静默）。
- 实现形态：**调度器与守卫全在 `NES_BOOTSTRAP_JS`（每扩展一份，纯 JS）**
  —— JS 值全部留在 JS 堆、Rust 侧零句柄的形态纪律不变；生成器推进在
  `__nes_update`/hat 派发的同一次 `rt.call` 内 = 同一份 50ms ExecBudget
  预算；S17.1 三道防线（异常隔离 / 死循环中断 / 自动停用）全数继承。
- 十 crate `cargo test --release` 全绿（基线 708 + 新增 20 = **728**，
  其中 T-COR-01..03 + T-PERM-01..03 及附面板全过），clippy 0 ×10，守卫
  15/15（零新依赖，G14/G15 不动），ext_demo 冒烟终态 **(146.41873,
  147.01727)** 与 S17.1/S17.2 基线**逐位一致**（hello.js 无声明 = 全授予
  回归的直接实证），JS 异常帧 0。

git：单提交（本文件随提交入库）；不 push。

---

## §1 C4 生成器协程（扫描文档 yield/startStackTimer 的 NES 收敛）

### 1.1 生成器语义表（冻结）

| 处理器写法 | 语义 |
|---|---|
| 普通函数 `onUpdate` | 现状：每帧调用一次，返回值忽略（回归照旧） |
| 生成器函数 `onUpdate` | 首帧钩子调用返回生成器对象 -> 入调度器 + **首段立即推进** + 钩子退役（一条持久线程 —— Scratch 绿旗语义；循环由生成器体内 `while (true)` 自持） |
| `yield;`（裸） | 暂停 **1** 帧（推进周期 2 帧：下一帧停顿、再下一帧推进） |
| `yield n;`（正数） | 暂停 **n** 帧（推进周期 n+1 帧；向下取整） |
| `yield 0;` / 负数 | 不停顿（下一帧立即推进） |
| `yield 非数`（undefined/对象等） | 按裸 yield（停一帧） |
| 生成器 return / 完成 | 实例出表（不重放；生成器 onUpdate 的钩子不复活） |
| 生成器内 throw | 逐表项 try/catch、坏实例出表、首错循环后重抛 -> fault（S17.1） |
| hat（`onSignal`）为生成器 | 每次触发**新建实例**（首段立即推进 = 泵内 in-tick，S17.2 裁决 A 继承）；同 hat 并发多实例、互不干扰；完成序 = 生成序 |
| 活动实例上限 | **每扩展 32**（`COROUTINE_CAP`，JS 侧 `__nes_coro_cap` 同源同步锚）：超限**拒新留旧** + 抛 `Error("coroutine cap exceeded: ...")` -> fault 计数 |

帧驱动规则（`__nes_update` 内，**先驱动全表再调一次性钩子** —— 保证
spawn 帧的 wait 不被同帧二次递减）：`wait > 0` -> 减一（归零当帧仍停，
次帧推进）；`wait == 0` -> `gen.next()` 推进；hat 协程派发后随下一帧的
`__nes_update` 推进（触发帧的余量计入停顿帧）。

### 1.2 确定性论证（帧计数驱动，无墙钟）

- 推进完全由**帧计数**驱动：同源码 + 同触发序 -> 同生成器实例表（表序 =
  生成序）-> 同 `gen.next()` 序 -> 同写队列 -> 同树状态 -> 同指纹。调度
  器状态（`__nes_coros`）是纯 JS 堆数据，随上下文隔离，跨扩展不可见。
- 无 `setTimeout`/无墙钟等待：startStackTimer 的"定时续跑"收敛为
  **`yield n` 的帧计数**（60Hz 下 `yield 60` ≈ 1 秒，宿主 delta 固定时
  精确）。真实墙钟定时器（宿主 delta 不定）不做 —— 破坏确定性口径。
- ExecBudget 继承：全表推进 + 钩子调用在一次 `rt.call` 内，50ms 预算
  罩住失控生成器（`while(true)` 无 yield 的生成器体同罪，测试
  T-HAT-05 同族路径）；hat 首段推进在派发那次 `rt.call` 内同预算。
- 上限的损失界：32 活动实例 x 每实例一次推进的预算份额 —— 失控损失
  上界与订阅 256 / 发射 4096 / 级联 1024 同一条"如实截断 + 计数"纪律。

### 1.3 上限面板：自级联 hat 的自我截断（T-COR-02b）

生成器 hat 首段发射下一条信号（`emitSignal("go", p+1)`）即成**自级联
自我繁殖**：同泵级联逐条派发、逐条生成实例；第 33 个实例在 `coro_start`
处被拒 —— **其首段（本应发射下一条的正是首段）不跑，级联到此自截止**
—— 恰一次拒新即一次 fault，泵存活、已接受实例照常挂起。上限因此是
**自 enforcement 的**（不需要外部断路器）。

---

## §2 B1-B4 收敛论证（NES 语境的裁决）

扫描文档的 B 组四件按"能力面架构已定"的语境逐件裁决：

| 件 | Web 世界的形态 | NES 语境裁决 | 理由 |
|---|---|---|---|
| B1 分区 | unsandboxed / iframe / worker 三档宿主分区 | **不适用（天然成立）**：rquickjs 上下文本身就是沙箱 —— 无 DOM、无文件、无网络、无定时器，扩展能触达的一切由注入的 `nes` 能力对象给定（S17 第 1 期冻结裁决的正面推论）；"分区"在这里退化为"每扩展一个 Context"（已实现） | 沙箱天然性：QuickJS 隔离全局堆 + 64 MiB/1 MiB 上限 + 50ms 预算 |
| B2 DTO 边界 | structured clone / postMessage 序列化 | **已有**：`NesValue` 值边界（函数/宿主对象不过界；Node 句柄不过界）—— 无活对象跨界的裁决 S17 第 1 期已冻结 | 双向都过 `NesValue`/树 `Value` 映射（S17.2 载荷映射表） |
| B3 权限模型 | permissions API / 权限提示 | **本期落地**（见 §2.1） | 唯一真实缺口 |
| B4 设备桥 | getUserMedia 等设备能力 | **即能力面**：设备 = 宿主桥出的 trait（P0 五能力即五类"设备"）；未来新设备 = 新 trait + 新权限名，架构位已留 | 能力注入面即设备面 |

### 2.1 B3 权限模型（本期实现）

**声明**：`nes.registerExtension(id, perms)` 第二参可选数组：

```js
nes.registerExtension("my-ext", ["scene.read", "scene.write", "input", "audio", "signal"]);
```

**权限表**（五项；命名空间 = 能力族）：

| 权限名 | 门卫的能力 |
|---|---|
| `scene.read` | `nes.scene.find` / `nes.node.getPos` / `nes.node.getName` |
| `scene.write` | `nes.node.setPos` / `nes.node.setVisible` |
| `input` | `nes.input.isPressed` |
| `audio` | `nes.audio.play` |
| `signal`（新增第五权限） | `nes.onSignal` / `nes.emitSignal`（信号是扩展 <-> 游戏双向面） |

**裁决语义**：

- **缺省 = 全授予**（不传 `perms`）—— hello.js 兼容，Beta 后可切
  default-deny（见 §4）；空数组 = 全拒绝；未知名忽略（前向兼容）。
- **裁决点 = 能力注入处的方法包装**（bootstrap `__nes_guard`）：逐调用
  检查 grants，未授予抛 `Error("permission denied: <cap>")` -> 沿标准
  `rt.call` 错误路径浮出成 fault（S17.1：计数 + 诊断 + 连续失败停用；
  不炸帧、不静默吞）。
- **声明期静态**：权限在 `registerExtension` 时定死（重复注册最后声明
  生效且只在此后生效）；**运行期无提权接口**——动态提权归权限模型后续
  期。守卫是纯 JS 闭包，grants 是上下文本地全局（每扩展一份，互不可见）。
- `registerExtension` / `onUpdate` 本身不受权限约束（生命周期面不是
  能力面；不给这两个，扩展连"自己是谁、每帧跑什么"都声明不了）。
- 拒绝的语义位置：注册期拒绝（如顶层 `onSignal` 无 `signal`）= 装载
  失败、订阅不进泵过滤器（零开销闸不开）；运行期拒绝 = 逐次 fault。

---

## §3 门禁（全部实测）

- 十 crate `cargo test --release` 全绿：nes-asset 34、nes-audio 52、
  nes-extension-api 7、nes-extension-js 24、nes-media 27、
  nes-render-api 45、nes-render-extract 56、nes-render-wgpu 123、
  nes-scene 247、nes-runtime 113 = **728 passed / 0 failed**
  （基线 708 + 新增 20：nes-extension-js 协程 7 + 权限 5、nes-runtime
  T-COR-01/02/02b/03 + T-PERM-01/02/03 + input 面共 8）。
- clippy 0 ×10（全 crate `--all-targets`）。
- 守卫 `check_dependency_direction.py` **15/15**（零新依赖；改动集中在
  nes-extension-js 的 bootstrap 字符串与测试 —— 白名单闭包不动）。
- ext_demo 冒烟：headless 300 帧全过，终态 **(146.41873, 147.01727)**
  与 S17.1/S17.2 基线逐位一致（hello.js 无声明 = 全授予；普通函数
  onUpdate 路径零变化 —— bootstrap 新增的驱动调用在空表上是 no-op），
  JS 异常帧 0。
- 契约测试落点：`nes-runtime/tests/s17_3_coroutine_perms.rs`
  （T-COR-01..03 + 上限面板 + T-PERM-01..03 + input 面）、
  `nes-extension-js/tests/coroutine_mock.rs`（调度器逐帧语义 + cap 32 +
  throw 隔离 + 生命周期 trait 面 + JS/Rust cap 字面量同步锚）、
  `nes-extension-js/tests/perms_mock.rs`（权限逐项拒绝/授予 + 缺省回归 +
  静态声明 + 守卫透传，mock 宿主记账"无声音副作用"）。

---

## §4 遗留（按裁决口径写明）

1. **default-deny 切换**：缺省全授予是 Beta 期兼容口径（hello.js 等
   首批扩展无感知）。正式期切 default-deny = bootstrap 一处布尔翻转 +
   既有扩展补声明 —— 翻转点留在本文件的 §2.1 口径上，届时逐扩展审计。
2. **动态提权 / 权限对话框**：运行期无提权接口（本期裁决）。未来若做，
   形态应是"宿主侧提示 + 下一帧生效的新 grants"，需要宿主 UI 面与
   扩展清单（manifest）—— 归权限模型后续期。
3. **C6/C7 工具与扩展存储**：扫描文档 C 组的扩展自有存储（localStorage
   等价物）与工具注册（扩展给游戏脚本暴露新指令）未做 —— 两者都会新
   增能力面（存储 trait / 工具注册 trait），落点在既有 capability 冻结
   面上扩条（additive），并各自配权限名（如 `storage` / `tools`）。
4. **生成器 onUpdate 的退役语义**：钩子在 spawn 后退役（一条持久线程）；
   若未来需要"生成器结束后自动重启"（Scratch 的 forever 包装形态），
   在 `__nes_update` 的 done 分支重建钩子即可（一处 JS 改动）—— 本期
   不做（显式 `while (true)` 更贴近 Scratch 帧语义且可静态读出）。
5. **`nes.waitFrames(n)` 语法糖不存在**（本期裁决）：`yield n` 即语义
   本体，文档化即可；包装函数只会在堆栈里多一帧、无表达力增益。
6. **hat 协程的取消**：hat 生成器实例无取消句柄（只能跑完或 throw 出表
   —— 扩展卸载/热重载时随 Context 整体丢弃）；与 S17.2 遗留的
   "offSignal / 扩展热重载"同项归档。
