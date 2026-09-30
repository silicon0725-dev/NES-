# NES 2.0 · S6 窗口与 Surface v1（S6.1）

> 交付日期：2026-09-30　｜　状态：**帧循环上真窗口（离屏与窗口共用同一条绘制路径）**
> 前置：M5 引擎组装层。本轮把帧循环从离屏 `RenderTarget` 搬到 **真实 Win32 窗口
> 表面**并逐帧呈现 —— 全程手写 FFI，零第三方依赖的纪律不动。

---

## 0. 一句话结论

新增 `window.rs`（Win32 窗口 + 消息泵）、`SurfaceTarget`/`SurfaceFrame`
（wgpu-native surface 装配/获取/呈现）、`consume_to_surface`（与离屏 `consume`
共用抽取出的 `draw_into`）；`nes-runtime` 增 `open_windowed_with_root` /
`frame_windowed`。**窗口路径没有新增任何渲染语义**：同一条命令流在离屏和窗口
两种目标上产出同一本账。出口准则 T-Surf-01..04 全过；示例 300/600 帧呈现 +
帧 150 磁盘热重载**直达窗口像素**（程序化截屏逐像素核验）；全仓测试基线升至
75 / 34 / 40 / 42 / **83** / 4 全绿，守卫 G1~G11 11/11，clippy 零警告。

---

## 1. 形态与纪律

### 1.1 为什么手写 Win32 FFI

本 crate 依赖纪律禁第三方 crate（winit 越界）。窗口是 surface 的前置
（`WGPUSurfaceSourceWindowsHWND` 要 HWND），所需 Win32 面极小：注册类、
`AdjustWindowRect` 建窗、`PeekMessage` 泵、销毁。全部 `extern "system"` 声明 +
`user32`/`kernel32` 隐式链接（Rust 标准库已链），无 build script —— 与
wgpu-native FFI 同一套方法论。

### 1.2 分层与调用链（本轮改动全部向下叠加，上游五 crate 语义零改动）

```text
ffi.rs      surface 常量/结构/符号（SourceWindowsHWND、Capabilities、
            Configuration、SurfaceTexture、present/unconfigure/...）
window.rs   Window：RegisterClassW -> AdjustWindowRect -> CreateWindowExW ->
            ShowWindow；pump()（PeekMessage 循环，WM_QUIT -> false）
gpu.rs      SurfaceTarget::new（create_surface -> get_capabilities 校验 ->
            选 RGBA8Unorm -> configure Fifo/Auto/RenderAttachment）；
            acquire / present / release_frame；Drop：unconfigure + release
renderer.rs draw_into（从 consume 抽取：命令处理 -> 相机 -> 字形展开 ->
            控件逆视图折算 -> 注册表选源 -> render）；
            consume_to_surface = acquire -> draw_into -> present -> release
runtime     open_windowed_with_root（GPU 先行防闪窗 -> 窗口 -> 表面）；
            frame_windowed（pump -> 提取 -> consume_to_surface）
```

**关键结构裁决**：`render()` 的目标从 `&RenderTarget` 改为裸视图指针 +
视口尺寸，`consume`（离屏读回）与 `consume_to_surface`（窗口呈现）共用
`draw_into` —— "文字/精灵/控件如何画"的知识只有一份，画到哪里是正交的。

### 1.3 身份与生命周期口径

- **表面在消费器之后创建、之前释放**：`SurfaceTarget::new(consumer.ctx(), &window)`
  借用 `GpuContext`（instance/adapter/device 同源），Drop 序 surface -> consumer
  -> window 由 `NesRuntime` 字段声明序保证（T-Surf-04 显式钉死）；
- **格式恒 RGBA8Unorm**：caps 不含该格式即装配失败（如实报错，不静默换格式
  换出一条未验证的管线）；
- **呈现模式 Fifo**（垂直同步），`alpha_mode = Auto`；
- 窗口关闭（点 X）=> `pump()` 返回 false => `frame_windowed` 返回 `Ok(None)`，
  宿主帧循环应停止 —— 关窗是正常退出路径，不是错误。

## 2. 出口准则（`nes-render-wgpu/tests/criterion_surface_contract.rs`，4/4）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Surf-01 | 客户区精确等于请求尺寸（AdjustWindowRect 按真实系统度量外扩）；表面尺寸 = 客户区；格式 = RGBA8Unorm | ✅ |
| T-Surf-02 | 命令流经 `consume_to_surface` 呈现：计数与离屏同账（drawn/from_registry/ignored）、`frame_index` 透传自 Submit、零 driver errors | ✅ |
| T-Surf-03 | 同一表面连续三帧 acquire/present 复用：计数不串帧、帧序独立 | ✅ |
| T-Surf-04 | 析构序 surface -> consumer -> window 走完进程干净存活 | ✅ |

注：这些用例会**短暂弹出真实窗口**（每条约 1 秒）—— 窗口是表面契约的物理
组成部分。surface 像素不读回（交换链无免费读回通道），画面正确性由下节的
示例 + 程序化截屏承担。

## 3. 端到端验证：示例 + 截屏像素契约（`nes-runtime/examples/engine_window.rs`）

### 3.1 示例

512x288 窗口、两张棋盘纹理、正弦动画容器（双子精灵）+ 翻转精灵 + HUD 控件；
默认 300 帧（`NES_WINDOW_FRAMES` 可延长），第 150 帧改写 `grid.bmp`（绿 -> 红）
并 `poll_reloads` + `upload_pending_textures`。运行记录：

```text
[窗口] 已创建并显示
[帧 150] 检查 2 张，热重载 1 张（右侧翻转精灵应变红）
[帧 150] 重传 1 张
[完成] 共呈现 600 帧到窗口表面        （无任何 driver_errors 行）
```

### 3.2 程序化截屏像素校验（方法论记录）

视觉模型通道被内容过滤拦截（截屏含宿主桌面内容），改走**确定性验证**：
`PrintWindow(PW_RENDERFULLCONTENT)` 直取窗口客户区（绕过遮挡与 Z 序）→
`LockBits` 字节级采样。修正字节序（GDI 内存序为 BGRA）后，帧 150 前后两次
截屏逐项核对：

| 契约位置（逻辑像素） | 帧前（绿帧） | 帧后（红帧） |
|---|---|---|
| 背景铺满客户区 (13,13,25) | ✅ 全覆盖 | ✅ 全覆盖 |
| HUD 边框 [288,496)x[16,64) 纯绿、内部全透明 | ✅ box=(288,16)-(495,63)，内部 0/2800 非背景 | ✅ 同 |
| 翻转精灵 [184,200)x[48,64)（绕左上原点镜像，与离屏契约一致） | ✅ 绿对 (0,200,120)/(20,30,50) 各 128px | ✅ **换成红对 (200,40,40)/(50,20,20)** |
| 棋盘 A/B 精灵在正弦轨道 y∈[32,48) | ✅ 绿帧 box 起点 x=42（帧 ~81 上升沿的解析位置） | ✅ 红帧 x=56（sin 峰值位） |

**结论**：热重载改动磁盘 -> poll -> 重传 -> 下一帧窗口表面像素变化，闭环成立；
窗口路径与离屏路径的绘制语义（含 flip 镜像口径）逐像素一致。

## 4. 过程中抓出的三个真 bug（症状 -> 证据 -> 修法）

1. **窗口模式热重载 0 张**（示例首跑）。根因：`open_windowed` 硬编码资产根
   `Path::new(".")`，示例把资产写在临时目录 —— 两路径对不上，加载全失败。
   而 `upload_pending_textures` 对"未就绪"槽位按设计静默跳过（状态留在表里
   待宿主查询），`poll_reloads` 没有 Ready 候选 → 0 张，expect 全不炸，窗口里
   实际只有背景 + HUD。修法：补 `open_windowed_with_root(root, ...)`（与离屏
   `open_with_root` 对齐），公共装配抽 `assemble`；`open_windowed` 保留为
   root="." 的便捷形式。**教训：静默跳过链 + 错误根目录 = 一切"正常"但什么
   都没画**——这也是示例日志打印 `checked/reloaded/重传` 三个数的原因。
2. **客户区 518x294 ≠ 512x288**。根因：建窗用硬编码边框补偿 +16/+39，本机
   实际度量不同。修法：`AdjustWindowRect` 按真实系统度量把客户区期望外扩成
   整窗尺寸（修后客户区分毫不差）。
3. **窄窗口客户区 162px 钳制**（T-Surf-01 首跑 128 请求得 162）。根因：Win32
   为容纳标题栏最小/最大/关闭按钮强制最小宽度。修法：测试窗口加宽到 256，
   并把"客户区精确性契约只在标题栏最小宽度之上成立"写进测试口径注释。
   （还有一条工具链教训：Windows PowerShell `Out-File` 默认 UTF-16，会让
   `grep` 对枚举结果假阴性 —— 排查窗口存在性时被它误导过一轮。）

## 5. 兼容性口径与预留

- **compatibleSurface 未用**：当前 `GpuContext::open` 无 surface 先行装配，
  adapter 选取不看表面。本机（单 GPU）caps 校验通过即事实兼容；多 GPU 环境若
  出现"选到的适配器与表面不兼容"，预留路径是 `open_windowed_with_root` 里
  先建窗口、以 `RequestAdapterOptions.compatible_surface` 重走装配（字段已在
  FFI 里备好）—— 属 S6 后续按需启用，不预先实现。
- **WM_SIZE 重配置**：窗口 resize 后表面仍按创建时尺寸呈现（DWM 拉伸），
  `WM_SIZE -> surface_configure` 重配置属后续里程碑。
- **DPI**：进程未声明 DPI 感知，高 DPI 屏上表面按逻辑像素呈现（DWM 缩放）。
  客户区尺寸与表面配置**同为逻辑像素**，自洽（T-Surf-01 证明），但物理分辨率
  未跟上 —— DPI 感知声明 + `GetDpiForWindow` 属遗留。

## 6. 遗留与后续（S6.1 之后）

| 事项 | 状态 |
|---|---|
| WM_SIZE -> 表面重配置（resize 跟随） | 未启动（本轮固定尺寸口径） |
| DPI 感知（per-monitor v2 + 物理像素表面） | 未启动 |
| 多 GPU：compatibleSurface 装配路径 | 预留未启用（§5） |
| 相机视口与客户区的自动同步（宿主手设口径不变） | 保持显式（文档口径） |
| 鼠标/键盘输入泵（client 坐标折算到世界） | 未启动 |
| tick 接线（enter/process/Cmd） | ✅ S6.2（见 `NES2.0_S6生命周期Tick接线_v1.md`） |
| 场景序列化闭环、headless Linux 认证 | 沿 M5 §4 候选清单 |

## 7. 记账

- 测试基线：nes-scene 75 / nes-asset 34 / nes-render-api 40 / nes-render-extract 42 /
  **nes-render-wgpu 83**（79 -> 83，+T-Surf 4）/ nes-runtime 4 —— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 产出物：`nes-runtime/output/client_v2_green.png`、`client_v2_red.png`
  （程序化截屏像素校验的取证截图）。

*（内容由AI生成，仅供参考）*
