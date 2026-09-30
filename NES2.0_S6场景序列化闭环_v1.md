# NES 2.0 · S6.3 场景序列化闭环 v1

> 交付日期：2026-10-01　｜　状态：**磁盘场景文件成为事实来源（load/save 接入帧循环）**
> 前置：S6.2 tick 接线、M5 组装层。本轮把 `nes-scene::scene_io`（M2/M3 已
> 冻结的解析/打包/资源槽位语义）接进运行时 —— M5 §4 候选 3 落地。

---

## 0. 一句话结论

`NesRuntime` 新增三个场景生命周期方法：`load_scene(rel_path)`（磁盘 RON ->
实例化 -> **全量替换**树/表/纹理槽位/上传账目）、`instantiate_scene(doc)`
（文档直入）、`save_scene(rel_path)`（当前树 + 资源表打包落盘）。属性里的
`Resource(n)` 槽位号经磁盘往返原样复原 —— **场景节点引用、资源表、GPU 注册
表三处身份位在文件两侧同源**（M3 埋的线接通到磁盘）。出口准则
T-Scene-01..04 全过（含"程序搭树 -> 落盘 -> 全新运行时读回 -> 像素一致"的
完整往返）。全仓测试 75 / 34 / 42 / 40 / 83 / **12** 全绿，守卫 11/11，
clippy 零警告。

---

## 1. 接的什么线

### 1.1 API 面（nes-runtime，全部组合既有语义）

| 方法 | 职责 |
|---|---|
| `load_scene(rel_path)` | 相对资产根读 RON -> `parse_ron` -> [`Self::instantiate_scene`]。读失败/解析失败如实报错（指名文件与解析位置） |
| `instantiate_scene(doc)` | `instantiate_doc_with_resources` -> 替换 `tree`/`table`；纹理槽位按表声明序重建上传队列；`uploaded_version` 清零 |
| `save_scene(rel_path)` | `write_ron_with_resources`（verbose）落盘。与 load 互逆 |

`scene_io` 一行未动 —— 槽位逐号复原、悬垂引用体检（`AdoptReport`）、
前向兼容（未知属性/版本门槛）都是 M2/M3 冻结语义，本轮只做组装层调用。

### 1.2 替换语义（裁决记录）

`load_scene` 是**全量替换**，不是合并：当前树/表/上传账目一并丢弃 ——
场景文件是事实来源（facts-on-disk）。两个直接推论，都有测试钉住：

- 替换后同纹理也会重传一次（账目清零，T-Scene-04 断言 `上传 B == 1`）；
- 旧场景的节点不可能泄漏进新帧（旧树整体丢弃）。

### 1.3 一条实坑：延迟结构 vs 序列化

`add_node` 立即在 arena 占位（返回可用 `NodeId`）但**结构挂接延迟到
apply_pending/tick**。程序化搭树后直接 `save_scene`，pending 里的节点不会
出现在文件里 —— T-Scene-03 首跑就撞上了（落盘文件只有 root 一个节点）。
裁决：**save 保持纯读语义**（不在保存内部偷偷 apply），口径写进
`save_scene` 文档：搭树后先 `tree_mut().apply_pending()`（或推进一帧）再存。
帧循环正常路径每帧 tick 已落地，编辑器流程天然安全。

## 2. 出口准则（`nes-runtime/tests/criterion_scene_loop.rs`，4/4）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Scene-01 | 手写 RON 落盘 -> load -> bind -> upload -> 首帧：四象限逐象限像素、`from_registry=1`、场景相机生效、体检干净 | ✅ |
| T-Scene-02 | 悬垂引用（`Resource(7)` 无声明）：`undeclared` 指名、无纹理可传（上传 0）、该精灵不入画且不崩溃 | ✅ |
| T-Scene-03 | 往返闭环：程序搭树 -> `save_scene` -> **全新运行时** `load_scene` -> 像素与源一致 | ✅ |
| T-Scene-04 | 替换语义：load A 渲染 -> load B（同纹理新布局）-> 旧位置回背景、新位置像素到位、账目清零重传一次 | ✅ |

（解析器/打包器的格式语义是 `nes-scene/tests/m2.rs`/`m3.rs` 的领土；
本组证明组装层接线与身份贯通。）

## 3. 身份链全景（S6.3 之后）

```text
场景文件(.ron)  "texture": Resource(1)
      │ parse_ron + instantiate_doc_with_resources（槽位逐号复原）
      ▼
节点属性 Value::Resource(1) ── ResourceTable(槽位1 -> Textures/demo.bmp)
      │ bind_assets                              │ FsLoader(root) 加载
      ▼                                          ▼
提取层 render_key_of_bits ←── AssetKey::as_render_key().to_bits()（位镜像）
      ▼
GPU 注册表 RenderAssetKey::from_bits —— 同一组位，三处同源，磁盘两侧不变
```

热重载（`poll_reloads`）与场景加载正交：替换场景后宿主照常轮询即可，
版本账目从零起算。

## 4. 复现入口（`nes-runtime/examples/scene_disk.rs`）

`cargo run --example scene_disk` 一条命令走完闭环，三个产物：

1. 手写场景（事实来源）`%TEMP%/nes_runtime_scene_disk/Scenes/handwritten.ron`
   —— 资源声明段（槽位 1/2）、容器(8,8) + 两精灵、单位相机，语法与
   `doc_to_ron` 输出同门；
2. 渲染图 `nes-runtime/output/scene_disk.png`（64x64，四象限 + 棋盘）；
3. 回存场景 `Scenes/roundtrip.ron` —— 当前树经 `save_scene` 落盘（verbose
   全量模式，schema 默认属性一并带出，资源段槽位原样复原）。

示例自带像素锚点断言（(10,10) 黄 / (34,10) 橙 / (4,4) 背景），不依赖目视。

## 5. 边界与刻意不做

- **不做场景合并/差量加载**：全量替换是最小正确语义，编辑器多场景组合
  （子场景 `sub_scene` 递归展开）属后续里程碑（草案 §10 预留）；
- **camera 视口仍是宿主责任**：场景文件里的 Camera2D 位置是场景事实，
  视口尺寸来自 `FrameInfo`（帧调用时给定），加载侧不代改；
- **T-Scene-02 的悬垂精灵计入 `skipped` 还是 `ignored` 的细分口径未钉**
  （本轮只断言 `drawn == 0` 与体检报告，不做超出口径的断言）；
- 保存格式只有 RON（`PackOptions::verbose`）；JSON 交换格式沿草案 §10
  后置口径。

## 6. 遗留与后续

| 事项 | 状态 |
|---|---|
| 子场景嵌套（`sub_scene` + AssetKey 递归实例化） | 未启动（草案 §10 预留） |
| 编辑器侧差量保存（`to_doc_omitting_defaults` 已具备，组装层未暴露） | 按需 |
| `paused` / `time_scale` / `ProcessMode`（草案 §9） | 未启动 |
| WM_SIZE 重配置 / DPI / 多 GPU（S6.1 遗留） | 未启动 |
| headless Linux 认证 / M5 Scratch 兼容层 | 沿 M5 §4 候选 |

## 7. 记账

- 测试基线：nes-scene 75 / nes-asset 34 / nes-render-api 40 /
  nes-render-extract 42 / nes-render-wgpu 83 / **nes-runtime 12**（8 -> 12，
  +T-Scene 4）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 产出物：`nes-runtime/output/scene_disk.png`（演示示例渲染图，见 §4 复现入口）；
- nes-scene / nes-scene::scene_io / 提取层 / 渲染层：**零改动**（本轮全部
  落在 nes-runtime、新测试文件与新示例）。

*（内容由AI生成，仅供参考）*
