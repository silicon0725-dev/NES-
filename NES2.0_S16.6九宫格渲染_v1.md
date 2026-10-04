# NES 2.0 · S16.6 Control 九宫格纹理渲染 v1

日期：2026-10-03　分支：`s16-6-nine-slice`（worktree `wt-9s`，基线 150a96b）
范围：Control 面板纹理化的经典件 —— 一张纹理按 3×3 网格渲染：**四角 1:1 固定、
四边单向拉伸、中心双向拉伸**，面板任意缩放角不变形。

---

## §0 契约面与输出序（冻结）

### 0.1 新增渲染命令

```rust
RenderCommand::SetNineSlice {
    handle: ItemHandle,
    texture: RenderAssetKey,   // 源纹理键；NIL = 恒等记录（见 0.3）
    l: f32, t: f32, r: f32, b: f32,   // 源纹理像素边距（3×3 切割线）
    // —— S16.7 就地扩展（S16.6 新命令无兼容层包袱）：
    modulate: bool,            // 模态染色（§5.2；缺省 false = 中性 tint）
    tiling: bool,              // 中间条平铺（§5.3；缺省 false = 拉伸）
}
```

- 属性动作、全量快照、同键覆写、条目销毁随条目消亡 —— 与 `SetTint` /
  `SetUv` / `SetPivot` 完全同一条纪律（先例第四条 = 输出序冻结）；
- 新增 `RenderServer::set_nine_slice(handle, texture, l, t, r, b, modulate,
  tiling)`（S16.7 就地扩参）；null 与 wgpu 两处簿记严格同构
  （`nines: BTreeMap<handle, NineSliceState>` —— S16.7 起载荷收进
  `nes_render_api::NineSliceState` 结构体：纹理键 + 边距四元组 + 两开关）。

### 0.2 输出序（每渲染物属性流，冻结）

```
SetTransform → SetFlip → SetZ → SetVisible
  → SetText → SetList → SetRect → SetClip
  → SetTint → SetUv → SetPivot → SetNineSlice → Submit
```

`SetNineSlice` 冻结在**链尾**（`SetPivot` 之后）；null / wgpu 两处
`submit_into` 严格同序（`criterion_contract_set_nine_slice_bookkeeping_and_order`
与 T-9S-04 双侧钉住）。

### 0.3 清除语义（选型报告）

选 **pivot 零向量先例**（`set_pivot(h, [0,0])` 照存照发），**不选**
`set_clip(None)` 的"摘记录"先例：

- NIL 键的 `SetNineSlice` 是**恒等记录**（fill/border 照旧）：簿记表照存、
  每帧快照重发；消费端（wgpu `CommandConsumer`）收到 NIL 后**摘跨帧九宫格
  簿记**，面板回到 fill/border 路径逐位同基线；
- 理由：簿记跨帧的后端只能从每帧全量快照里读到"清掉"这件事。实现期曾按
  `set_clip(None)` 口径做"服务端摘记录、流里不落条目"，T-9S-04 当场炸出
  陈旧残留（服务端清了、消费端永远收不到清除）—— 恒等记录是唯一能自洽
  承载清除的形状；
- 提取层持 `nines_active` 标记（`ItemSlot` 新增，照 clipped / uv_active /
  pivot_active 同一条"全量快照的生产者侧义务"）：**设过 → 清空的迁移帧**
  补推一次 NIL 恒等记录，稳态帧随快照重发（T-NSX-03 钉住）。

### 0.4 九宫格与 pivot / tint / uv 的交集裁定

- **pivot 无交集**：`SetPivot` 是 Sprite2D 属性（提取层只对精灵推）；
  后端 draw_into 的 Control 分支**不读** pivot 簿记，注册表精灵分支不读
  nines —— 两表按键互斥（Control 用节点身份键、精灵用资源键），语义与
  代码两处零交叉；
- **tint 缺省无交集**（S16.6）：九片 tint 恒中性 `[1,1,1,1]`（面板色即
  纹理色，不经着色通道）；`sprite_tint()` 只服务精灵分支。S16.7 的
  modulate 开关打破这条中性约定 —— `modulate = true` 时九片 tint 改取
  同条目 `ControlState.fill`（仍是 Control 分支内部的事，精灵分支照旧
  零交叉，见 §5.2）；
- **uv 无交集**：`SetUv` 只在注册表精灵分支消费；九宫子矩形独立折算。

### 0.5 渲染分支语义（缺省路径逐位不变）

- `nines.get(&handle)` 未命中（无记录 / 恒等记录）→ Control 走既有
  fill/border 条带 + 滚动条，**逐位同基线**（T-9S-03 / T-9S-04 钉住）；
- 命中且纹理键非 NIL → 面板改走九宫格展开，**fill 与 border_w 条带不画**
  （纹理自带边）；滚动条两种模式共用（内容 chrome，不随面板纹理走）；
  `stats.controls` 仍按条目计 1；
- 纹理未注册（`sample_info` / `texture_px_size` 缺席）或矩形非正宽高 →
  本帧**不画面板且不回退 fill/border**（防"有纹理画九宫、没纹理画边框"
  的模式间闪烁）；滚动条照画；
- `stats.updates` 计数口径纳入 `SetNineSlice`（未知句柄计 `ignored`）。

---

## §1 几何与算式（冻结，`push_nine_slice` 单处实现）

### 1.1 边距钳制公式（防负 / 防角重叠）

```
l' = max(l, 0).min(rect.w / 2)      t' = max(t, 0).min(rect.h / 2)
r' = max(r, 0).min(rect.w / 2)      b' = max(b, 0).min(rect.h / 2)
中段宽 cx = rect.w − l' − r'  ≥ 0   中段高 cy = rect.h − t' − b'  ≥ 0
```

推论：`x/2 + x/2 = x` 在 IEEE 754 下精确，故 `l' + r' ≤ rect.w` 恒成立，
零中段 = 合法退化（中带片宽/高 ≤ 0 整片跳过 —— T-9S-02 钉住 30px 控件、
1px 控件两档）。

### 1.2 目标几何（resolved rect r，视口空间）

| 片 | 目标矩形（相对 r 左上） | 拉伸 |
|---|---|---|
| 左上角 | `(0, 0, l', t')` | 1:1 |
| 上边 | `(l', 0, cx, t')` | 水平 ×、垂直 1:1 |
| 右上角 | `(l'+cx, 0, r', t')` | 1:1 |
| 左边 | `(0, t', l', cy)` | 垂直 ×、水平 1:1 |
| 中心 | `(l', t', cx, cy)` | 双向 × |
| 右边 | `(l'+cx, t', r', cy)` | 垂直 ×、水平 1:1 |
| 左下角 | `(0, t'+cy, l', b')` | 1:1 |
| 下边 | `(l', t'+cy, cx, b')` | 水平 ×、垂直 1:1 |
| 右下角 | `(l'+cx, t'+cy, r', b')` | 1:1 |

每片一个实例（照 Label"一字形一实例"的展开先例）：
`world = inv_view ∘ (T(r.xy+片位) ∘ S(片宽/16, 片高/16))`，
`source = [tile, 1.0]`（USER_TEX 路径），tint 中性。

### 1.3 源子矩形：全瓦片 uv 的分数内插

- **sample_info 尺寸面查证结论**：`TextureRegistry::sample_info(key)`
  返回 `(瓦片号, [u0, v0, us, vs])` —— 只有分数，**不含**源纹理像素尺寸。
  注册尺寸另取：新增 `TextureRegistry::texture_px_size(key) -> Option<(f32, f32)>`
  （读注册时的 `LayerData{width,height}`，与 sample_info 同源同表）；
- 切割线**锚在纹理角上**：右/下切割线从纹理右/下缘回退钳制边距
  （`sx_mid = tw − r'`、`sy_mid = th − b'`）。未钳制时与"左上顺序切"
  逐位同值；钳制退化时四角仍采到纹理真角（标准九宫格"角永远属于纹理角"
  口径 —— 否则 30px 控件的右上角会采到上边条纹）；
- 折算：`uv = 全瓦片.xy + 源px / 注册尺寸 × 全瓦片.wh`（单处实现；
  采样器 NEAREST + CLAMP_TO_EDGE，像素画点对点）。

---

## §2 schema 与提取

### 2.1 schema（nes-scene，Control 加性五 props）

| 属性 | 类型 | 缺省 | 提示 |
|---|---|---|---|
| `ns_tex` | Resource | `Resource(0)` | `H::Resource { kind: "texture" }` |
| `ns_l` / `ns_t` / `ns_r` / `ns_b` | I64 | `0` | None |

- 缺省 = 九宫格关闭，**现状逐位不变**（渲染侧无记录 = fill/border 照旧）；
- 序列化省略缺省值 → 旧场景文件字节不变；
- 注意：Label / Button / TextInput / ScrollView / ListView / Tabs 经 schema
  继承链同样**物化**这五个新键（prop 表 +5 键）——这是 dodge 基线重录的
  根因（§3.2），渲染语义对派生类 inert。

### 2.2 提取（nes-render-extract）

- `PROP_NS_TEX / PROP_NS_L / PROP_NS_T / PROP_NS_R / PROP_NS_B` 常量导出；
- `nine_slice_of(tree, node, source)`：`ns_tex` 经 `ResId::from_value` +
  `renderable_key`（与 Sprite 的 texture 同一条准入链）→ 键非空，且四条
  边距**至少一条 > 0**（全 0 = 没有切割线，视同关闭）→
  `Some((key, [l,t,r,b]))`；负边距照收（钳制权威在渲染侧单处）；
- 仅**裸 Control** 准入推送（`Admission::Control` 分臂）：Button /
  TextInput / ListView / Tabs 等摊平类与 Label 不读 ns 属性（T-NSX-04
  钉住）；有效时逐帧重发（全量快照口径），迁移帧补推 NIL 恒等记录。

### 2.3 测试夹具

`nes-runtime/examples/assets/Textures/nine_patch.bmp`（48×48，32bpp BMP，
照 walk_sheet.bmp / beep.wav 代码生成入库先例）：四角四色 16×16（红/绿/
蓝/黄）+ 上边条左品红右白、下边条左橙右青（验水平拉伸列映射）+ 左边条
上暗灰下亮灰、右边条上藏青下天蓝（验垂直拉伸行映射）+ 中心纯色
`(40,40,60)`。`criterion_nineslice::nine_patch_rgba()` 与之同布局
（测试代码内生成，不读磁盘 —— GPU 测试零文件依赖）。

---

## §3 门禁与基线记录

### 3.1 门禁（全绿）

| 门禁 | 结果 |
|---|---|
| 十 crate `cargo test --release` | 全绿：nes-scene 40、nes-render-api 8+35+5、nes-render-extract 6+28+12+6+8+4、nes-render-wgpu 全部套件（含新增 criterion_nineslice 4 例）、nes-asset 18+16、nes-audio 46+…、nes-media 25+2、nes-extension-api 7、nes-extension-js 8+4+7+5、nes-runtime 全部（含修复后的 dodge 基线） |
| clippy（`--all-targets`）×10 | 0 warning × 10 crate |
| 依赖方向守卫 | **15/15 通过** |
| 冒烟 | `tween_demo`（trace `0683b396c4c48264`，双跑一致）、`frame_demo`（`e8d799638c2526bf`，双跑一致）、`editor_shell`（`ea1967acdec40cad`，双跑一致）headless 120 帧 |
| wgpu 视觉基线 | `criterion_visual_baseline` 通过 —— `BASELINE_FNV1A 0x68d1_4f07_989f_cbaa` **不动**（缺省路径零新实例） |

### 3.2 dodge 基线重录（协议第 5 次）

- **根因**：dodge 场景含一个 `Label` 节点；Label 经继承链持有 Control
  schema → 五个新键进 prop 表 → `scene_fingerprint` 变化 → trace hash
  变化。属"加性 schema 物化"的预期漂移，非渲染/语义回归（渲染缺省路径
  逐位不变，wgpu 视觉基线未动）；
- **协议对照**：旧码（stash 回 HEAD）重跑 → 旧哈希
  `c3c1da4a83979df4` 复现；新码双跑 → `c9399dde09ebd656` 两次全等；
- **重录**：`expected_hash.txt` → `trace hash c9399dde09ebd656`
  （旧 `c3c1da4a83979df4`）；
- editor.ron 无冻结基线：双跑一致（`ea1967acdec40cad` ×2）即过。

### 3.3 新增测试

| 文件 | 用例 |
|---|---|
| `nes-render-wgpu/tests/criterion_nineslice.rs` | T-9S-01（96x96 面板：四角 1:1 / 中心纯色 / 上边条水平拉伸列映射 / 左边条垂直拉伸行映射 / drawn=9）、T-9S-02（30x30 < 边距和 32：min 钳制、四角锚纹理真角、drawn=4、1x1 不 panic）、T-9S-03（无 ns 记录逐位同基线）、T-9S-04（同键覆写改像素 / NIL 清除整帧逐位回基线 / 输出序 / 销毁清理） |
| `nes-render-extract/tests/s16_nineslice.rs` | T-NSX-01（有效五 props → SetNineSlice 载荷逐位 + 负边距照收）、T-NSX-02（缺省 / 未绑定 / 全零边距不推）、T-NSX-03（迁移帧补推 NIL 恒等记录一次 + 稳态重发）、T-NSX-04（Button 带 ns 属性不推） |
| `nes-render-api/tests/criterion_contract.rs` | null 簿记：覆写 / NIL 恒等记录照存照发 / 未知句柄忽略计数 / 输出序 / 销毁清理 |
| `nes-render-extract/tests/criterion_gaps.rs` | `kind_of` 补 `SetNineSlice` 枚举臂 |

测试字面量全 ASCII；注释中文；零新依赖（G1–G15 守卫全过即为证）。

---

## §4 遗留（不做在本期）

> S16.7 已收口本清单第 1、2 条（见 §5）；以下为**当前剩余**：

1. ~~**九宫格与 fill/border 混合模式**~~ → **S16.7 已做**：`ns_modulate`
   开关 = 九实例 tint 改取 fill_slot 解析色（模态染色，§5.2）。"九宫格 +
   再叠半透明填充罩 / 边框压纹理"的全合成语义仍留待真实创作需求；
2. ~~**平铺中间条（tiling）**~~ → **S16.7 已做**：`ns_tiling` 开关 =
   四边条与中心按源边距像素原生尺寸平铺（§5.3；NEAREST 采样下用逐片
   展开实现，无需 WRAP 地址模式与采样器改动）；
3. **派生控件接入**：Button / TextInput / ListView / Tabs 的 schema 已
   继承 ns 七键（含 S16.7 两开关）但提取层不读（冻结口径）。面板纹理化
   扩到按钮九宫需求出现时，在 `admit()` 的对应分臂接同一条
   `nine_slice_of` 即可；
4. **编辑器实时预览**：编辑器属性面板尚无 ns_tex 的资源选择器联动与
   视口即时刷新（ns_tex 改动经既有属性管线下一帧生效，但属性面板控件
   仍是裸 Resource 编辑）—— 归入编辑器组件库的后续打磨。

---

## §5 S16.7 模态染色与平铺（遗留项 1 / 2 收口）

日期：2026-10-03　分支：`s16-7-nine-slice-modes`（worktree `wt-9s2`，
基线 fea4083）。S16.6 的两个遗留项：**fill 混合**（模态染色）与
**中间条平铺**。两开关都是加性 Bool、缺省 false = S16.6 行为逐位不变。

### 5.1 契约面扩展（就地扩形状，无兼容层）

- **命令**：`SetNineSlice` 增 `modulate: bool` / `tiling: bool`（S16.6
  新命令，就地扩展无需兼容层）；输出序、全量快照、NIL 恒等记录语义
  全部不变；
- **簿记**：null / wgpu 两处 `nines` 表改存 `NineSliceState`
  （`nes_render_api::state` 新结构体：`texture` + `margins: [f32;4]` +
  `modulate` + `tiling`，`IDENTITY` 常量 = 清除载体）；
  `NullRenderServer::nine_slice_of` 返回面随之改形；
- **schema**（Control 加性两 props，缺省 = 既有行为逐位不变）：

  | 属性 | 类型 | 缺省 | 语义 |
  |---|---|---|---|
  | `ns_modulate` | Bool | `false` | 模态染色（§5.2） |
  | `ns_tiling` | Bool | `false` | 中间条平铺（§5.3） |

  继承链（Label / Button / TextInput / ScrollView / ListView / Tabs）照旧
  物化新键 —— dodge 基线第 6 次重录的根因（§5.6）；渲染语义对派生类
  inert（T-NSX-04 口径不变）；
- **提取**：`PROP_NS_MODULATE` / `PROP_NS_TILING` 常量导出；
  `nine_slice_of` 返回完整 `NineSliceState`（开关缺失/类型错 → false）；
  染色的**填色解析不在此层** —— 色源是 `fill_slot` 的既有解析载体
  `ControlState.fill`（`themed_control` 既有路径，随 `SetRect` 下发），
  提取层零重复解析；九宫格失效（ns_tex 未绑定/全零边距）时开关 inert，
  准入判据不变（T-NSX-05 钉住）。

### 5.2 模态染色（modulate）

- **语义**：`draw_into` Control 分支展开时
  `tint = modulate ? tint_of(ControlState.fill) : [1,1,1,1]`，九片（含
  平铺片）共享同一 tint（`push_nine_slice` 收调用方折好的 tint 参数）；
- **用法（灰阶纹理配方）**：纹理配**灰阶/白图** + `fill_slot` 配彩色
  槽位 → 面板观感 = 灰阶明暗 × 槽位色 —— 同一纹理多套面板配色，经典
  StyleBoxTexture modulate 手法。纹理配彩图时 RGB 相乘照语义发生（会
  变暗），不禁止但不推荐；
- **成对纪律**：需与 `fill_slot` 成对配置 —— 裸 Control 的 fill_slot
  缺省空名 = 解析色透明（fill `[0,0,0,0]`），modulate 下会把面板整体
  乘没（确定性、可诊断、非 panic；schema 文档与渲染侧注释双处写明）。
  fill alpha = 255 的主题槽位不受影响（tint 的 A 通道 = 1）；
- **像素证据**：T-NS-M-01 —— 灰 200 纹理 × fill 红 =
  `[200,0,0,255]`（unorm 逐通道：200/255×1 → 200、200/255×0 → 0）；
  开关关 = `[200,200,200,255]` 纯灰。

### 5.3 中间条平铺（tiling）

- **语义**：四边条与中心改按**源边距像素的原生尺寸**重复而非拉伸
  （非整数缩放防糊）；**角永远 1:1 不变**、不占平铺预算、恒发射；
- **几何公式**（`push_nine_slice` 单处实现，`dl/dt/dr/db` = **声明**
  边距、只钳负不钳半边 —— 平铺单元是纹理事实）：

  ```
  平铺单元   unit_h = tw − dl − dr        unit_v = th − dt − db
  轴向片数   n = ceil(条长 / 单元)        （条长 = 钳制后 cx / cy）
  世界片长   piece = min(单元, 剩余)      剩余 = 条长 − i×单元
  片 uv 长   src × piece / 单元           （src = 源条全长；常规情形
                                           piece = 单元 = src → 逐像素 1:1）
  ```

  显示条长仍受钳制后 rect 约束；边距钳制激活时源条按 `片长/单元` 比例
  截断映射（周期仍为声明单元长）；
- **退化轴**：单元 ≤ 0（声明边距和 ≥ 纹理边长）该轴回落单片拉伸；
  零中段（cx/cy ≤ 0）无片可画（不占预算）；
- **上限**：每个九宫格条目单次展开的平铺片合计 ≤
  `NINE_SLICE_TILE_CAP = 256`（四角不算）。超限按视觉序（上 → 左 →
  中 → 右 → 下）先到先画、其余截断；截断片数进
  `FrameStats::nines_truncated`（新诊断计数，非平铺帧恒 0）。触发条件
  = 大面板 × 小平铺单元（4128px 方块 × 16px 单元：上条恰 256 片吃满
  预算，左/中心/右/下截断 66304 片 —— T-NS-T-01 钉住）；
- **采样器**：保持 NEAREST + CLAMP_TO_EDGE，零改动 —— 平铺靠逐片展开
  （照 Label"一字形一实例"先例），不需要 WRAP 地址模式（S16.6 遗留
  评估项的结论）。

### 5.4 组合矩阵

| ns_modulate | ns_tiling | 行为 |
|---|---|---|
| false | false | S16.6 拉伸九片，中性 tint（既有行为逐位不变） |
| true | false | 九片拉伸，tint = fill_slot 解析色（§5.2） |
| false | true | 平铺展开（≤256 片 + 4 角），中性 tint |
| true | true | 平铺展开，全片（含角）tint = fill_slot 解析色（T-NS-T-02 钉住） |

两开关与边距、纹理注册、清除语义全部正交；开关翻转即改像素（全量
快照逐帧重发，无迁移帧问题）。

### 5.5 测试（criterion_nineslice.rs 扩展 + 回归）

| 编号 | 契约 |
|---|---|
| T-NS-M-01 | 灰阶纹理 + modulate + fill 红 → 中心/角/上条 = 灰×红（手算通道）；modulate=false 对照 = 纯灰 |
| T-NS-T-01 | 上条源列非均匀灰阶（16 列 = c×16）→ 平铺多点采样与源列一一对应、每单元从源条头重启、角 1:1、中心纯色；实例 36 片；拉伸对照同点失真；4128px × 16px 触发 256 上限，`nines_truncated = 66304` |
| T-NS-T-02 | modulate + tiling 同开：平铺片/中心/角同受 fill 染色（绿角×红 fill → R 归零） |
| 回归 | T-9S-01..04 照绿（两开关缺省 false = 逐位不变）；T-NSX-01..05（提取层，新增 05）；null 簿记契约测试随载荷形状同步更新 |

测试字面量全 ASCII；注释中文；零新依赖。

### 5.6 dodge 基线重录（协议第 6 次）

- **根因**：同 §3.2 机理 —— dodge 场景 1 个 Label（继承 Control）×
  2 个新键（ns_modulate / ns_tiling 缺省物化）→ `scene_fingerprint`
  的 prop 表计数变化 → trace hash 变化。渲染缺省路径逐位不变
  （`criterion_visual_baseline` 通过、`BASELINE_FNV1A 0x68d1_4f07_989f_cbaa`
  未动）；
- **协议对照**：旧码（stash 回 fea4083）重跑 → 旧哈希
  `c9399dde09ebd656` 复现；新码双跑 → `2674fa7f53e6f045` 两次全等；
- **重录**：`expected_hash.txt` → `trace hash 2674fa7f53e6f045`
  （旧 `c9399dde09ebd656`）；
- **冒烟哈希同源漂移**（无冻结基线，门禁 = 双跑一致）：tween_demo
  `0683b396c4c48264` **未动**（场景无 Control 链节点）；frame_demo
  `e8d799638c2526bf` → `879a46be09fac339`、editor_shell
  `ea1967acdec40cad` → `8b6783f0e5027465`（各含 1 个 Label，漂移机理
  与 dodge 相同；旧码 stash 对照逐一复现旧值，新码各自双跑全等）。

### 5.7 附带修复与遗留

- **附带修复**：`nes-scene/tests/s12_scroll.rs::t_sc_04` 的链上聚合计数
  断言在 **fea4083 即红**（S16.6 加 5 键时漏改：17 ≠ 12）—— 本期随
  两新键一并修正为 19（2 + 12 + 5）并注明；
- **剩余遗留**：§4 第 3、4 条（派生控件接入、编辑器实时预览），以及
  "九宫格 + 半透明填充罩 / 边框压纹理"的全合成语义（§4 第 1 条残余）。
