# NES 2.0 · S12.4 编辑器体验硬化 v1

> 交付日期：2026-10-03　｜　状态：**用户实测七项反馈全数落地——自适应/surface 重配/草稿生命周期/帧节拍/滚轮结论**（分支 s12-4-ui-hardening，worktree 隔离交付）
> 前置：S12.3 裁剪与滚动（223c370）；S12.2 文本输入。

---

## 0. 一句话结论

用户试玩 editor_shell 报出的七项体验问题全数处理：**最大化拉长** =
surface resize 重配（运行时每帧自动同步）+ 宿主每帧布局投影（面板恒宽
不拉伸）；**Tab/换选/撤销后输入框不刷新** = 一个根（草稿生命周期）——
获焦沿统一初始化草稿 + `reset_text` 换绑 API + undo/redo 后重绑；
**右上角标题裁剪** = 标题随右面板每帧投影；**交互延迟** = 壳层
`sleep(16ms)` + FIFO vsync 双重等待（删固定 sleep，实测帧差喂 delta）；
**滚轮** = 全链逐环查证无 bug——列表内容装得下时 `scroll_max=0`
本就不滚（设计），且装得下时滑块也不再画（"没有滚动条 = 没得滚"
观感自洽）。六 crate **532 测试全绿**、clippy 0、守卫 11/11。

## 1. 自适应与工作区分离

- **surface resize 重配**（`nes-render-wgpu`）：`SurfaceTarget::reconfigure`
  ——wgpu-native 的 `wgpuSurfaceConfigure` 语义允许同句柄重复配置（替换
  式），格式/用法/present mode 与装配逐项同源；`NesRuntime::
  sync_surface_to_window()` 在 `frame_windowed_with` 帧首自动调用
  （泵后、模拟前；客户区 0 = 最小化跳过）。RenderTarget（离屏读回）
  与 surface 尺寸**独立**，重配不影响 `frame_with` 读回路径（T-Sync-02）。
- **宿主每帧布局投影**（editor_shell）：视口 = 窗口真实客户区
  （`window_client_size()`），左层级面板恒宽 180、右检查器恒宽 190、
  状态栏贴底——全部每帧写 offset/size，**面板不随窗口拉伸**，中间
  视口吃剩余区域（相机中心每帧 = (cw/2, ch/2)）。工作区/面板/视口
  三区由此结构性分离。
- `grep 768|432` 清理：开窗尺寸保留为**初始**值（OPEN_CLIENT 常量），
  一切运行期几何由客户区推导。

## 2. 输入框草稿生命周期（三连不刷新的根）

| 症状 | 根 | 修 |
|---|---|---|
| Tab 切换后文字不变 | 获焦不初始化草稿 | `focus_node` 单点：TextInput 获焦沿（点击/Tab 同路）`draft = text 属性值, caret = len` |
| 换选节点后不刷新 | 草稿跟节点不跟选区 | `UiVm::reset_text(node, value)`（置草稿/光标、不动焦点、不触发提交）；editor_shell 选中变化即换绑 |
| Ctrl+Z 后不刷新 | 同上 | undo/redo 应用点同接 reset_text（双写绑定处统一，滞留 sink 先 drain） |

Button 占焦不建编辑会话、失焦不伪造提交（S12-2 语义保持）。
契约：T-UI-09/10（获焦初始化双路径、reset_text 三不：不动焦点/不触发
提交/可重复）。

## 3. 帧节拍（延迟不跟手的根）

`frame_windowed_with` 内时序本无等待（pump 非阻塞、FIFO present 自节
流、无读回无 flush）；延迟来自壳层帧循环固定 `thread::sleep(16ms)`
**叠加** vsync 等待（最坏 ~2 个垂直周期上屏）。修复：删固定 sleep，
`FrameInfo::delta` 用 `Instant` 实测帧差（clamp ≤0.1s 防切后台大步长）；
`NES_GAME_FRAMES` 冒烟语义不变。

## 4. 滚轮结论（查证无 bug）

全链六环逐环实证（窗口消息 → 折叠 → 快照 → SnapshotView → UiVm 路由
→ 烘焙）无缺陷。editor_shell 层级列表节点少时 `scroll_max = 0` →
滚轮命中后 clamp 值不变 → 不滚——**设计而非缺陷**；且本次起内容装得
下时滑块也不再绘制（提取层 `frac >= 1 → scroll_bar = None`，T-SCL-03/08
钉死）——"没有滚动条 = 没得滚"观感闭环。行数超出列表高度后滚轮与
滑块即生效。

## 5. 契约回归与门禁

| 套件 | 增量 |
|---|---|
| nes-render-wgpu | T-Surf-05（surface 重配后 client_size 一致）→ 98 |
| nes-runtime | T-Sync-01..03（sync 幂等/离屏 false/重配后离屏读回不受扰）→ 70 |
| nes-render-extract | T-SCL-03 装得下分支 + T-SCL-08 边界（恰装下 None / 溢出 1px Some）→ 56 |
| nes-scene | T-UI-09/10 → 229 |

最终门禁：**532 全绿 / clippy 六 crate 0 / 守卫 11/11（worktree 根）/
editor_shell 120 帧冒烟干净退出**。

## 6. 交付形态

本里程碑在独立 git worktree（`wt-s12fix`，分支 `s12-4-ui-hardening`）
完成——主工作区全程未动；"把工作区先分出来"同时落实了两层：交付隔离
（worktree）与编辑器布局的视口/面板分离（§1）。

## 7. 遗留

- 视口独立渲染目标（面板/视口异分辨率）归 S12-4 正题（F-4 集成）。
- 面板可折叠/拖拽分隔条归组件库 P2。
- Ctrl+Z 焦点内文本级撤销（输入框内部的 undo 栈）未做——当前撤销粒度
  是事务级（失焦提交才进事务），输入中 Ctrl+Z 先失焦再撤销。
