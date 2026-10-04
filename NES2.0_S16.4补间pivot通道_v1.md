# NES 2.0 · S16.4 补间 pivot 通道 v1

## 0. 结论

补间系统照 alpha 通道的同款模式新增第 4 条语句通道 `tween_pivot`（第 5 个
`TweenChannel` 变体）：把 Sprite2D 的 `pivot` 属性（S16.3 引入的归一化锚点，
Vec2）从当前值按缓动/模式推向终点。推进走**既有属性写路径**（`set_prop`，
进语义指纹，与 alpha/frame 同口径）；**零渲染改动** —— 提取层本就每帧直读
`pivot` 属性推 `SetPivot`（S16.3 既有路径），补间写属性后渲染自动跟随。
门禁全绿：十 crate `cargo test --release` 767 通过（基线 764 + 新增 3）、
clippy `-D warnings` ×10 零告警、依赖方向守卫 15/15、tween_demo / frame_demo
冒烟正常退出。既有三通道（pos/scale/alpha）与 frame 通道逐位不变
（无补间场景指纹零混入的口径未被触碰）。

## 1. 通道全景

| 通道 | 语句（形态） | 写入面 | 写入路径 | 引入 |
|---|---|---|---|---|
| pos | `tween_pos "name" x y ms [easing] [mode]` | `Transform2D.pos` | `set_local`（脏标记） | S16 第 1 期 |
| scale | `tween_scale "name" sx sy ms [easing] [mode]` | `Transform2D.scale` | `set_local`（脏标记） | S16.1 |
| alpha | `tween_alpha "name" a ms [easing] [mode]` | Sprite2D `alpha`（f32，落地夹取 0..1） | 属性写（进指纹） | S16.1 |
| frame | `tween_frame "name" from to ms [easing] [mode]` | Sprite2D `frame`（i64，线性插值 floor） | 属性写（进指纹） | S16.2 |
| **pivot** | `tween_pivot "name" px py ms [easing] [mode]` | Sprite2D `pivot`（Vec2 归一化） | 属性写（进指纹） | **S16.4（本期）** |

统一语义（全部通道共用）：last-wins 按（节点，通道）二元组；`from` 在 Cmd
落地时采样当前实际值；缓动 te = ease(t)；once/yoyo/loop 模式；完成发
`tween_done`（载荷 = 节点名，loop 永不到站）；登记表进语义指纹（条件混入，
非空才摺进）；`tween_stop "name"` = **全部通道**一并停（Pivot 臂天然包含，
 retain 按 target 不分通道，无需改码 —— 测试钉住）。

### pivot 属性写路径与渲染跟随

- **登记**：`Cmd::TweenPivot` 落地时读目标当前 `pivot` 属性作 `from`
  （缺失按缺省 `(0,0)`，与 schema 缺省及渲染"无记录 = 无平移"同口径）；
  px/py 越界照实接受不钳制（S16.3 越界锚定 = 锚点落在精灵外的合法语义）。
- **推进**：`apply_tween`/`land_tween` 的 Pivot 臂 x/y 按同一插值量各自推进
  （照 scale 的形状），经 `set_prop(id, "pivot", Value::Vec2)` 写入 ——
  真实树状态，进语义指纹；目标非 Sprite2D 时写入静默无效（照 SetProp 口径）。
- **渲染跟随（零新路径）**：提取层 S16.3 既有逻辑每帧直读 `pivot` 属性
  —— 非 `(0,0)` 逐帧推 `SetPivot`、`(0,0)→非(0,0)` 与 `非(0,0)→(0,0)`
  迁移补推由 `pivot_active` 簿记负责。补间只写属性，渲染自动跟随；
  nes-scene / nes-render-extract 本期均无渲染侧代码改动。

## 2. 契约

| 编号 | 契约 | 落点 |
|---|---|---|
| T-TP-01 | 解析产物形状（px/py/ms 三表达式 + 可选缓动/模式尾缀）、字面量 ms <= 0 解析期报错、保留字；登记 from = 当前 pivot（缺省 (0,0)）；中点 (0.5,0.5)@t=0.5 -> **pivot 属性读面 = (0.25,0.25)**；完成落 (0.5,0.5) + 到站信号；终点 (-0.25,1.5) 越界照实接受不钳制 | `nes-scene/tests/s16_tween.rs::t_tp_01_*` |
| T-TP-02 | yoyo 去回往返（p=1 到 to 换向不落位、p=2 回零落 from + 移除 + 信号恰一次）；last-wins 中途换程 from = 含本帧推进的当前 pivot（不跳变、不叠加、elapsed 归零）；`tween_stop` 全通道含 pivot（pivot+pos 并存一并停、各停当前值） | `nes-scene/tests/s16_tween.rs::t_tp_02_*` |
| T-TP-03 | 渲染跟随（extractor 侧，全链：编译 -> VM 登记 -> 树推进写属性 -> 提取）：推进中 pivot 属性 (0.25,0.25) 非 (0,0) -> **SetPivot 被推**（`srv.pivot_of` 断言，照既有 alpha 簿记断言形态）；时满落位 (0.5,0.5) 渲染照收；缺省帧零 SetPivot | `nes-render-extract/tests/criterion_extract.rs::tween_pivot_render_follows_set_pivot` |

脚本面：`tween_pivot` 照 `tween_scale` 同构双表达式语句（弹序 ms、py、px），
可选缓动/模式尾缀、保留字、ms 解析期校验全同既有通道；`tween_stop` 零改动。

## 3. 门禁（worktree wt-tp，HEAD b721e36）

| 门禁 | 结果 |
|---|---|
| `cargo test --release` ×10 crate（逐个构建） | 全绿，合计 **767**（基线 764 + 新增 3：asset 34 / render-api 47 / scene 259 / audio 52 / media 27 / extract 60 / wgpu 136 / ext-api 7 / ext-js 29 / runtime 116） |
| `cargo clippy --release --all-targets -- -D warnings` ×10 | 零告警 ×10 |
| 依赖方向守卫 `check_dependency_direction.py` | **15/15**（首版文档注释命中 G5 的 `nes_render` 符号扫描，已改写为不含 crate 符号的表述） |
| tween_demo 冒烟（`NES_GAME_FRAMES=180`） | 正常退出 |
| frame_demo 冒烟（`NES_GAME_FRAMES=180`） | 正常退出 |

## 4. 遗留

- **暂停门控**：补间推进不受暂停门控（S16 冻结的 v1 口径），pivot 通道沿用；
  若后续做"暂停冻结补间"，五通道一并动，无 pivot 特例。
- **pivot 非有限值**：与 scale 通道同口径（不做运行时非有限拒收；解析期
  字面量非有限会被词法层拒）。alpha 的 `to.clamp(0..1)` 刻意不适用于 pivot
  （越界锚定是合法语义）。
- **编辑器补间面**：编辑器属性面板对 pivot 的展示/编辑归 S16.3 既有面；
  补间登记表在编辑器侧的可视化（时间轴一类）仍归后续里程碑。
- **`tween_demo` 未演示 pivot**：现 harness 只走 pos 对比手写步进；pivot
  观感验证由 T-TP-03 的提取侧契约与 S16.3 的渲染测试覆盖，demo 扩展归后续。

## 5. 命中测算 pivot 联动（S16.5 追记）

### 5.1 问题：渲染与命中分叉

S16.3 之后精灵渲染经 pivot 平移（`world ∘ translate(-pivot * 16px)`，局部
内层平移），而脚本 `hit(..)`（`Op::Hit`）的命中盒仍是**固定 16x16 轴对齐盒
锚在 `world.tx/ty`** —— pivot (0.5,0.5) 的精灵画在中心锚定位，点它中心却
miss。命中盒原点据此改为「世界矩阵映射 pivot 平移后的锚点角」。

### 5.2 几何定义（单点助手）

```
origin = world.apply((-pivot.x * 16, -pivot.y * 16))
```

- **落点**：`SceneTree::sprite_hit_origin(id)`（`nes-scene/src/tree.rs`，
  公共方法）—— 全库唯一一份命中几何。三个消费方全部收敛到它：
  1. 脚本 `hit(..)`（`Op::Hit`，`nes-scene/src/script.rs`）；
  2. 编辑器宿主**点击选择**（`nes-runtime/examples/editor_shell.rs` ——
     原本就有第二份"与 hit 同逻辑的 Rust 版"，本次收敛；回归面排查结论：
     编辑器点击不走脚本 `Op::Hit`，是宿主自算，已收敛到同一助手）；
  3. 编辑器**框选探针**（同文件，原 `(tx+8, ty+8)` 即旧盒中心 —— 与命中
     盒同源的几何，pivot 下会与点击选择打架：点得中、框选罩不住 ——
     一并收敛为 `sprite_hit_origin + (8, 8)`）。
  Gizmo 拖拽偏移 / 选中指示框用的是 `world.tx/ty` 的**位置语义**（非命中
  面），不在本次口径内，未动。
- **仿射入口**：nes-scene 仿射类型的点变换入口是
  `Affine::apply(Vec2) -> Vec2`（`transform.rs`；`world ∘ rhs` 复合是
  `Affine::mul`）。世界仿射含父链/旋转/缩放。
- **旋转近似口径（不变）**：旋转/缩放下继续按轴对齐盒判定是**既有契约的
  近似** —— S10 命中口径本就是固定轴对齐 16 盒、忽略旋转；本次只把锚点角
  从裸 `(tx, ty)` 换成映射后的真实角点，近似口径保持不变、不做旋转 OBB。
- **pivot 缺省语义**：属性缺失/类型错/含非有限分量一律按缺省 `(0,0)`
  （与 schema 缺省、提取层 `sprite_pivot` 同口径）。此时 `apply((0,0)) ==
  (world.tx, world.ty)` —— **无 pivot 精灵命中几何逐位同旧口径**，基线哈希
  预期不动（dodge 场景无 pivot 属性，`t_abi_01_dodge_baseline` 原样绿即证）。

### 5.3 契约

| 编号 | 契约 | 落点 |
|---|---|---|
| T-HP-01 | 无 pivot（缺省）精灵 (32,32)：hit(33,33) 命中（含节点身份比对）、hit(31,33) miss —— **现状回归**；助手级断言 `sprite_hit_origin == (32,32)` 逐位 | `nes-scene/tests/s16_5_hit_pivot.rs::t_hp_01_no_pivot_default_box` |
| T-HP-02 | pivot (0.5,0.5) 同精灵（盒平移到 [24,40)²）：hit(36,36) 命中；hit(40,40) miss（半开终点；**旧口径此点命中 —— 分叉修复的证据**）；hit(25,25) 命中（旧口径 miss 区 —— 「点所见即所得」）；hit(33,33) 命中（点画出来的中心必须中）；助手级 origin == (24,24) | `nes-scene/tests/s16_5_hit_pivot.rs::t_hp_02_pivot_half_center_box` |
| T-HP-03 | pivot (1,1)（盒移到 [16,32)²）：hit(17,17) 命中；hit(32,32) miss（旧盒起点 = 新盒半开终点）；hit(15,17) miss；助手级 origin == (16,16) | `nes-scene/tests/s16_5_hit_pivot.rs::t_hp_03_pivot_one_full_shift` |
| T-HP-04 | pivot 属性类型错（`set_prop_raw` 塞字符串 / I64 / NaN Vec2）→ 缺省 (0,0) 回归、不 panic | `nes-scene/tests/s16_5_hit_pivot.rs::t_hp_04_bad_pivot_type_falls_back` |

### 5.4 门禁（worktree wt-hitpivot，HEAD 97bf34c）

| 门禁 | 结果 |
|---|---|
| `cargo test --release` x10 crate（逐个构建） | 全绿，合计 **771**（基线 767 + 新增 4：scene 263 / runtime 116 / asset 34 / render-api 47 / extract 60 / wgpu 136 / audio 52 / media 27 / ext-api 7 / ext-js 29） |
| `cargo clippy --release --all-targets -- -D warnings` x10 | 零告警 x10 |
| 依赖方向守卫 `check_dependency_direction.py` | **15/15** |
| `t_abi_01_dodge_baseline` | 原样绿（基线哈希不动） |
