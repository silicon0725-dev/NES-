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
//! 3. **音画同步 P0 = 起点对齐**：`video_play` 时若有音轨则即刻解码进
//!    混音器并开声部（非循环）；帧号 = `floor(播放经过时间 × fps)`，经过
//!    时间由帧循环 delta 累计。音轨播完或 `video_stop` 即停声部
//!    （[`nes_audio::Mixer::stop_key`]）。采样级严格同步归后续里程碑。
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
    /// 播放经过时间（秒；帧循环 delta 累计）。帧号 = `floor(elapsed × fps)`。
    pub elapsed: f32,
    /// 音轨混音器键（有音轨才有；`video_stop` / 自然播完时按它停声部）。
    pub audio_key: Option<String>,
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
    /// 起播语义（P0 音画同步 = 起点对齐）：计时清零；有音轨则混音器
    /// 在场时开一个**非循环**声部（未登记键先从缓存登记 —— 晚开音频的
    /// 补注册路径）。混音器不在场（headless / 未开音频）视频照播、只是
    /// 无声 —— 与 `play` 未开音频的丢弃语义同家法。
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

    /// 推进全部在播视频：累计经过时间 → 目标帧号 → 同键换页。
    ///
    /// * 帧号 = `floor(elapsed × fps)`（fps 来自容器元信息；fps 不可用
    ///   （≤0，病态头）时帧号停在 0 —— 静帧播放，直到 stop）；
    /// * 目标帧越界（≥ 帧数）：钳到末帧并**停播**（P0 不做 loop）——
    ///   末帧照常换页上屏（画面定格在末帧），音轨声部一并停掉；
    /// * 目标帧 == 已上页帧：跳过（同帧不重解码不重上传）。
    /// * 帧解码失败：如实上抛（与纹理解码失败同律 —— 指名道姓的
    ///   `BackendError`，不静默；宿主决定报行/降级）。
    ///
    /// 换页只在**本方法**发生，而本方法只在 `frame_with` /
    /// `frame_windowed_with`（带 GPU 的帧路径）被调 —— headless 只推进
    /// Cmd 消费，无 GPU 无换页（架构裁决 2）。
    ///
    /// 借用结构（三段式）：① 只读规划（表共享借用，随即结束）→
    /// ② 换页（`videos` / `consumer` / `video_swaps` 三个不相交字段的
    /// 可变借用，[`Self::upload_video_frame`] 不经 `&mut self`）→
    /// ③ 收款（停播态 + 停音轨声部）。
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
            // ① 只读规划：目标帧号 / 是否到末帧 / 音轨键。
            let (elapsed, target, ended, audio_key) = {
                let Some(entry) = self.videos.get(&id) else {
                    continue;
                };
                let Some(playing) = &entry.playing else {
                    continue;
                };
                let elapsed = playing.elapsed + delta;
                let info = entry.container.video_info();
                let frame_count = info.frame_count as i64;
                let raw = if info.fps > 0.0 {
                    (elapsed * info.fps).floor() as i64
                } else {
                    0
                };
                let (target, ended) = if frame_count <= 0 {
                    // 空流病态（parse 已保证至少一帧，防御而已）：当帧停播。
                    (0, true)
                } else if raw >= frame_count {
                    // 末帧钳制 + 停播（P0 无 loop）：先把末帧上屏再收摊。
                    (frame_count - 1, true)
                } else {
                    (raw, false)
                };
                (elapsed, target, ended, playing.audio_key.clone())
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
            // ③ 收款：到末帧 → 停播态 + 停音轨声部；否则回写累计时间。
            if ended {
                if let Some(entry) = self.videos.get_mut(&id) {
                    entry.playing = None;
                }
                finished.push(audio_key);
            } else if let Some(entry) = self.videos.get_mut(&id) {
                if let Some(playing) = &mut entry.playing {
                    playing.elapsed = elapsed;
                }
            }
        }
        // 音轨通常已自然播完 —— stop_key 幂等 0；显式停是"末帧即静"的
        // 语义收口（音轨比画面长的容器不残留背景声）。
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
