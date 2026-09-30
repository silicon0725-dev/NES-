//! S4.1 最小可视闭环：`清屏 + 精灵` 一帧 → 读回 → 逐像素断言 → PNG 落盘。
//!
//! # 验收口径（对照原型日志 `s41_probe3.log` 的锚点）
//!
//! | 位置 | 期望 | 含义 |
//! |---|---|---|
//! | `px(10,10)` | `rgba(255,0,0,255)` 红 | 精灵左上角（世界平移 `(10,10)`，16x16 格） |
//! | `px(13,13)` | `rgba(250,250,250,255)` 近白 | 精灵眼睛（局部 `(3,3)`） |
//! | `px(9,9)` / `px(26,26)` | `rgba(13,13,25,255)` 深藏青 | 清屏色（精灵之外） |
//! | 全画面 | 无 `rgba(255,0,255,255)` 品红 | 图集哨兵色 —— 出现即说明 UV 采到格 0 之外 |
//!
//! 相机按"中心 = 视口半尺寸"摆放，视图矩阵恰为单位：世界坐标 == 屏幕像素坐标，
//! 与原型日志同口径（同时真实走通了契约 I9 的 `view_matrix` 路径，而非跳过相机）。
//!
//! # 运行
//!
//! ```text
//! cargo run --example s41_visual_closure
//! ```
//!
//! 动态库按 `gpu::locate_library` 候选顺序解析（本仓库落点：`../wgpu-win/lib/`）。
//! 产物：`output/s41_visual_closure.png`（64x64，RGBA8）。
//!
//! # 交叉验证（PNG 解码侧，可选）
//!
//! `png` 模块不做自我解码（自证只能证明自洽）。落盘后可用外部解码器回读比对：
//!
//! ```text
//! powershell -Command "Add-Type -AssemblyName System.Drawing; $b=[System.Drawing.Bitmap]::FromFile('output/s41_visual_closure.png'); $b.GetPixel(10,10); $b.GetPixel(13,13); $b.GetPixel(9,9); $b.Dispose()"
//! ```
//!
//! 期望 `(255,0,0,255)` / `(250,250,250,255)` / `(13,13,25,255)`，与本例断言一致。

use std::path::Path;

use nes_render_api::{Affine2, Camera2DState, FrameInfo, RenderAssetKey, RenderServer, Vec2};
use nes_render_wgpu::{BackendError, CommandConsumer, FrameOutcome, WgpuRenderServer};

/// 清屏色（深藏青，与 `renderer` 的 `CLEAR_COLOR` 一致）。
const CLEAR: [u8; 4] = [13, 13, 25, 255];
/// 精灵底色（红）。
const BODY: [u8; 4] = [255, 0, 0, 255];
/// 精灵眼睛（近白；取 250 而非 255，是只可能来自本图案的值）。
const EYE: [u8; 4] = [250, 250, 250, 255];
/// 图集哨兵色（品红）：正常画面上绝不应出现。
const FILLER: [u8; 4] = [255, 0, 255, 255];

fn main() {
    match run() {
        Ok(outcome) => {
            report(&outcome);
            if verify(&outcome) {
                println!("\nS4.1 最小可视闭环：PASS（像素断言全部通过）");
            } else {
                println!("\nS4.1 最小可视闭环：FAIL");
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("S4.1 可视闭环失败（如实报告，不伪造截图）：{err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<FrameOutcome, BackendError> {
    // 1) 契约侧：一个精灵，世界平移 (10,10) —— 与原型 probe 完全同口径。
    let mut server = WgpuRenderServer::new();
    let key = RenderAssetKey::from_parts(16, 1); // slot 16 → 图集格 0（真实图案格）
    let sprite = server.create_item(key);
    server.set_transform(sprite, Affine2::translation(10.0, 10.0));
    server.set_z(sprite, 0, 0);
    server.set_visible(sprite, true);

    // 相机：中心 = 视口半尺寸 → 视图矩阵恰为单位（世界坐标 == 屏幕像素坐标）。
    let mut camera = Camera2DState::new(Vec2::new(64.0, 64.0));
    camera.transform = Affine2::translation(32.0, 32.0);
    server.set_camera(&camera);

    let frame = FrameInfo::new(0, 0.0, 0.0, Vec2::new(64.0, 64.0));
    let mut commands = Vec::new();
    server.submit_into(&frame, &mut commands);

    // 2) GPU 侧：装配 + 消费一帧（清屏 + 精灵 + 读回）。
    let mut consumer = CommandConsumer::open()?;
    let info = consumer.ctx().info();
    println!(
        "[装配] 库：{}",
        consumer.ctx().library_path(),
    );
    println!(
        "[装配] 适配器：{} / {} / backend={} ({}) / type={} ({})",
        info.vendor,
        info.device,
        info.backend_type,
        info.backend_type_name(),
        info.adapter_type,
        info.adapter_type_name(),
    );
    // 装配阶段（含上下文/目标/图集/管线创建）若有驱动侧未捕获错误，如实报出并判失败。
    let assembly_errors = consumer.ctx().errors_len();
    if assembly_errors > 0 {
        for line in consumer.ctx().errors_snapshot() {
            eprintln!("[装配错误] {line}");
        }
        eprintln!("装配阶段出现 {assembly_errors} 条驱动侧未捕获错误，闭环判失败");
        std::process::exit(1);
    }

    let outcome = consumer.consume(&commands)?;
    if outcome.stats.driver_errors > 0 {
        for line in consumer.errors_snapshot() {
            eprintln!("[驱动错误] {line}");
        }
    }

    // 3) 可视证据落盘。
    let path = Path::new("output/s41_visual_closure.png");
    let written = outcome.write_png(path)?;
    println!("[产物] PNG：{}（{written} 字节）", path.display());
    Ok(outcome)
}

/// 打印帧统计与抽样像素（证据链：失败时能直接看到"实际是什么"）。
fn report(outcome: &FrameOutcome) {
    let stats = &outcome.stats;
    println!(
        "[统计] frame={} commands={} creates={} updates={} drawn={} skipped={} ignored={} camera={} driver_errors={}",
        stats.frame_index,
        stats.commands,
        stats.creates,
        stats.updates,
        stats.drawn,
        stats.skipped,
        stats.ignored,
        stats.camera_applied,
        stats.driver_errors,
    );
    let image = &outcome.image;
    println!(
        "[像素] {}x{}（{} 存储），去重颜色 {} 种",
        image.width,
        image.height,
        image.storage_format(),
        image.distinct_colors(),
    );
    for (x, y) in [(9u32, 9u32), (10, 10), (13, 13), (25, 25), (26, 26)] {
        println!(
            "[像素] px({x},{y}) = {}",
            image.rgba_text(x, y).unwrap_or_else(|| "越界".into()),
        );
    }
}

/// 逐像素断言（口径见模块文档；返回是否全部通过）。
fn verify(outcome: &FrameOutcome) -> bool {
    let image = &outcome.image;
    let stats = &outcome.stats;
    let mut failures: Vec<String> = Vec::new();

    // 统计口径。
    if stats.drawn != 1 {
        failures.push(format!("期望 drawn=1，实际 {}", stats.drawn));
    }
    if stats.driver_errors != 0 {
        failures.push(format!(
            "driver_errors={}（驱动侧有未捕获错误，像素证据存疑）",
            stats.driver_errors
        ));
    }
    if !stats.camera_applied {
        failures.push("相机未生效（camera_applied=false）".to_string());
    }

    // 像素口径：原型锚点 + 边界 + 背景。
    let mut expect = |x: u32, y: u32, want: [u8; 4], what: &str| {
        let got = image.pixel(x, y).unwrap_or([0, 0, 0, 0]);
        if got != want {
            failures.push(format!(
                "px({x},{y}) 期望 {what} {want:?}，实际 {got:?}"
            ));
        }
    };
    expect(9, 9, CLEAR, "背景");
    expect(10, 10, BODY, "精灵左上(红)");
    expect(13, 13, EYE, "精灵眼睛(近白)");
    expect(25, 25, BODY, "精灵右下(红)");
    expect(26, 26, CLEAR, "精灵外(背景)");

    // 哨兵扫描：任何品红像素 = UV 采到图集格 0 之外。
    for y in 0..image.height {
        for x in 0..image.width {
            if image.pixel(x, y) == Some(FILLER) {
                failures.push(format!("px({x},{y}) 出现哨兵品红：UV 采到格 0 之外"));
            }
        }
    }

    if failures.is_empty() {
        return true;
    }
    eprintln!("\n像素/统计断言失败 {} 条：", failures.len());
    for failure in failures.iter().take(20) {
        eprintln!("  - {failure}");
    }
    let more = failures.len().saturating_sub(20);
    if more > 0 {
        eprintln!("  ……另有 {more} 条从略");
    }
    false
}
