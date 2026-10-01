# NES 2.0 · S8.0 脚本生命周期状态（Script State Lifecycle）v1

> 交付日期：2026-10-01　｜　状态：**`init` 块落地（首派发前执行一次 / 重挂载重跑 / 哨兵可观测）+ num_to_str（Script→Text 闭环第一块板）+ Dodge 兼容压力基线（游戏级 ABI）**
> 前置：S7.4 压力图（局部无初始化 = 结构性发现 #2；num_to_str = #4）。

---

## 0. 一句话结论

Script 从"能执行代码"向"能承载游戏对象行为"走的第一步：**`init` 块**
（`init { hp = 3 }` —— 可选、在入口前、每脚本一个；**挂载后首次派发前**
执行一次，与入口共享局部；重挂载 = 局部复位 + **重跑 init**，与 S6.32
"换程序不打补丁"同一条语义；执行后写哨兵局部 `__initialized` ——
可观测（`vm.locals`）且**天然进语义指纹**）+ **`num_to_str()`** 内建
（I64/F32 → 十进制 Str；F32 用最短往返表示 —— Rust Display 口径，
跨版本稳定；非数值停机）—— Dodge 的 HUD 现在显示 `HP: 2/3`，
`if s == 0 { s = 1; ... }` 手同步模式全部删除。另立**兼容压力基线**
（`examples/regression/dodge/`：场景 + 轨迹 + 期望哈希入库；T-ABI-01
每次改 VM/Scene/Signal 自动比对 600 帧指纹 —— NES 自己的游戏级 ABI）。
全仓测试 **432**（34 / 180 / 44 / 42 / 88 / 44）全绿，守卫 11/11，
clippy 零警告。

---

## 1. 语义与裁决

### 1.1 init 块（S8.0 主体）

```text
script := ["init" "{" stmts "}"] ("on" STRING | "every") "{" stmts "}"
```

| 裁决点 | 口径 |
|---|---|
| 执行时机 | **挂载后首次派发前**（process 首帧 / 首次信号处理器调用 —— 两路同口径）。不绑 enter/ready 回调：晚挂载（节点已 ready）的脚本一样在首个派发机会带头跑 init —— 时机统一为"该脚本自己的第一次"，不依赖装载与生命周期的相对时序 |
| 次数 | 每挂载一次。哨兵 `__initialized` 局部记录"已初始化"（可观测 + 进指纹）；后续派发跳过 |
| 重挂载 | attach 既有语义就是局部复位 —— init 随之重跑（热重载换程序 = 状态归零重来，不迁移） |
| 与"一脚本一入口" | init 是**块不是入口**：与入口同节点共存（S7.4 痛点"行为按入口拆节点"不减ule —— 拆分属后续裁决）；只有 init 没有入口 = 编译错误；init 在入口之后 = 编译错误（顺序即生命周期） |
| 局部作用域 | 与入口共享同一份局部表 —— init 写的 hp 就是入口读的 hp |
| 存量兼容 | 无 init 的脚本零变化（`Script.init = None`；不写哨兵） |

**确定性**：init 跑在首个派发内、序由挂载序（前序）与信号序决定 ——
都在 S7.1 冻结范围内；哨兵进指纹（局部表的一部分）。

### 1.2 num_to_str（S8.2 第一块板，按评审提前）

- `num_to_str(3)` → `"3"`；`num_to_str(1.5)` → `"1.5"`（F32 最短往返
  表示 —— Rust `Display` 口径，跨版本稳定；这是**显示**用格式，
  不是序列化格式）；
- 与既有 Str+Str 拼接闭环：`"HP: " + num_to_str(hp) + "/3"` ——
  **Script → Value → String → 属性 → 渲染文本** 全链通（HUD 是第一
  个真实需求，S7.4 压力图 #4）；
- 非数值停机（如实，不猜）。

### 1.3 兼容压力基线（游戏级 ABI）

```text
examples/regression/dodge/
    scene.ron          # 冻结的 Dodge 场景（含 init 版脚本）
    trace.txt          # 走位轨迹
    expected_hash.txt  # trace hash 4eec4143a559012e（600 帧）
```

- **T-ABI-01**：每次测试运行自动比对 —— VM/Scene/Signal 的任何语义
  变更在此炸响。区别于单元契约（钉单条语义）：基线压的是**合取语义**
  （输入×脚本×信号×生命周期×指纹的整条链，正如 S7.4 证明的：
  单条规则都对、合起来可能不是你以为的样子）；
- 基线更新流程：有意变更语义时重跑 CLI 生成新哈希 → 人工评审对应
  里程碑文档 → 更新 expected_hash.txt。哈希漂移本身不是错误，
  **未经评审的漂移**才是。

## 2. 实现落点

| 位置 | 改动 |
|---|---|
| `nes-scene/src/script.rs` | `Script.init: Option<Vec<Op>>`（+`with_init`）；文法（可选 init 块）；process/信号两路首派发前跑 init + 哨兵；`INIT_LOCAL` 常量（导出）；`Op::NumStr` + `num_to_str(` 内建 |
| `nes-scene/tests/s8_lifecycle.rs` | T-LC-01..05（新文件） |
| `nes-runtime/tests/criterion_headless.rs` | T-ABI-01（基线比对） |
| `nes-runtime/examples/regression/dodge/` | 基线三件套（新） |
| `nes-runtime/examples/assets/first_game.ron` | Dodge 迁移：`init { hp = 3; hud.text = ... }` + 掉血向下计数 + `HP: n/3` HUD；player_move 用 init 同步位置（删 `s == 0` 手同步） |

## 3. 出口准则

| 编号 | 契约 | 结果 |
|---|---|---|
| T-LC-01 | init 先于首次 process 执行一次；哨兵可观测；第二帧不重跑 | ✅ |
| T-LC-02 | init 先于首次信号处理器执行 | ✅ |
| T-LC-03 | 热重载（重挂载）：局部复位 + init 重跑 | ✅ |
| T-LC-04 | 无 init 存量零变化；init-无-入口/在入口后均报错 | ✅ |
| T-LC-05 | num_to_str：I64/F32/拼接/非数值停机 | ✅ |
| T-ABI-01 | Dodge 基线 600 帧指纹 == 期望哈希 | ✅ |
| T-GP-01 | （复验）Dodge 玩法闭环 + 确定性 —— init 迁移后仍绿 | ✅ |

## 4. 遗留与后续（沿 S8 路线）

| 事项 | 状态 |
|---|---|
| S8.1 游戏节拍语义（系统级内建 tick / fixed_process / process 关系冻结 —— 不开放 process 跨节点写，Cmd 权限模型不动） | 下一里程碑 |
| S8.3 集合原语（Array<Value> / for_each —— 不做 ECS） | 排队 |
| S8.4 第二个真实项目（不同类型：俯视角射击/平台跳跃/小型 RPG 房间 —— 压生命周期/多实体/UI/状态管理） | 排队 |
| Schema Extension（结构化游戏状态组件，替代 meta KV） | 方向记录（评审：属性表封闭是**好**设计，别用动态字典逃逸） |
| 实体规模抽象（Component 挂载模型） | S8 后半 / S9 评审 |
| 窗口视觉验收（S7.2 环境问题） | 独立验证项，不阻塞 |

## 5. 记账

- 测试基线：nes-asset 34 / **nes-scene 180**（175 -> 180，+T-LC-01..05）/
  nes-render-api 44 / nes-render-extract 42 / nes-render-wgpu 88 /
  **nes-runtime 44**（43 -> 44，+T-ABI-01）—— 全绿，合计 **432**；
- 守卫 11/11；六 crate `clippy --all-targets` 零警告；
- S7.4 压力图对账：#2（局部无初始化）**已解除**、#4（数字不能转
  字符串）**已解除**；#1/#3/#5/#6/#7 留在表上按 S8 路线处理。

*（内容由AI生成，仅供参考）*
