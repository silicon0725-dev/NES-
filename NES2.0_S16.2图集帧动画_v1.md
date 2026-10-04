# NES 2.0 · S16.2 图集帧动画 —— Sprite 子矩形采样 + 帧补间通道

日期：2026-10-03 · 分支：`s16-2-frame-anim`（独立 worktree `wt-sheet`）·
基线：c55f794（S16.1 补间后续）· 依赖 **零新增**（十个 crate 逐个构建；
依赖分层 G1–G15 原样，核心 crate 零第三方纪律不动）

---

## §0 结论

S16.1 之前，Sprite2D 只能显示注册表纹理的**整瓦片**（`sample_info` 全瓦片
uv），逐帧动画只能靠显隐伪装。本期做两件加性扩展，既有语义逐位不变：

1. **图集网格子矩形采样**：Sprite2D schema 增性三 props
   `sheet_cols` / `sheet_rows` / `frame`（缺省全 0 = 整图现状）；
   渲染契约新增 `RenderCommand::SetUv { handle, rect: [f32; 4] }`
   （照 SetTint 先例：server trait 方法 + null/wgpu 同构簿记 + 输出序
   SetTint 之后 + 无记录 = 既有整瓦片采样逐位不变）。
2. **帧补间通道**：`TweenChannel::Frame { from, to }` + 语句
   `tween_frame "name" from to ms ["easing"] ["mode"]` —— 推进按线性插值
   取整（floor）写 `frame` 属性（真实树状态，进语义指纹，与 alpha 同
   口径）；yoyo/loop 照常组合，**loop + frame + 终点越界回绕 = 无缝走路
   循环**；到站信号 `tween_done` 照发（loop 永不到站不发）。

**门禁**：十 crate `cargo test --release` 全绿 **755 / 0 failed**
（基线 745 + 新增 10：render-api 46 含 null SetUv 簿记、render-extract 58
含 sheet 推送、render-wgpu 129 含 T-F 像素 3 条、scene 257 含 T-FR 5 条）；
clippy **0 警告 × 10**；worktree 根守卫 **15/15**；冒烟见 §3.2。
已 git commit（未 push）。

---

## §1 网格与帧索引（SetUv 契约扩展）

### 1.1 schema：三 props（全部加性缺省 = 整图现状）

| 属性 | 类型 | 缺省 | 语义 |
|---|---|---|---|
| `sheet_cols` | I64 | 0 | 图集列数；**0 = 整图模式**（既有行为逐位不变）。带 `Number 0..4096` hint（编辑器滑杆 + schema clamp） |
| `sheet_rows` | I64 | 0 | 图集行数；**0 = 正方形网格（行数 = 列数）**——实现简洁取舍，本文与 schema 文档双处写明。同 hint |
| `frame` | I64 | 0 | 帧索引，**行主序**：`col = frame % cols`、`row = frame / cols`。**无数值 hint**（不钳制）—— 越界回绕是正主通道，见 1.3 |

序列化省略缺省值（RON 往返不携带三键，m2 打包字节确定性测试原样绿）；
语义指纹的属性表**逐键混入**——缺省物化让每个 Sprite2D 多三个键（BTree
名序：alpha / fiber… 中 `frame`、`sheet_cols`、`sheet_rows` 各就各位），
这是 dodge 基线重录的根因（§3.1）。

### 1.2 渲染契约：`RenderCommand::SetUv`

```rust
RenderCommand::SetUv { handle: ItemHandle, rect: [f32; 4] }
```

- **语义**：注册表纹理的**归一化 UV 矩形** `[u0, v0, us, vs]`（0..1，
  相对整张注册纹理）；`[0, 0, 1, 1]` = 恒等 = 既有整瓦片采样。
- **trait**：`RenderServer::set_uv(handle, rect)`；输出序冻结在
  `SetTint` 之后（每渲染物：… → SetRect → SetClip → SetTint → SetUv →
  Submit；null 与 wgpu 两处 submit 严格同序）。属性动作、全量快照、
  同键覆写、销毁随条目消亡、未知句柄静默忽略（契约 I1）。
- **NullRenderServer**：`uvs: BTreeMap` 簿记 + `uv_of()` 只读访问 +
  计数器口径不变。
- **wgpu 后端**：`WgpuRenderServer` 与 `CommandConsumer` 各一张 `uvs`
  表；**注册表精灵分支的 uv_rect 从全瓦片改查此表单处折算**：

  ```text
  最终 uv = [瓦片u0 + r.u0 × 瓦片us, 瓦片v0 + r.v0 × 瓦片vs,
             r.us × 瓦片us, r.vs × 瓦片vs]
  ```

  恒等矩形折算 = 全瓦片**逐位相同**（0.0 偏移 + 1.0 比例都是精确浮点，
  T-F-02 整帧 RGBA 逐位对比钉住）；无记录走原矩形 —— 既有路径逐位不变。
  图集格 / 字形 / 控件路径不查此表（各走既有采样）。

### 1.3 uv 折算方案选型（报告项）

**选了"归一化网格分数（提取层算）+ 渲染侧折算像素"**：

- 帧矩形是均匀网格的**纯分数**：`u0 = col/cols`、`us = 1/cols`（v 同理）
  —— 全程**不需要纹理像素尺寸**。查证结论：提取层对注册表尺寸**不可见**
  （upload 时的尺寸登记在渲染侧 `TextureRegistry`；G6 纪律下提取层的
  `RenderKeySource` 只给键不给尺寸，扩 trait 会把尺寸查询面传染给全部
  实现者）；
- 备选的"像素矩形"方案同样过不去：帧宽 = 纹理宽 ÷ cols，提取层不识纹理
  宽照样算不出像素矩形；
- 因此折算分两半：提取层出归一化分数（`sprite_sheet_uv_rect`，单点实现），
  wgpu 渲染侧拿 `sample_info` 的全瓦片矩形一折（§1.2 算式，单点实现）。
  两半各自只有一个权威算式，归一化/像素换算只在渲染侧发生一次。

### 1.4 网格与回绕口径（schema / 提取层 / 文档同源）

- 网格 = 纹理宽高 ÷ cols/rows；`sheet_rows = 0` 时 = cols 正方形网格；
  负值按 0 处理（= 缺省语义，不另造行为）；
- `frame` 越出 `cols*rows` 时**模运算回绕**（`rem_euclid`，负值同样回绕
  —— 补间 loop 到末帧回 0 的正主通道）；`cols` 溢出防护用
  `saturating_mul`（schema clamp 4096 下不会触达，纯防御）；
- 仅图集模式激活（cols > 0）的精灵才推 `SetUv`；**激活 → 整图迁移帧补推
  一次恒等矩形**（`ItemSlot::uv_active` 迁移标记，照 S12-3 裁剪 Some→None
  补推的同一"全量快照生产者侧义务"）—— 否则跨帧簿记里的陈旧子矩形会
  永久残留。恒等值入库后由服务端全量快照逐帧重发（与 tint 恒等值逐帧
  重发同一口径，渲染侧折算逐位还原，无害）。

---

## §2 帧补间通道（与 tween 系统组合）

### 2.1 语句与登记

```text
tween_frame "name" from to ms ["easing"] ["mode"]
```

- **栈交互**：压序 from、to、ms（编译序 = 源序），弹序 ms、to、from；
  三分量都须数值（I64/F32 提升，帧索引截断取整）；`from`/`to` 是语句
  字面量（帧序是创作意图的一部分）—— **与 pos/scale/alpha 的"from 落地
  采样"刻意不同**，文档写明；
- **登记**：`Cmd::TweenFrame` → `register_tween`（last-wins 按
  （节点，frame 通道）二元组；`duration_ms <= 0` 拒收不落地）；
  `tween_stop` = 全部通道一并停（含 frame）；
- **解析面**：`tween_frame` 入保留字表（RESERVED 22→23）；可选缓动/模式
  与三通道同款（未知名解析期报错附合法名单；缺省 linear / once；
  字面量 ms <= 0 解析期报错）。

### 2.2 推进：线性插值取整（floor）

- 推进阶段（tick 专属阶段 1.75，结构落地后、enter/process 前）：
  `v = from + (to - from) × te`（**f64 域**计算，避免 te 的小数误差在
  整数域抖动），`frame = floor(v)` 经**既有属性写路径**直写 —— frame 是
  真实树状态，进语义指纹（属性表逐键混入，天然覆盖）；
- **once**：时满精确落位 `to` + 移除 + 发 `tween_done`；
- **yoyo**：去程向 to、回程向 from、回零精确落位 `from` + 移除 + 发
  到站信号。注意离散帧的取整边界：折返点 p=1 处 shape = 2−p 带负
  epsilon，`floor(4.999…) = 4` —— `to` 只在 land 时精确写（yoyo 不在
  to 落位，本就该读不到 5，T-FR-02 钉住）；
- **loop**：进度对 1 取模 —— 永不移除、永不到站、到站信号不发（照
  S16.1 口径）。**loop + frame + 终点越界回绕 = 走路循环**：
  `tween_frame "walker" 0 4 600 "linear" "loop"` 在 2x2 sheet 上走出
  0→1→2→3→(回绕)0→… 的无缝循环（frame_demo 即此写法）；
- 目标没有 `frame` 属性（非 Sprite2D）时写入静默无效（照 alpha 同家法）。

### 2.3 指纹与会话态

- **指纹条件混入**：Frame 通道按既有口径摺进 —— 目标 uid + 通道标签
  `"frame"` + from/to 的 **i64 位形**（`to_le_bytes`，与 f32 位形同一
  "精确、无浮点歧义"口径）+ 缓动/模式稳定名 + elapsed/duration 位形
  （`determinism.rs`；T-FR-05 双跑逐位相同 + 含/不含必不同）；
- **会话态**：补间不进 RON 往返（不变）。

---

## §3 门禁与基线重录

### 3.1 基线重录记录（协议：真数据驱动 + 评审）

**漂移根因**：Sprite2D 属性表缺省物化新增 `sheet_cols` / `sheet_rows` /
`frame` 三键 —— 语义指纹的属性表逐键混入，每个 Sprite2D 节点多混三键 →
dodge 基线（含 4 个 Sprite2D）指纹必然漂移。**S16.1 alpha 先例的同一类
加性 schema 重录**，协议内动作。headless 不渲染，漂移纯属性采样面。

**重录前验证（真数据驱动，双跑对照）**：
1. `git stash -u` 回旧码跑 `t_abi_01`（600 帧 + 冻结轨迹）→ **绿**
   —— 旧基线 `trace hash 8fce32749d0d0bc2` 原样复现（漂移确由本期改动
   引入，非既有错误）；
2. `git stash pop` 后同轨迹跑两遍 → `trace hash 9ed6ecdbe2e03931`
   逐位相同（新基线自身确定性）。

**逐位不变论证（无图集 / cols=0 场景）**：cols=0 → 提取层不发 SetUv →
命令流与旧路径逐条相同；wgpu 精灵分支无记录走原矩形 —— 渲染输出与旧
路径逐位相同（T-F-02 的恒等矩形整帧 RGBA 逐位对比 + 既有 129 条 wgpu
契约全绿共同证明）。已更新 `expected_hash.txt` →
**旧 `8fce32749d0d0bc2` / 新 `9ed6ecdbe2e03931`**。

### 3.2 门禁清单

| 项 | 结果 |
|---|---|
| 十 crate `cargo test --release` | 全绿 **755 / 0 failed**（asset 34、audio 52、ext-api 7、ext-js 29、media 27、render-api 46、render-extract 58、render-wgpu 129、runtime 116、scene 257；基线 745 + 新增 10） |
| clippy（`--release --all-targets`） | **0 警告 × 10** |
| 依赖方向守卫 `check_dependency_direction.py` | **15/15**（白名单未动，零新依赖） |
| Dodge 基线 `t_abi_01` | 绿（重录后全套回归；runtime 116 全绿含 600 帧确定性） |
| frame_demo 冒烟 | ① `NES_GAME_FRAMES=60` 窗口化干净退出（图集 BMP 代码生成 + 2 角色相位差循环）；② headless CLI 300 帧跑两遍 `trace hash b82d6df9082cfb35` 逐位相同 |
| tween_demo 冒烟 | `NES_GAME_FRAMES=180` 干净退出；headless 300 帧双跑 `trace hash 99bdd879c3a253b6` 逐位相同（较 S16.1 记录的 `6a6639bc1b806edb` 漂移 = §3.1 同一根因：tween_demo 有 3 个 Sprite2D，三缺省键进指纹采样面；demo 无冻结基线，双跑一致即可） |

### 3.3 新增契约测试

| 编号 | 位置 | 钉什么 |
|---|---|---|
| T-F-01 | nes-render-wgpu/tests/criterion_frame_contract.rs | 32x32 四象限色块纹理、2x2 sheet 逐帧消费 —— 四探针各帧断言对应象限色（照 criterion_alpha 手法） |
| T-F-02 | 同上 | frame=5 / frame=-1 回绕；cols=0 整图四象限同屏；恒等矩形 vs 无记录整帧 RGBA **逐位相同** |
| T-F-03 | 同上 | 同键覆写、未知句柄静默、销毁随条目清理、SetUv 恒在 SetTint 之后 |
| （api） | nes-render-api/tests/criterion_contract.rs | null 簿记：uv_of / 覆写 / 忽略 / 销毁清理 / 输出序（null-wgpu 同构的契约侧锚点） |
| （extract） | nes-render-extract/tests/criterion_extract.rs | sheet 三属性 → SetUv：行主序、回绕（5 与 -1）、正方形网格缺省、迁移帧补推恒等矩形、稳态快照重发、销毁清理、输出序 |
| T-FR-01 | nes-scene/tests/s16_2_frame.rs | 600ms 4 帧 loop 走两圈：floor 序列 `[0,0,1,1,1,2,2,2,3,3,3,0]` ×2（末位回绕）+ 登记永不移除 |
| T-FR-02..05 | 同上 | once 落位 / yoyo 折返边界 / last-wins + pos 并存 + tween_stop / 解析面（弹序、ms<=0、未知名、目标不存在停机）/ 指纹双跑 |

### 3.4 演示与资产

- **frame_demo**（`nes-runtime/examples/frame_demo.rs` +
  `examples/assets/frame_demo.ron`）：2x4 走图图集（代码生成 BMP，四帧
  小方块位移图案 + 地面线，未用格斜线纹理）+ 两个 Sprite2D（相位差 2 帧）
  `sheet_cols=2 / sheet_rows=4` + `init { tween_frame 0→4 / 600ms /
  linear / loop }` —— 肉眼可见走路循环；4 倍放大显示（子矩形采样随世界
  变换缩放）；
- **资产入库**（beep.wav 先例）：`Textures/walk_sheet.bmp`（8246 字节，
  32bpp BMP）随示例生成逻辑一并入库 —— headless CLI 不经示例即可跑
  frame_demo.ron。

---

## §4 遗留（后续里程碑候选）

1. **锚点 / pivot**：精灵缩放/旋转仍绕左上原点；帧动画的"脚底锚点"
   （pivot 偏移）未做 —— 与 transform 缓存和命中测算联动，归专门的
   渲染语义里程碑。
2. **非均匀网格 / 间距与边距**：本期网格 = 均匀 `宽高 ÷ cols/rows`；
   带间距（spacing）/边距（margin）/非均匀帧格的图集需要在 SetUv 之上
   加每帧元数据（或引入图集清单资产），v1 冻结面刻意不碰。
3. **skeletal / 骨骼动画**：帧动画之外的变换层级动画（骨骼绑定、蒙皮）
   与本期的子矩形采样正交，需要新的渲染物形态与编辑器轨道 UI，归
   动画第 3 期评估。
4. **帧事件的音频钩子**：走路循环的脚步声按帧触发（frame == k 时
   play）目前要在脚本里轮询 `frame` 属性；若要"帧回调"引擎面，需要
   帧写入路径发信号 —— 与 S17.2 Hat 触发的语义衔接待评估。
5. **`frame` 的编辑器帧预览**：属性面板对 sheet 三键出的是普通数字
   滑杆；图集编辑器（帧格可视化、拖拽选帧）归编辑器组件库后续批次。
