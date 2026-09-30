---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 19886e2d8bf8a6bde5831a704b5a86f4_ce87228bbcc011f1b172525400248c00
    ReservedCode1: oCNsZLtmcm+7SxcDBQpd43OWPvUeT0AN8uTjkl590sIl0+cOl6nG6Au7msO+AOVcNra115GmbwXAoU04BdJ1G2MwqWS4jRzYHf3FpnXV1vGsm0mLs6FyG6lfgsIt4+iIMMNraZV63dpc6WQspHjQs7rX1qcKpEQ6NP3QpA+EqfcBXRs3KcolaMuV+Vg=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 19886e2d8bf8a6bde5831a704b5a86f4_ce87228bbcc011f1b172525400248c00
    ReservedCode2: oCNsZLtmcm+7SxcDBQpd43OWPvUeT0AN8uTjkl590sIl0+cOl6nG6Au7msO+AOVcNra115GmbwXAoU04BdJ1G2MwqWS4jRzYHf3FpnXV1vGsm0mLs6FyG6lfgsIt4+iIMMNraZV63dpc6WQspHjQs7rX1qcKpEQ6NP3QpA+EqfcBXRs3KcolaMuV+Vg=
---

# NES 2.0 · M4 渲染接入 归档说明 v1

> 归档日期：2026-09-30　｜　归档执行：Marvis file-agent　｜　状态：**S4.1 已封口（见 §11 修订）**
> 本文档记录本次归档的范围与校验结果、尚未完成的事项、计划事项、待裁决问题、架构约束与环境信息，供后续接手 S4.1 时直接使用。
> 本文档在 `output` 目录保留同一份副本：`...\conv_b3458d8493f34d69b1aa7f1a67fa414c\output\NES2.0_M4渲染接入_归档说明_v1.md`。

---

## 0. 一句话结论

~~M4 渲染接入的契约层、提取层与两个上游 crate 已封口且在本机全绿；后端 crate `nes-render-wgpu` 尚未封口，lib 构建剩 2 个编译错误。~~
**【v1.1 更新】S4.1 已于 2026-09-30 封口**：`nes-render-wgpu` 补齐 `renderer.rs` 后在本机实机跑通最小可视闭环（清屏 + 精灵 → 读回 → PNG，逐像素断言 + 外部解码器交叉验证均过），守卫扩至 G1~G10 全过。详见 `NES2.0_M4渲染接入_S4最小可视闭环_封口_v1.md` 与本文 §11。

---

## 1. 归档范围与校验结果

| 项目 | 内容 |
|---|---|
| 产物源 | `C:\Users\Administrator\AppData\Roaming\Tencent\Marvis\User\90EA32E1CDA20B32D089B53E8D26FB7F\workspace\conv_b3458d8493f34d69b1aa7f1a67fa414c\output` |
| 归档目标 | `F:\All NGVGE\ALL` |
| 归档文件数 | 复制产物 **61 个**（5 个 crate 源码/清单/测试 54 个 + 顶层文档与脚本 7 个）＋ 本次新增本文档 1 份 = 当前 **62 个** |
| 归档总字节 | **778,578 字节**（约 760 KB） |
| 排除项 | 各 crate 的 `target/` 构建缓存（5 个 crate 均存在）、`.git/`、`*.pdb` |
| 一致性校验 | 源与目标逐文件比对：**无缺失、无多余、无大小不一致**（61 / 61 完全一致） |
| 目录结构校验 | `nes-scene` / `nes-asset` / `nes-render-api` / `nes-render-extract` / `nes-render-wgpu` 五棵子树及顶层文件全部就位，`nes-render-wgpu` 下**没有** `examples/` 目录 |

### 归档后目录树

```text
F:\All NGVGE\ALL\
├── NES2.0_M4渲染接入_归档说明_v1.md          ← 本文档
├── NES2.0_M4渲染接入_S1契约冻结_v1.md        （含 §11 v1.1 修订）
├── NES2.0_M4渲染接入_S2提取层封口_v1.md
├── NES2.0_M4渲染接入_S3四项缺口封口_v1.md
├── NES2.0_扩展生态扫描与三块缺口清单_v1.md
├── NES2.0_节点场景树_接口草案_v1.md
├── m4_render_borrow_research.md
├── check_dependency_direction.py              （依赖方向守卫，G1~G7）
├── nes-scene\        （src 11 + tests 3 + Cargo.toml/lock，共 16 文件）
├── nes-asset\        （src 8 + tests 1 + Cargo.toml/lock，共 11 文件）
├── nes-render-api\   （src 8 + tests 1 + Cargo.toml/lock，共 11 文件）
├── nes-render-extract\（src 5 + tests 2 + Cargo.toml/lock，共 9 文件）
└── nes-render-wgpu\  （src 5：lib/error/ffi/gpu/png + Cargo.toml/lock，共 7 文件）
```

---

## 2. 本轮实测快照（2026-09-30，本机 Windows）

> 归档是对已落盘代码的**逐字节复制**，不改动任何源码。为让本说明中的"未完成项"准确，本轮在 `output` 副本上做了实跑复测（未写入归档目录，未污染归档内容）。

| 检查项 | 命令 | 结果 |
|---|---|---|
| 依赖方向守卫 | `python check_dependency_direction.py`（在 `F:\All NGVGE\ALL` 下跑） | **7/7 PASS，EXIT 0**（G1~G7） |
| `nes-scene` | `cargo build --lib` / `cargo test` / `cargo clippy --all-targets -- -D warnings` | EXIT 0 ｜ **75 项全绿**（lib 40 + m1 19 + m2 4 + m3 12） ｜ clippy 0 警告 |
| `nes-asset` | 同上 | EXIT 0 ｜ **34 项全绿**（lib 18 + m3 16） ｜ clippy 0 警告 |
| `nes-render-api` | 同上 | EXIT 0 ｜ **40 项全绿**（lib 8 + criterion_contract 32） ｜ clippy 0 警告 |
| `nes-render-extract` | 同上 | EXIT 0 ｜ **42 项全绿**（lib 6 + criterion_extract 24 + criterion_gaps 12） ｜ clippy 0 警告 |
| `nes-render-wgpu` | `cargo build --lib` | **EXIT 101，2 个错误**（见 §3.1） |
| `nes-render-wgpu` | `cargo clippy --all-targets` | EXIT 101：目标解析阶段即失败（`examples/s41_visual_closure.rs` 缺失） |

### 2.1 与历史交接记录的偏差（已按实测更正）

| 历史记录（交接要点） | 本轮实测 | 结论 |
|---|---|---|
| `nes-render-wgpu` lib 有 **9 个**编译错误（E0583 ×1、E0599 ×4 缺 `ConfigMismatch`、E0061 ×2、E0559 ×2） | 实际只剩 **2 个**（E0583 ×1、E0164 ×1） | `error.rs` 的 `ConfigMismatch` 变体已落盘、`gpu.rs` 的 FFI 调用已改用新 ABI 形参，旧 9 错已收敛；**以本轮实测为准** |
| 测试基线 75 / 34 / **39** / **41** | 75 / 34 / **40** / **42** | `nes-render-api` 集成测试 32 + lib 8 = 40；`nes-render-extract` 的 `criterion_gaps.rs` 实测 12 条（S3 文档记 11）。**文档基线未随新增用例回写**，属记账待办（见 §4.10） |
| "本机没有 MSVC 链接器 / gcc / cmake"（`nes-render-wgpu/src/lib.rs` 顶部注释） | `cargo test --no-run` 成功产出全部测试 exe 并实际运行通过 | 当前环境下**链接可用**，注释口径已过期，建议 S4.1 随封口更正（见 §4.11） |
| `check_dependency_direction.py` 覆盖 extract/wgpu 分层规则 | 脚本实际只输出 **G1~G7**，而 `nes-render-wgpu/Cargo.toml` 注释声明"由 G8 / G9 / G10 校验" | 守卫脚本**尚未扩到 G8~G10**（gpu crate 的依赖边未被守卫显式钉住），属待办（见 §5.4） |

---

## 3. 尚未完成的事项（未做的事）

### 3.1 【最高优先】`nes-render-wgpu` lib 构建仍有 2 个错误

| # | 错误 | 位置 | 事实与修法 |
|---|---|---|---|
| 1 | **E0583** `file not found for module 'renderer'` | `src/lib.rs:73` | `lib.rs` 已写 `pub mod renderer;` 并再导出 `renderer::{CommandConsumer, FrameOutcome, FrameStats, SpritePipeline, WgpuRenderServer}`，但 `src/renderer.rs` **未落盘**。需补该文件并至少提供这 5 个公开项（`RenderServer` 实现 + 命令消费器 + 帧统计 + 精灵管线） |
| 2 | **E0164** `expected tuple struct or tuple variant, found struct variant 'Self::MapFailed'` | `src/error.rs:102` | 枚举 `BackendError::MapFailed` 已改为结构体变体 `{ status: i32, message: String }`（`gpu.rs:806` 也按结构体构造），但 `Display` 分支仍写 `Self::MapFailed(status)`。改成 `Self::MapFailed { status, message } => ...` 即可，并顺带把 `message` 输出（当前该分支吞掉了驱动诊断串） |

> 佐证：`cargo build --lib` 报错尾部 `error: could not compile 'nes-render-wgpu' (lib) due to 2 previous errors`。

### 3.2 `examples/s41_visual_closure.rs` 缺失

- `Cargo.toml` 已声明 `[[example]] name = "s41_visual_closure", path = "examples/s41_visual_closure.rs"`，但 `examples/` 目录不存在。
- 后果：`cargo test` / `cargo clippy --all-targets` / `cargo build --examples` 在**目标解析阶段**就失败（`neither 'examples\s41_visual_closure.rs' nor 'examples\s41_visual_closure\main.rs' exists`），会先于 lib 的 2 个错误暴露，掩盖真实进度。本轮用 `--lib` 隔离后才发现真实错误数为 2。

### 3.3 S4 后端闭环未收尾

尚不能在 Rust 侧消费 `RenderCommand`、经 wgpu-native 画出「清屏 + 精灵」并落 PNG（离屏纹理 → 读回 → 手写 PNG 编码）。FFI 绑定与运行时装配（`ffi.rs` 38 KB、`gpu.rs` 46 KB、`png.rs` 8.9 KB）已成型，但缺 `renderer.rs` 这一层把命令流翻译成 GPU 动作。

### 3.4 `nes-render-wgpu` 出口准则测试未补

- 该 crate **无 `tests/` 目录**；`src` 内仅 4 条单测，全部在 `png.rs`（`crc32_matches_check_vector`、`adler32_matches_check_vector`、`encode_writes_well_formed_chunk_sequence`、`encode_rejects_wrong_pixel_count`）。
- `ffi.rs` / `gpu.rs`（合计 85 KB，S4 风险最集中处）**零测试**。

### 3.5 M5 Scratch 兼容层未启动

### 3.6 wgpu 的 Windows 工具链 / FFI cfg 移植未完成

当前 hand-written FFI 与 `LoadLibraryW` 动态加载路径只针对本机 Windows；面向 headless Linux 目标的编译条件（cfg 分支 / 动态库定位候选）尚未落地。

### 3.7 场景属性入口缺口（影响 S4 比对）

Label 排版参数（`line_spacing` / `align_h` / `align_v` / `wrap_width`）与相机 `limits` 在 `nes-scene` 侧**均无属性入口**，提取层"无米下锅"，S4 若涉及排版/夹紧比对需先补入口或明确降级口径。

### 3.8 编辑器 / 可视化脚本未启动

### 3.9 四项已知缺口的落地状态

| 编号 | 内容 | 状态 |
|---|---|---|
| D-S3-1 / Q-S3-1 | `ControlState::resolve` 的 `min_size` 零下界把负宽高压成 0 | **已按「修」落地**：`state.rs` 中 `min_size` 已改为 `Option<Vec2>`（缺省 `None` = 无下界），`resolve` 仅在 `Some(min)` 时施加下界，负宽高原样透传（S1 §11 v1.1 修订已记录） |
| D-S3-2 | `extractor.rs` 注释仍称 `order` 为"确定性前序序号"，与实现（取场景层 `NodeData::order` 子序键）不符 | **未修**：文档级表述失准（取值正确），S3 明确"本轮不改源码" |
| Q-S3-2 / Q-S3-3 | Label 排版参数、相机 `limits` 无入口 | 见 §6（待裁决） |
| Q-S3-4 | `order` 语义与同 z 绘制次序耦合 | 见 §6（待裁决） |

### 3.10 记账待办

- 封口文档中的测试基线（`nes-render-api` 39、`nes-render-extract` 41）与当前实测（40 / 42）差 +1，需在 S4 封口时按实测重写基线并说明差异来源。
- `nes-render-wgpu/src/lib.rs` 顶部"本机没有 MSVC 链接器"的注释口径与当前环境不符，需随封口更正。

---

## 4. 计划事项（要做的事）

按建议顺序：

| # | 事项 | 完成判据 |
|---|---|---|
| 5.1 | **补 `src/renderer.rs`**：实现 `RenderServer`（属性级推送）+ `CommandConsumer`（线性命令流消费）+ `FrameOutcome` / `FrameStats` / `SpritePipeline` / `WgpuRenderServer`，与 `lib.rs` 已有再导出对齐 | `cargo build --lib` EXIT 0 |
| 5.2 | **修 `error.rs:102`** 的 `MapFailed` 显示分支形状（改结构体模式并输出 `message`） | 同上（与 5.1 合并一次构建） |
| 5.3 | **写 `examples/s41_visual_closure.rs`**：清屏 + 精灵渲染 → 读回像素 → 落 PNG | 示例运行成功并产出 PNG；`cargo test` / `--all-targets` 不再在解析阶段失败 |
| 5.4 | **扩展 `check_dependency_direction.py` 至 G8 / G9 / G10**，把 `nes-render-wgpu` 的依赖边（仅允许 path 依赖 `nes-render-api`、禁第三方 crate、上游不得反向依赖后端）显式钉住——`Cargo.toml` 注释已声明这组守卫，但脚本尚未实现 | 脚本对新 crate 复跑仍 EXIT 0 |
| 5.5 | **补 `nes-render-wgpu` 出口准则测试**（当前 `gpu.rs` / `ffi.rs` 零测试），覆盖：库定位失败路径、符号缺失探测、离屏目标创建、读回行对齐、命令流不合法、PNG 落盘 | `cargo test` 全绿，用例数与基线一并锁进封口文档 |
| 5.6 | **S4 封口并回写文档**：更新本文档 §3、把实测测试基线写入封口文档、按 S1 §10 变更纪律追加修订小节（若动了冻结面签名） | 三项同时满足：`cargo test` 全绿 + `cargo clippy --all-targets -- -D warnings` 零警告 + 依赖守卫全 Pass |
| 5.7 | **测试基线对齐**（§3.10 两条记账项） | 封口文档数字与实测一致 |

### 后续（S4 之后，尚未启动）

1. M5 Scratch 兼容层。
2. wgpu 的 Windows 工具链 / headless Linux x86_64 移植与打通。
3. 场景属性入口补齐（Label 排版参数、相机 `limits`），取决于 §6 的裁决。
4. 编辑器 / 可视化脚本启动。

---

## 5. 待裁决问题

| 编号 | 问题 | 选项 | 影响面 |
|---|---|---|---|
| **Q-S3-2** | **Label 排版参数没有场景属性入口**：`line_spacing` / `align_h` / `align_v` / `wrap_width` 恒为契约缺省，`nes-scene` 侧无对应属性 → 居中对齐 / 自动换行在本里程碑**不可驱动** | **A** 为 Label 增补场景属性（需改 `nes-scene` schema，属上游范围）；**B** 接受"本里程碑不驱动排版"，把排版比对降级为文档口径 | S4 能否做"排版比对"；是否需要解锁上游 crate |
| **Q-S3-3** | **相机 `limits` 同样没有场景属性入口**：`Camera2DState.limits` 恒 `None`，S1 §7 Q1 裁决的"世界轴 AABB 夹紧"分支**永远走不到**，与 `twn-render-stage` 比对时也覆盖不到夹紧路径 | **A** 补场景属性以驱动夹紧（上游范围）；**B** 接受"本里程碑不驱动 limits"，把夹紧算式只留在契约层单测 | S4 与 `twn-render-stage` 的逐帧比对覆盖度 |
| **Q-S3-4** | **`order` 语义与同 z 绘制次序的耦合**（延续 S2 Q-A）：现裁决 `order` = 场景层子序键，故 `move_child` / `sort_children` 会在该帧改变同 z 组的绘制先后 | 在 S4 决策：**场景层提供稳定绘制序键** vs **提取层自持稳定序号** | 同 z 组绘制次序的稳定性（是否与产品预期/旧 stage 行为一致） |

> 已裁决并落地：**Q-S3-1 = 「修」**（契约层 `ControlState::resolve` 补齐到 S1 Q2 语义，`min_size` 改 `Option<Vec2>`，见 S1 §11.1）。
> S1 §9 的 Q1~Q8 已在 S1/S2/S3 期间沿用既定裁决（Q1 世界轴 AABB 夹紧、Q2 不钳制负宽高、Q5 句柄 64 位冻结、Q7 单相机单槽冻结等），本次归档未改动其口径。

---

## 6. 架构约束（不可退让的纪律）

### 6.1 方案 D 三件套

1. **渲染服务端化**：后端实现 `RenderServer` trait（属性级推送），提取层经 `ItemHandle` / RID 不透明柄操作，不暴露后端细节。
2. **单向依赖**：`nes-asset → nes-scene → nes-render-extract → nes-render-api → nes-render-wgpu`（renderer 只读场景，场景永不知渲染）。
3. **每帧提取**：每帧把 `NodeId ↔ ItemHandle` 重新提取一遍（不用增量订阅），保证确定性与可核对性。

### 6.2 分层与契约约束

- **契约层 `nes-render-api` 零依赖、零 `unsafe`，trait 保持对象安全**（不变量 I10）；不得直接使用 `nes-asset::AssetKey`，资源键以 `(slot, gen)` 位编码镜像。
- **冻结面**：S1 §2 签名与 §3 不变量在 S2/S3 期间只能**新增**（向后兼容的默认方法 / 可选字段），不得改形、不得改算式含义；必须改形时按 §10.2 追加「v1.x 修订」小节，逐条写明触发原因、改动前后签名、受影响测试、是否破坏已落地代码。
- **变更验收三联**：每次变更后必须同时满足 `cargo test` 全绿 + `cargo clippy --all-targets -- -D warnings` 零警告 + 依赖守卫全 Pass，任一不过视为未完成变更。
- **依赖守卫是持续不变量**，不随里程碑结束撤销；新增 crate 时若出现新依赖边，须在 G 系列登记预期方向（允许 `nes-render-extract → nes-render-api`，禁止任何 `nes-render-* → nes-scene` 反向边）。

### 6.3 已冻结的契约不变量（S1 §3，I1~I10 摘要）

| 编号 | 不变量 |
|---|---|
| I1 | 空句柄 / 未知 / 已销毁句柄的一切操作被静默忽略，不 panic、不影响同帧其余命令 |
| I2 | 句柄永不复用；slice 复用必须换 generation |
| I3 | `submit_into` 先清空 `out`，末条必为 `Submit`（调用方可跨帧复用缓冲 → 每帧零分配） |
| I4 | 命令流二分：一次性事件（Create/Destroy）+ 每帧全量快照（相机 + 各渲染物属性 + Submit），状态不变时快照逐条可重现 |
| I5 | 属性流按 `DrawKey`（`z → order → handle`）升序，禁止依赖哈希序或插入先后 |
| I6 | 帧内顺序固定：生命周期动作 → `SetCamera` → 各渲染物属性 →（Label）`SetText` →（Control）`SetRect` → `Submit` |
| I7 | `apply_item` 与逐项 setter 在契约上不可区分 |
| I8 | `world_transform == transform ∘ flip`，flip 不影响平移分量 |
| I9 | 相机注视点在旋转 + 缩放 + 夹紧后仍映射到视口中心；`enabled == false` 时 `view_matrix()` 为 `None` |
| I10 | 契约层零依赖、零 `unsafe`、trait 对象安全 |

### 6.4 后端 crate 的额外纪律（`nes-render-wgpu/Cargo.toml` 注释）

- 依赖树的**叶子**：直接依赖只有契约层 `nes-render-api`（path）；不得依赖 `nes-scene` / `nes-asset` / `nes-render-extract`。
- **不得引入任何第三方 crate**（registry 依赖一律越界）；FFI 绑定由本 crate 手写 `#[repr(C)]` + 运行时符号解析，不需要 `bindgen` / `wgpu-rs` / 任何 `build.rs`。
- 刻意**不并入**上层工作区：空 `[workspace]` 表把它显式钉成独立工作区根，避免 GPU 侧依赖面污染场景层构建图。
- 排版归属 **CPU 侧**（`SetText` 只登记"文本 + 字号"，字形度量不下沉 GPU）。
- "如实报告"纪律：库加载/适配器/设备/映射任何一步失败，一律返回带上下文的 `BackendError`，**不伪造截图、不谎报跑通**。

### 6.5 归档与封签边界

- 本次归档**只写入** `F:\All NGVGE\ALL`。
- **严禁**修改或删除 `F:\All NGVGE` 下同级其他目录：`NES 2.0`、`NGVGE`、`Next Generation Visual Game Engine`、`WGPU`、`TWN-5 Offline Certification Assets`、`TWN-6D0L Real Drawable Color Effect Contract`。其中 **TWN-* 为封签资产，任何情况下不得改动**。
- 归档内容为只读快照：`target/` 等构建产物一律不进入归档；如需验证，请在 `output` 副本或新目录下构建。

---

## 7. 环境信息

| 项目 | 值 | 来源 |
|---|---|---|
| 宿主系统 | Windows 10（Build 19045） | 本轮实测 |
| Rust 工具链 | `cargo 1.98.1 (797e8a9bc 2026-08-05)` / `rustc 1.98.1 (48a229cea 2026-09-01)`，host `x86_64-pc-windows-msvc`，LLVM 22.1.8 | 本轮实测 |
| Windows 链接能力 | **可用**：`cargo test --no-run` 成功产出并运行全部测试二进制 | 本轮实测 |
| Python | 3.11.8（依赖守卫脚本依赖 Python 3） | 交接记录 / 脚本可跑 |
| 原生认证目标 | headless **Linux x86_64** + **Mesa llvmpipe** + **wgpu-native v29.0.1.1 C ABI** | 交接记录（本轮未复测） |
| 交叉验证环境 | WSL Ubuntu（Linux x86_64 构建验证），WSL 固定在 2.7.10 | 交接记录（本轮未复测） |
| wgpu-native 资产 | 同级目录 `F:\All NGVGE\WGPU`；后端运行时按 `gpu::locate_library` 候选顺序解析，可用环境变量 `NES_RENDER_WGPU_LIB` 覆盖 | 交接记录 + 源码注释 |
| 产物 workspace | `C:\Users\Administrator\...\workspace\conv_b3458d8493f34d69b1aa7f1a67fa414c`（`output` = 源码产物，`temp` = 中间产物） | 本轮实测 |

---

## 8. 已知踩坑与操作纪律（沿用）

1. **MAX_PATH 限制**：在 `output` 下用 PowerShell `Get-ChildItem -Recurse` 会因 `target\debug\incremental\<hash>\...` 深层路径超 260 字符抛 `PathNotFound`；请改用 Python `os.walk` 并主动裁剪 `target` / `.git`，或直接对 `target` 目录整体跳过。
2. **后台跑 cargo**：必须**重定向 stdout/stderr 到文件**，完成后再读文件尾部；流式读取会被截断并吞掉报错。另注意 PowerShell 会把 cargo 写到 stderr 的进度行当作 `NativeCommandError` 报错，属噪声。
3. **中文控制台乱码**：Windows 控制台按 GBK 解码会把中文 cargo 输出变成乱码（如"失败"→"澶辫触"），排查时只认 ASCII 关键词（`error[`、`EXIT`）。
4. **example 缺失会掩盖真实错误**：`nes-render-wgpu` 的 `examples/` 缺席会让 `--all-targets` 在目标解析阶段就失败，先于 lib 的 2 个错误暴露；核对真实进度请先跑 `cargo build --lib`。
5. **路径以绝对路径为准**：历史记忆里残留的相对路径在不同 cwd 下会失效，一律使用 workspace 全路径。
6. **构建缓存不入档**：5 个 crate 均带 `target/`，归档一律排除；验证构建请在不污染归档的位置进行。

---

## 9. 复现入口

```powershell
# 依赖方向守卫（期望 7/7 PASS，EXIT 0）
cd "F:\All NGVGE\ALL"
python check_dependency_direction.py

# 四个已封口 crate（期望 EXIT 0，分别 75 / 34 / 40 / 42 项全绿）
cd "F:\All NGVGE\ALL\nes-scene";          cargo test; cargo clippy --all-targets -- -D warnings
cd "F:\All NGVGE\ALL\nes-asset";          cargo test; cargo clippy --all-targets -- -D warnings
cd "F:\All NGVGE\ALL\nes-render-api";     cargo test; cargo clippy --all-targets -- -D warnings
cd "F:\All NGVGE\ALL\nes-render-extract"; cargo test; cargo clippy --all-targets -- -D warnings

# 后端 crate（当前期望 EXIT 101，2 个错误：E0583 / E0164）
cd "F:\All NGVGE\ALL\nes-render-wgpu";    cargo build --lib
# 修完 5.1 / 5.2 后应转绿；再跑 example：cargo run --example s41_visual_closure
```

> 提示：上述命令若直接在归档目录执行会生成 `target/` 构建缓存。若希望归档目录保持"纯产物"状态，请先整体复制到临时工作目录再构建。

---

## 10. 归档完成声明

- 本次归档**只做复制与文档撰写**，未修改、未删除任何源码文件，未触碰 `F:\All NGVGE` 下同级其他目录。
- 归档内容与产物源 `output`（排除 `target/`）**逐字节一致**，可作为 S4.1 的接手基线。
- 本文档在 `output` 目录保留同一份副本，便于从 workspace 侧直接查阅。
*（内容由AI生成，仅供参考）*

---

## 11. v1.1 修订（2026-09-30 晚）：S4.1 封口回写

> 本节由 S4.1 封口时追加。§1~§10 保留归档当时的快照原貌（除 §0 结论同步更新外），
> 当期事实以本节与 `NES2.0_M4渲染接入_S4最小可视闭环_封口_v1.md` 为准。

### 11.1 计划事项完成状态（§4 表的回写）

| 计划项 | 结果 |
|---|---|
| 5.1 补 `src/renderer.rs`（5 个公开项） | ✅ 全部落地（`WgpuRenderServer` / `CommandConsumer` / `FrameStats` / `FrameOutcome` / `SpritePipeline`） |
| 5.2 修 `error.rs:102` | ✅ `MapFailed` 改结构体模式并输出 `message`；另补上 Display **整条缺失的 `ConfigMismatch` 分支**（被 E0583 掩盖的既有 E0004） |
| 5.3 写 `examples/s41_visual_closure.rs` | ✅ 实机 PASS（Intel Iris Xe / Vulkan / wgpu-native v29.0.1.1，`driver_errors=0`），PNG 经 System.Drawing 外部交叉验证 |
| 5.4 守卫扩 G8 / G9 / G10 | ✅ `check_dependency_direction.py` 扩至 **10/10** |
| 5.5 出口准则测试 | ✅ `tests/criterion_backend.rs` 7 条（库定位失败、符号缺失探针、目标几何、清屏 + 行对齐、非法命令流、精灵帧 + PNG 回读、清屏色锚点互锁） |
| 5.6 封口回写 | ✅ 封口文档已落盘：`NES2.0_M4渲染接入_S4最小可视闭环_封口_v1.md` |
| 5.7 基线对齐 | ✅ 更正后基线：75 / 34 / **40** / **42** / **15**（wgpu 为新增），守卫 10/10 |

### 11.2 与 §2 快照的关键差异（更正旧记录）

- §2 中"`nes-render-wgpu` lib 构建当前只剩 2 个错误"已失效：修完 renderer.rs 后又暴露
  `error.rs` Display 缺 `ConfigMismatch` 分支、`FrameImage` 缺 `Debug` 等被掩盖的问题，均已修复。
- §2.1 第 4 条"守卫脚本尚未扩到 G8~G10"已解决（10/10）。
- §4.11 的 `lib.rs` 过期"本机无链接器"注释已随封口更正。

### 11.3 运行期实证修正（摘录，全文见封口文档 §3）

十一项，含四项对本仓库既有代码的裁决性修正：读回通道序**实为 RGBA**（`FrameImage`
原 BGRA 假设错误，字段已更名）；图集边长 **64px**（原 256 与 `CELL_PX` 注释自相矛盾）；
`CommandConsumer` 字段声明序决定 GPU 部件析构序（子件先于上下文）；**动态库按进程
生命周期持有**（`FreeLibrary` 卸载后重载会以 0xC000041D 崩溃，"重复装配后端"由此
变回安全操作）。

### 11.4 新增资产

- `wgpu-win/`：wgpu-native v29.0.1.1 release 资产解压落点（`locate_library` 候选 2 约定路径）。
- `nes-render-wgpu/output/s41_visual_closure.png`：最小可视闭环的可视证据。
