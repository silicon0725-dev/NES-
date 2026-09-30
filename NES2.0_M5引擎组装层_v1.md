# NES 2.0 · M5 引擎组装层 v1

> 交付日期：2026-09-30　｜　状态：**引擎闭环贯通（nes-runtime 新 crate，全链六 crate）**
> 前置：M4 全部里程碑（S4.1~S4.7）。本轮是方案 D 的**收官拼图**：
> 真实场景树 + 磁盘资产驱动整条渲染管线 —— NES 2.0 第一次以"引擎"的形态运转。

---

## 0. 一句话结论

新增 `nes-runtime` 组装层 crate（零第三方依赖，只组合不实现语义），把五个既有
crate 装配成一条帧循环：**场景树节点 -> nes-asset 磁盘加载 -> GPU 纹理注册 ->
每帧提取 -> 命令流 -> 像素**。出口准则 E-Loop-01..04 全过（含**磁盘热重载直达
像素**），演示示例三精灵 + 父子复合 + 翻转 + 热重载出图正确。全仓六 crate
测试 **75 / 34 / 40 / 42 / 79 / 4 全绿**，守卫扩至 **G1~G11**。

---

## 1. 形态

### 1.1 位置与纪律

```text
nes-asset ──▶ nes-scene ──▶ nes-render-extract ──▶ nes-render-api ──▶ nes-render-wgpu
     └────────────┴──────────────┴────────────────────┴──────────────┘
                        全部被 nes-runtime 消费（G11：顶端叶子）
```

- **只组合，不实现语义**：变换/z 序/相机/锚点/排版算式仍冻结在各 crate；
  上游五行未动（本轮改动全部在新 crate 与守卫脚本里）；
- 零第三方依赖（五条 path 依赖），空 `[workspace]` 钉成独立工作区根；
- **G11**（守卫新增）：组装层只许向下依赖五个项目 crate（全 path）、零第三方、
  独立工作区根、无人反向依赖 —— 守卫 11/11。

### 1.2 关键衔接：三处身份由同一组位对齐

场景节点引用的 `ResId` 槽位 -> `ResourceTable` 绑定出 `AssetKey` ->
`AssetKey::as_render_key().to_bits()` 与提取层 `render_key_of_bits` 是**同一位镜像**
-> GPU 注册表按 `RenderAssetKey::from_bits(...)` 收纹理。于是"场景里的这个节点
画哪张磁盘图片"全程不需要任何查表翻译，位即身份（M3 埋的线今天接通）。

### 1.3 API 概览

| 方法 | 职责 |
|---|---|
| `open_with_root(root, w, h)` | 装配（GPU 上下文/目标/管线 + 场景树/资源表/注册表/提取器/服务端） |
| `tree_mut` / `resources_mut` / `registry_mut` / `consumer_mut` | 宿主编辑面 |
| `declare_texture(path)` | 声明纹理槽位（记录供上传遍历） |
| `bind_assets()` | 表 -> 注册表（注册/加载/持有，幂等） |
| `poll_reloads()` + `upload_pending_textures()` | 热重载：内容戳轮询 -> 版本变化重传 GPU |
| `frame(&FrameInfo)` | 一帧：apply_pending -> refresh_transforms -> extract_into（含 submit）-> consume |

辅助：`write_bmp_rgba`（测试/演示资产生成，与 `nes-render-wgpu::bmp` 解码器互逆）。

## 2. 出口准则（`tests/criterion_engine_loop.rs`，4/4）

| 编号 | 验证什么 | 结果 |
|---|---|---|
| E-Loop-01 | 全链路首帧：磁盘 BMP -> 加载 -> GPU 注册 -> 提取 -> 命令 -> 像素，四象限逐象限核对；`from_registry=1`、场景相机生效 | ✅ |
| E-Loop-02 | **热重载直达像素**：改磁盘文件 -> poll -> 重传 -> 下一帧四象限全部反转色 | ✅ |
| E-Loop-03 | 删节点 -> 下一帧渲染物消失（生命周期贯通场景层到 GPU） | ✅ |
| E-Loop-04 | 父子变换复合 + flip：容器(16,16)∘子(0,0)∘镜像 -> 象限左右互换且落位精确 | ✅ |

演示示例 `examples/engine_frame.rs`：两张棋盘纹理 + 容器双子精灵 + 翻转精灵 +
热重载（绿棋盘改红），三帧推进，产物 PNG 经目视确认（左侧橙/深蓝棋盘成对、
右侧热重载后红系且镜像位置正确）。

过程中抓到的工程问题：**并行测试共享临时目录竞态**（e_loop_02 改写文件时
e_loop_01 恰好加载，四象限颜色串台）。修正方式是**测试层隔离**（每用例独立
子目录）—— 见 §3 的收官记录与裁决。

## 3. 收官记录：磁盘敏感性与观测链（含共享临时目录竞态）

### 3.1 竞态的形态

```text
Test A（热重载用例）
   │
   ├── 修改 disk asset（demo.bmp）
   │
   ▼
shared temp directory
   ▲
   │
   └── Test B（首帧用例）正在读取
```

两个用例各自正确，共享目录后互相污染 —— **这不是架构缺陷，是架构生效的
证据**：Asset Pipeline 已经真实地对磁盘内容变化敏感。在此之前，资产系统的
测试验证的是 `input -> output` 的纯函数路径；E-Loop 之后，测试链变成了：

```text
filesystem state
       ↓
asset observation（内容戳轮询）
       ↓
runtime state（版本变化 -> GPU 重传）
       ↓
rendered pixels（下一帧象限反转）
```

这条四层观测链第一次**全链同时存在**，竞态只是它对并发观察者的自然推论。

### 3.2 裁决（记录在案）

- **修测试层，不改 Runtime 语义**：每用例独立子目录即够。不为测试便利给
  `NesRuntime` 加锁/加路径沙箱/加隔离参数 —— 引擎对磁盘敏感是**特性**
  （热重载的根基），不是要被掩盖的缺陷；
- E-Loop-02 因此有了双重身份：它既是热重载的出口准则，也是"磁盘状态可被
  观测到像素"这条链的端到端锚定。

## 4. 收官判断：两条互相加强的验证

里程碑弧线走到这里：

```text
M1  基础资源/身份      M2  场景/渲染语义      M3  资源身份贯通
M4  渲染链路           M5  Runtime Assembly
        ↓                    ↓
   引擎首次闭环        Text Rendering
                             ↓
                       更多 Renderables
```

**M5 证明了"引擎能跑"**：场景树 + 磁盘资产驱动整条管线，热重载直达像素
（E-Loop-01..04）。

**文字渲染证明了"引擎能够持续增加新的语义对象，而无需破坏既有渲染架构"** ——
新 Renderable 不需要新 Renderer。Glyph 的展开路径：

```text
Text -> Glyph expansion -> Quad instance -> existing batch -> existing draw
```

与 M5 的 Assembly 路径：

```text
Scene -> Extraction -> Commands -> Renderer
```

**正好无缝组合**：字形是实例流里的普通四边形，控件、注册表纹理、内建格同理；
T-Text-11（三类同帧同管线）与 E-Loop（场景驱动）分别在两端钉住了这个组合性。

这两个验证叠在一起，比单独完成一个 Text Renderer 有价值得多：前者是引擎的
**存在性证明**，后者是引擎的**可扩展性证明**。后续每新增一类 Renderable
（形状、粒子、九宫格……）都应沿同一判据验收：语义进契约/提取层，像素进
既有实例流，T-XX-11 式的同帧混画作架构回归。

## 5. 实测基线（最终）

| 检查项 | 结果 |
|---|---|
| `nes-runtime` | test **4** 全绿 ｜ clippy `--all-targets -D warnings` 零警告 ｜ `engine_frame` 示例 PASS ｜ 连续 3 轮无抖动 |
| 全仓 | **75 / 34 / 40 / 42 / 79 / 4** 全绿 ｜ 守卫 **G1~G11 = 11/11** ｜ 上游五 crate 零改动 |

## 6. 下一步候选

1. **窗口 / surface**：把 `frame()` 的离屏目标换成真实窗口（Win32 手写 FFI +
   `wgpuInstanceCreateSurface`），帧循环已是现成的；
2. **`tick` 接线**：场景层的 `enter/process` 生命周期与 `Cmd` 命令缓冲接入
   帧循环（当前只用结构落地 + 变换冲洗两条）；
3. **场景序列化闭环**：`SceneDoc` 从磁盘实例化 -> 渲染（`scene_io` 已冻结，
   只差组装层调用）；
4. headless Linux 认证 / M5 Scratch 兼容层（外围，口径已备）。

*（内容由AI生成，仅供参考）*
