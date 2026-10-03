# NES 2.0 · S14.3 AMV 解码移植 —— Adapter 通用性试金石 #2

日期：2026-10-03 · 分支：`s14-3-amv`（独立 worktree `wt-amv`）· 基线：2d88f66

---

## §0 结论

**交付**：nes-media 新增 [`amv`] 模块 —— 用户真实交付物（蜘蛛糸モノポリー
OP，14179882 字节）从 S14.2 的"指名拒绝"变为**真解码出帧**。容器侧是
RIFF 坏头变体的手写 demux（四条与标准 RIFF 相反的怪癖，见 §1）；帧侧按
**FFmpeg 自家 AMV 解码器的原方案**（`libavcodec/sp5xdec.c` 的
`ff_sp5x_process_packet` + `sp5x.h` 固定表）把无头帧体重包成标准 JPEG
交 `image` 白名单库解码。依赖**零新增**（G13 白名单原样覆盖 —— 合成表
是常量字节，不是依赖），引擎面仍是那两个干净 DTO（`DecodedImage` /
`nes_audio::Wav`）。

**门禁**：八 crate `cargo test --release` 全绿 **636**（基线 631 +
nes-media 新增 5）；clippy 0 警告 × 8；worktree 根守卫 **13/13**（G13
未动）；`cargo run --release --example avi_probe` 对真实 AMV 出 17 张
BMP（6 ms 解析 + 5 ms 解 17 帧）。已 git commit（未 push）。

**实测解码质量**：160x128 @ 15fps、4233 帧；31 帧全片取样 **0 解码失败**；
帧 141 / 1269 肉眼确认为彩色动画画面（人物立绘 + 片名文字方向正立，
单帧最多 ~11000 独立色）；首帧/尾帧为黑场淡入淡出（首帧熵流手工解码
证实：80 个 MCU 全部 DC=-78、零个非零 AC —— 解码产物逐像素 (1,1,1)，
非解码缺陷）。详见 §3。

---

## §1 容器与解码面

### 1.1 容器实测形态（确定性事实，全部来自真实文件）

探针期（`amv_hdrl_probe.bin` = 文件头 300 字节转储）+ 全文件扫块钉死：

```text
RIFF [size=0(坏)] 'AMV '
 ├─ LIST [size=0(坏)] 'hdrl'
 │   ├─ amvh 56B        +0 dwMicroSecPerFrame = 66667（-> 15fps）
 │   │                  +32 dwWidth = 160   +36 dwHeight = 128
 │   │                  （amvh 顶替 avih，布局同 avih；全文件唯一尺寸/帧率来源）
 │   ├─ LIST [size=0(坏)] 'strl' 流 0 = 视频
 │   │   ├─ strh 56B 全零（fccType/fps 全缺席）
 │   │   └─ strf 36B 全零 —— BITMAPINFOHEADER 声称 36 字节且一个非零字节都没有
 │   └─ LIST [size=0(坏)] 'strl' 流 1 = 音频
 │       ├─ strh 48B 全零
 │       └─ strf 20B WAVEFORMATEX：声明 PCM/1ch/22050Hz/16-bit（撒谎，见 1.3）
 ├─ LIST [size=0(坏)] 'movi'（四字码在文件 0x138，子块区走到文件尾）
 │   ├─ '00dc' 视频帧块（首帧 326B，中段数 KB；1:1 交替…）
 │   ├─ '01wb' 音频块（恒 743B，奇数）
 │   └─ 文件尾 8 字节字面量 'AMV_END_'（块遍历终止符）
 └─ （无 idx1）
```

四条铁律（每条都和标准 RIFF 相反，`avi.rs` 家法不能照搬）：

1. **声明长度不可信**：RIFF/hdrl/strl/movi 的 LIST 尺寸字段全 0 ——
   hdrl 走"结构走查"（块名 + 块自身声明长，LIST 一律下潜），movi 区间
   直接取"四字码位置到文件尾"（另有字面扫描兜底双路径）；
2. **无 pad**：块体奇数长后紧跟下一块头（实测 '01wb' 743B 奇数块后直接
   '00dc'）—— 标准 RIFF 的 +1 对齐规则不适用；
3. **表数据不在 strf**：视频 strf 是 36 字节全零。S14.2 探针期"量化表/
   哈夫曼表在 strf 附加数据（AVI JPEG extradata 经典形态）"的假设被
   真实文件**证伪** —— AMV 的表是**解码器侧常量**（见 1.2），
   `parse_avid` 那条 extradata 路线与 AMV 无关（那是普通 AVI 里的 MJPG）；
4. **流号即流序**：strh 全零无法识别流类型，按 strl 序数定（1 视频
   2 音频）—— 与 FFmpeg `avidec.c` 对 AMV 的强制同一条裁决。

### 1.2 帧解码路线（FFmpeg 实证方案的逐字节复刻）

帧体 = `FFD8` + 裸熵数据 + `FFD9`，中间没有任何 JPEG 标记段。FFmpeg 的
AMV 解码器（`AV_CODEC_ID_AMV` 走 `sp5xdec.c`）把每帧**重包成标准
JPEG**再走普通 MJPEG 解码；本模块同方案：

```text
SOI + DQT(固定两表) + DHT(标准 Annex K 四表) + SOF0(160x128, 4:2:0)
     + SOS + 帧体[2..len-2]（剥壳原样嵌入） + EOI   ->  image::decode_image
```

* **DQT**：SP5X/AMV 固定量化表（FFmpeg `sp5x.h` 的
  `sp5x_qscale_five_quant_table`，亮度表非 Annex K 标准值、色度高频段
  全 79）—— 常量嵌入 `amv.rs`，零依赖；
* **DHT**：ITU T.81 Annex K 标准四表（与 FFmpeg
  `init_default_huffman_tables` 同源公开规范值）；
* **SOF0**：宽高来自 amvh（FFmpeg 原样写 `avctx->coded_width/height`）。
  探针期"AMV 的 SOF 高度可能是显示高度两倍（半高场编码）"的传闻**证伪**
  —— FFmpeg 代码路径无此逻辑，本文件 31 帧取样解出即正立整帧，无
  场交错痕迹；**垂直翻转**才是真怪癖（FFmpeg 对 AMV 置 `s->flipped=1`：
  帧体栅序第 0 行是显示最底行），解出后翻转，方向由夹具测试钉死；
* **熵数据**：实测全程规范 stuffing（全文件 36069 处 `FF00`、0 处裸
  FF），剥壳后原样嵌入即合法 JPEG 扫描；
* **走库而非手写**：合成出的流是教科书级 baseline JPEG（4:2:0、标准
  DHT），`image` 白名单库（内部 zune-jpeg）直接覆盖 —— **任务书里
  "路线 2：手写哈夫曼/DQT/IDCT/上采样"无需启动**，这是对工程量的重大
  节省，也是"帧解码走库"组合在第二种容器上的再次成立。

解码产物垂直翻转后即 `DecodedImage`（RGBA8），与 S14.2 完全同构。

### 1.3 音频（第 1 期如实跳过）

音频 strf 声明 PCM/单声道/22050Hz/16-bit —— **AMV 头会说谎**：FFmpeg
对 AMV 音频无条件强制 `AV_CODEC_ID_ADPCM_IMA_AMV`（743B 奇数块尾随
15fps 视频块的形态亦与 IMA ADPCM 分块吻合）。第 1 期不做 ADPCM：
`audio()` 如实返回 `None`（视频照常可用），声明值经
`audio_declared_format()` 原样暴露（探针指名打印"DECLARED … skipped"）。

### 1.4 API 形态（引擎面）

```rust
let amv = AmvVideo::parse(&bytes)?;   // 整份拷进 Arc<[u8]>（avi.rs 同款）
let info = amv.video_info();          // VideoInfo { 160, 128, 15.0, 4233, VideoCodec::Amv }
let img  = amv.frame(i)?;             // DecodedImage（惰性逐帧，已翻转）
let wav  = amv.audio();               // 第 1 期恒 None（ADPCM 未做）
```

`VideoCodec` 新增 `Amv` 变体（不复用 `Mjpg` 的语义理由：AMV 帧字节
**不能**直接交 JPEG 解码器，需合成头 —— 调用方需要知道这个区别；
`AviVideo::frame` 对该值防御性指名报错，AVI 解析面到不了它）。

---

## §2 Adapter 通用性验证（本期元目的的实证结论）

本期立项的元目的：验证**手写容器 × 两类帧解码（库 / 手写）**的组合在
第二种容器上是否成立。结论：**成立，且边界比预期更清晰**。四条证据：

1. **容器怪癖被完整吸收在 demux 层**：坏尺寸/无 pad/表外置/流序定流
   四条怪癖全部消化在 `AmvVideo::parse`（~120 行），对引擎面零泄漏 ——
   `video_info()/frame()/audio()` 的形状与 `AviVideo` 逐一对齐，上层
   "换格式 = 换 parse 调用"成立；
2. **帧解码"走库"路线二次成立**：S14.2 的 MJPG 是"帧体即标准 JPEG"，
   本期 AMV 是"帧体缺头但可用常量表补齐"—— 两者的落点都是
   `image::decode_image` 这一个入口，G13 白名单一条未扩。**"手写帧
   解码"路线（DIB，S14.2）与"合成后走库"路线（AMV，本期）并存**，
   Adapter 两种帧解码形态各有一例实证；
3. **第三方类型依然不出 crate**：zune-jpeg（image 内部）与合成段字节
   都被封在 `amv.rs` 内，产出只有 `DecodedImage`/`nes_audio::Wav`；
4. **失败信息即下一轮输入的纪律兑现**：探针期两条假设（表在 strf、
   SOF 半高场）被真实文件证伪 —— 处理方式是"回到 FFmpeg 源码找实证"
   （`sp5xdec.c`/`avidec.c`/`mjpegdec.c`），不是硬凑。最终方案与
   FFmpeg 逐字节同源，为第 2 期（ADPCM 音频）留下了同一条参照路径
   （`adpcm.c` 的 `ADPCM_IMA_AMV` 分支）。

---

## §3 harness 与真实文件取证（examples/avi_probe.rs）

SAMPLES 表首项即真实 AMV（ASCII 路径副本）；探针按 RIFF form 分流
（'AMV ' -> `AmvVideo`，其余 -> `AviVideo`）。真实文件输出：

```text
[avi_probe] input: C:/Users/Administrator/Videos/text/spider_amv.amv
[avi_probe] size  : 160x128
[avi_probe] fps   : 14.999925
[avi_probe] frames: 4233
[avi_probe] codec : AMV (headerless MJPEG, sp5x-synthesized headers)
[avi_probe] parse : 6 ms (file 13847 KB)
[avi_probe] audio : DECLARED tag=1 ch=1 rate=22050 bits=16 -- skipped
                     (real payload is ADPCM_IMA_AMV, ...)
[avi_probe] frames decoded+saved: 17/4233 (stride 265, cap 16) -> amv_frame_NNN.bmp (5 ms)
```

**解码质量（肉眼 + 统计双重取证）**：

* 31 帧全片等距取样（含首/中/尾帧）：**0 解码失败**；
* 帧 141（银发人物立绘 + 片名文字）、帧 1269（双人场景 + 粉紫背景）
  肉眼确认：正立（翻转方向正确）、彩色、结构完整，单帧独立色最高
  ~11000；中段暗场帧（如帧 2822，独立色 92）同样成形；
* 逐帧趋势：开头 ~百帧黑场淡入（帧 0/1 实测均值 (1,1,1)）→ 正片彩色
  动画（暗红/亮白场景交替）→ 片尾黑场淡出（帧 4091/4232 黑）—— 与
  "OP 动画"的内容形态一致；首帧 326B 小块体即黑场全零系数块（80 MCU
  全部 DC=-78、零非零 AC，手工熵解码证实），**不是解码缺陷**；
* 偏色评估：中段帧通道均值随场景在暖白 (227,223,213) 与暗红 (48,21,16)
  间正常摆动，未见系统性色偏（固定 DQT 是 SP5X 固件原表，FFmpeg 同款）。

测试面（5 个新用例）：手工 4:2:0 熵流逐像素精确往返（独立于任何编码器
的规范推导流）、image 库 4:4:4 编码交叉验证 + 翻转方向钉死、无 pad 奇块
+ 坏尺寸 + 'AMV_END_' 尾巴三怪齐上、拒绝面（非 AMV/缺 movi/零尺寸/越界/
坏壳）、真实文件契约（skip-if-missing，BMP 落 tmpdir 不入仓库）。

---

## §4 门禁（全部在本 worktree 内执行）

| 项 | 结果 |
|---|---|
| nes-media `cargo test --release` | **26 绿**（基线 21 + amv 新增 5；含 real_media 2 项用户实测曲） |
| 八 crate `cargo test --release` | **636 绿 / 0 败**（基线 631 + 5）：asset 34 / audio 40 / media 26 / render-api 45 / render-extract 56 / render-wgpu 123 / runtime 79 / scene 233 |
| `cargo clippy --all-targets` | **0 警告 × 8** |
| worktree 根 `python check_dependency_direction.py` | **13/13**（G13 未改：合成表为常量、zune-jpeg 本就在 image 传递闭包内，白名单原样覆盖） |
| `cargo run --release --example avi_probe` | 真实 AMV 出 17 张 BMP + 两个真实 AVI 指名报告 + 合成演示全链路通过 |

改动面：`nes-media/src/amv.rs`（新增，demux + JPEG 合成 + 模块文档 +
5 用例）、`nes-media/src/avi.rs`（`VideoCodec::Amv` 变体 + frame 防御
臂）、`nes-media/src/lib.rs`（挂模块 + 重导出 + 覆盖面文档更新）、
`nes-media/examples/avi_probe.rs`（AMV 探针分流 + 文档头）、
`nes-media/Cargo.lock`（无新依赖）。守卫脚本一行未动。

---

## §5 遗留（下一轮输入）

1. **ADPCM 音频（第 2 期正题）**：'01wb' 实际载荷 IMA ADPCM
   （FFmpeg `adpcm.c` 的 `ADPCM_IMA_AMV` 分支是参照；块结构 4 字节头 +
   半字节 nibble 流）。`audio_chunks` 区间已留位，`audio()` 签名不动，
   补解码即出 `nes_audio::Wav`；
2. **播放管线**：与 S14.2 同一条遗留 —— 本期是资源管线，逐帧推进/
   seek/时钟同步是后续（`frame(i)` 惰性接口已留好形状）；
3. **AMV 变体鲁棒性**：本模块按"SP5X 家族固定表 + amvh 提供尺寸"的
   FFmpeg 口径实现；网上另有 'AMV_'（无空格）标记的近似变体与带 idx1
   的写手，拿到样本再验（`parse` 的 movi 字面扫描兜底已留）；
4. **MP4/H.264 仍不进本 crate**：avio/FFmpeg 包装路线不变（见 S14.2
   §5.4），两条路线汇合点是同一批引擎 DTO。
