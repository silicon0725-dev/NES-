# NES 2.0 S19.2 Inspector 重组（对象中心脚本视图 + 三分区）v1

- 分支：`s19-2-inspector-reorg`（wt-insp 独立 worktree）；基线 HEAD `cef2912`（S19.1 顶部菜单栏）
- 改动面：**仅 `nes-runtime/examples/editor_shell.rs`**（壳层件）—— 十 crate 源码零改动、零新依赖
- 蓝图依据：`NES2.0_S19.0编辑器组织蓝图_v1.md` §3.1（对象中心脚本视图）+ §4.3（分区结构）

## 0. 结论

1. Inspector 从两组扩成**对象中心三分区**：Transform（现状行不动，恒为首组）/ Appearance（新只读分区，Sprite2D 选中时 alpha/pivot/frame 三行每帧快照投影，非 Sprite = `(n/a)`）/ Script（升级为对象中心脚本列表：每已挂载 Script 子节点一行 `SCRIPT <basename> <ON|OFF>`，无挂载 = `(no scripts)`；F6 候选/Enter 挂载流原样并入）。
2. 数据模型查证结论（§1）：挂载 = 同一事务 Created（Script **子节点**）+ Modified（`registry_key`，落在**子节点**上）；多个 Script 子节点 schema 上可并存，但编辑器挂载流只在"无 Script 子节点"时新建 —— 编辑器流下每对象至多一个；`enabled` schema 缺省 `true`（挂载即 ON），当前无运行时消费方（显示面属性）。
3. 折叠组 2 → 3：`group_stage` 扩到 3 位（bit0/bit1/bit2 = Transform/Appearance/Script），F7 循环 0..=7，组标题点击翻对应位；折叠标记从显式前缀（`- Transform`）改后缀（`Transform -` / `Transform +`，蓝图 §3.1 示例口径）。
4. 布局连锁：分区 y 改**游标式动态分配**（每组标题 y = 前序分区底缘，正文行数随折叠/选中动态，折叠组 0 行不占位）；改名输入框槽位公式不变（Transform 恒为首组且行数固定，既有动态槽位机制自然跟随）。
5. 契约测试扩展：NES_EDIT_DEMO 新增 4 个滞容闩锁断言（脚本行 ON → E 后 OFF → U 后空态；Appearance 三行快照含 alpha 数值）——**零新增注入步骤**（复用既有帧 20/30/40 的挂载/E/U 沿）；既有断言（挂载/play/stop/reset/折叠/刷新/时间轴 APPLY/IME/音乐/字体/菜单链）全部保持。
6. 门禁：测试 787/787（= 基线，本里程碑零新增测试）、clippy 0 ×10、依赖守卫 15/15、editor_shell 冒烟 120 帧干净 + NES_EDIT_DEMO 420 帧全断言连跑 3 次稳定、first_game(Dodge) 240 帧干净退出。

## 1. 三分区结构与数据模型查证

### 1.1 Script 子节点形态（挂载流实读，`mount_script`）

- **挂载事务**（S12-7 冻结、S17.4 实战同款）：选中不是 Script 节点时，同一事务内
  `Hierarchy::create_child(选中节点, 脚本基名去 .nes 截 12 字, NodeKind::Script)`（Created）
  \+ `Inspector::modify_prop(子节点 uid, "registry_key", "Scripts/spin.nes")`（Modified）。
- **registry_key 在 Script 子节点上，不在父节点**（nes-scene schema：`registry_key` 是 Script 类型专属键，空串 = 未绑定）。
- **多个 Script 子节点可否并存**：schema 不限数量、`attach_all_with_sources` 遍历全部 Script 节点 —— 数据面**可并存**；但编辑器挂载流 `mount_target` 只解析"选中自身或**第一个**直接 Script 子节点"，找不到才新建 —— 编辑器流下每对象**至多一个** Script 子节点。列表投影按"全部子节点"写（数据面如实，不限个数），见 §2。
- **U 卸载** = registry_key 写空串（Script 子节点留存，树形态断言 `kids.len() == 1` 保持）；**E 切换** = 第一个 Script 子节点的 `enabled` 取反。
- **enabled 属性**：schema 缺省 `Bool(true)`（"是否参与脚本调度"）—— 挂载即 ON；当前 nes-scene/runtime 无运行时消费方（grep 仅 schema 定义），P0 是显示面属性 —— ON/OFF 行与 `enabled: Y/N/-` 行都是它的读面。
- U/E 的"首个"语义沿用 `mount_target`：首个 Script 子节点（无论其是否已挂载）—— 多子节点时 U 可能落"子节点 0 未挂载"的说明行，这是既有边界（§5.1）。

### 1.2 三分区结构（分组折叠机制照抄 S12-7）

| 组 | 位 | 标题（展开/折叠） | 正文 |
|---|---|---|---|
| Transform | bit0 | `Transform -` / `Transform +` | name/x/y/z 四行（hud_ins 多行文本，现状不动）+ 改名输入框 |
| Appearance | bit1 | `Appearance -` / `Appearance +` | Sprite2D：`alpha:`/`pivot:`/`frame:` 三行；其余：`(n/a)` |
| Script | bit2 | `Script -` / `Script +` | 脚本列表行 + 挂载流四行（`mount: F6`/候选/`unmount U`/`enabled:`） |

- F7 循环 0..=7（8 态二进制递进）；组标题行点击翻对应位（`group_stage ^= 1 << gi`）+ Output 记 `group {transform|appearance|script} {open|closed}`。
- 标题矩形照旧记 `title_rows`（上一帧投影产出 → 帧首命中，一帧滞后），三组矩形随游标布局每帧刷新。
- 无选中：三组标题 + 两正文 Label + 改名框全部隐藏（Godot 空面板直感，既有口径扩两节点）。

### 1.3 行布局样例（768×432，obj1 = Sprite2D 选中、全组展开、已挂载 spin.nes 且 enabled）

```
Inspector            y=32   ← hud_ins 标题行
Transform -          y=52   ← ins_tf_title
name obj1            y=72   ┐
x 280                y=92   │ hud_ins 多行文本（z 行显示属性表现值，
y 130                y=112  │ 选中高亮写 5 即显 5 —— 既有口径）
z 5                  y=132  ┘
[改名输入框]          y=156..176
Appearance -         y=180  ← ins_ap_title
alpha: 1.00          y=200  ┐
pivot: (0.00,0.00)   y=220  │ ins_appearance 三行快照
frame: 0             y=240  ┘
Script -             y=260  ← ins_sc_title
SCRIPT spin.nes ON   y=280  ┐ ins_script 正文：
mount: F6            y=300  │ 列表行（N 行）+ 挂载流四行
spin.nes             y=320  │（候选名超宽截 11 字 —— 既有预算）
unmount U            y=340  │
enabled: Y           y=360  ┘
```

- 变体：非 Sprite 选中 → Appearance 正文单行 `(n/a)`（2 行组占位）；无已挂载脚本 → 列表单行 `(no scripts)`；折叠组标题 `+` 后缀、正文 0 行不占位（后续组上移）。
- 改名输入框槽位公式不变：`input_y = 12+MENU_H+2×INS_ROW_H + (tf_open?4×INS_ROW_H:0) + 4`（Transform 恒为首组行数固定）—— 既有动态槽位机制，IME 锚点/冒烟点击位（(600,166)）零漂移。
- 最小窗（432 高）下全展开的 Script 正文会越过面板底缘压到时间轴带上 —— S18.1 起既有形态（原 Script 正文同样越界），非本里程碑引入，记录于 §5.5。

## 2. 脚本列表投影与操作

### 2.1 行投影（`script_list_rows`，只读）

- 数据源 = 选中节点的**直接 Script 子节点**全量遍历；**挂载判定 = registry_key 非空**（mount 事务落账面）—— U 卸载后子节点仍在树但从列表消失（空态行顶上）。
- 行格式（全 ASCII）：`SCRIPT <basename> <ON|OFF>` —— basename = 注册键文件基名（与挂载日志同 `base_name` 口径，`Scripts/spin.nes` → `spin.nes`）；ON/OFF = `enabled` 属性（缺省 true = 挂载即 ON）。
- 空态：无已挂载子节点 = 单行 `(no scripts)`（时间轴 `(no tweens on selection)` 同款空态文案纪律）。
- 行宽预算：行首 `SCRIPT ` + 基名 + 状态，典型 14..18 字 —— 超 S12-6 的 11 字位图预算（位图回退模式 16px 等宽下溢出到窗口右缘被裁，TTF 模式 14px 比例字 ≈100px 富余）；与规格行格式取舍记 §5.4。

### 2.2 操作（既有键原样，P0 零新键）

- **F6** 轮换候选 / **Enter** 挂载：mount 事务原样（列表随之多一行 ON）。
- **U** 卸载首个：registry_key 写空串 —— 对应行消失、空态行出现（子节点留存）。
- **E** 切换：首个 Script 子节点 enabled 取反 —— 对应行 ON↔OFF 翻转。
- **P0 不做行级选择差异化**：U/E 固定作用第一个目标，列表行纯只读投影（无行高亮/无行点击）—— 现状与限制记 §5.1。

### 2.3 冒烟断言（NES_EDIT_DEMO，零新增注入）

- 复用既有注入沿：帧 20 Enter 挂载 / 帧 30 E / 帧 40 U —— 三个滞容闩锁窗（22..=29 / 32..=39 / 42..=49）逐帧采样 `ins_script` 文本：`SCRIPT spin.nes ON` → `SCRIPT spin.nes OFF` → `(no scripts)` 各闩一次；退出时三断言。
- F7 折叠（帧 50 起）在全部窗口之后不干扰；Appearance 闩锁窗 12..=28 采样三行快照整体。

## 3. Appearance 只读分区

- **投影面**（`appearance_rows`，只读快照）：Sprite2D 选中时三行
  `alpha: {:.2}` / `pivot: ({:.2},{:.2})` / `frame: {}` —— 全部 S16 系既有属性读面，
  缺省兜底与提取层同款（alpha F32 缺省 1.0 / pivot Vec2 缺省 (0,0) / frame I64 缺省 0；
  缺失/类型错按缺省显示）。演示场景未写三属性，冒烟断言值 = schema 缺省
  （`alpha: 1.00` / `pivot: (0.00,0.00)` / `frame: 0`）。
- **非 Sprite 选中** = 单行 `(n/a)`（本演示的选择路径只产 Sprite 选中，`(n/a)` 分支为数据完备性而写）。
- **只读**：本分区零写路径（无输入框/无拖拽）—— alpha/pivot/frame 的编辑归后续里程碑（§5.3）。
- 每帧现算（投影无状态口径）：脚本补间/运行期改动下一帧即反映，与 Inspector 其余行同纪律。

## 4. 门禁

| 项 | 结果 |
|---|---|
| `cargo test --release` ×10 crate | **787 passed / 0 failed**（= 基线 cef2912 实测 787，本里程碑零新增测试 —— 壳层件；分 crate：asset 34 / audio 52 / ext-api 7 / ext-js 29 / media 27 / render-api 48 / render-extract 65 / render-wgpu 143 / scene 266 / runtime 116） |
| `cargo clippy --release --all-targets` ×10 | **0 警告 ×10** |
| 依赖方向守卫 `check_dependency_direction.py` | **15/15 通过** |
| editor_shell 冒烟：120 帧干净窗 | 干净退出 |
| editor_shell 冒烟：`NES_EDIT_DEMO=1 NES_GAME_FRAMES=420` | 全链路断言通过（新增 S19.2 四断言：脚本行 ON / E 后 OFF / U 后空态 / Appearance 三行快照含 `alpha: 1.00`；既有断言挂载/play/stop/reset/折叠/刷新/时间轴/IME/音乐/字体/菜单链全部保持），**连跑 3 次稳定** |
| first_game(Dodge) 冒烟 | NES_GAME_FRAMES=240 干净退出（回归） |

实现注：冒烟注入时间线**零改动**（最晚注入帧仍 348）—— S19.2 断言全部复用既有动作沿 + 滞容闩锁窗（S12-11 IME 先例），不加重注入粒度抖动风险。

## 5. 遗留（后续里程碑/Q1）

1. **行级选择差异化**：脚本列表行无选中/高亮/点击；U/E 固定"首个目标"（`mount_target` 语义）—— 多 Script 子节点并存时（手量数据面可达，编辑器流不可达）无法指名卸载/切换；行级目标锚定（行→uid 映射 + UiVm 行回调）归后续。
2. **脚本双击编辑跳转**：列表行 → Script 面板/文本编辑器（S6 编辑器脚本面板的 script_panel 例已独立存在）；Inspector 内跳转需面板路由（蓝图 Q1）。
3. **Appearance 编辑**：alpha/pivot/frame 只读；升级可编辑（输入框/拖拽 + 事务落账）与 Transform 行同款改造，归后续。
4. **行宽预算**：`SCRIPT <basename> <ON|OFF>` 行（典型 14..18 字）超 S12-6 的 11 字位图预算 —— 位图回退模式下溢出被窗口右缘裁切（TTF 模式富余）；方案（缩短前缀/动态截基名/面板加宽）未裁决前保持规格格式。
5. **最小窗溢出**：432 高全展开时 Script 正文越过面板底缘压到时间轴带（S18.1 起既有形态，本里程碑行数更多、越界更深）—— 面板滚动/分区自动折叠策略归后续。
6. **enabled 运行时语义**：当前无消费方（attach 不看 enabled）—— ON/OFF 是显示面；运行时按 enabled 跳过调度的语义落地后，本分区即真开关（需要 nes-scene 源码改动，本里程碑零源码纪律不碰）。
