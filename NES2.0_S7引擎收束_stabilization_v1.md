# NES 2.0 · S7 引擎收束（Engine Stabilization）v1

> 交付日期：2026-10-01　｜　状态：**不横向加功能 —— 用 396（现 400）项测试网 + 11 条依赖守卫反向审整个 Runtime 架构，审出五项发现、修四项半、清洁账单开出**
> 前置：S6.34 编辑器脚本面板（全仓 396 测试全绿的时点）。

---

## 0. 一句话结论

收束阶段的做法：**先审后修、只修高置信项、每项带测试**。静态扫描
（panic 家族 / unsafe / 全局静态 / 无界增长 / 帧路径迭代序确定性）+
语义审计（VM 登记、信号泵、表面生命周期、装配序）共得**五项发现
（F1~F5）**：一项一致性缺口（死节点清理五表不统一）、一项错误
分类缺口（表面 Timeout 误诊为配置矛盾 —— 本会话实测过的瞬态错曾
把示例进程杀死）、两项健壮性（输入队列无界、BMP 算术域）、一项
宿主策略（示例对瞬态帧错零容忍）。全部修复（F1~F5）+ 新增
T-Stab-01..04。**清洁账单**：panic 家族在库代码零裸用（全部
`#[cfg(test)]` / 构造保证 / 前置判空）、帧路径零 HashMap 迭代
（连接按注册序 Vec、渲染按 BTreeMap、树按前序）、NodeId 带代号、
unsafe 逐块 SAFETY 注。全仓测试 **400**（34 / 166 / 40 / 42 / 87 / 31）
全绿，守卫 11/11，clippy 零警告。

---

## 1. 审计方法与覆盖面

| 审计维度 | 手段 | 结论 |
|---|---|---|
| panic 家族（unwrap/expect/panic!/todo!/unreachable!） | 全 src 扫描逐个分类 | **清洁**：非测试代码中全部是（a）`#[cfg(test)]` 模块、（b）构造保证不变量（如编译器循环栈配对、"上一步已收集"）、（c）前置判空后的解包（如 `entry.path().unwrap()` 前一行判 `is_none() → continue`） |
| unsafe 块 | 逐块抽查（wnd_proc / FFI / Drop / surface） | **清洁**：每块带 SAFETY 注；句柄释放先判空再置空；静态队列用 Mutex（毒化走 `into_inner` 恢复） |
| 全局静态 | 扫描 `static` | 仅窗口字符队列一处（进程级、单窗口口径，已有文档）+ FFI 常量 |
| 帧路径确定性 | 逐结构审计 | **清洁**：信号按连接注册序（`Vec<SignalConnection>`）、处理器表 HashMap 只做键查不迭代、渲染层 BTreeMap 全序、树遍历前序 —— **帧路径无 HashMap 迭代**（Rust 迭代序按进程随机，这是"396 绿但行为不确定"的潜伏通道，审过没有） |
| 无界增长 | 登记/队列/缓存逐个过 | **F4**：字符队列无界（已修）；VM `scripts` 表按串键缓存（共享编译产物，设计如此）；其余登记表见 F1 |
| 错误分类 | BackendError 变体 vs 实际故障 | **F2**：表面获取失败全归 `ConfigMismatch` 裸数字（已修） |
| 数值域 | 解码/尺寸算术 | **F5**：BMP 尺寸乘加 + `w*h*4` 的 **u32 域**回绕（已修） |

## 2. 五项发现与处置

### F1 · 死节点清理五表不统一（一致性缺口，已修）

S6.32 引入 attach_all 前置清理（四表），S6.33 加了第五张 `file_stamp`
但**漏进清理表**，且 `attach_all_with_sources` 整个入口**没有清理**
（依赖宿主先跑 poll）。NodeId 带代号（slot+gen）使陈旧键不会误交付
—— 这是卫生缺口不是正确性缺口，但违反了 S6.32 写下的不变量。
**修**：抽出 `prune_dead`（五表统一），五个装载/轮询入口
（attach_all / attach_all_with_sources / poll_reloads /
poll_reloads_with_sources）全部走它；新增 `tracked_nodes()` 观测口径
（五表取最大）。T-Stab-01 钉死：外置装载 → 整树替换 → 再装载 →
登记归零（两条路径都断言）。

### F2 · 表面获取失败误诊（错误分类缺口，已修）

`acquire` 把全部非成功状态报成 `ConfigMismatch`（"装配参数自相矛盾"）
+ 裸数字。**本会话实测过 status=3 —— wgpu 的 `Timeout`，瞬态可重试**
（窗口遮挡/合成器停顿时呈现队列暂满），却被当成致命配置错。**修**：
ffi 补齐 3~6 状态常量；`surface_status_name` 命名化（纯函数可测）；
Timeout 归 `BackendError::Timeout`（既有瞬态类），其余（OUTDATED/
LOST/OUT_OF_MEMORY）保持 ConfigMismatch 但带状态名。T-Stab-02 钉死
六个状态名 + 未知码如实报"未知"。

### F3 · 示例对瞬态帧错零容忍（宿主策略，已修）

`script_panel` / `engine_window` 对任一帧 `Err` 直接 `exit(1)` ——
F2 的一次 Timeout 就能杀死进程（实测发生过）。**修**：连续瞬态错
计数（上限 120 帧约 2 秒），期间跳帧重试，成功清零；超限才退出。
引擎语义不变 —— 容忍策略是宿主侧的，错误分类是引擎侧的，两层合
起来才是完整的瞬态语义。

### F4 · 字符输入队列无界（健壮性，已修）

`TYPED` 静态队列只进不出（宿主不排空时）无上限。**修**：容量上限
4096，满时丢新保旧（键盘语义：FIFO 头是最早到达的字符）；wnd_proc
的真实按键与 `inject_char` 走同一上限入口。T-Stab-03 钉死封顶、
FIFO 头保留、drain 清空。

### F5 · BMP 尺寸算术域（健壮性，已修）

两处：① `data_off + row*h` 未 checked（64 位下 i32 域尺寸实际到不了
溢出，但 checked 使口径与目标位宽无关）；② **`Vec::with_capacity((w*h*4) as usize)` 是 u32 域乘法** —— 合法的 32768² 32bpp 大图（4GB
文件）在 debug 构建直接 panic。**修**：① checked_mul/checked_add →
"头尺寸字段非法"；② 容量改 usize 域。T-Stab-04 钉死极端头（i32 上限
尺寸）走 Err 不 panic、平凡截断照常指名。

## 3. 清洁账单（审过无恙的部分）

- **信号泵语义**：交付集 = 宿主预发 + 回调发射，BFS 级联（非同步递归），
  迭代上限 1024 如实计数丢弃；路由按注册序、广播与路由共用上限；
  订阅过滤只管广播（S6.19 实证修正的口径在注释里可追溯）。
- **脚本 VM 停机纪律**：类型/栈/步数（10000）三类停机局部可观测
  （`__halt`），不崩帧；除零 checked；移位 wrapping 显式。
- **错误分类学**：BackendError 十四变体语义清晰（库缺失/符号/句柄/
  适配器/设备/超时/映射/配置/命令流/像素缓冲/尺寸/IO），本轮只补了
  表面状态一类。
- **代数身份**：NodeId slot+gen（arena 复用不撞身份）、ResId 同构 ——
  F1 的"卫生而非正确性"判定正建立在这上面。
- **析构序**：NesRuntime 字段序声明了消费器在窗口/表面之前析构；
  SurfaceTarget Drop 先 unconfigure 再 release、置空防双释放。
- **零依赖纪律**：手写 FFI/解析器/编解码器全部遵守"如实报错不猜"，
  本轮加固的 BMP 算术是同一条纪律的收口。

## 4. 遗留分级汇总（全部 S6 文档去重后）

| 类 | 事项（去重） | 状态/口径 |
|---|---|---|
| **A 平台端口** | WM_SIZE → 表面重配置；DPI 感知（per-monitor v2）；headless Linux + Mesa 认证；多 GPU（compatibleSurface） | 未启动（出现于 12+ 文档的"沿遗留表"，收束不改 —— 属移植工程非架构缺口） |
| **B 编辑器壳层** | 光标移动/选区/多行编辑；IME 与字形表扩覆盖；多窗口输入分流（队列按 HWND）；面板滚动/折叠；事件监视器（`signals_dropped` 面板）；处理器表只读视图；编辑子场景实例；`.nes` 语法提示；差量保存组装层；编辑器提示族（rename 识别 / 同名后缀 / 下标漂移） | 未启动（等编辑器成为产品方向；S6.34 面板是首块基石） |
| **C 脚本语言** | Vec2 任意表达式（Pack）；切片 `s[a..b]`/`contains`/`upper`/`lower`；字节序；内插值；编译期警告；表达式位 `++`；`DUP`/`Swap`；短路逻辑 | 未启动（按需逐个加臂；Op 的 RON 编码已被 source 文本路径**取代**） |
| **D 运行时语义裁决** | `paused`/`time_scale`/`ProcessMode`；process 入口 Arg=delta；观察者复合（宿主+VM 转发器）；热重载状态迁移 opt-in；成员写即视语义；tick 外 `apply_pending` 入泵；连接载荷/优先级/once；多观察者/通配订阅 | 未启动（逐个里程碑裁决，不提前实现） |
| **E 性能** | AssetRegistry Scene 键缓存；重排最少移动生成；StrIndex 双次字符计数；信号队列 `remove(0)` O(n²)（上限 1024 内） | 观测口径（无正确性收益，按需） |

## 5. 出口准则

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Stab-01 | 死节点清理五表统一：外置装载 → 整树替换 → `attach_all_with_sources`/`attach_all` 再装载 → `tracked_nodes()==0` | ✅ |
| T-Stab-02 | 表面状态命名映射：六个 wgpu 状态码 ↔ 名字，未知码报"未知" | ✅ |
| T-Stab-03 | 字符队列有界：5000 注入 → 4096 封顶、FIFO 头保留、drain 清空 | ✅ |
| T-Stab-04 | BMP 恶构头（i32 上限尺寸）走 Err 不 panic；平凡截断指名 | ✅（debug + release 双构建验证） |

## 6. 记账

- 测试基线：nes-asset 34 / **nes-scene 166**（165 -> 166，+T-Stab-01）/
  nes-render-api 40 / nes-render-extract 42 / **nes-render-wgpu 87**
  （84 -> 87，+T-Stab-02..04）/ nes-runtime 31 —— 全绿，合计 **400**；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
  examples 全部零警告构建；
- 改动面：nes-scene/script.rs（prune_dead + tracked_nodes）、
  nes-render-wgpu（ffi.rs 状态常量、gpu.rs 命名与分类、window.rs
  队列上限、bmp.rs checked 算术）、nes-runtime 两示例（瞬态容忍）；
  **帧循环语义零改动**（收束不碰行为面 —— F2 只改错误分类不改
  返回时机，F3 只改宿主侧策略）。

*（内容由AI生成，仅供参考）*
