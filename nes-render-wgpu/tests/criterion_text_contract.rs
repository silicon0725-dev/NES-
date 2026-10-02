//! T-Text 契约回归：`SetText` 文本光栅化的**契约面**逐项钉死（S4.5）。
//!
//! # 与 `criterion_backend` 的分工
//!
//! `criterion_backend_label_text_raster` 验证机制；本套件按编号钉**契约口径**，
//! 每条对应一个可独立指认的行为，失败时能直接报出违约条目。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Text-01 | ASCII 32..=126 全覆盖：每个可打印字符的字格逐字符像素断言 |
//! | T-Text-02 | 空格：只推进笔位，不产生四边形 |
//! | T-Text-03 | 表外字符（非 ASCII、超范围码点）：只推进笔位 |
//! | T-Text-04 | `\n`：换行落到 行高 x 行号 |
//! | T-Text-05 | 连续 `\n`：空行占位（行号继续推进），无字形 |
//! | T-Text-06 | 空字符串：零字形；`SetText` 清空已显示文本 |
//! | T-Text-07 | `font == NIL`：走默认字体 |
//! | T-Text-08 | 自定义 `font` 键：走对应字体；未登记键退回默认 |
//! | T-Text-09 | `FrameStats::glyphs`：与文本的非空格字符数严格一致 |
//! | T-Text-10 | Text + Sprite 同帧：互不干扰 |
//! | T-Text-11 | **Text + Control + Sprite 同帧（架构回归）** |
//! | T-Text-12 | 世界变换原点 = 笔起点；缩放作用于整段文本 |
//! | T-Text-13 | `line_spacing`：行距精确叠加进行高 |
//! | T-Text-14 | 异常路径：坏字体参数指名道姓；注册表扩容后字体仍正确 |
//!
//! # T-Text-11 的架构含义
//!
//! 三类渲染物在同一帧、同一条管线、同一个 `DrawKey` 全序里绘制：
//! 文本是现有渲染管线的一种**语义输入**，不是一套新的渲染后端。可观测证据：
//! ① 三者像素各自正确；② `stats.drawn == 控件 + 字形 + 精灵` 一次提交；
//! ③ 任意两类的 z 互换，遮挡关系随 DrawKey 翻转（全序在同一条实例流上成立）；
//! ④ `driver_errors == 0`。
//!
//! GPU 用例沿用 `criterion_backend` 的跳过纪律：无库跳过，有库失败即失败。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{
    Affine2, Camera2DState, ControlState, FrameInfo, LabelState, RenderAssetKey, RenderServer,
    Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FontParams, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

/// 清屏色字节。
const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
/// 精灵底色（红）与眼睛（近白）。
const BODY_RGBA: [u8; 4] = [255, 0, 0, 255];
const EYE_RGBA: [u8; 4] = [250, 250, 250, 255];
/// 控件边框（绿）。
const CONTROL_RGBA: [u8; 4] = [0, 255, 0, 255];

const CELL: u32 = 16;
const COLS: u32 = 16;
const FIRST: u32 = 32;
const COUNT: u32 = 95;

/// GPU 串行锁（与 criterion_backend 同款；测试二进制之间 cargo 顺序执行，
/// 锁只约束本二进制内的并行）。
fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 0.0, 0.0, Vec2::new(256.0, 128.0))
}

/// 字符码点 -> 程序化字形色（默认字体口径）。
fn char_color(code: u32) -> [u8; 4] {
    [
        40 + (code * 3 % 200) as u8,
        40 + (code * 7 % 200) as u8,
        40 + (code * 11 % 200) as u8,
        255,
    ]
}

/// 自定义字体的字形色（与默认字体刻意不同：整体反相，保证"用错了字体"必然显形）。
fn alt_char_color(code: u32) -> [u8; 4] {
    let base = char_color(code);
    [255 - base[0], 255 - base[1], 255 - base[2], 255]
}

/// 程序化字形表（`alt` 为真时用反相色，当第二套字体）。
fn font_sheet(alt: bool) -> (FontParams, Vec<u8>) {
    let rows = COUNT.div_ceil(COLS);
    let (w, h) = (CELL * COLS, CELL * rows); // 256x96
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for i in 0..COUNT {
        if FIRST + i == b' ' as u32 {
            continue; // 空格无墨
        }
        let color = if alt { alt_char_color(FIRST + i) } else { char_color(FIRST + i) };
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

/// 装配带默认字体的 256x128 消费器。返回的守卫持有 GPU 串行锁，
/// **必须**绑定到测试作用域（`let (_guard, consumer) = ...`），护住整个测试体。
fn open_with_font() -> (MutexGuard<'static, ()>, Option<CommandConsumer>) {
    let guard = gpu_lock();
    let consumer = match GpuContext::open() {
        Ok(ctx) => {
            let target = RenderTarget::with_size(&ctx, 256, 128).expect("256x128 目标");
            let atlas = SpriteAtlas::new(&ctx).expect("图集");
            CommandConsumer::new(ctx, target, atlas).expect("消费器")
        }
        Err(BackendError::NoLibraryCandidates(tried)) => {
            eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库，已尝试：{tried}");
            return (guard, None);
        }
        Err(err) => panic!("GPU 装配失败（应如实暴露）：{err}"),
    };
    let mut consumer = consumer;
    let (params, sheet) = font_sheet(false);
    consumer
        .set_default_font(params, &sheet)
        .expect("设置默认字体");
    (guard, Some(consumer))
}

/// 提交一帧并消费（单位相机：世界坐标 == 画布像素）。
fn render_one(
    consumer: &mut CommandConsumer,
    server: &mut WgpuRenderServer,
) -> FrameOutcome {
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(128.0, 64.0);
    server.set_camera(&camera);
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("消费一帧")
}

fn new_label(server: &mut WgpuRenderServer, text: &str, x: f32, y: f32) -> nes_render_api::ItemHandle {
    let handle = server.create_item(RenderAssetKey::NIL);
    server.set_transform(handle, Affine2::translation(x, y));
    server.set_text(handle, &LabelState::new(text, 16.0));
    handle
}

// ------------------------------------------------------------ T-Text-01..06

/// T-Text-01：ASCII 32..=126 全覆盖 —— 每行 15 字符折行，**逐字符**断言
/// 字格左上像素 == 该字符的程序化字形色（94 个非空格字符全部核对）。
#[test]
fn t_text_01_ascii_full_coverage() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    let mut text = String::new();
    for (i, ch) in (32u32..=126).map(|c| c as u8 as char).enumerate() {
        if i > 0 && i % 15 == 0 {
            text.push('\n');
        }
        text.push(ch);
    }
    new_label(&mut server, &text, 0.0, 0.0);
    let outcome = render_one(&mut consumer, &mut server);

    assert_eq!(outcome.stats.glyphs, 94, "94 个非空格可打印字符");
    assert_eq!(outcome.stats.driver_errors, 0);
    let image = &outcome.image;
    for (line_index, line) in text.split('\n').enumerate() {
        for (char_index, ch) in line.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let (x, y) = (char_index as u32 * CELL, line_index as u32 * CELL);
            assert_eq!(
                image.pixel(x, y),
                Some(char_color(ch as u32)),
                "字符 {ch:?}（第 {line_index} 行第 {char_index} 列）的字格色不符"
            );
        }
    }
}

/// T-Text-02：空格只推进笔位（"A B" 的 B 落在 x=32，空格区无像素）。
#[test]
fn t_text_02_space_advances_pen_only() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    new_label(&mut server, "A B", 0.0, 0.0);
    let outcome = render_one(&mut consumer, &mut server);
    assert_eq!(outcome.stats.glyphs, 2);
    let image = &outcome.image;
    assert_eq!(image.pixel(0, 0), Some(char_color(b'A' as u32)));
    assert_eq!(image.pixel(16, 0), Some(CLEAR_RGBA), "空格区无字形");
    assert_eq!(image.pixel(31, 15), Some(CLEAR_RGBA));
    assert_eq!(image.pixel(32, 0), Some(char_color(b'B' as u32)), "B 越过空格");
}

/// T-Text-03：表外字符（CJK 与 Latin-1 补充）只推进笔位。
#[test]
fn t_text_03_out_of_table_advances_pen_only() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    new_label(&mut server, "A\u{20AC}\u{4e2d}B", 0.0, 0.0); // A € 中 B
    let outcome = render_one(&mut consumer, &mut server);
    assert_eq!(outcome.stats.glyphs, 2, "只有 A/B 产生字形");
    let image = &outcome.image;
    assert_eq!(image.pixel(16, 0), Some(CLEAR_RGBA), "€ 无字形");
    assert_eq!(image.pixel(32, 0), Some(CLEAR_RGBA), "中 无字形");
    assert_eq!(image.pixel(48, 0), Some(char_color(b'B' as u32)), "B 落在第 4 笔位");
}

/// T-Text-04：`\n` 换行 = 行高 x 行号。
#[test]
fn t_text_04_newline_lines() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    new_label(&mut server, "A\nB", 0.0, 0.0);
    let outcome = render_one(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(image.pixel(0, 0), Some(char_color(b'A' as u32)));
    assert_eq!(image.pixel(0, 16), Some(char_color(b'B' as u32)), "第二行 y=16");
    assert_eq!(image.pixel(0, 15), Some(char_color(b'A' as u32)), "首行字格底");
}

/// T-Text-05：连续 `\n` —— 空行占位（行号推进），无字形。
#[test]
fn t_text_05_consecutive_newlines() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    new_label(&mut server, "A\n\nB", 0.0, 0.0);
    let outcome = render_one(&mut consumer, &mut server);
    assert_eq!(outcome.stats.glyphs, 2);
    let image = &outcome.image;
    assert_eq!(image.pixel(0, 16), Some(CLEAR_RGBA), "空行（第二行）无字形");
    assert_eq!(image.pixel(0, 32), Some(char_color(b'B' as u32)), "B 落在第三行 y=32");
}

/// T-Text-06：空字符串零字形；`SetText` 清空已显示文本。
#[test]
fn t_text_06_empty_text_clears() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    let label = new_label(&mut server, "", 0.0, 0.0);
    let outcome = render_one(&mut consumer, &mut server);
    assert_eq!(outcome.stats.glyphs, 0, "空字符串零字形");
    assert_eq!(outcome.image.pixel(0, 0), Some(CLEAR_RGBA));

    // 设文本 -> 显示；再设空 -> 清除（下一帧背景恢复）。
    server.set_text(label, &LabelState::new("AB", 16.0));
    let shown = render_one(&mut consumer, &mut server);
    assert_eq!(shown.stats.glyphs, 2);
    assert_eq!(shown.image.pixel(0, 0), Some(char_color(b'A' as u32)));

    server.set_text(label, &LabelState::new("", 16.0));
    let cleared = render_one(&mut consumer, &mut server);
    assert_eq!(cleared.stats.glyphs, 0);
    assert_eq!(cleared.image.pixel(0, 0), Some(CLEAR_RGBA), "清空后背景恢复");
}

// ------------------------------------------------------------ T-Text-07..09

/// T-Text-07：`font == NIL` 走默认字体（`LabelState::new` 的缺省即 NIL）。
#[test]
fn t_text_07_nil_font_uses_default() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    let label = server.create_item(RenderAssetKey::NIL);
    server.set_transform(label, Affine2::translation(0.0, 0.0));
    let state = LabelState::new("A", 16.0);
    assert!(state.font.is_nil(), "测试前提：缺省字体键为 NIL");
    server.set_text(label, &state);
    let outcome = render_one(&mut consumer, &mut server);
    assert_eq!(outcome.stats.glyphs, 1);
    assert_eq!(outcome.image.pixel(0, 0), Some(char_color(b'A' as u32)), "默认字体色");
}

/// T-Text-08：自定义 `font` 键走对应字体；未登记键退回默认字体。
#[test]
fn t_text_08_custom_font_key_and_fallback() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    // 第二套字体（反相色）登记在自定义键下。
    let (alt_params, alt_sheet) = font_sheet(true);
    let custom_key = RenderAssetKey::from_parts(777, 1);
    consumer
        .set_custom_font(custom_key, alt_params, &alt_sheet)
        .expect("登记自定义字体");

    let mut server = WgpuRenderServer::new();
    // 自定义键：用反相色。
    let custom = server.create_item(RenderAssetKey::NIL);
    server.set_transform(custom, Affine2::translation(0.0, 0.0));
    let mut custom_state = LabelState::new("AB", 16.0);
    custom_state.font = custom_key;
    server.set_text(custom, &custom_state);
    // 未登记键：退回默认（正相色）。
    let unknown = server.create_item(RenderAssetKey::NIL);
    server.set_transform(unknown, Affine2::translation(0.0, 64.0));
    let mut unknown_state = LabelState::new("AB", 16.0);
    unknown_state.font = RenderAssetKey::from_parts(888, 1);
    server.set_text(unknown, &unknown_state);

    let outcome = render_one(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(image.pixel(0, 0), Some(alt_char_color(b'A' as u32)), "自定义字体生效（反相色）");
    assert_eq!(image.pixel(16, 0), Some(alt_char_color(b'B' as u32)));
    assert_eq!(image.pixel(0, 64), Some(char_color(b'A' as u32)), "未登记键退回默认（正相色）");
    assert_eq!(image.pixel(16, 64), Some(char_color(b'B' as u32)));

    // NIL 键的自定义字体登记被拒（NIL 保留给默认语义）。
    let (params, sheet) = font_sheet(false);
    assert!(matches!(
        consumer.set_custom_font(RenderAssetKey::NIL, params, &sheet),
        Err(BackendError::ConfigMismatch(_))
    ));
}

/// T-Text-09：`FrameStats::glyphs` 与非空格字符数严格一致（多标签、多行、混合跳过）。
#[test]
fn t_text_09_glyph_stats_accounting() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    new_label(&mut server, "ABC", 0.0, 0.0); // 3
    new_label(&mut server, "A B", 0.0, 32.0); // 2（空格跳过）
    new_label(&mut server, "A\n\nB", 0.0, 64.0); // 2（空行无字形）
    new_label(&mut server, "A\u{20AC}B", 0.0, 96.0); // 2（表外跳过）
    new_label(&mut server, "", 128.0, 0.0); // 0
    let outcome = render_one(&mut consumer, &mut server);
    assert_eq!(outcome.stats.glyphs, 9, "3+2+2+2+0");
    assert_eq!(outcome.stats.drawn, 9, "本帧全部四边形都是字形");
}

// ------------------------------------------------------------ T-Text-10..11

/// T-Text-10：Text + Sprite 同帧，互不干扰。
#[test]
fn t_text_10_text_and_sprite_same_frame() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    let sprite = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(sprite, Affine2::translation(0.0, 0.0));
    new_label(&mut server, "AB", 48.0, 0.0);
    let outcome = render_one(&mut consumer, &mut server);

    assert_eq!(outcome.stats.drawn, 3, "精灵 1 + 字形 2");
    assert_eq!(outcome.stats.glyphs, 2);
    let image = &outcome.image;
    assert_eq!(image.pixel(10, 10), Some(BODY_RGBA), "精灵照常");
    assert_eq!(image.pixel(3, 3), Some(EYE_RGBA), "精灵眼睛照常（精灵在原点，眼睛局部 (3,3)）");
    assert_eq!(image.pixel(48, 0), Some(char_color(b'A' as u32)), "文本照常");
    assert_eq!(image.pixel(64, 0), Some(char_color(b'B' as u32)));
    assert_eq!(image.pixel(40, 0), Some(CLEAR_RGBA), "两区之间是背景");
}

/// T-Text-11（架构回归）：Text + Control + Sprite 同一帧、同一条管线、
/// 同一个 DrawKey 全序。见模块文档的架构含义。
#[test]
fn t_text_11_text_control_sprite_same_pipeline() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();

    // 场景：精灵 (16,16)、控件边框 (8,8,48,48) z=1、文本 "OK" (72,8) z=2。
    let sprite = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(sprite, Affine2::translation(16.0, 16.0));
    server.set_z(sprite, 0, 0);
    let control = server.create_item(RenderAssetKey::NIL);
    server.set_z(control, 1, 0);
    server.set_rect(control, &ControlState::new([0.0; 4], [8.0, 8.0, 48.0, 48.0]));
    let text = server.create_item(RenderAssetKey::NIL);
    server.set_z(text, 2, 0);
    server.set_transform(text, Affine2::translation(72.0, 8.0));
    server.set_text(text, &LabelState::new("OK", 16.0));

    let outcome = render_one(&mut consumer, &mut server);
    // ① 计数：一次提交里 精灵 1 + 控件 1 + 字形 2。
    // E-1（S12.1）：控件 = 4 条 1px 边框条 —— 1 精灵 + 4 + 2 字形 = 7。
    assert_eq!(outcome.stats.drawn, 7);
    assert_eq!(outcome.stats.controls, 1);
    assert_eq!(outcome.stats.glyphs, 2);
    // ② 像素各自正确（控件边框贴矩形边缘、内部透明透出精灵；边框的**像素厚度**
    // 随缩放与驱动取整在 2~4px 间浮动，是非契约量 —— 锚点取无歧义位置）。
    let image = &outcome.image;
    assert_eq!(image.pixel(16, 16), Some(BODY_RGBA), "精灵（控件内部透出）");
    assert_eq!(image.pixel(19, 19), Some(EYE_RGBA), "精灵眼睛");
    assert_eq!(image.pixel(8, 8), Some(CONTROL_RGBA), "控件边框角点");
    assert_eq!(image.pixel(9, 9), Some(CLEAR_RGBA), "边框恰 1px（E-1）");
    assert_eq!(image.pixel(11, 11), Some(CLEAR_RGBA), "深入内部透明");
    assert_eq!(image.pixel(72, 8), Some(char_color(b'O' as u32)), "文本 O");
    assert_eq!(image.pixel(88, 8), Some(char_color(b'K' as u32)), "文本 K");
    // ④ 单管线健康。
    assert_eq!(outcome.stats.driver_errors, 0);

    // ③ 全序可翻转：精灵 z 提到 3 后，实心字格被精灵盖住（同位遮挡）。
    server.set_z(sprite, 3, 0);
    server.set_transform(text, Affine2::translation(16.0, 16.0)); // 文本移到精灵正下方同位
    let covered = render_one(&mut consumer, &mut server);
    let image = &covered.image;
    assert_eq!(image.pixel(17, 17), Some(BODY_RGBA), "z 高的精灵盖住文本字格");
    assert_ne!(image.pixel(17, 17), Some(char_color(b'O' as u32)));
    // 反转：文本 z 提到 4，实心字格盖住精灵。
    server.set_z(text, 4, 0);
    let covering = render_one(&mut consumer, &mut server);
    assert_eq!(
        covering.image.pixel(17, 17),
        Some(char_color(b'O' as u32)),
        "z 高的文本盖住精灵"
    );
}

// ------------------------------------------------------------ T-Text-12..14

/// T-Text-12：世界变换 = 笔起点（平移精确落位）；缩放作用于整段文本。
#[test]
fn t_text_12_world_transform_origin_and_scale() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    // 平移：原点即首字格左上。
    new_label(&mut server, "A", 20.0, 10.0);
    // 缩放 x2：整段文本（含字距）翻倍 —— "AB" 占 64px 宽。
    let scaled = server.create_item(RenderAssetKey::NIL);
    server.set_transform(scaled, Affine2::translation(0.0, 40.0).mul(&Affine2::scale(2.0, 2.0)));
    server.set_text(scaled, &LabelState::new("AB", 16.0));

    let outcome = render_one(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(image.pixel(20, 10), Some(char_color(b'A' as u32)), "笔起点精确落位");
    assert_eq!(image.pixel(19, 9), Some(CLEAR_RGBA), "原点外是背景");
    assert_eq!(image.pixel(5, 45), Some(char_color(b'A' as u32)), "缩放后 A 格（0..31）");
    assert_eq!(image.pixel(35, 45), Some(char_color(b'B' as u32)), "缩放后 B 格（32..63，字距也翻倍）");
    assert_eq!(image.pixel(63, 71), Some(char_color(b'B' as u32)), "B 格右下");
    assert_eq!(image.pixel(64, 72), Some(CLEAR_RGBA), "缩放段外是背景");
}

/// T-Text-13：`line_spacing` 精确叠加进行高（0 / 4 / 8 三档）。
#[test]
fn t_text_13_line_spacing_stack() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    for (row, spacing) in [0.0f32, 4.0, 8.0].iter().enumerate() {
        let handle = server.create_item(RenderAssetKey::NIL);
        server.set_transform(handle, Affine2::translation(0.0, row as f32 * 32.0));
        let mut state = LabelState::new("A\nB", 16.0);
        state.line_spacing = *spacing;
        server.set_text(handle, &state);
    }
    let outcome = render_one(&mut consumer, &mut server);
    let image = &outcome.image;
    for (row, spacing) in [0.0f32, 4.0, 8.0].iter().enumerate() {
        let base = row as u32 * 32;
        assert_eq!(
            image.pixel(0, base + 16 + *spacing as u32),
            Some(char_color(b'B' as u32)),
            "line_spacing={spacing} 的第二行应落在 {}",
            16 + *spacing as u32
        );
        // 上一行字格底与下一行顶之间的间隙（spacing>0 时）是背景。
        if *spacing > 0.0 {
            assert_eq!(image.pixel(0, base + 16 + *spacing as u32 - 1), Some(CLEAR_RGBA));
        }
    }
}

/// T-Text-14：异常路径 —— 坏字体参数指名道姓；注册表扩容后字体仍正确。
#[test]
fn t_text_14_font_anomalies_and_registry_growth() {
    let (_guard, consumer) = open_with_font();
    let Some(mut consumer) = consumer else {
        return;
    };
    let (params, sheet) = font_sheet(false);

    // 坏参数三连：零字格 / 表容不下 / 非正字距。
    assert!(matches!(
        consumer.set_custom_font(RenderAssetKey::from_parts(901, 1), FontParams { cell_w: 0, ..params }, &sheet),
        Err(BackendError::ConfigMismatch(_))
    ), "零字格拒绝");
    assert!(matches!(
        consumer.set_custom_font(
            RenderAssetKey::from_parts(902, 1),
            FontParams { width: 16, height: 16, ..params },
            &sheet
        ),
        Err(BackendError::ConfigMismatch(_))
    ), "表容不下拒绝");
    assert!(matches!(
        consumer.set_custom_font(
            RenderAssetKey::from_parts(903, 1),
            FontParams { advance: f32::NAN, ..params },
            &sheet
        ),
        Err(BackendError::ConfigMismatch(_))
    ), "非有限字距拒绝");
    assert_eq!(consumer.registry().len(), 1, "被拒的登记不留痕（只剩默认字体）");

    // 注册表扩容（初始 4 瓦片：字体 1 + 再传 5 张纯色 -> 扩到 16）后，字体仍正确。
    for i in 0..5u32 {
        let key = RenderAssetKey::from_parts(16 * (i + 10), 1);
        let tex = vec![100u8 + i as u8; 16 * 16 * 4];
        consumer.register_texture(key, 16, 16, &tex).expect("注册纯色纹理");
    }
    assert!(consumer.registry().len() > 4, "已触发扩容");

    let mut server = WgpuRenderServer::new();
    new_label(&mut server, "AB", 0.0, 0.0);
    let outcome = render_one(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(image.pixel(0, 0), Some(char_color(b'A' as u32)), "扩容后字体字格仍正确");
    assert_eq!(image.pixel(16, 0), Some(char_color(b'B' as u32)));
    assert_eq!(outcome.stats.driver_errors, 0);
}

/// T-Text-09（S8.2 实证修复）：**非紧排字形表**（纹理高 > rows*cell ——
/// 真实烘焙图集 256x256 只占顶部 96px）的字格 UV 按纹理实际尺寸折算。
/// 旧算法按 rows 除 → 每格采样 2.67 倍高度压进四边形，字形竖向压扁
/// 成 ~3px（面板/HUD 文字一直过小的根因）。断言：留白表墨高 >= 6px
/// 且字格横距正确（紧排表行为不变的对照在 T-Text-01..08）。
#[test]
fn t_text_09_padded_sheet_glyph_height() {
    let guard = gpu_lock();
    let consumer = match GpuContext::open() {
        Ok(ctx) => {
            let target = RenderTarget::with_size(&ctx, 256, 128).expect("256x128 目标");
            let atlas = SpriteAtlas::new(&ctx).expect("图集");
            CommandConsumer::new(ctx, target, atlas).expect("消费器")
        }
        Err(BackendError::NoLibraryCandidates(tried)) => {
            eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库，已尝试：{tried}");
            return;
        }
        Err(err) => panic!("GPU 装配失败（应如实暴露）：{err}"),
    };
    let mut consumer = consumer;

    // 留白表：字形只画在顶部 rows*cell 高（96px），纹理 256 高 —— 与
    // 真实烘焙图集同形态。字形格内上半 8px 实心（有墨可量）。
    const CELL: u32 = 16;
    const COLS: u32 = 16;
    const COUNT: u32 = 95;
    let (w, h) = (COLS * CELL, 256u32); // 留白：h > 紧排高（rows*cell = 96px）
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for i in 0..COUNT {
        if 32 + i == b' ' as u32 {
            continue;
        }
        let (cx, cy) = ((i % COLS) * CELL, (i / COLS) * CELL);
        for y in 2..10u32 {
            for x in 3..13u32 {
                let at = (((cy + y) * w + cx + x) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    let params = FontParams {
        width: w,
        height: h,
        cell_w: CELL,
        cell_h: CELL,
        cols: COLS,
        first_char: 32,
        count: COUNT,
        advance: CELL as f32,
        line_height: CELL as f32,
    };
    consumer.set_default_font(params, &rgba).expect("设置默认字体");

    let mut server = WgpuRenderServer::new();
    let label = new_label(&mut server, "AJ", 20.0, 40.0);
    let _ = label;
    let outcome = render_one(&mut consumer, &mut server);
    let mut miny = u32::MAX;
    let mut maxy = 0;
    let mut ink = 0u32;
    for y in 0..128u32 {
        for x in 0..256u32 {
            if outcome.image.pixel(x, y) == Some([255, 255, 255, 255]) {
                ink += 1;
                miny = miny.min(y);
                maxy = maxy.max(y);
            }
        }
    }
    let height = maxy.saturating_sub(miny) + 1;
    assert!(ink > 0, "应有墨迹");
    assert!(height >= 6, "留白表字形墨高 {height}px（压扁回归：旧算法 ~3px）");
    assert_eq!(miny, 42, "墨迹起点 = 字格顶 + 2px（旧算法起点在格顶即被压扁）");
    drop(guard);
}
