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
}
```

- 属性动作、全量快照、同键覆写、条目销毁随条目消亡 —— 与 `SetTint` /
  `SetUv` / `SetPivot` 完全同一条纪律（先例第四条 = 输出序冻结）；
- 新增 `RenderServer::set_nine_slice(handle, texture, l, t, r, b)`；
  null 与 wgpu 两处簿记严格同构（`nines: BTreeMap<handle, (key, [l,t,r,b])>`）。

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
- **tint 无交集**：九片 tint 恒中性 `[1,1,1,1]`（面板色即纹理色，不经着色
  通道）；`sprite_tint()` 只服务精灵分支；
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

1. **九宫格与 fill/border 混合模式**：现为互斥分臂（九宫格时 fill/border
   不画）。"九宫格 + 再叠主题色边框 / 半透明填充罩"需要明确的合成语义
   （先纹理后着色？边框压纹理？），留待真实创作需求出现再定；
2. **平铺中间条（tiling）**：中心与四边恒为拉伸。窗口内衬、条纹装饰等
   需要 `middle = repeat` 的模式（uv 折算改 `fract` 循环），需在契约层
   加模式位（加性 props），配合采样器 WRAP 地址模式评估；
3. **派生控件接入**：Button / TextInput / ListView / Tabs 的 schema 已
   继承 ns 五键但提取层不读（本期冻结口径）。面板纹理化扩到按钮九宫
   需求出现时，在 `admit()` 的对应分臂接同一条 `nine_slice_of` 即可；
4. **编辑器实时预览**：编辑器属性面板尚无 ns_tex 的资源选择器联动与
   视口即时刷新（ns_tex 改动经既有属性管线下一帧生效，但属性面板控件
   仍是裸 Resource 编辑）—— 归入编辑器组件库的后续打磨。
