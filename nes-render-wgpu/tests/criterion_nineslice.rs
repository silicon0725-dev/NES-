//! S16.6（Control 九宫格纹理渲染）契约：`SetNineSlice` 的像素事实。
//! S16.7 增模态染色（ns_modulate）与中间条平铺（ns_tiling）两开关。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-9S-01 | Control 96x96（源 48x48 等比 2x）：四角像素 = 源四角对应像素（1:1 不变形）；中心 = 源中心色；上边中点 = 源上边条对应列色（单向拉伸）；左边条 = 垂直单向拉伸 |
//! | T-9S-02 | Control 30x30 < 源边距和（32）：边距钳制（min 公式）生效 —— 无 panic、四角完整（锚纹理真角）、零中段合法退化 |
//! | T-9S-03 | 无 ns 记录的 Control → 输出与既有路径逐位同（fill/border 照旧） |
//! | T-9S-04 | 同键覆写 / NIL 清除：改边距即改像素；NIL 清除后整帧回基线**逐位**；输出序 SetNineSlice 恒在 SetPivot 之后 |
//! | T-NS-M-01 | S16.7 modulate：灰阶纹理 + 两开关 true + fill 红 → 面板 = 灰 x 红（通道手算）；开关 false 对照 = 纯灰 |
//! | T-NS-T-01 | S16.7 tiling：上条源列非均匀灰阶 → 平铺下多点采样与源列一一对应（每 16px 从源条头重启）；实例 36 片；4128px 面板 x 16px 单元触发 256 片上限，截断 66304 片进诊断计数 |
//! | T-NS-T-02 | S16.7 组合：modulate + tiling 同开 —— 平铺片与角同受 fill 染色 |
//!
//! 手法与 `criterion_control_contract` / `criterion_alpha_contract` 同源：
//! 直接驱动契约层（WgpuRenderServer -> 命令流 -> CommandConsumer），离屏
//! 消费 + 读回断言。测试纹理 48x48 代码生成（与入库的
//! `Textures/nine_patch.bmp` 同一布局：四角四色 16x16 + 四边双色条纹 +
//! 中心纯色）。GPU 用例沿用跳过纪律：无库跳过，有库失败即失败。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{
    ControlState, FrameInfo, ItemHandle, RenderAssetKey, RenderCommand, RenderServer, Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];

// 九宫测试纹理的九区颜色（48x48、边距 16；与 nine_patch.bmp 生成式同布局）。
const RED: [u8; 4] = [255, 0, 0, 255]; // 左上角
const GREEN: [u8; 4] = [0, 255, 0, 255]; // 右上角
const BLUE: [u8; 4] = [0, 0, 255, 255]; // 左下角
const YELLOW: [u8; 4] = [255, 255, 0, 255]; // 右下角
const MAGENTA: [u8; 4] = [255, 0, 255, 255]; // 上边条左半
const WHITE: [u8; 4] = [255, 255, 255, 255]; // 上边条右半
const DGRAY: [u8; 4] = [70, 70, 70, 255]; // 左边条上半
const LGRAY: [u8; 4] = [200, 200, 200, 255]; // 左边条下半
const CENTER: [u8; 4] = [40, 40, 60, 255]; // 中心纯色

/// 纹理注册键（测试内约定）。
fn tex_key() -> RenderAssetKey {
    RenderAssetKey::from_parts(7, 1)
}

/// 灰阶纹理键（T-NS-M-01 modulate 配方用）。
fn gray_key() -> RenderAssetKey {
    RenderAssetKey::from_parts(8, 1)
}

/// 灰阶列纹纹理键（T-NS-T-01/T-02 平铺用）。
fn ramp_key() -> RenderAssetKey {
    RenderAssetKey::from_parts(6, 1)
}

/// 生成 48x48 九宫测试纹理（RGBA 行主序；边距 16，中带 16）。
///
/// 布局：四角各 16x16 纯色（红/绿/蓝/黄）；上边条左右两半品红/白（验
/// 水平拉伸的列映射）、下边条橙/青；左边条上下两半暗灰/亮灰（验垂直
/// 拉伸的行映射）；中心纯色（双向拉伸处颜色恒定，采样容差无谓）。
fn nine_patch_rgba() -> Vec<u8> {
    const W: u32 = 48;
    const H: u32 = 48;
    const M: u32 = 16;
    let orange = [255, 128, 0, 255];
    let cyan = [0, 255, 255, 255];
    let navy = [30, 80, 160, 255];
    let sky = [120, 180, 240, 255];
    let color_at = |x: u32, y: u32| -> [u8; 4] {
        if x < M && y < M {
            RED
        } else if x >= W - M && y < M {
            GREEN
        } else if x < M && y >= H - M {
            BLUE
        } else if x >= W - M && y >= H - M {
            YELLOW
        } else if y < M {
            if x < M + (W - 2 * M) / 2 {
                MAGENTA
            } else {
                WHITE
            }
        } else if y >= H - M {
            if x < M + (W - 2 * M) / 2 {
                orange
            } else {
                cyan
            }
        } else if x < M {
            if y < M + (H - 2 * M) / 2 {
                DGRAY
            } else {
                LGRAY
            }
        } else if x >= W - M {
            if y < M + (H - 2 * M) / 2 {
                navy
            } else {
                sky
            }
        } else {
            CENTER
        }
    };
    let mut rgba = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            rgba.extend_from_slice(&color_at(x, y));
        }
    }
    rgba
}

fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 生成 48x48 全灰灰阶纹理（RGBA；modulate 配方：灰阶/白图配彩色 fill_slot，
/// 面板观感 = 灰阶明暗 x 槽位色）。
fn flat_gray_rgba() -> Vec<u8> {
    const W: u32 = 48;
    const H: u32 = 48;
    let mut rgba = Vec::with_capacity((W * H * 4) as usize);
    for _ in 0..W * H {
        rgba.extend_from_slice(&[200, 200, 200, 255]);
    }
    rgba
}

/// 生成 48x48 平铺测试纹理（RGBA 行主序；边距 16，中带 16）。
///
/// 布局与 [`nine_patch_rgba`] 同族，唯一差别：上边条 16 列**非均匀灰阶**
///（源 x = 16+c -> 灰阶 c*16）—— 拉伸模式整条缩放后列色失真，平铺模式
/// 逐列点对点、每个 16px 单元从源条头重启，多点采样可与源列一一对应。
fn ramp_nine_patch_rgba() -> Vec<u8> {
    const W: u32 = 48;
    const H: u32 = 48;
    const M: u32 = 16;
    let orange = [255, 128, 0, 255];
    let cyan = [0, 255, 255, 255];
    let color_at = |x: u32, y: u32| -> [u8; 4] {
        if x < M && y < M {
            RED
        } else if x >= W - M && y < M {
            GREEN
        } else if x < M && y >= H - M {
            BLUE
        } else if x >= W - M && y >= H - M {
            YELLOW
        } else if y < M {
            let c = (x - M) as u8; // 0..16
            let v = c * 16; // 0..240，灰阶列纹（c < 16 不溢出）
            [v, v, v, 255]
        } else if y >= H - M {
            if x < M + (W - 2 * M) / 2 {
                orange
            } else {
                cyan
            }
        } else if x < M {
            DGRAY
        } else if x >= W - M {
            LGRAY
        } else {
            CENTER
        }
    };
    let mut rgba = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            rgba.extend_from_slice(&color_at(x, y));
        }
    }
    rgba
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 0.0, 0.0, Vec2::new(256.0, 128.0))
}

/// 装配 256x128 消费器 + 已注册的 48x48 九宫纹理（守卫护住整个测试体）。
fn open_canvas() -> (MutexGuard<'static, ()>, Option<CommandConsumer>) {
    let guard = gpu_lock();
    let consumer = match GpuContext::open() {
        Ok(ctx) => {
            let target = RenderTarget::with_size(&ctx, 256, 128).expect("256x128 目标");
            let atlas = SpriteAtlas::new(&ctx).expect("图集");
            match CommandConsumer::new(ctx, target, atlas) {
                Ok(mut consumer) => {
                    consumer
                        .register_texture(tex_key(), 48, 48, &nine_patch_rgba())
                        .expect("注册九宫测试纹理");
                    Some(consumer)
                }
                Err(err) => panic!("消费器装配失败（应如实暴露）：{err}"),
            }
        }
        Err(BackendError::NoLibraryCandidates(tried)) => {
            eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库，已尝试：{tried}");
            return (guard, None);
        }
        Err(err) => panic!("GPU 装配失败（应如实暴露）：{err}"),
    };
    (guard, consumer)
}

/// 无相机提交消费一帧（回退视图 = 单位 —— 世界坐标即视口像素）。
fn flush(consumer: &mut CommandConsumer, server: &mut WgpuRenderServer) -> FrameOutcome {
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("消费一帧")
}

/// 一块九宫格 Control：矩形 (16,16) 起的 `size` 方块 + 16px 边距记录
///（S16.7 两开关缺省 false —— 既有行为）。
fn nine_control(server: &mut WgpuRenderServer, size: f32, margins: [f32; 4]) -> ItemHandle {
    nine_control_on(server, tex_key(), size, margins, [0; 4], false, false)
}

/// 全参版：指定纹理键、fill（modulate 的 tint 色源 = fill_slot 解析载体）
/// 与 S16.7 两开关。
#[allow(clippy::too_many_arguments)]
fn nine_control_on(
    server: &mut WgpuRenderServer,
    key: RenderAssetKey,
    size: f32,
    margins: [f32; 4],
    fill: [u8; 4],
    modulate: bool,
    tiling: bool,
) -> ItemHandle {
    let h = server.create_item(RenderAssetKey::NIL);
    server.set_z(h, 0, 0);
    let mut state = ControlState::new([0.0; 4], [16.0, 16.0, 16.0 + size, 16.0 + size]);
    state.fill = fill;
    server.set_rect(h, &state);
    server.set_nine_slice(
        h,
        key,
        margins[0],
        margins[1],
        margins[2],
        margins[3],
        modulate,
        tiling,
    );
    h
}

/// T-9S-01：96x96 面板（源 48x48 等比 2x）—— 四角 1:1 不变形、中心
/// 纯色、上边条水平单向拉伸的列映射、左边条垂直单向拉伸的行映射。
#[test]
fn t_9s_01_corners_1to1_edges_stretch() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    nine_control(&mut server, 96.0, [16.0, 16.0, 16.0, 16.0]);

    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.controls, 1, "九宫格面板仍是 1 个控件条目");
    assert_eq!(outcome.stats.drawn, 9, "九片全发（96x96 无退化）");
    let image = &outcome.image;

    // 四角 1:1：目标 (16..32) 方块逐像素 = 源角块（探针离边界 >= 3px，
    // 邻片过渡不掺样）。目标角块 16px，探针取角内 4.5px 处。
    assert_eq!(image.pixel(20, 20), Some(RED), "左上角 = 源 (4,4)");
    assert_eq!(image.pixel(17, 17), Some(RED), "左上角贴角点 (1,1) = 源 (1,1)");
    assert_eq!(image.pixel(100, 20), Some(GREEN), "右上角 = 源右角（水平翻位）");
    assert_eq!(image.pixel(20, 100), Some(BLUE), "左下角 = 源下角（垂直翻位）");
    assert_eq!(image.pixel(100, 100), Some(YELLOW), "右下角 = 源右下角");

    // 中心双向拉伸：源中心纯色，任意采样点都同色。
    assert_eq!(image.pixel(64, 64), Some(CENTER), "面板中心 = 源中心色");

    // 上边条（目标 32..96 x 16..32 <- 源 16..32 x 0..16，水平 4x 拉伸、
    // 垂直 1:1）：中点映射源中点列（texel 24 = 白/品红分界右侧），四分
    // 位映射源左半（品红）；同行两探针 = 垂直不拉伸。
    assert_eq!(image.pixel(64, 20), Some(WHITE), "上边中点 = 源上边条中点列色");
    assert_eq!(image.pixel(64, 28), Some(WHITE), "上边中点下移 8px 同色（垂直 1:1）");
    assert_eq!(image.pixel(40, 20), Some(MAGENTA), "上边四分位 = 源左半条色");
    assert_eq!(image.pixel(88, 20), Some(WHITE), "上边四分之三 = 源右半条色");

    // 左边条（目标 16..32 x 32..96 <- 源 0..16 x 16..32，垂直 4x 拉伸）：
    // 探针行映射源行（上半暗灰 / 下半亮灰）。
    assert_eq!(image.pixel(20, 60), Some(DGRAY), "左边条上半 = 源上带色");
    assert_eq!(image.pixel(20, 76), Some(LGRAY), "左边条下半 = 源下带色");

    // 面板之外是清屏色（矩形几何未被九宫格改变）。
    assert_eq!(image.pixel(8, 8), Some(CLEAR_RGBA));
    assert_eq!(image.pixel(120, 64), Some(CLEAR_RGBA));
}

/// T-9S-02：30x30 < 源边距和（16x2 = 32）—— 边距钳制 min 公式生效：
/// 无 panic、四角完整（源锚纹理真角：TR 角仍是绿的）、中带全零合法退化
///（只发 4 片角）。
#[test]
fn t_9s_02_margin_clamp_degenerate() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    nine_control(&mut server, 30.0, [16.0, 16.0, 16.0, 16.0]);

    // 钳制后边距 = min(16, 30/2) = 15：四角各 15x15，恰好铺满 30x30。
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.controls, 1);
    assert_eq!(outcome.stats.drawn, 4, "零中段退化：只发四角片");
    let image = &outcome.image;

    // 四角完整：TR/BR 角锚纹理右缘 —— 采到的仍是角色（条纹不进角）。
    assert_eq!(image.pixel(20, 20), Some(RED), "左上角 15x15 内");
    assert_eq!(image.pixel(40, 20), Some(GREEN), "右上角锚纹理真角（右缘回退切割）");
    assert_eq!(image.pixel(20, 40), Some(BLUE), "左下角锚纹理真角（下缘回退切割）");
    assert_eq!(image.pixel(40, 40), Some(YELLOW), "右下角锚双缘");
    // 钳制边界衔接：15 分界两侧角块相邻无缝（x=30 仍红、x=31 已绿）。
    assert_eq!(image.pixel(30, 20), Some(RED));
    assert_eq!(image.pixel(31, 20), Some(GREEN));
    // 面板外清屏色。
    assert_eq!(image.pixel(8, 20), Some(CLEAR_RGBA));
    assert_eq!(image.pixel(50, 50), Some(CLEAR_RGBA));

    // 更极端：1x1 面板（边距钳到 0.5，亚像素角）—— 不 panic、几何合法。
    let mut server = WgpuRenderServer::new();
    nine_control(&mut server, 1.0, [16.0, 16.0, 16.0, 16.0]);
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.controls, 1, "1x1 面板仍是合法控件条目");
}

/// T-9S-03：无 ns 记录的 Control → 与既有路径逐位同（fill/border 照旧）。
#[test]
fn t_9s_03_no_record_is_bit_identical_legacy() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = server.create_item(RenderAssetKey::NIL);
    server.set_z(h, 0, 0);
    let mut state = ControlState::new([0.0; 4], [16.0, 16.0, 112.0, 112.0]);
    state.fill = [200, 10, 10, 255];
    state.border = [0, 255, 0, 255];
    server.set_rect(h, &state);

    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.controls, 1);
    let image = &outcome.image;
    // E-1 既有事实：1px 平直边框 + 内部填充。
    assert_eq!(image.pixel(16, 16), Some([0, 255, 0, 255]), "边框像素照旧");
    assert_eq!(image.pixel(17, 17), Some([200, 10, 10, 255]), "内部填充照旧");
    assert_eq!(image.pixel(64, 64), Some([200, 10, 10, 255]), "中心填充照旧");
}

/// T-9S-04：同键覆写 / NIL 清除 —— 改边距即改像素（后写者生效）；NIL
/// 清除后整帧回基线**逐位**（`set_clip(None)` 同款清除先例的像素面）。
#[test]
fn t_9s_04_overwrite_and_clear_semantics() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };

    // 基线：从未设过九宫格的同位控件（fill/border 路径）。
    let mut server = WgpuRenderServer::new();
    let h = server.create_item(RenderAssetKey::NIL);
    server.set_z(h, 0, 0);
    let mut state = ControlState::new([0.0; 4], [16.0, 16.0, 112.0, 112.0]);
    state.fill = [200, 10, 10, 255];
    server.set_rect(h, &state);
    let baseline = flush(&mut consumer, &mut server);

    // 设九宫格：像素离开基线（哨兵：模式切换必须可见）。
    server.set_nine_slice(h, tex_key(), 16.0, 16.0, 16.0, 16.0, false, false);
    let sliced = flush(&mut consumer, &mut server);
    assert_ne!(sliced.image.rgba, baseline.image.rgba, "九宫格切换可见");
    assert_eq!(sliced.image.pixel(20, 20), Some(RED), "左上角出现纹理角色");

    // 同键覆写（左边距 16 -> 8）：后写者生效 —— 左角块缩到 8px，上边条
    // 左端改采源角块右侧的红色 texel，品红/白分界从目标 x=64 右移到
    // x=72：探针 (68,20) 由白（l=16）变品红（l=8）。
    server.set_nine_slice(h, tex_key(), 8.0, 16.0, 16.0, 16.0, false, false);
    let overwritten = flush(&mut consumer, &mut server);
    assert_ne!(overwritten.image.rgba, sliced.image.rgba, "覆写改变像素");
    assert_eq!(
        overwritten.image.pixel(68, 20),
        Some(MAGENTA),
        "l=8 后品红/白分界右移：(68,20) 落回品红半条"
    );
    assert_eq!(
        sliced.image.pixel(68, 20),
        Some(WHITE),
        "哨兵：l=16 时同点在白半条（覆写确实改变了映射）"
    );

    // 输出序：SetNineSlice 恒在 SetPivot 之后（契约冻结链尾）。
    server.set_pivot(h, [0.5, 0.5]);
    let mut commands = Vec::new();
    server.submit_into(&frame(1), &mut commands);
    let pivot_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetPivot { .. }));
    let nine_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetNineSlice { .. }));
    assert!(pivot_at.is_some() && nine_at.is_some());
    assert!(nine_at > pivot_at, "SetNineSlice 恒在 SetPivot 之后");

    // NIL 清除：整帧逐位回基线（无记录 = fill/border 照旧）。
    server.set_nine_slice(h, RenderAssetKey::NIL, 0.0, 0.0, 0.0, 0.0, false, false);
    let cleared = flush(&mut consumer, &mut server);
    assert_eq!(
        cleared.image.rgba, baseline.image.rgba,
        "NIL 清除后整帧逐位回基线"
    );

    // 销毁：九宫格随条目消亡 —— 命令流不再出现 SetNineSlice、像素消失。
    server.set_nine_slice(h, tex_key(), 16.0, 16.0, 16.0, 16.0, false, false);
    server.destroy_item(h);
    let mut commands = Vec::new();
    server.submit_into(&frame(2), &mut commands);
    assert!(
        commands
            .iter()
            .all(|c| !matches!(c, RenderCommand::SetNineSlice { .. })),
        "销毁后命令流不再出现 SetNineSlice"
    );
    let dead = consumer.consume(&commands).expect("消费销毁帧");
    assert_eq!(dead.image.pixel(20, 20), Some(CLEAR_RGBA), "条目已消失");
}

/// T-NS-M-01（S16.7 模态染色）：灰阶纹理 + modulate + fill 红 → 面板
/// 逐通道 = 灰 x 红（手算：200/255 x [1,0,0,1] -> [200,0,0,255]，unorm
/// 逐通道舍入）；modulate=false 对照 = 纯灰（中性 tint 恒等）。
#[test]
fn t_ns_m_01_modulate_tints_panel_by_fill() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    consumer
        .register_texture(gray_key(), 48, 48, &flat_gray_rgba())
        .expect("register gray texture");

    // modulate = true：九实例 tint = fill（fill_slot 解析色的既有载体）。
    // 像素证据：中心 / 角 / 上条三类片同受染色 —— tint 是九片共享的。
    let mut server = WgpuRenderServer::new();
    nine_control_on(&mut server, gray_key(), 96.0, [16.0; 4], RED, true, false);
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.controls, 1);
    assert_eq!(outcome.stats.drawn, 9, "modulate does not change piece count");
    let modulated: [u8; 4] = [200, 0, 0, 255];
    assert_eq!(
        outcome.image.pixel(64, 64),
        Some(modulated),
        "center = gray x red (200/255 x [1,0,0,1])"
    );
    assert_eq!(outcome.image.pixel(20, 20), Some(modulated), "corner tinted too");
    assert_eq!(outcome.image.pixel(64, 20), Some(modulated), "top edge tinted too");

    // modulate = false 对照：中性白 tint = 纹理原色（纯灰），fill 不参与。
    let mut server = WgpuRenderServer::new();
    nine_control_on(&mut server, gray_key(), 96.0, [16.0; 4], RED, false, false);
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(
        outcome.image.pixel(64, 64),
        Some([200, 200, 200, 255]),
        "switch off = neutral tint, plain gray"
    );
}

/// T-NS-T-01（S16.7 中间条平铺）：上条源列非均匀灰阶 → 平铺模式多点
/// 采样与源列一一对应（每 16px 单元从源条头重启、角不参与平铺）；
/// 拉伸对照同点失真；实例计数 36；4128px 面板 x 16px 单元触发 256 片
/// 上限，截断片数进 `stats.nines_truncated` 诊断。
#[test]
fn t_ns_t_01_tiling_keeps_native_columns() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    consumer
        .register_texture(ramp_key(), 48, 48, &ramp_nine_patch_rgba())
        .expect("register ramp texture");

    // tiling = true：上条目标区长 64px = 4 个原生 16px 单元；条从
    // rect.x + l = 32 起，像素 (32+k, 20) 的中心映射源列 16 + k%16，
    // 灰阶 = (k%16)*16（平铺单元 = 声明边距推导的源条原生长度
    // 48-16-16 = 16px）。
    let mut server = WgpuRenderServer::new();
    nine_control_on(&mut server, ramp_key(), 96.0, [16.0; 4], [0; 4], false, true);
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.controls, 1);
    // 实例计数：角 4 + 上/下/左/右条各 4 + 中心 4x4 = 36（拉伸恒 9）。
    assert_eq!(outcome.stats.drawn, 36, "tiling unfolds at native source size");
    assert_eq!(outcome.stats.nines_truncated, 0, "cap not reached");
    let image = &outcome.image;
    assert_eq!(image.pixel(32, 20), Some([0, 0, 0, 255]), "tile 1 col 0 -> ramp 0");
    assert_eq!(image.pixel(33, 20), Some([16, 16, 16, 255]), "tile 1 col 1");
    assert_eq!(image.pixel(47, 20), Some([240, 240, 240, 255]), "tile 1 col 15");
    assert_eq!(
        image.pixel(48, 20),
        Some([0, 0, 0, 255]),
        "tile 2 restarts from strip head"
    );
    assert_eq!(image.pixel(56, 20), Some([128, 128, 128, 255]), "tile 2 col 8");
    assert_eq!(image.pixel(64, 20), Some([0, 0, 0, 255]), "tile 3 restarts");
    assert_eq!(image.pixel(95, 20), Some([240, 240, 240, 255]), "tile 4 last col");
    assert_eq!(image.pixel(96, 20), Some(GREEN), "corner stays 1:1 (not tiled)");
    assert_eq!(image.pixel(64, 64), Some(CENTER), "center tiles keep source color");

    // 拉伸对照：同点整条缩放后采样失真（源 16..32 被拉到 64px 长条里），
    // 平铺的原生列色不可复现 —— 这就是平铺开关存在的像素理由。
    let mut server = WgpuRenderServer::new();
    nine_control_on(&mut server, ramp_key(), 96.0, [16.0; 4], [0; 4], false, false);
    let stretched = flush(&mut consumer, &mut server);
    assert_eq!(stretched.stats.drawn, 9, "stretch mode stays nine pieces");
    assert_ne!(
        stretched.image.pixel(47, 20),
        image.pixel(47, 20),
        "stretch distorts the column color that tiling keeps native"
    );

    // 上限：4128px 方块面板 x 16px 单元 -> 上条恰 256 片吃满预算，其余按
    // 视觉序截断（左条 256 + 中心 256x256 = 65536 + 右条 256 + 下条 256）
    // —— 诊断计数逐片累加（大面板 x 小平铺单元的触发形态）。
    let mut server = WgpuRenderServer::new();
    nine_control_on(&mut server, ramp_key(), 4128.0, [16.0; 4], [0; 4], false, true);
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.controls, 1, "off-target panel still a control entry");
    assert_eq!(
        outcome.stats.drawn,
        4 + 256,
        "corners 4 + budgeted top-strip tiles 256"
    );
    assert_eq!(
        outcome.stats.nines_truncated,
        256 + 65536 + 256 + 256,
        "left/center/right/bottom truncated piece by piece"
    );
}

/// T-NS-T-02（S16.7 组合）：modulate + tiling 同开 —— 平铺片与角同受
/// fill 染色（tint 与平铺几何正交：染色只换 tint，平铺只换片几何）。
#[test]
fn t_ns_t_02_modulate_and_tiling_combined() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    consumer
        .register_texture(ramp_key(), 48, 48, &ramp_nine_patch_rgba())
        .expect("register ramp texture");

    let mut server = WgpuRenderServer::new();
    nine_control_on(&mut server, ramp_key(), 96.0, [16.0; 4], RED, true, true);
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.controls, 1);
    assert_eq!(outcome.stats.drawn, 36, "piece count unchanged when combined");
    assert_eq!(outcome.stats.nines_truncated, 0);
    let image = &outcome.image;
    // 平铺片灰阶 128 x 红：R = 128x1 = 128、G/B = 128x0 = 0。
    assert_eq!(
        image.pixel(40, 20),
        Some([128, 0, 0, 255]),
        "tiled piece carries the modulate tint"
    );
    // 中心片（源中心 [40,40,60]）x 红。
    assert_eq!(image.pixel(64, 64), Some([40, 0, 0, 255]), "center tile tinted");
    // 角同受染色：红角 x 红 fill = 原样；绿角 x 红 fill -> R 通道归零。
    assert_eq!(image.pixel(20, 20), Some(RED), "red corner x red fill unchanged");
    assert_eq!(image.pixel(100, 20), Some([0, 0, 0, 255]), "green corner x red fill");
}
