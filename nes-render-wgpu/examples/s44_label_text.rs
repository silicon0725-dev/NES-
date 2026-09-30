//! S4.4 演示：`SetText` 文本光栅化 —— 真实字体（Consolas 12px，外部烘焙成
//! 字形表 BMP）+ 精灵 + 控件 + 文本混排一帧。
//!
//! # 职责边界（与 S4.3 同一条纪律）
//!
//! - 字形栅格化在外部完成（System.Drawing 烘焙 `examples/assets/font_atlas.bmp`
//!   与 `font_metrics.txt`），本 crate 零依赖；
//! - 消费器把文本按字形表展开成"每字形一个四边形"（等宽字距 16px、行高 16px），
//!   采样注册表里的字形表纹理 —— 与精灵共用同一条管线。
//!
//! # 运行
//!
//! ```text
//! cargo run --example s44_label_text
//! ```
//!
//! 产物：`output/s44_label_text.png`（512x256）。

use std::collections::BTreeMap;
use std::path::Path;

use nes_render_api::{
    Affine2, Camera2DState, ControlState, FrameInfo, RenderAssetKey, RenderServer, Vec2,
};
use nes_render_wgpu::bmp;
use nes_render_wgpu::{BackendError, CommandConsumer, FontParams, FrameOutcome};

/// 清屏色的字节形态。
const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];

/// 画布尺寸。
const CANVAS: (u32, u32) = (512, 256);

fn main() {
    match run() {
        Ok(outcome) => {
            report(&outcome);
            if verify(&outcome) {
                println!("\nS4.4 文本光栅化演示：PASS");
            } else {
                println!("\nS4.4 文本光栅化演示：FAIL");
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("S4.4 文本光栅化演示失败（如实报告）：{err}");
            std::process::exit(1);
        }
    }
}

/// 解析 font_metrics.txt（`key=value` 空格分隔）。
fn parse_metrics(text: &str) -> Result<BTreeMap<String, String>, BackendError> {
    let mut map = BTreeMap::new();
    for token in text.split_whitespace() {
        if let Some((k, v)) = token.split_once('=') {
            map.insert(k.to_string(), v.to_string());
        }
    }
    if map.is_empty() {
        return Err(BackendError::Io("font_metrics.txt 为空或格式不符".into()));
    }
    Ok(map)
}

fn run() -> Result<FrameOutcome, BackendError> {
    // 1) 装载字形表（外部烘焙产物）。
    let atlas_path = Path::new("examples/assets/font_atlas.bmp");
    let metrics_path = Path::new("examples/assets/font_metrics.txt");
    let sheet_bytes = std::fs::read(atlas_path)
        .map_err(|e| BackendError::Io(format!("读 {} 失败：{e}", atlas_path.display())))?;
    let (sheet_w, sheet_h, sheet_rgba) = bmp::load_rgba(&sheet_bytes)?;
    let metrics_text = std::fs::read_to_string(metrics_path)
        .map_err(|e| BackendError::Io(format!("读 {} 失败：{e}", metrics_path.display())))?;
    let metrics = parse_metrics(&metrics_text)?;
    let get = |k: &str| -> Result<f32, BackendError> {
        metrics
            .get(k)
            .and_then(|v| v.parse::<f32>().ok())
            .ok_or_else(|| BackendError::Io(format!("font_metrics.txt 缺少字段 {k}")))
    };
    let (cw, ch) = metrics
        .get("cell")
        .and_then(|v| v.split_once('x'))
        .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
        .ok_or_else(|| BackendError::Io("font_metrics.txt 的 cell 字段格式不符".into()))?;
    let params = FontParams {
        width: sheet_w,
        height: sheet_h,
        cell_w: cw,
        cell_h: ch,
        cols: get("cols")? as u32,
        first_char: get("first")? as u32,
        count: get("count")? as u32,
        advance: get("advance")?,
        line_height: get("line_height")?,
    };
    println!(
        "[字体] {}：{}x{} 表、{}x{} 字格、每行 {} 格、覆盖 {} 字符、字距 {}、行高 {}",
        metrics.get("font").map(String::as_str).unwrap_or("?"),
        sheet_w,
        sheet_h,
        cw,
        ch,
        params.cols,
        params.count,
        params.advance,
        params.line_height
    );

    // 2) GPU 装配 + 设置默认字体。
    let ctx = nes_render_wgpu::GpuContext::open()?;
    println!("[装配] 库：{}", ctx.library_path());
    let target = nes_render_wgpu::RenderTarget::with_size(&ctx, CANVAS.0, CANVAS.1)?;
    let atlas = nes_render_wgpu::SpriteAtlas::new(&ctx)?;
    let mut consumer = CommandConsumer::new(ctx, target, atlas)?;
    let tile = consumer.set_default_font(params, &sheet_rgba)?;
    println!("[字体] 注册到瓦片 {tile}");

    // 3) 混排场景：精灵 + 控件边框 + 四段文本。
    let mut server = nes_render_wgpu::WgpuRenderServer::new();

    let sprite = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(sprite, Affine2::translation(16.0, 16.0));
    server.set_z(sprite, 0, 0);

    let frame_box = server.create_item(RenderAssetKey::NIL);
    server.set_z(frame_box, 1, 0);
    server.set_rect(frame_box, &ControlState::new([0.0; 4], [200.0, 24.0, 280.0, 88.0]));

    let mut add_label = |text: &str, x: f32, y: f32, order: u64| {
        let handle = server.create_item(RenderAssetKey::NIL);
        server.set_transform(handle, Affine2::translation(x, y));
        server.set_z(handle, 2, order);
        server.set_text(handle, &nes_render_api::LabelState::new(text, 12.0));
    };
    add_label("NES 2.0", 232.0, 32.0, 0);
    add_label("wgpu-native", 232.0, 52.0, 1);
    add_label("S4.4 text", 232.0, 72.0, 2);
    add_label("clear + sprite + control + text", 8.0, 224.0, 3);

    let mut camera = Camera2DState::new(Vec2::new(CANVAS.0 as f32, CANVAS.1 as f32));
    camera.transform = Affine2::translation(CANVAS.0 as f32 / 2.0, CANVAS.1 as f32 / 2.0);
    server.set_camera(&camera);

    let frame = FrameInfo::new(0, 0.0, 0.0, Vec2::new(CANVAS.0 as f32, CANVAS.1 as f32));
    let mut commands = Vec::new();
    server.submit_into(&frame, &mut commands);
    let outcome = consumer.consume(&commands)?;
    if outcome.stats.driver_errors > 0 {
        for line in consumer.errors_snapshot() {
            eprintln!("[驱动错误] {line}");
        }
    }

    let out_path = Path::new("output/s44_label_text.png");
    let written = outcome.write_png(out_path)?;
    println!("[产物] PNG：{}（{written} 字节）", out_path.display());
    Ok(outcome)
}

fn report(outcome: &FrameOutcome) {
    let stats = &outcome.stats;
    println!(
        "[统计] commands={} drawn={} sprites(registry)={} controls={} glyphs={} driver_errors={}",
        stats.commands, stats.drawn, stats.from_registry, stats.controls, stats.glyphs, stats.driver_errors
    );
}

/// 校验：文本区域有"墨"、文本行带宽之外是背景、字形计数与账面一致。
fn verify(outcome: &FrameOutcome) -> bool {
    let stats = &outcome.stats;
    let image = &outcome.image;
    let mut failures = Vec::new();

    let texts = ["NES 2.0", "wgpu-native", "S4.4 text", "clear + sprite + control + text"];
    let expect_glyphs: usize = texts
        .iter()
        .map(|t| t.chars().filter(|c| *c != ' ').count())
        .sum();
    if stats.glyphs as usize != expect_glyphs {
        failures.push(format!(
            "期望 glyphs={expect_glyphs}（四段文本的非空格字符数），实际 {}",
            stats.glyphs
        ));
    }
    if stats.controls != 1 {
        failures.push(format!("期望 controls=1，实际 {}", stats.controls));
    }
    if stats.driver_errors != 0 {
        failures.push(format!("driver_errors={}", stats.driver_errors));
    }

    // "NES 2.0" 行带 (232..344, 32..48) 应有足够墨迹。
    let mut ink = 0u32;
    for y in 32..48 {
        for x in 232..344 {
            if image.pixel(x, y) != Some(CLEAR_RGBA) {
                ink += 1;
            }
        }
    }
    if ink < 40 {
        failures.push(format!("NES 2.0 行带墨迹仅 {ink} 像素，文本没有画出来"));
    }
    // 文本行带之外的框内区域 (356..470, 32..48) 应是背景（框线不在该带内）。
    for y in 32..48 {
        for x in 356..470 {
            if image.pixel(x, y) != Some(CLEAR_RGBA) {
                failures.push(format!("({}, {}) 应为背景（文本行带外）", x, y));
                break;
            }
        }
    }

    if failures.is_empty() {
        return true;
    }
    eprintln!("\n校验失败 {} 条：", failures.len());
    for failure in &failures {
        eprintln!("  - {failure}");
    }
    false
}
