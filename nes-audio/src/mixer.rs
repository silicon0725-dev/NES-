//! 纯数学混音器：声音库 + 播放声部 + 线性重采样 + 软削顶，**不碰任何设备**。
//!
//! # 设计要点
//!
//! * **声音库与声部分离**：[`Mixer::register`] 把 [`Wav`] 以字符串键登记为
//!   共享库（`Arc` 克隆零拷贝）；[`Mixer::play`] 只是从库里取引用开一个
//!   [`Voice`]（光标从 0 起步）。同一声音可同时开任意多个声部。
//! * **重采样即步进**：每个声部持一个 `f32` 光标（单位：源样本），
//!   每产出一个设备帧推进 `wav.sample_rate / device_rate`。源比设备快就
//!   跳步（44100 → 22050 每帧走 2.0），慢就磨蹭；帧内做线性插值。
//! * **软削顶**：全部声部在 `f32` 上累加，最后 clamp 回 `i16` 范围 ——
//!   两个满幅声音相加饱和到 ±满幅，而不是回绕炸音。
//! * **确定性**：混音不依赖时钟、线程或随机数；[`Mixer::mix_into`] 在给定
//!   相同缓冲与声部状态下永远产出相同字节。因此**混音数学的全部测试
//!   都不需要设备**（设备线程只负责"每隔一会调一次 mix_into"这一件事）。
//!
//! # 未做（有意留白，第 2 期再议）
//!
//! * 不做声像（pan）/ 包络 / 淡出 —— 声部只有音量一个系数；
//! * 不做优先级抢占或声部上限 —— 交给上层策略；
//! * 不缓存重采样结果 —— 每帧即算，NES 级别音量下这不会是热点。

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::wav::Wav;

/// 一个正在播放的声部：库中某个 [`Wav`] 的引用 + 播放状态。
#[derive(Debug, Clone)]
pub struct Voice {
    /// 声部引用的源声音（与声音库共享同一份样本，零拷贝）。
    pub wav: Arc<Wav>,
    /// 开声时用的库键（S15 起记录：[`Mixer::stop_key`] 按它点名停声 ——
    /// 视频音轨停止语义需要"只停这一个键的声部"，`stop_all` 太宽）。
    pub key: String,
    /// 播放光标，单位 = 源样本（`f32` 以支持重采样步进）；`floor(cursor)` 为当前帧。
    pub cursor: f32,
    /// 声部音量，登记时已钳到 0..=1。
    pub volume: f32,
    /// 是否循环：true 时光标越过末尾绕回头部，永不自动移除。
    pub looped: bool,
}

/// 混音器可报告的失败点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MixerError {
    /// [`Mixer::play`] 用了未登记的声音键，携带该键。
    UnknownSound(String),
}

impl fmt::Display for MixerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MixerError::UnknownSound(key) => {
                write!(f, "混音器：声音键 {key:?} 未注册（register 后才能 play）")
            }
        }
    }
}

impl std::error::Error for MixerError {}

/// 混音器：注册声音库 + 活动声部列表 + 总音量。
#[derive(Debug)]
pub struct Mixer {
    library: HashMap<String, Arc<Wav>>,
    voices: Vec<Voice>,
    master_volume: f32,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mixer {
    /// 空混音器：无声音库、无声部、总音量 1.0。
    pub fn new() -> Self {
        Self { library: HashMap::new(), voices: Vec::new(), master_volume: 1.0 }
    }

    /// 登记一个声音；同键覆盖（旧 `Arc` 由仍在播放的声部继续持有）。
    pub fn register(&mut self, key: impl Into<String>, wav: Arc<Wav>) {
        self.library.insert(key.into(), wav);
    }

    /// 已登记的声音键，按字典序（确定性输出，诊断/脚本枚举用）。
    pub fn list(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.library.keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys
    }

    /// 键是否已登记。
    pub fn has(&self, key: &str) -> bool {
        self.library.contains_key(key)
    }

    /// 开一个声部播放已登记的声音；未登记的键报 [`MixerError::UnknownSound`]。
    ///
    /// `volume` 越界值钳到 0..=1（NaN 按 0 处理），不视为错误。
    pub fn play(&mut self, key: &str, volume: f32, looped: bool) -> Result<(), MixerError> {
        let wav = self
            .library
            .get(key)
            .ok_or_else(|| MixerError::UnknownSound(key.to_string()))?;
        self.voices.push(Voice {
            wav: Arc::clone(wav),
            key: key.to_string(),
            cursor: 0.0,
            volume: clamp01(volume),
            looped,
        });
        Ok(())
    }

    /// 停掉全部声部（声音库不受影响）。
    pub fn stop_all(&mut self) {
        self.voices.clear();
    }

    /// 停掉引用指定库键的全部声部（S15：视频音轨停止语义），返回停掉
    /// 的数目。键未开过声部 = 返回 0（幂等无害）；声音库不受影响。
    pub fn stop_key(&mut self, key: &str) -> usize {
        let before = self.voices.len();
        self.voices.retain(|v| v.key != key);
        before - self.voices.len()
    }

    /// 设置总音量；越界值钳到 0..=1（NaN 按 0 处理）。
    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = clamp01(volume);
    }

    /// 当前总音量。
    pub fn master_volume(&self) -> f32 {
        self.master_volume
    }

    /// 活动声部数。
    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    /// 活动声部只读视图（诊断/测试用：可以直接断言光标推进与音量钳制）。
    pub fn voices(&self) -> &[Voice] {
        &self.voices
    }

    /// 指定键的活动声部**已播源样本位**与源采样率（S15.1 音频钟观测面）。
    ///
    /// * 返回 `(已播源样本数, 源采样率)`：样本数 = `floor(voice.cursor)`
    ///   （光标单位本来就是源样本，见 [`Voice::cursor`]；[`Mixer::mix_into`]
    ///   被设备真实消耗逐帧推进 —— 所以这个读数天然是"耳朵已经听到的位置"，
    ///   视频帧号由它导出即得采样级音画对齐）；
    /// * 该键没有活动声部（未播放 / 非循环已播完被移除 / `stop_key` 点名停）
    ///   返回 `None` —— 上层以"见过声部之后变 None"判音画同终；
    /// * 同键多个声部取第一个（视频音轨每键只开一个声部；普通声音不经过
    ///   此面）；只读，不动声部、不加锁语义 —— 与 [`Self::voices`] 同读面。
    pub fn voice_position(&self, key: &str) -> Option<(u64, u32)> {
        self.voices
            .iter()
            .find(|v| v.key == key)
            .map(|v| (v.cursor.floor().max(0.0) as u64, v.wav.sample_rate))
    }

    /// 把全部活动声部混进一个设备缓冲。
    ///
    /// * `out`：交错 `i16` 样本，长度应能被 `channels` 整除（余数按 0 混出后写回）；
    /// * `device_rate`：设备采样率；`channels`：设备声道数，只认 1/2；
    /// * 声道换算：立体声源 → 单声道设备取平均；单声道源 → 立体声设备两路复制；
    /// * 返回**本帧内播完并被移除**的声部数（诊断用；循环声部永不播完）。
    ///
    /// # 防御口径
    ///
    /// `device_rate == 0`、`channels` 非 1/2、或 `out` 为空时不出声
    /// （`out` 清零）并返回 0 —— 设备层传坏参数不应变成 panic 或死循环；
    /// 源 `sample_rate == 0`（解析器放行的病态值）按步进 1.0 兜底。
    pub fn mix_into(&mut self, out: &mut [i16], device_rate: u32, channels: u16) -> usize {
        let usable =
            device_rate > 0 && (channels == 1 || channels == 2) && !out.is_empty();
        let dev_ch = if usable { channels as usize } else { 0 };
        let frames = if usable { out.len() / dev_ch } else { 0 };

        let mut acc = vec![0.0f32; out.len()];
        let mut finished = 0usize;

        if usable {
            for slot in self.voices.iter_mut() {
                let volume = slot.volume;
                let looped = slot.looped;
                let wav = Arc::clone(&slot.wav);
                let src_ch = usize::from(wav.channels);
                let src_frames = wav.samples.len() / src_ch;
                if src_ch != 1 && src_ch != 2 {
                    continue; // 解析器保证不会发生；防御性地跳过
                }
                if src_frames == 0 {
                    finished += 1; // 空样本：无论是否循环都立即结束
                    continue;
                }
                let step = if wav.sample_rate == 0 {
                    1.0
                } else {
                    wav.sample_rate as f32 / device_rate as f32
                };
                let src = &wav.samples;
                let cursor = &mut slot.cursor;
                for frame in 0..frames {
                    if *cursor >= src_frames as f32 {
                        if !looped {
                            break; // 非循环到尾：本帧余下部分静音，声部待移除
                        }
                        *cursor %= src_frames as f32; // 循环回绕（可越过一整个周期）
                    }
                    let pos = (*cursor) as usize;
                    let frac = *cursor - pos as f32;
                    // 相邻源帧：循环在头部绕回，非循环钳在最后一帧（插值退化为定值）。
                    let next = if pos + 1 < src_frames {
                        pos + 1
                    } else if looped {
                        0
                    } else {
                        pos
                    };
                    // 每个输出声道取插值后的源值（立体声源→单声道取平均，反之复制）。
                    for c in 0..dev_ch {
                        let base = |at: usize| -> f32 {
                            if dev_ch == 1 && src_ch == 2 {
                                (f32::from(src[at * 2]) + f32::from(src[at * 2 + 1])) * 0.5
                            } else if dev_ch == 1 {
                                f32::from(src[at])
                            } else if c == 0 {
                                f32::from(src[at * src_ch])
                            } else {
                                f32::from(src[at * src_ch + src_ch - 1])
                            }
                        };
                        let s0 = base(pos);
                        let s1 = base(next);
                        acc[frame * dev_ch + c] += (s0 + (s1 - s0) * frac) * volume;
                    }
                    *cursor += step;
                }
                if !looped && *cursor >= src_frames as f32 {
                    finished += 1;
                }
            }
        }

        // 播完的声部就地移除：非循环且光标越过末尾，或空样本声部。
        self.voices.retain(|v| {
            if v.looped {
                let src_frames = v.wav.frames();
                return src_frames != 0; // 空样本的循环声部无处可绕，一并移除
            }
            let src_frames = v.wav.frames();
            !(src_frames == 0 || v.cursor >= src_frames as f32)
        });

        // 软削顶：f32 累加 × 总音量，clamp 回 i16（两个满幅相加饱和不回绕）。
        let master = self.master_volume;
        for (o, a) in out.iter_mut().zip(acc.iter()) {
            *o = (a * master).clamp(-32768.0, 32767.0) as i16;
        }
        finished
    }
}

/// 钳到 0..=1；NaN 按 0（最保守：无声）。
fn clamp01(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

// ------------------------------------------------------------ 单元测试
//
// 全部确定性、无设备：常量样本 + 精确断言。所有采样点都是 f32 精确可表示
// 的整数/半整数，因此断言用 `==`（不引入 epsilon 噪声）。

#[cfg(test)]
mod tests {
    use super::*;

    /// 单声道测试声音。
    fn mono(rate: u32, samples: &[i16]) -> Arc<Wav> {
        Arc::new(Wav { sample_rate: rate, channels: 1, samples: samples.to_vec() })
    }

    /// 立体声测试声音（交错帧）。
    fn stereo(rate: u32, samples: &[i16]) -> Arc<Wav> {
        Arc::new(Wav { sample_rate: rate, channels: 2, samples: samples.to_vec() })
    }

    #[test]
    fn t_mix01_same_rate_passthrough_sums() {
        let mut m = Mixer::new();
        // 源长 9 帧、缓冲 8 帧：混完不触底，声部都应存活。
        m.register("a", mono(44100, &[1000; 9]));
        m.register("b", mono(44100, &[500; 9]));
        m.play("a", 1.0, false).unwrap();
        m.play("b", 1.0, false).unwrap();
        let mut out = [0i16; 8];
        let finished = m.mix_into(&mut out, 44100, 1);
        assert_eq!(out, [1500; 8]);
        assert_eq!(finished, 0, "8 帧内两声部都未播完（源长 9 帧）");
        assert_eq!(m.active_voices(), 2);
    }

    #[test]
    fn t_mix02_clamp_saturates_both_polarity() {
        // 正极性：两个满幅相加 65534，必须饱和到 32767（不是回绕）。
        let mut pos = Mixer::new();
        pos.register("a", mono(44100, &[32767; 5]));
        pos.register("b", mono(44100, &[32767; 5]));
        pos.play("a", 1.0, false).unwrap();
        pos.play("b", 1.0, false).unwrap();
        let mut out = [0i16; 4];
        pos.mix_into(&mut out, 44100, 1);
        assert_eq!(out, [32767; 4]);

        // 负极性：两个负满幅相加 -65536，必须饱和到 -32768。
        let mut neg = Mixer::new();
        neg.register("c", mono(44100, &[-32768; 5]));
        neg.register("d", mono(44100, &[-32768; 5]));
        neg.play("c", 1.0, false).unwrap();
        neg.play("d", 1.0, false).unwrap();
        let mut out = [0i16; 4];
        neg.mix_into(&mut out, 44100, 1);
        assert_eq!(out, [-32768; 4]);
    }

    #[test]
    fn t_mix03_resample_step_is_rate_ratio() {
        // 44100 源在 22050 设备：步进 2.0/帧，取到源的第 0/2/4 帧。
        let mut m = Mixer::new();
        m.register("ramp", mono(44100, &[0, 100, 200, 300, 400, 500, 600, 700, 800, 900]));
        m.play("ramp", 1.0, false).unwrap();
        let mut out = [0i16; 3];
        m.mix_into(&mut out, 22050, 1);
        assert_eq!(out, [0, 200, 400]);
        assert_eq!(m.voices()[0].cursor, 6.0, "3 帧 × 2.0 步进");
    }

    #[test]
    fn t_mix04_linear_interpolation_halfway() {
        // 3000 源在 2000 设备：步进 1.5。第 2 帧落在源 1.5 处：100↔200 中点 = 150。
        let mut m = Mixer::new();
        m.register("ramp", mono(3000, &[0, 100, 200, 300, 400]));
        m.play("ramp", 1.0, false).unwrap();
        let mut out = [0i16; 2];
        m.mix_into(&mut out, 2000, 1);
        assert_eq!(out, [0, 150]);
        assert_eq!(m.voices()[0].cursor, 3.0);
    }

    #[test]
    fn t_mix05_looped_wraps_and_repeats() {
        let mut m = Mixer::new();
        m.register("tick", mono(8000, &[77; 10]));
        m.play("tick", 1.0, true).unwrap();
        let mut out = [0i16; 25];
        let finished = m.mix_into(&mut out, 8000, 1);
        assert_eq!(out, [77; 25], "10 帧循环声部填满 25 帧");
        assert_eq!(finished, 0, "循环声部永不播完");
        assert_eq!(m.active_voices(), 1);
        assert_eq!(m.voices()[0].cursor, 5.0, "25 mod 10 = 5，回绕后余量保留");
    }

    #[test]
    fn t_mix06_nonloop_finishes_and_is_removed() {
        let mut m = Mixer::new();
        m.register("s", mono(8000, &[10, 20, 30, 40, 50, 60, 70, 80, 90, 100]));
        m.play("s", 1.0, false).unwrap();
        let mut out = [0i16; 16];
        let finished = m.mix_into(&mut out, 8000, 1);
        assert_eq!(finished, 1, "本帧播完的声部数");
        assert_eq!(m.active_voices(), 0, "非循环到尾自动移除");
        assert_eq!(out[..10], [10, 20, 30, 40, 50, 60, 70, 80, 90, 100]);
        assert_eq!(out[10..], [0; 6], "到尾后余帧静音");
    }

    #[test]
    fn t_mix07_stereo_source_averaged_to_mono() {
        let mut m = Mixer::new();
        m.register("s", stereo(44100, &[800, 400, 800, 400]));
        m.play("s", 1.0, false).unwrap();
        let mut out = [0i16; 2];
        m.mix_into(&mut out, 44100, 1);
        assert_eq!(out, [600, 600], "(800+400)/2 = 600");
    }

    #[test]
    fn t_mix08_mono_source_copied_to_stereo() {
        let mut m = Mixer::new();
        m.register("s", mono(44100, &[700, 700]));
        m.play("s", 1.0, false).unwrap();
        let mut out = [0i16; 4];
        m.mix_into(&mut out, 44100, 2);
        assert_eq!(out, [700, 700, 700, 700], "单声道源两路复制");
    }

    #[test]
    fn t_mix09_stereo_passthrough_keeps_channels() {
        let mut m = Mixer::new();
        m.register("s", stereo(44100, &[100, -100, 200, -200]));
        m.play("s", 1.0, false).unwrap();
        let mut out = [0i16; 4];
        m.mix_into(&mut out, 44100, 2);
        assert_eq!(out, [100, -100, 200, -200]);
    }

    #[test]
    fn t_mix10_master_volume_scales_and_clamps() {
        let mut m = Mixer::new();
        m.register("a", mono(44100, &[1000; 4]));
        m.play("a", 1.0, false).unwrap();
        m.set_master_volume(0.5);
        assert_eq!(m.master_volume(), 0.5);
        let mut out = [0i16; 4];
        m.mix_into(&mut out, 44100, 1);
        assert_eq!(out, [500; 4]);

        m.set_master_volume(2.0);
        assert_eq!(m.master_volume(), 1.0, "越界总音量钳到 1");
        m.set_master_volume(-1.0);
        assert_eq!(m.master_volume(), 0.0, "越界总音量钳到 0");
    }

    #[test]
    fn t_mix11_voice_volume_clamped() {
        let mut m = Mixer::new();
        m.register("a", mono(44100, &[100; 2]));
        m.play("a", 2.0, false).unwrap();
        m.play("a", -0.5, false).unwrap();
        assert_eq!(m.voices()[0].volume, 1.0);
        assert_eq!(m.voices()[1].volume, 0.0);
        let mut out = [0i16; 2];
        m.mix_into(&mut out, 44100, 1);
        assert_eq!(out, [100, 100], "负音量声部完全无声");
    }

    #[test]
    fn t_mix12_unknown_key_reports_named_error() {
        let mut m = Mixer::new();
        let err = m.play("ghost", 1.0, false).unwrap_err();
        assert_eq!(err, MixerError::UnknownSound("ghost".into()));
        assert!(err.to_string().contains("ghost"), "Display 指名道姓");
        assert_eq!(m.active_voices(), 0);
    }

    #[test]
    fn t_mix13_stop_all_clears_voices_not_library() {
        let mut m = Mixer::new();
        m.register("a", mono(44100, &[1; 4]));
        m.play("a", 1.0, true).unwrap();
        m.stop_all();
        assert_eq!(m.active_voices(), 0);
        assert_eq!(m.list(), vec!["a"], "声音库不受 stop_all 影响");
    }

    #[test]
    fn t_mix14_empty_wav_finishes_immediately() {
        let mut m = Mixer::new();
        m.register("empty", mono(44100, &[]));
        m.register("empty_loop", mono(44100, &[]));
        m.play("empty", 1.0, false).unwrap();
        m.play("empty_loop", 1.0, true).unwrap();
        let mut out = [0i16; 4];
        let finished = m.mix_into(&mut out, 44100, 1);
        assert_eq!(finished, 2, "空样本声部（含循环）立即结束");
        assert_eq!(m.active_voices(), 0);
        assert_eq!(out, [0; 4]);
    }

    #[test]
    fn t_mix15_bad_device_params_mix_silence() {
        let mut m = Mixer::new();
        m.register("a", mono(44100, &[500; 4]));
        m.play("a", 1.0, true).unwrap();
        let mut out = [7i16; 4]; // 预填垃圾值：防御路径必须清零
        assert_eq!(m.mix_into(&mut out, 0, 1), 0, "设备采样率 0：不出声");
        assert_eq!(out, [0; 4]);
        assert_eq!(m.mix_into(&mut out, 44100, 3), 0, "设备声道 3：不出声");
        assert_eq!(out, [0; 4]);
        assert_eq!(m.active_voices(), 1, "防御路径不移除声部");
    }

    #[test]
    fn t_mix16_register_list_overwrites_and_sorts() {
        let mut m = Mixer::new();
        m.register("b", mono(44100, &[1]));
        m.register("a", mono(44100, &[2]));
        m.register("a", mono(22050, &[3])); // 同键覆盖
        assert_eq!(m.list(), vec!["a", "b"], "list 按字典序");
        assert!(m.has("a"));
        assert!(!m.has("c"));
    }

    #[test]
    fn t_mix17_voice_position_tracks_consumed_source_samples() {
        // 100 样本 @1000Hz 的"小 Wav"：无设备，纯 mix_into 手动推进。
        let mut m = Mixer::new();
        m.register("v", mono(1000, &[300; 100]));
        assert_eq!(m.voice_position("v"), None, "未播放：键无活动声部");
        m.play("v", 1.0, false).unwrap();
        assert_eq!(m.voice_position("v"), Some((0, 1000)), "开声即播：光标 0、源采样率如实带出");

        // 同率推进：1000Hz 设备吃 40 帧 -> 已播 40 源样本。
        let mut out = [0i16; 40];
        m.mix_into(&mut out, 1000, 1);
        assert_eq!(m.voice_position("v"), Some((40, 1000)));
        m.mix_into(&mut out, 1000, 1);
        assert_eq!(m.voice_position("v"), Some((80, 1000)));

        // 重采样口径：读数是**源样本**空间 —— 500Hz 设备步进 2.0/帧，
        // 10 设备帧 = 20 源样本（不是 10）。
        let mut r = Mixer::new();
        r.register("r", mono(1000, &[500; 100]));
        r.play("r", 1.0, false).unwrap();
        let mut half = [0i16; 10];
        r.mix_into(&mut half, 500, 1);
        assert_eq!(r.voice_position("r"), Some((20, 1000)), "光标单位 = 源样本");
        assert_eq!(m.voice_position("v"), Some((80, 1000)), "各自混音器互不串扰");
    }

    #[test]
    fn t_mix18_voice_position_none_after_finish_and_loop_wraps() {
        let mut m = Mixer::new();
        m.register("s", mono(1000, &[10; 100]));
        m.play("s", 1.0, false).unwrap();
        // 一次吃 120 帧（> 源长 100）：声部播完被移除 -> 读数归 None。
        let mut out = [0i16; 120];
        assert_eq!(m.mix_into(&mut out, 1000, 1), 1, "非循环到尾本帧移除");
        assert_eq!(m.voice_position("s"), None, "播完移除后键无活动声部");

        // stop_key 点名停同样归 None（音画同终判据的另一条来路）。
        m.register("k", mono(1000, &[10; 100]));
        m.play("k", 1.0, false).unwrap();
        assert!(m.voice_position("k").is_some());
        assert_eq!(m.stop_key("k"), 1);
        assert_eq!(m.voice_position("k"), None);

        // 循环声部永不移除：读数随回绕取模（10 帧源吃 25 帧 -> 光标 5）。
        m.register("loop", mono(1000, &[10; 10]));
        m.play("loop", 1.0, true).unwrap();
        let mut out = [0i16; 25];
        m.mix_into(&mut out, 1000, 1);
        assert_eq!(m.voice_position("loop"), Some((5, 1000)), "回绕后余量保留");
        assert_eq!(m.voice_position("ghost"), None, "未注册键如实 None");
    }
}
