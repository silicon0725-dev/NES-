# NES 2.0 · S16.3 精灵锚点 —— pivot 可配置的绘制/旋转/缩放基准点

日期：2026-10-03 · 分支：`s16-3-pivot`（独立 worktree `wt-pivot`）·
基线：7b7b968（S16.2 图集帧动画）· 依赖 **零新增**（十个 crate 逐个构建；
依赖分层 G1–G15 原样，核心 crate 零第三方纪律不动）

---

## §0 结论

S16.3 之前，精灵四边形从变换原点向 +x/+y 展开（顶角贴原点），旋转/缩放
绕**左上角**——"脚底锚定的走路循环""中心锚定的转盘"都做不出来。本期做
一件加性扩展，既有语义逐位不变：

**Sprite2D schema 增性一 prop `pivot`**（Vec2，缺省 `(0, 0)`）——归一化锚点
（0..1 相对精灵矩形；越界值照实接受 = 锚点落在精灵外，拖尾/关节挂点等
合法创作用途，不设数值钳制）；渲染契约新增
`RenderCommand::SetPivot { handle, pivot: [f32; 2] }`（照 SetTint/SetUv
先例：server trait 方法 + null/wgpu 同构簿记 + 输出序冻结 SetUv 之后 +
无记录 = 无平移）。语义：精灵四边形在**变换前的局部空间**平移
`-pivot × 16px 基准格`——旋转/缩放/位置全部以锚点为基准，`(0.5, 0.5)`
= 中心锚定（位置即精灵中心）。

**门禁**：十 crate `cargo test --release` 全绿 **764 / 0 failed**
（基线 755 + 新增 9：render-api 47 含 null SetPivot 簿记、render-extract 59
含 pivot 推送、render-wgpu 136 含 T-P 像素 7 条）；clippy **0 警告 × 10**；
worktree 根守卫 **15/15**；冒烟见 §3.2。已 git commit（未 push）。

---

## §1 几何定义（局部平移序；与帧/旋转/缩放的关系）

### 1.1 schema：一 prop（加性缺省 = 既有行为）

| 属性 | 类型 | 缺省 | 语义 |
|---|---|---|---|
| `pivot` | Vec2 | `(0.0, 0.0)` | 归一化锚点（0..1 相对精灵矩形）。`(0,0)` = 左上角 = **既有行为逐位不变**；`(0.5,0.5)` = 中心锚定；`(0,1)` = 底边中点（"脚底"）。**无数值 hint（不钳制）**——越界值照实接受 = 锚点落在精灵矩形外（ muzzle 火焰挂点、拖尾起点等合法用途），取舍权威在渲染侧单处折算 |

序列化省略缺省值（RON 往返不携带该键，m2 打包字节确定性测试原样绿）；
语义指纹的属性表**逐键混入**——缺省物化让每个 Sprite2D 多一个键，
这是 dodge 基线第 4 次重录的根因（§3.1）。

### 1.2 局部平移与乘法序（本期唯一的几何事实）

精灵四边形的既有局部形状：`corner × 16px`（`corner ∈ {0,1}²`，从原点向
+x/+y 展开）。pivot 的全部语义 = 在**采样前**把四边形平移
`-pivot × (16px, 16px)`：

```text
world = world_transform ∘ translation(-px × 16, -py × 16)
```

**乘法序查证结论**（报告项）：`nes-render-api::math::Affine2::mul(&self,
rhs)` 的文档与实现都是 `self ∘ rhs` —— **先应用 rhs、再应用 self**（测试
`composition_order_is_self_after_rhs` 长期钉住）。因此
`world_transform().mul(&translation(...))` 里平移 rhs 是**内层先行的局部
变换**：局部点先被平移、再过世界变换的旋转/缩放/父链。这正是"旋转/缩放
绕锚点"的来源——锚点被平移到变换原点上，随后的 `R/S` 对它原地作用；
位置（`tx/ty`）也因平移先行而自然变成"锚点的落点"。**反序
（`translation ∘ world`）会把平移抬到世界空间**：旋转轴错位、锚定退化成
纯位置偏移——序错则语义整体作废。折算只有一处（wgpu `draw_into` 的
注册表精灵分支），注释里写明同一结论。

矩阵乘法本身对零记录也是**逐位精确**的：`translation(±0, ±0)` 与任何
仿射阵相乘只产生 `×1.0`、`×±0.0`、`+±0.0` 三类运算，对有限值全部无舍入
（`x + (-0.0) = x` 精确成立）——所以 `[0,0]` 记录（清除补推）与无记录
在像素上逐位相同，T-P-01/T-P-05 整帧 RGBA 逐位对比钉住。

### 1.3 与图集帧 / 旋转 / 缩放 / 翻转的关系

- **图集帧**：pivot 归一化相对**当前帧矩形**——帧采样（S16.2 `SetUv`）
  只改 uv 不改几何，四边形形状与锚点平移量与帧号无关，"换帧不换 pivot
  语义"**天然成立**，T-P-04 帧切换前后包围盒探针钉一处；
- **旋转/缩放**：随世界变换先行平移后作用（§1.2），T-P-03（90° 旋转后
  四象限重排、包围盒不动）与 T-P-06（缩放四角对称外扩）像素钉住；
- **翻转（flip）**：flip 是 `transform ∘ flip` 的渲染期子局部后乘
  （契约 I8），pivot 平移在其后再乘——翻转轴仍过锚点（中心锚定下
  翻转不挪位置），与"flip 绕自身原点"的既有语义同一坐标基；
- **控件 / 字形 / 图集格路径**：不查 pivots 表——pivot 是注册表精灵
  分支的专属语义（Sprite2D 独有属性），其余绘制路径逐位不变。

---

## §2 契约与基线重录记录

### 2.1 渲染契约：`RenderCommand::SetPivot`（照 SetTint/SetUv 先例三同构）

```rust
RenderCommand::SetPivot { handle: ItemHandle, pivot: [f32; 2] }
```

- **语义**：归一化锚点 `[px, py]`（0..1 相对精灵矩形；负值/越界照实
  接受）；`[0, 0]` = 零平移 = 恒等（"清除"零向量即可表达）。
- **trait**：`RenderServer::set_pivot(handle, pivot)`；输出序冻结在
  `SetUv` 之后（每渲染物属性流序：… → SetRect → SetClip → SetTint →
  SetUv → SetPivot → Submit；null 与 wgpu 两处 submit 严格同序）。
  属性动作、全量快照、同键覆写、销毁随条目消亡、未知句柄静默忽略
  （契约 I1）。
- **NullRenderServer**：`pivots: BTreeMap` 簿记 + `pivot_of()` 只读访问 +
  计数器口径不变。
- **wgpu 后端**：`WgpuRenderServer` 与 `CommandConsumer` 各一张 `pivots`
  表（同键覆写、`DestroyItem` 随条目清理、`updates/ignored` 计数同
  SetUv 口径）；**注册表精灵分支的 world 从 `transform ∘ flip` 改查此表
  多乘一截局部平移**（§1.2 算式，单处折算）。无记录走原矩阵——既有
  路径逐位不变。图集格 / 字形 / 控件路径不查此表。

### 2.2 提取层：`pivot` 属性 → `SetPivot`

- Sprite 准入读 `pivot`（Vec2 直读；缺省/类型错 → `(0,0)`；**非有限值
  按缺省处理**——NaN 进平移会把世界矩阵整体污染，与
  `sprite_tint_rgba` 的非有限兜底同一口径）；
- **非 (0,0) 逐帧重发**（照 tint/uv 全量快照先例）；**(0,0) 缺省不推**——
  缺省路径的命令流与既有路径逐条相同；
- **非(0,0) → (0,0) 迁移帧补推一次零向量**：`ItemSlot::pivot_active`
  标记（照裁剪 Some→None / uv 激活→整图迁移的同一"全量快照生产者侧
  义务"——pivots 簿记跨帧持久，不显式清除则陈旧锚点永久残留）；
  零向量入库后由服务端全量快照逐帧重发（渲染侧零平移 = 恒等，无害）；
- 稳态提取侧零新增（was=false 不再补推）；销毁随条目清理。

### 2.3 基线重录记录（协议：真数据驱动 + 评审；第 4 次）

**漂移根因**：Sprite2D 属性表缺省物化新增 `pivot` 一键——语义指纹的
属性表逐键混入（`determinism.rs` 对每节点属性表逐键混哈希），每个
Sprite2D 节点多混一键 → dodge 基线（含 4 个 Sprite2D）指纹必然漂移。
S16.1 alpha / S16.2 sheet 三键**同一类加性 schema 重录**，协议内动作。
headless 不渲染，漂移纯属性采样面（渲染侧 pivot (0,0) = 零平移 =
逐位不变，§1.2）。

**重录前验证（真数据驱动，双跑对照）**：
1. 改动前 `git` 干净树（HEAD=7b7b968 旧码）跑 `t_abi_01` → **绿**——
   旧基线 `trace hash 9ed6ecdbe2e03931` 原样复现（漂移确由本期改动
   引入，非既有错误）；
2. 新码同轨迹跑两遍（headless CLI × 2）→
   `trace hash c3c1da4a83979df4` 逐位相同（新基线自身确定性）。

已更新 `expected_hash.txt` → **旧 `9ed6ecdbe2e03931` / 新
`c3c1da4a83979df4`**，`t_abi_01` 复绿（runtime 116 全绿）。

---

## §3 门禁与新增契约测试

### 3.1 门禁清单

| 项 | 结果 |
|---|---|
| 十 crate `cargo test --release` | 全绿 **764 / 0 failed**（asset 34、scene 257、render-api 47、render-extract 59、render-wgpu 136、audio 52、media 27、ext-api 7、ext-js 29、runtime 116；基线 755 + 新增 9） |
| clippy（`--release --all-targets`） | **0 警告 × 10** |
| 依赖方向守卫 `check_dependency_direction.py` | **15/15**（白名单未动，零新依赖） |
| Dodge 基线 `t_abi_01` | 绿（重录后全套回归；runtime 116 全绿含 600 帧确定性） |
| frame_demo 冒烟 | ① `NES_GAME_FRAMES=60` 窗口化干净退出；② headless CLI 300 帧跑两遍 `trace hash dcc49992ec0bc799` 逐位相同（较 S16.2 的 `b82d6df9082cfb35` 漂移 = §2.3 同一根因：demo 场景 3 个 Sprite2D 的缺省键进指纹采样面；demo 无冻结基线，双跑一致即可） |
| tween_demo 冒烟 | `NES_GAME_FRAMES=180` 窗口化干净退出；headless 300 帧双跑 `trace hash 331f56c37c623b7b` 逐位相同（漂移同上，3 个 Sprite2D） |

### 3.2 新增契约测试

| 编号 | 位置 | 钉什么 |
|---|---|---|
| T-P-01 | nes-render-wgpu/tests/criterion_pivot_contract.rs | pivot (0,0) 对照：显式零记录与无记录整帧 RGBA **逐位相同** + 缺省顶角贴 pos 的既有像素（既有行为不变的渲染侧根） |
| T-P-02 | 同上 | pivot (0.5,0.5) 精灵放 (32,32)：像素断言占 (24..40)（中心锚定，位置即中心；四向对称退让） |
| T-P-03 | 同上 | pivot (0.5,0.5) + 旋转 90°：四象限按 90° 重排（红→右上）、包围盒不动（绕锚点旋转的像素事实） |
| T-P-04 | 同上 | 图集帧 + pivot 中心（缩放 2，占 (16..48)）：帧切换只换采样色、包围盒探针不动（pivot 相对当前帧矩形） |
| T-P-05 | 同上 | 设非零回调 (0,0)：锚定位移可见 → 补推零向量清除后整帧**逐位回基线** |
| T-P-06 | 同上 | scale + pivot 中心（缩放 2）：四角对称外扩 (16..48)，与中心等距四向探针（缩放绕中心的像素事实） |
| （bookkeeping） | 同上 | wgpu 侧同键覆写、未知句柄静默、销毁随条目清理、输出序 SetPivot 恒在 SetUv 之后 + pivot (0,1) 底边锚定的像素形状 |
| （api） | nes-render-api/tests/criterion_contract.rs | null 簿记：pivot_of / 覆写 / 忽略计数 / 销毁清理 / 输出序 SetTint→SetUv→SetPivot（null-wgpu 同构的契约侧锚点） |
| （extract） | nes-render-extract/tests/criterion_extract.rs | pivot 属性 → SetPivot：缺省不推（T-P-01 提取侧根）、同键覆写、越界不钳制、非(0,0)→(0,0) 迁移补推零向量一次、稳态只剩快照重发、输出序、销毁清理 |

---

## §4 遗留（后续里程碑候选）

1. **tween_pivot 通道**：补间系统已有 pos/scale/alpha/frame 通道；锚点
   补间（`tween_pivot "name" x0 y0 x1 y1 ms`）可做"锚点迁移动画"（走路
   循环的脚底换脚），需要 TweenChannel 新变体 + 指纹位形，归补间第 3 期。
2. **编辑器 pivot 手柄**：属性面板当前给 `pivot` 出普通 Vec2 控件；可视
   化锚点手柄（拖拽九宫格定位点 + 吸附）归编辑器组件库后续批次。
3. **九宫格 / 3-patch**：pivot 解决"绕哪变"，不解决"边不随缩放变形"；
   九宫格需要每精灵 9 个实例 + 边框切分元数据，与控件边框条机制（S12.1）
   的关系待评估。
4. **命中测算联动**：渲染侧锚点已闭合；`nes-scene` 侧命中/包围盒测算
   （如有需要消费 pivot 的编辑器选中框、点击测试）尚未读该属性——
   渲染与命中的几何权威分属两层，联动时须单处折算（同 §1.2 算式）。
