# NES 2.0 · S6.22 循环控制 v1

> 交付日期：2026-10-01　｜　状态：**break / continue（编译期循环上下文栈）**
> 前置：S6.21 控制流与逻辑（while 的跳转结构已备）。

---

## 0. 一句话结论

脚本文法新增 `break` / `continue`：编译器维护**循环上下文栈**
（`LoopCtx { top, breaks }`）—— `continue` 的目标（循环顶）即时可知；
`break` 的出口下标在循环收尾统一回填（与 `if` 的 `JumpIfNot` 同一占位
手法）。嵌套循环自然绑定**最内层**。保留字 10→12。出口准则
T-Cmp-10..12 全过。全仓测试 **142** / 34 / 42 / 40 / 83 / **27** 全绿，
守卫 11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 目标与回填

- `continue` -> `Jump(top)`：top 在 while 开头即定，无需回填；
- `break` -> `Jump(0)` 占位：出口（尾跳之后）在循环编译完才确定，
  收尾统一回填（`ctx.breaks` 清单）—— 与 `if`/`else` 的跳转回填同一
  家法，零新指令；
- 循环尾跳（`Jump(top)`）在 break 之后**不可达但保留**：结构完整性
  （while 的产物形状恒定）优先于死码消除（T-Cmp-12 产物断言钉死）。

### 1.2 绑定与错误

- 嵌套：`break`/`continue` 绑定栈顶（最内层）—— 内层 break 不影响
  外层（T-Cmp-11 双层实证）；
- 循环外使用 -> 编译错误（"break 在循环外"，行定位）—— 不静默、
  不当作 no-op（错误的家法）。

### 1.3 与 while true 的关系

`while true { ... break }` 是合法惯用法 —— break 是**唯一**出口时
不依赖 `SCRIPT_MAX_STEPS` 兜底（T-Cmp-10 断言无 `__halt`）；死循环
（无 break）仍由步数上限保护（S6.21 口径不变）。

## 2. 实现落点

| 位置 | 改动 |
|---|---|
| `LoopCtx` + `TextParser.loops` | 编译期循环栈（top + break 占位清单） |
| `stmt` | `break`（占位+登记）/ `continue`（即时 Jump(top)）两臂 + 循环外报错 |
| `while` 臂 | 入栈 -> 体（先出栈后 `?`，错误不泄漏栈帧） -> 收尾回填 |
| `RESERVED` | 增 `break` / `continue` |

## 3. 出口准则（`s6_script_text.rs` 追加，3/3）

| 编号 | 契约 | 结果 |
|---|---|
| T-Cmp-10 | `while true` + 计数到 3 `break` 逃出；循环后语句照常执行；无 `__halt`（不靠步数兜底） | ✅ |
| T-Cmp-11 | `continue` 跳过偶数（odd=1+3+5=9）；双层嵌套内层 break 各停 2、外层照常 3 轮 | ✅ |
| T-Cmp-12 | 循环外 break/continue 编译错（行定位）；产物断言（break 回填到出口、尾跳保留） | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 带标签 break（跨层跳出） | 未启动（栈结构已备，加 label 表即可） |
| `for`/区间迭代（`for i in 0..n`）—— 纯糖（while 可表达） | 未启动 |
| 位运算 / `%` / 字符串拼接 | 未启动 |
| 脚本进场景文件 / headless Linux / WM_SIZE / DPI | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 142**（140 -> 142，+T-Cmp-10..12）/ 其余不变
 （34/42/40/83/27）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（script.rs 编译器循环栈）与新测试；其余零改动。

*（内容由AI生成，仅供参考）*
