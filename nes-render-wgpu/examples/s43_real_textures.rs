//! S4.3 演示：把真实图片（JPEG / GIF / PNG 经外部工具统一转成 32bpp BMP）
//! 注册进纹理注册表，按原尺寸拼贴渲染 + 逐纹理像素校验。
//!
//! # 职责边界（如实报告）
//!
//! - 本 crate 保持零第三方依赖：**不**在 Rust 侧解码 JPEG/PNG/GIF。预处理由
//!   外部工具（System.Drawing，见 `assets` 下 BMP 的生成脚本说明）完成，
//!   统一转成无压缩 32bpp BMP —— BMP 是唯一可以安全手写解析的格式
//!   （BI_RGB 无压缩、行序固定，没有熵编码路径）。
//! - SVG 被外部工具跳过（GDI+ 不光栅化矢量），不参与本演示。
//! - 像素校验口径：每个纹理的内部采样点，输出像素应与**源图对应纹素**一致
//!   （源纹素透明则应透出清屏色）。缩放损失（外部工具的双三次插值）不在
//!   校验范围内 —— 我们比对的是"上传后原样读回"，不是"解码还原"。
//!
//! # 运行
//!
//! ```text
//! cargo run --example s43_real_textures
//! ```
//!
//! 产物：`output/s43_real_textures.png`（512x768 拼贴画）。

use std::path::Path;

use nes_render_api::{Affine2, Camera2DState, FrameInfo, RenderAssetKey, RenderServer, Vec2};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

/// 清屏色的字节形态。
const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];

/// 画布尺寸（宽 512、高 768：三张 ≤256px 的图两列排得下）。
const CANVAS: (u32, u32) = (512, 768);

fn main() {
    let sources = match load_sources() {
        Ok(sources) => sources,
        Err(err) => {
            eprintln!("S4.3 真实纹理演示失败（装载阶段，如实报告）：{err}");
            std::process::exit(1);
        }
    };
    match run(&sources) {
        Ok(outcome) => {
            report(&outcome);
            if verify(&outcome, &sources) {
                println!("\nS4.3 真实纹理演示：PASS（逐纹理像素校验全部通过）");
            } else {
                println!("\nS4.3 真实纹理演示：FAIL");
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("S4.3 真实纹理演示失败（如实报告）：{err}");
            std::process::exit(1);
        }
    }
}

/// 解析 32bpp BI_RGB 自底向上的 BMP（GDI+ 输出形态），返回 (宽, 高, RGBA)。
fn load_bmp_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if bytes.len() < 54 || &bytes[0..2] != b"BM" {
        return Err("不是 BMP（魔数不符）".to_string());
    }
    let u16at = |o: usize| u16::from_le_bytes(bytes[o..o + 2].try_into().unwrap());
    let u32at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let i32at = |o: usize| i32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let data_off = u32at(10) as usize;
    if u32at(14) < 40 {
        return Err(format!("不支持的 DIB 头大小 {}", u32at(14)));
    }
    let (w, h) = (i32at(18), i32at(22));
    let (bpp, comp) = (u16at(28), u32at(30));
    if bpp != 32 || comp != 0 {
        return Err(format!("只支持 32bpp BI_RGB，实际 bpp={bpp} comp={comp}"));
    }
    if w <= 0 || h <= 0 {
        return Err(format!("不支持的尺寸 {w}x{h}（顶朝下 BMP 不在 GDI+ 输出形态内）"));
    }
    let (w, h) = (w as u32, h as u32);
    let row = (w * 4) as usize;
    let mut rgba = Vec::with_capacity(row * h as usize);
    for y in (0..h).rev() {
        let start = data_off + y as usize * row;
        let line = bytes
            .get(start..start + row)
            .ok_or_else(|| "像素数据截断".to_string())?;
        for px in line.chunks_exact(4) {
            // BMP 32bpp 存储序是 BGRA。
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    Ok((w, h, rgba))
}

/// 一张已装载的源图。
struct SourceImage {
    name: String,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// 画布上的左上角（货架布局分配）。
    origin: (u32, u32),
    /// 注册键。
    key: RenderAssetKey,
}

impl SourceImage {
    fn texel(&self, x: u32, y: u32) -> [u8; 4] {
        let base = ((y * self.width + x) * 4) as usize;
        self.rgba[base..base + 4].try_into().unwrap()
    }
}

fn load_sources() -> Result<Vec<SourceImage>, BackendError> {
    // 装载 assets 下的全部 BMP（外部工具预处理产物），货架布局分配画布位置。
    let assets = Path::new("examples/assets");
    let mut entries: Vec<_> = std::fs::read_dir(assets)
        .map_err(|e| BackendError::Io(format!("读 assets 目录失败：{e}")))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bmp")))
        .collect();
    entries.sort();
    if entries.is_empty() {
        return Err(BackendError::Io(
            "examples/assets 下没有 BMP（先跑外部转换脚本）".to_string(),
        ));
    }

    let mut sources: Vec<SourceImage> = Vec::new();
    let (mut cursor_x, mut cursor_y, mut row_h) = (0u32, 0u32, 0u32);
    for (i, path) in entries.iter().enumerate() {
        let bytes = std::fs::read(path)
            .map_err(|e| BackendError::Io(format!("读 {} 失败：{e}", path.display())))?;
        let (w, h, rgba) = load_bmp_rgba(&bytes)
            .map_err(|e| BackendError::Io(format!("{} 解析失败：{e}", path.display())))?;
        if w > 256 || h > 256 {
            return Err(BackendError::ConfigMismatch(format!(
                "{} 为 {w}x{h}，超过注册表瓦片上限（预处理应缩到 256 内）",
                path.display()
            )));
        }
        if cursor_x + w > CANVAS.0 {
            cursor_x = 0;
            cursor_y += row_h;
            row_h = 0;
        }
        let origin = (cursor_x, cursor_y);
        cursor_x += w;
        row_h = row_h.max(h);
        println!(
            "[装载] {} -> {}x{}，画布位置 {:?}",
            path.file_name().unwrap().to_string_lossy(),
            w,
            h,
            origin
        );
        sources.push(SourceImage {
            name: path.file_stem().unwrap().to_string_lossy().into_owned(),
            width: w,
            height: h,
            rgba,
            origin,
            key: RenderAssetKey::from_parts(16 * (i as u32 + 1), 1),
        });
    }
    let used_h = cursor_y + row_h;
    if used_h > CANVAS.1 {
        return Err(BackendError::ConfigMismatch(format!(
            "拼贴需要 {used_h}px 高，超过画布 {}px",
            CANVAS.1
        )));
    }
    Ok(sources)
}

fn run(sources: &[SourceImage]) -> Result<FrameOutcome, BackendError> {

    // 2) GPU 装配（512x768 离屏目标）+ 注册全部纹理。
    let ctx = GpuContext::open()?;
    println!("[装配] 库：{}", ctx.library_path());
    let info = ctx.info();
    println!(
        "[装配] 适配器：{} / {} / backend={} ({})",
        info.vendor,
        info.device,
        info.backend_type,
        info.backend_type_name(),
    );
    let target = RenderTarget::with_size(&ctx, CANVAS.0, CANVAS.1)?;
    let atlas = SpriteAtlas::new(&ctx)?;
    let mut consumer = CommandConsumer::new(ctx, target, atlas)?;
    let assembly_errors = consumer.ctx().errors_len();
    if assembly_errors > 0 {
        for line in consumer.ctx().errors_snapshot() {
            eprintln!("[装配错误] {line}");
        }
        return Err(BackendError::Io(format!(
            "装配阶段出现 {assembly_errors} 条驱动侧未捕获错误"
        )));
    }
    for source in sources {
        let tile = consumer.register_texture(
            source.key,
            source.width,
            source.height,
            &source.rgba,
        )?;
        println!(
            "[注册] {} -> 瓦片 {tile}（{}x{}）",
            source.name, source.width, source.height
        );
    }

    // 3) 每张图一个精灵：按原图尺寸画（16px 基准四边形 x 尺寸缩放）。
    let mut server = WgpuRenderServer::new();
    for source in sources {
        let handle = server.create_item(source.key);
        let quad = Affine2::translation(source.origin.0 as f32, source.origin.1 as f32).mul(
            &Affine2::scale(
                source.width as f32 / 16.0,
                source.height as f32 / 16.0,
            ),
        );
        server.set_transform(handle, quad);
        server.set_z(handle, 0, source.key.slot() as u64);
    }
    let mut camera = Camera2DState::new(Vec2::new(CANVAS.0 as f32, CANVAS.1 as f32));
    camera.transform =
        Affine2::translation(CANVAS.0 as f32 / 2.0, CANVAS.1 as f32 / 2.0);
    server.set_camera(&camera);

    let frame = FrameInfo::new(
        0,
        0.0,
        0.0,
        Vec2::new(CANVAS.0 as f32, CANVAS.1 as f32),
    );
    let mut commands = Vec::new();
    server.submit_into(&frame, &mut commands);
    let outcome = consumer.consume(&commands)?;
    if outcome.stats.driver_errors > 0 {
        for line in consumer.errors_snapshot() {
            eprintln!("[驱动错误] {line}");
        }
    }

    // 4) 落盘可视证据。
    let path = Path::new("output/s43_real_textures.png");
    let written = outcome.write_png(path)?;
    println!("[产物] PNG：{}（{written} 字节）", path.display());
    Ok(outcome)
}

fn report(outcome: &FrameOutcome) {
    let stats = &outcome.stats;
    println!(
        "[统计] commands={} drawn={} from_registry={} controls={} driver_errors={}",
        stats.commands, stats.drawn, stats.from_registry, stats.controls, stats.driver_errors
    );
    let image = &outcome.image;
    println!(
        "[像素] {}x{}（{}），去重颜色 {} 种",
        image.width,
        image.height,
        image.storage_format(),
        image.distinct_colors(),
    );
}

/// 逐纹理像素校验：内部采样点（避开缩放边缘）上，输出像素应与源纹素一致；
/// 源纹素透明则应透出清屏色。
fn verify(outcome: &FrameOutcome, sources: &[SourceImage]) -> bool {
    let image = &outcome.image;
    let mut failures: Vec<String> = Vec::new();
    for source in sources {
        // 采样点：中心与四分之一处（内部点，避开缩放边缘）。
        let points = [
            (source.width / 2, source.height / 2),
            (source.width / 4, source.height / 4),
            (3 * source.width / 4, 3 * source.height / 4),
        ];
        for (tx, ty) in points {
            let src = source.texel(tx, ty);
            let want = if src[3] >= 128 { src } else { CLEAR_RGBA };
            let got = image
                .pixel(source.origin.0 + tx, source.origin.1 + ty)
                .unwrap_or([0, 0, 0, 0]);
            if got != want {
                failures.push(format!(
                    "{} 纹素({tx},{ty})：源 {src:?}（期望 {want:?}），输出 {got:?} @ 画面({},{})",
                    source.name,
                    source.origin.0 + tx,
                    source.origin.1 + ty,
                ));
            }
        }
    }
    if failures.is_empty() {
        return true;
    }
    eprintln!("\n像素校验失败 {} 条：", failures.len());
    for failure in failures.iter().take(12) {
        eprintln!("  - {failure}");
    }
    false
}

/// 编译期锚点：清屏色字节与 f64 通道一致（与出口准则测试同款互锁）。
#[test]
fn clear_color_channels_match() {
    use nes_render_wgpu::renderer::CLEAR_COLOR;
    let expect = |channel: f64, byte: u8| {
        assert!((channel * 255.0 - byte as f64).abs() < 0.5);
    };
    expect(CLEAR_COLOR.r, CLEAR_RGBA[0]);
    expect(CLEAR_COLOR.g, CLEAR_RGBA[1]);
    expect(CLEAR_COLOR.b, CLEAR_RGBA[2]);
    expect(CLEAR_COLOR.a, CLEAR_RGBA[3]);
}
