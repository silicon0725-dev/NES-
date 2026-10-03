//! AVI 视频资源管线实验 —— **S14.2"小而完整"**（RIFF 容器手写 + 帧解码组合既有能力）。
//!
//! # 为什么是 AVI、为什么这样写
//!
//! 用户裁决：Beta 需要的媒体能力做完整即可，**不为"支持视频"提前引入完整
//! 媒体框架**。AVI 是理想的实验格式：
//!
//! * 容器是 **RIFF** —— 与既有 `nes-audio/src/wav.rs` 手写解析器同族，
//!   块头遍历 / pad 字节 / 不信任总长字段的家法原样沿用；
//! * 帧编码选两个极端：未压缩 DIB（手写 BGR->RGBA 百行内完成）与
//!   Motion JPEG（逐块走 `image` 库解码）—— **容器手写、帧解码走库**，
//!   这正是 S14"Adapter 抽象通用性"的证明点：解码产物仍是干净 DTO
//!   [`DecodedImage`]，第三方类型不出本 crate；
//! * 依赖零新增：`image` 已在 G13 白名单（MJPG 帧解码复用 [`crate::image`]
//!   的同一入口），音轨 PCM 直接产引擎既有类型 `nes_audio::Wav` 进混音器。
//!
//! # 容器结构（AVI 1.0，本模块的覆盖面）
//!
//! ```text
//! RIFF('AVI ')
//!  ├─ LIST('hdrl')
//!  │   ├─ avih                  主头（56B；本模块只做存在性跳过 —— 权威字段
//!  │   │                        取自 strh/strf/索引，真实文件 avih.dwWidth 与
//!  │   │                        strf 不一致者存在，不信头信数据）
//!  │   └─ LIST('strl') × N
//!  │       ├─ strh              流头（56B；fccType 'vids'/'auds'；
//!  │       │                    fps = dwRate / dwScale）
//!  │       └─ strf              流格式：视频 = BITMAPINFOHEADER（40B 起），
//!  │                            音频 = PCMWAVEFORMAT（16B 起，WAVE_FORMAT 1）
//!  ├─ LIST('movi')
//!  │   ├─ '00dc' / '00db'       视频帧块（两位流号 + dc 压缩 / db 未压缩）
//!  │   ├─ '01wb'                音频块（同一条两位流号规则）
//!  │   └─ LIST('rec ')          帧分组（扫描兜底路径递归展开）
//!  └─ idx1                      帧索引（16B/项：ckid + flags + offset + size）
//! ```
//!
//! # 帧定位（idx1 与扫描兜底，oxideav-avi 同款双路径）
//!
//! * 有 `idx1`：逐项解析 16 字节条目。条目 offset 的基准有两种流派
//!   （相对 'movi' 四字码 / 文件绝对），先用**首个非零条目做探针**——
//!   两种基准算出的位置上若恰好躺着条目声称的 ckid，即认定该基准
//!   （两者都中/都不中时保守取 movi 相对，与 oxideav-avi 的
//!   `build_idx_table` 同一裁决）；单条越界的坏条目跳过不中断；
//! * 无 `idx1`（或 idx1 里一帧都没有）：按 `movi` 顺序流式扫块兜底，
//!   `'rec '` 分组递归展开。
//!
//! # 编解码器覆盖面（与拒绝面同样明确）
//!
//! * 收（视频）：`biCompression = 0` 或 `'DIB '` 的**未压缩 24-bit BGR
//!   DIB**（底朝上行序；负 biHeight 的顶朝下变体也认）——手写转换；
//!   `'MJPG'`（Motion JPEG）—— 逐块 `image::load_from_memory`（有损，
//!   像素不保证逐位还原）；
//! * 收（音频）：`WAVE_FORMAT_PCM (1)` 的 8/16/24-bit 轨 —— 直接组装成
//!   `nes_audio::Wav`（16-bit 面板：8-bit 无符号偏移换算、24-bit 取高
//!   16 位，与 `nes-audio/src/wav.rs` 的 24-bit 口径一致）；
//! * 拒（视频）：其它四字码如实报 [`VideoCodec::Unsupported`]（原文值
//!   保留），[`AviVideo::frame`] 给指名错误 —— 解容器不等于解得动帧；
//! * 拒（音频）：非 PCM 标签（float/a-law/ADPCM…）**跳过音轨**（视频
//!   照常可用，[`AviVideo::audio`] 返回 `None`）—— 容器级容忍、解码级
//!   拒绝，两层失败面分开；
//! * 断：非 RIFF/非 'AVI '（报 [`MediaError::UnsupportedFormat`]）、块体
//!   越界（截断，报 [`MediaError::Decode`]）、缺 `movi`/视频流（同上）。
//!
//! # 明确不做（遗留面，见 S14.2 文档 §5）
//!
//! * OpenDML 2.0（`AVIX` 续段 / `indx` 超级索引 / `ix##`）：只认首个
//!   `RIFF AVI ` 段（实验用例都是小文件）；
//! * 其它帧编码（'XVID'/'DIVX'/H.264/…）、调色板 DIB（bpp <= 8）、
//!   32-bit DIB：容器认得出、帧解不动，报指名错误；
//! * 流式播放（seek/逐帧推进光标）：本期是**资源管线**（解出帧/音轨
//!   给上层），播放管线是后续。
//!
//! # 与 avio（FFmpeg 包装）路线的边界
//!
//! 未来 MP4/H.264 这类"容器 + 编解码器都要重炮"的格式走 avio 适配
//! （参照源码 `ruference/avio` 把解码产物抽象成 DTO 的接口形态），
//! **不进本 crate** —— 本模块是"手写容器 + 白名单帧解码"的轻量路线
//! 闭环，两条路线的汇合点是同一批引擎 DTO（[`DecodedImage`] /
//! `nes_audio::Wav`）。
//!
//! # AMV 预留位
//!
//! AMV（MTV 播放器变体：RIFF 容器变体 + 自有帧编码）是下一轮 Adapter
//! 通用性试金石，本模块不做；预留方式 = 本 crate 新模块
//! （`amv.rs`）+ 既有 DTO 复用，容器手法直接回抄本文件的 RIFF 遍历。
//!
//! # 数据持有（ttf.rs 先例）
//!
//! [`AviVideo::parse`] 接 `&[u8]`，一次拷贝进内部 `Arc<[u8]>` —— 结构体
//! 拥有数据、可 Clone、跨线程共享；帧/音轨的解码是**惰性逐帧**的
//! （[`AviVideo::frame`]），音轨 PCM 在 [`AviVideo::audio`] 时组装。

#![forbid(unsafe_code)]

use std::ops::Range;
use std::sync::Arc;

use nes_audio::Wav;

use crate::image::DecodedImage;
use crate::MediaError;

// ------------------------------------------------------------
// 公开 DTO（引擎面）
// ------------------------------------------------------------

/// 视频流编码器（`strf` BITMAPINFOHEADER 的 `biCompression` 四字码归一）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// 未压缩 BGR DIB（`biCompression = 0` 即 BI_RGB，或字面四字码 `'DIB '`）。
    Dib,
    /// Motion JPEG（`'MJPG'`）—— 每帧是一张独立 JPEG。
    Mjpg,
    /// AMV 变体 Motion JPEG（S14.3）：帧体是无头 MJPEG
    /// （`FFD8`+熵数据+`FFD9`），需按 FFmpeg sp5x 方案合成标准 JPEG
    /// 才可解。只由 [`crate::amv::AmvVideo`] 产生 —— AVI 容器的
    /// `biCompression` 分类不会给出该值（语义选择：不复用 Mjpg，因为
    /// "帧字节可直接交 JPEG 解码器"对 AMV 不成立，调用方需知道这点）。
    Amv,
    /// 未收录的四字码（LE u32 原值保留，指名报错用）。
    Unsupported(u32),
}

/// 视频流元信息（[`AviVideo::video_info`] 出口，干净值类型）。
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    /// 宽（像素，来自 BITMAPINFOHEADER `biWidth`）。
    pub width: u32,
    /// 高（像素；`|biHeight|`，行序符号另记解析器内部）。
    pub height: u32,
    /// 帧率（`strh.dwRate / dwScale`；分母为 0 的病态头记 0.0，不 panic）。
    pub fps: f32,
    /// 帧数（**实际定位到的视频块数**，来自 idx1 或扫描 —— 不信 strh.dwLength）。
    pub frame_count: u32,
    /// 帧编码器（见 [`VideoCodec`] 的覆盖面说明）。
    pub codec: VideoCodec,
}

/// 一个解析完成的 AVI：视频流元信息 + 惰性逐帧解码 + 可选 PCM 音轨。
///
/// 数据持有照 `nes-render-wgpu::ttf` 先例：`parse` 时整份字节拷进
/// `Arc<[u8]>`，结构体拥有数据、可 Clone。
#[derive(Debug, Clone)]
pub struct AviVideo {
    /// 整份文件字节（帧块体按区间从这切）。
    data: Arc<[u8]>,
    /// 视频流元信息（解析期定死）。
    info: VideoInfo,
    /// 底朝上 DIB（`biHeight > 0`，最常见）；`false` = 顶朝下（负 biHeight）。
    top_down: bool,
    /// `biBitCount`（DIB 帧解码只认 24，其余指名报错）。
    bit_count: u16,
    /// 每个视频帧块体的绝对字节区间（movi 内定位，顺序即帧序）。
    frames: Vec<Range<usize>>,
    /// 24-bit DIB 的行步长（含 4 字节对齐填充）。
    stride: usize,
    /// 音轨（PCM 归一参数 + 块体区间；非 PCM 音轨为 `None`，记模块文档）。
    audio: Option<AudioTrack>,
}

/// 音轨归一参数 + PCM 块体区间（[`AviVideo::audio`] 时组装成 `Wav`）。
#[derive(Debug, Clone)]
struct AudioTrack {
    sample_rate: u32,
    channels: u16,
    /// 位深（8/16/24；换算到 Wav 的 16-bit 面板）。
    bits: u16,
    /// 每个 'NNwb' 块体的绝对区间（顺序拼接）。
    chunks: Vec<Range<usize>>,
}

// ------------------------------------------------------------
// 解析
// ------------------------------------------------------------

impl AviVideo {
    /// 从内存字节解析一个 AVI（只认字节流，不做文件 IO）。
    ///
    /// 整份字节拷贝进 `Arc<[u8]>`（ttf.rs 先例）；视频流必须存在且至少
    /// 定位到一帧，音轨可缺。失败面见模块文档"覆盖面"。
    pub fn parse(data: &[u8]) -> Result<Self, MediaError> {
        let data: Arc<[u8]> = Arc::from(data.to_vec().into_boxed_slice());
        let parsed = parse_container(&data)?;

        let video = parsed.video.ok_or_else(|| {
            MediaError::Decode("AVI 解析失败：hdrl 中未找到视频流（'vids' strl）".into())
        })?;
        let movi = parsed.movi.ok_or_else(|| {
            MediaError::Decode("AVI 解析失败：未找到 movi 列表（无媒体数据）".into())
        })?;

        // 帧定位：idx1 优先，扫描兜底（两条路径对音轨块同样生效）。
        let (mut frames, mut audio_chunks) = match parsed.idx1 {
            Some(range) => locate_frames_by_index(&data, range, &movi, video.stream_index),
            None => (Vec::new(), Vec::new()),
        };
        if frames.is_empty() {
            // idx1 缺失，或坏索引一帧都没给出：按 movi 顺序扫块兜底。
            let scanned = locate_frames_by_scan(&data, movi.children_start, movi.children_end, video.stream_index, parsed.audio.as_ref().map(|a| a.stream_index));
            frames = scanned.frames;
            audio_chunks = scanned.audio_chunks;
        }
        if frames.is_empty() {
            return Err(MediaError::Decode(
                "AVI 解析失败：movi 中未定位到任何视频帧块".into(),
            ));
        }

        // 音轨收口：非 PCM / 病态参数（声道、位深出格）整体跳过（文档面）。
        let audio = parsed.audio.and_then(|a| {
            let pcm_ok = a.format_tag == 1
                && (a.channels == 1 || a.channels == 2)
                && (a.bits == 8 || a.bits == 16 || a.bits == 24);
            if pcm_ok {
                Some(AudioTrack {
                    sample_rate: a.sample_rate,
                    channels: a.channels,
                    bits: a.bits,
                    chunks: audio_chunks,
                })
            } else {
                None
            }
        });

        let fps = if video.scale > 0 {
            video.rate as f32 / video.scale as f32
        } else {
            0.0
        };
        let codec = classify_codec(video.compression);
        let stride = dib_stride(video.width);
        Ok(AviVideo {
            data,
            info: VideoInfo {
                width: video.width,
                height: video.height,
                fps,
                frame_count: frames.len() as u32,
                codec,
            },
            top_down: video.top_down,
            bit_count: video.bit_count,
            frames,
            stride,
            audio,
        })
    }

    /// 视频流元信息（宽高 / 帧率 / 帧数 / 编解码器）。
    pub fn video_info(&self) -> VideoInfo {
        self.info.clone()
    }

    /// 解码第 `i` 帧（0 起）为 RGBA8（复用图像面 [`DecodedImage`] DTO）。
    ///
    /// DIB：手写 BGR->RGBA + 底朝上行序翻转（每像素 alpha 恒 255 —— DIB
    /// 无 alpha 语义）；MJPG：整块交 `image` 库解码（有损，像素不逐位
    /// 保证）；其它编码：指名报错。
    pub fn frame(&self, i: u32) -> Result<DecodedImage, MediaError> {
        let idx = i as usize;
        if idx >= self.frames.len() {
            return Err(MediaError::Decode(format!(
                "AVI 帧解码失败：帧序号 {i} 越界（共 {} 帧）",
                self.frames.len()
            )));
        }
        let range = self.frames[idx].clone();
        let body = self
            .data
            .get(range)
            .ok_or_else(|| MediaError::Decode("AVI 帧解码失败：帧块体区间越界".into()))?;
        match self.info.codec {
            VideoCodec::Dib => {
                if self.bit_count != 24 {
                    return Err(MediaError::Decode(format!(
                        "AVI 帧解码失败：DIB 仅支持 24 位（实际 {} 位）",
                        self.bit_count
                    )));
                }
                if self.stride == 0 {
                    return Err(MediaError::Decode(
                        "AVI 帧解码失败：DIB 宽度为 0".into(),
                    ));
                }
                let h = self.info.height as usize;
                let need = self
                    .stride
                    .checked_mul(h)
                    .ok_or_else(|| MediaError::Decode("AVI 帧解码失败：像素总量溢出".into()))?;
                if body.len() < need {
                    return Err(MediaError::Decode(format!(
                        "AVI 帧解码失败：DIB 像素数据不足（需 {need} 字节，实际 {}）",
                        body.len()
                    )));
                }
                decode_dib_24(body, self.info.width, self.info.height, self.top_down, self.stride)
            }
            VideoCodec::Mjpg => crate::image::decode_image(body),
            // Amv 只会由 amv::AmvVideo 产生（AVI 解析面到不了这个值）——
            // 防御性指名报错，不假装会解。
            VideoCodec::Amv => Err(MediaError::Decode(
                "AVI 帧解码失败：AMV 帧编码不属于 AVI 容器解码面（见 amv 模块）".into(),
            )),
            VideoCodec::Unsupported(raw) => {
                let bytes = raw.to_le_bytes();
                Err(MediaError::Decode(format!(
                    "AVI 帧解码失败：不支持的帧编码（biCompression 四字码 {:?}）",
                    String::from_utf8_lossy(&bytes)
                )))
            }
        }
    }

    /// 音轨（PCM 归一成 `nes_audio::Wav`，直接可进混音器）。
    ///
    /// 无音轨、或音轨非 PCM（float/ADPCM/…，解析期已跳过）时返回 `None`
    /// —— 视频照常可用，音频缺失不构成容器级失败（记模块文档）。
    pub fn audio(&self) -> Option<Wav> {
        let track = self.audio.as_ref()?;
        let bytes_per = usize::from(track.bits / 8);
        let mut pcm = Vec::new();
        for range in &track.chunks {
            if let Some(body) = self.data.get(range.clone()) {
                pcm.extend_from_slice(body);
            }
        }
        // 不足一个样本的尾巴丢弃（坏写手的奇数尾巴不炸解码）。
        pcm.truncate(pcm.len() - pcm.len() % bytes_per);
        let samples: Vec<i16> = match track.bits {
            8 => pcm
                .iter()
                .map(|&v| (i16::from(v) - 128) << 8)
                .collect(),
            16 => pcm
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect(),
            // 24-bit 取高 16 位（算术右移保留符号）—— 与 nes-audio wav.rs 同一口径。
            _ => pcm
                .chunks_exact(3)
                .map(|c| {
                    let v = (c[0] as i32) | ((c[1] as i32) << 8) | ((c[2] as i32) << 16);
                    let ext = (v << 8) >> 8; // 符号扩展
                    (ext >> 8) as i16
                })
                .collect(),
        };
        Some(Wav {
            sample_rate: track.sample_rate,
            channels: track.channels,
            samples,
        })
    }
}

// ------------------------------------------------------------
// 容器遍历（RIFF 家法：按块头走、奇数补 pad、不信任总长字段）
// ------------------------------------------------------------

/// movi 列表定位：四字码绝对位置 + 子块区间。
#[derive(Debug, Clone, Copy)]
struct MoviLoc {
    /// 'movi' 四字码的绝对位置（idx1 相对偏移的常见基准点）。
    fourcc_pos: usize,
    /// 子块区间的开闭端（LIST 声明长度划界，截断在容器层就已报错）。
    children_start: usize,
    children_end: usize,
}

/// 视频流参数（strh + BITMAPINFOHEADER 合并视图）。
struct VideoStream {
    stream_index: usize,
    width: u32,
    height: u32,
    /// `biHeight < 0`（顶朝下 DIB）。
    top_down: bool,
    compression: u32,
    /// `biBitCount`（DIB 解码只认 24；记录在案以便指名报错）。
    bit_count: u16,
    rate: u32,
    scale: u32,
}

/// 音频流参数（strh + PCMWAVEFORMAT 合并视图）。
struct AudioStream {
    stream_index: usize,
    format_tag: u16,
    channels: u16,
    sample_rate: u32,
    bits: u16,
}

/// 容器一轮遍历的产出。
struct Container {
    /// strl 计数（movi 块名的两位流号 = strl 在 hdrl 里的序数，与流类型
    /// 无关 —— 未知类型流也占号，这决定 '00dc'/'01wb' 的对位）。
    stream_count: usize,
    movi: Option<MoviLoc>,
    idx1: Option<Range<usize>>,
    video: Option<VideoStream>,
    audio: Option<AudioStream>,
}

/// 遍历顶层 RIFF（'AVI '）：收集 hdrl 流头 / movi 定位 / idx1 区间。
///
/// 家法与 wav.rs 同一条：块按"4 字节名 + 4 字节 LE 长 + 体"走，奇数体
/// 长补一个 pad 字节；声明长度越过实际缓冲即报截断（不静默吃掉）；
/// 顶层 RIFF 总长字段不信任（以实际缓冲结束为准）；未知块（JUNK/
/// INFO/…）按块头跳过。
fn parse_container(data: &[u8]) -> Result<Container, MediaError> {
    if data.len() < 12 {
        return Err(MediaError::UnsupportedFormat);
    }
    if &data[0..4] != b"RIFF" {
        return Err(MediaError::UnsupportedFormat);
    }
    if &data[8..12] != b"AVI " {
        return Err(MediaError::UnsupportedFormat);
    }

    let mut out = Container { stream_count: 0, movi: None, idx1: None, video: None, audio: None };
    let mut pos = 12usize;
    while pos + 8 <= data.len() {
        let id: [u8; 4] = data[pos..pos + 4].try_into().expect("块名固定 4 字节");
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().expect("长度固定 4 字节"))
            as usize;
        let body = pos + 8;
        if body + size > data.len() {
            return Err(MediaError::Decode(format!(
                "AVI 解析失败：顶层块 {:?} 声明长度越界（文件截断或损坏）",
                String::from_utf8_lossy(&id)
            )));
        }
        match &id {
            b"LIST" => {
                if size < 4 {
                    return Err(MediaError::Decode(
                        "AVI 解析失败：LIST 块长度不足 4（装不下表类型）".into(),
                    ));
                }
                let form = &data[body..body + 4];
                match form {
                    b"hdrl" => parse_hdrl(data, body + 4, body + size, &mut out)?,
                    // 只认首个 movi（AVI 1.0 单段；AVIX 续段不做，记文档）。
                    b"movi" if out.movi.is_none() => {
                        out.movi = Some(MoviLoc {
                            fourcc_pos: body,
                            children_start: body + 4,
                            children_end: body + size,
                        });
                    }
                    _ => {} // 后续 movi / INFO 等元数据列表：跳过
                }
            }
            b"idx1" => out.idx1 = Some(body..body + size),
            _ => {} // JUNK / 其它：按块头跳过
        }
        pos = body + size;
        // RIFF 规范 pad：奇数体长后补 1 字节对齐；文件恰好在末尾时不补读。
        if size % 2 == 1 && pos < data.len() {
            pos += 1;
        }
    }
    Ok(out)
}

/// 遍历 hdrl：跳过 avih（权威字段信 strh/strf，记模块文档），逐个 strl 认流。
fn parse_hdrl(data: &[u8], start: usize, end: usize, out: &mut Container) -> Result<(), MediaError> {
    let mut pos = start;
    while pos + 8 <= end {
        let id: [u8; 4] = data[pos..pos + 4].try_into().expect("块名固定 4 字节");
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().expect("长度固定 4 字节"))
            as usize;
        let body = pos + 8;
        if body + size > end.min(data.len()) {
            return Err(MediaError::Decode(
                "AVI 解析失败：hdrl 内块声明长度越界".into(),
            ));
        }
        if &id == b"LIST" && size >= 4 && &data[body..body + 4] == b"strl" {
            parse_strl(data, body + 4, body + size, out)?;
            out.stream_count += 1;
        }
        // avih / JUNK / odml 等：跳过。
        pos = body + size;
        if size % 2 == 1 && pos < end {
            pos += 1;
        }
    }
    Ok(())
}

/// 遍历一个 strl：strh（流类型/时基）+ strf（位图头/波形格式）。
///
/// 多视频流取第一个（'vids'），多音频流取第一个（'auds'）—— 实验面
/// 单视频 + 单音频已覆盖，多流选择记文档遗留。
fn parse_strl(data: &[u8], start: usize, end: usize, out: &mut Container) -> Result<(), MediaError> {
    let mut stream_type: Option<[u8; 4]> = None;
    let stream_index = out.stream_count;
    let mut rate = 0u32;
    let mut scale = 0u32;
    let mut strf: Option<Range<usize>> = None;

    let mut pos = start;
    while pos + 8 <= end {
        let id: [u8; 4] = data[pos..pos + 4].try_into().expect("块名固定 4 字节");
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().expect("长度固定 4 字节"))
            as usize;
        let body = pos + 8;
        if body + size > end.min(data.len()) {
            return Err(MediaError::Decode(
                "AVI 解析失败：strl 内块声明长度越界".into(),
            ));
        }
        match &id {
            b"strh" => {
                if size < 56 {
                    return Err(MediaError::Decode(format!(
                        "AVI 解析失败：strh 流头不足 56 字节（实际 {size}）"
                    )));
                }
                let ty: [u8; 4] = data[body..body + 4].try_into().expect("fccType 固定 4 字节");
                scale = u32_le(data, body + 20);
                rate = u32_le(data, body + 24);
                stream_type = Some(ty);
            }
            b"strf" => strf = Some(body..body + size),
            _ => {} // strd（编解码器私数据）/ strn（流名）/ JUNK：跳过
        }
        pos = body + size;
        if size % 2 == 1 && pos < end {
            pos += 1;
        }
    }

    let (Some(ty), Some(range)) = (stream_type, strf) else {
        return Ok(()); // 半截 strl（缺 strh/strf）：容忍，不认流。
    };
    match &ty {
        b"vids" if out.video.is_none() => {
            if range.len() < 40 {
                return Err(MediaError::Decode(format!(
                    "AVI 解析失败：视频 strf 不足 BITMAPINFOHEADER 40 字节（实际 {}）",
                    range.len()
                )));
            }
            let width_raw = i32_le(data, range.start + 4);
            let height_raw = i32_le(data, range.start + 8);
            if width_raw <= 0 || height_raw == 0 {
                return Err(MediaError::Decode(format!(
                    "AVI 解析失败：视频流尺寸非法（{width_raw}x{height_raw}）"
                )));
            }
            let bit_count = u16_le(data, range.start + 14);
            let compression = u32_le(data, range.start + 16);
            out.video = Some(VideoStream {
                stream_index,
                width: width_raw as u32,
                height: height_raw.unsigned_abs(),
                top_down: height_raw < 0,
                compression,
                bit_count,
                rate,
                scale,
            });
        }
        b"auds" if out.audio.is_none() => {
            if range.len() < 16 {
                return Ok(()); // 音频 strf 残缺：跳过音轨（视频优先）。
            }
            out.audio = Some(AudioStream {
                stream_index,
                format_tag: u16_le(data, range.start),
                channels: u16_le(data, range.start + 2),
                sample_rate: u32_le(data, range.start + 4),
                bits: u16_le(data, range.start + 14),
            });
        }
        _ => {} // 第二条视频/音频流或未知流类型：跳过（记文档遗留）。
    }
    Ok(())
}

// ------------------------------------------------------------
// 帧定位（idx1 双基准探针 / movi 顺序扫描，oxideav-avi 同款双路径）
// ------------------------------------------------------------

/// 解析两位流号块名：`b"00dc"` -> `(0, b"dc")`。
fn parse_stream_ckid(ckid: &[u8; 4]) -> Option<(usize, [u8; 2])> {
    let d1 = (ckid[0] as char).to_digit(10)?;
    let d2 = (ckid[1] as char).to_digit(10)?;
    Some(((d1 * 10 + d2) as usize, [ckid[2], ckid[3]]))
}

fn is_video_code(code: [u8; 2]) -> bool {
    code == *b"dc" || code == *b"db"
}

/// 按 idx1 定位帧/音轨块。offset 基准用首个非零条目探针裁决：
/// 两个候选位置（movi 四字码相对 / 文件绝对）上若恰好躺着条目声称的
/// ckid 即认定该基准；都中/都不中保守取 movi 相对（业界多数派，且
/// 与 oxideav-avi 的 `build_idx_table` 同一裁决）。单条越界跳过不中断
/// （坏索引不该废掉整个文件），一条有效条目都没有时返回空交扫描兜底。
fn locate_frames_by_index(
    data: &[u8],
    idx1: Range<usize>,
    movi: &MoviLoc,
    video_index: usize,
) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let raw = &data[idx1];
    let n = raw.len() / 16;
    if n == 0 {
        return (Vec::new(), Vec::new());
    }

    // ---- 探针：定基准 ----
    let mut base = movi.fourcc_pos as u64; // 保守默认：movi 相对。
    for i in 0..n {
        let at = i * 16;
        let ckid: [u8; 4] = raw[at..at + 4].try_into().expect("ckid 固定 4 字节");
        let off = u32::from_le_bytes(raw[at + 8..at + 12].try_into().expect("偏移固定 4 字节"));
        if parse_stream_ckid(&ckid).is_none() || off == 0 {
            continue;
        }
        let try_movi = movi.fourcc_pos as u64 + off as u64;
        let movi_ok = probe_ckid_at(data, try_movi, &ckid);
        let abs_ok = probe_ckid_at(data, off as u64, &ckid);
        base = match (movi_ok, abs_ok) {
            (true, false) => movi.fourcc_pos as u64,
            (false, true) => 0,
            _ => movi.fourcc_pos as u64, // 都中/都不中：多数派。
        };
        break;
    }

    // ---- 正式收集 ----
    let mut frames = Vec::new();
    let mut audio_chunks = Vec::new();
    for i in 0..n {
        let at = i * 16;
        let ckid: [u8; 4] = raw[at..at + 4].try_into().expect("ckid 固定 4 字节");
        let size = u32::from_le_bytes(raw[at + 12..at + 16].try_into().expect("长度固定 4 字节"))
            as usize;
        let off = u32::from_le_bytes(raw[at + 8..at + 12].try_into().expect("偏移固定 4 字节"));
        let Some((stream, code)) = parse_stream_ckid(&ckid) else {
            continue; // 'rec ' 等非流条目：跳过。
        };
        let id_pos = base + off as u64;
        if id_pos + 8 + size as u64 > data.len() as u64 {
            continue; // 越界条目：跳过（坏索引容忍）。
        }
        let id_pos = id_pos as usize;
        if data[id_pos..id_pos + 4] != ckid {
            continue; // 条目与实际块名不符：跳过。
        }
        let body = id_pos + 8..id_pos + 8 + size;
        if stream == video_index && is_video_code(code) {
            frames.push(body);
        } else if code == *b"wb" {
            audio_chunks.push(body);
        }
    }
    (frames, audio_chunks)
}

/// 探针：`pos` 处是否恰好躺着 `ckid`（含越界防御）。
fn probe_ckid_at(data: &[u8], pos: u64, ckid: &[u8; 4]) -> bool {
    let pos = match usize::try_from(pos) {
        Ok(p) => p,
        Err(_) => return false,
    };
    pos + 4 <= data.len() && &data[pos..pos + 4] == ckid
}

/// movi 扫描兜底的收集结果。
struct ScanResult {
    frames: Vec<Range<usize>>,
    audio_chunks: Vec<Range<usize>>,
}

/// 无 idx1（或索引全坏）时按 movi 顺序流式扫块（'rec ' 分组递归展开）。
///
/// 与容器层同一条家法：按块头走、奇数补 pad；子块声明越过 movi 划界
/// 即停扫（截断容忍 —— 扫描兜底本就是"能救几帧救几帧"的路径）。
fn locate_frames_by_scan(
    data: &[u8],
    start: usize,
    end: usize,
    video_index: usize,
    audio_index: Option<usize>,
) -> ScanResult {
    let mut out = ScanResult { frames: Vec::new(), audio_chunks: Vec::new() };
    scan_movi_range(data, start, end, video_index, audio_index, &mut out);
    out
}

fn scan_movi_range(
    data: &[u8],
    start: usize,
    end: usize,
    video_index: usize,
    audio_index: Option<usize>,
    out: &mut ScanResult,
) {
    let end = end.min(data.len());
    let mut pos = start;
    while pos + 8 <= end {
        let id: [u8; 4] = data[pos..pos + 4].try_into().expect("块名固定 4 字节");
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().expect("长度固定 4 字节"))
            as usize;
        let body = pos + 8;
        if body + size > end {
            return; // 声明越界：停扫（截断容忍）。
        }
        if &id == b"LIST" && size >= 4 {
            // 'rec ' 帧分组：递归展开（分组只是打包，不改变块语义）。
            scan_movi_range(data, body + 4, body + size, video_index, audio_index, out);
        } else if let Some((stream, code)) = parse_stream_ckid(&id) {
            let range = body..body + size;
            if stream == video_index && is_video_code(code) {
                out.frames.push(range);
            } else if Some(stream) == audio_index && code == *b"wb" {
                out.audio_chunks.push(range);
            }
        }
        pos = body + size;
        if size % 2 == 1 && pos < end {
            pos += 1;
        }
    }
}

// ------------------------------------------------------------
// 帧解码（DIB 手写 / MJPG 走 image）
// ------------------------------------------------------------

/// 24-bit DIB 行步长：每行 3 字节/像素，按 4 字节对齐补齐。
fn dib_stride(width: u32) -> usize {
    (width as usize * 3).div_ceil(4) * 4
}

/// 未压缩 24-bit BGR DIB -> RGBA8（行序按 biHeight 符号翻转）。
fn decode_dib_24(
    body: &[u8],
    width: u32,
    height: u32,
    top_down: bool,
    stride: usize,
) -> Result<DecodedImage, MediaError> {
    let w = width as usize;
    let h = height as usize;
    let mut rgba = Vec::with_capacity(w.checked_mul(h).ok_or_else(|| {
        MediaError::Decode("AVI 帧解码失败：像素总量溢出".into())
    })? * 4);
    for y in 0..h {
        // 底朝上 DIB（biHeight > 0，最常见）：文件里第 0 行是图像最后一行
        // （bmp.rs 同一家法）；负 biHeight 的顶朝下变体按原序取行。
        let src_row = if top_down { y } else { h - 1 - y };
        let line = &body[src_row * stride..src_row * stride + w * 3];
        for px in line.chunks_exact(3) {
            // 存储序 BGR -> RGBA；DIB 无 alpha 语义，恒填 255。
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    Ok(DecodedImage { width, height, rgba, frame_count: None })
}

/// `biCompression` 四字码 -> 编解码器枚举（0 与 'DIB ' 同指未压缩 BGR）。
fn classify_codec(compression: u32) -> VideoCodec {
    match compression {
        0x0000_0000 => VideoCodec::Dib,
        c if c == u32::from_le_bytes(*b"DIB ") => VideoCodec::Dib,
        c if c == u32::from_le_bytes(*b"MJPG") => VideoCodec::Mjpg,
        other => VideoCodec::Unsupported(other),
    }
}

fn u32_le(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().expect("u32 固定 4 字节"))
}

fn i32_le(data: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(data[at..at + 4].try_into().expect("i32 固定 4 字节"))
}

fn u16_le(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data[at..at + 2].try_into().expect("u16 固定 2 字节"))
}

// ------------------------------------------------------------ 单元测试
//
// 自验证的最短路径是**写一个测试 muxer**：装配未压缩 DIB 的合法 AVI
// （hdrl/movi/idx1 全量），mux -> demux 往返逐像素对账；MJPG 路径用
// image crate 现场编码 JPEG 字节当帧装配（依赖已在白名单）。夹具手法
// 与 nes-audio/src/wav.rs 的 wav_bytes 同一条家法：仓库不提交二进制
// 资产，测试字面量全 ASCII、注释中文。

#[cfg(test)]
mod tests {
    use super::*;

    /// idx1 条目的 keyframe 旗标（vfw.h AVIIF_KEYFRAME）。
    const AVIIF_KEYFRAME: u32 = 0x0000_0010;

    /// 'ABCD' 四字码的 LE u32 值。
    fn fourcc(s: &[u8; 4]) -> u32 {
        u32::from_le_bytes(*s)
    }

    /// 装配一个块：4 字节块名 + 4 字节 LE 长度 + 块体（奇数长度补 pad）。
    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + body.len() + 1);
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        if body.len() % 2 == 1 {
            out.push(0); // RIFF 规范 pad 字节
        }
        out
    }

    /// 装配一个 LIST：LIST + LE 长度（含 4 字节表类型）+ 表类型 + 子块。
    fn list(form: &[u8; 4], children: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + children.len());
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&((children.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(form);
        out.extend_from_slice(children);
        out
    }

    /// RGBA8 -> 24-bit 底朝上 BGR DIB 块体（行按 4 字节对齐补齐）。
    fn rgba_to_dib_body(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let stride = dib_stride(w);
        let mut body = vec![0u8; stride * h as usize];
        for y in 0..h as usize {
            let dst_row = h as usize - 1 - y; // 底朝上：第 0 行垫底。
            for x in 0..w as usize {
                let s = (y * w as usize + x) * 4;
                let d = dst_row * stride + x * 3;
                body[d] = rgba[s + 2];
                body[d + 1] = rgba[s + 1];
                body[d + 2] = rgba[s];
            }
        }
        body
    }

    /// muxer 夹具的全部输入 + 测试后处理需要的布局锚点。
    struct AviFixture {
        width: u32,
        height: u32,
        /// biCompression 原值（0 / 'MJPG' / 其它）。
        compression: u32,
        /// 视频块类型码（未压缩 'db' / 压缩 'dc'）。
        video_code: [u8; 2],
        /// 视频帧块体（已按编码备好的线格式字节）。
        frame_bodies: Vec<Vec<u8>>,
        fps: u32,
        /// 音轨：Some((声道, 采样率, 位深, PCM 线格式字节))。
        audio_raw: Option<(u16, u32, u16, Vec<u8>)>,
        /// 音频 strf 的 format_tag（默认 1 = PCM；测试拒收面时改）。
        audio_tag: u16,
        /// 是否写 idx1（缺省路径测试用）。
        with_idx1: bool,
    }

    /// 装配产物：字节 + 布局锚点（idx1 偏移改写等测试需要）。
    struct Fixture {
        bytes: Vec<u8>,
        /// 'movi' 四字码的绝对位置（idx1 相对偏移的基准点）。
        movi_fourcc_pos: usize,
        /// idx1 块名（"idx1"）的绝对位置；无 idx1 时 None。
        idx1_pos: Option<usize>,
    }

    /// 全量装配一个合法 AVI 1.0（hdrl / movi / idx1）。
    ///
    /// 布局：RIFF('AVI '){ LIST(hdrl){ avih, LIST(strl){strh,strf}[, 音频 strl] },
    /// LIST(movi){ 帧块…[, 音频块] }, idx1 }。idx1 偏移写"相对 movi 四字码、
    /// 指向块名"的业界多数派形态（绝对基准由测试改写探针覆盖）。
    fn build_avi(f: &AviFixture) -> Fixture {
        // ---- 视频流头 strh（56 字节）----
        let mut v_strh = Vec::with_capacity(56);
        v_strh.extend_from_slice(b"vids");
        v_strh.extend_from_slice(&f.compression.to_le_bytes()); // fccHandler
        v_strh.extend_from_slice(&0u32.to_le_bytes()); // dwFlags
        v_strh.extend_from_slice(&0u16.to_le_bytes()); // wPriority
        v_strh.extend_from_slice(&0u16.to_le_bytes()); // wLanguage
        v_strh.extend_from_slice(&0u32.to_le_bytes()); // dwInitialFrames
        v_strh.extend_from_slice(&1u32.to_le_bytes()); // dwScale
        v_strh.extend_from_slice(&f.fps.to_le_bytes()); // dwRate（fps = rate/scale）
        v_strh.extend_from_slice(&0u32.to_le_bytes()); // dwStart
        v_strh.extend_from_slice(&(f.frame_bodies.len() as u32).to_le_bytes()); // dwLength
        v_strh.extend_from_slice(&0u32.to_le_bytes()); // dwSuggestedBufferSize
        v_strh.extend_from_slice(&u32::MAX.to_le_bytes()); // dwQuality（-1 默认）
        v_strh.extend_from_slice(&0u32.to_le_bytes()); // dwSampleSize
        v_strh.extend_from_slice(&0i16.to_le_bytes()); // rcFrame.left
        v_strh.extend_from_slice(&0i16.to_le_bytes()); // rcFrame.top
        v_strh.extend_from_slice(&(f.width as i16).to_le_bytes()); // rcFrame.right
        v_strh.extend_from_slice(&(f.height as i16).to_le_bytes()); // rcFrame.bottom

        // ---- 视频 strf：BITMAPINFOHEADER（40 字节，正值高 = 底朝上）----
        let stride = dib_stride(f.width);
        let mut v_strf = Vec::with_capacity(40);
        v_strf.extend_from_slice(&40u32.to_le_bytes()); // biSize
        v_strf.extend_from_slice(&(f.width as i32).to_le_bytes()); // biWidth
        v_strf.extend_from_slice(&(f.height as i32).to_le_bytes()); // biHeight
        v_strf.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
        v_strf.extend_from_slice(&24u16.to_le_bytes()); // biBitCount
        v_strf.extend_from_slice(&f.compression.to_le_bytes()); // biCompression
        v_strf.extend_from_slice(&((stride * f.height as usize) as u32).to_le_bytes()); // biSizeImage
        v_strf.extend_from_slice(&0u32.to_le_bytes()); // biXPelsPerMeter
        v_strf.extend_from_slice(&0u32.to_le_bytes()); // biYPelsPerMeter
        v_strf.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
        v_strf.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

        let v_chunk_id: [u8; 4] = [b'0', b'0', f.video_code[0], f.video_code[1]];
        let mut v_strl = chunk(b"strh", &v_strh);
        v_strl.extend_from_slice(&chunk(b"strf", &v_strf));
        let mut hdrl_children = chunk(b"avih", &avih_body(f));
        hdrl_children.extend_from_slice(&list(b"strl", &v_strl));

        // ---- 音频流（可缺；流号 1 = 第二个 strl，块名 '01wb'）----
        let mut movi_tail: Option<([u8; 4], u32, Vec<u8>)> = None; // (块名, idx1 旗标, PCM)
        if let Some((channels, rate, bits, pcm)) = &f.audio_raw {
            let block_align = channels * (bits / 8);
            let mut a_strh = Vec::with_capacity(56);
            a_strh.extend_from_slice(b"auds");
            a_strh.extend_from_slice(&0u32.to_le_bytes()); // fccHandler
            a_strh.extend_from_slice(&0u32.to_le_bytes()); // dwFlags
            a_strh.extend_from_slice(&0u16.to_le_bytes()); // wPriority
            a_strh.extend_from_slice(&0u16.to_le_bytes()); // wLanguage
            a_strh.extend_from_slice(&0u32.to_le_bytes()); // dwInitialFrames
            a_strh.extend_from_slice(&1u32.to_le_bytes()); // dwScale
            a_strh.extend_from_slice(&rate.to_le_bytes()); // dwRate
            a_strh.extend_from_slice(&0u32.to_le_bytes()); // dwStart
            a_strh.extend_from_slice(&((pcm.len() / block_align as usize) as u32).to_le_bytes()); // dwLength
            a_strh.extend_from_slice(&0u32.to_le_bytes()); // dwSuggestedBufferSize
            a_strh.extend_from_slice(&u32::MAX.to_le_bytes()); // dwQuality
            a_strh.extend_from_slice(&u32::from(block_align).to_le_bytes()); // dwSampleSize
            a_strh.extend_from_slice(&[0u8; 8]); // rcFrame（音频流恒 0）
            let mut a_strf = Vec::with_capacity(16);
            a_strf.extend_from_slice(&f.audio_tag.to_le_bytes()); // wFormatTag
            a_strf.extend_from_slice(&channels.to_le_bytes()); // nChannels
            a_strf.extend_from_slice(&rate.to_le_bytes()); // nSamplesPerSec
            a_strf.extend_from_slice(&((*rate as usize * block_align as usize) as u32).to_le_bytes()); // nAvgBytesPerSec
            a_strf.extend_from_slice(&block_align.to_le_bytes()); // nBlockAlign
            a_strf.extend_from_slice(&bits.to_le_bytes()); // wBitsPerSample
            let mut a_strl = chunk(b"strh", &a_strh);
            a_strl.extend_from_slice(&chunk(b"strf", &a_strf));
            hdrl_children.extend_from_slice(&list(b"strl", &a_strl));
            movi_tail = Some((*b"01wb", 0u32, pcm.clone()));
        }

        let hdrl = list(b"hdrl", &hdrl_children);

        // ---- movi：帧块 + 音频块（单块整轨，与定位逻辑无耦合）----
        let movi_fourcc_pos = 12 + hdrl.len() + 8; // RIFF 头 12 + hdrl + LIST 头 8
        let mut movi_children = Vec::new();
        let mut child_off = 4usize; // 子块名从四字码后 4 字节起。
        let mut entries: Vec<([u8; 4], u32, usize, usize)> = Vec::new(); // (ckid, flags, 相对偏移, 体长)
        for body in &f.frame_bodies {
            movi_children.extend_from_slice(&chunk(&v_chunk_id, body));
            entries.push((v_chunk_id, AVIIF_KEYFRAME, child_off, body.len()));
            child_off += 8 + body.len() + (body.len() % 2);
        }
        if let Some((a_chunk_id, a_entry_flag, pcm)) = &movi_tail {
            movi_children.extend_from_slice(&chunk(a_chunk_id, pcm));
            entries.push((*a_chunk_id, *a_entry_flag, child_off, pcm.len()));
        }        let movi = list(b"movi", &movi_children);

        // ---- idx1：偏移 = 块名绝对位置 - movi 四字码绝对位置 ----
        let mut idx1 = Vec::new();
        for (ckid, flags, rel, size) in &entries {
            idx1.extend_from_slice(ckid);
            idx1.extend_from_slice(&flags.to_le_bytes());
            idx1.extend_from_slice(&(*rel as u32).to_le_bytes());
            idx1.extend_from_slice(&(*size as u32).to_le_bytes());
        }
        let idx1_chunk = if f.with_idx1 { chunk(b"idx1", &idx1) } else { Vec::new() };

        let riff_len = 4 + hdrl.len() + movi.len() + idx1_chunk.len();
        let mut bytes = Vec::with_capacity(8 + riff_len);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(riff_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"AVI ");
        bytes.extend_from_slice(&hdrl);
        bytes.extend_from_slice(&movi);
        let idx1_pos = if f.with_idx1 {
            let p = bytes.len();
            bytes.extend_from_slice(&idx1_chunk);
            Some(p)
        } else {
            None
        };

        Fixture { bytes, movi_fourcc_pos, idx1_pos }
    }

    /// avih 主头（56 字节；解析器只做存在性跳过，这里按规范如实填）。
    fn avih_body(f: &AviFixture) -> Vec<u8> {
        let mut avih = Vec::with_capacity(56);
        let micro = if f.fps > 0 { (1_000_000.0 / f.fps as f32) as u32 } else { 0 };
        avih.extend_from_slice(&micro.to_le_bytes()); // dwMicroSecPerFrame
        avih.extend_from_slice(&0u32.to_le_bytes()); // dwMaxBytesPerSec
        avih.extend_from_slice(&0u32.to_le_bytes()); // dwPaddingGranularity
        let flags: u32 = if f.with_idx1 { 0x0000_0010 } else { 0 }; // AVIF_HASINDEX
        avih.extend_from_slice(&flags.to_le_bytes()); // dwFlags
        avih.extend_from_slice(&(f.frame_bodies.len() as u32).to_le_bytes()); // dwTotalFrames
        avih.extend_from_slice(&0u32.to_le_bytes()); // dwInitialFrames
        avih.extend_from_slice(&((f.audio_raw.is_some() as u32) + 1).to_le_bytes()); // dwStreams
        avih.extend_from_slice(&0u32.to_le_bytes()); // dwSuggestedBufferSize
        avih.extend_from_slice(&f.width.to_le_bytes()); // dwWidth
        avih.extend_from_slice(&f.height.to_le_bytes()); // dwHeight
        avih.extend_from_slice(&[0u8; 16]); // dwReserved[4]
        avih
    }

    /// 常用 DIB 夹具：3 帧 3x3 渐变（3*3*3 = 27 字节奇数体长，顺带练 pad），
    /// 单声道 PCM 音轨。
    fn fixture_dib_gradient(audio: Option<Wav>) -> AviFixture {
        let frame = |seed: u8| {
            let mut rgba = Vec::with_capacity(3 * 3 * 4);
            for i in 0..3 * 3 {
                let p = i as u8;
                rgba.extend_from_slice(&[
                    seed.wrapping_add(p.wrapping_mul(7)),
                    seed.wrapping_add(p.wrapping_mul(13)),
                    seed.wrapping_add(p.wrapping_mul(29)),
                    255,
                ]);
            }
            rgba
        };
        let frame_bodies = (0..3u8).map(|s| rgba_to_dib_body(3, 3, &frame(s * 40))).collect();
        AviFixture {
            width: 3,
            height: 3,
            compression: 0,
            video_code: *b"db",
            frame_bodies,
            fps: 12,
            audio_raw: audio.map(|w| (w.channels, w.sample_rate, 16u16, {
                let mut pcm = Vec::with_capacity(w.samples.len() * 2);
                for s in &w.samples {
                    pcm.extend_from_slice(&s.to_le_bytes());
                }
                pcm
            })),
            audio_tag: 1,
            with_idx1: true,
        }
    }

    // ---- 用例 ----

    #[test]
    fn t_avi01_dib_mux_demux_roundtrip_pixels_and_audio() {
        // mux -> demux 往返：元信息、逐帧逐像素、音轨逐样本全部对账。
        let audio = Wav { sample_rate: 22050, channels: 1, samples: vec![10, -20, 30, 0, -32768, 32767] };
        let fx = build_avi(&fixture_dib_gradient(Some(audio.clone())));
        let avi = AviVideo::parse(&fx.bytes).expect("合法 DIB AVI 必须可解析");

        let info = avi.video_info();
        assert_eq!(info.width, 3);
        assert_eq!(info.height, 3);
        assert_eq!(info.fps, 12.0);
        assert_eq!(info.frame_count, 3);
        assert_eq!(info.codec, VideoCodec::Dib);

        for i in 0..3u32 {
            let img = avi.frame(i).expect("每一帧都必须可解码");
            assert_eq!((img.width, img.height), (3, 3));
            assert_eq!(img.frame_count, None, "AVI 单帧无动图帧数语义");
            let s = i as u8 * 40;
            let expected: Vec<u8> = (0..3 * 3u32)
                .flat_map(|p| {
                    let p = p as u8;
                    [
                        s.wrapping_add(p.wrapping_mul(7)),
                        s.wrapping_add(p.wrapping_mul(13)),
                        s.wrapping_add(p.wrapping_mul(29)),
                        255,
                    ]
                })
                .collect();
            assert_eq!(img.rgba, expected, "第 {i} 帧逐像素往返保真");
        }

        assert_eq!(avi.audio(), Some(audio), "音轨逐样本往返保真");
    }

    #[test]
    fn t_avi02_mjpg_frames_decoded_via_image_crate() {
        // MJPG：image crate 现场编码 JPEG 当帧装配；解码面尺寸必须准、
        // 纯色块像素允许量化误差（JPEG 有损，不逐位断言）。
        let frame_jpeg = |v: u8| {
            let img = ::image::RgbaImage::from_raw(8, 8, vec![v; 8 * 8 * 4]).expect("像素缓冲");
            let mut out = std::io::Cursor::new(Vec::new());
            ::image::DynamicImage::ImageRgba8(img)
                .to_rgb8()
                .write_to(&mut out, ::image::ImageFormat::Jpeg)
                .expect("编码 JPEG");
            out.into_inner()
        };
        let fx = build_avi(&AviFixture {
            compression: fourcc(b"MJPG"),
            video_code: *b"dc",
            frame_bodies: vec![frame_jpeg(30), frame_jpeg(220)],
            width: 8,
            height: 8,
            fps: 24,
            audio_raw: None,
            audio_tag: 1,
            with_idx1: true,
        });
        let avi = AviVideo::parse(&fx.bytes).expect("合法 MJPG AVI 必须可解析");
        let info = avi.video_info();
        assert_eq!(info.codec, VideoCodec::Mjpg);
        assert_eq!(info.frame_count, 2);
        assert_eq!(info.fps, 24.0);
        for (i, v) in [30u8, 220].into_iter().enumerate() {
            let img = avi.frame(i as u32).expect("JPEG 帧必须可解码");
            assert_eq!((img.width, img.height), (8, 8), "JPEG 帧尺寸必须准");
            assert!(img.rgba.chunks_exact(4).all(|px| {
                (i16::from(px[0]) - i16::from(v)).abs() <= 12
                    && (i16::from(px[1]) - i16::from(v)).abs() <= 12
                    && (i16::from(px[2]) - i16::from(v)).abs() <= 12
                    && px[3] == 255
            }), "第 {i} 帧纯色块近似保真");
        }
        assert_eq!(avi.audio(), None, "无音轨返回 None");
    }

    #[test]
    fn t_avi03_missing_idx1_scan_fallback_same_pixels() {
        // 无 idx1：movi 顺序扫块兜底，帧序/像素/音轨与 idx1 路径一致。
        let audio = Wav { sample_rate: 8000, channels: 2, samples: vec![100, -100, 200, -200] };
        let fixture = fixture_dib_gradient(Some(audio.clone()));
        let fx = build_avi(&AviFixture { with_idx1: false, ..fixture });
        let avi = AviVideo::parse(&fx.bytes).expect("无 idx1 的 AVI 必须可扫描解析");
        assert_eq!(avi.video_info().frame_count, 3);
        for i in 0..3u32 {
            let img = avi.frame(i).expect("扫描路径每帧可解码");
            let expected_seed = i as u8 * 40;
            assert_eq!(img.rgba[0], expected_seed, "扫描路径帧序正确（首像素即帧种子）");
        }
        assert_eq!(avi.audio(), Some(audio), "扫描路径音轨完整");
    }

    #[test]
    fn t_avi04_absolute_offset_index_probe() {
        // idx1 偏移改写成"文件绝对"流派：探针必须自动切基准，解出同样的帧。
        let fixture = fixture_dib_gradient(None);
        let fx = build_avi(&AviFixture { with_idx1: true, ..fixture });
        let mut bytes = fx.bytes.clone();
        let idx1_at = fx.idx1_pos.expect("有 idx1");
        let n = (u32_le(&bytes, idx1_at + 4)) as usize / 16;
        for i in 0..n {
            let at = idx1_at + 8 + i * 16 + 8; // 偏移字段位置
            let rel = u32_le(&bytes, at);
            bytes[at..at + 4].copy_from_slice(&(rel + fx.movi_fourcc_pos as u32).to_le_bytes());
        }
        let patched = AviVideo::parse(&bytes).expect("绝对偏移索引必须可解析");
        let baseline = AviVideo::parse(&fx.bytes).expect("相对偏移索引必须可解析");
        assert_eq!(patched.video_info(), baseline.video_info());
        for i in 0..3u32 {
            assert_eq!(patched.frame(i).expect("帧"), baseline.frame(i).expect("帧"));
        }
    }

    #[test]
    fn t_avi05_unsupported_video_codec_named() {
        // 未收录四字码：容器照常解析，codec 如实报 Unsupported(原值)，
        // 帧解码给指名错误（四字码可读）。
        let xvid = fourcc(b"XVID");
        let fixture = fixture_dib_gradient(None);
        let fx = build_avi(&AviFixture { compression: xvid, ..fixture });
        let avi = AviVideo::parse(&fx.bytes).expect("容器本身合法，必须可解析");
        assert_eq!(avi.video_info().codec, VideoCodec::Unsupported(xvid));
        assert_eq!(avi.video_info().frame_count, 3, "帧块定位与编解码器无关");
        let err = avi.frame(0).expect_err("解不动的帧必须报错");
        assert!(matches!(err, MediaError::Decode(_)));
        assert!(err.to_string().contains("XVID"), "错误指名四字码：{err}");
    }

    #[test]
    fn t_avi06_non_pcm_audio_track_skipped() {
        // 音频 format_tag = 2（ADPCM）：视频照常可用，音轨跳过返回 None。
        let fixture = fixture_dib_gradient(Some(Wav { sample_rate: 8000, channels: 1, samples: vec![5] }));
        let fx = build_avi(&AviFixture { audio_tag: 2, ..fixture });
        let avi = AviVideo::parse(&fx.bytes).expect("非 PCM 音轨不构成容器级失败");
        assert_eq!(avi.video_info().frame_count, 3, "视频不受音轨影响");
        assert_eq!(avi.audio(), None, "非 PCM 音轨如实跳过");
    }

    #[test]
    fn t_avi07_24bit_audio_takes_top_16_bits() {
        // 24-bit PCM 音轨取高 16 位（与 nes-audio wav.rs 同一口径）。
        // 样本 1：0x12 0x34 0x56 = 0x00563412，符号扩展后 >>8 = 0x5634；
        // 样本 2：0xAA 0xBB 0xCC = 0x00CCBBAA，符号扩展后 >>8 = 0xFFCCBB，
        // 截到 i16 = 0xCCBB（负数高位溢出的坏样本如实截断，不回绕炸音）。
        let pcm: Vec<u8> = [0x12u8, 0x34, 0x56, 0xAA, 0xBB, 0xCC].to_vec();
        let fx = build_avi(&AviFixture {
            audio_raw: Some((2, 44100, 24, pcm)),
            ..fixture_dib_gradient(None)
        });
        let avi = AviVideo::parse(&fx.bytes).expect("24-bit PCM 音轨必须可解析");
        let wav = avi.audio().expect("音轨必须存在");
        assert_eq!(wav.sample_rate, 44100);
        assert_eq!(wav.channels, 2);
        assert_eq!(wav.samples, vec![0x5634i16, -13125]);
    }

    #[test]
    fn t_avi08_rejects_garbage_truncation_and_missing_movi() {
        // 拒绝面：非 RIFF / 非 'AVI ' -> UnsupportedFormat；截断 / 缺 movi -> 指名 Decode。
        assert_eq!(
            AviVideo::parse(b"RIFXxxxxAVI ").expect_err("非 RIFF 必须被拒"),
            MediaError::UnsupportedFormat
        );
        let fixture = fixture_dib_gradient(None);
        let full = build_avi(&AviFixture { with_idx1: true, ..fixture });
        let mut bad_form = full.bytes.clone();
        bad_form[8..12].copy_from_slice(b"WAVE");
        assert_eq!(
            AviVideo::parse(&bad_form).expect_err("非 'AVI ' 必须被拒"),
            MediaError::UnsupportedFormat
        );

        let mut truncated = full.bytes.clone();
        truncated.truncate(truncated.len() - 5); // idx1 块体被裁短
        let err = AviVideo::parse(&truncated).expect_err("截断必须被拒");
        assert!(matches!(err, MediaError::Decode(_)), "截断报指名 Decode：{err:?}");

        // 只留 hdrl：把 movi 起整个裁掉（RIFF 总长字段解析器不信任，改不改不影响）。
        let no_movi = build_avi(&AviFixture { with_idx1: false, ..fixture_dib_gradient(None) });
        let hdrl_end = no_movi.movi_fourcc_pos - 8; // LIST(movi) 块头起点
        let cut = no_movi.bytes[..hdrl_end].to_vec();
        let err = AviVideo::parse(&cut).expect_err("缺 movi 必须被拒");
        assert!(err.to_string().contains("movi"), "错误指名 movi：{err}");
    }

    #[test]
    fn t_avi09_frame_index_out_of_range_named() {
        let fx = build_avi(&fixture_dib_gradient(None));
        let avi = AviVideo::parse(&fx.bytes).expect("合法 AVI 必须可解析");
        let err = avi.frame(3).expect_err("越界帧必须报错");
        assert!(err.to_string().contains("越界") && err.to_string().contains('3'), "错误指名帧号：{err}");
    }
}
