//! S15 视频资产面 · 运行时契约回归：Video 资源装载链 + 同键逐帧换页 +
//! 音画同播 + 确定性边界。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Vid-01 | 装载链：declare_video -> bind -> 容器解析入表（派生键存在、类别 Video、渲染键存在、AdoptReport 干净 —— texture 提示放宽） |
//! | T-Vid-02 | headless 确定性：video_play 的 Cmd 消费即弃，指纹与"无 video_play"对照面**逐位相同**；同轨迹两遍同 |
//! | T-Vid-03 | 同键逐帧换页（GPU）：bind 即上传首帧（swaps=1）；play 后逐帧推进 swaps 增长、当前帧号前进；stop 定格。**未开音频** —— 本用例走的就是 S15.1 三态分叉的"回退帧差钟"分支（混音器缺席），页号随 elapsed 推进即回退路径回归 |
//! | T-Vid-04 | 音画同播：play 时音轨声部启动（active_voices>=1）；stop 后归零（stop_key 点名停，不碰其他声部） |
//! | T-Vid-05 | 场景声明面：RON `Res(kind:"Video")` + Sprite2D.texture 引用 + 脚本 video_play 全链（headless 消费无错） |
//!
//! S15.1 音频钟主控（帧号从声部已播采样位导出 / 声部移除即音画同终 / 无轨
//! 回退帧差钟）的机器断言面在 `src/video.rs` 的模块内测试（`t_vid_sync01..03`
//! + `t_vfa01..02`）：注入无设备混音器 + 手动 mix_into 推进，全程确定性、
//!   不依赖 waveOut 实时性。
//!
//! 真实测试资产（skip-if-missing 惯例）：`spider_amv.amv`（13.5MB，160x128
//! @ 15fps + IMA ADPCM 音轨）在用户机器上，不在仓库 —— CI/他机安全跳过。
//! 测试把它拷进每用例独立的临时资产根（`Media/spider.amv`），与真实装载
//! 路径同形。GPU 缺席 / waveOut 设备缺席的路径如实跳过（"没有设备"与
//! "有设备但跑不通"不许互装 —— s13/s14 同一条纪律）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_audio::device::waveout_device_count;
use nes_asset::AssetKind;
use nes_render_api::{FrameInfo, Vec2};
use nes_runtime::{asset_key_of, NesRuntime};

/// 真实 AMV 测试资产（用户机器；不在仓库 —— 缺失即跳过）。
const SOURCE_AMV: &str = "C:/Users/Administrator/Videos/text/spider_amv.amv";
/// 场景里声明的视频资产路径（拷入临时根后的形态）。
const VIDEO_REL: &str = "Media/spider.amv";
/// 派生键（= 路径去扩展名；与声音键同一推导单点）。
const VIDEO_KEY: &str = "Media/spider";

/// 每个用例独立的临时资产根（进程内计数器保证目录唯一，测试可并行）。
fn assets_root(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "nes_s15_video_{tag}_{}_{}",
        n,
        std::process::id()
    ));
    std::fs::create_dir_all(dir.join("Media")).expect("建临时资产根");
    dir
}

/// 读真实 AMV（skip-if-missing：文件不在则返回 None，用例如实打印跳过）。
fn real_amv_bytes() -> Option<Vec<u8>> {
    std::fs::read(SOURCE_AMV).ok()
}

/// 把真实 AMV 拷进资产根（调用方已判存在）。
fn seed_video(root: &Path, bytes: &[u8]) {
    std::fs::write(root.join(VIDEO_REL), bytes).expect("拷入测试视频");
}

/// 设备用例串行锁（waveOut 是进程级单例 —— s13 同款）。
fn device_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 最小场景：Video 资源 + 全屏 Sprite 引用它 + 脚本首帧 video_play
///（`with_stop = true` 时第二帧 video_stop —— stop 面的对照）。
/// 脚本源用 raw 字面量写 RON 转义（`\n` / `\"` 进 RON 后是换行/引号）。
fn write_scene(root: &Path, name: &str, with_play: bool, with_stop: bool) {
    let play = if with_play {
        r#"every { if n < 1 { n = n + 1\n  video_play \"Media/spider\" } }"#
    } else {
        r#"every { if n < 1 { n = n + 1 } }"#
    };
    let stop = if with_stop {
        r#"every { if n < 2 { n = n + 1\n  video_stop \"Media/spider\" } }"#
    } else {
        r#"every { if n < 1 { n = n + 1 } }"#
    };
    let scene = format!(
        r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "{VIDEO_REL}", kind: "Video"),
    ],
    root: Node(
        name: "main",
        kind: "Node",
        children: [
            Node(name: "cam", kind: "Camera2D", local: (x: 80.0, y: 64.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0), children: [],),
            Node(name: "tv", kind: "Sprite2D", local: (x: 0.0, y: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, skew: 0.0), props: {{ "texture": Resource(1) }}, children: [],),
            Node(name: "play_btn", kind: "Script", props: {{ "source": "{play}" }}, children: [],),
            Node(name: "stop_btn", kind: "Script", props: {{ "source": "{stop}" }}, children: [],),
        ],
    ),
)
"#
    );
    std::fs::write(root.join(name), scene).expect("写场景");
}

/// T-Vid-01 + T-Vid-05：装载链 + 场景声明面（headless —— 装载/解析与
/// GPU 无关；换页缺席是 headless 语义，见 T-Vid-03）。
#[test]
fn t_vid_01_bind_chain_and_scene_declare() {
    let Some(bytes) = real_amv_bytes() else {
        println!("[skip] real AMV not found (user machine asset): {SOURCE_AMV}");
        return;
    };
    let root = assets_root("bind");
    seed_video(&root, &bytes);

    // 直声明面：declare_video -> bind -> 容器入表。
    let mut rt = NesRuntime::open_headless(&root).expect("headless 装配");
    let id = rt.declare_video(VIDEO_REL).expect("声明视频");
    let report = rt.bind_assets();
    assert!(report.is_clean(), "装载干净（无解码失败）：{report:?}");
    assert_eq!(rt.video_count(), 1, "容器解析入表");
    let key = asset_key_of(&rt, id).expect("已绑定键");
    assert_eq!(key.kind(), AssetKind::Video, "类别 = Video");
    assert!(
        key.as_render_key().is_some(),
        "Video 类是渲染面（渲染键存在 —— Sprite texture 引用的前提）"
    );
    // headless 无 GPU：bind 不换页（无纹理可上传），页账目停在未上传。
    assert_eq!(rt.video_page_swaps(), 0, "headless 无换页（架构裁决 2）");

    // 场景声明面：RON Res(kind:"Video") + Sprite2D.texture 引用 —— 体检
    // 干净（texture 提示放宽为 texture|video），load = 替换 + bind。
    write_scene(&root, "vid_it.ron", false, false);
    let adopt = rt.load_scene("vid_it.ron").expect("加载场景");
    assert!(adopt.is_clean(), "场景体检干净（texture 引 Video 不误报）：{adopt:?}");
    assert_eq!(rt.video_count(), 1, "场景替换后视频表重记");
}

/// T-Vid-02：headless 确定性 —— video_play / video_stop 的 Cmd 消费即弃；
/// 同轨迹两遍指纹逐位相同；播放状态在渲染侧真实建立/清除（消费链取证）。
/// "消费与否不改指纹"的严格对照在场景层钉死（nes-scene `t_vid_s_03` ——
/// 运行时的消费是无分支的固定路径；跨场景对照不成立：source 属性文本
/// 本身是树状态，进指纹）。
#[test]
fn t_vid_02_headless_fingerprint_ignores_video_cmds() {
    let Some(bytes) = real_amv_bytes() else {
        println!("[skip] real AMV not found (user machine asset): {SOURCE_AMV}");
        return;
    };
    let root = assets_root("fp");
    seed_video(&root, &bytes);
    write_scene(&root, "with_play.ron", true, false);
    write_scene(&root, "play_stop.ron", true, true);

    let mut rt = NesRuntime::open_headless(&root).expect("headless 装配");
    // 同轨迹两遍：逐位同（Cmd 消费是 tick 后固定步骤，不引入分歧）。
    let r1 = rt.run_headless("with_play.ron", &[], 10, 1.0 / 60.0).expect("跑一遍");
    let r2 = rt.run_headless("with_play.ron", &[], 10, 1.0 / 60.0).expect("跑两遍");
    assert_eq!(r1.trace_hash, r2.trace_hash, "同轨迹两遍指纹逐位相同");
    // 消费链取证：video_play 的 Cmd 被消费后播态在渲染侧建立（headless
    // 无换页 —— 架构裁决 2 —— 但播放状态机真实在走）。
    assert!(rt.video_is_playing(VIDEO_KEY), "video_play 消费后播态建立");
    // play + stop 混排的轨迹：同样确定性（两遍同），且跑完是停态
    //（第 2 帧的 video_stop 被消费 —— 停播态清除）。
    let s1 = rt.run_headless("play_stop.ron", &[], 10, 1.0 / 60.0).expect("混排一遍");
    let s2 = rt.run_headless("play_stop.ron", &[], 10, 1.0 / 60.0).expect("混排两遍");
    assert_eq!(s1.trace_hash, s2.trace_hash, "play/stop 混排同样两遍逐位同");
    assert!(!rt.video_is_playing(VIDEO_KEY), "video_stop 消费后播态清除");
}

/// T-Vid-03：同键逐帧换页（GPU 用例；无 GPU 如实跳过）—— bind 即上传
/// 首帧；play 后逐帧推进：换页数增长、当前帧号前进；stop 定格（换页停止）。
#[test]
fn t_vid_03_frame_page_swap_on_gpu() {
    let Some(bytes) = real_amv_bytes() else {
        println!("[skip] real AMV not found (user machine asset): {SOURCE_AMV}");
        return;
    };
    let root = assets_root("gpu");
    seed_video(&root, &bytes);
    // GPU 缺席（无 DLL）= 如实跳过（s14 同款惯例）。
    let mut rt = match NesRuntime::open_with_root(&root, 320, 240) {
        Ok(rt) => rt,
        Err(_) => {
            eprintln!("[skip] no GPU backend: video page-swap path skipped");
            return;
        }
    };
    rt.declare_video(VIDEO_REL).expect("声明视频");
    let report = rt.bind_assets();
    assert!(report.is_clean(), "装载干净：{report:?}");
    // bind 即上传首帧：未起播 Sprite 显示首帧而不是黑块（换页账目 = 1）。
    assert_eq!(rt.video_page_swaps(), 1, "bind 首帧上传");
    assert_eq!(rt.video_current_frame(VIDEO_KEY), Some(0), "当前帧 = 0");

    // 起播 + 跑 45 帧（delta 1/60，累计 0.75s；15fps 视频目标帧应到 ~11）。
    assert!(rt.play_video(VIDEO_KEY), "起播");
    assert!(rt.play_video(VIDEO_KEY), "重复 play 幂等（键仍在，不重启）");
    let frame = |i: u64| FrameInfo::new(i, 1.0 / 60.0, i as f64 / 60.0, Vec2::new(320.0, 240.0));
    for i in 0..45u64 {
        rt.frame(&frame(i)).expect("帧推进（含视频换页）");
    }
    let swaps_after = rt.video_page_swaps();
    assert!(
        swaps_after > 1,
        "播放中发生换页（首帧之外至少一次）：{swaps_after}"
    );
    let cur = rt.video_current_frame(VIDEO_KEY).expect("在表");
    assert!(cur >= 5, "当前帧号随播放前进（0.75s@15fps 应 >=5）：{cur}");
    assert!(rt.video_is_playing(VIDEO_KEY), "45 帧内仍在播（视频长得多）");

    // 停止：播态清除 + 换页定格（画面停在停那刻的帧）。
    assert!(rt.stop_video(VIDEO_KEY), "停止");
    assert!(!rt.stop_video(VIDEO_KEY), "重复 stop 幂等");
    assert!(!rt.video_is_playing(VIDEO_KEY), "播态清除");
    let swaps_at_stop = rt.video_page_swaps();
    for i in 45..60u64 {
        rt.frame(&frame(i)).expect("停止后帧推进");
    }
    assert_eq!(
        rt.video_page_swaps(),
        swaps_at_stop,
        "停止后不再换页（画面定格）"
    );
}

/// T-Vid-04：音画同播 P0 —— play 时音轨声部启动（>=1）；stop 后归零；
/// 点名停不碰其他声部（同帧 play 一个普通声音：视频 stop 后它还在）。
/// 设备门：无 waveOut 如实跳过。
#[test]
fn t_vid_04_audio_starts_and_stops_with_video() {
    let Some(bytes) = real_amv_bytes() else {
        println!("[skip] real AMV not found (user machine asset): {SOURCE_AMV}");
        return;
    };
    if waveout_device_count() == 0 {
        println!("[skip] no waveOut device: video audio path skipped");
        return;
    }
    let _dev = device_lock();
    let root = assets_root("aud");
    seed_video(&root, &bytes);

    let mut rt = NesRuntime::open_headless(&root).expect("headless 装配（音频同链）");
    rt.open_audio().expect("开音频");
    rt.declare_video(VIDEO_REL).expect("声明视频");
    let report = rt.bind_assets();
    assert!(report.is_clean(), "装载干净：{report:?}");
    assert_eq!(rt.active_voice_count(), 0, "起播前无声部");

    // 起播：音轨（IMA ADPCM 解码）即刻开声部 —— 音频钟主控的同步起点
    //（帧号从声部已播采样位导出；分叉的机器断言在 src/video.rs 模块内测试）。
    assert!(rt.play_video(VIDEO_KEY), "起播");
    assert!(
        rt.active_voice_count() >= 1,
        "video_play 后音轨声部启动"
    );

    // 点名停：视频音轨停、别人家的声部不动（先直注一个旁观声音）。
    {
        // 宿主直注一个旁观声音（键名避开视频音轨键空间）。
        let wav = nes_audio::Wav { sample_rate: 22050, channels: 1, samples: vec![0; 220] };
        rt.register_host_sound("bystander", std::sync::Arc::new(wav));
        rt.play_host_sound("bystander", 1.0, false).expect("旁观声部");
    }
    let before = rt.active_voice_count();
    assert!(before >= 2, "视频音轨 + 旁观声部并存：{before}");
    assert!(rt.stop_video(VIDEO_KEY), "停止");
    assert_eq!(
        rt.active_voice_count(),
        before - 1,
        "视频音轨停、旁观声部保留（stop_key 点名停）"
    );
    assert_eq!(rt.active_voice_count(), 1);
    drop(rt); // 显式先关运行时（含设备）再放锁 —— s13 同款收尾
}

/// T-Vid-05：坏视频不挡场景 —— 内容不是视频的文件按槽位进缺口清单
/// （`report.failed`），不 panic、不影响其他资源装载。
#[test]
fn t_vid_05_bad_video_lands_in_gap_ledger() {
    let root = assets_root("bad");
    std::fs::write(root.join(VIDEO_REL), b"this is not a video at all")
        .expect("写坏文件");
    let mut rt = NesRuntime::open_headless(&root).expect("headless 装配");
    rt.declare_video(VIDEO_REL).expect("声明视频");
    let report = rt.bind_assets();
    assert!(
        !report.failed.is_empty(),
        "坏视频进缺口清单：{:?}",
        report.failed
    );
    assert!(
        report.failed[0].1.contains(VIDEO_REL),
        "缺口指名路径：{:?}",
        report.failed
    );
    assert_eq!(rt.video_count(), 0, "解析失败的容器不入表");
    // 幂等：重复 bind 同一缺口不爆不重（版本账目没记 —— 失败可重试面）。
    let report = rt.bind_assets();
    assert!(!report.failed.is_empty(), "重复 bind 仍如实报缺口");
}
