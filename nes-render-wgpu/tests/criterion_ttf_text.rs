//! T-GT 契约回归：TTF 动态字形图集渲染集成（S12-11 第 2 期）。
//!
//! 与 `criterion_ttf`（第 1 期：解析 / 光栅化，纯 CPU）分工：本套件钉的是
//! **集成后的契约面** —— `set_ttf_default` 装载后 `font == NIL` 的文本改走
//! 真字体动态字形图集排版，且 TTF 未装载时基线逐位不变。
//!
//! 跳过纪律（与既有 GPU 用例一致）：无 wgpu-native 动态库 skip；无系统字体
//! （`C:/Windows/Fonts/simhei.ttf`）skip —— 契约在"库 + 字体"都在场的机器上
//! 验证，两处前提缺失都不伪造失败。
//!
//! 测试字面量全 ASCII（CJK 用 `\u{4E2D}` 转义），注释中文。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-GT-01 | `set_ttf_default` 后 NIL 文本产生像素：drawn/glyphs > 0、两帧逐位相同（确定性）、driver_errors == 0 |
//! | T-GT-02 | 多字号：同文本 font_size 16 vs 32，32 的字形覆盖明显更大 |
//! | T-GT-03 | 光标：caret Some(1)（"Ag"）的竖条出现在 x = advance('A') 处（容差 2px），高度贯穿整行 |
//! | T-GT-04 | **基线不变**：未装载 TTF 的位图默认字体路径输出与主分支逐位相同；TTF 已装载时显式 font 键仍走位图 |
//! | T-GT-05 | CJK 上屏：'中'（`\u{4E2D}`）字形像素存在（非零覆盖） |

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{Affine2, Camera2DState, FrameInfo, LabelState, RenderAssetKey, RenderServer, Vec2};
use nes_render_wgpu::ttf::TtfFont;
use nes_render_wgpu::{
    BackendError, CommandConsumer, FontParams, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

/// 清屏色字节（与后端 CLEAR_COLOR 一致，"这个像素是背景"才有区分度）。
const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
/// TTF 文本的着色（纯白：采样色 x tint 后 coverage=255 的墨迹即纯白）。
const INK_RGBA: [u8; 4] = [255, 255, 255, 255];

/// 目标尺寸（像素）。
const W: u32 = 256;
const H: u32 = 128;

/// GPU 串行锁（与 criterion_text_contract 同款）。
fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 0.0, 0.0, Vec2::new(W as f32, H as f32))
}

/// 读系统黑体（本机 Windows 自带；缺失返回 None 由调用方 skip）。
fn simhei() -> Option<Vec<u8>> {
    std::fs::read(std::path::Path::new("C:/Windows/Fonts/simhei.ttf")).ok()
}

/// 独立解析一份 simhei（测试内自算期望值用，与消费器内部状态无关）。
fn simhei_font() -> Option<TtfFont> {
    simhei().and_then(|data| TtfFont::parse(&data).ok())
}

/// 装配"已装载 TTF 默认字体"的 256x128 消费器。
///
/// 返回 `None` = GPU 或字体不可用（skip）；`Some` 时守卫必须绑定到测试作用域
///（`let (_guard, Some(consumer)) = ...`），护住整个测试体的 GPU 串行。
fn open_ttf_consumer() -> (MutexGuard<'static, ()>, Option<CommandConsumer>) {
    let guard = gpu_lock();
    let Some(data) = simhei() else {
        eprintln!("[skip] C:/Windows/Fonts/simhei.ttf not found");
        return (guard, None);
    };
    let ctx = match GpuContext::open() {
        Ok(ctx) => ctx,
        Err(BackendError::NoLibraryCandidates(tried)) => {
            eprintln!("[skip] wgpu-native not found: {tried}");
            return (guard, None);
        }
        Err(err) => panic!("GPU assembly failed (must be honest): {err}"),
    };
    let target = RenderTarget::with_size(&ctx, W, H).expect("256x128 target");
    let atlas = SpriteAtlas::new(&ctx).expect("atlas");
    let mut consumer = CommandConsumer::new(ctx, target, atlas).expect("consumer");
    consumer.set_ttf_default(&data).expect("set_ttf_default");
    (guard, Some(consumer))
}

/// 提交一帧并消费（单位相机：世界坐标 == 画布像素）。
fn render_one(consumer: &mut CommandConsumer, server: &mut WgpuRenderServer) -> FrameOutcome {
    let mut camera = Camera2DState::new(Vec2::new(W as f32, H as f32));
    camera.transform = Affine2::translation(W as f32 / 2.0, H as f32 / 2.0);
    server.set_camera(&camera);
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("consume one frame")
}

/// 新建一个 NIL 字体的文本渲染物（世界变换 = 笔起点）。
fn new_label(
    server: &mut WgpuRenderServer,
    text: &str,
    font_size: f32,
    x: f32,
    y: f32,
) -> nes_render_api::ItemHandle {
    let handle = server.create_item(RenderAssetKey::NIL);
    server.set_transform(handle, Affine2::translation(x, y));
    let mut state = LabelState::new(text, font_size);
    state.color = INK_RGBA;
    server.set_text(handle, &state);
    handle
}

/// 统计一个水平带（`y0..y1`）内的墨迹像素数（非背景即墨）。
fn ink_pixels(image: &nes_render_wgpu::FrameImage, y0: u32, y1: u32) -> u32 {
    let mut ink = 0;
    for y in y0..y1.min(H) {
        for x in 0..W {
            if image.pixel(x, y) != Some(CLEAR_RGBA) {
                ink += 1;
            }
        }
    }
    ink
}

/// 求一个水平带内墨迹像素的横向包围盒（`(min_x, max_x)`；无墨返回 `None`）。
fn ink_x_span(image: &nes_render_wgpu::FrameImage, y0: u32, y1: u32) -> Option<(u32, u32)> {
    let (mut min_x, mut max_x) = (u32::MAX, 0u32);
    for y in y0..y1.min(H) {
        for x in 0..W {
            if image.pixel(x, y) != Some(CLEAR_RGBA) {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
            }
        }
    }
    (min_x <= max_x).then_some((min_x, max_x))
}

// ------------------------------------------------------------ T-GT-01

/// T-GT-01：装载 TTF 后 NIL 文本产生像素 —— 两帧输出逐位相同（确定性）、
/// 记账健康（glyphs/drawn 与文本的非空格字符数一致、driver_errors == 0）。
#[test]
fn t_gt_01_ttf_text_deterministic() {
    let (_guard, mut consumer) = open_ttf_consumer();
    let Some(consumer) = consumer.as_mut() else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    // "Ag \u{4E2D}"：A、g、空格、中 —— 3 个非空格字符。
    new_label(&mut server, "Ag \u{4E2D}", 16.0, 8.0, 8.0);

    let first = render_one(consumer, &mut server);
    assert!(first.stats.drawn > 0, "TTF 文本必须产生四边形");
    assert_eq!(first.stats.glyphs, 3, "3 个非空格字符各一个字形");
    assert_eq!(first.stats.driver_errors, 0);
    assert!(
        ink_pixels(&first.image, 0, H) > 0,
        "画布必须有墨迹（TTF 文本产生像素）"
    );

    // 第二帧（稳态快照，无生命周期命令）：输出与首帧逐位相同。
    let second = render_one(consumer, &mut server);
    assert_eq!(second.stats.glyphs, 3);
    assert_eq!(second.image.rgba, first.image.rgba, "同场景两帧必须逐位相同");
    assert_eq!(second.stats.driver_errors, 0);
}

// ------------------------------------------------------------ T-GT-02

/// T-GT-02：多字号 —— 同一文本 font_size 16 vs 32，32 的墨迹覆盖明显更大
///（像素数与横向跨度都应显著增长：比例字号真实生效，不是固定格网）。
#[test]
fn t_gt_02_font_size_scales_glyphs() {
    let (_guard, mut consumer) = open_ttf_consumer();
    let Some(consumer) = consumer.as_mut() else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    new_label(&mut server, "W", 16.0, 0.0, 0.0);
    new_label(&mut server, "W", 32.0, 0.0, 64.0);

    let outcome = render_one(consumer, &mut server);
    assert_eq!(outcome.stats.glyphs, 2);
    let ink16 = ink_pixels(&outcome.image, 0, 64);
    let ink32 = ink_pixels(&outcome.image, 64, 128);
    assert!(ink16 > 0 && ink32 > 0, "两档都必须有墨迹");
    assert!(ink32 > ink16 * 2, "32px 的覆盖（{ink32}）应显著大于 16px（{ink16}）");
    let (min16, max16) = ink_x_span(&outcome.image, 0, 64).expect("16px 墨迹跨度");
    let (min32, max32) = ink_x_span(&outcome.image, 64, 128).expect("32px 墨迹跨度");
    assert!(
        max32 - min32 > max16 - min16,
        "32px 的字宽跨度应大于 16px：{} vs {}",
        max32 - min32,
        max16 - min16
    );
}

// ------------------------------------------------------------ T-GT-03

/// T-GT-03：光标随比例字宽 —— "Ag" 且 caret Some(1)：竖条应出现在
/// x = advance('A') 处（容差 2px），且从行顶贯穿到行底（文本墨迹只在
/// 基线附近，竖条独占的整行跨度是与文本区分的判据）。
#[test]
fn t_gt_03_caret_tracks_proportional_advance() {
    let (_guard, mut consumer) = open_ttf_consumer();
    let Some(consumer) = consumer.as_mut() else {
        return;
    };
    // 期望值独立自算：advance('A') @ 16px 与行高（同字体同字号）。
    let font = simhei_font().expect("字体");
    let gid_a = font.glyph_index('A').expect("'A' 有字形");
    let expected_x = font.advance(gid_a, 16.0).expect("advance('A')");
    let metrics = font.metrics(16.0).expect("16px metrics");
    let line_bottom = metrics.line_height.ceil() as u32;

    let mut server = WgpuRenderServer::new();
    let label = new_label(&mut server, "Ag", 16.0, 0.0, 0.0);
    let mut state = LabelState::new("Ag", 16.0);
    state.color = INK_RGBA;
    state.caret = Some(1);
    server.set_text(label, &state);

    let outcome = render_one(consumer, &mut server);
    // "Ag" 两个字形 + 1 根光标条；光标不计入 glyphs（与位图路径同口径）。
    assert_eq!(outcome.stats.glyphs, 2);
    assert_eq!(outcome.stats.drawn, 3);

    // 逐列统计"整行贯穿"的墨迹高度：只有光标条会从行顶延伸到行底
    //（文本字形顶部距行顶 >= ascent - cap_height > 0，够不到贯穿阈值）。
    let threshold = (metrics.line_height * 0.9).ceil() as u32;
    let mut caret_columns: Vec<u32> = Vec::new();
    for x in 0..W {
        let run = (0..line_bottom)
            .take_while(|&y| y < H)
            .filter(|&y| outcome.image.pixel(x, y) == Some(INK_RGBA))
            .count() as u32;
        if run >= threshold {
            caret_columns.push(x);
        }
    }
    assert!(
        !caret_columns.is_empty(),
        "必须有贯穿整行的光标列（阈值 {threshold}px / 行高 {}）",
        metrics.line_height
    );
    let (first_col, last_col) = (
        *caret_columns.first().expect("非空已断言"),
        *caret_columns.last().expect("非空已断言"),
    );
    assert!(
        (first_col as f32 - expected_x).abs() <= 2.0,
        "光标条左缘 {first_col} 应在 x = advance('A') = {expected_x} 附近（2px 容差）"
    );
    assert!(
        (last_col as f32 - expected_x).abs() <= 2.0,
        "光标条 1px 宽：右缘 {last_col} 不应远离 {expected_x}"
    );
}

// ------------------------------------------------------------ T-GT-04

/// 位图字形表的程序化栅格（与 criterion_text_contract 同款口径）。
fn font_sheet() -> (FontParams, Vec<u8>) {
    const CELL: u32 = 16;
    const COLS: u32 = 16;
    const FIRST: u32 = 32;
    const COUNT: u32 = 95;
    let rows = COUNT.div_ceil(COLS);
    let (w, h) = (CELL * COLS, CELL * rows);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for i in 0..COUNT {
        let code = FIRST + i;
        if code == b' ' as u32 {
            continue;
        }
        let color = [
            40 + (code * 3 % 200) as u8,
            40 + (code * 7 % 200) as u8,
            40 + (code * 11 % 200) as u8,
            255,
        ];
        let (cx, cy) = ((i % COLS) * CELL, (i / COLS) * CELL);
        for y in 0..CELL {
            for x in 0..CELL {
                let at = (((cy + y) * w + cx + x) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&color);
            }
        }
    }
    (
        FontParams {
            width: w,
            height: h,
            cell_w: CELL,
            cell_h: CELL,
            cols: COLS,
            first_char: FIRST,
            count: COUNT,
            advance: CELL as f32,
            line_height: CELL as f32,
        },
        rgba,
    )
}

/// 位图字格色（与 font_sheet 的程序化色一致）。
fn cell_color(code: u32) -> [u8; 4] {
    [
        40 + (code * 3 % 200) as u8,
        40 + (code * 7 % 200) as u8,
        40 + (code * 11 % 200) as u8,
        255,
    ]
}

/// T-GT-04：**基线不变**（additive 纪律的总闸）。
///
/// ① 未装载 TTF：位图默认字体路径的输出与主分支逐位相同 —— 同一场景两遍
/// 消费逐位一致，且字格像素/16px 等宽步进与 T-Text-01 钉死的口径相同；
/// ② TTF 已装载：显式 `font` 键（含未登记键）仍走位图默认字体，NIL 才走
/// TTF —— 装载动作只劫持 NIL 路径。
#[test]
fn t_gt_04_bitmap_baseline_unchanged() {
    let guard = gpu_lock();
    // ① 无 TTF：位图默认字体路径与主分支逐位相同。
    let ctx = match GpuContext::open() {
        Ok(ctx) => ctx,
        Err(BackendError::NoLibraryCandidates(tried)) => {
            eprintln!("[skip] wgpu-native not found: {tried}");
            return;
        }
        Err(err) => panic!("GPU assembly failed (must be honest): {err}"),
    };
    let target = RenderTarget::with_size(&ctx, W, H).expect("target");
    let atlas = SpriteAtlas::new(&ctx).expect("atlas");
    let mut consumer = CommandConsumer::new(ctx, target, atlas).expect("consumer");
    let (params, sheet) = font_sheet();
    consumer.set_default_font(params, &sheet).expect("bitmap default font");
    assert!(!consumer.registry().is_empty());

    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::NIL);
    server.set_transform(handle, Affine2::translation(0.0, 0.0));
    let mut state = LabelState::new("AB", 16.0);
    state.color = [255, 255, 255, 255];
    server.set_text(handle, &state);

    let first = render_one(&mut consumer, &mut server);
    assert_eq!(first.stats.glyphs, 2);
    // 字格左上像素 == 程序化字格色；字距 16px 等宽（主分支冻结口径）。
    assert_eq!(first.image.pixel(0, 0), Some(cell_color(b'A' as u32)));
    assert_eq!(first.image.pixel(16, 0), Some(cell_color(b'B' as u32)));
    let second = render_one(&mut consumer, &mut server);
    assert_eq!(second.image.rgba, first.image.rgba, "位图路径两帧逐位相同");
    drop(server);
    drop(consumer);
    drop(guard);

    // ② TTF 已装载：显式 font 键仍走位图（未登记键退回位图默认字体）。
    let (_guard2, mut consumer) = open_ttf_consumer();
    let Some(consumer) = consumer.as_mut() else {
        return;
    };
    let (params, sheet) = font_sheet();
    consumer.set_default_font(params, &sheet).expect("bitmap default font");
    let mut server = WgpuRenderServer::new();
    let explicit = server.create_item(RenderAssetKey::NIL);
    server.set_transform(explicit, Affine2::translation(0.0, 0.0));
    let mut state = LabelState::new("AB", 16.0);
    state.color = [255, 255, 255, 255];
    state.font = RenderAssetKey::from_parts(4242, 1); // 未登记键
    server.set_text(explicit, &state);
    let outcome = render_one(consumer, &mut server);
    assert_eq!(outcome.image.pixel(0, 0), Some(cell_color(b'A' as u32)), "未登记键退回位图默认");
    assert_eq!(outcome.image.pixel(16, 0), Some(cell_color(b'B' as u32)), "16px 等宽步进不变");
}

// ------------------------------------------------------------ T-GT-05

/// T-GT-05：CJK 上屏 —— '中'（U+4E2D）字形有非零覆盖（位图默认字体路径
/// 对表外 CJK 只推笔位，这条契约只在 TTF 路径成立）。
#[test]
fn t_gt_05_cjk_glyph_on_screen() {
    let (_guard, mut consumer) = open_ttf_consumer();
    let Some(consumer) = consumer.as_mut() else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    new_label(&mut server, "\u{4E2D}", 32.0, 0.0, 0.0);

    let outcome = render_one(consumer, &mut server);
    assert_eq!(outcome.stats.glyphs, 1, "CJK 字形记账 +1");
    assert_eq!(outcome.stats.driver_errors, 0);
    let ink = ink_pixels(&outcome.image, 0, 128);
    assert!(ink > 32, "'中' 必须有实质墨迹（实测 {ink}px）");
    let (min_x, max_x) = ink_x_span(&outcome.image, 0, 128).expect("有墨迹必有跨度");
    assert!(max_x - min_x >= 8, "'中' 是方块字：横向跨度应可观（{min_x}..{max_x}）");
}
