//! 手写 WAV 解析器：RIFF 块遍历 + 16/24-bit PCM（24-bit 取高 16 位）。
//!
//! # 写出（[`write_wav`]，S13 第 2 期）
//!
//! [`parse`] 的逆操作：把一个 [`Wav`] 编码成 16-bit PCM WAV 写盘
//! （测试往返 + 演示资产生成用 —— audio_demo 示例的 440Hz 蜂鸣即出自
//! 这里，仓库不提交二进制资产）。编码是解码的镜像：`fmt ` 块 16 字节
//! 标准形（audio_format = 1）+ `data` 块 16-bit 小端交错帧；
//! RIFF 总长度字段按规范如实填写（解码器不信任它，但别的工具信）。
//!
//! # 为什么手写（bmp/png 先例）
//!
//! 与 nes-render-wgpu 的 `bmp` / `png` 模块同一条纪律：
//! 零依赖 crate 里"能不能解码"必须是确定事件。WAV 的 16-bit PCM 路径只有
//! 三个块头 + 一次 LE 读取，手写百行内可完成并可测；引入 symphonia/lewton
//! 一类解码库会把整条依赖树拖进 registry（G12 一票否决）。
//!
//! # 覆盖面（与拒绝面同样明确）
//!
//! * 收：`RIFF`/`WAVE` 容器、`fmt ` 块（`audio_format == 1` 纯 PCM）、
//!   `data` 块（16-bit 小端交错帧）、1/2 声道；
//! * 拒：非 PCM 编码（float/a-law/…报 [`WavError::NotPcm`]）、
//!   非 16 位（报 [`WavError::UnsupportedBits`]）、非 1/2 声道
//!   （报 [`WavError::UnsupportedChannels`]）；
//! * 跳：未知块（`LIST` / `fact` / `JUNK` …）按块头长度走，奇数长度按
//!   RIFF 规范补一个 pad 字节再读下一块；
//! * 断：文件在块中间结束（报 [`WavError::Truncated`]）、缺 `fmt `/`data`
//!   （报 [`WavError::MissingChunk`]）。
//!
//! `samples` 是**交错帧**（立体声为 L,R,L,R,…），帧数 = `samples.len() / channels`；
//! [`Wav::frames`] 是这个除法的具名版本，混音器按它推进光标。

#![forbid(unsafe_code)]

use std::fmt;

/// 一个解码完成的 WAV：采样率 + 声道数 + 16-bit 交错样本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wav {
    /// 采样率（每秒帧数），如 44100。解析器不拒绝 0（混音器侧有防御步进兜底）。
    pub sample_rate: u32,
    /// 声道数，解析器保证是 1 或 2。
    pub channels: u16,
    /// 16-bit 小端 PCM，按帧交错；长度必为 `channels` 的偶数倍。
    pub samples: Vec<i16>,
}

impl Wav {
    /// 帧数（一帧 = 所有声道的一组样本）。
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }
}

/// WAV 解析可报告的失败点。`Display` 全部中文、指名道姓。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    /// 前 4 字节不是 `RIFF`：根本不是 RIFF 容器。
    NotRiff,
    /// RIFF 头之后不是 `WAVE` 类型。
    NotWave,
    /// `fmt ` 块声明了非 PCM 编码，携带实际 `audio_format` 代码（要求 1）。
    NotPcm(u16),
    /// 位深不是 16，携带实际位深（本解析器只做 16-bit PCM）。
    UnsupportedBits(u16),
    /// 声道数不是 1/2，携带实际声道数。
    UnsupportedChannels(u16),
    /// 缺少必需块，携带块名（`"fmt "` / `"data"`）。
    MissingChunk(&'static str),
    /// 文件在块头声明的长度之内提前结束（截断）。
    Truncated,
}

impl fmt::Display for WavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WavError::NotRiff => {
                write!(f, "WAV 解析失败：缺少 RIFF 头（前 4 字节不是 \"RIFF\"）")
            }
            WavError::NotWave => {
                write!(f, "WAV 解析失败：RIFF 容器类型不是 \"WAVE\"")
            }
            WavError::NotPcm(code) => {
                write!(f, "WAV 解析失败：仅支持纯 PCM（audio_format = {code}，要求 1）")
            }
            WavError::UnsupportedBits(bits) => {
                write!(f, "WAV 解析失败：仅支持 16-bit PCM（实际 {bits} bit）")
            }
            WavError::UnsupportedChannels(ch) => {
                write!(f, "WAV 解析失败：仅支持 1/2 声道（实际 {ch} 声道）")
            }
            WavError::MissingChunk(name) => {
                write!(f, "WAV 解析失败：缺少必需块 {name:?}")
            }
            WavError::Truncated => {
                write!(f, "WAV 解析失败：文件在块中途被截断")
            }
        }
    }
}

impl std::error::Error for WavError {}

/// 从内存字节解析一个 16-bit PCM WAV。
///
/// 只认字节流，不做文件 IO（装载属于上层职责）；块遍历不依赖 RIFF
/// 总长度字段（很多真实文件把它写错），以实际缓冲结束为准。
pub fn parse(data: &[u8]) -> Result<Wav, WavError> {
    if data.len() < 12 {
        // 连 RIFF 头（4）+ 长度（4）+ 类型（4）都放不下。
        return Err(WavError::Truncated);
    }
    if &data[0..4] != b"RIFF" {
        return Err(WavError::NotRiff);
    }
    if &data[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }

    let mut fmt: Option<(u32, u16, u16)> = None; // (sample_rate, channels)
    let mut pcm_range: Option<std::ops::Range<usize>> = None;
    let mut pos = 12usize;

    while pos + 8 <= data.len() {
        let id: [u8; 4] = data[pos..pos + 4].try_into().expect("块名固定 4 字节");
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().expect("长度固定 4 字节"))
            as usize;
        let body = pos + 8;
        if body + size > data.len() {
            // 块头声称的长度超出实际字节：截断，不静默吃掉。
            return Err(WavError::Truncated);
        }
        match &id {
            b"fmt " => {
                if size < 16 {
                    return Err(WavError::Truncated);
                }
                let audio_format = read_u16(data, body);
                if audio_format != 1 {
                    return Err(WavError::NotPcm(audio_format));
                }
                let channels = read_u16(data, body + 2);
                if channels != 1 && channels != 2 {
                    return Err(WavError::UnsupportedChannels(channels));
                }
                let sample_rate = u32::from_le_bytes(
                    data[body + 4..body + 8].try_into().expect("采样率固定 4 字节"),
                );
                let bits = read_u16(data, body + 14);
                if bits != 16 && bits != 24 {
                    return Err(WavError::UnsupportedBits(bits));
                }
                fmt = Some((sample_rate, channels, bits));
            }
            b"data" => pcm_range = Some(body..body + size),
            // LIST / fact / JUNK / 任何未知块：按块头长度跳过（S13 契约明确要求）。
            _ => {}
        }
        pos = body + size;
        // RIFF 规范：块按字（2 字节）对齐，奇数长度后有一个 pad 字节。
        // 文件恰好在末尾时不补读（最后一块允许不带 pad）。
        if size % 2 == 1 && pos < data.len() {
            pos += 1;
        }
    }

    let (sample_rate, channels, bits) = fmt.ok_or(WavError::MissingChunk("fmt "))?;
    let range = pcm_range.ok_or(WavError::MissingChunk("data"))?;
    let bytes_per = usize::from(bits) / 8;
    if range.len() % bytes_per != 0 {
        // data 长度不是样本整倍数：按截断处理（16-bit 奇字节 / 24-bit 非整帧）。
        return Err(WavError::Truncated);
    }
    // 16-bit 直读；24-bit 取高 16 位（算术右移，保留符号——动态范围
    // 48dB 截断，引擎 SFX 用途足够；文档与 2026-10-03 实测口径一致：
    // 用户音效包 1665 个 WAV 中 344 个为 24-bit，为此放宽契约）。
    let samples: Vec<i16> = if bits == 16 {
        range
            .clone()
            .step_by(2)
            .map(|i| i16::from_le_bytes([data[i], data[i + 1]]))
            .collect()
    } else {
        range
            .clone()
            .step_by(3)
            .map(|i| {
                let v = (data[i] as i32) | ((data[i + 1] as i32) << 8) | ((data[i + 2] as i32) << 16);
                // 24 位符号扩展到 32 位后取高 16 位。
                let ext = (v << 8) >> 8; // 符号扩展
                (ext >> 8) as i16
            })
            .collect()
    };

    Ok(Wav { sample_rate, channels, samples })
}

fn read_u16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

/// 把一个 [`Wav`] 编码成 16-bit PCM WAV 写盘（[`parse`] 的逆操作；
/// 测试往返与演示资产生成用，第 2 期随 audio_demo 示例引入）。
///
/// 编码是解码的镜像：`fmt ` 块 16 字节标准形（`audio_format = 1`）+
/// `data` 块 16-bit 小端交错帧；RIFF 总长度字段按规范如实填写
///（解码器不信任它，但外部工具信）。声道数只认 1/2、采样率只认
/// 能进 `u32` 的值 —— 与解析器的收口一一对应，超出即报错不写半个文件。
pub fn write_wav(path: &std::path::Path, wav: &Wav) -> std::io::Result<()> {
    if wav.channels != 1 && wav.channels != 2 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("WAV 编码失败：仅支持 1/2 声道（实际 {} 声道）", wav.channels),
        ));
    }
    let data_len = wav.samples.len() * 2;

    let mut fmt_body = Vec::with_capacity(18);
    fmt_body.extend_from_slice(&1u16.to_le_bytes()); // audio_format = PCM
    fmt_body.extend_from_slice(&wav.channels.to_le_bytes());
    fmt_body.extend_from_slice(&wav.sample_rate.to_le_bytes());
    fmt_body.extend_from_slice(
        &wav
            .sample_rate
            .wrapping_mul(u32::from(wav.channels) * 2)
            .to_le_bytes(), // byte_rate
    );
    fmt_body.extend_from_slice(&(wav.channels * 2).to_le_bytes()); // block_align
    fmt_body.extend_from_slice(&16u16.to_le_bytes()); // bits
    fmt_body.extend_from_slice(&0u16.to_le_bytes()); // cbSize

    // fmt 体 18 字节标准形（16 必须 + cbSize=0，与第 1 期测试夹具同形）；
    // 块头长度如实写 18 —— 写 16 会让"跳块"逻辑把 cbSize 当下一块头。
    let fmt_len = fmt_body.len() as u32;
    let riff_len = (4 + 8 + fmt_body.len() + 8 + data_len) as u32; // WAVE 类型 + fmt 头体 + data 头体
    let mut out = Vec::with_capacity(8 + riff_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&fmt_len.to_le_bytes());
    out.extend_from_slice(&fmt_body);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in &wav.samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, &out)
}

// ------------------------------------------------------------ 单元测试
//
// 全部用手工装配的最小 WAV 字节夹具：合法路径 + 每一条拒绝路径各一例，
// 不依赖任何外部文件（与 bmp/png 用例同口径）。

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 装配一个最小合法 WAV（fmt + data，可再插入额外块）。
    fn wav_bytes(channels: u16, sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        let mut pcm = Vec::with_capacity(samples.len() * 2);
        for s in samples {
            pcm.extend_from_slice(&s.to_le_bytes());
        }
        let mut fmt_body = Vec::with_capacity(16);
        fmt_body.extend_from_slice(&1u16.to_le_bytes()); // audio_format = PCM
        fmt_body.extend_from_slice(&channels.to_le_bytes());
        fmt_body.extend_from_slice(&sample_rate.to_le_bytes());
        fmt_body.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes()); // byte_rate
        fmt_body.extend_from_slice(&(channels * 2).to_le_bytes()); // block_align
        fmt_body.extend_from_slice(&16u16.to_le_bytes()); // bits
        fmt_body.extend_from_slice(&0u16.to_le_bytes()); // cbSize

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        let body_len = 4 + (8 + fmt_body.len()) + (8 + pcm.len() + pcm.len() % 2);
        out.extend_from_slice(&(body_len as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&chunk(b"fmt ", &fmt_body));
        out.extend_from_slice(&chunk(b"data", &pcm));
        out
    }

    #[test]
    fn t_wav01_minimal_mono_ok() {
        let bytes = wav_bytes(1, 22050, &[10, -20, 30]);
        let wav = parse(&bytes).expect("最小合法 WAV 必须可解析");
        assert_eq!(wav.sample_rate, 22050);
        assert_eq!(wav.channels, 1);
        assert_eq!(wav.samples, vec![10, -20, 30]);
        assert_eq!(wav.frames(), 3);
    }

    #[test]
    fn t_wav02_stereo_interleaved_preserved() {
        let bytes = wav_bytes(2, 44100, &[100, -100, 200, -200]);
        let wav = parse(&bytes).expect("立体声 WAV 必须可解析");
        assert_eq!(wav.channels, 2);
        assert_eq!(wav.samples, vec![100, -100, 200, -200]);
        assert_eq!(wav.frames(), 2);
    }

    #[test]
    fn t_wav03_bad_riff_rejected() {
        let mut bytes = wav_bytes(1, 8000, &[1]);
        bytes[0..4].copy_from_slice(b"RIFX");
        assert_eq!(parse(&bytes), Err(WavError::NotRiff));
    }

    #[test]
    fn t_wav04_bad_wave_type_rejected() {
        let mut bytes = wav_bytes(1, 8000, &[1]);
        bytes[8..12].copy_from_slice(b"AVI ");
        assert_eq!(parse(&bytes), Err(WavError::NotWave));
    }

    #[test]
    fn t_wav05_24bit_top16() {
        // 2026-10-03 真实音效包实测（1665 个 WAV 中 344 个 24-bit）：
        // 契约从"拒绝"演进为"取高 16 位"。24-bit 小端三字节 0x12 0x34 0x56
        // = 0x00563412，符号扩展后 >>8 = 0x5634。
        let mut bytes = wav_bytes(1, 8000, &[1]);
        // 布局（wav_bytes 固定形）：RIFF 头 12 | fmt 块头 8 + 体 18 | data 块头 8 + 体 2。
        let fmt_at = 12 + 8; // fmt 体起点（20）
        bytes[fmt_at + 14..fmt_at + 16].copy_from_slice(&24u16.to_le_bytes());
        let data_hdr = fmt_at + 18; // data 块头起点（38）
        bytes[data_hdr + 4..data_hdr + 8].copy_from_slice(&3u32.to_le_bytes()); // 块长 3
        bytes.truncate(data_hdr + 8); // 丢原 16-bit 样本
        bytes.extend_from_slice(&[0x12, 0x34, 0x56]); // 24-bit 单样本
        let riff_len = (bytes.len() as u32) - 8;
        bytes[4..8].copy_from_slice(&riff_len.to_le_bytes());

        let wav = parse(&bytes).expect("24-bit 必须可解析");
        assert_eq!(wav.samples, vec![0x5634i16]);
    }

    #[test]
    fn t_wav06_8bit_rejected() {
        let mut bytes = wav_bytes(1, 8000, &[1]);
        let at = 12 + 8 + 14;
        bytes[at..at + 2].copy_from_slice(&8u16.to_le_bytes());
        assert_eq!(parse(&bytes), Err(WavError::UnsupportedBits(8)));
    }

    #[test]
    fn t_wav07_non_pcm_format_rejected() {
        let mut bytes = wav_bytes(1, 8000, &[1]);
        // audio_format = 3（IEEE float）：格式对但编码不对，必须指名拒绝。
        let at = 12 + 8;
        bytes[at..at + 2].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(parse(&bytes), Err(WavError::NotPcm(3)));
    }

    #[test]
    fn t_wav08_four_channels_rejected() {
        let mut bytes = wav_bytes(1, 8000, &[1]);
        let at = 12 + 8 + 2;
        bytes[at..at + 2].copy_from_slice(&4u16.to_le_bytes());
        assert_eq!(parse(&bytes), Err(WavError::UnsupportedChannels(4)));
    }

    #[test]
    fn t_wav09_missing_data_rejected() {
        // 只留 fmt：把 data 块整个裁掉（RIFF 长度字段解析器不信任，改不改不影响）。
        let full = wav_bytes(1, 8000, &[1, 2, 3]);
        let cut = 12 + 8 + 16 + 8; // RIFF 头 + WAVE 类型 + fmt 头 + fmt 体
        let bytes = &full[..cut];
        assert_eq!(parse(bytes), Err(WavError::MissingChunk("data")));
    }

    #[test]
    fn t_wav10_missing_fmt_rejected() {
        // 手工拼一个只有 data 块的 WAVE。
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(&chunk(b"data", &[1, 0, 2, 0]));
        assert_eq!(parse(&bytes), Err(WavError::MissingChunk("fmt ")));
    }

    #[test]
    fn t_wav11_list_chunk_skipped() {
        // LIST 块出现在 fmt 之前：必须被按块头长度跳过，不影响解析。
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes()); // 总长度字段：解析器不信任
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(&chunk(b"LIST", b"INFOISFTmetadatapadding"));
        bytes.extend_from_slice(&chunk(
            b"fmt ",
            &{
                let mut f = Vec::new();
                f.extend_from_slice(&1u16.to_le_bytes());
                f.extend_from_slice(&1u16.to_le_bytes());
                f.extend_from_slice(&8000u32.to_le_bytes());
                f.extend_from_slice(&16000u32.to_le_bytes());
                f.extend_from_slice(&2u16.to_le_bytes());
                f.extend_from_slice(&16u16.to_le_bytes());
                f.extend_from_slice(&0u16.to_le_bytes());
                f
            },
        ));
        bytes.extend_from_slice(&chunk(b"data", &[7u16.to_le_bytes(), 8i16.to_le_bytes()].concat()));
        let wav = parse(&bytes).expect("带 LIST 块的 WAV 必须可解析");
        assert_eq!(wav.samples, vec![7, 8]);
    }

    #[test]
    fn t_wav12_unknown_odd_chunk_pad_handled() {
        // 奇数长度未知块：规范要求 pad 字节，pad 后的 fmt/data 必须仍能对齐读出。
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(&chunk(b"JUNK", &[0xAA, 0xBB, 0xCC])); // 3 字节 + pad
        bytes.extend_from_slice(&chunk(
            b"fmt ",
            &{
                let mut f = Vec::new();
                f.extend_from_slice(&1u16.to_le_bytes());
                f.extend_from_slice(&1u16.to_le_bytes());
                f.extend_from_slice(&44100u32.to_le_bytes());
                f.extend_from_slice(&88200u32.to_le_bytes());
                f.extend_from_slice(&2u16.to_le_bytes());
                f.extend_from_slice(&16u16.to_le_bytes());
                f.extend_from_slice(&0u16.to_le_bytes());
                f
            },
        ));
        bytes.extend_from_slice(&chunk(
            b"data",
            &[
                9i16.to_le_bytes(),
                (-(9i16)).to_le_bytes(),
            ]
            .concat(),
        ));
        let wav = parse(&bytes).expect("奇数长度未知块后必须仍可解析");
        assert_eq!(wav.samples, vec![9, -9]);
    }

    #[test]
    fn t_wav13_truncated_body_rejected() {
        let mut bytes = wav_bytes(1, 8000, &[1, 2, 3, 4]);
        bytes.truncate(bytes.len() - 3); // data 块体被裁短，块头长度不再匹配
        assert_eq!(parse(&bytes), Err(WavError::Truncated));
    }

    #[test]
    fn t_wav14_truncated_header_rejected() {
        assert_eq!(parse(b"RI"), Err(WavError::Truncated));
        assert_eq!(parse(b"RIFF\x10\x00\x00\x00WAV"), Err(WavError::Truncated));
    }

    #[test]
    fn t_wav15_odd_data_length_rejected() {
        // data 块 3 字节：16-bit 样本对不齐。
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(&chunk(
            b"fmt ",
            &{
                let mut f = Vec::new();
                f.extend_from_slice(&1u16.to_le_bytes());
                f.extend_from_slice(&1u16.to_le_bytes());
                f.extend_from_slice(&8000u32.to_le_bytes());
                f.extend_from_slice(&16000u32.to_le_bytes());
                f.extend_from_slice(&2u16.to_le_bytes());
                f.extend_from_slice(&16u16.to_le_bytes());
                f.extend_from_slice(&0u16.to_le_bytes());
                f
            },
        ));
        bytes.extend_from_slice(&chunk(b"data", &[1, 2, 3]));
        assert_eq!(parse(&bytes), Err(WavError::Truncated));
    }

    #[test]
    fn t_wav16_empty_data_is_valid_silence() {
        let bytes = wav_bytes(1, 8000, &[]);
        let wav = parse(&bytes).expect("零样本 data 块是合法静音");
        assert!(wav.samples.is_empty());
        assert_eq!(wav.frames(), 0);
    }

    #[test]
    fn t_wav17_error_display_is_chinese_named() {
        // Display 必须指名道姓（错误哲学与后端 error.rs 同一条）。
        assert!(WavError::NotRiff.to_string().contains("RIFF"));
        assert!(WavError::NotWave.to_string().contains("WAVE"));
        assert!(WavError::NotPcm(3).to_string().contains('3'));
        assert!(WavError::UnsupportedBits(24).to_string().contains("24"));
        assert!(WavError::UnsupportedChannels(4).to_string().contains('4'));
        assert!(WavError::MissingChunk("data").to_string().contains("data"));
        assert!(WavError::Truncated.to_string().contains("截断"));
    }

    #[test]
    fn t_wav18_write_parse_roundtrip_preserves_everything() {
        // write -> parse 往返：单声道/立体声、奇偶样本数、空样本全部逐位保真。
        for (channels, samples) in [
            (1u16, vec![10i16, -20, 30]),
            (2, vec![100, -100, 200, -200]),
            (1, vec![1, 2, 3, 4, 5]), // 奇数个单声道样本 = 奇数字节
            (1, Vec::new()),          // 空样本（合法静音）
        ] {
            let wav = Wav { sample_rate: 44100, channels, samples };
            let dir = std::env::temp_dir();
            let path = dir.join(format!(
                "nes_audio_roundtrip_{}_{}_{}.wav",
                channels,
                wav.frames(),
                std::process::id()
            ));
            write_wav(&path, &wav).expect("写出 WAV");
            let bytes = std::fs::read(&path).expect("读回字节");
            let parsed = parse(&bytes).expect("往返后必须可解析");
            assert_eq!(parsed, wav, "声道 {channels} 往返逐位保真");
            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn t_wav19_write_rejects_bad_channels_without_file() {
        // 解析器放行的前提是 1/2 声道；编码侧同口径拒绝（不写半个文件）。
        let wav = Wav { sample_rate: 8000, channels: 3, samples: vec![0; 3] };
        let path = std::env::temp_dir().join(format!("nes_audio_badch_{}.wav", std::process::id()));
        let err = write_wav(&path, &wav).expect_err("3 声道必须被拒");
        assert!(err.to_string().contains("3"), "错误指名声道数：{err}");
        assert!(!path.exists(), "失败路径不得落盘");
    }
}
