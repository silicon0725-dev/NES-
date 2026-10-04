# NES 2.0 · S15 视频资产面 —— Video 资源 / 逐帧渲染 / 脚本控制 / 音画同播

- worktree：`wt-video`（分支 `s15-video-assets`，基线 HEAD = c02ea7c）
- 上游：S14.3（AMV 解码 + IMA ADPCM 音轨，`nes-media::AmvVideo` / `AviVideo` 已就绪）
- 本期把视频从"解码器"升格为**一等资产**：场景声明 Video 资源 → Sprite 显示当前帧 →
  脚本 `video_play` / `video_stop` 控制 → 有音轨则同步出声。

## §0 结论

视频资产面全链打通并在真实 13.5MB AMV（160x128 @ 15fps + IMA ADPCM 音轨）上端到端验证：

```
场景 Res(kind:"Video") ──▶ bind：nes-media 内容探测解析（AMV/AVI）+ 首帧上 GPU
    ──▶ 脚本 video_play "key" ──▶ Cmd::VideoPlay ──▶ tick 后转渲染侧播放状态机
    ──▶ 每帧 floor(elapsed × fps) 帧 同键覆写 GPU 注册表（Sprite 即播画面）
    ──▶ 音轨即刻开声部（非循环，P0 起点对齐）；video_stop / 播完即停声
```

- 契约测试新增 11 条全绿：nes-scene `tests/s15_video.rs`（6）+ nes-runtime
  `tests/s15_video.rs`（5，真文件 skip-if-missing）；
- 门禁：八 crate `cargo test --release` 657 通过（基线 646 + 新增 11）、
  clippy 0 警告 ×8、依赖方向守卫 13/13、editor_shell 冒烟（NES_EDIT_DEMO=1
  420 帧，含 video on/video stopped 取证）、video_demo 冒烟（NES_GAME_FRAMES=180
  真实有声跑，换页计数随帧正确推进）；
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

## §3 音画同步 P0 口径

- **起点对齐**：`video_play` 时刻 = 计时清零 + 音轨声部开声（同一同步点）；
- 帧号 = `floor(播放经过时间 × fps)`，经过时间由帧循环 delta 累计（渲染侧）；
  fps ≤ 0（病态头）帧号停 0（静帧播放直到 stop）；
- **终点**：目标帧越界 → 钳到末帧上屏 + 停播 + 停声（P0 无 loop；音轨比画面
  长的容器不残留背景声 —— `stop_key` 幂等收口）；`video_stop` 同样双停；
- 采样级严格同步（漂移校正/时钟主从）**不在本期**，归 §5；
- 声部点名停是本期唯一的 nes-audio 改动：`Voice` 记录库键 + `Mixer::stop_key`
  （additive，既有 `stop_all`/voices 语义不动，混音数学零变化）。

## §4 门禁（全部在本 worktree 内执行）

| 项 | 结果 |
|---|---|
| `cargo test --release` ×8 | 全绿 657（基线 646 + 新增 11）：nes-asset 34 / nes-audio 49 / nes-scene 239 / nes-render-api 45 / nes-render-extract 56 / nes-render-wgpu 123 / nes-media 27 / nes-runtime 84 |
| `cargo clippy --release --all-targets` ×8 | 0 警告 |
| `check_dependency_direction.py` | 13/13（G13 白名单未扩 —— nes-media 零新依赖） |
| editor_shell 冒烟 | `NES_EDIT_DEMO=1 NES_EDIT_FRAMES=420` 通过：video on / video stopped 取证 + 无 "video:" 错误行；res:// 树 Media/ 行适配（F9 切档 + 双击行修正，两种形态断言兼容） |
| video_demo 冒烟 | `NES_GAME_FRAMES=180` 真实有声跑通：缺失自动从用户目录复制；换页 1→45 随帧正确推进（30 帧 @60fps = 第 7 帧 @15fps） |

新增文件：`nes-runtime/src/video.rs`、`nes-runtime/examples/video_demo.rs`、
`nes-runtime/examples/assets/video_demo.ron`、`nes-runtime/tests/s15_video.rs`、
`nes-scene/tests/s15_video.rs`；`.gitignore` 增 `Media/*.amv|*.avi`（演示视频不入库）。

## §5 遗留（下一轮输入）

1. **采样级严格音画同步**：P0 只对齐起点，长播漂移不校正（视频帧率与音轨时长
   独立累计）；时钟主从（音轨驱动视频）归后续；
2. **loop / 倍速 / 倒放**：P0 播完即停；`video_play` 无循环与速率参数；
3. **压缩视频资源内存策略**：整容器驻留内存 + 音轨全量 PCM 缓存（13.5MB AMV
   实测进程增量约 15-20MB），流式 demux / LRU 换出 / 帧解码缓存上限均未做；
4. **每帧解码开销**：MJPG 帧每帧全量 JPEG 解码（160x128 实测无压力；大分辨率
   视频需要帧缓存或跳帧策略）；
5. **编辑器资源面板集成**：res:// 双击 .amv 目前仅"列出"（fs open 无动作分支）；
   检查器对 Video 资源的专用控件（时长/帧率/缩略图）未做。
