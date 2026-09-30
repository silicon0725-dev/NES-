---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 19886e2d8bf8a6bde5831a704b5a86f4_087cc5d8b8a011f189c8525400393706
    ReservedCode1: LXzIvuBjsWLTLTHmGodRaHZI7GKRPsyGyBoWh8x1Eu3GDHLbLhNy0r28kf0VkeR4WrgpvbJjs1xw4CzQTPdFxgdH1TgBLdLdDHtvRbPe6UYk+6+6OM0O0gG0DMmYN0UhhJCzMObVJfdKXbVWG3M44pgBk4f3WxPETgkxQYumSjxuft77dpIkeu7nboU=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 19886e2d8bf8a6bde5831a704b5a86f4_087cc5d8b8a011f189c8525400393706
    ReservedCode2: LXzIvuBjsWLTLTHmGodRaHZI7GKRPsyGyBoWh8x1Eu3GDHLbLhNy0r28kf0VkeR4WrgpvbJjs1xw4CzQTPdFxgdH1TgBLdLdDHtvRbPe6UYk+6+6OM0O0gG0DMmYN0UhhJCzMObVJfdKXbVWG3M44pgBk4f3WxPETgkxQYumSjxuft77dpIkeu7nboU=
---

# NES 2.0 × TWN-5 扩展生态兼容性扫描 与三块缺口清单 v1

- 扫描对象：`F:\All NGVGE\NES 2.0\ruference\拓展`
- 扫描方式：静态正则统计（全量 619 个 .js/.mjs，无抽样），与 TWN-5 已认证能力面比对
- 参考基线：TurboWarp/scratch-vm@c4823421
- 扫描日期：2026-09-25
- 说明：本扫描为**语义参考反推**，即用真实扩展的调用面来定义内核对 load / sandbox / BlockUtility 的最小可用要求，不涉及把扩展翻译进 Rust。

---

## 1. 总体规模

| 指标 | 数值 |
|---|---|
| JS 文件 | 619 |
| 总行数 | 477,441 |
| 总体积 | 25.89 MB |
| 作者/来源目录 | 21 |
| 含 `getInfo` | 596（96.3%） |
| 含注册调用 | 584 |
| 声明 `blocks` | 576 |
| opcode 总数 | 8,852 |
| menus 声明数 | 355 |
| 唯一扩展 id | 431 |

**读法**：619 文件 → 431 唯一 id，说明存在大量同名副本/多版本（如「简单3D」同时存在于两个作者目录）。加载器必须具备 **id 去重与指纹识别**，否则注册表必然冲突。

来源集中度：杂项 435 文件（32.5 万行）、YL_YOLO 87 文件（10.5 万行），两者占全生态 89% 行数。

---

## 2. 生态真实依赖面（按静态引用统计）

### 2.1 scratch-vm 门面（按引用次数）

| 门面 | 次数 | 命中文件 | 内核需交付 |
|---|---|---|---|
| `Scratch.ArgumentType` | 10,687 | 463 | 常量表 + 参数类型校验 |
| `Scratch.BlockType` | 7,033 | 514 | 常量表 + 块形状 |
| `Scratch.Cast` | 2,389 | 123 | 完整 Cast 语义（数字/字符串/颜色/布尔/数组） |
| `Scratch.translate` | 1,709 | 62 | i18n 门面 + 词条装载 |
| `Scratch.extensions` | 791 | — | register / unsandboxed / isSandboxed |
| `Scratch.vm` | 597 | 192 | vm 门面（runtime / renderer / 全局单例） |
| `Scratch.gui` | 64 | — | GUI 门面（窗口、菜单、弹窗） |
| `Scratch.TargetType` | 61 | — | 常量表 |
| `Scratch.openWindow` / `canOpenWindow` / `canEmbed` / `download` / `canFetch` | 16 / 2 / 2 / 6 / 15 | — | **权限模型**（沙箱下必须可裁决） |

### 2.2 runtime 门面（`this.runtime.*`，Top 20）

| 成员 | 次数 | 性质 |
|---|---|---|
| `requestRedraw` | 200 | 公开 |
| `targets` | 180 | 公开（需活对象集合） |
| `on` | 143 | 公开（事件总线） |
| `stageWidth` / `stageHeight` | 128 / 99 | 公开 |
| `renderer` | 121 | 公开（门面） |
| `extensionStorage` | 108 | 公开（持久化） |
| `getTargetForStage` | 86 | 公开 |
| `ioDevices` | 63 | 公开（沙箱下唯一设备桥） |
| `startHats` | 57 | 公开（**需运行时可重入**） |
| `getSpriteTargetByName` / `getTargetById` | 52 / 46 | 公开 |
| `ext_pen` | 44 | **跨扩展互访** |
| `ext_xeltallivSimple3Dapi` | 34 | **跨扩展互访** |
| `logSystem` | 30 | 内部 |
| `_editingTarget` | 25 | **私有** |
| `isPackaged` | 22 | 内部 |
| `_step` | 21 | **私有** |
| `once` | 21 | 内部 |
| `fontManager` | 20 | 内部 |

### 2.3 renderer 门面（`this.renderer.*`，Top 12）

| 成员 | 次数 | 性质 |
|---|---|---|
| `dirty` | 188 | 公开 |
| `_allSkins` | 181 | **私有字段** |
| `_allDrawables` | 168 | **私有字段** |
| `updateDrawableSkinId` | 107 | 内部 |
| `canvas` | 84 | 公开 |
| `_groupOrdering` | 80 | **私有字段** |
| `draw` | 76 | 内部 |
| `_layerGroups` | 64 | **私有字段** |
| `_drawThese` | 62 | **私有方法** |
| `_nativeSize` | 50 | **私有字段** |
| `exports` | 49 | 内部 |
| `destroySkin` | 48 | 内部 |

### 2.4 BlockUtility 实调面（`util.*`，Top 18）

| 成员 | 次数 | 性质 |
|---|---|---|
| `target` | 890 | **活对象（Target）** |
| `stackFrame` | 181 | **活对象（帧）** |
| `thread` | 161 | **活对象（线程）** |
| `inherits` | 86 | 控制流（返回 Promise） |
| `runtime` | 82 | 活对象 |
| `startBranch` | 81 | 控制流（返回 Promise） |
| `yield` | 20 | 协程让出 |
| `ioQuery` | 19 | 沙箱设备桥 |
| `isFunction` / `isNullOrUndefined` / `isString` / `isNull` / `isBuffer` | 16 / 10 / 8 / 8 / 8 | 类型判定 helper |
| `stack` / `pushParam` | 7 / 6 | **栈帧结构操作** |
| `stackTimerNeedsInit` / `startStackTimer` / `stackTimerFinished` | 4 / 4 / 4 | 定时让出 |

### 2.5 浏览器宿主依赖（按命中文件数）

| 依赖 | 命中文件 | 占 619 |
|---|---|---|
| `document.` | 324 | 52.3% |
| `window.` | 199 | 32.1% |
| `setTimeout` | 177 | 28.6% |
| `fetch(` | 75 | 12.1% |
| `navigator.` | 48 | 7.8% |
| `localStorage` | 29 | 4.7% |
| `AudioContext` | 26 | 4.2% |
| `require(` | 19 | 3.1% |
| `new Worker` | 13 | 2.1% |
| `WebSocket` | 12 | 1.9% |
| `XMLHttpRequest` | 11 | 1.8% |
| `import(` | 8 | 1.3% |
| `OffscreenCanvas` | 3 | 0.5% |

### 2.6 异步特征

`async` 195 文件 / `await` 179 文件 / `Promise` 196 文件 / `then(` 命中广泛。

**结论**：block func 的返回值必须是 **thenable 感知**的，内核执行器必须支持 Promise 返回块，否则近 1/3 扩展行为错误。

---

## 3. 三块缺口清单

### 块 A — 加载器（对应 scratch-vm extension-manager）

| # | 缺口 | 证据 | 内核需交付 | 优先级 |
|---|---|---|---|---|
| A1 | getInfo 规范化与校验 | 596/619 有 getInfo；23 个无 | getInfo 解析器 + 缺失/畸形降级策略 | P1 |
| A2 | id 去重与副本识别 | 619 文件 / 431 唯一 id | 内容指纹 + id 冲突裁决 | P1 |
| A3 | opcode 命名空间拼接 | 8,852 opcodes | `id + '_' + opcode` 注册与反解 | P0 |
| A4 | 参数类型/形状校验 | ArgumentType 10,687 次、BlockType 7,033 次 | 常量表 + 声明期校验 | P1 |
| A5 | 菜单/字段装载 | 355 处 menus | menus → 下拉数据源 | P1 |
| A6 | 模块依赖解析 | `require(` 19 文件、`import(` 8 文件 | 单文件内联优先，外部依赖需解析或拒载 | P2 |
| A7 | 生命周期（注册/卸载/重载） | 584 注册；重启竞态已在 TWN-5 认证 | 加载器侧触发点 + 幂等 | P1 |
| A8 | `Scratch.extensions` 门面 | 791 次 | register / unsandboxed 标志 | P0 |

### 块 B — 沙箱宿主

**关键判定：不可一刀切。** 52.3% 的扩展直接操作 `document`，32.1% 操作 `window`；纯 Worker 沙箱会直接判死一半生态。必须分区：

| 分区 | 适用 | 隔离手段 | 覆盖 |
|---|---|---|---|
| Unsandboxed（主上下文） | 依赖 DOM 且可信/本机 | 无隔离，仅权限声明 | ~300+ 文件 |
| iframe 沙箱 | 需要 DOM 但需隔离 | 独立文档 + postMessage 桥 | 剩余 DOM 类 |
| Worker 沙箱 | 纯计算、无 DOM | Worker + ioQuery 桥 | 计算类扩展 |

| # | 缺口 | 证据 | 内核需交付 | 优先级 |
|---|---|---|---|---|
| B1 | 沙箱分区策略与判定规则 | document 324 / window 199 | 静态判定 + 清单覆盖 | P1 |
| B2 | 消息传输与序列化契约 | TWN-5 已有 DTO 契约 33/33 | 扩展实例句柄、活对象禁传边界 | P1 |
| B3 | 权限模型 | `canFetch`/`canOpenWindow`/`canEmbed` 共 19 次，fetch 75 文件 | 声明式权限 + 运行期裁决 | P2 |
| B4 | 设备桥 | `ioQuery` 19 次、`runtime.ioDevices` 63 次 | 沙箱下唯一设备通路 | P1 |
| B5 | 网络能力 | fetch 75 / WebSocket 12 / XHR 11 | 沙箱代理由宿主发起 | P2 |
| B6 | 存储能力 | localStorage 29 文件 | 虚拟存储或放行 | P2 |
| B7 | 音频能力 | AudioContext 26 文件 | 音频后端联动（内核音频为壳） | P2 |
| B8 | 子 Worker / 动态载入 | `new Worker` 13、`import(` 8 | 策略裁决（放行/拒绝/降级） | P3 |
| B9 | GPU 能力 | OffscreenCanvas 3 | 与 wgpu 后端对齐 | P3 |

### 块 C — BlockUtility 能力面

**最重要的发现**：BlockUtility 不是纯 DTO，它**泄出活对象引用**（`target` 890 次、`stackFrame` 181 次、`thread` 161 次）。因此内核必须先有 **target / thread / stack frame 三元运行时模型**，否则本块无法真实实现——这是「TWN-5 有调用契约」与「扩展能跑起来」之间的本质断层。

| # | 缺口 | 证据 | 内核需交付 | 优先级 |
|---|---|---|---|---|
| C1 | Target 活对象 | `util.target` 890 | 目标对象模型 + 属性读写（x/y/方向/造型/变量/克隆） | **P0** |
| C2 | Thread / StackFrame | thread 161、stackFrame 181、`stack`/`pushParam` 13 | 线程与栈帧结构 + 参数栈 | **P0** |
| C3 | 分支与继承执行 | `startBranch` 81、`inherits` 86 → 均返回 Promise | 控制流原语 + Promise 化执行 | **P0** |
| C4 | 协程让出与计时 | `yield` 20、`startStackTimer*` 12 | 帧内让出 + 定时续跑 | P1 |
| C5 | Hat 触发 | `runtime.startHats` 57 | 事件 → hat 重入 | P1 |
| C6 | 类型判定 helper | 5 个判据共 50 次 | 工具函数集 | P2 |
| C7 | 线程内数据（extensionStorage 等） | `extensionStorage` 108 | 扩展级持久键值 | P2 |

---

## 4. 关键风险（扫描暴露，非推测）

1. **跨扩展实例互访**：`runtime.ext_pen`(44)、`runtime.ext_xeltallivSimple3Dapi`(34) 表明扩展直接按 `ext_<id>` 约定访问**其他扩展实例**。一旦放进 Worker/iframe 沙箱，此类互访必然断裂 → 沙箱需提供跨扩展实例的**代理注册表**，或把互访类扩展强制划入主上下文。
2. **私有 API 名必须逐字对齐**：`_editingTarget`、`_step`、`renderer._allSkins`、`_allDrawables`、`_groupOrdering`、`_layerGroups`、`_drawThese`、`_nativeSize` 合计被引用 800+ 次。内核渲染层若自造命名，3D/画笔/皮肤类扩展将静默失效。兼容槽需按 TurboWarp 原名保留。
3. **Promise 返回块**：196 文件使用 Promise，执行器不支持 thenable 返回即为大面积行为偏差。
4. **DOM 占比过半**：决定了「纯沙箱化」路线不成立，必须三档分区。

---

## 5. 建议实施顺序

1. **P0 先行（决定能否跑通单个扩展）**：C1/C2/C3（target + thread + stackFrame + 分支原语）→ A3/A8（opcode 注册 + extensions 门面）。
2. **P1（决定能否批量跑真实扩展）**：A1/A2/A4/A5/A7（加载器全链）+ C4/C5 + B1/B2/B4 + 2.1 常量门面（ArgumentType/BlockType/Cast/translate）。
3. **P2/P3（决定生态完整度）**：B3/B5/B6/B7 + C6/C7 + renderer 私有槽兼容 + GUI 门面（`Scratch.gui` 64 次、`openWindow` 16 次，量小可后置）。

**验收建议**：以「随机抽 20 个真实扩展，能在内核完成 加载 → 参数校验 → block 执行 → util.target 读写 → Promise 返回」为 P0 出口准则；以「596 个含 getInfo 的扩展编译期全部可加载、运行期无门面缺失报错」为 P1 出口准则。
*（内容由AI生成，仅供参考）*
