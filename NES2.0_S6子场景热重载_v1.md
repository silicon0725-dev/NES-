# NES 2.0 · S6.7 子场景热重载 v1

> 交付日期：2026-10-01　｜　状态：**改子场景文件 -> 整树自动重载重展开（S6.6 遗留落地）**
> 前置：S6.6 子场景嵌套、M3 资产热重载（内容戳轮询）。
> 本轮补上"子场景文件是资产，变化就该跟上"的最后一环。

---

## 0. 一句话结论

`NesRuntime` 新增来源跟踪（`load_scene` 记录、`instantiate_scene` 清除）与
[`poll_scene_reload`]：轮询资产注册表，任一 **Scene 类**资产（子场景文件，
展开时已作为资源声明并绑定）内容变化时，从来源**整树重载**（重新解析根
文件、重新展开子场景读到新内容、全量替换、重新绑定）。出口准则
T-SubR-01..03 全过（热重载直达像素 / 幂等不重复 / 无来源安全）；示例
`subscene_reload` 双 PNG 取证。全仓测试 91 / 34 / 42 / 40 / 83 / **19** 全绿，
守卫 11/11，clippy 零警告。

---

## 1. 机制与裁决

### 1.1 为什么几乎不用写新东西

子场景文件在 S6.6 的展开中就已经作为 `kind: "Scene"` 资源声明进表、随
`bind_assets` 加载进资产注册表 —— **它本来就在 M3 内容戳轮询的范围内**
（T-Scene-05 的"资产 2 项"早就数到了它）。缺的只是两块：

1. **来源跟踪**：树是哪份根文件加载的（`scene_source`，`load_scene` 记录 /
   `instantiate_scene` 清除）；
2. **变化 -> 重载**：`poll_scene_reload` 把"Scene 类资产重载了"翻译成
   "从来源整树重载 + 重绑定"。

### 1.2 整树重载的语义口径（冻结在方法文档）

- **场景文件是事实来源**：加载之后的宿主侧树编辑会被重载丢弃 ——
  与 `load_scene` 的全量替换语义一致，热重载不开例外；
- 生命周期重放：下一 tick 全部节点重新 enter/ready；
- 旧 `NodeId` 全部失效（新树新 arena），观察者持有的句柄需重新寻址；
- **根场景文件自身的变更不在检测范围**（它不是自己 resources 段里的资产
  条目）—— 宿主改根文件后重调 `load_scene` 即可；
- `instantiate_scene` 直入的树没有来源：有变化也不重载（如实返回 `None`，
  不假装做了）。

### 1.3 与 `poll_reloads` 的关系（二选一）

`poll_scene_reload` 内部做一次**全量**轮询（纹理变化也被消耗：版本照常
递增，随后的 `upload_pending_textures` 照常工作）。因此宿主帧循环里
`poll_reloads` 与 `poll_scene_reload` **二选一**，不要都调（第二个什么都
看不到）。推荐组合：

```rust
if rt.poll_scene_reload()?.is_some() {
    rt.upload_pending_textures()?;   // 账目随替换清零，纹理重传
}
```

### 1.4 刻意不做（记录）

- **外科手术式局部重展开**（只重建被改子场景的实例子树、保留其余宿主
  编辑）：需要包装节点 -> 子文件的实例来源跟踪、对当前表的增量槽位合并、
  GPU 侧孤儿纹理回收。整树重载是"文件即事实"的最小正确语义；局部重展开
  等实例级覆盖（Godot override）一起属于后续里程碑；
- 根文件变更检测（把根注册为资产或另存戳）：根是加载入口，宿主重调
  `load_scene` 语义已足够，不为它发明第二条资产通道。

## 2. 出口准则

### 2.1 运行时（`nes-runtime/tests/criterion_subscene_reload.rs`，3/3）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-SubR-01 | 改子场景文件（精灵挪位 + 新增精灵）-> `poll_scene_reload` 返回来源路径 -> 上传账目清零重传 -> 下一帧 drawn=2、新位置像素到位、旧位置回背景 | ✅ |
| T-SubR-02 | 幂等：无变化返回 None 且树不重建；触发重载后再轮询 None（戳已消费，不循环重载） | ✅ |
| T-SubR-03 | 无来源安全：`instantiate_scene` 直入后，场景文件变化不触发重载；**纹理热重载照常生效**（反色重传直达像素，既有路径不受影响） | ✅ |

### 2.2 演示（`examples/subscene_reload.rs`）

加载父场景（单精灵）出图 -> 改写子场景文件 -> `poll_scene_reload` 整树
重载 -> 双精灵新布局出图。产物：`output/subscene_before.png` /
`output/subscene_after.png`，自带像素锚点断言（(26,10)/(10,34) 黄、
(10,10) 背景）。

## 3. 遗留与后续

| 事项 | 状态 |
|---|---|
| 局部重展开（只重建被改实例的子树） | 未启动（整树重载已含覆盖保留，S6.8） |
| 实例级属性覆盖（Godot override 语义） | ✅ S6.8（见 `NES2.0_S6实例级属性覆盖_v1.md`） |
| AssetRegistry 以 Scene 键缓存已解析文档（当前每处引用独立读盘） | 性能项，未启动 |
| 编辑器"编辑子场景实例并存回子文件" | 未启动（回写边界已备好） |
| SignalBus / WM_SIZE / DPI / headless Linux | 沿各文档遗留表 |

## 4. 记账

- 测试基线：nes-scene 91 / nes-asset 34 / nes-render-api 40 /
  nes-render-extract 42 / nes-render-wgpu 83 / **nes-runtime 19**
  （16 -> 19，+T-SubR 3）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-runtime（`scene_source` 字段 + `load_scene`/`instantiate_scene`
  维护 + `poll_scene_reload`）与新测试/示例；场景/提取/渲染层零改动。

*（内容由AI生成，仅供参考）*
