//! 音频解码（symphonia 0.5 系）：任意已收录容器/编码 -> `nes_audio::Wav` 同构。
//!
//! # 为什么是适配层
//!
//! 引擎核心的声音口径是 **16-bit PCM WAV**（`nes-audio::wav` 手写解析，
//! 零依赖纪律；[`nes_audio::Wav`] 是混音器的唯一输入形状）。外部交付物
//! 却是 MP3/FLAC/OGG/M4A —— 本模块把 symphonia 收口在这里：入口只有
//! [`decode_audio`] 一个函数，出口是**引擎既有类型** `nes_audio::Wav`
//! （sample_rate/channels/16-bit 交错样本），symphonia 的 Format/Decoder/
//! Packet 等第三方类型**不出本 crate**。
//!
//! # 解码口径
//!
//! * probe 按内容嗅探容器（不看扩展名）→ 取第一条有编码的音轨 →
//!   逐包解码到内存 PCM（**全轨进内存**：一首 4 分钟 44.1kHz 立体声
//!   约 40MB i16 —— P0 可接受，流式是后续，见 S14 文档 §5）；
//! * 采样格式经 f32 中间面统一：`f32 -> i16` clamp 缩放（symphonia f32
//!   面满幅单位为 s/32768 —— 2 的幂次缩放在 IEEE754 上精确可逆，16-bit
//!   源逐位保真；越界样本钳回 [-32768, 32767]，不回绕炸音）；
//! * 任意源采样率/声道**如实保留**进 `Wav`（Mixer 自带线性重采样与
//!   1/2 声道换算；>2 声道源保留原样 —— Mixer 现只说 1/2 声道，此类
//!   源登记后暂不出声，见 S14 文档 §5 遗留）；
//! * 单个坏包跳过（音乐可听性优先：一个损坏帧不该废掉整首曲子），
//!   容器/解码器级失败才整体报 [`MediaError`]。
//!
//! # 与 nes-audio 的装载序（上层纪律，这里只提供能力）
//!
//! 上层（nes-runtime）Sound 装载先试 `nes_audio::wav::parse`（零解码
//! 开销的快路径），失手再回落本模块 —— WAV 资产零额外开销，MP3/FLAC/
//! OGG 由此通吃（8-bit/float WAV 等原生解析器拒收的变体也顺带被
//! symphonia 接住）。

#![forbid(unsafe_code)]

use std::io::Cursor;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::MediaError;

/// 从内存字节解码一段音频为 16-bit 交错 PCM（`nes_audio::Wav` 同构）。
///
/// 格式覆盖面 = 本 crate 的 symphonia features（mp3/flac/ogg/vorbis/
/// pcm/wav/isomp4-aac）。任意源采样率/声道如实保留。
pub fn decode_audio(data: &[u8]) -> Result<nes_audio::Wav, MediaError> {
    // 字节流 -> MediaSourceStream（Cursor 全内存；不做文件 IO）。
    let mss = MediaSourceStream::new(Box::new(Cursor::new(data.to_vec())), Default::default());
    // 无扩展名可用：probe 纯按容器魔数嗅探（与图像面同一口径 —— 字节不说谎）。
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| symphonia_err("容器探测", e))?;
    let mut format = probed.format;

    // 第一条"有编码器可解"的音轨（多轨容器忽略附加轨 —— 引擎只要一条声带）。
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| MediaError::Decode("容器内没有可解码的音频轨".into()))?
        .clone();
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| symphonia_err("解码器构造", e))?;

    // 采样率/声道：先取轨级声明（包解码前即可判空），随每个包的实测
    // spec 刷新（某些容器的轨级声明缺省或与实际不符）。
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut channels = track.codec_params.channels.map(|c| c.count() as u16).unwrap_or(0);
    let mut samples: Vec<i16> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // 正常结束：容器读到 EOF（symphonia 用 UnexpectedEof 表达）。
            Err(SymphoniaError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(e) => return Err(symphonia_err("读包", e)),
        };
        if packet.track_id() != track_id {
            continue; // 非目标音轨（歌词/封面轨等）：整包跳过
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                if spec.rate > 0 {
                    sample_rate = spec.rate;
                }
                let ch = spec.channels.count();
                if ch > 0 {
                    channels = ch as u16;
                }
                // 统一经 f32 中间面（SampleBuffer 负责各原生格式 -> f32），
                // 再做 f32 -> i16 clamp 缩放（见模块头；越界钳回不回绕）。
                // 缩放基准取 32768（symphonia f32 面的满幅单位是 s/32768
                // —— 2 的幂次往返在 IEEE754 上**精确**：16-bit 源逐位保真，
                // 满幅正样本 1.0 钳到 32767，负满幅 -1.0 恰为 -32768）。
                let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                buf.copy_interleaved_ref(decoded);
                for &s in buf.samples() {
                    samples.push((s * 32768.0).clamp(-32768.0, 32767.0) as i16);
                }
            }
            // 单包损坏：跳过继续（可听性优先；连续坏包只是少一段音频）。
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(symphonia_err("解码", e)),
        }
    }

    if sample_rate == 0 || channels == 0 {
        return Err(MediaError::Decode(
            "解码产物缺采样率或声道信息（0 个样本轨或轨头残缺）".into(),
        ));
    }
    Ok(nes_audio::Wav { sample_rate, channels, samples })
}

/// symphonia 错误 -> [`MediaError`]（边界处收口，第三方错误类型不出本模块；
/// `ResetRequired` 等容器级状态与 `IoError` 如实分叉，不互相冒充）。
fn symphonia_err(stage: &str, e: SymphoniaError) -> MediaError {
    match e {
        SymphoniaError::Unsupported(_) => MediaError::UnsupportedFormat,
        SymphoniaError::IoError(io) => MediaError::Io(format!("{stage}：{io}")),
        other => MediaError::Decode(format!("{stage}：{other}")),
    }
}

// ------------------------------------------------------------ 单元测试
//
// 合成字节夹具：用 nes-audio 的 write_wav 现场生成合法 WAV（16-bit PCM
// 经 symphonia 的 wav 解码器应逐位保真 —— 无损路径可精确断言）。真实
// MP3/FLAC 契约测试在 tests/real_media.rs（skip-if-missing，字体惯例）。

#[cfg(test)]
mod tests {
    use super::*;
    use nes_audio::wav::write_wav;

    #[test]
    fn t_aud01_wav_via_symphonia_roundtrip_bit_exact() {
        // 16-bit PCM WAV 是无损路径：symphonia 解码产物必须与源逐位一致
        //（单声道/立体声、负样本、奇数帧数一并覆盖）。
        for (channels, samples) in [
            (1u16, vec![10i16, -20, 30]),
            (2, vec![100, -100, 200, -200]),
            (1, (0..255i32).map(|i| (i * 257 - 32768) as i16).collect::<Vec<_>>()),
        ] {
            let src = nes_audio::Wav { sample_rate: 22050, channels, samples };
            let path = std::env::temp_dir().join(format!(
                "nes_media_wav_{}_{}.wav",
                channels,
                std::process::id()
            ));
            write_wav(&path, &src).expect("写 WAV 夹具");
            let bytes = std::fs::read(&path).expect("读回夹具");
            std::fs::remove_file(&path).ok();
            let decoded = decode_audio(&bytes).expect("WAV 必须经 symphonia 可解码");
            assert_eq!(decoded.sample_rate, 22050);
            assert_eq!(decoded.channels, channels);
            assert_eq!(decoded.samples, src.samples, "无损路径逐位保真");
            assert_eq!(decoded.frames(), src.frames());
        }
    }

    #[test]
    fn t_aud02_garbage_reports_error_not_panic() {
        // 明确不是任何已收录容器的字节：报错、不 panic、错误可读。
        let err = decode_audio(b"\x00not a media container at all").expect_err("垃圾字节必须被拒");
        assert!(
            matches!(
                err,
                MediaError::UnsupportedFormat | MediaError::Decode(_) | MediaError::Io(_)
            ),
            "垃圾字节报三态之一：{err:?}"
        );
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn t_aud03_empty_input_reports_error() {
        let err = decode_audio(&[]).expect_err("空输入必须被拒");
        assert!(err.to_string().contains("媒体"), "Display 中文：{err}");
    }

    #[test]
    fn t_aud04_output_is_mixer_compatible_shape() {
        // 引擎面契约：解码产物必须能直接进 Mixer（register + play + 混出
        // 非零 —— 这就是"播放验证"的确定性版本，无需任何设备）。
        let src = nes_audio::Wav {
            sample_rate: 22050,
            channels: 1,
            samples: (0..2205).map(|i| ((i % 100) * 200 - 10000) as i16).collect::<Vec<_>>(),
        };
        let path = std::env::temp_dir().join(format!("nes_media_mix_{}.wav", std::process::id()));
        write_wav(&path, &src).expect("写 WAV 夹具");
        let bytes = std::fs::read(&path).expect("读回夹具");
        std::fs::remove_file(&path).ok();
        let wav = decode_audio(&bytes).expect("解码");
        let mut mixer = nes_audio::Mixer::new();
        mixer.register("t", std::sync::Arc::new(wav));
        mixer.play("t", 1.0, false).expect("混音器直接可播");
        let mut out = [0i16; 100];
        mixer.mix_into(&mut out, 22050, 1);
        assert!(out.iter().any(|&s| s != 0), "混音产物必须非零（可听）");
    }
}
