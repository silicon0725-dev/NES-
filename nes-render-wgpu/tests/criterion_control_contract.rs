//! T-Control 契约回归：控件（`SetRect`）HUD 口径的契约面逐项钉死（S4.5）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Control-01 | FULL_RECT 铺满**相机视口**，边框贴边、内部透明 |
//! | T-Control-02 | 任意像素矩形的边框位置 |
//! | T-Control-03 | 锚点按视口解析（`ControlState::resolve` 唯一权威算式，0.5 锚点） |
//! | T-Control-04 | 内部透明透出下层精灵（alpha 丢弃，无混合） |
//! | T-Control-05 | **HUD 不随相机移动**（逆视图折算）：相机 zoom/平移时精灵动、控件不动 |
//! | T-Control-06 | 可见性跳过与销毁注销（不可见 != 销毁） |
//! | T-Control-07 | `SetRect` 更新下一帧生效（位置热更新） |
//! | T-Control-08 | 锚点的**父尺寸 = 相机视口**（视口小于画布时矩形只铺视口） |
//!
//! 边框像素厚度随缩放与驱动取整浮动（2~4px），是非契约量；锚点取无歧义位置。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{
    Affine2, Camera2DState, ControlState, FrameInfo, RenderAssetKey, RenderServer, Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
const EYE_RGBA: [u8; 4] = [250, 250, 250, 255];
const CONTROL_RGBA: [u8; 4] = [0, 255, 0, 255];

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

/// 装配 256x128 消费器（守卫须绑定到测试作用域）。
fn open_canvas() -> (MutexGuard<'static, ()>, Option<CommandConsumer>) {
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
    (guard, Some(consumer))
}

/// 单位相机（视口 = 画布）+ 提交消费一帧。
fn flush(consumer: &mut CommandConsumer, server: &mut WgpuRenderServer) -> FrameOutcome {
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(128.0, 64.0);
    server.set_camera(&camera);
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("消费一帧")
}

fn control_at(
    server: &mut WgpuRenderServer,
    offsets: [f32; 4],
    z: i32,
) -> nes_render_api::ItemHandle {
    let handle = server.create_item(RenderAssetKey::NIL);
    server.set_z(handle, z, 0);
    server.set_rect(handle, &ControlState::new([0.0; 4], offsets));
    handle
}

/// T-Control-01：FULL_RECT 铺满视口，边框贴边、内部透明。
#[test]
fn t_control_01_full_rect_covers_viewport() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::NIL);
    server.set_rect(handle, &ControlState::FULL_RECT);
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.controls, 1);
    let image = &outcome.image;
    assert_eq!(image.pixel(0, 0), Some(CONTROL_RGBA), "视口左上角");
    // E-1（S12.1）：边框是像素精确的 1px 平直条 —— (1,1) 已是内部。
    assert_eq!(image.pixel(1, 1), Some(CLEAR_RGBA), "边框恰 1px（内一格即透明）");
    assert_eq!(image.pixel(255, 127), Some(CONTROL_RGBA), "视口右下角");
    assert_eq!(image.pixel(16, 16), Some(CLEAR_RGBA), "内部透明");
    assert_eq!(image.pixel(128, 64), Some(CLEAR_RGBA), "中心透出背景");
}

/// T-Control-02：任意像素矩形（8,8,48,48）的边框位置。
#[test]
fn t_control_02_pixel_rect_position() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    control_at(&mut server, [8.0, 8.0, 48.0, 48.0], 0);
    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(image.pixel(8, 8), Some(CONTROL_RGBA), "矩形角点");
    assert_eq!(image.pixel(9, 9), Some(CLEAR_RGBA), "边框恰 1px（E-1 平直边框）");
    assert_eq!(image.pixel(11, 11), Some(CLEAR_RGBA), "深入内部透明");
    assert_eq!(image.pixel(60, 60), Some(CLEAR_RGBA), "矩形（8..56）之外");
    assert_eq!(image.pixel(4, 4), Some(CLEAR_RGBA), "矩形之外（左上）");
}

/// T-Control-03：锚点按视口解析 —— 四边 0.5 锚点 + ±16 偏移 -> (112,48,32,32)。
#[test]
fn t_control_03_anchor_resolution() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::NIL);
    server.set_rect(
        handle,
        &ControlState::new([0.5, 0.5, 0.5, 0.5], [-16.0, -16.0, 16.0, 16.0]),
    );
    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(image.pixel(112, 48), Some(CONTROL_RGBA), "0.5 锚点矩形角 (112,48)");
    assert_eq!(image.pixel(111, 47), Some(CLEAR_RGBA), "矩形外");
    assert_eq!(image.pixel(120, 56), Some(CLEAR_RGBA), "矩形内透明");
}

/// T-Control-04：内部透明透出下层精灵（alpha 丢弃，无混合）。
#[test]
fn t_control_04_transparent_interior() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let sprite = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(sprite, Affine2::translation(20.0, 20.0));
    server.set_z(sprite, 0, 0);
    control_at(&mut server, [16.0, 16.0, 32.0, 32.0], 1);
    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(image.pixel(23, 23), Some(EYE_RGBA), "精灵眼睛透过控件内部");
    assert_eq!(image.pixel(20, 20), Some([255, 0, 0, 255]), "精灵主体透过");
    assert_eq!(image.pixel(16, 16), Some(CONTROL_RGBA), "控件边框仍在最上层");
}

/// T-Control-05（架构口径）：HUD 不随相机移动 —— 相机 zoom=2 且注视别处时，
/// 精灵随视图缩放移动，控件边框**钉在视口位置**（逆视图折算）。
#[test]
fn t_control_05_hud_immovable_under_camera() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let sprite = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(sprite, Affine2::translation(60.0, 60.0));
    server.set_z(sprite, 0, 0);
    control_at(&mut server, [16.0, 16.0, 48.0, 48.0], 1);

    // 相机：zoom 2、中心 (64,64) -> 世界 (60,60) 映射到视口 (120,56)。
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(64.0, 64.0);
    camera.zoom = Vec2::new(2.0, 2.0);
    server.set_camera(&camera);
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("消费一帧");

    let image = &outcome.image;
    assert_eq!(image.pixel(120, 56), Some([255, 0, 0, 255]), "精灵随相机缩放移动到 (120,56)");
    assert_eq!(image.pixel(126, 62), Some(EYE_RGBA), "精灵眼睛 (63,63) -> (126,62)");
    assert_eq!(image.pixel(16, 16), Some(CONTROL_RGBA), "控件钉在视口 (16,16)");
    assert_eq!(image.pixel(17, 17), Some(CLEAR_RGBA), "边框恰 1px（E-1）");
    assert_eq!(image.pixel(30, 30), Some(CLEAR_RGBA), "控件内部（无精灵处）背景");
    // 若控件错误地走了视图矩阵，边框会落到 ((16-64)*2+128, (16-64)*2+64)=(32,-16)：
    // (16,16) 将是背景 -> 上面的断言即是本契约的反证锚点。
}

/// T-Control-06：可见性跳过与销毁注销。
#[test]
fn t_control_06_visibility_and_destroy() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = control_at(&mut server, [8.0, 8.0, 32.0, 32.0], 0);

    server.set_visible(handle, false);
    let hidden = flush(&mut consumer, &mut server);
    assert_eq!(hidden.stats.controls, 0, "不可见不画");
    assert_eq!(hidden.stats.skipped, 1);

    server.set_visible(handle, true);
    let shown = flush(&mut consumer, &mut server);
    assert_eq!(shown.stats.controls, 1, "恢复可见即回来");
    assert_eq!(shown.image.pixel(8, 8), Some(CONTROL_RGBA));

    server.destroy_item(handle);
    let dead = flush(&mut consumer, &mut server);
    assert_eq!(dead.stats.controls, 0, "销毁后注销");
    assert_eq!(dead.image.pixel(8, 8), Some(CLEAR_RGBA));
}

/// T-Control-07：`SetRect` 位置热更新，下一帧生效。
#[test]
fn t_control_07_rect_hot_update() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = control_at(&mut server, [8.0, 8.0, 32.0, 32.0], 0);
    let before = flush(&mut consumer, &mut server);
    assert_eq!(before.image.pixel(8, 8), Some(CONTROL_RGBA));

    // 注意 offsets 是四边偏移（left/top/right/bottom），不是 (x,y,w,h)：
    // 矩形 (64,8)-(96,40) 写作 [64, 8, 96, 40]。（写错次序会得到负宽 —— 契约
    // 规定负宽高原样透传，表现为四边形镜像，本用例顺带钉住这个语义。）
    server.set_rect(handle, &ControlState::new([0.0; 4], [64.0, 8.0, 96.0, 40.0]));
    let after = flush(&mut consumer, &mut server);
    assert_eq!(after.image.pixel(8, 8), Some(CLEAR_RGBA), "旧位置空出");
    assert_eq!(after.image.pixel(64, 8), Some(CONTROL_RGBA), "新位置生效");
}

/// T-Control-08：锚点的父尺寸 = **相机视口**（不是画布）—— 相机视口 128x128
/// 时 FULL_RECT 只铺画布左半。
#[test]
fn t_control_08_parent_is_camera_viewport() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::NIL);
    server.set_rect(handle, &ControlState::FULL_RECT);

    // 相机视口 128x128（画布 256x128），中心 (64,64) 保持单位视图。
    let mut camera = Camera2DState::new(Vec2::new(128.0, 128.0));
    camera.transform = Affine2::translation(64.0, 64.0);
    server.set_camera(&camera);
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("消费一帧");

    let image = &outcome.image;
    assert_eq!(image.pixel(0, 0), Some(CONTROL_RGBA), "视口左上");
    assert_eq!(image.pixel(1, 1), Some(CONTROL_RGBA));
    assert_eq!(image.pixel(100, 100), Some(CLEAR_RGBA), "视口内部透明");
    assert_eq!(image.pixel(200, 60), Some(CLEAR_RGBA), "视口（128 宽）之外的画布区域不受影响");
}
