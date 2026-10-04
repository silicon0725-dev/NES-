//! S16.6（Control 九宫格纹理渲染）契约：`SetNineSlice` 的像素事实。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-9S-01 | Control 96x96（源 48x48 等比 2x）：四角像素 = 源四角对应像素（1:1 不变形）；中心 = 源中心色；上边中点 = 源上边条对应列色（单向拉伸）；左边条 = 垂直单向拉伸 |
//! | T-9S-02 | Control 30x30 < 源边距和（32）：边距钳制（min 公式）生效 —— 无 panic、四角完整（锚纹理真角）、零中段合法退化 |
//! | T-9S-03 | 无 ns 记录的 Control → 输出与既有路径逐位同（fill/border 照旧） |
//! | T-9S-04 | 同键覆写 / NIL 清除：改边距即改像素；NIL 清除后整帧回基线**逐位**；输出序 SetNineSlice 恒在 SetPivot 之后 |
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

/// 一块九宫格 Control：矩形 (16,16) 起的 `size` 方块 + 16px 边距记录。
fn nine_control(server: &mut WgpuRenderServer, size: f32, margins: [f32; 4]) -> ItemHandle {
    let h = server.create_item(RenderAssetKey::NIL);
    server.set_z(h, 0, 0);
    let half = size * 0.5;
    server.set_rect(
        h,
        &ControlState::new([0.0; 4], [16.0, 16.0, 16.0 + size, 16.0 + size]),
    );
    server.set_nine_slice(
        h,
        tex_key(),
        margins[0],
        margins[1],
        margins[2],
        margins[3],
    );
    let _ = half;
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
    server.set_nine_slice(h, tex_key(), 16.0, 16.0, 16.0, 16.0);
    let sliced = flush(&mut consumer, &mut server);
    assert_ne!(sliced.image.rgba, baseline.image.rgba, "九宫格切换可见");
    assert_eq!(sliced.image.pixel(20, 20), Some(RED), "左上角出现纹理角色");

    // 同键覆写（左边距 16 -> 8）：后写者生效 —— 左角块缩到 8px，上边条
    // 左端改采源角块右侧的红色 texel，品红/白分界从目标 x=64 右移到
    // x=72：探针 (68,20) 由白（l=16）变品红（l=8）。
    server.set_nine_slice(h, tex_key(), 8.0, 16.0, 16.0, 16.0);
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
    server.set_nine_slice(h, RenderAssetKey::NIL, 0.0, 0.0, 0.0, 0.0);
    let cleared = flush(&mut consumer, &mut server);
    assert_eq!(
        cleared.image.rgba, baseline.image.rgba,
        "NIL 清除后整帧逐位回基线"
    );

    // 销毁：九宫格随条目消亡 —— 命令流不再出现 SetNineSlice、像素消失。
    server.set_nine_slice(h, tex_key(), 16.0, 16.0, 16.0, 16.0);
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
