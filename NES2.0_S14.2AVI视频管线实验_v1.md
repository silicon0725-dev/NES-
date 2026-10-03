# NES 2.0 · S14.2 AVI 视频管线实验 —— "小而完整"资源管线

日期：2026-10-03 · 分支：`s14-2-avi`（独立 worktree `wt-avi`）· 基线：0200749

---

## §0 结论

**交付**：nes-media 新增 [`avi`] 模块 —— **AVI 视频资源管线实验**。用户裁决
（Beta 媒体能力做完整即可，不为"支持视频"提前引入完整媒体框架）落地为一条
"小而完整"的轻量路线：**容器手写 RIFF（wav.rs 同族家法）+ 帧解码组合既有
能力**（未压缩 DIB 手写 BGR->RGBA，MJPG 逐帧走 `image` 白名单库）+ 音轨
PCM 直接产 `nes_audio::Wav` 进混音器。依赖**零新增**（G13 白名单原样覆盖），
引擎面仍是那两个干净 DTO（`DecodedImage` / `nes_audio::Wav`）。

**门禁**：八 crate `cargo test --release` 全绿 **631**（基线 622 + nes-media
新增 9）；clippy 0 警告 × 8；worktree 根守卫 **13/13**（G13 未动，nes-media
白名单原样覆盖新模块）；`cargo run --release --example avi_probe` 演示通过
（合成 32 帧 AVI -> 解析 -> 全帧 BMP 落盘 -> 音轨 WAV 落盘，产出经独立
脚本结构级复核）。已 git commit（未 push）。

**实测形态**：mux -> demux 往返测试逐像素/逐样本对账（DIB 路径无损全等；
MJPG 路径尺寸准确 + 纯色块近似保真）；本机无 ffmpeg、无第三方 AVI 样本
（用户视频均为 MP4 —— 那是 avio/FFmpeg 路线的领地，见 §5），真实第三方
AVI 的现场验证留给 harness（`AVI_IN=<路径>` 即验，见 §3）。

---

## §1 容器与解码面

### 1.1 容器结构（AVI 1.0 单段，本模块覆盖面）

```text
RIFF('AVI ')
 ├─ LIST('hdrl')
 │   ├─ avih                  主头（56B；存在性跳过 —— 权威字段信 strh/strf/索引，
 │   │                        真实文件 avih.dwWidth 与 strf 不一致者存在）
 │   └─ LIST('strl') × N
 │       ├─ strh              流头（56B；fccType 'vids'/'auds'；fps = dwRate/dwScale）
 │       └─ strf              视频 = BITMAPINFOHEADER（40B 起）
 │                            音频 = PCMWAVEFORMAT（16B 起，WAVE_FORMAT 1）
 ├─ LIST('movi')
 │   ├─ '00dc'/'00db'         视频帧块（两位流号 + dc/db）
 │   ├─ '01wb'                音频块
 │   └─ LIST('rec ')          帧分组（扫描兜底路径递归展开）
 └─ idx1                      帧索引（16B/项：ckid + flags + offset + size）
```

RIFF 家法与 `nes-audio/src/wav.rs` 同一条：按"4 字节块名 + 4 字节 LE 长 +
体"走、奇数体长补 pad 字节、**不信任总长字段**（顶层 RIFF 长度不参与遍历，
以实际缓冲为准）、块体声明越界即指名报错不静默吃掉；JUNK/INFO/strd/strn
等未知块按块头跳过。

### 1.2 帧定位（双路径，参照 oxideav-avi 的 `build_idx_table` 裁决）

* **有 idx1**：逐项解析 16 字节条目。offset 基准有两种流派（相对 'movi'
  四字码 / 文件绝对），用**首个非零条目做探针**：两个候选位置上若恰好躺着
  条目声称的 ckid 即认定该基准（都中/都不中时保守取 movi 相对 —— 业界
  多数派）；单条越界的坏条目跳过不中断（坏索引不废掉整个文件）；
* **无 idx1（或索引一帧都没给出）**：按 movi 顺序流式扫块兜底，`'rec '`
  分组递归展开。

两条路径对音轨块（'NNwb'）同样生效；帧数 = **实际定位到的块数**（不信
strh.dwLength）。

### 1.3 解码面（编解码器覆盖与拒绝同样明确）

| 面 | 收 | 拒/断（指名报错） |
|---|---|---|
| 视频帧 | `biCompression = 0` 或 `'DIB '` 的**未压缩 24-bit BGR DIB**（底朝上；负 biHeight 顶朝下变体也认）—— 手写 BGR->RGBA + 行序翻转，alpha 恒 255；`'MJPG'` —— 逐帧 `image::load_from_memory` | 其它四字码（'XVID'/'DIVX'/…）容器认得出、如实报 `VideoCodec::Unsupported(原值)`，`frame()` 给指名错误；调色板 DIB（bpp <= 8）、32-bit DIB、非 24 位 biBitCount 指名拒绝 |
| 音轨 | `WAVE_FORMAT_PCM (1)`，8/16/24-bit —— 组装成 `nes_audio::Wav`（16-bit 面板：8-bit 无符号偏移换算、24-bit 取高 16 位，与 wav.rs 同一口径） | 非 PCM 标签（float/ADPCM/…）**跳过音轨**：视频照常可用、`audio()` 返回 `None` —— 容器级容忍、解码级拒绝，两层失败面分开 |
| 容器 | `RIFF`+`'AVI '`、hdrl/strl/movi/idx1 | 非 RIFF/非 'AVI ' -> `UnsupportedFormat`；块体越界/缺 movi/缺视频流 -> 指名 `Decode` |

### 1.4 API 形态（引擎面）

```rust
let avi = AviVideo::parse(&bytes)?;        // 整份拷进 Arc<[u8]>（ttf.rs 先例）
let info = avi.video_info();               // { width, height, fps, frame_count, codec }
let img  = avi.frame(i)?;                  // DecodedImage（惰性逐帧）
let wav  = avi.audio();                    // Option<nes_audio::Wav>，直接进 Mixer
```

---

## §2 Adapter 通用性验证记录

本期立项的元目的：证明 S14 适配层形状对"视频"这一新维度成立。三条证据：

1. **容器手写、帧解码走库，第三方类型不出 crate**：AVI 容器层 0 依赖
   （手写 RIFF，~250 行）；MJPG 帧解码复用 `image`（G13 白名单既有依赖）
   的同一入口 `decode_image` —— 新格式没有拖进任何新 registry 依赖，
   G13 白名单一条未扩；
2. **解码产物仍是既有 DTO**：帧 -> `DecodedImage`（与图像面/GIF 首帧同构，
   纹理上传路径直接可用）；音轨 -> `nes_audio::Wav`（与 WAV/MP3/FLAC 面
   同构，Mixer 直接消费）—— 上层对"这段媒体来自 AVI"零感知；
3. **写 muxer 是自验证的最短路径**：测试夹具内装配合法 AVI 1.0
   （hdrl/movi/idx1 全量），mux -> demux 往返逐像素/逐样本对账；MJPG
   路径用 image crate 现场编码 JPEG 当帧装配。9 个新用例覆盖：DIB 往返、
   MJPG、无 idx1 扫描兜底、idx1 绝对偏移探针、未收录四字码、非 PCM 音轨
   跳过、24-bit 音轨、拒绝面（垃圾/截断/缺 movi）、帧序号越界。

---

## §3 harness 用法（examples/avi_probe.rs）

```bash
# 用户自己的 AVI 直接验：
AVI_IN=<avi 路径> [AVI_OUT=<输出目录>] cargo run --release -p nes-media --example avi_probe

# 无输入时现场合成演示样例（32 帧 32x24 渐变 DIB + 22050Hz 蜂鸣 PCM）：
cargo run --release -p nes-media --example avi_probe
```

产出：元信息打印（尺寸/fps/帧数/编解码器/音轨参数）+ 全帧写 24-bit BMP
（`frame_000.bmp`…，写在示例内避免污染库公共面）+ 音轨写 WAV（可回灌
`nes_audio::wav::parse` / Mixer）。

本期演示输出（合成样例，`AVI_OUT=avi_probe_out`）：

```text
[avi_probe] demo avi assembled: 96906 bytes
[avi_probe] size  : 32x24
[avi_probe] fps   : 12
[avi_probe] frames: 32
[avi_probe] codec : DIB (uncompressed BGR)
[avi_probe] parse : 0 ms (file 94 KB)
[avi_probe] frames decoded+saved: 32/32 -> avi_probe_out/frame_NNN.bmp (3 ms)
[avi_probe] audio : 22050Hz 1ch 11025 frames (~0.5 s) -> avi_probe_out\audio.wav
```

产出复核（独立 Python 结构校验，不入仓库）：BMP 头（BM/40 字节 DIB 头/
24bpp/尺寸/载荷长度）与抽查像素（对角渐变波形值逐字节相符）、WAV
（22050Hz/单声道/11025 帧/16-bit）全部通过。

---

## §4 门禁（全部在本 worktree 内执行）

| 项 | 结果 |
|---|---|
| nes-media `cargo test --release` | **21 绿**（基线 12 + avi 新增 9；含 real_media 2 项用户实测曲） |
| 八 crate `cargo test --release` | **631 绿 / 0 败**（基线 622 + 9）：asset 34 / audio 40 / media 21 / render-api 45 / render-extract 56 / render-wgpu 123 / runtime 79 / scene 233 |
| `cargo clippy --release --all-targets` | **0 警告 × 8** |
| worktree 根 `python check_dependency_direction.py` | **13/13**（G13 未改：新模块零新增依赖，`image`/`symphonia`/`nes-audio` 白名单原样覆盖） |
| `cargo run --release --example avi_probe` | 合成演示全链路通过（见 §3 输出） |

改动面：`nes-media/src/avi.rs`（新增，demuxer + 模块文档 + muxer 测试夹具
+ 9 用例）、`nes-media/src/lib.rs`（挂模块 + 重导出 + 文档面补视频行）、
`nes-media/examples/avi_probe.rs`（新增）、`nes-media/Cargo.lock`（无新
依赖，仅注释引发的元数据刷新）、本里程碑文档。守卫脚本一行未动。

---

## §5 遗留（下一轮输入）

1. **AMV 第二格式**（下一轮正题）：RIFF 容器变体 + 自有帧编码，作为
   Adapter 通用性试金石；预留位已定 —— nes-media 新模块 + 既有 DTO 复用，
   容器遍历直接回抄本文件；
2. **播放管线**：本期是资源管线（解帧/音轨给上层），逐帧推进/seek/时钟
   同步是后续；`AviVideo` 的惰性逐帧接口（`frame(i)`）已为按需解码留好
   形状；
3. **真实第三方 AVI 现场验证**：本机无 ffmpeg、无 AVI 样本（用户视频均为
   MP4）。harness 已备好 `AVI_IN` 入口，拿到样本即验；idx1 双基准探针
   覆盖了两种真实写手流派，但"业界文件总有惊喜"的验证只能现场做；
4. **MP4/H.264 不进本 crate**：那是 avio/FFmpeg 包装路线（`ruference/avio`
   的 DTO 接口形态是参照）—— 两条路线的汇合点是同一批引擎 DTO，届时
   nes-media 之上或平行一个 avio 适配 crate 再立项；
5. **多流选择**：多视频/多音频流取第一个（'vids'/'auds' 各一），多流
   切换记为需要时的扩展；OpenDML 2.0（AVIX 续段 / indx / ix##）明确
   不做（实验用例皆小文件）。
