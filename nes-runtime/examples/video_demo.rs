//! S15 视频资产面演示：**Video 资源 + 全屏 Sprite + 脚本 video_play +
//! 音画严格同步（音频钟主控）** 全链（照 audio_demo 的宿主形态）。
//!
//! ```text
//! Media/spider.amv（用户机器实测 AMV，不入库）──▶ Res(kind:"Video")
//!     ──▶ bind：nes-media 解析容器（AMV 内容探测）+ 第 0 帧上传 GPU
//!     ──▶ 脚本 video_play "Media/spider" ──▶ Cmd::VideoPlay ──▶ tick 后
//!     转渲染侧播放状态机 ──▶ 每帧当前解码帧同键覆写上 GPU（Sprite 即播
//!     画面）。帧号由混音器声部的已播采样位导出（音频钟主控，S15.1）——
//!     设备把声部队列真实消耗之前视频保持首帧，采样级对齐；声部播完移除
//!     = 视频同步停播（音画同终，控制台见 "video ended (audio-clock sync)"）。
//! ```
//!
//! 资产口径（不入库的纪律与取材链）：
//! - `examples/assets/Media/*.amv` 在 .gitignore（十几 MB 级用户资产）；
//! - 启动时 `Media/spider.amv` 缺失则从 `SOURCE_AMV`（用户视频目录）复制；
//! - 两处都没有：打印提示**正常退出**（不 panic —— 演示缺素材是常态）。
//!
//! 帧号观测：宿主每 30 帧向 stdout 打印当前视频帧号（选打印、不走
//! set_prop —— 播放状态是渲染侧的，写树会破坏"不进语义状态"的裁决）。
//! 音频钟在场的观测口径：帧号增速与 15fps 一致（页号 = 已播秒数 × fps）。
//!
//! 冒烟：`NES_GAME_FRAMES=180 cargo run --release --example video_demo`
//! （真 AMV 在场时 180 帧有声跑完；换页计数随帧增长，见
//! `tests/s15_video.rs` 的机器断言面与 `src/video.rs` 模块内分叉测试）。

use std::path::Path;
use std::time::Instant;

use nes_render_api::{FrameInfo, Vec2};
use nes_runtime::NesRuntime;
use nes_scene::ScriptVm;

/// 演示视频在资产根内的路径（场景文件同款声明）。
const VIDEO_REL: &str = "Media/spider.amv";
/// 用户机器上的实测 AMV（不在仓库；缺失即提示退出）。
const SOURCE_AMV: &str = "C:/Users/Administrator/Videos/text/spider_amv.amv";

fn main() {
    // 资产根 = 仓库内演示目录；演示视频缺了再从用户目录复制（不入库）。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    let media_dir = root.join("Media");
    std::fs::create_dir_all(&media_dir).unwrap();
    let video = root.join(VIDEO_REL);
    if !video.exists() {
        match std::fs::read(SOURCE_AMV) {
            Ok(bytes) => {
                std::fs::write(&video, &bytes).expect("复制演示视频");
                println!("[video] copied {} -> {} ({} KB)", SOURCE_AMV, VIDEO_REL, bytes.len() / 1024);
            }
            Err(_) => {
                println!("[video] 演示视频缺失：{VIDEO_REL} 与 {SOURCE_AMV} 都不在 —— 无素材可播，退出");
                println!("[video] 放一份 .amv 到 examples/assets/Media/ 后重跑即可");
                return;
            }
        }
    }

    let mut rt = NesRuntime::open_windowed_with_root(&root, "NES 2.0 - Video Demo (S15)", 384, 216)
        .expect("窗口装配");
    // 开音频：视频音轨（IMA ADPCM）在 video_play 时经混音器出声。
    // 失败如实报一行、演示照常（无声播放 —— headless 语义同家法）。
    match rt.open_audio() {
        Ok(()) => println!("[audio] on (waveOut 48000Hz stereo)"),
        Err(e) => eprintln!("[audio] 不可用：{e}（视频照播，无声）"),
    }
    rt.load_scene("video_demo.ron").expect("加载场景");
    println!("[video] declared {} video(s), page swaps so far: {}", rt.video_count(), rt.video_page_swaps());
    let mut vm = ScriptVm::new();
    {
        let table = rt.resources_mut().clone();
        let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
            std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
        });
        assert!(issues.is_empty(), "脚本装载：{issues:?}");
    }
    rt.mount_input_view(&mut vm);

    let total: u64 = std::env::var("NES_GAME_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
    let mut now = Instant::now();
    let mut delta = 1.0 / 60.0;
    let mut transient = 0u64;
    const TRANSIENT_LIMIT: u64 = 120;
    // 音画同终观测：在播 -> 非在播的翻转沿打印一行（S15.1 音频钟主控下，
    // 视频的终点 = 声部播完移除，宿主在此取证）。
    let mut was_playing = rt.video_is_playing("Media/spider");
    for index in 0..total {
        let snap = rt.collect_input();
        let _ = rt.emit_input_signals(&snap);
        // tick 由引擎内建发射（S8.1）；video_play 的 Cmd 在 tick 后由
        // 运行时消费；换页发生在帧路径提取之前（当帧像素即当前帧）。
        let frame = FrameInfo::new(index, delta,
            index as f64 / 60.0,
            Vec2::new(384.0, 216.0),
        );
        match rt.frame_windowed_with(&frame, &mut vm) {
            Ok(Some(_stats)) => transient = 0,
            Ok(None) => {
                println!("[帧 {index}] 窗口已关闭，退出");
                break;
            }
            Err(err) => {
                transient += 1;
                eprintln!("[帧 {index}] 失败（{transient}/{TRANSIENT_LIMIT}）：{err}");
                if transient >= TRANSIENT_LIMIT {
                    std::process::exit(1);
                }
            }
        }
        // 音画同终取证（翻转沿打一次，不刷屏）。
        let playing = rt.video_is_playing("Media/spider");
        if was_playing && !playing {
            println!(
                "[帧 {index}] video ended (audio-clock sync) | page swaps = {} | last frame = {:?}",
                rt.video_page_swaps(),
                rt.video_current_frame("Media/spider"),
            );
        }
        was_playing = playing;
        // 帧号观测：打印（不走 set_prop —— 播放状态不进树/指纹）。
        if index % 30 == 0 {
            println!(
                "[帧 {index}] video frame = {:?} | page swaps = {}",
                rt.video_current_frame("Media/spider"),
                rt.video_page_swaps(),
            );
        }
        // 帧节拍（S12-4 同款纪律）：无固定 sleep——FIFO present 自节流，
        // 固定 sleep + vsync = 双重等待（卡顿感的根因）。delta 用实测
        // 帧差（clamp 0.1s），视频换页与音画同步都吃真实时间。
        delta = now.elapsed().as_secs_f32().min(0.1);
        now = std::time::Instant::now();
    }
    println!("[完成] Video Demo 退出（page swaps = {}）", rt.video_page_swaps());
}
