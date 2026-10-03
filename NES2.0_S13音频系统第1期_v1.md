# NES 2.0 · S13 音频系统第 1 期（内核 + 全线接入）v1

分支 `s13-audio-core`（独立 git worktree `wt-audio`）。本提交把 **nes-audio 第 1 期**（未提交的工作区成果，waveOut 设备 + WAV 解码 + Mixer）与**第 2 期**（运行时/资产/脚本/编辑器全线接入）集成为一个提交。

## §0 结论

**三个真实游戏宿主能出声了**：资产可以声明 `Sound`、运行时一行 `open_audio` 出声、脚本一句 `play "key"` 播放、play-in-editor 运行态有声。全程零第三方依赖、`unsafe` 仍只在 nes-audio 的 device 模块一处、**音频不进语义指纹**（开/不开音频同轨迹 trace_hash 逐位相同，T-Aud-04 实证）。

交付面：

| 层 | 交付 |
|---|---|
| nes-audio（第 1 期） | `wav`（16-bit PCM 手写解析）+ `mixer`（纯数学混音）+ `device`（winmm waveOut + 专属填充线程）+ `write_wav`（第 2 期补的编码逆操作） |
| nes-asset | 零改动 —— `AssetKind::Audio` 变体第 1 期前已存在（见 §2.1 裁决） |
| nes-scene | `Op::Play`（文本语法 `play "key"`）+ `Cmd::PlaySound` + 树侧落地缓冲（不解释音频） |
| nes-runtime | `open_audio`/`audio`/`declare_sound`/`register_pending_sounds` + 装载链注册 + tick 后 Cmd 转交 |
| 编辑器/示例 | editor_shell PLAY 自动开音频（Output 记 `audio on`）+ `audio_demo` 示例 + `audio_demo.ron` + 代码生成的 440Hz `beep.wav` |
| 守卫 | `check_dependency_direction.py`：G11 白名单 +nes-audio、G12 反向禁令移出 runtime（正向 `runtime ──▶ nes-audio` 方向唯一） |

## §1 nes-audio（第 1 期内核，本提交一并入库）

### 1.1 三件套

```text
WAV 字节 ──▶ [wav] 解码 ──▶ [mixer] 混音 ──▶ [device] waveOut 出声
```

- **`wav`**：手写 RIFF 块遍历 + 16-bit PCM（`fmt ` 块 18 字节标准形含 cbSize=0、`data` 块小端交错帧、1/2 声道）；拒绝面明确（非 PCM/非 16 位/非 1-2 声道/截断/缺块各自指名报错）；未知块按块头长度跳过、奇数长度补 pad。`#![forbid(unsafe_code)]`。
- **`mixer`**：声音库（`register` 键 -> `Arc<Wav>` 共享）+ 声部列表 + 线性插值重采样（f32 光标 × 采样率比步进）+ 软削顶（f32 累加 clamp 回 i16，两满幅相加饱和不回绕）；`mix_into` 纯确定性（不依赖时钟/线程/随机数）。`#![forbid(unsafe_code)]`。
- **`device`**：winmm `waveOut` + `CALLBACK_NULL` 轮询；**全部 winmm 调用单线程化**——专属填充线程每 ~10ms 对 4 个 `WAVEHDR` 轮转（跳过未播完的 → `mix_into` 回填 → `waveOutWrite`），关闭序列（`waveOutReset → UnprepareHeader × 4 → waveOutClose`）在同一线程收尾段串行执行，从根上消除"句柄在 write 中途被另一线程 close"竞态。手写 `#[repr(C)]` + 编译期尺寸自检；`unsafe` 全部收敛在本模块、逐调用点 SAFETY 注释。进程级单设备（`AtomicBool` 占位，二次 open 报 `AlreadyOpen`）。

### 1.2 失败纪律（照 GPU 用例）

无设备（`waveOutGetNumDevs() == 0`）与打开失败一律返回 `AudioError`（`NoDevice` / `OpenFailed{code}` / `AlreadyOpen`），不 panic、不静默换路；测试在无设备环境如实打印跳过，有设备而打开失败直接判失败。

### 1.3 第 2 期补充：`write_wav`

`parse` 的逆操作（编码是解码的镜像：`fmt ` 块如实写 18 字节体长——写 16 会让跳块逻辑把 cbSize 当下一块头，本轮实测修正），供测试往返（`t_wav18` 四形态逐位保真）与演示资产生成（audio_demo/editor_shell 的 440Hz 蜂鸣，代码生成、缺了再写，仓库只背一份 13KB 小文件——与 bmp 资产同一惯例）。

### 1.4 双重填充确认（任务清单第 2.3 项）

混入**只**发生在设备线程的 `mix_into`（~10ms 一缓冲）。runtime 帧循环不做每帧混音：`simulate`/`tick_headless` 在 tick 后只做 `take_played_sounds` → `Mixer::play`（把声音键变成声部），填充仍由设备线程独占。全链一处填充，无双重填充。

## §2 资产 / 运行时 / 脚本通道（第 2 期）

### 2.1 AssetKind 裁决：不新增枚举成员

任务清单写的是"AssetKind::Sound（additive 枚举成员）"。查证：`nes-asset::AssetKind` **早已有 `Audio` 变体**（第 7 成员、`as_str() = "Audio"`、`AssetKind::ALL` 含它、key.rs 测试覆盖），且 `nes-scene::resources::asset_kind_of_hint` 的别名表已把 `"sound" | "sounds" | "sfx" | "wav" | "ogg"…` 映射到 `AssetKind::Audio`——场景 RON 写 `kind: "Sound"` 本就可解析（宽容匹配先精确后别名）。再添一个 `Sound` 变体会造成两个同义类别的身份分裂（同路径双键），违背"分类是身份的一部分"的既有纪律。故**采用既有 `Audio` 变体，零 nes-asset 改动**；资源表条目（`ResEntry`）天然能装 Sound 键，穷尽 match 逐处核查无缺臂（`is_render_facing` 只认 Texture，Audio 如实无渲染视图）。

### 2.2 运行时装配（nes-runtime）

- **`open_audio(&mut self) -> Result<(), BackendError>`**：构造 `Mixer`（`Arc<Mutex<…>>`，与设备线程共享同一份）+ `AudioDevice::open(48000, 2)`。**幂等**（已开返回 Ok）；无设备/打开失败如实报 `Err`（不 panic）；headless 也能开（同一运行时，S7.3 口径）但通常不开。开完顺手把已就绪的 Sound 资源补注册（晚开音频场景）。单条解码失败不阻塞开音频。
- **`audio(&mut self) -> Option<&mut Mixer>`**：设备未开时 `Some`（`Arc::get_mut` 独占 + 安全的 `Mutex::get_mut`，不留毒化窗口）；设备已开时**如实 `None`**——填充线程持有 Arc 克隆且每 tick 锁一次，Rust 无法证明无别名，不假装能给出 `&mut`。已开设备的宿主走装载链（`bind_assets`）注册声音。
- **`declare_sound` / `register_pending_sounds`**：与纹理的 `declare_texture` / `upload_pending_textures` 同构——槽位清单（`sound_slots`，声明序确定）+ 版本账目（`registered_version`，热重载重注册判定、失败不记账）。返回 `(注册数, 按槽位归因的失败清单)`。
- **装载链**：`bind_assets` 在 `table.bind` 之后（仅当混音器已构造——**不开音频零动作、逐位同基线**）注册就绪且版本变化的 Sound 资源：读注册表字节 → `wav::parse` → `Mixer::register(键, Arc<Wav>)`。**键约定 = 资源路径去扩展名**（`Audio/beep.wav` → `Audio/beep`；`sound_key_of` 单一推导点，注册与 Cmd 转交两侧共用）；解码失败按槽位进 `BindReport.failed` 缺口清单（编辑器红条、不阻塞场景——与装载失败同家法）。
- **`registered_sound_count`** 诊断自由函数（与 `uploaded_texture_count` 同构）。
- `instantiate_scene` 随纹理槽位一并重建 `sound_slots` 并清版本账目。

### 2.3 脚本通道裁决：`Cmd::PlaySound` 走既有 Cmd 流

- **树不认识音频**（不解码、不混音、不碰设备）；`SceneTree` 不新增"音频副作用队列"这种平行通道——`play` 的意图以 **`Cmd::PlaySound { key }`** 进入**既有 Cmd 流**（`NodeCtx`/`SignalCtx` 各加一个 `play_sound` 入口，process 与信号两入口同权，照 `emit` 口径；play 不写树，"process 只写自身"纪律不涉及）。
- **消费点查证**：Cmd 的落地在 `SceneTree::tick` 内部的 `apply_cmd`（runtime 不经手）——因此树侧为这条它无法解释的命令提供一个**落地缓冲**（`played_sounds: Vec<String>`，发射序、可重复；与 `pending`/`signal_queue` 同一家法的单帧副作用缓冲），公开 `take_played_sounds()` 取走即清。runtime 在 **`simulate` 的每个 tick 后**（以及 `tick_headless`）取走转交混音器：开音频 → `Mixer::play(key, 1.0, false)`；未开音频 → 取走即弃；未注册键 → 静默丢弃（音频不是语义状态，坏键不崩帧，与 `SetProp` 静默口径同家法）。
- **缓冲不跨帧积压**：两个 tick 入口都即取即清，headless 长跑无泄漏。
- **文本语法**：`play "boom"`（语句级，照 `emit` 的解析样式——关键字 + 字符串字面量、无载荷表达式；`Op::Play { key }` 零栈交互）。`play` 入保留字（16 个），`play = 1` 报"保留字"、`play 42` 报"期望字符串字面量"。**循环播（`play_loop` 一类）P0 不做**，归 §5 遗留。
- **契约测试**（`nes-scene/tests/s6_script_text.rs`）：T-Cmp-33 编译产物逐指令相等 + process/信号两入口的 Cmd 流落地面断言 + 取走幂等；T-Cmp-34 播放请求不是树状态——消费与否不影响 `scene_fingerprint`（uid 钉成确定性派生后两遍逐位同）。

### 2.4 确定性确认（任务清单第 4 项）

- **指纹采样面查证**：`scene_fingerprint` 只采样 帧号/暂停位/时间缩放/前序节点（uid、名字、类型、父引用、生命周期位、ProcessMode、本地变换逐字段位形、属性表、脚本局部）；`state_fingerprint` 再混输入按住态。**音频面全部不在其中**：混音器库/声部在 runtime 字段（不属树）、`played_sounds` 缓冲不在采样面、`Mixer` 状态纯音频。
- **实证**：`tests/s13_audio.rs` T-Aud-04——同轨迹先不开音频跑两遍、再开音频跑一遍，三份 `trace_hash` 逐位相同。headless 消费语义 = 取走即弃，`Cmd` 的落地记录（键名缓冲）不进任何指纹口径——**与现状同口径的说明**：既有 Cmd（SetProp 等）落地进树状态故进指纹；PlaySound 的落地物不是树状态，刻意不进指纹（音频是表现不是语义）。
- **additive 纪律**：不开音频的路径（headless 全部既有测试、farm/dungeon/editor.ron 场景与 trace）逐位同基线——561 基线测试零改动零失败实证（ nes-scene 仅新增 2 个用例、其余不动）。

## §3 编辑器 / 游戏接线

### 3.1 editor_shell

- 启动即声明演示声音（`declare_sound("Audio/beep.wav")`）并随 bind 装载（`report.loaded.len()` 断言 5 → **6**）；`Audio/beep.wav` 代码生成（440Hz/250ms，缺了再写）。
- **PLAY 会话自动开音频**：`PlaySession::start` 查资源表含 Audio 条目则 `open_audio`——成功 Output 记 **`audio on`**，失败记 `audio: {错误}` 一行**不中断**（带病也能跑的既有口径）；幂等（运行中重启不重复开）。**STOP 不关音频**（混音器与设备跨会话存活，空混音器静音填充，幂等无害）。
- FileSystem dock 白名单加 `wav`（声音资产与纹理/脚本同为项目资产，res:// 树如实列出）；NES_EDIT_DEMO 的 fs 双击坐标随新行序修正（Audio/ 目录插入使 spin.nes 下移两行，y=256→292，注释同步）。
- **demo 钩子**：PLAY 后断言 Output 含 `audio on`（无设备环境如实退化为 `audio:` 错误行断言——"没有设备"与"接线断了"不许互装）。实测本机：`[demo] … 冒烟断言通过`，日志含 `audio on`。
- 附带发现并修正：editor_shell 从不加载 editor.ron（它是 criterion_headless 测试的场景）——editor.ron **零改动**。

### 3.2 audio_demo（新示例，照 first_game 形态）

- `examples/assets/audio_demo.ron`：`Res(id:1, path:"Audio/beep.wav", kind:"Sound")` + 脚本节点（`init { play "Audio/beep" }` 装载即播一声 + `on "input/key_down"` 空格再播）。
- `examples/audio_demo.rs`：先 `open_audio` 再 `load_scene`（装载链 bind 即注册；两侧同键约定，顺序无关），打印 `[audio] on` / `[audio] registered 1 sound(s)`；无设备如实报行、演示照常。
- 冒烟（`NES_GAME_FRAMES=180`）：`audio on` + `registered 1 sound(s)` + 干净退出。

### 3.3 不添乱核对

farm.ron / dungeon.ron / tower_defense.ron / 各 trace 文件**零改动**（`git status` 实证）；editor.ron 零改动；nes-asset / nes-render-* 零改动。

## §4 门禁（全绿）

| 项 | 结果 |
|---|---|
| 七 crate `cargo test --release` | **606 通过 / 0 失败**（基线 561 + nes-audio 40〔38 + write_wav 往返 2〕+ nes-scene 2〔T-Cmp-33/34〕+ nes-runtime 3〔T-Aud-01/02/05〕）。分 crate：nes-asset 34、nes-render-api 45、nes-render-extract 56、nes-render-wgpu 123、nes-scene 233、nes-audio 40、nes-runtime 75 |
| `cargo clippy --release --all-targets` | **0 警告 × 7 crate** |
| worktree 根守卫 `check_dependency_direction.py` | **12/12 通过**（G11 白名单 +nes-audio、G12 反向禁令移出 runtime 后复跑） |
| editor_shell 冒烟 | 干净退出；`NES_EDIT_DEMO=1 NES_GAME_FRAMES=260` 全部断言过（含 `audio on`、fs 双击、RESET 还原、IME 草稿） |
| audio_demo 冒烟 | `NES_GAME_FRAMES=180`：`audio on` + `registered 1 sound(s)` + 干净退出 |
| 确定性 | T-Aud-04：开/不开音频同轨迹 trace_hash 逐位相同；T-Cmp-34：消费与否不影响 scene_fingerprint |

## §5 遗留（归后续里程碑）

1. **音量/声道脚本面**：`play` 固定音量 1.0；`play "key" 0.5`（声部音量）、总音量、声像（pan）未有语法——`Mixer` 侧系数已就绪（`play(key, volume, looped)`、`set_master_volume`），只差脚本面与通道载荷设计。
2. **循环播**：`Mixer::play` 的 `looped` 参数已就绪但脚本面 P0 未接（BGM 场景需要）；需一并设计"同一键已循环播时再 play 的幂等语义"。
3. **预载策略**：Sound 资源在 bind 时全量解码驻留内存；大音频（音乐轨）需要流式/懒解码策略与内存账目（WAV 无压缩，代价线性）。
4. **B7 扩展音频联动**：扩展清单（`NES2.0_扩展生态扫描与三块缺口清单_v1.md`）中"脚本可感知播放结束/声部事件"的联动——需先裁决音频事件是否进信号总线（表现事件 vs 语义事件的边界，本期裁决先例：不进）。
5. **audio() 访问器的开放面**：设备开着时返回 `None`（§2.2）——若宿主需要"开着设备也直接调音量/停声部"，需补 `with_audio(f)` 锁式访问器（Mutex 锁就是正确的同步原语）。
6. **热重载声部接管**：重注册同键覆盖库条目，正在播的旧 `Arc` 声部继续播完——换版即听头（可接受）；若要"重载即换声"需声部级重定向。
