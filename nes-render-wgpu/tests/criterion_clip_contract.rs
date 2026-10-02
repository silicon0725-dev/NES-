//! T-Clip 契约回归：E-2 裁剪契约（`SetClip`，语义裁决 D1）的契约面逐项钉死（S12-3）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Clip-01 | 命令流语义：`SetClip` 恒在 `SetRect` 之后；`None` 清除；未知句柄静默忽略；销毁随条目消亡 |
//! | T-Clip-02 | 像素语义：裁剪矩形按半开区间归属边界像素，clip 外像素保持清屏色 |
//! | T-Clip-03 | 缺省路径：无 `SetClip`（及清除后）的输出与裁剪机制之前逐位相同 |
//! | T-Clip-04 | 越界与空交集：与目标边界求交，交为空整段不画 |
//! | T-Clip-05 | 分段绘制：两个不同 clip 的条目各画各的 scissor，状态不跨段泄漏 |
//! | T-Clip-06 | ListView 滚动（S12-3 任务 4）：scroll=3*row_h 时第 0-2 行字形像素消失、第 3 行起可见；滚动条滑块像素存在且在右缘内侧 4px |
//! | T-Clip-07 | 零尺寸裁剪矩形 = 全裁：条目实例整条省略（与 NO_CLIP 哨兵在实例侧不可区分，必须在消费器提前拦下） |
//!
//! 裁剪矩形是**已解析的视口空间**矩形（D1）；本测试无相机（视口 = 目标尺寸），
//! 折算比例恰为 1:1，断言用整数坐标即无歧义。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{
    ControlState, FrameInfo, ItemHandle, ListAxis, ListState, Rect, RenderAssetKey, RenderCommand,
    RenderServer, ScrollBar, Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FontParams, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
/// 面板填充色（ControlState.fill 显式着色，走填充路径；非清屏色即可判定归属）。
const FILL_RGBA: [u8; 4] = [30, 144, 255, 255];
/// 分段绘制用第二块面板的填充色。
const FILL_B_RGBA: [u8; 4] = [255, 0, 0, 255];

fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 0.0, 0.0, Vec2::new(128.0, 128.0))
}

/// 装配 128x128 消费器（守卫须绑定到测试作用域）。
fn open_canvas() -> (MutexGuard<'static, ()>, Option<CommandConsumer>) {
    let guard = gpu_lock();
    let consumer = match GpuContext::open() {
        Ok(ctx) => {
            let target = RenderTarget::with_size(&ctx, 128, 128).expect("128x128 target");
            let atlas = SpriteAtlas::new(&ctx).expect("atlas");
            CommandConsumer::new(ctx, target, atlas).expect("consumer")
        }
        Err(BackendError::NoLibraryCandidates(tried)) => {
            eprintln!("[skip GPU cases] wgpu-native library not found, tried: {tried}");
            return (guard, None);
        }
        Err(err) => panic!("GPU assembly failed (must surface honestly): {err}"),
    };
    (guard, Some(consumer))
}

/// 铺满视口的纯填充面板（无相机：视口 = 目标 = 128x128，折算比例 1:1）。
fn full_panel(server: &mut WgpuRenderServer, fill: [u8; 4]) -> ItemHandle {
    let handle = server.create_item(RenderAssetKey::NIL);
    let mut state = ControlState::FULL_RECT;
    state.fill = fill;
    state.border = [0, 0, 0, 0];
    server.set_rect(handle, &state);
    handle
}

/// 无相机提交消费一帧（视图 = 单位矩阵，世界空间 == 视口空间 == 目标像素）。
fn flush(consumer: &mut CommandConsumer, server: &mut WgpuRenderServer) -> FrameOutcome {
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("consume one frame")
}

/// T-Clip-01（无 GPU 依赖）：命令流语义 —— 推送序、`None` 清除、未知句柄忽略、
/// 销毁随条目消亡。
#[test]
fn t_clip_01_stream_order_clear_and_ignore() {
    let mut server = WgpuRenderServer::new();
    let handle = full_panel(&mut server, FILL_RGBA);

    // 推送序：SetClip 恒在 SetRect 之后，且在 Submit 之前。
    server.set_clip(handle, Some(Rect::new(16.0, 16.0, 32.0, 32.0)));
    let stream = server.submit(&frame(1));
    let index_of = |pred: &dyn Fn(&RenderCommand) -> bool| {
        stream
            .iter()
            .position(|c| c.handle() == Some(handle) && pred(c))
            .expect("command for handle must exist")
    };
    let rect_idx = index_of(&|c| matches!(c, RenderCommand::SetRect { .. }));
    let clip_idx = index_of(&|c| matches!(c, RenderCommand::SetClip { rect: Some(_), .. }));
    assert!(
        rect_idx < clip_idx,
        "SetClip must come after SetRect (D1 order)"
    );
    assert!(
        clip_idx < stream.len() - 1,
        "Submit must remain the terminator"
    );

    // `rect: None` 清除：下一次 submit 不再出现 SetClip。
    server.set_clip(handle, None);
    let cleared = server.submit(&frame(2));
    assert!(
        cleared
            .iter()
            .all(|c| !matches!(c, RenderCommand::SetClip { .. })),
        "rect: None must clear the clip"
    );

    // 重新设置又回来（属性语义，可反复推）。
    server.set_clip(handle, Some(Rect::new(0.0, 0.0, 8.0, 8.0)));
    let again = server.submit(&frame(3));
    assert_eq!(
        again
            .iter()
            .filter(|c| matches!(c, RenderCommand::SetClip { rect: Some(_), .. }))
            .count(),
        1,
        "re-set clip must emit exactly one SetClip"
    );

    // 未知句柄：静默忽略（契约 I1），不产生任何命令。
    server.set_clip(ItemHandle::from_raw(999), Some(Rect::new(0.0, 0.0, 4.0, 4.0)));
    let unknown = server.submit(&frame(4));
    assert_eq!(
        unknown
            .iter()
            .filter(|c| matches!(c, RenderCommand::SetClip { .. }))
            .count(),
        1,
        "unknown-handle set_clip must be ignored"
    );

    // 销毁随条目消亡（DestroyItem 本身携带句柄，属生命周期动作；其后不得
    // 再有任何指向该句柄的属性命令）。
    server.destroy_item(handle);
    let dead = server.submit(&frame(5));
    assert!(
        dead.iter()
            .filter(|c| !c.is_lifecycle())
            .all(|c| c.handle() != Some(handle)),
        "destroyed item's property commands must not appear in the stream"
    );
    assert!(
        dead.iter()
            .all(|c| !matches!(c, RenderCommand::SetClip { .. })),
        "destroyed item's clip must die with it"
    );
}

/// T-Clip-02：像素级 scissor —— `SetClip(16,16,32,32)` 后 clip 内 == 填充色、
/// clip 外 == 清屏色，边界像素归属符合半开区间（[16,48) 内、48 外）。
#[test]
fn t_clip_02_pixel_scissor_half_open() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = full_panel(&mut server, FILL_RGBA);
    server.set_clip(handle, Some(Rect::new(16.0, 16.0, 32.0, 32.0)));
    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(outcome.stats.controls, 1);

    // 左边界：15 外、16 内（半开区间含左端）。
    assert_eq!(image.pixel(15, 20), Some(CLEAR_RGBA), "left of clip: clear");
    assert_eq!(image.pixel(16, 20), Some(FILL_RGBA), "clip start x=16: fill");
    // 右边界：47 内（最后一列）、48 外（半开区间不含右端）。
    assert_eq!(image.pixel(47, 20), Some(FILL_RGBA), "last column x=47: fill");
    assert_eq!(image.pixel(48, 20), Some(CLEAR_RGBA), "x=48 is outside [16,48)");
    // 上边界。
    assert_eq!(image.pixel(20, 15), Some(CLEAR_RGBA), "above clip: clear");
    assert_eq!(image.pixel(20, 16), Some(FILL_RGBA), "clip start y=16: fill");
    // 下边界。
    assert_eq!(image.pixel(20, 47), Some(FILL_RGBA), "last row y=47: fill");
    assert_eq!(image.pixel(20, 48), Some(CLEAR_RGBA), "y=48 is outside [16,48)");
    // 角点。
    assert_eq!(image.pixel(16, 16), Some(FILL_RGBA), "clip corner in");
    assert_eq!(image.pixel(47, 47), Some(FILL_RGBA), "clip corner in");
    assert_eq!(image.pixel(0, 0), Some(CLEAR_RGBA), "far corner out");
    assert_eq!(image.pixel(127, 127), Some(CLEAR_RGBA), "far corner out");
}

/// T-Clip-03：缺省路径逐位不变 —— 无 `SetClip` 的输出（每像素 == 填充色）
/// 与"设置过再清除"后的输出逐位相同（同一场景两遍消费比对 RGBA）。
#[test]
fn t_clip_03_no_clip_output_bitwise_unchanged() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = full_panel(&mut server, FILL_RGBA);

    // 第一遍：从未设置过裁剪（缺省路径）。
    let baseline = flush(&mut consumer, &mut server);
    // 缺省路径像素级锚点：全图 == 填充色（与裁剪机制之前的满铺行为一致）。
    for y in 0..128u32 {
        for x in 0..128u32 {
            assert_eq!(
                baseline.image.pixel(x, y),
                Some(FILL_RGBA),
                "default path must fill every pixel at ({x},{y})"
            );
        }
    }

    // 第二遍：设置裁剪再清除（`rect: None`），输出必须与第一遍逐位相同。
    server.set_clip(handle, Some(Rect::new(16.0, 16.0, 32.0, 32.0)));
    let clipped = flush(&mut consumer, &mut server);
    assert_eq!(
        clipped.image.pixel(0, 0),
        Some(CLEAR_RGBA),
        "clipped frame must differ at (0,0) for this test to be meaningful"
    );
    server.set_clip(handle, None);
    let restored = flush(&mut consumer, &mut server);
    assert_eq!(
        restored.image.rgba, baseline.image.rgba,
        "cleared-clip output must be bitwise identical to the never-clipped output"
    );
}

/// T-Clip-04：越界裁剪与目标边界求交；与目标完全无交集的段整段不画。
#[test]
fn t_clip_04_bounds_intersection_and_empty_clip() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();

    // 完全在目标之外：整段跳过，全图保持清屏色。
    let alone = full_panel(&mut server, FILL_RGBA);
    server.set_clip(alone, Some(Rect::new(200.0, 200.0, 32.0, 32.0)));
    let empty = flush(&mut consumer, &mut server);
    assert_eq!(empty.stats.drawn, 0, "empty intersection draws nothing");
    assert_eq!(empty.image.pixel(0, 0), Some(CLEAR_RGBA));
    assert_eq!(empty.image.pixel(127, 127), Some(CLEAR_RGBA));

    // 负原点：(-16,-16,32,32) 与目标求交 -> [0,16)²。
    server.set_clip(alone, Some(Rect::new(-16.0, -16.0, 32.0, 32.0)));
    let neg = flush(&mut consumer, &mut server);
    assert_eq!(neg.image.pixel(0, 0), Some(FILL_RGBA), "intersection includes (0,0)");
    assert_eq!(neg.image.pixel(15, 15), Some(FILL_RGBA), "intersection corner in");
    assert_eq!(neg.image.pixel(16, 16), Some(CLEAR_RGBA), "outside [0,16): clear");
    assert_eq!(neg.image.pixel(127, 127), Some(CLEAR_RGBA));

    // 越过右下角：(96,96,64,64) 与目标求交 -> [96,128)²。
    server.set_clip(alone, Some(Rect::new(96.0, 96.0, 64.0, 64.0)));
    let over = flush(&mut consumer, &mut server);
    assert_eq!(over.image.pixel(95, 100), Some(CLEAR_RGBA), "x=95 outside");
    assert_eq!(over.image.pixel(96, 100), Some(FILL_RGBA), "x=96 inside");
    assert_eq!(over.image.pixel(127, 127), Some(FILL_RGBA), "target edge clamped in");
}

/// T-Clip-05：分段绘制 —— 两个不同 clip 的条目各画各的 scissor；若 scissor
/// 状态跨段泄漏，后一段会画进前一段的矩形（(64,64) 将是清屏色，即反证锚点）。
#[test]
fn t_clip_05_segments_keep_their_own_scissor() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let a = full_panel(&mut server, FILL_RGBA);
    let b = full_panel(&mut server, FILL_B_RGBA);
    server.set_clip(a, Some(Rect::new(8.0, 8.0, 32.0, 32.0)));
    server.set_clip(b, Some(Rect::new(64.0, 64.0, 32.0, 32.0)));
    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;

    // 段 A：[8,40)² 只有一号填充色。
    assert_eq!(image.pixel(8, 8), Some(FILL_RGBA), "segment A area");
    assert_eq!(image.pixel(39, 39), Some(FILL_RGBA), "segment A last pixel");
    assert_eq!(image.pixel(40, 40), Some(CLEAR_RGBA), "outside segment A");
    // 段 B：[64,96)² 只有三号填充色。
    assert_eq!(image.pixel(64, 64), Some(FILL_B_RGBA), "segment B area (own scissor)");
    assert_eq!(image.pixel(95, 95), Some(FILL_B_RGBA), "segment B last pixel");
    // 两段互不渗透：段 B 的色不出现在段 A 区域，反之亦然。
    assert_eq!(image.pixel(64, 8), Some(CLEAR_RGBA), "A scissor must not leak into B");
    assert_eq!(image.pixel(8, 64), Some(CLEAR_RGBA), "B must not paint inside A's scissor");
    // 其余区域清屏。
    assert_eq!(image.pixel(0, 0), Some(CLEAR_RGBA));
    assert_eq!(image.pixel(127, 127), Some(CLEAR_RGBA));
    assert_eq!(image.pixel(48, 48), Some(CLEAR_RGBA), "between the two segments");
}

// ------------------------------------------------------------ S12-3 任务 4（List 摊平）

/// 滚动条滑块色（与清屏 / 边框 / 字形色互异，归属判定无歧义）。
const THUMB_RGBA: [u8; 4] = [220, 200, 40, 255];
/// 控件边框色（ControlState::new 缺省哨兵绿的显式形式）。
const BORDER_RGBA: [u8; 4] = [0, 255, 0, 255];

const CELL: u32 = 16;
const COLS: u32 = 16;
const FIRST: u32 = 32;
const COUNT: u32 = 95;

/// 字符码点 -> 程序化字形色（与 criterion_text_contract 同式）。
fn char_color(code: u32) -> [u8; 4] {
    [
        40 + (code * 3 % 200) as u8,
        40 + (code * 7 % 200) as u8,
        40 + (code * 11 % 200) as u8,
        255,
    ]
}

/// 程序化字形表：16px 字格内墨迹只占顶部 14px（底部 2 行透明 —— 模拟
/// 真实字体的行内空隙）。滚动裁切的逐像素断言因此能在行带边界处做到
/// 严格：字格底部 2px 的"溢出行带"不会产生墨。
fn font_sheet_14px() -> (FontParams, Vec<u8>) {
    let rows = COUNT.div_ceil(COLS);
    let (w, h) = (CELL * COLS, CELL * rows);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for i in 0..COUNT {
        if FIRST + i == b' ' as u32 {
            continue; // 空格无墨
        }
        let color = char_color(FIRST + i);
        let (cx, cy) = ((i % COLS) * CELL, (i / COLS) * CELL);
        for y in 0..14 {
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

/// 装配带默认字体的 128x128 消费器（守卫必须绑定到测试作用域）。
fn open_canvas_with_font() -> (MutexGuard<'static, ()>, Option<CommandConsumer>) {
    let (guard, mut consumer) = open_canvas();
    if let Some(c) = consumer.as_mut() {
        let (params, sheet) = font_sheet_14px();
        c.set_default_font(params, &sheet).expect("默认字体登记");
    }
    (guard, consumer)
}

/// T-Clip-06（S12-3 任务 4）：ListView 滚动烘焙 + 滚动条 —— 20 行、
/// row_h 18、视口高 98、scroll = 54（3*row_h）：
/// ① 第 0-2 行字形像素消失（行带整体滚出矩形顶）；
/// ② 第 3 行起可见；
/// ③ 滚动条滑块像素存在且在矩形右缘内侧 4px（x = rect.x+rect.w-5）。
#[test]
fn t_clip_06_list_view_scroll_rows_and_thumb() {
    let (_guard, consumer) = open_canvas_with_font();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::NIL);

    // 20 行列表：矩形 (10,10,90,98)（视口高 ~98）。
    let mut state = ControlState::new([0.0; 4], [10.0, 10.0, 100.0, 108.0]);
    state.fill = [0, 0, 0, 0];
    state.border = BORDER_RGBA;
    // 内容总高 = 20*18 + 8 内衬 = 368；scroll_max = 368 - 98 = 270。
    state.scroll_bar = Some(ScrollBar {
        frac: 98.0 / 368.0,
        pos: 54.0 / 270.0,
        color: THUMB_RGBA,
    });
    server.set_rect(handle, &state);
    let mut text = String::new();
    for i in 0..20 {
        if i > 0 {
            text.push('\n');
        }
        text.push_str(&format!("ROW{i:02}"));
    }
    let rows = ListState {
        text: text.into(),
        font: RenderAssetKey::NIL,
        font_size: 16.0,
        row_h: 18.0,
        tab_w: 64.0,
        axis: ListAxis::Vertical,
        scroll: 54.0,
        selected: None,
        text_color: [255, 255, 255, 255],
        sel_fill: [0x2E, 0x4A, 0x6B, 0xFF],
    };
    server.set_list(handle, &rows);
    server.set_clip(handle, Some(Rect::new(10.0, 10.0, 90.0, 98.0)));

    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(outcome.stats.controls, 1);
    assert_eq!(outcome.stats.driver_errors, 0);
    // 行 2..8 各 5 个非空格字形（行 0/1 整行在矩形顶之上被省实例，
    // 行 9 起超出矩形底被截停）—— 省实例口径的确定性锚点。
    assert_eq!(outcome.stats.glyphs, 35);

    // ① 第 0-2 行字形像素消失：行 3 的墨从 y=14 开始，其上（矩形顶内衬
    //    之下）必须全为清屏色 —— 覆盖行 0-2 若未滚出本应出现墨迹的列段。
    for y in 11..14u32 {
        for x in 14..94u32 {
            assert_eq!(image.pixel(x, y), Some(CLEAR_RGBA), "行 0-2 已滚出：({x},{y}) 应为清屏色");
        }
    }

    // ② 第 3 行起可见：行 i 的墨带 = [18i-40, 18i-26)（笔 y = 4+i*18-54，
    //    墨 14px）。行 3 = 14..28，行 4 = 32..46 …行 7 = 86..100。
    for (row, y) in [(3u32, 20u32), (4, 38), (5, 56), (6, 74), (7, 92)] {
        assert_eq!(
            image.pixel(15, y),
            Some(char_color(b'R' as u32)),
            "行 {row} 首字符 'R' 的墨迹可见（y={y}）"
        );
    }
    // 行 3 第二字符 'O'（x 30..46）。
    assert_eq!(image.pixel(31, 20), Some(char_color(b'O' as u32)), "行 3 'O' 可见");

    // ③ 滑块：x = rect.x+rect.w-5 = 95，4px 宽（95..99），右缘 1px 仍是
    //    边框；行程 y = rect.y+1+pos*(h-2-滑块高)，滑块高 = frac*(h-2)。
    for x in 95..99u32 {
        assert_eq!(image.pixel(x, 38), Some(THUMB_RGBA), "滑块像素在右缘内侧（x={x}）");
    }
    assert_eq!(image.pixel(94, 38), Some(CLEAR_RGBA), "滑块左侧即字形列段之外");
    assert_eq!(image.pixel(99, 38), Some(BORDER_RGBA), "滑块右缘贴 1px 边框");
    assert_eq!(image.pixel(100, 38), Some(CLEAR_RGBA), "矩形右缘之外");
    // 滑块纵向：行程 25.09..50.65 —— 两端之外无滑块色。
    assert_eq!(image.pixel(96, 26), Some(THUMB_RGBA), "滑块上段");
    assert_eq!(image.pixel(96, 50), Some(THUMB_RGBA), "滑块下段（末像素行）");
    assert_eq!(image.pixel(96, 51), Some(CLEAR_RGBA), "滑块行程之外");
}

/// T-Clip-07（S12-3 任务 4）：零尺寸裁剪矩形 = 全裁 —— 条目实例整条
/// 省略（`drawn == 0`、全图清屏色）。这是"空交集 = 全裁"的像素出口：
/// 零矩形在实例侧与 NO_CLIP 哨兵不可区分，必须在消费器提前拦下。
#[test]
fn t_clip_07_zero_rect_clip_draws_nothing() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = full_panel(&mut server, FILL_RGBA);
    server.set_clip(handle, Some(Rect::new(0.0, 0.0, 0.0, 0.0)));
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 0, "全裁：一次 draw 都不发");
    for y in [0u32, 64, 127] {
        for x in [0u32, 64, 127] {
            assert_eq!(outcome.image.pixel(x, y), Some(CLEAR_RGBA), "全裁：({x},{y}) 清屏色");
        }
    }
}
