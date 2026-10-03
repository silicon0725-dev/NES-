//! S14 第 1 期契约回归：nes-media 适配层接入运行时（图片 + 音频回落序）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Med-01 | `declare_image`：合成 PNG 落盘 -> 声明 -> bind 装载 -> 经 nes-media 解码上传 GPU（张数记账） |
//! | T-Med-02 | 装载序：BMP 快路径优先（既有 BMP 资产零行为变化）；PNG 走适配层回落 |
//! | T-Med-03 | Sound 回落：手写 WAV 拒收的格式（FLAC）经 nes-media 装载注册进混音器 |
//! | T-Med-04 | 宿主直注：`register_host_sound` 未开音频也登记；`audio()` 借出可播可混 |
//! | T-Med-05 | 双路全失败：坏文件进缺口清单、指名两路原因、不 panic 不阻塞 |
//!
//! 文件纪律：PNG 由测试内**手写最小 PNG 编码器**现场合成（仓库不提交二进制
//! 资产，WAV 蜂鸣/BMP 演示纹理同一条家法）；FLAC 用用户实测曲
//! skip-if-missing（字体用例惯例 —— 文件不在仓库，CI/他机安全；打印与
//! 断言全 ASCII）。GPU 用例在无 DLL 环境以 `open().ok()?` 惯例跳过。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_audio::device::waveout_device_count;
use nes_media::decode_audio;
use nes_runtime::{registered_sound_count, NesRuntime};

/// 每个用例独立的临时资产根（进程内计数器保证目录唯一，测试可并行）。
fn assets_root(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "nes_s14_media_{tag}_{}_{}",
        n,
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("建临时资产根");
    dir
}

/// 设备用例串行锁（waveOut 设备是进程级单例，S13 同款）。
fn device_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---- 手写最小 PNG 编码器（测试夹具；无彩色/滤波花样 —— 够 nes-media 解）----

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(body);
    let mut crc_input = Vec::with_capacity(4 + body.len());
    crc_input.extend_from_slice(tag);
    crc_input.extend_from_slice(body);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// 合成一张 RGBA8 PNG（stored deflate：无压缩字节面，解码器照单全收）。
fn png_bytes(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), (w * h * 4) as usize);
    // 原始扫描线：每行前置 filter 字节 0（None）。
    let mut raw = Vec::with_capacity(rgba.len() + h as usize);
    for y in 0..h as usize {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]);
    }
    // zlib wrapper + stored deflate 块（BTYPE=00；单块足够 —— 测试图很小）。
    let mut idat = vec![0x78, 0x01];
    idat.push(0x01); // BFINAL=1, BTYPE=00
    let len = raw.len() as u16;
    idat.extend_from_slice(&len.to_le_bytes());
    idat.extend_from_slice(&(!len).to_le_bytes());
    idat.extend_from_slice(&raw);
    idat.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8bit / RGBA / deflate / adaptive / no-interlace
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &idat);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

const FLAC_NAME: &str = "心似烟火.flac";
const FLAC_DIR: &str = "C:/Users/Administrator/Music/text";

/// T-Med-01 + T-Med-02：合成 PNG 经 declare_image -> bind -> nes-media
/// 解码上传（GPU 记账 1 张）；同一根里再放一张 BMP 走快路径 —— 两张都
/// 上传即"快路径优先、回落可用"并存（既有 BMP 面零行为变化）。
#[test]
fn t_med_01_declare_image_png_upload() {
    let root = assets_root("img");
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    // 4 象限 PNG（16x16）：与引擎闭环用例同款色块 —— 像素对不对得上
    // 由 nes-media 单测钉住，这里钉的是链路（声明/装载/解码/上传记账）。
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            let c: [u8; 4] = if y < 8 {
                if x < 8 { [255, 255, 0, 255] } else { [0, 255, 255, 255] }
            } else if x < 8 {
                [200, 200, 200, 255]
            } else {
                [80, 80, 80, 255]
            };
            rgba.extend_from_slice(&c);
        }
    }
    std::fs::write(root.join("Textures").join("quad.png"), png_bytes(16, 16, &rgba))
        .expect("写合成 PNG");
    let bmp = root.join("Textures").join("plain.bmp");
    nes_runtime::write_bmp_rgba(&bmp, 16, 16, &vec![9u8; 16 * 16 * 4]).expect("写 BMP");

    // GPU 缺席（无 DLL）= 如实跳过（criterion_engine_loop 同款惯例）。
    let mut rt = match NesRuntime::open_with_root(&root, 64, 64) {
        Ok(rt) => rt,
        Err(_) => {
            eprintln!("[skip] no GPU backend: declare_image upload path skipped");
            return;
        }
    };
    let png_id = rt.declare_image("Textures/quad.png").expect("声明图片");
    let bmp_id = rt.declare_texture("Textures/plain.bmp").expect("声明纹理");
    let report = rt.bind_assets();
    assert!(report.is_clean(), "装载干净：{report:?}");
    // 上传张数 = 2：PNG 走 nes-media 回落、BMP 走手写快路径，同一账本。
    let uploaded = rt.upload_pending_textures().expect("上传");
    assert_eq!(uploaded, 2, "PNG（适配层）+ BMP（快路径）都要上传：{uploaded}");
    // 重复上传幂等（版本账目，与纹理面同构）。
    assert_eq!(rt.upload_pending_textures().expect("重传"), 0);
    let _ = png_id;
    let _ = bmp_id;
}

/// T-Med-03（设备门 + 文件门）：场景 Sound 资源指向 FLAC —— 手写 WAV
/// 解析必拒、nes-media 回落必收，整条装载链（declare_sound -> bind ->
/// open_audio 补注册）按路径键注册进混音器。
#[test]
fn t_med_03_sound_fallback_flac_registered() {
    let src = std::path::Path::new(FLAC_DIR).join(FLAC_NAME);
    if !src.exists() {
        eprintln!("[skip] flac sample not found (user music dir absent)");
        return;
    }
    let root = assets_root("flac");
    std::fs::create_dir_all(root.join("Audio")).unwrap();
    std::fs::copy(&src, root.join("Audio").join("song.flac")).expect("复制实测曲到资产根");
    let scene = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Audio/song.flac", kind: "Sound"),
    ],
    root: Node(name: "main", kind: "Node", children: []),
)
"#;
    std::fs::write(root.join("music_it.ron"), scene).expect("写场景");

    let _dev = device_lock();
    let mut rt = NesRuntime::open_headless(&root).expect("headless 装配");
    rt.load_scene("music_it.ron").expect("加载场景");
    let report = rt.bind_assets();
    assert!(report.is_clean(), "装载干净（解码在注册时才发生）：{report:?}");
    if waveout_device_count() == 0 {
        eprintln!("[skip] no waveOut device: mixer registration path skipped");
        return;
    }
    rt.open_audio().expect("开音频（补注册路径）");
    assert_eq!(
        registered_sound_count(&rt),
        1,
        "FLAC 经 nes-media 回落注册恰一条"
    );
    drop(rt); // 先关设备再放锁（S13 同款 drop 序）
}

/// T-Med-04：宿主直注 —— decode_audio 产物 register_host_sound 未开音频
/// 也登记（混音器惰性构造），`audio()` 借出后直接可播可混（无设备的
/// 确定性播放验证）。
#[test]
fn t_med_04_host_register_and_mix_headless() {
    let src = std::path::Path::new(FLAC_DIR).join(FLAC_NAME);
    if !src.exists() {
        eprintln!("[skip] flac sample not found (user music dir absent)");
        return;
    }
    let bytes = std::fs::read(&src).expect("读实测曲");
    let wav = decode_audio(&bytes).expect("FLAC 可解码");
    let mut rt = NesRuntime::open_headless(&assets_root("host")).expect("headless 装配");
    assert!(!rt.audio_open(), "未开音频");
    rt.register_host_sound("music", std::sync::Arc::new(wav));
    {
        let mixer = rt.audio().expect("未开音频：混音器可变借出（惰性构造）");
        assert!(mixer.has("music"), "直注键已登记");
        mixer.play("music", 1.0, false).expect("借出的混音器可直接播");
        let mut out = [0i16; 480];
        mixer.mix_into(&mut out, 48_000, 2);
        assert!(
            out.iter().any(|&s| s != 0),
            "曲中混音产物非零（确定性播放验证，无需设备）"
        );
    }
    // stop_host_sounds：声部清零、库保留。
    rt.stop_host_sounds();
    let mixer = rt.audio().expect("仍在");
    assert_eq!(mixer.active_voices(), 0, "停声后无声部");
    assert!(mixer.has("music"), "声音库保留");
}

/// T-Med-05：双路全失败 —— 坏字节挂 .flac 后缀：WAV 快路径与 nes-media
/// 回落都拒收，失败进缺口清单并指名两路原因；不 panic、不阻塞开音频。
#[test]
fn t_med_05_double_failure_lands_in_gap_ledger() {
    let root = assets_root("bad");
    std::fs::create_dir_all(root.join("Audio")).unwrap();
    std::fs::write(root.join("Audio").join("broken.flac"), b"not audio at all")
        .expect("写坏文件");
    let scene = r#"Scene(
    version: 1,
    resources: [
        Res(id: 1, path: "Audio/broken.flac", kind: "Sound"),
    ],
    root: Node(name: "main", kind: "Node", children: []),
)
"#;
    std::fs::write(root.join("bad_it.ron"), scene).expect("写场景");

    let _dev = device_lock();
    let mut rt = NesRuntime::open_headless(&root).expect("headless 装配");
    if waveout_device_count() > 0 {
        rt.open_audio().expect("坏文件不阻塞开音频");
    }
    rt.load_scene("bad_it.ron").expect("场景照常加载");
    let report = rt.bind_assets();
    if rt.audio_open() {
        assert!(
            report
                .failed
                .iter()
                .any(|(_, why)| why.contains("解码失败") && why.contains("nes-media")),
            "缺口清单指名两路原因：{report:?}"
        );
    }
    // 语义面无感：headless 确定性不受坏文件影响。
    let r1 = rt.run_headless("bad_it.ron", &[], 4, 1.0 / 60.0).expect("跑一遍");
    let r2 = rt.run_headless("bad_it.ron", &[], 4, 1.0 / 60.0).expect("跑两遍");
    assert_eq!(r1.trace_hash, r2.trace_hash, "坏文件下指纹仍确定");
    drop(rt);
}
