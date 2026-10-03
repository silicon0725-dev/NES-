//! NES 2.0 媒体解码适配层 —— **S14 第 1 期**（依赖分层政策正式生效）。
//!
//! # 这个 crate 是什么
//!
//! 引擎核心坚持零第三方依赖（bmp/png/wav 手写解析先例），但现实交付物
//! 是 MP3 / FLAC / OGG / PNG / JPEG / WebP…… 手写这些解码器不可行。
//! 本 crate 是两者之间的**唯一闸口**：把成熟 Rust 编解码库收口在最外圈
//! 的一个叶子上，对引擎暴露干净 DTO。
//!
//! ```text
//! PNG/JPEG/GIF/WebP 字节 ──▶ [image] 解码 ──▶ DecodedImage（RGBA8）
//! MP3/FLAC/OGG/WAV/M4A 字节 ──▶ [audio] 解码 ──▶ nes_audio::Wav（i16 交错）
//! ```
//!
//! # 依赖分层政策（S14 用户裁决，守卫 G13 校验）
//!
//! **本 crate 是全仓库唯一允许第三方依赖的 crate —— 编解码适配层，
//! 引擎核心零依赖纪律不变。** 白名单：`image` 系（含全部传递依赖）+
//! `symphonia` 系（含全部传递依赖）+ 仓库内 `nes-audio`（path）；
//! 其它任何 registry 依赖一律越界。消费方向唯一：`nes-runtime ──▶
//! nes-media`（任何其它 crate 不得依赖本 crate）。
//!
//! # 引擎面（干净 DTO）
//!
//! * 图像：[`decode_image`] -> [`DecodedImage`] `{ width, height, rgba }`
//!   —— RGBA8 直通既有纹理注册/上传路径（`CommandConsumer::register_texture`）；
//! * 音频：[`decode_audio`] -> `nes_audio::Wav` **同构**（任意源率/声道
//!   如实保留，16-bit 交错 PCM）—— Mixer 自带线性重采样与 1/2 声道换算，
//!   上层对"这条声音来自 MP3 还是 WAV"零感知；
//! * 视频（S14.2"小而完整"实验）：[`avi::AviVideo`] —— AVI 容器手写
//!   RIFF（wav.rs 同族家法），帧解码 DIB 手写 / MJPG 走 `image`，音轨
//!   PCM 直接产 `nes_audio::Wav` 进混音器；产物同为上面两个 DTO；
//! * 错误：[`MediaError`] 三态（格式不支持 / 解码失败 / IO），Display 中文。
//!
//! # 覆盖面与边界
//!
//! * 图像覆盖面 = `image` 0.25 默认 feature 集（PNG/JPEG/GIF/WebP/BMP/
//!   TIFF/…）；GIF 取首帧，动图帧数如实记 [`DecodedImage::frame_count`]；
//! * 音频覆盖面 = symphonia features（mp3/flac/ogg/vorbis/pcm/wav/
//!   isomp4/aac）；解码是**全轨进内存**（一首 4 分钟曲子约 40-80MB
//!   PCM —— P0 可接受，流式是后续，见 S14 文档 §5）；
//! * 视频：AVI 1.0 单段（DIB / MJPG 帧 + PCM 音轨），覆盖面与 AMV /
//!   avio（FFmpeg）路线的边界见 [`avi`] 模块文档；
//! * **AMV 不做**（下一轮 Adapter 通用性试金石）：适配层已留位 —— 新
//!   格式 = 本 crate 新模块 + G13 白名单扩条，引擎核心不动。
//!
//! # unsafe 与测试纪律
//!
//! `#![forbid(unsafe_code)]`（第三方库内部不归我们管，我们的代码面零
//! unsafe）。真实媒体文件的契约测试 skip-if-missing（字体用例惯例）——
//! 用户音乐不在仓库，CI/他机安全。

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

pub mod avi;
pub mod audio;
pub mod image;

pub use avi::{AviVideo, VideoCodec, VideoInfo};
pub use audio::decode_audio;
pub use image::{decode_image, DecodedImage};

use std::fmt;

/// 媒体解码可报告的失败点。`Display` 全中文（错误哲学与引擎各 crate 同一条）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// 文件头无法识别 / 本 crate 未收录该格式（不含解码中途的损坏）。
    UnsupportedFormat,
    /// 数据损坏或解码器拒绝，携带指名道姓的中文原因。
    Decode(String),
    /// 读缓冲/容器层 IO 失败，携带原因。
    Io(String),
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaError::UnsupportedFormat => {
                write!(f, "媒体解码失败：格式不支持（文件头无法识别或未收录该格式）")
            }
            MediaError::Decode(why) => write!(f, "媒体解码失败：{why}"),
            MediaError::Io(why) => write!(f, "媒体读取失败：{why}"),
        }
    }
}

impl std::error::Error for MediaError {}
