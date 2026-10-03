//! avi_probe —— AVI 资源管线 harness（S14.2 验收形态）。
//!
//! # 用法
//!
//! ```text
//! cargo run --release -p nes-media --example avi_probe
//! （探测目标写死在源码 SAMPLES 常量表——换样本改表重编译）
//! ```
//!
//! * `SAMPLES` 常量表：要验的文件列表（真数据样本写死，空串 = 合成演示）；
//! * 落盘目录固定 `avi_probe_out`（不存在则创建）；
//! * 表内空串项：现场合成一段演示 AVI（32 帧 32x24 渐变 + 22050Hz
//!   单声道蜂鸣音轨，DIB 未压缩）再走完整管线 —— 不依赖任何输入文件
//!   也能演示"装配 -> 解析 -> 逐帧落盘 -> 音轨进 WAV"全链路。
//!
//! # 产出
//!
//! 1. 元信息打印（尺寸/帧率/帧数/编解码器/音轨参数）；
//! 2. 全帧解出写 24-bit BMP 落盘（`frame_000.bmp` …）—— BMP 写盘是
//!    nes-render-wgpu `bmp.rs` 装载器的镜像手法，**写在示例内**避免
//!    污染库公共面（库的面只有 demux DTO）；
//! 3. 音轨写 WAV（`nes_audio::wav::write_wav`，混音器同款 16-bit 面）。
//!
//! 打印全 ASCII（Windows 控制台代码页安全，注释中文）。

#![forbid(unsafe_code)]

use nes_media::{AviVideo, VideoCodec};

/// 探测目标写**常量表**（词法干净：无环境变量污染流向文件系统——
/// 本地安全扫描 S12-10 实测会拦 env->fs 1 跳污染）。真数据试跑的三
/// 个样本 + 缺省合成演示；换样本改表重编译即可（探针不是产品）。
const SAMPLES: [&str; 4] = [
    "C:/Users/Administrator/Videos/text/spider_amv.amv",
    "C:/Users/Administrator/Videos/text/【4K超高清】蜘蛛糸モノポリー.avi",
    "C:/Users/Administrator/Videos/text/【初音ミク】妄想感傷代償連盟【DECO_27】.avi",
    "", // 空串 = 缺省合成演示 AVI
];

fn main() {
    const OUT_DIR: &str = "avi_probe_out";
    for path in SAMPLES {
        if path.is_empty() {
            println!("[avi_probe] building synthetic demo AVI in memory");
            let bytes = build_demo_avi();
            println!("[avi_probe] demo avi assembled: {} bytes", bytes.len());
            probe_and_dump(&bytes, OUT_DIR, "<synthetic demo>");
            continue;
        }
        println!("[avi_probe] input: {path}");
        let Ok(bytes) = std::fs::read(path) else {
            println!("[avi_probe]   skip (unreadable/absent)");
            continue;
        };
        // AMV 等未收录格式：指名报告后继续跑表内其余样本（panic 会让
        // 后续样本跑不到——真数据试跑要的是全景）。
        if let Err(e) = AviVideo::parse(&bytes) {
            println!("[avi_probe]   unsupported/unparseable: {e}");
            continue;
        }
        probe_and_dump(&bytes, OUT_DIR, path);
    }
}

/// 解析 -> 元信息打印 -> 全帧写 BMP -> 音轨写 WAV。
fn probe_and_dump(bytes: &[u8], out_dir: &str, source: &str) {
    let started = std::time::Instant::now();
    let avi = AviVideo::parse(bytes).unwrap_or_else(|e| panic!("parse {source}: {e}"));
    let parse_ms = started.elapsed().as_millis();

    let info = avi.video_info();
    let codec = match info.codec {
        VideoCodec::Dib => "DIB (uncompressed BGR)".to_string(),
        VideoCodec::Mjpg => "MJPG (Motion JPEG)".to_string(),
        VideoCodec::Unsupported(raw) => {
            let b = raw.to_le_bytes();
            format!("Unsupported({})", String::from_utf8_lossy(&b))
        }
    };
    println!("[avi_probe] size  : {}x{}", info.width, info.height);
    println!("[avi_probe] fps   : {}", info.fps);
    println!("[avi_probe] frames: {}", info.frame_count);
    println!("[avi_probe] codec : {codec}");
    println!("[avi_probe] parse : {parse_ms} ms (file {} KB)", bytes.len() / 1024);

    std::fs::create_dir_all(out_dir).expect("create out dir");

    // ---- 解帧写 24-bit BMP（自底向上行序，DIB 同族写法）----
    // AVI_MAX_FRAMES：落盘帧数上限（真数据试跑防 4K 帧 BMP 洪水——
    // 单帧 3840x2160 BMP ≈ 24MB；缺省 16，0 = 全量）。步进取样保证
    // 首尾帧都被覆盖。
    let max_out: u32 = 16;
    let stride = if max_out == 0 {
        1
    } else {
        info.frame_count.div_ceil(max_out).max(1)
    };
    let frames_start = std::time::Instant::now();
    let mut written = 0u32;
    let mut decode_fail: Option<(u32, String)> = None;
    for i in 0..info.frame_count {
        if i % stride != 0 && i + 1 != info.frame_count {
            continue;
        }
        let img = match avi.frame(i) {
            Ok(img) => img,
            Err(e) => {
                if decode_fail.is_none() {
                    decode_fail = Some((i, e.to_string()));
                }
                continue;
            }
        };
        let path = std::path::Path::new(out_dir).join(format!("frame_{i:03}.bmp"));
        write_bmp(&path, img.width, img.height, &img.rgba)
            .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        written += 1;
    }
    println!(
        "[avi_probe] frames decoded+saved: {written}/{} (stride {stride}, cap {max_out}) -> {out_dir}/frame_NNN.bmp ({} ms)",
        info.frame_count,
        frames_start.elapsed().as_millis(),
    );
    if let Some((i, e)) = decode_fail {
        println!("[avi_probe] frame decode failures: first at {i}: {e}");
    }

    // ---- 音轨写 WAV（直接可进 Mixer 的 16-bit 面）----
    match avi.audio() {
        Some(wav) => {
            let path = std::path::Path::new(out_dir).join("audio.wav");
            nes_audio::wav::write_wav(&path, &wav).expect("write audio.wav");
            let secs = if wav.sample_rate > 0 {
                wav.frames() as f64 / f64::from(wav.sample_rate)
            } else {
                0.0
            };
            println!(
                "[avi_probe] audio : {}Hz {}ch {} frames (~{secs:.1} s) -> {}",
                wav.sample_rate,
                wav.channels,
                wav.frames(),
                path.display()
            );
        }
        None => println!("[avi_probe] audio : none (missing or non-PCM track, see avi.rs docs)"),
    }
    println!("[avi_probe] done.");
}

// ------------------------------------------------------------
// 24-bit BMP 写盘（nes-render-wgpu bmp.rs 装载器的镜像；示例内私有）
// ------------------------------------------------------------

/// RGBA8 -> 24-bit 底朝上 BMP 落盘。
fn write_bmp(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let stride = (w as usize * 3).div_ceil(4) * 4; // 行按 4 字节对齐
    let pixels = stride * h as usize;
    let mut out = Vec::with_capacity(54 + pixels);
    // BITMAPFILEHEADER（14 字节）
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + pixels) as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // 保留
    out.extend_from_slice(&0u16.to_le_bytes()); // 保留
    out.extend_from_slice(&54u32.to_le_bytes()); // 像素数据偏移
    // BITMAPINFOHEADER（40 字节）
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(w as i32).to_le_bytes()); // biWidth
    out.extend_from_slice(&(h as i32).to_le_bytes()); // biHeight（正 = 底朝上）
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&24u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&(pixels as u32).to_le_bytes()); // biSizeImage
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 DPI
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    // 像素：底朝上 BGR 行（与 AVI DIB 帧存储序一致）
    let mut body = vec![0u8; pixels];
    for y in 0..h as usize {
        let dst_row = h as usize - 1 - y;
        for x in 0..w as usize {
            let s = (y * w as usize + x) * 4;
            let d = dst_row * stride + x * 3;
            body[d] = rgba[s + 2];
            body[d + 1] = rgba[s + 1];
            body[d + 2] = rgba[s];
        }
    }
    out.extend_from_slice(&body);
    std::fs::write(path, &out)
}

// ------------------------------------------------------------
// 演示 AVI 现场装配（与 avi.rs 测试夹具同一家法：DIB 未压缩 + PCM 音轨）
// ------------------------------------------------------------

/// 装配一个 32 帧 32x24 渐变动画 + 0.5 秒 22050Hz 蜂鸣的合法 AVI 1.0。
fn build_demo_avi() -> Vec<u8> {
    const W: u32 = 32;
    const H: u32 = 24;
    const FRAMES: u32 = 32;
    const FPS: u32 = 12;
    const RATE: u32 = 22050;

    // 帧：随帧号滚动的对角渐变（RGBA8 -> BGR 底朝上块体）。
    let stride = (W as usize * 3).div_ceil(4) * 4;
    let mut frame_bodies = Vec::new();
    for f in 0..FRAMES {
        let mut body = vec![0u8; stride * H as usize];
        for y in 0..H as usize {
            let dst_row = H as usize - 1 - y;
            for x in 0..W as usize {
                let d = dst_row * stride + x * 3;
                let wave = ((x + y + f as usize) * 8) as u8;
                body[d] = wave; // B
                body[d + 1] = wave.wrapping_mul(2); // G
                body[d + 2] = 255 - wave; // R
            }
        }
        frame_bodies.push(body);
    }
    // 音轨：0.5 秒 440Hz 正弦（16-bit 单声道）。
    let samples: Vec<i16> = (0..RATE as usize / 2)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            ((t * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16
        })
        .collect();
    let mut pcm = Vec::with_capacity(samples.len() * 2);
    for s in &samples {
        pcm.extend_from_slice(&s.to_le_bytes());
    }

    let chunk = |id: &[u8; 4], body: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + body.len() + 1);
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
        out
    };
    let list = |form: &[u8; 4], children: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + children.len());
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&((children.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(form);
        out.extend_from_slice(children);
        out
    };

    // 视频流 strl（流 0，块名 '00db'）。
    let mut v_strh = Vec::with_capacity(56);
    v_strh.extend_from_slice(b"vids");
    v_strh.extend_from_slice(&[0u8; 16]); // handler + flags + priority/lang + initial frames
    v_strh.extend_from_slice(&1u32.to_le_bytes()); // dwScale
    v_strh.extend_from_slice(&FPS.to_le_bytes()); // dwRate
    v_strh.extend_from_slice(&0u32.to_le_bytes()); // dwStart
    v_strh.extend_from_slice(&FRAMES.to_le_bytes()); // dwLength
    v_strh.extend_from_slice(&[0u8; 16]); // bufsize + quality + samplesize
    v_strh.extend_from_slice(&[0u8; 8]); // rcFrame
    let mut v_strf = Vec::with_capacity(40);
    v_strf.extend_from_slice(&40u32.to_le_bytes());
    v_strf.extend_from_slice(&(W as i32).to_le_bytes());
    v_strf.extend_from_slice(&(H as i32).to_le_bytes());
    v_strf.extend_from_slice(&1u16.to_le_bytes());
    v_strf.extend_from_slice(&24u16.to_le_bytes());
    v_strf.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    v_strf.extend_from_slice(&((stride * H as usize) as u32).to_le_bytes());
    v_strf.extend_from_slice(&[0u8; 16]);
    let mut v_strl = chunk(b"strh", &v_strh);
    v_strl.extend_from_slice(&chunk(b"strf", &v_strf));

    // 音频流 strl（流 1，块名 '01wb'，PCMWAVEFORMAT）。
    let mut a_strh = Vec::with_capacity(56);
    a_strh.extend_from_slice(b"auds");
    a_strh.extend_from_slice(&[0u8; 16]); // handler + flags + priority/lang + initial frames
    a_strh.extend_from_slice(&1u32.to_le_bytes()); // dwScale
    a_strh.extend_from_slice(&RATE.to_le_bytes()); // dwRate
    a_strh.extend_from_slice(&0u32.to_le_bytes()); // dwStart
    a_strh.extend_from_slice(&(samples.len() as u32).to_le_bytes()); // dwLength
    a_strh.extend_from_slice(&[0u8; 8]); // bufsize + quality
    a_strh.extend_from_slice(&2u32.to_le_bytes()); // dwSampleSize
    a_strh.extend_from_slice(&[0u8; 8]); // rcFrame
    let mut a_strf = Vec::with_capacity(16);
    a_strf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    a_strf.extend_from_slice(&1u16.to_le_bytes()); // mono
    a_strf.extend_from_slice(&RATE.to_le_bytes());
    a_strf.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    a_strf.extend_from_slice(&2u16.to_le_bytes()); // block align
    a_strf.extend_from_slice(&16u16.to_le_bytes()); // bits
    let mut a_strl = chunk(b"strh", &a_strh);
    a_strl.extend_from_slice(&chunk(b"strf", &a_strf));

    // avih（56 字节）。
    let mut avih = Vec::with_capacity(56);
    avih.extend_from_slice(&(1_000_000u32 / FPS).to_le_bytes());
    avih.extend_from_slice(&[0u8; 8]); // max bytes + padding
    avih.extend_from_slice(&0x0000_0010u32.to_le_bytes()); // AVIF_HASINDEX
    avih.extend_from_slice(&FRAMES.to_le_bytes());
    avih.extend_from_slice(&0u32.to_le_bytes()); // initial frames
    avih.extend_from_slice(&2u32.to_le_bytes()); // streams
    avih.extend_from_slice(&[0u8; 4]); // suggested buffer
    avih.extend_from_slice(&W.to_le_bytes());
    avih.extend_from_slice(&H.to_le_bytes());
    avih.extend_from_slice(&[0u8; 16]); // reserved

    let mut hdrl_children = chunk(b"avih", &avih);
    hdrl_children.extend_from_slice(&list(b"strl", &v_strl));
    hdrl_children.extend_from_slice(&list(b"strl", &a_strl));
    let hdrl = list(b"hdrl", &hdrl_children);

    // movi：32 个 '00db' 帧 + 1 个 '01wb' 音频块；顺带记 idx1 条目。
    let mut movi_children = Vec::new();
    let mut idx1 = Vec::new();
    let mut child_off = 4usize; // 相对 'movi' 四字码，首块名在 +4。
    for body in &frame_bodies {
        movi_children.extend_from_slice(&chunk(b"00db", body));
        idx1.extend_from_slice(b"00db");
        idx1.extend_from_slice(&0x0000_0010u32.to_le_bytes()); // keyframe
        idx1.extend_from_slice(&(child_off as u32).to_le_bytes());
        idx1.extend_from_slice(&(body.len() as u32).to_le_bytes());
        child_off += 8 + body.len() + (body.len() % 2);
    }
    movi_children.extend_from_slice(&chunk(b"01wb", &pcm));
    idx1.extend_from_slice(b"01wb");
    idx1.extend_from_slice(&0u32.to_le_bytes());
    idx1.extend_from_slice(&(child_off as u32).to_le_bytes());
    idx1.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    let movi = list(b"movi", &movi_children);
    let idx1_chunk = chunk(b"idx1", &idx1);

    let riff_len = 4 + hdrl.len() + movi.len() + idx1_chunk.len();
    let mut out = Vec::with_capacity(8 + riff_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(riff_len as u32).to_le_bytes());
    out.extend_from_slice(b"AVI ");
    out.extend_from_slice(&hdrl);
    out.extend_from_slice(&movi);
    out.extend_from_slice(&idx1_chunk);
    out
}
