# NES 2.0 · S14 媒体解码适配层第 1 期 —— 依赖分层政策正式生效

日期：2026-10-03 · 分支：`s14-media-codecs`（独立 worktree `wt-media`）· 基线：717f6bf

---

## §0 结论

**交付**：新增第八个 crate **nes-media**（编解码适配层）—— 全仓库**唯一允许第三方
依赖的 crate**。用户裁决的依赖分层政策落进守卫（新 **G13**，守卫总数 12 -> **13**）；
运行时图片/扩展音频解码经适配层接入（`declare_image` + Sound 装载回落序）；编辑器
壳层 res:// 白名单扩外部交付格式、**用户实测两首曲子接入数字键 0 三态音乐循环**。

**门禁**：八 crate `cargo test --release` 全绿 **622**（基线 606 + nes-media 新增 12 +
nes-runtime 新增 4）；clippy 0 警告 × 8；worktree 根守卫 **13/13**；editor_shell 冒烟 +
`NES_EDIT_DEMO=1` 全断言（含新音乐取证）通过。已 git commit（未 push）。

**实测**（用户音乐，`C:/Users/Administrator/Music/text/`，不在仓库）：

| 曲目 | 格式 | 实测解码产物 | 文件 | PCM 峰值 | 解码耗时 |
|---|---|---|---|---|---|
| 心似烟火.flac | FLAC | **96000 Hz** stereo · 174 sec | 55.8 MB | 64 MB | **335 ms** |
| Montagem Nada.mp3 | MP3 | 44100 Hz stereo · 122 sec | 2.0 MB | 20 MB | **134 ms** |

（release 构建、全量解码进内存；与立项预估 40-80MB/首一致 —— FLAC 是 96kHz 高码率
源，PCM 略超预估但 P0 可接受。播放验证：demo 注入数字键 0 三次，两曲先后经 waveOut
真实出声（Output 状态行只在 `play_host_sound` 返回 Ok 时落账）；另有确定性无设备验证
—— 曲中切片过 Mixer 混出非零样本，见 nes-media/nes-runtime 契约测试。）

---

## §1 依赖分层政策

### 1.1 用户裁决（立项纪律变更，全文）

> **编解码器采用成熟 Rust 库，引擎核心保持零依赖。**

由此生效的分层政策：

- **引擎核心 crate**（nes-asset / nes-scene / nes-render-api / nes-render-extract /
  nes-render-wgpu / nes-audio）：维持**零第三方依赖**，既有 G3/G10/G11/G12 等守卫
  逐条保持、一条不松；
- **新增 nes-media crate（编解码适配层）**：唯一允许白名单第三方 —— `image` 系
  （image 及其全部传递依赖）+ `symphonia` 系（symphonia 及其全部传递依赖）+ 仓库内
  `nes-audio`（path）；其它任何 registry 依赖越界；
- nes-media 对引擎的面是**干净 DTO**：解码产物映射到既有类型（音频 ->
  `nes_audio::Wav` 同构；图像 -> RGBA8），上层不感知第三方；
- **视频（AMV 解码）是下一轮**：适配层已留位（新格式 = nes-media 新模块 + G13
  白名单扩条，引擎核心不动），**Beta 不做完整视频管线**。

### 1.2 选型记录

**图像：image 0.25 系（image-rs）**。理由：
1. 格式覆盖面是生态最全的（PNG/JPEG/GIF/WebP/BMP/TIFF/…默认 feature 全开），
   且按内容（文件头魔数）探测格式 —— 与资产管线"扩展名会说谎，字节不会"的口径一致；
2. 纯 Rust、无传递 C 依赖；`image::load_from_memory -> to_rgba8` 两步即得引擎
   纹理面的入参形状（RGBA8 行主序），适配层 50 行收口；
3. **zune-image 对比**：zune 系（zune-jpeg/zune-png/zune-inflate）单格式解码器
   更快，但容器/格式覆盖薄（无 GIF/WebP 编码面、无统一 probe），做"任意交付物
   解码"要自己拼多解码器分派 —— 适配层反而变厚。**结论：选 image 0.25**；
   0.25 内部 JPEG 解码已用 zune-jpeg（传递依赖里有 `zune-jpeg`，G13 白名单闭包
   可见）—— 性能关键路径等价覆盖。zune-image 独立对比评估归 §5 遗留。

**音频：symphonia 0.5 系**。理由：
1. 纯 Rust 全家桶（容器 probe + 解码器 + 元数据），一个 facade 按 feature 收口；
2. features：`["mp3", "flac", "ogg", "vorbis", "pcm", "wav", "isomp4", "aac"]`
   （实际全部启用；facade 传递带入 `symphonia-format-mkv` / `-riff` /
   `symphonia-codec-adpcm` 等，全部落在 G13 家族闭包内 —— 白名单按"根的传递
   闭包"计，第三方补丁版本换内部依赖时守卫不跟着改）；
3. 解码口径：逐包到 f32 中间面（`SampleBuffer<f32>`）-> **f32 -> i16 clamp 缩放**
   （基准 32768 —— symphonia f32 面满幅单位是 s/32768，2 的幂次缩放在 IEEE754 上
   精确可逆：16-bit 源经 symphonia 往返**逐位保真**，有单测钉住）；
4. 采样率/声道**如实保留**进 `Wav`（Mixer 自带线性重采样与 1/2 声道换算；
   实测 FLAC 是 96000Hz —— 混音器步进重采样直接吃下，无需适配层做率转换）；
5. 单个坏包跳过（可听性优先），容器级失败整体报 `MediaError`。

**依赖事实**（G13 输出）：nes-media 传递依赖 138 包，其中白名单家族闭包 137 包 +
仓库内 nes-audio（path）。版本纪律 caret（`image = "0.25"`、`symphonia = "0.5"`），
不锁死 minor。

### 1.3 守卫落地（check_dependency_direction.py，仓库根唯一被改文件）

- **G13（新）**：nes-media 第三方依赖白名单 = image 系传递闭包 ∪ symphonia 系
  传递闭包；仓库内直接依赖只许 `nes-audio`（path）；除 nes-runtime 正向接入外
  任何 crate 不得依赖 nes-media。实现上新增 `reachable_names_from()`：从白名单根
  （`image` / `symphonia`）在 nes-media 的 resolve 图里 BFS 圈家族闭包 —— 跟踪
  家族而不是枚举包名；
- **G11**：runtime 直接依赖白名单追加 `nes-media`（path），文案"六个项目 crate"
  改"**七个**项目 crate"；
- G1-G12 逐条保持，输出 **13/13 通过**。

---

## §2 nes-media API（引擎面 = 干净 DTO）

```text
nes-media/
  Cargo.toml    edition 2021；空 [workspace] 表钉独立根（G13 家族成员纪律同 G10/G12）
  src/lib.rs    分层政策模块文档 + MediaError + re-export
  src/image.rs  decode_image
  src/audio.rs  decode_audio
  tests/real_media.rs   真实文件契约（skip-if-missing）
```

| API | 签名 | 说明 |
|---|---|---|
| `decode_image` | `(&[u8]) -> Result<DecodedImage, MediaError>` | 按内容探测格式；`load_from_memory -> to_rgba8`；GIF 取首帧 |
| `DecodedImage` | `{ width: u32, height: u32, rgba: Vec<u8>, frame_count: Option<u32> }` | RGBA8 直通 `register_texture` 入参；动图帧数如实报告 |
| `decode_audio` | `(&[u8]) -> Result<nes_audio::Wav, MediaError>` | symphonia probe + 全轨解码；i16 交错；任意源率/声道保留 |
| `MediaError` | `UnsupportedFormat / Decode(String) / Io(String)` | Display 全中文指名道姓 |

`#![forbid(unsafe_code)]`（我们的代码面零 unsafe；第三方库内部不归本仓库管辖）。
测试 12 条：合成 PNG/JPEG/GIF 现场编码往返（image 库同库编码 —— 仓库不提交二进制
资产的家法）、垃圾字节/截断拒绝面、WAV 经 symphonia 逐位保真、Mixer 形状兼容；
真实文件 2 条（FLAC/MP3 skip-if-missing，打印与断言全 ASCII）。

---

## §3 运行时 / 编辑器接入

### 3.1 nes-runtime（Cargo.toml +nes-media path；G11 白名单同步）

- **`declare_image(&mut self, rel) -> Result<ResId, BackendError>`**：kind =
  `AssetKind::Texture`（查证结论：`AssetKind` 无 Image 变体，图片与 BMP 是同一
  渲染面的两种交付格式，`is_render_facing()` 只认 Texture —— 用 Texture 同路），
  槽位进同一上传队列；
- **`upload_pending_textures` 解码分发**：**BMP 手写快路径优先 -> nes-media 回落**
  （既有 BMP 资产零行为变化；两路全失败指名路径报两路原因）。缺口清单口径说明：
  纹理面的既有契约是"上传时解码失败如实 `Err`、指名路径不静默"（冻结契约，不变）；
  装载失败照旧进 bind 缺口清单 —— "解码失败进缺口"先例的完整语义落在音频面
  （下条），图片面保持既有更强契约；
- **`register_pending_sounds` 解析序 = 先原生 WAV 后 nes-media**。顺序理由：**零
  解码开销快路径优先** —— WAV 解析只是几个块头读取，绝大多数声音资产就是
  16-bit PCM WAV，不该为它们付一次 symphonia probe + 全量解码；失手（MP3/FLAC/
  OGG/M4A，乃至 8-bit/float WAV 等原生解析器拒收的变体）才回落适配层，失败清单
  指名两路原因（编辑器红条不崩帧）；
- **宿主直注三件套**（编辑器音乐预览用）：`register_host_sound(key, Arc<Wav>)`
  （未开音频也登记 —— 混音器 Arc 惰性构造）、`play_host_sound(key, vol, looped)`
  （未开音频先开、无设备如实 Err）、`stop_host_sounds()`。背景：`audio()` 访问器
  在设备打开后如实返回 `None`（填充线程持 Arc 克隆），宿主预览需要绕开该限制的
  正门。

### 3.2 editor_shell（示例）

- **res:// 白名单** 7 -> 15 项：`+jpg/jpeg/webp/gif/flac/mp3/ogg/m4a`。白名单只是
  **列出** —— 场景没声明它们就只是树里一行，不产生解码成本；
- **用户实测资产接入**：装配时若 `C:/Users/Administrator/Music/text/` 存在实测曲，
  经 nes-media 解码宿主直注混音器（键 `music` = 心似烟火.flac、`music2` =
  Montagem Nada.mp3）；Output 记 `music loaded (flac, 96000Hz stereo, 174 sec,
  57 MB, 335 ms)` 一行/曲（ASCII 描述 —— 中文文件名是用户数据，不进日志/断言）。
  文件不在则整段跳过（CI/他机安全）；解码失败记一行不阻塞（带病也能跑）；
- **数字键 0 三态循环**：停 -> 心似烟火(FLAC) -> Montagem(MP3) -> 停…（按实际装载
  成功的曲目数轮换；切态先 `stop_all` 曲间不打架；looped 循环 —— Output 状态行与
  实际出声一致）。编辑态专属（运行态混音器归游戏脚本）；改名框持焦让位输入
  （焦点门与挂载流同门）。播放失败如实记行回停态。**整曲 PCM 进内存（实测 20-64MB
  /首）—— 流式是后续**（§5）；
- **EDITOR_LOG_KEEP 20 -> 26**：音乐装载 2 行 + 三态 3 行入 Output，既有断言行的
  窗口余量照旧；
- **NES_EDIT_DEMO 钩子**：音乐在场时断言 Output 有 `music loaded` 行 + 三态循环
  取证（`music: flac (looped)` / `music: mp3 (looped)` / `music: stopped`）。注入序：
  Esc 回滚改名草稿（失焦 = 提交是 UiVm 契约，不回滚会把 obj1 改名成 "obj1中"、
  踩空树形态断言 —— demo 联调实录）-> 点 Output dock 失焦 -> 三次 Num0。

---

## §4 门禁（全部在本 worktree 内执行）

| 门 | 结果 |
|---|---|
| `cargo test --release` × 8 crate | **622 全绿**（nes-asset 34 / nes-scene 233 / nes-render-api 45 / nes-render-extract 56 / nes-render-wgpu 123 / nes-audio 40 / nes-runtime 79 / nes-media 12；基线 606 + 16 新增） |
| `cargo clippy --release --all-targets` × 8 | **0 警告 × 8**（nes-media 我们代码面 0 —— 第三方库内部不归本仓库管辖） |
| 守卫 `check_dependency_direction.py` | **13/13 通过**（新 G13；G11 文案七 crate） |
| editor_shell 冒烟（NES_EDIT_FRAMES=120） | 干净退出 |
| `NES_EDIT_DEMO=1`（NES_EDIT_FRAMES=300） | 全断言通过（挂载/卸载/enabled/折叠/刷新/play/stop/reset + S14 音乐四断言） |
| 真实媒体实测 | 见 §0 表（335ms / 134ms 解码；waveOut 真实出声 + Mixer 确定性混音双验证） |

---

## §5 遗留（下一轮输入）

1. **AMV 视频解码**（下一轮）：适配层留位 —— 新模块 + G13 白名单扩条；Beta 不做
   完整视频管线（帧纹理上传路径与音频面不同，需另行裁决）；
2. **流式音频**：`decode_audio` 是全量进内存（96kHz FLAC 64MB/首）；symphonia 的
   `MediaSource` 抽象支持拉流 —— 做成"按需段解码 + 环形缓冲"归后续里程碑（编辑器
   预览 P0 可接受全量）；
3. **GIF 动图帧**：现只取首帧 + `frame_count` 如实报告；帧序列解码/帧动画播放
   （`Delay` 已在解码器面可得）归后续；
4. **zune-image 对比评估**：§1.2 结论选 image 0.25（覆盖面 + 统一 probe）；zune 系
   单解码器性能优势若将来成为热点（批量纹理导入）再立项评估 —— 白名单闭包已含
   zune-jpeg，替换成本被适配层隔离在 nes-media 一个 crate 内；
5. **>2 声道源**：`decode_audio` 如实保留（5.1 FLAC 不降混），Mixer 现只说 1/2
   声道 —— 此类源登记后暂不出声；降混策略归 Mixer 侧里程碑；
6. README 的 crate 地图/计数未随本里程碑更新（守卫脚本与 crate 清单为准；
   README 属仓库根文件，按本次纪律不动）。
