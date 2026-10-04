# NES 2.0 · S15 视频资产面 —— Video 资源 / 逐帧渲染 / 脚本控制 / 音画同播

- worktree：`wt-video`（分支 `s15-video-assets`，基线 HEAD = c02ea7c）；
  **S15.1 升级**：`wt-sync`（分支 `s15-1-av-sync`，基线 HEAD = 431a4e0）——
  音画从"P0 起点对齐"升级为"严格同步：音频钟主控"（见 §3）
- 上游：S14.3（AMV 解码 + IMA ADPCM 音轨，`nes-media::AmvVideo` / `AviVideo` 已就绪）
- 本期把视频从"解码器"升格为**一等资产**：场景声明 Video 资源 → Sprite 显示当前帧 →
  脚本 `video_play` / `video_stop` 控制 → 有音轨则同步出声。

## §0 结论

视频资产面全链打通并在真实 13.5MB AMV（160x128 @ 15fps + IMA ADPCM 音轨）上端到端验证：

```
场景 Res(kind:"Video") ──▶ bind：nes-media 内容探测解析（AMV/AVI）+ 首帧上 GPU
    ──▶ 脚本 video_play "key" ──▶ Cmd::VideoPlay ──▶ tick 后转渲染侧播放状态机
    ──▶ 每帧 当前帧号 同键覆写 GPU 注册表（Sprite 即播画面；S15.1 起
    帧号从声部已播采样位导出 —— 音频钟主控，严格同步）
    ──▶ 音轨即刻开声部（非循环）；video_stop / 声部播完移除即音画同终
```

- 契约测试新增 11 条全绿：nes-scene `tests/s15_video.rs`（6）+ nes-runtime
  `tests/s15_video.rs`（5，真文件 skip-if-missing）；**S15.1 再增 7 条**：
  nes-audio `t_mix17/18`（`voice_position` 采样位口径）+ nes-runtime
  `src/video.rs` 模块内 `t_vfa01/02`（音频钟纯函数）与 `t_vid_sync01..03`
  （回退钟回归 / 保持首帧+音画同终 / GPU 页帧号跟随采样位）；
- 门禁（S15.1 后）：八 crate `cargo test --release` 665 通过（基线 658 +
  新增 7）、clippy 0 警告 ×8、依赖方向守卫 13/13、editor_shell 冒烟
  （NES_EDIT_DEMO=1 420 帧，含 video on/video stopped 取证）、video_demo
  冒烟（NES_GAME_FRAMES=180 真实有声跑：帧号增速与 15fps 一致 ——
  音频钟按真实消耗推进，页号 = 已播秒数 × fps）；
- 提交：见 §4（单提交，`Media/*.amv` 不入库）。

## §1 架构

### 1.1 三条裁决的落地

1. **视频纹理 = 同键逐帧覆写**。`AssetKind` 增 `Video` 变体且
   `is_render_facing() = true`（nes-asset/src/key.rs）—— Video 资源与纹理共用
   `RenderAssetKey` 命名空间机制（键位 = `(slot, gen)`，两类资源天然不同槽位）。
   提取层 `RenderKeySource` 把 Sprite2D 的 texture 属性解析到同一键 → GPU 注册表
   `register` 同键重复调用 = 覆写同一瓦片（字形页/纹理热重载已验证的路径，本
   期 `video_page_swaps` 观测面逐帧取证：30 帧 @60fps 恰好推进到第 7 帧 =
   0.5s × 15fps）。运行侧状态机在 `nes-runtime/src/video.rs`：bind 解析 +
   首帧上传（未起播显示首帧而非黑块）、`play_video` / `stop_video` 单点、
   `advance_videos` 逐帧换页（只在 `frame_with` / `frame_windowed_with` 调）。
2. **schema 提示放宽（additive 最小面）**。查证结论：`H::Resource { kind }` 的
   kind 只在 `ResourceTable::adopt_tree` 的类别体检处比对（编辑期控件过滤用）。
   最终选择：新增 `EditorHint::ResourceMany { kinds }` 变体（编译期常量白名单，
   **不是**任意 kind 放行口），Sprite2D.texture 改为 `&["texture", "video"]`；
   `adopt_tree` 按"可接受全集"核对（表内合法、表外仍如实报 mismatch，报告口径
   类别 = `kinds[0]`）。**未走退路**（新增 Sprite2D `video` prop + 提取层
   admit 支持）——校验面只有 adopt_tree 一处比对，`ResourceMany` 三处改动
   （schema.rs 枚举 + own_props + resources.rs 体检）即封口，面比退路小一个量级。
   契约 `t_vid_s_06` 双向钉住：Video 引用不误报、Script 引用 texture 仍报。
3. **确定性边界**。播放状态（计时/当前帧/音轨键）全在 runtime 渲染侧
   `videos: BTreeMap<ResId, VideoEntry>` —— 不进树、不进序列化、不进语义指纹；
   脚本控制经 `Cmd::VideoPlay` / `Cmd::VideoStop`（树侧单帧缓冲
   `take_video_cmds`，play/stop 混排发射序如实保留；`simulate` 与
   `tick_headless` 两处消费点，照 `PlaySound` 先例；headless 消费即弃）。
   headless 无换页（无 GPU 无纹理，`video_page_swaps` 恒 0）；场景层契约
   `t_vid_s_03` 钉死"消费与否指纹逐位同"，运行时 `t_vid_02` 钉死"同轨迹两遍
   逐位同 + 播态真实建立/清除"。

### 1.2 装载链与热重载

- `declare_video(rel)`（照 declare_texture/declare_sound 同构）→ `bind_assets`
  的视频步 `parse_videos_collecting`：就绪且版本变化的 Video 资源按**内容四字码**
  探测解析（RIFF + `AMV `/`AVI `，不看扩展名 —— 与图片装载"内容说了算"同口径），
  失败按槽位进 `report.failed` 缺口清单（一个坏视频不挡场景，`t_vid_05`）；
- 派生键 = 资源路径去扩展名（**复用 `sound_key_of` 单点**，`Media/spider.amv` →
  `Media/spider`）；音轨键 = `__video_audio__/<派生键>`（前缀非合法资源路径形态，
  与场景声音键空间隔离）；
- 版本账目：热重载换版本 → 重建容器 + 重传首帧 + 停止旧播放（资产换了续播无意义）；
- 内存口径（如实写明）：**整容器驻留内存**（AmvVideo/AviVideo parse 时拷贝整份
  文件字节）+ 音轨解码产物缓存一份 `Arc<Wav>`（`RefCell` 缓存，容器只在宿主线程
  触碰、非 Send/Sync 不共享）—— 与音频面"全轨进内存"同一现状，压缩策略归 §5。

## §2 脚本面

| 语句 | 编译产物 | 语义 |
|---|---|---|
| `video_play "key"` | `Op::VideoPlay { key }` | 起播（幂等：已在播不重启、计时不清零）；有音轨则混音器开非循环声部 |
| `video_stop "key"` | `Op::VideoStop { key }` | 停播 + 停音轨声部（`Mixer::stop_key` 点名停，不碰其他声部）；画面定格当前帧 |

- 语法照 `play` 同款（语句级关键字 + 字符串字面量、零栈交互、两入口同权、
  关键字进 RESERVED）；键未声明静默丢弃（与 play 未注册键同家法）；
- 宿主直调 API：`play_video` / `stop_video` / `video_is_playing` /
  `video_current_frame` / `video_page_swaps` / `video_count` / `active_voice_count`。

## §3 音画严格同步口径（S15.1：音频钟主控，升级自 P0 起点对齐）

- **同步起点**：`video_play` 时刻 = 计时清零 + 音轨声部开声（同一同步点）；
  声部开动后**帧号从声部已播采样位导出**：`帧号 = floor(已播源样本 /
  源采样率 × fps)`（`video_frame_from_audio` 纯函数，f64 中间量）。采样位
  由设备钟真实消耗推进（混音器 mix_into 逐帧吃光标）—— 天然采样级对齐，
  免疫帧节拍抖动与起播缓冲延迟（waveOut 队列 ~80ms 深度：设备消耗队列
  之前读数为 0，视频保持首帧，正是唇同步的正确起点）；
- **步进三态分叉**（`advance_videos`，每帧每在播视频）：

  | 态 | 条件 | 帧号来源 | 终局判据 |
  |---|---|---|---|
  | 主路径（音频钟） | 有音轨且声部在场 | `floor(已播源样本 / 源采样率 × fps)` | 声部移除 → 停播（音画同终） |
  | 同终 | 有音轨、声部开过后消失（非循环播完被移除） | 钳末帧上屏 | 本帧即停播 + 停声 |
  | 回退（帧差钟） | 无音轨 / 混音器不在场（headless、未开音频） | `floor(elapsed × fps)`（P0 既有行为逐位保留） | 目标帧越界 → 钳末帧 + 停播 |

- 画面先于音轨走完时钳末帧定格、等声部收尾才停播 —— **终点以声部移除为
  主判据**（elapsed 钳制路径退役为回退钟专用）；`video_stop` 依旧双停；
- fps ≤ 0（病态头）：两钟同口径帧号停 0（静帧播放直到 stop）；
- nes-audio 改动 additive：`Mixer::voice_position(key) -> Option<(u64, u32)>`
  只读观测面（活动声部的 `floor(cursor)` + 源采样率；键无活动声部 = None），
  混音数学零变化；确定性边界不变（播放状态仍全在渲染侧表，不进指纹）；
- 机器断言面：nes-audio `t_mix17/18`（采样位口径）、nes-runtime
  `src/video.rs` 模块内 `t_vfa01/02`（纯函数）、`t_vid_sync01`（回退钟
  回归）、`t_vid_sync02`（保持首帧 + 同终）、`t_vid_sync03`（GPU 页帧号
  逐位跟随采样位 —— 注入无设备混音器 + 手动 mix_into，全程确定性）。

## §4 门禁（全部在本 worktree 内执行）

| 项 | 结果 |
|---|---|
| `cargo test --release` ×8 | 全绿 665（基线 658 + S15.1 新增 7）：nes-asset 34 / nes-audio 52 / nes-scene 239 / nes-render-api 45 / nes-render-extract 56 / nes-render-wgpu 123 / nes-media 27 / nes-runtime 89 |
| `cargo clippy --release --all-targets` ×8 | 0 警告 |
| `check_dependency_direction.py` | 13/13（G13 白名单未扩 —— nes-media 零新依赖；nes-audio additive 只读面不触分层） |
| editor_shell 冒烟 | `NES_EDIT_DEMO=1 NES_EDIT_FRAMES=420` 通过：video on / video stopped 取证 + 无 "video:" 错误行；res:// 树 Media/ 行适配（F9 切档 + 双击行修正，两种形态断言兼容） |
| video_demo 冒烟 | `NES_GAME_FRAMES=180` 真实有声跑通：缺失自动从用户目录复制；音频钟主控下帧号增速与 15fps 一致（帧 30→150 页号 4→24 = 真实秒数 × 15fps，页号随已播采样位推进）；音画同终日志行已备（"video ended (audio-clock sync)"，282s 容器在截断冒烟内不可达 —— 终局判据由 `t_vid_sync02` 机器断言） |

新增文件：`nes-runtime/src/video.rs`、`nes-runtime/examples/video_demo.rs`、
`nes-runtime/examples/assets/video_demo.ron`、`nes-runtime/tests/s15_video.rs`、
`nes-scene/tests/s15_video.rs`；`.gitignore` 增 `Media/*.amv|*.avi`（演示视频不入库）。

## §5 遗留（下一轮输入）

1. **loop / 倍速 / 倒放**：播完即停（音频钟下终点 = 声部移除）；
   `video_play` 无循环与速率参数；
2. **压缩视频资源内存策略**：整容器驻留内存 + 音轨全量 PCM 缓存（13.5MB AMV
   实测进程增量约 15-20MB），流式 demux / LRU 换出 / 帧解码缓存上限均未做；
3. **每帧解码开销**：MJPG 帧每帧全量 JPEG 解码（160x128 实测无压力；大分辨率
   视频需要帧缓存或跳帧策略）；
4. **编辑器资源面板集成**：res:// 双击 .amv 目前仅"列出"（fs open 无动作分支）；
   检查器对 Video 资源的专用控件（时长/帧率/缩略图）未做。
