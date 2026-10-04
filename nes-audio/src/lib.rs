//! NES 2.0 音频核心 —— **S13 第 1 期**（引擎至今无声，本 crate 补上内核音频）。
//!
//! # 这个 crate 是什么
//!
//! 纯 Rust 零依赖的音频三件套：
//!
//! ```text
//! WAV 字节 ──────────────▶ [wav] 解码 ──▶ [mixer] 混音 ──▶ [device] waveOut 出声
//! AMV IMA ADPCM 块 ──▶ [adpcm] 解码 ──┘
//! ```
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`wav`] | 手写 WAV 解析器：RIFF 块遍历 + 16-bit PCM，只做这一种（bmp/png 先例） |
//! | [`adpcm`] | IMA ADPCM 解码（AMV 音轨，FFmpeg `ADPCM_IMA_AMV` 语义）-> 16-bit PCM |
//! | [`mixer`] | 纯数学混音器：声音库 + 声部 + 线性重采样 + 软削顶，**不碰设备** |
//! | [`device`] | winmm waveOut 输出：手写 FFI + CALLBACK_NULL 轮询 + 专属填充线程 |
//!
//! 运行时 / 脚本 / 资产集成是**第 2 期**（另行派工）：本 crate 不认识 nes-scene、
//! nes-asset、nes-runtime，任何 crate 也不得依赖本 crate（第 2 期时 runtime
//! 正向接入，方向唯一：`runtime ──▶ nes-audio`，见 `check_dependency_direction.py` G12）。
//!
//! # 依赖与 unsafe 纪律
//!
//! * **零依赖**：`Cargo.toml` 的 `[dependencies]` 为空表，连本仓库其它 crate 也不见 ——
//!   与 nes-asset / nes-render-api 同一条理由（本机工具链不确定性 + 叶子不背依赖面）；
//! * **unsafe 只在 [`device`] 一个模块里**：wav / adpcm / mixer 三个模块
//!   各自 `#![forbid(unsafe_code)]`，FFI 与线程收敛到 [`device`]，每个调用点带 SAFETY 注释；
//! * **设备不可用不是 panic**：`waveOutGetNumDevs() == 0` 或 `waveOutOpen` 失败
//!   一律返回 [`AudioError`]，与 GPU 用例"没有库就如实报告跳过"同口径。
//!
//! # waveOut 线程模型（[`device`] 模块 doc 有全图）
//!
//! `CALLBACK_NULL`（无回调）+ 专属线程轮询：线程每 ~10ms 用 [`Mixer::mix_into`]
//! 填充环形 4 个 `WAVEHDR` 缓冲并 `waveOutWrite` 提交；关闭时同线程串行执行
//! `waveOutReset → waveOutUnprepareHeader × N → waveOutClose` 后退出，
//! 所有 winmm 调用单线程化，根除"句柄在 write 中被另一线程 close"一类竞态。
//!
//! # 怎么跑
//!
//! ```text
//! cargo test --release          # 混音数学全部无设备可测；设备冒烟在无 waveOut 时自动跳过
//! cargo clippy --release --all-targets
//! ```

#![deny(rust_2018_idioms)]

pub mod adpcm;
pub mod device;
pub mod mixer;
pub mod wav;

pub use adpcm::{decode_ima_amv, AdpcmError};
pub use device::{AudioDevice, AudioError};
pub use mixer::{Mixer, MixerError, Voice};
pub use wav::{parse, Wav, WavError};
