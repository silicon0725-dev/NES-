//! S14 真实媒体契约测试（skip-if-missing，字体用例惯例）：
//! 用户实测音乐（MP3 / FLAC）经 nes-media 解码的端到端取证。
//!
//! 纪律：
//! * 文件在用户机器上（`C:/Users/Administrator/Music/text/`），**不在仓库**
//!   —— 缺失即 `[skip]`，CI/他机安全（"没有文件"与"有文件跑不通"不许互装）；
//! * 打印与断言全 ASCII（中文文件名不进日志/断言，路径用 ASCII 描述）；
//! * "播放验证"是确定性版本：解码产物登记进 Mixer 混出**非零样本**
//!   （无设备、无时钟、纯数学 —— 与 nes-audio 的混音测试同一条家法）。

use std::sync::Arc;
use std::time::Instant;

use nes_audio::Mixer;
use nes_media::decode_audio;

/// 用户音乐目录（真实交付物现场；两首实测曲所在）。
const MUSIC_DIR: &str = "C:/Users/Administrator/Music/text";
/// 实测曲 1：FLAC（无损，44.1kHz 立体声；文件名为中文，as-is 常量、
/// 打印与断言一律用 ASCII 描述"flac sample"）。
const FLAC_NAME: &str = "心似烟火.flac";
/// 实测曲 2：MP3（有损）。
const MP3_NAME: &str = "Montagem Nada.mp3";

fn full_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(MUSIC_DIR).join(name)
}

/// 解码 + 取证：速率/声道/样本数/解码耗时/内存，打印 ASCII 摘要行。
fn decode_and_report(name: &str, label: &str) -> nes_audio::Wav {
    let path = full_path(name);
    let bytes = std::fs::read(&path).expect("读用户音乐（exists 已 gate）");
    let started = Instant::now();
    let wav = decode_audio(&bytes).expect("用户实测曲必须可解码");
    let elapsed = started.elapsed();
    let secs = if wav.sample_rate > 0 {
        wav.frames() as u64 / u64::from(wav.sample_rate)
    } else {
        0
    };
    eprintln!(
        "[{}] decoded: rate={}Hz ch={} frames={} (~{} sec) file={}KB pcm={}MB decode={}ms",
        label,
        wav.sample_rate,
        wav.channels,
        wav.frames(),
        secs,
        bytes.len() / 1024,
        wav.samples.len() * 2 / (1024 * 1024),
        elapsed.as_millis(),
    );
    wav
}

/// 断言解码产物形状健康 + Mixer 混出非零（确定性"播放验证"）。
///
/// 验证段取**曲中切片**：MP3 编码器在文件头部有 delay/填充（开头数百毫秒
/// 常为数字静音），曲中没有这个假象 —— 播放性要看真音乐在的地方。
fn assert_playable(label: &str, wav: nes_audio::Wav) {
    assert!(wav.sample_rate > 8000, "[{label}] 采样率合理");
    assert!(
        wav.channels == 1 || wav.channels == 2,
        "[{label}] 声道数在 Mixer 能说的范围内（1/2）"
    );
    assert!(!wav.samples.is_empty(), "[{label}] PCM 非空");
    assert!(
        wav.samples.iter().any(|&s| s != 0),
        "[{label}] PCM 必须非全零（真音乐不会是直流静音）"
    );
    // 曲中 4800 帧切片过一遍混音器（重采样 + 声道换算的实链路验证）。
    let ch = usize::from(wav.channels);
    let total_frames = wav.samples.len() / ch;
    let from = (total_frames / 2).min(total_frames.saturating_sub(4800));
    let slice = nes_audio::Wav {
        sample_rate: wav.sample_rate,
        channels: wav.channels,
        samples: wav.samples[from * ch..(from + 4800).min(total_frames) * ch].to_vec(),
    };
    let mut mixer = Mixer::new();
    mixer.register("music", Arc::new(slice));
    mixer.play("music", 1.0, false).expect("登记后必须可播");
    let mut out = [0i16; 480];
    mixer.mix_into(&mut out, 48_000, 2);
    let nonzero = out.iter().filter(|&&s| s != 0).count();
    assert!(nonzero > 0, "[{label}] 曲中切片混音产物必须非零");
}

#[test]
fn t_real01_flac_decodes_and_plays() {
    let path = full_path(FLAC_NAME);
    if !path.exists() {
        eprintln!("[skip] flac sample not found (user music dir absent)");
        return;
    }
    let wav = decode_and_report(FLAC_NAME, "flac");
    assert_playable("flac", wav);
}

#[test]
fn t_real02_mp3_decodes_and_plays() {
    let path = full_path(MP3_NAME);
    if !path.exists() {
        eprintln!("[skip] mp3 sample not found (user music dir absent)");
        return;
    }
    let wav = decode_and_report(MP3_NAME, "mp3");
    assert_playable("mp3", wav);
}
