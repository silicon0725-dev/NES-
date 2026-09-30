# NES 2.0 · S6.10 结构性覆盖 v1

> 交付日期：2026-10-01　｜　状态：**实例内增删节点成为覆盖记录（add / remove）**
> 前置：S6.8 实例级属性覆盖、S6.9 diff 式回写。Godot "editable children"
> 语义的最小口径：**按节点**的结构覆盖，不是整实例展开内联。

---

## 0. 一句话结论

`InstanceOverride` 扩展两个结构性字段：`add: Vec<NodeDoc>`（在该节点下追加
父场景拥有的子树）与 `remove: bool`（移除该节点整棵子树），与字段覆盖
（local/process_mode/props）同一条管道 —— 解析/写出/应用/diff/热重载全部
复用。diff 式回写随之升级：参照独有的节点生成 `remove` 记录、当前独有的
节点按父路径聚合成 `add` 记录（子树用 `node_to_doc` 全量导出）。出口准则
T-Ovr-07..09 + T-Scene-07 全过（含"运行时加节点 -> 烘焙 -> 存盘 -> 全新
运行时加载复现"的端到端像素证据）。全仓测试 **100** / 34 / 42 / 40 / 83 /
**22** 全绿，守卫 11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 形态与归属

```ron
overrides: [
    Override(path: "holder", add: [
        Node(name: "extra", kind: "Sprite2D", local: (...), props: { ... }, children: []),
    ]),
    Override(path: "holder/sprite", remove: true),
]
```

- `add` 的节点（含后代/属性/变换）是**父场景文件事实**，`Resource(n)` 用
  父槽位编号；`remove` 移除整棵子树（`keep_children: false`）；
- 同一记录 `remove` 与 `add` **互斥**（解析层报语义错误）；
- 结构覆盖与字段覆盖同管道：热重载（重读父文件 + 重展开 + 重应用）天然
  保留；记录镜像在 `NodeData.overrides` 上随 `save_scene` 写出。

### 1.2 应用次序（结构落地的两段式）

`add_node` 与 `TreeOp::Remove` 都是**延迟队列操作**，而覆盖的路径解析基于
当前结构 —— 因此实例化次序是：构建（存记录）-> `apply_pending`（展开子树
落地）-> 应用覆盖（字段即时写；remove 入队、add 经 `build_child` 占位入队）
-> **再 `apply_pending`**（结构覆盖落地）-> 变换冲洗。对被移除节点内部的
后续记录路径：解析时该节点仍在（移除未落地），属未定义组合 —— 口径是
**别这么写**（文件作者的责任，引擎不为此发明二次校验）。

### 1.3 diff 的结构化（S6.9 的自然延伸）

- 参照独有的节点 -> 该路径一条 `remove` 记录；
- 当前独有的节点 -> 按父路径聚合**一条** `add` 记录，子树用 `node_to_doc`
  全量导出（属性/变换/后代/嵌套，一次性）；
- **重命名自 S6.11 起识别为 `rename` 记录**（保留跟踪，见
  `NES2.0_S6重命名覆盖_v1.md`；本行系 S6.10 当时口径的更正）；
- 与字段记录同列表全量重生成：自清洁、幂等口径不变。

### 1.4 边界（刻意不做）

- 追加目标必须在**参照子树内**（含根 `""`）；往包装节点下、与展开根并列
  添加兄弟不可表达（写回边界会丢弃它们）；
- `remove` 不保留子节点（`keep_children: false`）；想"移除父留子"应分别
  remove 父 + add 子；
- add 的节点若与子场景更新后的同名节点相撞，落地时按既有规则自动加后缀
  （`Sprite2D` -> `Sprite2D2`），不报错。

## 2. 实现落点

| 位置 | 改动 |
|---|---|
| `scene_io::InstanceOverride`（自 tree.rs 迁入） | `add` / `remove` 字段；迁址原因：`add` 装 `NodeDoc`，本质是序列化形态（条目层面无模块环） |
| 解析器 | `add: [Node(...)]`（复用 `node_list`）/ `remove: true|false`；互斥校验 |
| 写出器 | 记录内按序 `remove` 先、`add` 块最后收尾（嵌套 `write_node` 递归） |
| `apply_overrides` | remove -> `queue(TreeOp::Remove)`；add -> `build_child`（复用构建路径） |
| `build_tree` | 覆盖应用后**二次** `apply_pending`（结构覆盖落地） |
| `diff_override_node` | 双向未配对 -> remove / add 记录（`node_to_doc` 全量导出） |

nes-runtime **零改动**（`sync_overrides` / `load_scene` / 热重载全部自动获得）。

## 3. 出口准则

### 3.1 场景层（`s6_override.rs` 追加，3/3）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Ovr-07 | 应用：extra 落在 holder 下（属性/变换保真）、sprite 整棵移除、记录镜像在树上可读回 | ✅ |
| T-Ovr-08 | diff：删 sprite -> remove 记录；holder 下新增（含子节点）-> add 记录（子树全量导出）；干净实例无记录 | ✅ |
| T-Ovr-09 | 回写往返：add/remove 写出、回读相等；再展开实例化结构复现 | ✅ |

### 3.2 运行时端到端（`criterion_scene_loop.rs` 追加，1/1）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Scene-07 | 磁盘 add/remove 记录加载渲染（移除精灵消失、追加精灵入画，纹理去重后单张）；运行时实例内加 `late` -> 烘焙存盘（文件含 add 记录）-> 全新运行时加载**双精灵复现、移除保持** | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 重命名覆盖（保留节点身份的结构改写） | ✅ S6.11（见 `NES2.0_S6重命名覆盖_v1.md`） |
| add 节点与子场景同名相撞的后缀化审计（编辑器提示） | 未启动（引擎侧自动后缀） |
| 仓库化（GitHub NES-） | 本轮起建立（见提交历史） |
| SignalBus / WM_SIZE / DPI / headless Linux | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 100**（97 -> 100，+T-Ovr-07..09）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  **nes-runtime 22**（21 -> 22，+T-Scene-07）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（InstanceOverride 迁址+扩字段、解析/写出/应用/diff、
  build_tree 二次落地）与新测试；nes-runtime 零改动。

*（内容由AI生成，仅供参考）*
