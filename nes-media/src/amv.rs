//! AMV 视频资源管线 —— **S14.3 Adapter 通用性试金石 #2**（手写容器变体 demux + 帧解码）。
//!
//! # 这个模块是什么
//!
//! AMV 是中国山寨 MP4/MTV 播放器的录像格式：**RIFF 容器的坏头变体 +
//! 无头 MJPEG 帧**。用户交付物（蜘蛛糸モノポリー，14179882 字节）在
//! S14.2 的 [`crate::avi`] 路线里只能指名拒绝；本模块让它真解出帧。
//! 它是"Adapter 抽象通用性"的第二块试金石：**容器再怪、帧解码路线再
//! 特殊，引擎面仍是那两个干净 DTO**（[`DecodedImage`] / `nes_audio::Wav`），
//! 第三方类型照旧不出本 crate。
//!
//! # 容器怪癖清单（全部来自真实文件实测，非推测）
//!
//! ```text
//! RIFF [size=0(坏，不信任)] 'AMV '
//!  ├─ LIST [size=0(坏)] 'hdrl'
//!  │   ├─ amvh            主头（56B；AMV 用 'amvh' 顶替 AVI 的 'avih'，布局同 avih）
//!  │   │                  +0  dwMicroSecPerFrame（66667 -> 15 fps）
//!  │   │                  +32 dwWidth（160）  +36 dwHeight（128）—— 全文件唯一尺寸来源
//!  │   ├─ LIST [size=0(坏)] 'strl'   流 0 = 视频（AMV 约定：strl 序数定流，1 视频 2 音频）
//!  │   │   ├─ strh  56B 全零（fccType/fps 全部缺席 —— 实测本文件如此）
//!  │   │   └─ strf  36B 全零（BITMAPINFOHEADER 声称 36 字节且全零 —— 无宽高、无表数据！）
//!  │   └─ LIST [size=0(坏)] 'strl'   流 1 = 音频
//!  │       ├─ strh  48B 全零
//!  │       └─ strf  20B WAVEFORMATEX：声明 PCM/22050Hz/单声道/16-bit（撒谎，见下）
//!  ├─ LIST [size=0(坏)] 'movi'        子块区间不靠声明长度，走到文件尾
//!  │   ├─ '00dc' 视频帧块（326B ~ 数 KB 交替，**块长奇数也绝不补 pad**）
//!  │   ├─ '01wb' 音频块（743B，奇数）
//!  │   └─ 文件尾 8 字节字面量 'AMV_END_'（块遍历在此终止）
//!  └─ （无 idx1 —— AMV 没有索引，帧定位只能顺序扫块）
//! ```
//!
//! 四条铁律（每条都和标准 RIFF 相反，[`crate::avi`] 的家法不能照搬）：
//!
//! 1. **声明长度不可信**：RIFF/hdrl/strl/movi 的 LIST 尺寸字段全为 0 ——
//!    解析只能"按结构走"（块名 + 块自身声明长），LIST 一律下潜；
//! 2. **无 pad**：块体奇数长后**紧跟下一块头**，标准 RIFF 的 +1 对齐
//!    规则不适用（实测 '01wb' 743B 奇数块后直接 '00dc'）；
//! 3. **表数据不在 strf**：视频 strf 全零 —— 量化表/哈夫曼表/宽高
//!    必然外置（见下节），S14.2 探针期"表在 strf 附加数据"的假设被
//!    真实文件证伪；
//! 4. **流号即流序**：strh 全零无法识别流类型，按 strl 序数定（第 1 个
//!    视频流、第 2 个音频流）—— 与 FFmpeg `avidec.c` 对 AMV 的强制
//!    （`tag1 = stream_index ? 'auds' : 'vids'`）同一条裁决。
//!
//! # 帧解码路线（FFmpeg 实证：合成标准 JPEG，帧体原样嵌入）
//!
//! 帧体 = `FFD8` + 裸熵数据 + `FFD9`，中间**没有任何 JPEG 标记段**。
//! FFmpeg 的 AMV 解码器（`libavcodec/sp5xdec.c` 的
//! `ff_sp5x_process_packet`，`AV_CODEC_ID_AMV` 分支）的做法是**把帧体
//! 重新包进一张标准 JPEG**：
//!
//! ```text
//! SOI + DQT(固定两表) + DHT(标准 Annex K 四表) + SOF0(宽高来自容器)
//!      + SOS + 帧体[2..len-2]（剥壳后原样拷贝） + EOI
//! ```
//!
//! 本模块逐字节复刻该方案（常量与 FFmpeg `sp5x.h` 一致，注释标注）：
//!
//! * **DQT**：SP5X/AMV 固定量化表（`sp5x_qscale_five_quant_table`，
//!   SP5X 固件 qscale 表 index 5）—— 亮度表非标准 Annex K，色度表
//!   高频段全 79；
//! * **DHT**：标准 JPEG（ITU T.81 Annex K）四张哈夫曼表
//!   （DC/AC × 亮度/色度）—— FFmpeg 的 `init_default_huffman_tables`
//!   同款标准表；
//! * **SOF0**：8-bit、3 分量、4:2:0（亮度 h=2,v=2）、宽高来自 amvh
//!   （FFmpeg 原样写 `avctx->coded_width/height`，**不是**显示高的
//!   两倍 —— "AMV 帧按半高场编码"的传闻与 FFmpeg 实际代码不符，
//!   本文件逐帧验证也是整帧非场编码）；
//! * **SOS**：三分量交错扫描（Y:表0，Cb/Cr:表1），Ss=0 Se=63；
//! * **输出垂直翻转**：FFmpeg 对 AMV 置 `s->flipped = 1`
//!   （`mjpegdec.c` init）—— 帧体栅序第 0 行是显示图最底行，解出后
//!   翻转（与 BMP 底朝上行序同一家法）。
//!
//! 合成出的标准 JPEG 交 [`crate::image::decode_image`]（`image` 白名单
//! 库）解码 —— **容器手写、表合成手写、熵解码走库**，依赖零新增。
//! 熵数据实测全程规范 stuffing（`FF` -> `FF 00`，全文件 36069 处、0 处
//! 裸 FF），与解码器的标记扫描兼容。
//!
//! # 音频（第 1 期如实跳过）
//!
//! 音频 strf 声明 PCM 22050Hz 单声道 16-bit —— **AMV 头会说谎**：
//! FFmpeg 对 AMV 音频无条件强制 `AV_CODEC_ID_ADPCM_IMA_AMV`（743B 块
//! 尾随 15fps 视频块的形态也与 IMA ADPCM 分块一致）。第 1 期不做
//! ADPCM：[`AmvVideo::audio`] 如实返回 `None`，声明值经
//! [`AmvVideo::audio_declared_format`] 原样暴露供指名报告。
//!
//! # 失败面
//!
//! * 非 RIFF / 非 'AMV '：[`MediaError::UnsupportedFormat`]；
//! * 缺 amvh / 尺寸为 0 / 缺 movi / 无视频帧块 / 帧体缺 SOI-EOI 壳：
//!   指名 [`MediaError::Decode`]；
//! * movi 尾部截断或 'AMV_END_' 尾巴：扫块路径"能救几帧救几帧"，
//!   已定位的帧照常可用（与 [`crate::avi`] 扫描兜底同一裁决）。
//!
//! # 数据持有
//!
//! 与 [`crate::avi::AviVideo`] 同一家法：[`AmvVideo::parse`] 接
//! `&[u8]`，一次拷进 `Arc<[u8]>`；帧惰性逐帧解码；结构体可 Clone。

#![forbid(unsafe_code)]

use std::ops::Range;
use std::sync::Arc;

use nes_audio::Wav;

use crate::image::DecodedImage;
use crate::{MediaError, VideoCodec, VideoInfo};

// ------------------------------------------------------------
// SP5X/AMV 固定 JPEG 表（与 FFmpeg libavcodec/sp5x.h 逐字节一致；
// DHT 四表为 ITU T.81 Annex K 公开标准值）
// ------------------------------------------------------------

/// 亮度量化表（FFmpeg `sp5x_qscale_five_quant_table[0]`，SP5X qscale
/// 表 index 5 —— 非 Annex K 标准表，AMV 专用固定值）。
const QUANT_TABLE_LUMA: [u8; 64] = [
    13, 9, 10, 11, 10, 8, 13, 11, 10, 11, 14, 14, 13, 15, 19, 32, //
    21, 19, 18, 18, 19, 39, 28, 30, 23, 32, 46, 41, 49, 48, 46, 41, //
    45, 44, 51, 58, 74, 62, 51, 54, 70, 55, 44, 45, 64, 87, 65, 70, //
    76, 78, 82, 83, 82, 50, 62, 90, 97, 90, 80, 96, 74, 81, 82, 79,
];

/// 色度量化表（FFmpeg `sp5x_qscale_five_quant_table[1]`；高频段全 79）。
const QUANT_TABLE_CHROMA: [u8; 64] = [
    14, 14, 14, 19, 17, 19, 38, 21, 21, 38, 79, 53, 45, 53, 79, 79, //
    79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, //
    79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, //
    79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79, 79,
];

/// 标准 DHT 段（FF `C4` + 长度 + 四张 Annex K 标准哈夫曼表：
/// DC 亮度 / DC 色度 / AC 亮度 / AC 色度）。
const STANDARD_DHT_SEGMENT: [u8; 420] = [
    0xFF, 0xC4, 0x01, 0xA2, //
    // DC 亮度（class 0, id 0）
    0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, //
    // DC 色度（class 0, id 1）
    0x01, 0x00, 0x03, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, //
    // AC 亮度（class 1, id 0）
    0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00,
    0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13,
    0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1,
    0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19,
    0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43,
    0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A,
    0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
    0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
    0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4,
    0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA,
    0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6,
    0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, //
    // AC 色度（class 1, id 1）
    0x11, 0x00, 0x02, 0x01, 0x02, 0x04, 0x04, 0x03, 0x04, 0x07, 0x05, 0x04, 0x04, 0x00, 0x01,
    0x02, 0x77, 0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51,
    0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09,
    0x23, 0x33, 0x52, 0xF0, 0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1,
    0x17, 0x18, 0x19, 0x1A, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A,
    0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78,
    0x79, 0x7A, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95,
    0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2,
    0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8,
    0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE2, 0xE3, 0xE4, 0xE5,
    0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA,
];

// ------------------------------------------------------------
// 公开 DTO 与解析产物
// ------------------------------------------------------------

/// 一个解析完成的 AMV：视频流元信息 + 惰性逐帧解码。
///
/// 数据持有照 [`crate::avi::AviVideo`] 先例：`parse` 时整份字节拷进
/// `Arc<[u8]>`，结构体拥有数据、可 Clone、跨线程共享。
#[derive(Debug, Clone)]
pub struct AmvVideo {
    /// 整份文件字节（帧块体按区间从这切）。
    data: Arc<[u8]>,
    /// 视频流元信息（解析期定死；codec 恒 [`VideoCodec::Amv`]）。
    info: VideoInfo,
    /// 每个视频帧块体的绝对字节区间（movi 内顺序即帧序）。
    frames: Vec<Range<usize>>,
    /// 每个 'NNwb' 音频块体的绝对字节区间。第 1 期仅定位不解码
    /// （IMA ADPCM 是后续期）—— 留位字段，暂无读取方。
    #[allow(dead_code)]
    audio_chunks: Vec<Range<usize>>,
    /// 音频 strf（WAVEFORMATEX）的声明值 —— AMV 头会说谎，仅供参考。
    audio_declared: Option<(u16, u16, u32, u16)>,
    /// 亮度采样因子 (h, v)，合成 SOF0 用。真实 AMV 恒 4:2:0（2,2）——
    /// 容器不声明采样，与 FFmpeg 硬编码 `sp5x_data_sof` 的 0x22 同一
    /// 裁决；测试夹具（image 库编码为 4:4:4）经同模块测试改写为 (1,1)
    /// 以保持 SOF 与熵数据 MCU 结构一致。
    luma_sampling: (u8, u8),
}

impl AmvVideo {
    /// 从内存字节解析一个 AMV（只认字节流，不做文件 IO）。
    ///
    /// 解析策略见模块文档"容器怪癖清单"：不信任任何 LIST 声明长度
    /// （全按结构走）、无 pad、流号按 strl 序数。视频流必须存在且至少
    /// 定位到一帧，amvh 尺寸字段必须非零（FFmpeg 同款要求）。
    pub fn parse(data: &[u8]) -> Result<Self, MediaError> {
        if data.len() < 24 || &data[0..4] != b"RIFF" || &data[8..12] != b"AMV " {
            return Err(MediaError::UnsupportedFormat);
        }

        // ---- hdrl 区平面走查：amvh 主头 + 两个 strl 的 strf ----
        // strh 全零没有信息量；流序 = strl 序数（1 视频 2 音频）。
        let mut us_per_frame = 0u32;
        let mut width = 0u32;
        let mut height = 0u32;
        let mut audio_declared: Option<(u16, u16, u32, u16)> = None;
        let mut strf_count = 0usize;
        let mut movi_children: Option<Range<usize>> = None;

        let mut pos = 24usize; // RIFF(12) + LIST 头(8) + 'hdrl'(4)
        while pos + 8 <= data.len() {
            let id: [u8; 4] = data[pos..pos + 4].try_into().expect("块名固定 4 字节");
            let size =
                u32::from_le_bytes(data[pos + 4..pos + 8].try_into().expect("长度固定 4 字节"))
                    as usize;
            let body = pos + 8;
            if body + size > data.len() {
                break; // hdrl 声明长度越界（坏头家法）：停止走查，靠 movi 扫描兜底。
            }
            match &id {
                b"amvh" if size >= 40 => {
                    us_per_frame = u32_le(data, body);
                    width = u32_le(data, body + 32);
                    height = u32_le(data, body + 36);
                }
                b"LIST" if body + 4 <= data.len() => {
                    // 声明尺寸不看（AMV 全为 0），表类型照常在头后 4 字节。
                    let form: [u8; 4] = data[body..body + 4].try_into().expect("表类型固定 4 字节");
                    match &form {
                        // movi：子块区间不靠声明长度，直接走到文件尾。
                        b"movi" => {
                            movi_children = Some(body + 4..data.len());
                            break;
                        }
                        // strl：声明尺寸不可信，下潜继续平面走查（strh/strf 随后）。
                        b"strl" => {
                            pos = body + 4;
                            continue;
                        }
                        _ => {} // 其它列表：按声明长度跳过
                    }
                }
                // strl 序数定流（1=视频 2=音频）：第 1 个 strf 是视频 strf
                // （本格式全零，无信息量，跳过），第 2 个是音频 WAVEFORMATEX。
                b"strf" => {
                    strf_count += 1;
                    if strf_count == 2 && size >= 16 {
                        audio_declared = Some((
                            u16_le(data, body),      // wFormatTag（AMV 声明 PCM=1）
                            u16_le(data, body + 2),  // nChannels
                            u32_le(data, body + 4),  // nSamplesPerSec
                            u16_le(data, body + 14), // wBitsPerSample
                        ));
                    }
                }
                _ => {} // strh（全零）/ strd / 未知块：按块头跳过
            }
            pos = body + size;
            // AMV 家法：奇数体长**不补 pad**（与标准 RIFF 相反，实测铁律）。
        }

        let movi = match movi_children {
            Some(range) => range,
            // hdrl 走查失败（坏头把 walk 带歪）时兜底：字面扫描 'movi'
            // 四字码（要求紧跟 LIST 头之后），第一个流块形态校验通过才算。
            None => find_movi_by_scan(data).ok_or_else(|| {
                MediaError::Decode("AMV 解析失败：未找到 movi 列表（无媒体数据）".into())
            })?,
        };

        if width == 0 || height == 0 {
            return Err(MediaError::Decode(format!(
                "AMV 解析失败：amvh 尺寸字段非法（{width}x{height}，无法确定帧尺寸）"
            )));
        }
        if width > u16::MAX as u32 || height > u16::MAX as u32 {
            return Err(MediaError::Decode(format!(
                "AMV 解析失败：amvh 尺寸越界（{width}x{height}，JPEG 段上限 65535）"
            )));
        }

        // ---- movi 扫块（无 pad 家法）：收集 '00dc'/'00db' 帧块 ----
        let mut frames = Vec::new();
        let mut audio_chunks = Vec::new();
        let mut pos = movi.start;
        while pos + 8 <= movi.end {
            let id: [u8; 4] = data[pos..pos + 4].try_into().expect("块名固定 4 字节");
            let size =
                u32::from_le_bytes(data[pos + 4..pos + 8].try_into().expect("长度固定 4 字节"))
                    as usize;
            let body = pos + 8;
            if body + size > movi.end {
                break; // 尾块被截断（或撞上 'AMV_END_' 尾巴）：能救几帧救几帧。
            }
            match parse_stream_ckid(&id) {
                Some((stream, code)) => {
                    let range = body..body + size;
                    if stream == 0 && (code == *b"dc" || code == *b"db") {
                        frames.push(range);
                    } else if code == *b"wb" {
                        audio_chunks.push(range);
                    }
                }
                // 'AMV_END_' 等非流块：movi 走到头了。
                None => break,
            }
            pos = body + size; // 奇数体长不补 pad（同上，实测铁律）。
        }
        if frames.is_empty() {
            return Err(MediaError::Decode(
                "AMV 解析失败：movi 中未定位到任何视频帧块（'00dc'）".into(),
            ));
        }

        let fps = if us_per_frame > 0 {
            1_000_000.0 / us_per_frame as f32
        } else {
            0.0
        };
        let data: Arc<[u8]> = Arc::from(data.to_vec().into_boxed_slice());
        Ok(AmvVideo {
            data,
            info: VideoInfo {
                width,
                height,
                fps,
                frame_count: frames.len() as u32,
                codec: VideoCodec::Amv,
            },
            frames,
            audio_chunks,
            audio_declared,
            luma_sampling: (2, 2), // 真实 AMV 恒 4:2:0（FFmpeg sp5x_data_sof 同款硬编码）
        })
    }

    /// 视频流元信息（宽高 / 帧率 / 帧数；codec 恒 [`VideoCodec::Amv`]）。
    pub fn video_info(&self) -> VideoInfo {
        self.info.clone()
    }

    /// 解码第 `i` 帧（0 起）为 RGBA8（复用图像面 [`DecodedImage`] DTO）。
    ///
    /// 路线（FFmpeg `sp5xdec.c` 实证同款）：剥掉帧体的 `FFD8`/`FFD9`
    /// 壳，把裸熵数据原样嵌进合成标准 JPEG（固定 DQT/DHT + 容器宽高
    /// 的 SOF0 + SOS），交 `image` 库解码，最后按 AMV 约定垂直翻转
    /// （FFmpeg `s->flipped = 1`）。
    pub fn frame(&self, i: u32) -> Result<DecodedImage, MediaError> {
        let idx = i as usize;
        let body = match self.frames.get(idx) {
            Some(range) => self
                .data
                .get(range.clone())
                .ok_or_else(|| MediaError::Decode("AMV 帧解码失败：帧块体区间越界".into()))?,
            None => {
                return Err(MediaError::Decode(format!(
                    "AMV 帧解码失败：帧序号 {i} 越界（共 {} 帧）",
                    self.frames.len()
                )));
            }
        };
        if body.len() < 4 || body[0..2] != [0xFF, 0xD8] || body[body.len() - 2..] != [0xFF, 0xD9] {
            return Err(MediaError::Decode(format!(
                "AMV 帧解码失败：第 {i} 帧缺少 FFD8/FFD9 壳（帧体损坏）"
            )));
        }
        let jpeg = synthesize_jpeg(
            body,
            self.info.width,
            self.info.height,
            self.luma_sampling.0,
            self.luma_sampling.1,
        );
        let mut img = crate::image::decode_image(&jpeg)?;
        flip_vertical(&mut img);
        Ok(img)
    }

    /// 音轨（第 1 期恒 `None`）。
    ///
    /// AMV 音频实际载荷是 IMA ADPCM（FFmpeg 无条件强制
    /// `AV_CODEC_ID_ADPCM_IMA_AMV`），第 1 期不做 ADPCM 解码 —— 音轨
    /// 如实缺席、视频照常可用。声明值见 [`Self::audio_declared_format`]。
    pub fn audio(&self) -> Option<Wav> {
        None
    }

    /// 音频 strf 的声明值 `(format_tag, channels, sample_rate, bits)`。
    ///
    /// **AMV 头会说谎**：本样本声明 PCM/单声道/22050Hz/16-bit，实际
    /// 是 IMA ADPCM —— 此处只做如实暴露（探针指名报告用），不保证
    /// 与真实载荷一致。
    pub fn audio_declared_format(&self) -> Option<(u16, u16, u32, u16)> {
        self.audio_declared
    }
}

// ------------------------------------------------------------
// JPEG 合成（FFmpeg sp5xdec.c 的逐字节复刻）
// ------------------------------------------------------------

/// 把 AMV 帧体（FFD8 + 熵数据 + FFD9）重新包进一张标准 JPEG。
///
/// 段序与常量同 FFmpeg `ff_sp5x_process_packet`（`AV_CODEC_ID_AMV`
/// 分支）：SOI + DQT(固定两表) + DHT(Annex K 四表) + SOF0(容器宽高，
/// 亮度采样因子可参) + SOS + 帧体剥壳原样拷贝（不做 FF00 处理 ——
/// AMV 熵数据已按规范 stuffing）+ EOI。
fn synthesize_jpeg(body: &[u8], width: u32, height: u32, luma_h: u8, luma_v: u8) -> Vec<u8> {
    let entropy = &body[2..body.len() - 2];
    // 头部总量：SOI(2) + DQT(134) + DHT(420) + SOF(19) + SOS(14) = 589。
    let mut out = Vec::with_capacity(589 + entropy.len());
    out.extend_from_slice(&[0xFF, 0xD8]); // SOI
    // DQT：两张 8-bit 表（Pq/Tq=0 与 1）。
    out.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x84, 0x00]);
    out.extend_from_slice(&QUANT_TABLE_LUMA);
    out.push(0x01);
    out.extend_from_slice(&QUANT_TABLE_CHROMA);
    out.extend_from_slice(&STANDARD_DHT_SEGMENT);
    // SOF0：8-bit 基线，3 分量 4:2:0（亮度 h/v，色度 1/1），量化槽 0/1/1。
    out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
    out.extend_from_slice(&(height as u16).to_be_bytes());
    out.extend_from_slice(&(width as u16).to_be_bytes());
    out.push(0x03);
    out.extend_from_slice(&[0x01, (luma_h << 4) | luma_v, 0x00]);
    out.extend_from_slice(&[0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    // SOS：三分量交错扫描（Y:DC0/AC0，Cb/Cr:DC1/AC1），Ss=0 Se=63 Ah/Al=0。
    out.extend_from_slice(&[
        0xFF, 0xDA, 0x00, 0x0C, 0x03, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3F, 0x00,
    ]);
    out.extend_from_slice(entropy);
    out.extend_from_slice(&[0xFF, 0xD9]); // EOI
    out
}

/// RGBA8 垂直翻转（FFmpeg 对 AMV 置 `flipped = 1` 的等价操作：
/// 帧体栅序第 0 行是显示图最底行，与 BMP 底朝上行序同一家法）。
fn flip_vertical(img: &mut DecodedImage) {
    let stride = img.width as usize * 4;
    let h = img.height as usize;
    for y in 0..h / 2 {
        let a = y * stride;
        let b = (h - 1 - y) * stride;
        for x in 0..stride {
            img.rgba.swap(a + x, b + x);
        }
    }
}

// ------------------------------------------------------------
// 容器辅助
// ------------------------------------------------------------

/// 解析两位流号块名：`b"00dc"` -> `(0, b"dc")`。
fn parse_stream_ckid(ckid: &[u8; 4]) -> Option<(usize, [u8; 2])> {
    let d1 = (ckid[0] as char).to_digit(10)?;
    let d2 = (ckid[1] as char).to_digit(10)?;
    Some(((d1 * 10 + d2) as usize, [ckid[2], ckid[3]]))
}

/// 兜底：字面扫描 'movi' 四字码（要求紧跟 LIST 头之后，且首个子块
/// 形如流块）。hdrl 平面走查被坏头带歪时的第二路径。
fn find_movi_by_scan(data: &[u8]) -> Option<Range<usize>> {
    if data.len() < 16 {
        return None;
    }
    for pos in 12..data.len() - 8 {
        if &data[pos..pos + 4] == b"movi" && pos >= 8 && &data[pos - 8..pos - 4] == b"LIST" {
            let children = pos + 4..data.len();
            let head_ok = data
                .get(children.start..children.start + 8)
                .and_then(|head| {
                    let size =
                        u32::from_le_bytes(head[4..8].try_into().expect("长度固定 4 字节")) as usize;
                    parse_stream_ckid(
                        &head[0..4].try_into().expect("块名固定 4 字节"),
                    )
                    .map(|_| head.len() == 8 && children.start + 8 + size <= children.end)
                })
                .unwrap_or(false);
            if head_ok {
                return Some(children);
            }
        }
    }
    None
}

fn u32_le(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().expect("u32 固定 4 字节"))
}

fn u16_le(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data[at..at + 2].try_into().expect("u16 固定 2 字节"))
}

// ------------------------------------------------------------ 单元测试
//
// 家法与 avi.rs 相同：仓库不提交二进制资产，测试字面量全 ASCII。
// 帧体两条来源（互为独立证据）：
// * 手工 4:2:0 熵流（按 ITU T.81 Annex K 标准表逐位算出）—— 采样因子
//   与生产路径一致，验证"合成 + 解码"逐像素精确；
// * image 库现场编码的标准 JPEG 拆头（编码器是 4:4:4）—— 独立第三方
//   熵编码交叉验证合成段，采样因子经测试改写对齐夹具。
// 真实交付物的契约测试在本模块尾部（skip-if-missing，字体用例惯例）。

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实交付物（蜘蛛糸モノポリー，14179882 字节；ASCII 名副本）。
    const REAL_AMV: &str = "C:/Users/Administrator/Videos/text/spider_amv.amv";

    // ---- AMV 容器夹具（坏头形态逐项复刻真实文件）----

    /// 装配一个块：4 字节块名 + 4 字节 LE 长度 + 块体（**无 pad** —— AMV 家法）。
    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + body.len());
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    /// LIST 头：声明尺寸写 0（真实 AMV 的坏头形态）+ 表类型。
    fn broken_list(form: &[u8; 4]) -> Vec<u8> {
        let mut out = Vec::with_capacity(12);
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(form);
        out
    }

    /// amvh 主头（56B，布局同 avih：+0 微秒每帧 / +32 宽 / +36 高）。
    fn amvh(us_per_frame: u32, width: u32, height: u32) -> Vec<u8> {
        let mut out = vec![0u8; 56];
        out[0..4].copy_from_slice(&us_per_frame.to_le_bytes());
        out[32..36].copy_from_slice(&width.to_le_bytes());
        out[36..40].copy_from_slice(&height.to_le_bytes());
        out
    }

    /// 音频 strf：WAVEFORMATEX（20B；声明 PCM —— AMV 头的谎，真实载荷
    /// 是 IMA ADPCM，见模块文档）。
    fn audio_strf(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let mut out = vec![0u8; 20];
        out[0..2].copy_from_slice(&tag.to_le_bytes());
        out[2..4].copy_from_slice(&channels.to_le_bytes());
        out[4..8].copy_from_slice(&rate.to_le_bytes());
        out[14..16].copy_from_slice(&bits.to_le_bytes());
        out
    }

    /// 夹具输入（坏头 + 奇块 + 尾巴三怪齐上的开关面）。
    struct AmvFixture {
        width: u32,
        height: u32,
        us_per_frame: u32,
        /// 视频帧块体（'00dc'，与音频块 1:1 交替，真实文件形态）。
        frame_bodies: Vec<Vec<u8>>,
        /// 音频块体长（真实文件 743，奇数）。
        audio_chunk_size: usize,
        /// 音频 strf 的 wFormatTag（真实文件撒谎写 PCM=1）。
        audio_tag: u16,
        /// 是否追加 'AMV_END_' 字面量尾巴。
        with_trailer: bool,
    }

    /// 全量装配一个坏头 AMV：
    /// RIFF(尺寸 0)、LIST hdrl(尺寸 0){amvh, LIST strl(尺寸 0){strh,strf} x2}、
    /// LIST movi(尺寸 0){'00dc'/'01wb' 交替}，可选 'AMV_END_' 尾巴。
    /// strh/视频 strf 全零（真实文件形态）；块体奇数长不补 pad。
    fn build_amv(f: &AmvFixture) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes()); // 顶层尺寸同样坏
        bytes.extend_from_slice(b"AMV ");
        bytes.extend_from_slice(&broken_list(b"hdrl"));
        bytes.extend_from_slice(&chunk(b"amvh", &amvh(f.us_per_frame, f.width, f.height)));
        // 视频流 strl（流 0）：strh 56B + strf 36B 全零。
        bytes.extend_from_slice(&broken_list(b"strl"));
        bytes.extend_from_slice(&chunk(b"strh", &[0u8; 56]));
        bytes.extend_from_slice(&chunk(b"strf", &[0u8; 36]));
        // 音频流 strl（流 1）：strh 48B 全零 + strf 20B WAVEFORMATEX。
        bytes.extend_from_slice(&broken_list(b"strl"));
        bytes.extend_from_slice(&chunk(b"strh", &[0u8; 48]));
        bytes.extend_from_slice(&chunk(b"strf", &audio_strf(f.audio_tag, 1, 22050, 16)));
        // movi：帧块 + 音频块交替。
        bytes.extend_from_slice(&broken_list(b"movi"));
        let audio = vec![0xA5u8; f.audio_chunk_size];
        for body in &f.frame_bodies {
            bytes.extend_from_slice(&chunk(b"00dc", body));
            bytes.extend_from_slice(&chunk(b"01wb", &audio));
        }
        if f.with_trailer {
            bytes.extend_from_slice(b"AMV_END_");
        }
        bytes
    }

    /// 手工 4:2:0 熵流：纯灰 128 帧。
    ///
    /// level-shift 后全零块 -> 每块 DC diff=0（亮度 DC 表 cat0 = "00"），
    /// 加 EOB 码（亮度 AC 表首码 "1010"）；色度 DC cat0 与 AC EOB 均为
    /// "00"。一个 MCU（4 亮度块加 Cb、Cr）= 32 bit =
    /// [0x28, 0xA2, 0x8A, 0x00]，天然字节对齐、无 FF 字节。
    fn gray420_body(mcus: usize) -> Vec<u8> {
        let mut body = vec![0xFF, 0xD8];
        for _ in 0..mcus {
            body.extend_from_slice(&[0x28, 0xA2, 0x8A, 0x00]);
        }
        body.extend_from_slice(&[0xFF, 0xD9]);
        body
    }

    /// 用 image 库现场编码一张"上半 top 下半 bottom"的 JPEG 并拆头：
    /// 返回 (亮度采样 h, v, 熵数据)。采样从编码器自己写的 SOF0 读出
    /// —— 不信文档注释信字节（image 0.25 实为 4:4:4）。
    fn encode_and_split(w: u32, h: u32, top_rgb: [u8; 3], bottom_rgb: [u8; 3]) -> (u8, u8, Vec<u8>) {
        let mut px = Vec::with_capacity((w * h) as usize * 4);
        let half = (h / 2) as usize;
        for y in 0..h as usize {
            let c = if y < half { top_rgb } else { bottom_rgb };
            for _ in 0..w {
                px.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        let img = ::image::RgbaImage::from_raw(w, h, px).expect("像素缓冲");
        let mut out = std::io::Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgba8(img)
            .to_rgb8()
            .write_to(&mut out, ::image::ImageFormat::Jpeg)
            .expect("编码 JPEG");
        let jpeg = out.into_inner();
        // 逐标记走到 SOS 末尾：其后即熵数据，直到结尾 FFD9。
        let mut pos = 2usize;
        let mut luma_hv = (1u8, 1u8);
        loop {
            assert_eq!(jpeg[pos], 0xFF, "标记段以 FF 开头");
            let marker = jpeg[pos + 1];
            pos += 2;
            let seg_len = || u16::from_be_bytes([jpeg[pos], jpeg[pos + 1]]) as usize;
            match marker {
                0xC0 => {
                    // SOF0：段内 +2 精度、+3..5 高、+5..7 宽、+7 分量数、
                    // +8 起 [id, hv, q]（亮度分量第一）→ hv 在 +9。
                    luma_hv = (jpeg[pos + 9] >> 4, jpeg[pos + 9] & 0x0F);
                    pos += seg_len();
                }
                0xDA => {
                    let entropy_start = pos + seg_len();
                    let entropy_end = jpeg.len() - 2; // 去掉结尾 FFD9
                    assert_eq!(&jpeg[entropy_end..], &[0xFF, 0xD9], "编码 JPEG 以 EOI 收尾");
                    return (luma_hv.0, luma_hv.1, jpeg[entropy_start..entropy_end].to_vec());
                }
                _ => pos += seg_len(), // APP0/DQT/DHT 等均带长度字段
            }
        }
    }

    /// RGBA8 -> 24-bit BMP 落盘（avi_probe 同款镜像，肉眼取证用）。
    fn write_bmp(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
        let stride = (w as usize * 3).div_ceil(4) * 4;
        let pixels = stride * h as usize;
        let mut out = Vec::with_capacity(54 + pixels);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((54 + pixels) as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(w as i32).to_le_bytes());
        out.extend_from_slice(&(h as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(pixels as u32).to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        let mut body = vec![0u8; pixels];
        for y in 0..h as usize {
            let dst_row = h as usize - 1 - y;
            for x in 0..w as usize {
                let s = (y * w as usize + x) * 4;
                let d = dst_row * stride + x * 3;
                body[d] = rgba[s + 2];
                body[d + 1] = rgba[s + 1];
                body[d + 2] = rgba[s];
            }
        }
        out.extend_from_slice(&body);
        std::fs::write(path, &out)
    }

    /// 夹具缺省值（32x16、15fps、743B 奇数音频块、带尾巴）。
    fn fixture(bodies: Vec<Vec<u8>>) -> AmvFixture {
        AmvFixture {
            width: 32,
            height: 16,
            us_per_frame: 66667,
            frame_bodies: bodies,
            audio_chunk_size: 743,
            audio_tag: 1,
            with_trailer: true,
        }
    }

    // ---- 用例 ----

    #[test]
    fn t_amv01_handcrafted_gray420_exact_pixels() {
        // 手工 4:2:0 熵流（32x16 = 2 MCU）走生产采样因子全管线：纯灰 128
        // level-shift 后全零 -> DC=0 且色度恒 128，与量化表无关 —— 解码
        // 必须逐像素精确 (128,128,128,255)。顺带全量断言元信息。
        let bytes = build_amv(&fixture(vec![gray420_body(2), gray420_body(2)]));
        let amv = AmvVideo::parse(&bytes).expect("合法 AMV 必须可解析");
        let info = amv.video_info();
        assert_eq!((info.width, info.height), (32, 16));
        assert_eq!(info.fps, 1_000_000.0 / 66667.0, "66667us/帧 -> 15fps");
        assert_eq!(info.frame_count, 2);
        assert_eq!(info.codec, VideoCodec::Amv);
        for i in 0..2u32 {
            let img = amv.frame(i).expect("帧必须可解码");
            assert_eq!((img.width, img.height), (32, 16));
            assert!(
                img.rgba.chunks_exact(4).all(|px| px == [128, 128, 128, 255]),
                "第 {i} 帧必须逐像素精确纯灰 128"
            );
        }
        assert_eq!(amv.audio(), None, "第 1 期音轨如实缺席");
        assert_eq!(amv.audio_declared_format(), Some((1, 1, 22050, 16)), "声明值如实暴露");
    }

    #[test]
    fn t_amv02_encoder_crosscheck_and_flip_direction() {
        // image 库（4:4:4、标准 Annex K 哈夫曼）交叉验证：上半红、下半蓝。
        // AMV 栅序 = 编码器栅序，而 AMV 帧体第 0 行是"显示最底行"
        // （FFmpeg flipped=1）-> 解码+翻转后显示上半应为蓝、下半为红
        // —— 翻转的存在与方向一并钉死。
        let (lh, lv, entropy) = encode_and_split(32, 32, [220, 60, 50], [50, 60, 220]);
        let mut body = vec![0xFF, 0xD8];
        body.extend_from_slice(&entropy);
        body.extend_from_slice(&[0xFF, 0xD9]);
        let mut fx = fixture(vec![body]);
        fx.width = 32; // amvh 尺寸必须与熵流一致（合成 SOF 从容器读尺寸）
        fx.height = 32;
        let mut amv = AmvVideo::parse(&build_amv(&fx)).expect("合法 AMV 必须可解析");
        amv.luma_sampling = (lh, lv); // 夹具编码 4:4:4：SOF 与熵流 MCU 结构对齐
        let img = amv.frame(0).expect("帧必须可解码");
        assert_eq!((img.width, img.height), (32, 32));
        let row_mean = |y: usize, ch: usize| -> i32 {
            let base = y * 32 * 4;
            img.rgba[base..base + 32 * 4]
                .iter()
                .skip(ch)
                .step_by(4)
                .map(|&v| i32::from(v))
                .sum::<i32>()
                / 32
        };
        let (top_r, top_b) = (row_mean(4, 0), row_mean(4, 2));
        let (bot_r, bot_b) = (row_mean(27, 0), row_mean(27, 2));
        assert!(top_b > top_r + 40, "显示上半应为蓝：R={top_r} B={top_b}");
        assert!(bot_r > bot_b + 40, "显示下半应为红：R={bot_r} B={bot_b}");
    }

    #[test]
    fn t_amv03_no_pad_odd_blocks_trailer_and_zero_declared_sizes() {
        // 三怪齐上：奇数块体（13B 帧体 / 743B 音频）后不补 pad、LIST 声明
        // 尺寸全 0、尾部 'AMV_END_' 字面量 —— 帧数与帧序必须全对。
        let even = gray420_body(2);
        let mut odd = vec![0xFF, 0xD8];
        odd.extend_from_slice(&[0u8; 9]); // 13 字节奇数帧体（熵流不可解，仅测定位）
        odd.extend_from_slice(&[0xFF, 0xD9]);
        let bytes = build_amv(&fixture(vec![even.clone(), odd.clone(), even, odd]));
        let amv = AmvVideo::parse(&bytes).expect("无 pad 奇块必须可解析");
        assert_eq!(amv.video_info().frame_count, 4, "奇数块长 + 尾巴不得吞帧");
        assert_eq!(amv.audio_declared_format(), Some((1, 1, 22050, 16)));
        // 可解帧在翻转后的顺序核对：第 0/2 帧仍是精确纯灰（若解析错位，
        // 帧体会挪到别的块上，解码立即失真）。
        for i in [0u32, 2] {
            let img = amv.frame(i).expect("可解帧必须可解码");
            assert!(img.rgba.chunks_exact(4).all(|px| px == [128, 128, 128, 255]));
        }
    }

    #[test]
    fn t_amv04_rejects_non_amv_and_broken_headers_named() {
        // 非 RIFF / 非 'AMV ' -> UnsupportedFormat。
        assert_eq!(
            AmvVideo::parse(b"RIFF\x12\x00\x00\x00AVI xxx").expect_err("非 AMV 必须被拒"),
            MediaError::UnsupportedFormat
        );
        assert_eq!(
            AmvVideo::parse(b"RIFXxxxxAVI ").expect_err("非 RIFF 必须被拒"),
            MediaError::UnsupportedFormat
        );

        // 缺 movi（hdrl 后直接截断）-> 指名 Decode。
        let base = build_amv(&fixture(vec![gray420_body(2)]));
        let movi_at = base.windows(4).position(|w| w == b"movi").expect("夹具有 movi");
        let cut = &base[..movi_at - 8];
        let err = AmvVideo::parse(cut).expect_err("缺 movi 必须被拒");
        assert!(err.to_string().contains("movi"), "错误指名 movi：{err}");

        // amvh 尺寸为 0 -> 指名 Decode（FFmpeg 同款要求：无尺寸不解帧）。
        let mut zero_dims = build_amv(&fixture(vec![gray420_body(2)]));
        let at = 24 + 8 + 32; // hdrl LIST 头 + amvh 块头 + 块体 +32 = 宽字段
        zero_dims[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        let err = AmvVideo::parse(&zero_dims).expect_err("零尺寸必须被拒");
        assert!(err.to_string().contains("尺寸"), "错误指名尺寸：{err}");

        // 帧序号越界 -> 指名。
        let amv = AmvVideo::parse(&base).expect("合法 AMV 必须可解析");
        let err = amv.frame(9).expect_err("越界帧必须报错");
        assert!(err.to_string().contains("越界") && err.to_string().contains('9'), "{err}");

        // 帧体缺 FFD8/FFD9 壳 -> 指名 Decode。
        let mut broken = build_amv(&fixture(vec![gray420_body(2)]));
        let first_frame = broken.windows(4).position(|w| w == b"00dc").expect("有帧块") + 8;
        broken[first_frame] = 0x00; // 破坏 SOI
        let amv = AmvVideo::parse(&broken).expect("容器照常解析");
        let err = amv.frame(0).expect_err("坏壳帧必须报错");
        assert!(err.to_string().contains("FFD8"), "错误指名壳：{err}");
    }

    #[test]
    fn t_amv05_real_file_decodes_to_colored_frames() {
        // 真实交付物端到端（skip-if-missing）：容器事实 + 逐帧解码落
        // BMP（tmpdir，不入仓库）+ 非零覆盖与彩色度断言。
        let Ok(bytes) = std::fs::read(REAL_AMV) else {
            eprintln!("[skip] real amv sample not found (user video dir absent)");
            return;
        };
        let started = std::time::Instant::now();
        let amv = AmvVideo::parse(&bytes).expect("真实 AMV 必须可解析");
        let parse_ms = started.elapsed().as_millis();
        let info = amv.video_info();
        // 容器探针期钉死的确定性事实。
        assert_eq!((info.width, info.height), (160, 128), "amvh 尺寸 160x128");
        assert!((info.fps - 15.0).abs() < 0.01, "66667us/帧 -> 15fps（实际 {}）", info.fps);
        assert_eq!(info.frame_count, 4233, "movi 扫块实测 4233 帧");
        assert_eq!(info.codec, VideoCodec::Amv);
        assert_eq!(amv.audio(), None, "ADPCM 音轨第 1 期跳过");
        assert_eq!(amv.audio_declared_format(), Some((1, 1, 22050, 16)), "头声明 PCM（谎）");
        eprintln!(
            "[amv real] parsed: {}x{} fps={} frames={} ({} ms, {} KB)",
            info.width, info.height, info.fps, info.frame_count, parse_ms, bytes.len() / 1024
        );

        let dir = std::env::temp_dir().join(format!("nes_amv_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建 tmpdir");
        let n = (info.width * info.height) as f64;
        // 实测形态：首帧起是黑场淡入、片尾黑场淡出（首帧熵流手工解码
        // 证实：80 个 MCU 全部 DC=-78、零个非零 AC -> 逐像素 (1,1,1)），
        // 中段是彩色动画画面。取样断言两类形态。
        let dark = [0u32, 1, info.frame_count - 1];
        let colorful = [100u32, 1269, 3386];
        for i in dark {
            let img = amv.frame(i).expect("真实帧必须可解码");
            assert_eq!((img.width, img.height), (160, 128));
            let mean: f64 = img
                .rgba
                .chunks_exact(4)
                .map(|px| (i32::from(px[0]) + i32::from(px[1]) + i32::from(px[2])) as f64 / 3.0)
                .sum::<f64>()
                / n;
            assert!(mean < 16.0, "帧 {i} 应是黑场淡入/淡出（实测均值 {mean:.1}）");
            let path = dir.join(format!("amv_frame_{i:03}.bmp"));
            write_bmp(&path, img.width, img.height, &img.rgba).expect("写 BMP");
        }
        for i in colorful {
            let frame_started = std::time::Instant::now();
            let img = amv.frame(i).expect("真实帧必须可解码");
            assert_eq!((img.width, img.height), (160, 128));
            let mut sum = [0i64; 3];
            let mut sum_sq = [0i64; 3];
            for px in img.rgba.chunks_exact(4) {
                for c in 0..3 {
                    sum[c] += i64::from(px[c]);
                    sum_sq[c] += i64::from(px[c]) * i64::from(px[c]);
                }
            }
            let mean = [sum[0] as f64 / n, sum[1] as f64 / n, sum[2] as f64 / n];
            let var = [
                sum_sq[0] as f64 / n - mean[0] * mean[0],
                sum_sq[1] as f64 / n - mean[1] * mean[1],
                sum_sq[2] as f64 / n - mean[2] * mean[2],
            ];
            assert!(
                var.iter().any(|&v| v > 100.0),
                "帧 {i} 必须非平坦（方差 {var:?}）"
            );
            let spread = mean[0].max(mean[1]).max(mean[2]) - mean[0].min(mean[1]).min(mean[2]);
            assert!(spread > 2.0, "帧 {i} 必须非纯灰（通道均值差 {spread:.1}）");
            let path = dir.join(format!("amv_frame_{i:03}.bmp"));
            write_bmp(&path, img.width, img.height, &img.rgba).expect("写 BMP");
            eprintln!(
                "[amv real] frame {}: mean=({:.0},{:.0},{:.0}) var=({:.0},{:.0},{:.0}) {} ms -> {}",
                i, mean[0], mean[1], mean[2], var[0], var[1], var[2],
                frame_started.elapsed().as_millis(),
                path.display()
            );
        }
    }
}
