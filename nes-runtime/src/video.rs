//! S15 视频资产面：Video 资源 -> 解析容器 -> 渲染侧同键逐帧覆写。
//!
//! # 架构裁决（三条，落地即本模块）
//!
//! 1. **视频纹理 = 同键逐帧覆写**：Video 资源在 [`AssetKind::Video`] 下绑定
//!    （`is_render_facing` = true），与纹理共用 `RenderAssetKey` 命名空间
//!    机制 —— 提取层 `RenderKeySource` 把 Sprite2D 的 texture 属性解析到
//!    同一个键，GPU 注册表 `register` 同键重复调用 = 覆写同一瓦片（字形页
//!    /纹理热重载已验证的路径）。运行中每帧把当前解码帧经同一键注册，
//!    Sprite 即显示"当前帧"。
//! 2. **确定性边界**：播放状态（计时/当前帧/音轨声部键）**全在本模块的
//!    渲染侧表里**（`NesRuntime.videos`）—— 不进树、不进语义指纹；脚本
//!    控制经 `Cmd::VideoPlay` / `Cmd::VideoStop`（树侧单帧缓冲 →
//!    `simulate` / `tick_headless` 消费，照 `PlaySound` 先例；headless
//!    消费即弃）。逐帧换页只在带 GPU 的帧路径（`frame_with` /
//!    `frame_windowed_with`）做 —— headless 无纹理，只推进 Cmd 消费。
//! 3. **音画严格同步 = 音频钟主控（S15.1，P0 起点对齐的升级）**：`video_play`
//!    时若有音轨则即刻解码进混音器并开声部（非循环）；**有音轨的视频帧号
//!    不再由帧差累计，而从混音器声部的已播采样位导出** ——
//!    `帧号 = floor(已播源样本 / 源采样率 × fps)`（[`video_frame_from_audio`]）。
//!    采样位由设备钟真实消耗推进（mix_into 逐帧吃光标），天然采样级对齐，
//!    免疫帧节拍抖动与起播缓冲延迟（声部队列未消耗前读数为 0 → 视频保持
//!    首帧，正是唇同步的正确起点）；声部播完移除（读数由 Some 变 None）=
//!    视频同步停播（音画同终）。**无音轨、或混音器不在场（headless / 未开
//!    音频）**则回退帧差累计钟（`floor(elapsed × fps)`，P0 既有行为逐位保留）。
//!    `video_stop` 即停声部（[`nes_audio::Mixer::stop_key`]）。
//!
//! # 内存口径（如实写明）
//!
//! 解析后的**整容器驻留内存**（`AmvVideo`/`AviVideo` 都在 parse 时拷贝整份
//! 文件字节），音轨解码产物按需缓存一份 `Arc<Wav>` —— 与音频面"全轨进
//! 内存"同一现状；压缩资源内存策略（流式/按需换页）归后续（S15 文档 §5）。

use std::cell::RefCell;
use std::sync::Arc;

use nes_asset::AssetKind;
use nes_media::{DecodedImage, MediaError, VideoInfo};
use nes_render_api::RenderAssetKey;
use nes_scene::{ResId, TableError};

use crate::{sound_key_of, BackendError, NesRuntime};

// ------------------------------------------------------------
// 容器：AMV / AVI 按内容探测
// ------------------------------------------------------------

/// 一个解析完成的视频容器。AMV 与 AVI 是同族 RIFF 变体：**按内容探测**
///（偏移 8..12 的四字码），不看扩展名 —— 与图片装载"内容说了算"同一口径。
pub(crate) struct VideoContainer {
    inner: VideoInner,
    /// 音轨解码缓存：`None` = 未解码过；`Some(None)` = 解码过、无音轨；
    /// `Some(Some(wav))` = 解码过的音轨（每次 `audio()` 零重复解码）。
    /// 容器只在宿主线程触碰（bind / 帧路径，从不跨线程）—— 内部可变性
    /// 用 `RefCell` 即可，不引 `Arc`（非 Send/Sync，运行侧表独占持有）。
    audio_cache: RefCell<Option<Option<Arc<nes_audio::Wav>>>>,
}

enum VideoInner {
    /// AMV 容器变体（S14.3：无头 MJPEG 帧合成 + IMA ADPCM 音轨）。
    Amv(nes_media::AmvVideo),
    /// AVI 1.0 单段（S14.2：DIB 手写 / MJPG 走 image；PCM 音轨）。
    Avi(nes_media::AviVideo),
}

impl VideoContainer {
    /// 从内存字节解析（只认字节流）。非 RIFF / 四字码未收录 =
    /// [`MediaError::UnsupportedFormat`]（解码失败原因随后续 parse 如实带出）。
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let inner = if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" {
            match &bytes[8..12] {
                b"AMV " => VideoInner::Amv(nes_media::AmvVideo::parse(bytes)?),
                b"AVI " => VideoInner::Avi(nes_media::AviVideo::parse(bytes)?),
                _ => return Err(MediaError::UnsupportedFormat),
            }
        } else {
            return Err(MediaError::UnsupportedFormat);
        };
        Ok(Self {
            inner,
            audio_cache: RefCell::new(None),
        })
    }

    /// 视频流元信息（宽高 / 帧率 / 帧数 / 编解码器）。
    pub fn video_info(&self) -> VideoInfo {
        match &self.inner {
            VideoInner::Amv(v) => v.video_info(),
            VideoInner::Avi(v) => v.video_info(),
        }
    }

    /// 解码第 `i` 帧（0 起）为 RGBA8。
    pub fn frame(&self, i: u32) -> Result<DecodedImage, MediaError> {
        match &self.inner {
            VideoInner::Amv(v) => v.frame(i),
            VideoInner::Avi(v) => v.frame(i),
        }
    }

    /// 音轨（首次调用解码并缓存；AMV 的 IMA ADPCM 走 nes-audio 的
    /// FFmpeg `ADPCM_IMA_AMV` 语义解码 —— S14.3 既有路径）。无音轨 =
    /// `Some(None)` 也缓存，避免反复探测。
    pub fn audio(&self) -> Option<Arc<nes_audio::Wav>> {
        if self.audio_cache.borrow().is_none() {
            let decoded = match &self.inner {
                VideoInner::Amv(v) => v.audio(),
                VideoInner::Avi(v) => v.audio(),
            };
            *self.audio_cache.borrow_mut() = Some(decoded.map(Arc::new));
        }
        self.audio_cache
            .borrow()
            .as_ref()
            .expect("上面刚填过缓存")
            .clone()
    }
}

// ------------------------------------------------------------
// 运行侧视频表
// ------------------------------------------------------------

/// 一个在播视频的渲染侧状态（**不进树、不进语义指纹** —— 确定性裁决）。
pub(crate) struct VideoPlaying {
    /// 帧差累计钟的经过时间（秒；帧循环 delta 累计）。只服务**回退路径**
    ///（无音轨 / 混音器缺席，见 [`NesRuntime::advance_videos`] 三态分叉）；
    /// 有音轨时帧号由音频钟导出，此字段仍照常累计（诊断一致性，零额外面）。
    pub elapsed: f32,
    /// 音轨混音器键（有音轨才有；`video_stop` / 声部播完移除时按它收口）。
    pub audio_key: Option<String>,
    /// 音频钟是否已见到过声部（`voice_position` 拿到过 `Some`）：true 之后
    /// 声部消失 = 非循环播完被移除 → 视频同步停播（音画同终）；false 时
    /// 声部缺席 = 混音器不在场 / 声部尚未开 → 回退帧差累计钟。
    pub audio_started: bool,
}

/// 一个已解析视频资源的运行侧条目。
pub(crate) struct VideoEntry {
    /// 解析容器（bind 时一次，热重载换版本时重建）。只在宿主线程触碰
    ///（bind / 帧路径），音轨缓存用 `RefCell` —— 不跨线程共享，无需 Arc。
    pub container: VideoContainer,
    /// GPU 纹理注册表键（与纹理同命名空间；提取层经 texture 属性解析到它）。
    pub render_key: RenderAssetKey,
    /// 派生键（= 资源路径去扩展名，与声音键同一推导单点）—— 脚本
    /// `video_play` / 宿主 `play_video` 按它引用。
    pub key: String,
    /// 已解析的资产版本（热重载重解析判定；与纹理上传账目同构）。
    pub version: u32,
    /// 已上传到 GPU 的页帧号（-1 = 尚未上传 —— headless 恒此值）。
    pub page_frame: i64,
    /// 播放状态（`None` = 停止；起播时计时清零）。
    pub playing: Option<VideoPlaying>,
}

/// 视频音轨的混音器键（与场景声音键空间隔离：前缀不会是合法资源路径的
/// 形态，路径键无碰撞纪律不受扰）。
fn video_audio_key(video_key: &str) -> String {
    format!("__video_audio__/{video_key}")
}

/// 音频钟换算（纯函数，单测面）：已播源样本位 → 视频帧号。
///
/// `已播秒数 = pos / rate`，`帧号 = floor(已播秒数 × fps)`。f64 中间量：
/// 长视频的样本位（282s @ 22050Hz ≈ 6.2M）虽在 f32 尾数内，f64 让口径
/// 不依赖这个巧合。`rate == 0` 或 `fps <= 0`（病态头）返回 0 —— 帧号停
/// 首帧，与帧差钟的病态口径一致；调用方再按容器帧数钳末帧。
pub fn video_frame_from_audio(pos: u64, rate: u32, fps: f32) -> u32 {
    if rate == 0 || fps <= 0.0 {
        return 0;
    }
    let seconds = pos as f64 / f64::from(rate);
    let frame = (seconds * f64::from(fps)).floor();
    if frame <= 0.0 { 0 } else { frame as u32 }
}

impl NesRuntime {
    // ---------- 装载链（声明 / bind 解析 / 首帧上传）----------

    /// 声明一个视频资源（记录槽位，供 bind 解析遍历；与
    /// [`crate::NesRuntime::declare_texture`] / `declare_sound` 同构）。
    pub fn declare_video(&mut self, path: &str) -> Result<ResId, TableError> {
        let id = self.table.declare(path, AssetKind::Video)?;
        self.video_slots.push(id);
        Ok(id)
    }

    /// 把就绪且**版本变化**的 Video 资源解析进运行侧视频表（bind 的
    /// 视频步；照 `register_pending_sounds` 的账目家法）。
    ///
    /// * 解析成功：容器入表 + 派生键 + GPU 首帧（第 0 帧）立即上传
    ///   （未起播时 Sprite 显示首帧而不是黑块）；有音轨则顺带解码缓存，
    ///   混音器在场时按音轨键登记（`video_play` 时开声部）。
    /// * 解析失败**不中断**：指名路径的失败清单交调用方（`bind_assets`
    ///   挂进缺口 —— 一个坏视频不挡其他资源，与声音解码缺口同律）。
    /// * 热重载：注册表版本变化 → 重建容器 + 重传首帧 + 停止旧播放
    ///   （旧容器即刻失效 —— 资产换了，续播没有意义）。
    ///
    /// GPU 在场才上传首帧；headless 只解析（无纹理可上传，页账目停在 -1）。
    pub(crate) fn parse_videos_collecting(&mut self) -> Vec<(ResId, String)> {
        let mut failures: Vec<(ResId, String)> = Vec::new();
        let slots: Vec<ResId> = self.video_slots.clone();
        for id in slots {
            let (asset_key, path_text, version, bytes) = {
                let Some(entry) = self.table.entry(id) else {
                    continue;
                };
                let Some(key) = entry.key() else {
                    continue; // 未绑定（还没 bind）
                };
                let Some(loaded) = self.registry.loaded(key) else {
                    continue; // 未就绪 / 加载失败（状态留在表里，宿主可重试）
                };
                let path_text = entry
                    .path()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| format!("slot {}", id.get()));
                (key, path_text, loaded.version, loaded.bytes.clone())
            };
            // 版本账目：同一版本已解析过就跳过（bind 幂等 + 热重载重解析判定）。
            if self.videos.get(&id).map(|v| v.version) == Some(version) {
                continue;
            }
            // 解析容器（内容探测：AMV / AVI）。
            let container = match VideoContainer::parse(&bytes) {
                Ok(c) => c,
                Err(e) => {
                    failures.push((id, format!("视频 {path_text} 解析失败：{e}")));
                    continue;
                }
            };
            // 派生键（路径去扩展名 —— 与声音键同一推导单点）。
            let key_text = sound_key_of(&path_text);
            // 渲染键：Video 类是渲染面（is_render_facing），键位与纹理同源。
            let Some(view) = asset_key.as_render_key() else {
                failures.push((id, format!("视频 {path_text} 缺渲染键（Video 类应是渲染面）")));
                continue;
            };
            let render_key = RenderAssetKey::from_bits(view.to_bits());
            // 音轨：解码一次进缓存；混音器在场即刻登记（video_play 只开声部）。
            let audio = container.audio();
            if let Some(wav) = &audio {
                if let Some(mixer) = &self.mixer {
                    let mut guard =
                        mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    guard.register(video_audio_key(&key_text), Arc::clone(wav));
                }
            }
            let mut entry = VideoEntry {
                container,
                render_key,
                key: key_text,
                version,
                page_frame: -1,
                playing: None, // 换版本重建：旧播放状态一并作废（资产换了）
            };
            // GPU 首帧（第 0 帧）立即上传 —— 未起播时 Sprite 显示首帧。
            if let Some(consumer) = self.consumer.as_mut() {
                match Self::upload_video_frame(consumer, &mut entry, 0) {
                    Ok(()) => self.video_swaps += 1,
                    Err(e) => {
                        failures.push((id, format!("视频 {path_text} 首帧上传失败：{e}")));
                        continue;
                    }
                }
            }
            self.videos.insert(id, entry);
        }
        failures
    }

    /// 把指定帧解码并经**同键覆写**注册进 GPU 注册表（换页的单点实现）。
    /// 自由函数（见文件底部）：消费器/条目/计数是三个不相交的借用面，
    /// 按"只读规划 → 换页 → 收款"分段借 —— 不经 `&mut self` 打包。
    fn upload_video_frame(
        consumer: &mut nes_render_wgpu::CommandConsumer,
        entry: &mut VideoEntry,
        frame_no: u32,
    ) -> Result<(), BackendError> {
        let img = entry.container.frame(frame_no).map_err(|e| {
            BackendError::Io(format!("视频 {} 第 {frame_no} 帧解码失败：{e}", entry.key))
        })?;
        consumer.register_texture(entry.render_key, img.width, img.height, &img.rgba)?;
        entry.page_frame = frame_no as i64;
        Ok(())
    }

    // ---------- 播放控制（宿主直调 + Cmd 消费共用单点）----------

    /// 开始播放一个已声明视频（宿主直调 API；脚本 `video_play` 的 Cmd
    /// 最终也落到这里）。已是播放态 = 幂等（计时不清零，照"若未播放
    /// 则起播"的口径）；键未声明 = 返回 `false`（静默丢弃面由调用方定）。
    ///
    /// 起播语义（音频钟主控的同步起点）：计时清零 + `audio_started = false`；
    /// 有音轨则混音器在场时开一个**非循环**声部（未登记键先从缓存登记 ——
    /// 晚开音频的补注册路径）。此后帧号由声部已播采样位导出：设备把声部
    /// 队列真实消耗之前读数为 0，视频保持首帧 —— 起播缓冲延迟不再撕开
    /// 音画错位。混音器不在场（headless / 未开音频）视频照播、只是无声，
    /// 步进回退帧差累计钟 —— 与 `play` 未开音频的丢弃语义同家法。
    pub fn play_video(&mut self, key: &str) -> bool {
        let Some(id) = self.videos.iter().find(|(_, e)| e.key == key).map(|(id, _)| *id)
        else {
            return false; // 键未声明：与 play 未注册键同口径（调用方决定报行）
        };
        // ① 只读判定（借用随即结束）：是否已在播、有无音轨、音轨键。
        let (already, audio_key) = {
            let Some(entry) = self.videos.get(&id) else {
                return false;
            };
            let audio_key = if entry.playing.is_some() {
                None
            } else {
                entry.container.audio().map(|_| video_audio_key(&entry.key))
            };
            (entry.playing.is_some(), audio_key)
        };
        if already {
            return true; // 幂等：已在播（不重启、计时不清零）
        }
        // ② 开声部（&mut self 自成一节；起点对齐 = 此刻）。
        if let Some(ak) = &audio_key {
            self.start_video_voice(ak);
        }
        // ③ 落播放态。
        let Some(entry) = self.videos.get_mut(&id) else {
            return false;
        };
        entry.playing = Some(VideoPlaying {
            elapsed: 0.0,
            audio_key,
            audio_started: false,
        });
        true
    }

    /// 停止播放（宿主直调 API；脚本 `video_stop` 的 Cmd 最终也落到这里）。
    /// 停计时 + 停音轨声部；当前页保持在停那一刻的帧（画面定格 ——
    /// 与"视频资源显示当前帧"的语义一致）。键未声明/未在播 = `false`。
    pub fn stop_video(&mut self, key: &str) -> bool {
        let Some(id) = self.videos.iter().find(|(_, e)| e.key == key).map(|(id, _)| *id)
        else {
            return false;
        };
        // ① 只读判定：是否在播、音轨键。
        let (was, audio_key) = {
            let Some(entry) = self.videos.get(&id) else {
                return false;
            };
            match &entry.playing {
                Some(p) => (true, p.audio_key.clone()),
                None => (false, None),
            }
        };
        if !was {
            return false; // 未在播：幂等无害
        }
        // ② 停播态 + 停声部（各自成节，借用不相交）。
        if let Some(entry) = self.videos.get_mut(&id) {
            entry.playing = None;
        }
        if let Some(ak) = audio_key {
            self.stop_video_voice(&ak);
        }
        true
    }

    /// 开视频音轨声部（混音器在场才出声；未登记键先从容器缓存登记）。
    fn start_video_voice(&mut self, audio_key: &str) {
        let Some(mixer) = &self.mixer else {
            return; // 未开音频：无声播放（headless 语义）
        };
        let mut guard = mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !guard.has(audio_key) {
            // 晚开音频的补注册：bind 时混音器缺席、音轨只进了缓存。
            // 反查容器缓存拿解码产物（音轨键即由容器派生，回查安全）。
            let wav = self
                .videos
                .values()
                .find(|e| video_audio_key(&e.key) == audio_key)
                .and_then(|e| e.container.audio());
            if let Some(wav) = wav {
                guard.register(audio_key, wav);
            }
        }
        // 音量 1.0、播一遍（非循环 —— 与 play 语句同口径）。
        let _ = guard.play(audio_key, 1.0, false);
    }

    /// 停视频音轨声部（按音轨键点名停 —— 不碰其他声音的声部）。
    fn stop_video_voice(&mut self, audio_key: &str) {
        if let Some(mixer) = &self.mixer {
            let mut guard =
                mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.stop_key(audio_key);
        }
    }

    /// 读视频音轨声部的已播源样本位（音频钟读数；只读面）。
    ///
    /// 混音器不在场（headless / 未开音频）= `None` —— 调用方据此走帧差
    /// 累计钟回退；声部不存在（未开/已播完移除）也是 `None`，两态由
    /// [`VideoPlaying::audio_started`] 区分（见 `advance_videos` 三态分叉）。
    fn video_voice_position(&self, audio_key: &str) -> Option<(u64, u32)> {
        let mixer = self.mixer.as_ref()?;
        let guard = mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.voice_position(audio_key)
    }

    /// 把脚本 `video_play` / `video_stop` 落地的命令转交播放状态机
    /// （tick 后的 Cmd 消费点，照 `consume_played_sounds` 先例）。
    ///
    /// 消费纪律：
    /// - **每次 tick 后取走**（`simulate` 与 `tick_headless` 两处入口）——
    ///   缓冲即取即清，不跨帧积压；
    /// - 未声明的键：静默丢弃（视频不是语义状态，坏键不崩帧；与
    ///   `PlaySound` 未注册键的静默口径同家法）；
    /// - headless 消费即弃：播放状态建在渲染侧表里，无 GPU 时换页不做、
    ///   混音器通常也不在场 —— 命令消费本身零渲染副作用（确定性不受扰）。
    pub(crate) fn consume_video_cmds(&mut self) {
        let cmds = self.tree.take_video_cmds();
        if cmds.is_empty() {
            return;
        }
        for cmd in cmds {
            match cmd {
                nes_scene::VideoCmd::Play { key } => {
                    self.play_video(&key);
                }
                nes_scene::VideoCmd::Stop { key } => {
                    self.stop_video(&key);
                }
            }
        }
    }

    // ---------- 逐帧推进（只在带 GPU 的帧路径调用）----------

    /// 推进全部在播视频：定帧号 → 同键换页。帧号按**三态分叉**裁定：
    ///
    /// | 态 | 条件 | 帧号来源 | 终局判据 |
    /// |---|---|---|---|
    /// | 主路径 | 有音轨且声部在场 | `floor(已播源样本 / 源采样率 × fps)`（音频钟） | 声部移除 → 停播（音画同终） |
    /// | 同终 | 有音轨、声部开过后消失 | 钳末帧上屏 | 本帧即停播 + 停声 |
    /// | 回退 | 无音轨 / 混音器不在场（headless、未开音频） | `floor(elapsed × fps)`（帧差累计钟，P0 既有行为逐位保留） | 目标帧越界 → 钳末帧 + 停播 |
    ///
    /// * 音频钟的读数 = 声部光标（设备真实消耗的源样本位）—— 采样级对齐，
    ///   免疫帧节拍抖动与起播缓冲延迟：声部队列未消耗前读数 0 → 视频保持
    ///   首帧（唇同步起点）；画面先于音轨走完时钳末帧定格，等声部收尾才
    ///   停播（终点以声部移除为主判据）；
    /// * fps ≤ 0（病态头）：回退钟帧号停 0（静帧播放，直到 stop）；音频钟
    ///   帧号由 [`video_frame_from_audio`] 同口径兜 0；
    /// * 空流病态（帧数 ≤ 0，parse 已保证至少一帧，防御而已）：当帧停播；
    /// * 目标帧 == 已上页帧：跳过（同帧不重解码不重上传）；
    /// * 帧解码失败：如实上抛（与纹理解码失败同律 —— 指名道姓的
    ///   `BackendError`，不静默；宿主决定报行/降级）。
    ///
    /// 换页只在**本方法**发生，而本方法只在 `frame_with` /
    /// `frame_windowed_with`（带 GPU 的帧路径）被调 —— headless 只推进
    /// Cmd 消费，无 GPU 无换页（架构裁决 2）。
    ///
    /// 借用结构（三段式）：① 只读规划（表共享借用 + 混音器只读锁，随即
    /// 结束）→ ② 换页（`videos` / `consumer` / `video_swaps` 三个不相交
    /// 字段的可变借用，[`Self::upload_video_frame`] 不经 `&mut self`）→
    /// ③ 收款（停播态 + 停音轨声部 + 回写累计时间/已见声部旗标）。
    pub(crate) fn advance_videos(&mut self, delta: f32) -> Result<(), BackendError> {
        let delta = delta.max(0.0);
        let mut finished: Vec<Option<String>> = Vec::new();
        let ids: Vec<ResId> = self
            .videos
            .iter()
            .filter(|(_, e)| e.playing.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            // ① 只读规划：三态分叉定帧号与终局。
            let (elapsed, target, ended, voice_alive, audio_key) = {
                let Some(entry) = self.videos.get(&id) else {
                    continue;
                };
                let Some(playing) = &entry.playing else {
                    continue;
                };
                let info = entry.container.video_info();
                let frame_count = info.frame_count as i64;
                let elapsed = playing.elapsed + delta;
                let audio_key = playing.audio_key.clone();
                // 音频钟读数：该键活动声部的已播源样本位（混音器缺席 = None）。
                let voice = audio_key
                    .as_deref()
                    .and_then(|ak| self.video_voice_position(ak));
                let (target, ended, voice_alive) = if frame_count <= 0 {
                    // 空流病态（parse 已保证至少一帧，防御而已）：当帧停播。
                    (0i64, true, false)
                } else {
                    let last = frame_count - 1;
                    match voice {
                        // 主路径（音频钟主控）：帧号随声部已播采样位走。
                        Some((pos, rate)) => {
                            let raw = video_frame_from_audio(pos, rate, info.fps) as i64;
                            (raw.min(last), false, true)
                        }
                        // 同终：声部开过后消失 = 非循环播完被移除 —— 画面钳
                        // 末帧上屏、本帧停播（音画同终的单一判据）。
                        None if playing.audio_started => (last, true, false),
                        // 回退（无音轨 / 混音器缺席 / 声部未开）：帧差累计钟
                        // —— P0 既有行为逐位保留（headless 与未开音频语义）。
                        None => {
                            let raw = if info.fps > 0.0 {
                                (elapsed * info.fps).floor() as i64
                            } else {
                                0
                            };
                            if raw >= frame_count {
                                (last, true, false)
                            } else {
                                (raw, false, false)
                            }
                        }
                    }
                };
                (elapsed, target, ended, voice_alive, audio_key)
            };
            // ② 换页：目标帧 != 已上页帧才解码重传（同帧跳过）。
            let page = self.videos.get(&id).map(|e| e.page_frame).unwrap_or(-1);
            if target != page {
                if let (Some(entry), Some(consumer)) =
                    (self.videos.get_mut(&id), self.consumer.as_mut())
                {
                    Self::upload_video_frame(consumer, entry, target as u32)?;
                    self.video_swaps += 1;
                }
                // 无消费端（理论上到不了：本方法只在 GPU 帧路径被调）：
                // 不换页也不失败 —— 换页是无 GPU 时的无害缺席。
            }
            // ③ 收款：到终局 → 停播态 + 停音轨声部；否则回写累计时间与
            // 已见声部旗标（音频钟从此接管帧号 —— 同终判据就此武装）。
            if ended {
                if let Some(entry) = self.videos.get_mut(&id) {
                    entry.playing = None;
                }
                finished.push(audio_key);
            } else if let Some(entry) = self.videos.get_mut(&id) {
                if let Some(playing) = &mut entry.playing {
                    playing.elapsed = elapsed;
                    playing.audio_started |= voice_alive;
                }
            }
        }
        // 终局收口：声部通常已自然播完 —— stop_key 幂等 0；显式停是"末帧
        // 即静"的语义兜底（音轨比画面长的容器不残留背景声；回退钟末帧
        // 越界路径同样经此收口）。
        for ak in finished.into_iter().flatten() {
            self.stop_video_voice(&ak);
        }
        Ok(())
    }

    // ---------- 诊断面（测试/宿主观测用）----------

    /// 视频换页累计数（含 bind 首帧上传；诊断/测试观测面 —— "换页发生"
    /// 的可观测口径）。
    pub fn video_page_swaps(&self) -> u64 {
        self.video_swaps
    }

    /// 已解析进运行侧表的视频条目数（bind 后非零；场景替换清零重记 ——
    /// 与 `uploaded_texture_count` / `registered_sound_count` 同构）。
    pub fn video_count(&self) -> usize {
        self.videos.len()
    }

    /// 指定键的视频是否在播（诊断/测试用）。
    pub fn video_is_playing(&self, key: &str) -> bool {
        self.videos
            .values()
            .any(|e| e.key == key && e.playing.is_some())
    }

    /// 指定键的视频当前页帧号（诊断/测试用；未声明 = `None`）。
    pub fn video_current_frame(&self, key: &str) -> Option<u32> {
        self.videos
            .values()
            .find(|e| e.key == key)
            .map(|e| e.page_frame.max(0) as u32)
    }

    /// 混音器当前活动声部数（诊断/测试用；未开音频 = 0）。
    pub fn active_voice_count(&self) -> usize {
        let Some(mixer) = &self.mixer else {
            return 0;
        };
        let guard = mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.active_voices()
    }
}

// ------------------------------------------------------------ 单元测试
//
// 三块面：
// 1. 纯函数换算（零依赖，逐位断言）；
// 2. 回退钟回归（混音器缺席 —— headless 直驱 advance_videos，无 GPU 也走
//    得通：换页缺席是"无害缺席"，规划/终局逻辑照常在走）；
// 3. 音频钟三态分叉（注入一个**无设备**混音器 —— mix_into 手动推进，全程
//    确定性；GPU 在场时还能断言页帧号逐位跟随采样位）。
//
// 真实 AMV（skip-if-missing，与 tests/s15_video.rs 同一资产口径）：13.5MB
// @ 15fps + IMA ADPCM 音轨，在用户机器上、不入仓库。

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use nes_audio::Mixer;

    use super::*;

    /// 真实 AMV 测试资产（用户机器；不在仓库 —— 缺失即跳过）。
    const SOURCE_AMV: &str = "C:/Users/Administrator/Videos/text/spider_amv.amv";
    /// 场景里声明的视频资产路径（拷入临时根后的形态）。
    const VIDEO_REL: &str = "Media/spider.amv";
    /// 派生键。
    const VIDEO_KEY: &str = "Media/spider";

    /// 每用例独立临时资产根（计数器 + 进程号保证并行唯一）。
    fn assets_root(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "nes_s15_video_sync_{tag}_{}_{}",
            n,
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("Media")).expect("create temp asset root");
        dir
    }

    /// 装配一个"已 bind 的 headless 运行时 + 注入的无设备混音器"。
    /// 返回 (runtime, fps, 帧数, 音轨采样率, 音轨声道数)。
    fn headless_with_injected_mixer(
        tag: &str,
        bytes: &[u8],
    ) -> (NesRuntime, f32, i64, u32, u16) {
        let root = assets_root(tag);
        std::fs::write(root.join(VIDEO_REL), bytes).expect("seed video");
        let mut rt = NesRuntime::open_headless(&root).expect("headless assemble");
        // 设备未开的混音器注入：声部光标只被我们手动 mix_into 推进 ——
        // 全程确定性（真实设备线程才是"每隔一会调一次 mix_into"的那个）。
        rt.mixer = Some(std::sync::Arc::new(std::sync::Mutex::new(Mixer::new())));
        rt.declare_video(VIDEO_REL).expect("declare video");
        let report = rt.bind_assets();
        assert!(report.is_clean(), "bind clean: {report:?}");
        let (fps, frame_count, rate, channels) = {
            let entry = rt.videos.values().next().expect("video in table");
            let info = entry.container.video_info();
            let wav = entry.container.audio().expect("AMV has audio track");
            (info.fps, info.frame_count as i64, wav.sample_rate, wav.channels)
        };
        (rt, fps, frame_count, rate, channels)
    }

    // ---------- 1. 纯函数换算 ----------

    #[test]
    fn t_vfa01_frame_is_floor_seconds_times_fps() {
        // 1s @22050Hz、15fps -> 15；1/3s -> 5；半帧不及 -> floor。
        assert_eq!(video_frame_from_audio(0, 22050, 15.0), 0, "零位 = 首帧");
        assert_eq!(video_frame_from_audio(22050, 22050, 15.0), 15);
        assert_eq!(video_frame_from_audio(7350, 22050, 15.0), 5, "1/3s x 15");
        assert_eq!(video_frame_from_audio(1000, 1000, 30.0), 30);
        assert_eq!(video_frame_from_audio(1, 1000, 15.0), 0, "不足一帧 floor 到 0");
        assert_eq!(video_frame_from_audio(999, 1000, 1000.0), 999, "每样本一帧的极端口径");
    }

    #[test]
    fn t_vfa02_long_video_precision_and_degenerate_heads() {
        // 2500s @22050Hz、15fps：f64 中间量下无精度丢失（37500 帧）。
        assert_eq!(video_frame_from_audio(55_125_000, 22050, 15.0), 37_500);
        assert_eq!(video_frame_from_audio(1000, 0, 15.0), 0, "采样率 0 病态兜 0");
        assert_eq!(video_frame_from_audio(1000, 1000, 0.0), 0, "fps 0 病态兜 0");
        assert_eq!(video_frame_from_audio(1000, 1000, -5.0), 0, "负 fps 同病态口径");
    }

    // ---------- 2. 回退钟回归（混音器缺席 = headless / 未开音频语义）----------

    #[test]
    fn t_vid_sync01_fallback_elapsed_clock_without_mixer() {
        let Some(bytes) = std::fs::read(SOURCE_AMV).ok() else {
            println!("[skip] real AMV not found (user machine asset): {SOURCE_AMV}");
            return;
        };
        let root = assets_root("fb");
        std::fs::write(root.join(VIDEO_REL), &bytes).expect("seed video");
        let mut rt = NesRuntime::open_headless(&root).expect("headless assemble");
        assert!(rt.mixer.is_none(), "headless 默认无混音器");
        rt.declare_video(VIDEO_REL).expect("declare video");
        let report = rt.bind_assets();
        assert!(report.is_clean(), "bind clean: {report:?}");
        let (fps, frame_count) = {
            let entry = rt.videos.values().next().expect("video in table");
            let info = entry.container.video_info();
            (info.fps, info.frame_count as i64)
        };
        assert!(rt.play_video(VIDEO_KEY), "play");
        // 声部从未被见过（混音器缺席）：回退钟帧差累计 —— 累计时间照走、
        // 不因"音频钟拿不到读数"而停摆，也不误判音画同终。
        let frame = 1.0 / 60.0;
        for _ in 0..45 {
            rt.advance_videos(frame).expect("advance");
        }
        {
            let entry = rt.videos.values().next().expect("video in table");
            let playing = entry.playing.as_ref().expect("still playing");
            assert!(!playing.audio_started, "混音器缺席：声部从未在场");
            assert!(
                (playing.elapsed - 45.0 * frame).abs() < 1e-4,
                "回退钟累计 45 帧 delta：{}",
                playing.elapsed
            );
        }
        assert!(rt.video_is_playing(VIDEO_KEY), "45 帧内长视频仍在播");
        // 末帧越界（delta 巨大）→ 钳末帧停播（回退钟终局判据，P0 既有）。
        rt.advance_videos(10_000.0).expect("advance");
        assert!(!rt.video_is_playing(VIDEO_KEY), "越界即停播（回退钟判据）");
        assert!(frame_count > 0 && fps > 0.0, "真实 AMV 元信息健全");

    }

    // ---------- 3. 音频钟三态分叉（注入无设备混音器，确定性推进）----------

    #[test]
    fn t_vid_sync02_audio_clock_holds_first_frame_then_same_end() {
        let Some(bytes) = std::fs::read(SOURCE_AMV).ok() else {
            println!("[skip] real AMV not found (user machine asset): {SOURCE_AMV}");
            return;
        };
        let (mut rt, _fps, _frames, rate, channels) =
            headless_with_injected_mixer("hold", &bytes);
        let audio_key = video_audio_key(VIDEO_KEY);
        assert!(rt.play_video(VIDEO_KEY), "play");
        assert_eq!(rt.active_voice_count(), 1, "音轨声部已开（无设备，光标 0）");

        // 主路径起点：声部未被消耗 -> 读数 0 -> 帧号 0。巨大 delta 下帧差钟
        // 早已越界停播，音频钟却按"耳朵听到的位置"保持首帧 —— 分叉判据的
        // 决定性对照（起点对齐 + 起播缓冲延迟免疫，同一机制）。
        rt.advance_videos(10_000.0).expect("advance");
        assert!(rt.video_is_playing(VIDEO_KEY), "声部未消耗：视频保持首帧不停播");
        {
            let entry = rt.videos.values().next().expect("video in table");
            let playing = entry.playing.as_ref().expect("still playing");
            assert!(playing.audio_started, "见到声部后 audio_started 武装");
            assert_eq!(playing.elapsed, 10_000.0, "累计照走（诊断面）");
        }

        // 手动消费半秒源样本（mix_into 纯函数推进 —— 无设备确定性）：仍在播。
        let half = (rate as usize / 2) * usize::from(channels);
        {
            let mixer = std::sync::Arc::clone(rt.mixer.as_ref().expect("injected"));
            let mut guard = mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut buf = vec![0i16; half];
            guard.mix_into(&mut buf, rate, channels);
        }
        rt.advance_videos(1.0 / 60.0).expect("advance");
        assert!(rt.video_is_playing(VIDEO_KEY), "半秒声部消耗后仍在播");

        // 同终：声部消失（此处用 stop_key 点名移除模拟"播完移除" —— 分叉
        // 只认"开过后消失"这一事实）→ 本帧停播，音画同终。
        {
            let mixer = std::sync::Arc::clone(rt.mixer.as_ref().expect("injected"));
            let mut guard = mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(guard.stop_key(&audio_key), 1, "点名移除音轨声部");
        }
        rt.advance_videos(1.0 / 60.0).expect("advance");
        assert!(!rt.video_is_playing(VIDEO_KEY), "声部移除 = 视频同步停播");
        assert_eq!(rt.active_voice_count(), 0, "停播收口后零声部");

    }

    #[test]
    fn t_vid_sync03_gpu_page_frames_follow_audio_clock() {
        let Some(bytes) = std::fs::read(SOURCE_AMV).ok() else {
            println!("[skip] real AMV not found (user machine asset): {SOURCE_AMV}");
            return;
        };
        // GPU 缺席如实跳过（s14/s15 同款惯例）：页帧号只有 GPU 换页可观测。
        let root = assets_root("gpu");
        std::fs::write(root.join(VIDEO_REL), &bytes).expect("seed video");
        let mut rt = match NesRuntime::open_with_root(&root, 320, 240) {
            Ok(rt) => rt,
            Err(_) => {
                eprintln!("[skip] no GPU backend: audio-clock page-swap path skipped");
                return;
            }
        };
        rt.mixer = Some(std::sync::Arc::new(std::sync::Mutex::new(Mixer::new())));
        rt.declare_video(VIDEO_REL).expect("declare video");
        let report = rt.bind_assets();
        assert!(report.is_clean(), "bind clean: {report:?}");
        let (fps, _frames, rate, channels) = {
            let entry = rt.videos.values().next().expect("video in table");
            let info = entry.container.video_info();
            let wav = entry.container.audio().expect("AMV has audio track");
            (info.fps, info.frame_count as i64, wav.sample_rate, wav.channels)
        };
        assert_eq!(rt.video_current_frame(VIDEO_KEY), Some(0), "bind 首帧");
        assert!(rt.play_video(VIDEO_KEY), "play");

        // 声部零消耗：音频钟帧号 0 == 已上页帧 -> 不换页（swaps 不动）。
        rt.advance_videos(1.0 / 60.0).expect("advance");
        assert_eq!(rt.video_page_swaps(), 1, "首帧已上，零消耗不重传");
        assert_eq!(rt.video_current_frame(VIDEO_KEY), Some(0), "保持首帧（唇同步起点）");

        // 消费整整 1 秒源样本 -> advance：页帧号 = floor(1s x fps)。
        // 同期帧差钟只累计了 2 个 1/60 delta（≈ 0.5 帧 -> 0）—— 页号跟的
        // 是采样位不是帧差，分叉在此逐位取证。
        let consume_seconds = 1.0f32;
        {
            let mixer = std::sync::Arc::clone(rt.mixer.as_ref().expect("injected"));
            let mut guard = mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let n = (rate as usize * consume_seconds as usize) * usize::from(channels);
            let mut buf = vec![0i16; n];
            guard.mix_into(&mut buf, rate, channels);
        }
        rt.advance_videos(1.0 / 60.0).expect("advance");
        let expected = video_frame_from_audio(
            (rate as f64 * f64::from(consume_seconds)) as u64,
            rate,
            fps,
        );
        assert!(expected > 0, "1s @15fps 至少 1 帧");
        assert_eq!(
            rt.video_current_frame(VIDEO_KEY),
            Some(expected),
            "页帧号 = floor(已播样本/采样率 x fps)"
        );
        assert_eq!(rt.video_page_swaps(), 2, "恰好一次新换页");

        // 再吃 1 秒：帧号走 floor(2s x fps)，帧差钟口径下这不可能（才 ~1 帧）。
        {
            let mixer = std::sync::Arc::clone(rt.mixer.as_ref().expect("injected"));
            let mut guard = mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let n = (rate as usize * consume_seconds as usize) * usize::from(channels);
            let mut buf = vec![0i16; n];
            guard.mix_into(&mut buf, rate, channels);
        }
        rt.advance_videos(1.0 / 60.0).expect("advance");
        let expected2 = video_frame_from_audio(rate as u64 * 2, rate, fps);
        assert_eq!(
            rt.video_current_frame(VIDEO_KEY),
            Some(expected2),
            "页帧号继续跟随采样位"
        );
        assert!(rt.video_is_playing(VIDEO_KEY), "长视频远未到头");

    }
}
