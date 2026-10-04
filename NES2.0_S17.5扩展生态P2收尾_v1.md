# NES 2.0 — S17.5 扩展生态 P2 收尾（C6 工具集 / C7 扩展存储 / default-deny 选入）v1

## §0 结论

扩展生态 P2 三件小件**全清**，全部落在 `nes-extension-js/src/binding.rs` 的
`NES_BOOTSTRAP_JS`（每扩展一份的引导脚本）——**零 Rust 语义改动、零新依赖**，
沿用"JS 值留 JS 堆、Rust 零句柄"的形态纪律：

* **C6 工具函数集** `nes.util.*`（clamp / lerp / sign / dist / rand / randInt，
  纯 JS）——`dist` 直接收编 shake.js 手搓 `Math.sqrt` 的实战痛点
  （**dogfooding 回归**：shake.js 改用 `nes.util.dist` 后 S17.4 既有断言
  T-GE-01..03 照绿）；
* **C7 扩展级存储** `nes.storage`（get/set/has/remove/keys + `at(id)` 显式
  跨访）——声明期自动命名空间，值域 = JSON 可序列化面（函数/undefined/环
  引用 set 即抛），生命周期 = 运行时会话；
* **B3 default-deny 选入**——`nes.registerExtension(id, perms, opts)` 第三参
  `{ strict: true }`：strict 下 perms 缺省从"全授予"变为"全拒"（声明了什么
  才有什么）；非 strict（缺省）行为逐位不变（hello.js 兼容，两种模式都有
  测试钉住）。

门禁全绿：十 crate `cargo test --release` **736** passed（基线 731 + 新增 5）、
clippy 0 ×10、守卫 15/15、ext_demo 300 帧冒烟通过（JS 异常帧 0）、first_game
300 帧冒烟通过（hello + shake 装载零错误）+ dungeon_game 同口径复跑零错误。
场景文件、regression/ 基线、能力 trait 冻结面零改动。

## §1 三件小件（全部 bootstrap 纯 JS）

### 1.1 C6 工具函数集（`nes.util.*`）

| 函数 | 语义 | 边界口径 |
|---|---|---|
| `clamp(v, min, max)` | 夹取到闭区间 | `min > max` 时交换（全函数）；NaN 透传 |
| `lerp(a, b, t)` | 线性插值 `a + (b-a)*t` | 参数经 `+` 强制数值（防字符串拼接） |
| `sign(v)` | -1 / 0 / 1 | NaN → 0（`(v>0)-(v<0)` 公式） |
| `dist(x1, y1, x2, y2)` | 欧氏距离 | 3-4-5 直角三角形精确（`sqrt(25)=5` IEEE 精确） |
| `rand(min, max)` | 均匀 `[min, max)` | **非确定性分区**（见下） |
| `randInt(min, max)` | 均匀整数，**含两端** | `floor(min + r*(max-min+1))`；`min > max` 交换 |

**非确定性分区（文档口径）**：`rand`/`randInt` 基于 `Math.random`，扩展自行
选择使用；引擎核心确定性承诺（headless 基线 / regression 指纹）**不覆盖**
扩展内部随机——headless CLI 与 `run_headless` 不装载扩展，基线路径逐位不动
（S17.4 同款裁决，本期把它升级成工具集的显式契约文档）。

### 1.2 C7 扩展级存储（`nes.storage`）

* **声明期自动命名空间**：`nes.registerExtension(id, ...)` 即建
  `__nes_storage[String(id)]`——隔离单元 = 扩展 id；根表与各命名空间均用
  `Object.create(null)`（`__proto__`/`constructor` 等键按普通属性处理，
  无原型链走私，测试钉住）；同 id 重注册不清库（会话内幂等）。
* **store 面（每 id 五方法）**：`get(key, defaultValue)`（缺省缺省值 =
  null）/ `set(key, value)` / `has(key)` / `remove(key)` / `keys()`。
* **值域 = JSON 可序列化面（NesValue 同族）**：`set` 经 `JSON.stringify`
  试编码——顶层函数/undefined 使 stringify 返回 undefined → 抛
  TypeError；嵌套函数由 replacer 抓 → 抛；环引用 stringify 必抛；入库值
  = `JSON.parse` 重建的**纯 JSON 面**。`get` 返回**深拷贝**——改返回值不
  落库，也无法把活对象/函数从读侧走私进存储。
* **跨扩展互访**：`nes.storage.at(id)` 返回同款 store 面（显式才可达）。
  信任模型 = **同上下文互信**（P0 文档口径；隐私隔离归权限后续期）。
  生产宿主每扩展一个上下文（上下文即沙箱边界），跨上下文的存储天然完全
  不可见——`at(id)` 的语义域是"同上下文多扩展"形态（绑定层契约，测试
  两种粒度都钉住）。
* **生命周期 = 运行时会话**：上下文在即存续；扩展停用不清、运行时重建
  即清（新上下文从空库开始）。落盘持久化归后续期。

### 1.3 default-deny 选入（B3 收官）

```text
nes.registerExtension(id[, perms[, opts]])
  opts 缺省 / { strict: false }  ->  perms 缺省 = 全授予（S17.3 口径不变）
  opts = { strict: true }        ->  perms 缺省 = 全拒（[]）
  显式 perms 数组：两模式同义（空数组 = 全拒；未知名忽略）
```

注册期定死、运行期无提权的既有口径不动；strict 只是"缺省值"的翻转点——
正是 S17.3 遗留 1 预留的"bootstrap 一处布尔翻转"，本期以**选入**形态落地
（缺省仍全授予，Beta 期扩展无感知；正式期全量切 strict = 逐扩展补
`{ strict: true }` 或宿主侧装配策略，机制已就位）。

### 1.4 shake.js dogfooding（T-REG）

`examples/assets/Extensions/shake.js` 的距离计算从手搓
`Math.sqrt(dx*dx + dy*dy)` 换成 `nes.util.dist(pp[0], pp[1], ep[0], ep[1])`
（头注释契约表同步加一行 S17.5 C6）。既有 `s17_5_game_ext.rs` 用例
T-GE-01（Dodge 自然逼近触发震动 + 震后逐位归位）/ T-GE-02（Mini Dungeon
同文件触发）/ T-GE-03（坏扩展不挡游戏）**零改动照绿**——工具集收编实战
痛点且行为不变的正面证据。

## §2 扩展生态总盘点表（P0/P1/P2 全清单 × 状态）

NES 扩展运行时是**能力对象形态**（QuickJS + 注入 `nes` 对象），不是
scratch-vm 移植——扫描清单（`NES2.0_扩展生态扫描与三块缺口清单_v1.md`）
是参照系，各项按 NES 语境收敛。下表为终局盘点（P0 = S17 第 1 期 +
S17.1；P1 = S17.2/S17.3/S17.4；P2 = 本期）。

### 2.1 块 A — 加载器

| # | 扫描项 | NES 收敛形态 | 状态 |
|---|---|---|---|
| A3 | opcode 命名空间 | N.A.——无积木/opcode 面，能力对象直调 | 不适用 |
| A8 | `Scratch.extensions` 门面 | `nes.registerExtension(id[, perms[, opts]])` | **P0 已交付** |
| A1 | getInfo 规范化 | 自报 id + 文件名兜底（`load_extension_file`）；无块元数据 | **P0 已收敛** |
| A2 | id 去重 | 每扩展独立 Context（重名互不干扰）；市场级去重归 manifest | 收敛 + 后续期 |
| A4 | 参数类型校验 | `NesValue` 冻结边界 + rquickjs 类型转换（load/call 参数化） | **P0 已交付** |
| A5 | 菜单/字段 | N.A.——无积木 UI 面 | 不适用 |
| A6 | 模块依赖 | 单文件全局脚本（P0 裁决）；import/require 不支持 | 裁决关闭 |
| A7 | 生命周期 | 装载/注册/更新有；卸载/热重载未做 | 部分（§4 遗留） |

### 2.2 块 B — 沙箱宿主

| # | 扫描项 | NES 收敛形态 | 状态 |
|---|---|---|---|
| B1 | 沙箱分区 | 单分区：每扩展一 QuickJS Context（内存 64MiB + 栈 1MiB + 50ms 预算三道闸）；桌面三分区（DOM 52%）不适用于内核形态，已文档化 | **P0 已收敛**（分区遗留项关闭为文档化） |
| B2 | 消息/序列化契约 | `NesValue` 冻结边界（活对象/函数不跨界） | **P0 已交付** |
| B3 | 权限模型 | 声明期五权限名 + 逐调用守卫（S17.3）；**default-deny 选入（本期 `{ strict: true }`）** | **P2 已收官** |
| B4 | 设备桥 | `nes.input.isPressed`（帧快照读） | **P0 已交付** |
| B5 | 网络能力 | 无 fetch/WebSocket/XHR——沙箱零网络 | 后续期（未排期） |
| B6 | 存储能力 | `nes.storage`（会话内 JSON 键值，本期）；落盘归后续期 | **P2 已交付**（落盘后续期） |
| B7 | 音频能力 | `nes.audio.play(key, volume)`（混音器 key） | **P0 已交付** |
| B8 | 子 Worker/动态载入 | 不适用（QuickJS 无 Worker；无动态 eval） | 不适用 |
| B9 | GPU 能力 | 不适用（扩展无渲染面；相机偏移层归渲染后续期） | 不适用 |

### 2.3 块 C — BlockUtility 能力面

| # | 扫描项 | NES 收敛形态 | 状态 |
|---|---|---|---|
| C1 | Target 活对象 | `NodeRef` 不透明句柄 + `nes.scene.find` / `nes.node.*` | **P0 已交付** |
| C2 | Thread/StackFrame | 收敛为生成器协程表（`__nes_coros`，不暴露栈帧） | **P1 已收敛**（S17.3） |
| C3 | 分支/继承执行 | JS 原生控制流（宿主语言承担） | 不适用 |
| C4 | 协程让出 | `function*` + `yield n` = 停 n 帧（帧计数驱动，确定性） | **P1 已交付**（S17.3） |
| C5 | Hat 触发 | `nes.onSignal`/`emitSignal`（泵内同步重入，重入语义 = 新建实例） | **P1 已交付**（S17.2） |
| C6 | 类型判定 helper → 工具函数集 | `nes.util.*`（六函数，§1.1） | **P2 已交付（本期）** |
| C7 | extensionStorage → 扩展级存储 | `nes.storage`（§1.2） | **P2 已交付（本期）** |

### 2.4 NES 系交付面（S17 序列）

| 期 | 交付 | 状态 |
|---|---|---|
| P0（S17 第 1 期） | 冻结 API（NesValue / JsRuntime / 五能力 traits）+ QuickJS backend + 失控三道闸 + hello.js + ext_demo | 已交付 |
| P0（S17.1 硬化） | 执行预算中断闸 + 错误隔离（不炸不静默）+ 连续故障自动停用 | 已交付 |
| P1（S17.2） | 信号 hat 触发（订阅 + 反向发射 + 泵内重入） | 已交付 |
| P1（S17.3） | 生成器协程 + 权限模型（声明期五权限名） | 已交付 |
| P1（S17.4） | 宿主惯例（Extensions/ 目录三宿主接线）+ 跨游戏真扩展 shake.js | 已交付 |
| **P2（本期）** | **C6 `nes.util.*` + C7 `nes.storage` + B3 default-deny 选入 + shake.js dogfooding** | **本期交付** |

## §3 门禁（全部实测，worktree wt-p2 / 分支 s17-5-p2）

| 门禁 | 结果 |
|---|---|
| 十 crate `cargo test --release` | **全绿，合计 736 通过 / 0 失败**（基线 731 + 新增 5） |
| 新增测试 | `nes-extension-js/tests/s17_5_util_storage.rs`：T-U-01（util 六函数数学断言）、T-S-01（隔离 + `at(id)` 跨访 + 跨上下文不可见）、T-S-02（会话生命周期三段）、T-S-03（值域 JSON 面：函数/undefined/环拒收、get 深拷贝、`__proto__` 不走私、未注册显式报错）、T-D-01（strict 全拒 / strict 只读 / `strict:false` 全授予 / 空数组同全拒）|
| T-REG（dogfooding 回归） | `nes-runtime/tests/s17_5_game_ext.rs` T-GE-01..03 零改动照绿（shake.js 换用 `nes.util.dist` 后两游戏震动行为不变） |
| clippy（10 crate × all-targets） | **0 警告 0 错误 ×10** |
| 守卫 `check_dependency_direction.py` | **15/15**（零新依赖） |
| ext_demo 冒烟 | `NES_GAME_FRAMES=300`：hello 装载、位移 111.49px、**JS 异常帧 0**、`[OK]` |
| first_game 冒烟（dogfooding 主战场） | `NES_GAME_FRAMES=300`：hello + shake 装载，error/fault/exception/diagnostic 零命中 |
| dungeon_game 冒烟（复跑） | `NES_GAME_FRAMES=300`：同口径零错误 |

分 crate 计数：nes-scene 247 / nes-asset 34 / nes-render-api 45 /
nes-render-extract 56 / nes-render-wgpu 123 / nes-audio 52 / nes-media 27 /
nes-extension-api 7 / nes-extension-js 29（24 + 新增 5）/ nes-runtime 116。

零改动面：`first_game.ron` / `dungeon.ron` / `farm.ron` 场景文件、
`examples/regression/` 基线、nes-extension-api 冻结面（能力 traits 一个
签名未动）——本期改动 = `nes-extension-js/src/binding.rs`（bootstrap JS +
文档注释）、`nes-extension-js/src/lib.rs`（文档注释）、`nes-extension-js/
tests/s17_5_util_storage.rs`（新）、`examples/assets/Extensions/shake.js`
（dogfooding 一行 + 注释）。

## §4 遗留（真正剩下的）

1. **存储落盘持久化**：`nes.storage` 生命周期 = 运行时会话（本期裁决）。
   落盘需要宿主侧存储 trait（additive 扩能力冻结面）+ 写时机策略（帧末
   合并/即时），归扩展生态后续期。
2. **扩展卸载 / 热重载**（S17.1 遗留延续）：无卸载 API；届时 = 丢弃
   Context + 清订阅名 + 移除 hat 表 + `__nes_storage` 随上下文整体消亡
   （与"会话即生命周期"口径自洽）。`offSignal` 同项归档。
3. **沙箱分区（确定性回放场景）**：`Math.random`（含 `nes.util.rand`）在
   扩展分区合法；若未来要"带扩展的确定性回放"，需引擎侧随机注入/记录
   （S17 第 1 期遗留项）。headless 基线路径不装载扩展，现状不受影响。
4. **流式音频等媒体项**：归 S14 系（媒体解码适配层）演进，与扩展生态
   解耦——扩展侧 `nes.audio.play` 的 key 面不变。
5. **动态提权 / 权限对话框 / 扩展 manifest**（S17.3 遗留 2 + S17.4 遗留
   1/2 延续）：机制已就位（grants 注册期定死 + strict 翻转点），宿主 UI
   面与清单格式归后续期。
6. **B 桌面分区无关项**：扫描清单 B1 的桌面三分区（DOM/iframe/Worker）
   对内核扩展形态无适用对象——本表 §2.2 已按"单分区收敛 + 文档化"关闭，
   非待办。
