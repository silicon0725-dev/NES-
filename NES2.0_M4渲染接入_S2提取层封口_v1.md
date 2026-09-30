---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 19886e2d8bf8a6bde5831a704b5a86f4_810b6fb3b98711f1b172525400248c00
    ReservedCode1: EdXhQuIO+9Gt1tQMml8TtPRxS61AXWN12jznFxJH8ijSyxSxpwTDY4uyXqL1VGy1h06KFNFevFvXlrbYAJ8e+bVBy5xjH6c2k1s330i6NbgsTvvMmbdxpzibKCpVd3SqBvo4Ds+0AC4IEwo5cCvr+IQTW+qyfRs+R9/Q5dOKlaXd1ev/4WWO25Q+Vmo=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 19886e2d8bf8a6bde5831a704b5a86f4_810b6fb3b98711f1b172525400248c00
    ReservedCode2: EdXhQuIO+9Gt1tQMml8TtPRxS61AXWN12jznFxJH8ijSyxSxpwTDY4uyXqL1VGy1h06KFNFevFvXlrbYAJ8e+bVBy5xjH6c2k1s330i6NbgsTvvMmbdxpzibKCpVd3SqBvo4Ds+0AC4IEwo5cCvr+IQTW+qyfRs+R9/Q5dOKlaXd1ev/4WWO25Q+Vmo=
---

# NES 2.0 · M4 渲染接入 S2「提取层最小闭环」封口报告 v1

- 日期：2026-09-26
- 状态：**S2 出口候选（本文档即封口证据）**
- 依据：《`m4_render_borrow_research.md`》第 4 节 方案 D 阶段表（S2 行）、《`NES2.0_M4渲染接入_S1契约冻结_v1.md`》（冻结面 I1~I10）
- 交付根目录：`C:\Users\Administrator\AppData\Roaming\Tencent\Marvis\User\90EA32E1CDA20B32D089B53E8D26FB7F\workspace\conv_b3458d8493f34d69b1aa7f1a67fa414c\output`（下文相对路径均以此为根）
- 实测环境：Windows 10 (19045) / cargo 1.98.1 / rustc 1.98.1

---

## 0. 一句话结论

提取层 `nes-render-extract` 已落盘并通过本轮收尾实测：`cargo test --all-targets` **29 项全过（5 单测 + 24 集成）、0 失败、EXIT=0**；`cargo clippy --all-targets -- -D warnings` **零警告零错误、EXIT=0**；依赖方向守卫 **7/7 PASS、EXIT=0**（新增 G6/G7 覆盖本 crate）。三条 S2 出口准则（变换传播一致 / z 序稳定 / 节点增删无泄漏，含资源消失与资源换代）逐条有对应用例钉住；**无阻塞级缺陷**；契约层 `nes-render-api` 在本阶段零改动（文件时间戳证据见 §2.4）。残留 2 项文档/文案级缺陷与 2 项待上游拍板的裁决问题，均不阻塞出口。

---

## 1. S2 出口准则逐条对照

调研报告第 4 节原文：

> **S2 · 内容**：提取层最小闭环：遍历 → create_item/set_transform/set_z/set_visible，NodeId↔ItemHandle 生命周期（含节点删除/资源消失）
> **S2 · 出口准则**：集成测试 `criterion_*`：变换传播一致、z 序稳定、节点增删无泄漏

| # | 出口准则 | 本层落地方式 | 钉住用例（`tests/criterion_extract.rs`） | 判定 |
|---|---|---|---|---|
| 1 | 遍历 → 属性级推送 | `extractor.rs::extract_into`：`refresh_transforms` → `collect_order`（非递归前序、缓冲预分配）→ 逐节点 复用/换代/新建 → `set_transform`/`set_flip`/`set_z`/`set_visible` → `retain_seen` → `submit_into` | `bookkeeping` 组 6 项 | ✅ |
| 2 | 变换传播一致 | 推的是 `tree.world(node)`（场景层冲洗后的世界矩阵），本层**不**自乘父子链，无第二处可漂移 | `transform` 组 5 项：`transform_matches_scene_world_cache`、`transform_matches_scene_recompute`、`transform_follows_parent_change_next_frame`、`transform_ignores_non_renderable_nodes`、`flip_not_folded_into_transform` | ✅ |
| 3 | z 序稳定 | `z` 取 `Node2D::z_index` 属性；`order` 取场景层 `NodeData::order`（子序键），二者构成契约层 `DrawKey(z, order, handle)` 全序，不依赖任何容器迭代顺序 | `z_order` 组 6 项：`draw_order_matches_scene_key_order`、`draw_order_z_index_grouping`、`draw_order_idle_frames_unchanged`、`draw_order_move_child_takes_effect_without_recreate`、`draw_order_reproducible_across_sessions`、`order_semantics_is_scene_sibling_key` | ✅ |
| 4 | 节点增删无泄漏（含资源消失 / 资源换代） | `map.rs::NodeItemMap`（`BTreeMap<NodeId, ItemSlot>` + `seen_frame` 标记 + `retain_seen` 清扫，句柄不复用）；换代走 `destroy_item` + `create_item` | `lifecycle` 组 7 项：`add_node_creates_item_next_frame`、`remove_node_destroys_item_no_leak`、`remove_subtree_sweeps_descendants`、`keep_children_removal_only_sweeps_removed_node`、`resource_disappear_destroys_item`、`resource_rebind_creates_new_handle_no_alias`、`long_churn_keeps_map_and_server_in_sync` | ✅ |
| 5 | （超额）记账自洽 | `ExtractStats` 三条不变式（`created = fresh + rebound`、`pushed = fresh + rebound + reused`、`destroyed = rebound + dropped + swept`）逐帧断言 | `stats_consistent_every_frame`、`no_recreate_on_stable_tree` | ✅ |
| 6 | （超额）热路径不每帧分配 | 遍历缓冲、输出缓冲跨帧复用；`ScratchStats` 可观测 | `scratch_buffers_do_not_grow`、`out_buffer_reused_across_frames`、`out_buffer_holds_full_snapshot` | ✅ |
| 7 | （超额）生产资源源可用 | `RenderKeySource` trait 隔离 `ResourceTable`，提取层不直接依赖 `nes-asset` | `production_source_reads_resource_table` | ✅ |

集成用例分布：`transform` 5 + `z_order` 6 + `lifecycle` 7 + `bookkeeping` 6 = **24 项**；另 crate 内单测 5 项（`bridge` 4 + `extractor::stats_consistency_rule` 1）。

---

## 2. 实测结果（本轮收尾重跑，读取日志尾部与追加退出码）

### 2.1 `cargo test --all-targets`

- 命令：`cargo test --all-targets`（cwd = `output/nes-render-extract`）
- 结果：**EXIT=0**

| 目标 | 结果 |
|---|---|
| `unittests src\lib.rs` | **5 passed; 0 failed; 0 ignored** |
| `tests\criterion_extract.rs` | **24 passed; 0 failed; 0 ignored** |

单测 5 项：`bridge::tests::{affine_bridge_copies_every_field, flip_bridge_maps_props, render_key_bridge_is_bit_copy, affine_bridge_keeps_identity}`、`extractor::tests::stats_consistency_rule`。

### 2.2 `cargo clippy --all-targets -- -D warnings`

- 命令：`cargo clippy --all-targets -- -D warnings`
- 结果：**EXIT=0**，输出仅 `Checking nes-render-extract` + `Finished`，**零警告、零错误**（`-D warnings` 生效下通过 = 无 lint 债务）
- crate 内策略：`#![forbid(unsafe_code)]` / `#![deny(missing_docs)]` / `#![deny(rust_2018_idioms)]`

> 说明：日志中形如 `NativeCommandError` 的行是 PowerShell 5.1 对原生程序 stderr 的包装记录，**不是**编译器诊断；全日志无 `warning:` / `error[` 记录。

### 2.3 `check_dependency_direction.py`

- 命令：`python check_dependency_direction.py --root <output> --json <temp>/s2_final_guard.json`
- 结果：**EXIT=0，7/7 PASS**

| 守卫 | 结论 | 实测细节 |
|---|---|---|
| G1 | nes-scene 依赖树不含 `nes-render-*` | 传递依赖 1 个：`nes-asset` |
| G2 | nes-asset 依赖树不含 `nes-render-*` | 传递依赖 0 个 |
| G3 | `nes-render-api` 零依赖 | 声明依赖 0 条 |
| G4 | `nes-render-api` 不依赖 scene/asset | 传递依赖 0 个 |
| G5 | 场景层/资源层源码不出现 `nes_render` 符号 | 扫描 25 个 `.rs/.toml`，命中 0 处 |
| **G6** | `nes-render-extract` 直接依赖仅限 `nes-scene` / `nes-render-api`（path），源码不直引 `nes-asset` | 声明依赖 2 条（均 path）；传递依赖 3 个（scene 带来的 asset 属上下游既有单向依赖）；源码扫描 6 个 `.rs`，`nes_asset` 直引 0 处 |
| **G7** | scene / asset / api 均不（直接或传递）依赖 `nes-render-extract` | 三者反向传递依赖均 0 |

负向注入复验（本阶段前序已完成，日志留存）：在 `Cargo.toml` 加 `nes-asset` 直依 + `source.rs` 加 `use nes_asset` → **G6 FAIL**；还原后 **7/7 PASS**。证明 G6 不是"恒过"的空守卫。

### 2.4 契约层冻结面未被改动（证据）

- `nes-render-api` 全部文件（`Cargo.toml`、8 个 `src` 模块、`tests/criterion_contract.rs`）最后写入时间 **≤ 2026-09-26 02:44:31**（S1 会话时段）；`nes-render-extract` 全部文件为 **11:32**（S2 时段）。二者时间戳不交叠 → S2 期间未触碰契约层文件。
- G3/G4 在本轮实测中持续 PASS，契约层仍为零依赖。
- 结论：I1~I10 的冻结签名、类型与算式**一个都没动**。

### 2.5 日志落盘位置（中间产物，可复核）

| 日志 | 路径（均位于 temp） | 说明 |
|---|---|---|
| cargo test | `s2_final_test.log` | 末尾追加 `EXIT=0` |
| cargo clippy | `s2_final_clippy.log` | 末尾追加 `EXIT=0` |
| 依赖守卫 | `s2_final_guard.log` / `s2_final_guard.json` | 7/7 PASS，末尾追加 `EXIT=0` |
| 负向注入复验 | `dep_guard_negtest_注入后.log` / `dep_guard_negtest_还原后.log` | G6 可失败性证据 |
| 重跑脚本 | `s2_final_run.ps1` | 本轮三项命令的可复现脚本 |

---

## 3. 真缺陷清单

| # | 缺陷 | 性质 | 处置 | 是否阻塞出口 |
|---|---|---|---|---|
| D1 | 集成测试初版**编译失败**：同表达式内对 `SceneTree` 双重借用（`add_sprite(tree, tree.root(), …)` 形态）；另有未使用的 `use` 项 | 测试代码缺陷（Rust 借用检查 / lint），非实现缺陷 | 已修：新增 `add_root_sprite` / `add_root_container` 辅助函数，先取 `root()` 再借 `&mut`；清理未用 import | 否（已修） |
| D2 | 测试初版三处**断言语义与场景层真实语义不符**：① `nodes_visited` 应含根节点；② 多个渲染物共用同一 `ResId` 键、该键消失时应"同键同退"（`dropped` 计数语义）；③ `move_child` 会触发场景层集中重写兄弟 `order` 键（`nes-scene/src/tree.rs` L1272-1280） | 测试侧假设错误（对场景层语义理解偏差），非实现缺陷 | 已修：按真实语义重写断言，并**新增收紧用例** `criterion_extract_order_semantics_is_scene_sibling_key` 把该语义钉死。**未删除任何用例、未放宽任何断言**（集成用例数 24，与初版规划一致） | 否（已修） |
| D3 | `extractor.rs` L33-35 与 `lib.rs` 模块注释称 `order` 为"前序遍历序号（即场景层 `NodeData::order`，'第几个被遍历到'）"，与实现不符：`NodeData::order` 实为**子序键**（`next_order()` 单调分配、重排时集中重写，见 `tree.rs` L6 / L432 / L1039-1041 / L1272-1280），并非全局前序序号 | **文档级真缺陷**（代码取值 `data.order` 正确，仅注释表述失准） | 本轮**不改源码**（改动会导致刚完成的实测日志失效、需整套重跑），登记 S5 文档回写时修正措辞 | 否 |
| D4 | `check_dependency_direction.py` 的 G6 明细文案 "合法直接依赖白名单：nes-scene, nes-render-api（**ne-asset** 不在其中）" 少一个 `s`，应为 `nes-asset` | 文案错别字（低影响） | 不影响判定逻辑与结果，登记后续统一修字 | 否 |

**阻塞级缺陷：0 项。**

---

## 4. 与 S1 契约 I1~I10 的偏差

判定口径：S2 只允许**调用**契约层，不得改形。下表逐条核对提取层的落点。

| 不变量 | S2 落地 | 判定 |
|---|---|---|
| I1 空/未知句柄操作被静默忽略 | 本层从不构造 `ItemHandle::NIL`（`NodeItemMap` 仅在"节点为 `Sprite2D` 且资源键非空"时 `create_item`）；忽略语义由契约层负责 | **无偏差** |
| I2 句柄永不复用 | 键不变 → 复用既有句柄；资源换代 → `destroy_item`(旧) + `create_item`(新)，句柄由契约层分配，本层从不手工拼句柄；`NodeItemMap` 写入口限 crate 内 | **无偏差** |
| I3 `submit_into` 先清空 `out` | 本层不触碰 `out` 旧内容，直接把 `&mut out` 透传给 `server.submit_into` | **无偏差**（用例 `out_buffer_reused_across_frames`、`out_buffer_holds_full_snapshot`） |
| I4 事件一次性 + 每帧全量快照 | 稳定树上空闲帧 **0 create / 0 destroy**（`no_recreate_on_stable_tree`），每帧对每个渲染物全量 `set_*` | **无偏差** |
| I5 属性流按 `DrawKey(z → order → handle)` 升序 | 本层只提供 `z` 与 `order` 原料，排序由契约层 `submit` 内部完成；`z_order` 组以 `server.draw_order()` 与场景层 `(z_index, order, handle)` 排序结果逐项比对 | **无偏差** |
| I6 帧内顺序固定 | 本层推送顺序为 `set_transform` → `set_flip` → `set_z` → `set_visible`，落在 I6 "各渲染物属性段"内；`SetCamera` / `SetText` / `SetRect` 三段本层**不产出**（相机/Label/Control 属 S3 缺口补齐范围） | **⚠️ 部分覆盖（范围界定，非偏差）**，见 §5 Q-B |
| I7 `apply_item` 与逐项 setter 不可区分 | 本层走逐项 setter，未使用 `apply_item`，与 I7 无冲突 | **无偏差** |
| I8 `world_transform() == transform ∘ flip` | `flip` **不**折进 `transform`（`flip_not_folded_into_transform`）；推的 `transform` 与场景层 `world` 缓存逐位一致 | **无偏差** |
| I9 相机不变量 | 相机（`Camera2DState`）属 S3 范围，本层未触碰 | **不适用（范围外）** |
| I10 契约层零依赖、零 unsafe | 本层 `forbid(unsafe_code)`、仅 2 条 path 依赖、无第三方 crate（G6 实测）；契约层文件时间戳未变（§2.4）；G3/G4 持续 PASS | **无偏差** |

### 需裁决问题（待上游拍板，不阻塞 S2 出口）

- **Q-A｜`order` 语义与同 z 绘制次序的耦合**：本层 `set_z` 的 `order` 直接取场景层子序键，故 `move_child` / `sort_children` 会在该帧改变同 z 组的绘制次序（`draw_order_move_child_takes_effect_without_recreate` 已把这个行为钉住）。若产品要求"重排子节点不改变同 z 组的绘制先后"，需在 S3/S4 决策：由场景层提供稳定绘制序键，或提取层改用自己的稳定序号。当前按"已裁决：不为场景层加字段"执行。
- **Q-B｜S2 出口是否应包含相机推送**：本层只推 `transform`/`flip`/`z`/`visible` 四类属性；相机视图矩阵、Label 文本、Control 矩形三段按调研报告属 S3。若上游认为 S2 出口需含相机推送，请裁决（当前判定为 S3 范围，故记为"部分覆盖"而非偏差）。

---

## 5. 产物清单

| # | 产物 | 路径（相对 output） | 大小 | 说明 |
|---|---|---|---|---|
| 1 | crate 清单 | `nes-render-extract/Cargo.toml` | 1336 B | 仅 2 条 path 依赖 + 依赖方向硬约束注释 |
| 2 | crate 根 | `nes-render-extract/src/lib.rs` | 3037 B | 模块总览 + 依赖方向图 + I1~I10 落点说明；`forbid(unsafe_code)`/`deny(missing_docs)` |
| 3 | 类型桥接 | `nes-render-extract/src/bridge.rs` | 3317 B | `Affine→Affine2` 逐字段、资源键位拷贝、`flip_h/flip_v→Flip` |
| 4 | 提取器 | `nes-render-extract/src/extractor.rs` | 15410 B | 每帧 `extract_into` + `ExtractStats` + `ScratchStats` |
| 5 | 身份映射 | `nes-render-extract/src/map.rs` | 5131 B | `NodeItemMap`（`BTreeMap` + `seen_frame` + `retain_seen`） |
| 6 | 资源键来源 | `nes-render-extract/src/source.rs` | 2180 B | `RenderKeySource` trait 隔离 `ResourceTable` |
| 7 | 出口准则测试 | `nes-render-extract/tests/criterion_extract.rs` | 51307 B | 24 项集成用例（transform 5 / z_order 6 / lifecycle 7 / bookkeeping 6） |
| 8 | 依赖守卫（扩展） | `check_dependency_direction.py` | — | 新增 G6/G7 覆盖 `nes-render-extract` 分层规则 |
| 9 | 本报告 | `NES2.0_M4渲染接入_S2提取层封口_v1.md` | 本次新增 | S2 封口证据（准则对照 / 真缺陷 / I1~I10 核对 / 产物清单） |

未产出/未触碰：契约层 `nes-render-api/*`（S1 冻结物，本阶段零改动）、GPU/窗口/后端实现（S4）。

---

## 6. 对 S3 的接续约束（本阶段直接后果）

1. **契约面仍是调用方**：S3 补相机 / Label / Control / flip 四缺口时，只在提取层新增推送段，**不得**改 `nes-render-api` 任何签名（走 S1 文档第 10 节变更纪律）。
2. **依赖边不得新增**：提取层直接依赖仍只能有 `nes-scene` / `nes-render-api` 两条（G6 看守），`nes-asset` 只能经 `RenderKeySource` trait 间接使用。
3. **`order` 语义保持现状**：在 Q-A 裁决前，`set_z` 的 `order` 继续取场景层子序键，不得在本层私自换序。
4. **日志与守卫须随阶段扩展**：新增推送段后需同步扩展 G6/G7 覆盖面与 `criterion_extract_*` 用例，并重跑三项验证落盘。
*（内容由AI生成，仅供参考）*
