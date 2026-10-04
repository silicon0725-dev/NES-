# NES 2.0 · S16.1 补间后续 —— 缓动族 + yoyo/loop + scale/alpha 通道 + 到站信号

日期：2026-10-03 · 分支：`s16-1-tween-channels`（独立 worktree `wt-tween2`）·
基线：159766f（S17.5 扩展生态 P2 收尾）· 依赖 **零新增**（十个 crate 逐个
构建；依赖分层 G1–G15 原样，核心 crate 零第三方纪律不动）

---

## §0 结论

S16 第 1 期把 `tween_pos` 做成了引擎一等公民；本期在其冻结骨架（专属推进
阶段、last-wins、from 落地采样、条件混入指纹、会话态序列化）内做**三件
加性扩展**，既有语义逐位不变：

1. **缓动函数族**（冻结 5 个，纯函数单点实现）：`linear`（= 现状恒等）、
   `smoothstep`（t²(3−2t)）、`ease_in`（t²）、`ease_out`（1−(1−t)²）、
   `ease_in_out`（t<0.5 ? 2t² : 1−2(1−t)²）。语法：`ms` 后可跟 0–2 个
   可选字符串字面量（第一 = 缓动名、第二 = 模式名）；未知名**解析期报错**
   并附合法名单（拼写错误不静默降级 linear）。
2. **播放模式**：`once`（缺省 = 现状）/ `yoyo`（总时长 = 2×duration，
   回零才落位 from 并移除）/ `loop`（进度对 1 取模，**永不自动移除、
   永不到站**；停用走 `tween_stop`，停在当前值）。
3. **通道化**：补间从"位置"升级为三通道 —— `pos`（现状）、`scale`
   （`tween_scale`，写 `Transform2D.scale`）、`alpha`（`tween_alpha`，写
   Sprite2D 新增 `alpha` 属性）。last-wins 按**（节点，通道）二元组**——
   同节点三通道并存互不干扰；`tween_stop` = 该节点**全部通道**一并停。
4. **到站信号**：once/yoyo 完成时引擎发 `tween_done`（载荷 =
   `Value::Str(节点名)`），推进阶段泵前发出 —— 同 tick 阶段 5 送达；
   **每通道完成各发一条**（同帧多通道完成 = 多条信号，次序 = 注册序）；
   **loop 永不完成，永不发到站信号**（文档口径：loop 补间没有"到站"）。

**门禁**：十 crate `cargo test --release` 全绿 **745 / 0 failed**
（nes-scene 252 含 s16_tween 10 条；nes-render-api 45；nes-render-extract
57 含 alpha 簿记；nes-render-wgpu 126 含 alpha 像素 3 条）；clippy 0 ×10；
worktree 根守卫 **15/15**；tween_demo 冒烟双通道（见 §4）。
已 git commit（未 push）。

---

## §1 缓动族与模式

### 1.1 缓动：应用点与逐位不变论证

**应用点**（`tree.rs` 推进阶段）：线性进度 t 算出后先过形状函数
`te = ease(t)`（f64 域），插值用 `te as f32` —— 与 S16 第 1 期的
`t as f32` 同一落点。**linear = 恒等函数**，因此缺省路径与既有补间轨迹
**逐位相同**（T-TW-01 既有断言原样全绿即证明）。

| 名 | 算式 | t=0.5 |
|---|---|---|
| `linear` | t（现状） | 0.5 |
| `smoothstep` | t·t·(3−2t) | 0.5 |
| `ease_in` | t² | 0.25 |
| `ease_out` | 1−(1−t)² | 0.75 |
| `ease_in_out` | t<0.5 ? 2t² : 1−2(1−t)² | 0.5（分支边界） |

集合**冻结**（`TweenEasing`，`as_str` 稳定拼写不得改名）；`LEGAL` 常量
名单与解析期报错文案同源。契约：T-E-01（五缓动 t=0.5 各自断言 + 未知
缓动/模式名解析报错附名单）。

### 1.2 模式：once / yoyo / loop

推进阶段按模式分臂（`TweenMode`，`as_str` 稳定拼写）：

- **once**（缺省）：现状 —— `t = clamp(elapsed/duration, 0, 1)`；
  `t >= 1` 落位 to、移除、发到站信号。
- **yoyo**：内部进度 `p = elapsed/duration`（不 clamp）；
  位置 = `p < 1 ? ease(p) : ease(2−p)`；完成判定 `p >= 2` —— 回零
  **精确落位 from**（不经插值，f32 乘法不引入误差）并移除 + 发到站
  信号。总时长 = 2×duration。
- **loop**：`p = (elapsed/duration) % 1.0` —— **永不自动移除、永不
  完成、到站信号不发**（`tween_done` 的口径是"一次性补间的到站"，
  loop 没有；停用走 `tween_stop`，停在当前值）。demo 与文档双处写明。

实现注意（实证修正）：脚本语法一个节点只有一个入口（`on` **或**
`every`，S6 冻结）—— "到站再武装"的 demo 写法是两个 Script 节点
（`every` 起程 + `on "tween_done"` 直接再起程，信号入口可发补间 =
两入口同权的既有口径）。

### 1.3 语义保持清单（逐项验证）

- **last-wins**：按（节点，通道）二元组替换 —— `register_tween` 先
  `retain` 掉同目标同通道，再按登记序追加；不同通道并存（T-S-01 实证：
  重发 scale 后 pos 的 elapsed 连续推进、scale 从头计且新 from = 落地时
  当前 scale，不跳变）。
- **from 采样时机**：仍在 **Cmd 落地时**（apply 即刻），三通道同口径 ——
  pos/scale 读 local，alpha 读属性（缺省 1.0）。
- **指纹条件混入**：登记表非空才摺进 —— 无补间场景零混入；混入字段
  扩为 目标 uid + 通道标签 + from/to 位形 + 缓动/模式稳定名 +
  elapsed/duration 位形（determinism.rs；T-TW-03 双跑逐位相同 +
  含/不含必不同原样成立）。
- **会话态**：补间不进 RON 往返（`Tween` 不在 `NodeData`，`to_doc`
  天然不携带）—— 不变。
- **死目标自动清、推进先于脚本**：不变（T-TW-03 / T-TW-05 原样绿）。

---

## §2 alpha 通道（唯一渲染契约扩展）

### 2.1 场景侧：加性 schema + 真实树状态

- Sprite2D schema 增 `alpha`（`ValueType::F32`，缺省 `F32(1.0)`，
  `H::Number 0..1 step 0.01`）—— 出生即满配（`default_store` 物化），
  schema 校验 + 越界 clamp 0..1。
- `tween_alpha "name" a ms ["easing"] ["mode"]`：推进阶段经**既有属性
  写路径**（`set_prop` → schema 校验夹取）每 tick 直写 —— alpha 是
  **真实树状态**，与 pos 同口径进语义指纹（属性表逐键混入，天然覆盖）；
  终点非有限值落地拒收、越界落地夹取（T-A-01）。

### 2.2 渲染契约：`RenderCommand::SetTint`

```rust
RenderCommand::SetTint { handle: ItemHandle, rgba: [u8; 4] }
```

- **语义** = E-1 相乘色（采样色 x tint，中性 `[255,255,255,255]` =
  恒等），但作为**独立属性命令**：精灵没有自己的颜色字段，alpha 通道
  经此进入。提取层只动 A：`rgba = [255, 255, 255, (alpha * 255) as u8]`。
- **trait**：`RenderServer::set_tint(handle, rgba)`；输出序冻结在
  `SetRect` / `SetClip` 同段之后（每渲染物：… → SetRect → SetClip →
  SetTint → Submit；null 与 wgpu 两处 submit 严格同序）。属性动作、
  全量快照、同键覆写、销毁随条目消亡、未知句柄静默忽略（契约 I1）。
- **NullRenderServer**：`tints: BTreeMap` 簿记 + `tint_of()` 只读访问 +
  `ServerCounters` 口径不变。
- **nes-render-extract**：Sprite 准入每帧推 `SetTint`（全量快照口径）；
  alpha 缺省/非有限按 1.0、越界夹取 0..1（`sprite_tint_rgba`）。
- **wgpu 后端**：`WgpuRenderServer` 与 `CommandConsumer` 各一张
  `tints` 表；**精灵实例 tint 从中性改查 tints 表 —— 无记录 = 中性，
  逐位不变**（图集格路径与注册表路径两处；字形/控件实例不查表，颜色
  各走既有契约字段）。

### 2.3 基线重录记录（协议：真数据驱动 + 评审）

**漂移根因**：Sprite2D 属性表缺省物化新增 `alpha = 1.0` —— 语义指纹的
属性表逐键混入（BTree 名序，`alpha` 排在 flip_h 前），每个 Sprite2D
节点多混一键 → dodge 基线（含 4 个 Sprite2D）指纹必然漂移。这是
**S12-1 color_slot 先例**的同一类加性 schema 重录，协议内动作。

**重录前验证（真数据驱动）**：
1. `git stash` 回旧码跑 CLI（600 帧 + 冻结轨迹）→ `trace hash
   b5b51dac27012ace` —— **旧基线原样复现**（漂移确由本期改动引入，
   非既有错误）；
2. `git stash pop` 后同轨迹跑两遍 → `trace hash 8fce32749d0d0bc2`
   逐位相同（新基线自身确定性）。

**逐位不变论证（无补间 / alpha=1 场景）**：alpha = 1.0 → 提取层
`(1.0 * 255.0) as u8 = 255` → tint 归一化 255/255 = 1.0（f32 精确）→
`采样色 x 1.0` 恒等 —— 渲染输出与无 SetTint 命令的旧路径逐位相同
（T-Alpha-01 像素断言 + 既有 126 条 wgpu 契约全绿共同证明）；场景侧
唯一变化是属性表多一个缺省键（指纹采样面，非行为面）。已更新
`expected_hash.txt` → **旧 `b5b51dac27012ace` / 新 `8fce32749d0d0bc2`**。

### 2.4 alpha 像素证据（nes-render-wgpu/tests/criterion_alpha_contract.rs）

照 criterion_sprite_contract 手法（离屏消费 + 读回断言）：

- T-Alpha-01：无簿记 = 恒等（RGB/A 全 255 逐位）；alpha 0.5 → 读回
  `[255,255,255,127]`（RGB 不变、A = 127 —— 直 alpha 写入）；
- T-Alpha-03：RGB 相乘路径 `255 x 128/255 = 128`（与 E-1 同族）；
- T-Alpha-04：同键覆写后写者生效、未知句柄静默、销毁随条目清理、
  图集格路径同查 tint。
- 提取层簿记断言在 nes-render-extract（`alpha_channel_pushes_set_tint`：
  255/127/夹取/销毁清理 + 命令流出现位）。

---

## §3 scale 通道与到站信号

### 3.1 `tween_scale "name" sx sy ms ["easing"] ["mode"]`

- 写 `Transform2D.scale`，x/y 按**同一插值量 te 各自推进**（两轴独立
  起终点，同步插值）；from = 落地时当前 scale（不跳变）。
- 与 pos 同注册表（`tweens`），通道字段区分（`TweenChannel::Scale`）；
  last-wins 按（节点，通道）二元组 —— **pos 与 scale 并存互不干扰**
  （T-S-01：并存登记、各自推进、重发 scale 不打断 pos）。
- scale 无 schema 夹取（变换字段，非属性表）—— 负值/零原样语义
  （与编辑器手改 scale 同权）。

### 3.2 到站信号 `tween_done`

- **载荷**：`Value::Str(节点名)`；**源**：`src = None`（引擎源，照
  `tree/*` 桥信号口径）。
- **时序**：推进阶段（1.75）发 —— 泵（阶段 5）前，同 tick 送达；
  时满落位同帧，脚本读终值与收信号同帧（T-TW-05 口径延续）。
- **每通道完成各发一条**：pos + alpha 同帧完成 = 两条 `tween_done`；
  次序 = 注册序（登记表顺序推进，确定性）—— T-SIG-01 用双节点双通道
  断言载荷与次序（`first = "aaa"`、`second = "bbb"`）。
- **loop 不发**：永不完成即永不到站（§1.2 口径）。

---

## §4 门禁

| 项 | 结果 |
|---|---|
| 十 crate `cargo test --release` | 全绿 **745 / 0 failed**（asset 34、audio 52、ext-api 7、ext-js 29、media 27、render-api 45、render-extract 57、render-wgpu 126、runtime 116、scene 252） |
| clippy（`--release --all-targets`） | **0 警告 × 10** |
| 依赖方向守卫 `check_dependency_direction.py` | **15/15**（G13 白名单未动，零新依赖） |
| Dodge 基线 `t_abi_01` | 绿（重录后全套回归：runtime 116 全绿含 600 帧确定性） |
| tween_demo 冒烟 | ① headless CLI 300 帧跑两遍 `trace hash 6a6639bc1b806edb` 逐位相同；② `NES_GAME_FRAMES=180 cargo run --release --example tween_demo` 干净退出 |

demo 更新（§1.2 实证修正的落地）：tween_demo.ron 增第二行方块 ——
`ease_out + yoyo` 往返 + `on "tween_done"` 直接再起程；第一行方块叠加
`alpha` 呼吸（`smoothstep + loop`，永不移除永不到站）—— 三通道、五缓动
之四、三模式中的两模式同屏对照。

---

## §5 遗留（后续里程碑候选）

1. **自定义贝塞尔缓动**：`cubic-bezier(...)` 注册表（脚本侧命名引用）
   —— 形状函数 trait 化后即可挂，当前 5 个冻结成员未预留运行时扩展点
   （刻意：解析期名单校验需要封闭集合）。
2. **编辑器时间轴**：关键帧轨道（多段 tween 链式编排、缓动逐段可选）
   —— 现有 tween_done 信号链可拼出顺序播放，但创作体验需要真轨道 UI
   与 undo 集成。
3. **弹簧物理**（spring/damping）：速度状态的连续动力学补间 —— 与
   现帧模型（无状态插值）不同族，需要 Tween 结构增速度分量并重审
   指纹/会话态口径。
4. **补间的暂停门控**：v1 冻结面推进不受 `paused` 影响（S16 §5 遗留
   延续）；三通道扩大了该口径的影响面（alpha 呼吸在暂停中照跳），
   若要"暂停即冻结"需给 Tween 挂 process_mode 门控 —— 语义裁决归
   专门里程碑。
