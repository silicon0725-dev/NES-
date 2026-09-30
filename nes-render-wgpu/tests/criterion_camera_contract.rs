//! T-Camera 契约回归：相机（`SetCamera` -> `view_matrix`）的契约面逐项钉死（S4.5）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Camera-01 | 单位口径：相机中心 = 视口半尺寸时视图矩阵恰为单位（世界 == 像素） |
//! | T-Camera-02 | 平移：注视点搬到视口中心 |
//! | T-Camera-03 | 缩放按轴生效；非正 zoom 退化为 1（`effective_zoom` 契约） |
//! | T-Camera-04 | 旋转：视图绕视口中心旋转（纹素区间映射可精确预计） |
//! | T-Camera-05 | `enabled == false`：帧本地回退单位视图（I9 的"不做变换"分支） |
//! | T-Camera-06 | 视口非法（0）：回退单位视图 + 目标尺寸（防除零） |
//! | T-Camera-07 | 无相机：单位视图，`camera_applied == false` |
//! | T-Camera-08 | 逐条消费语义：流内多条 `SetCamera` 时**最后一条生效** |
//!
//! 眼睛位置一律按**纹素区间**（[3,4)）经视图矩阵映射后所覆盖的像素计算，
//! 不用纹素中心（flip/旋转两轮实证过的口径）。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::command::RenderCommand;
use nes_render_api::{
    Affine2, Camera2DState, FrameInfo, RenderAssetKey, RenderServer, Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
const BODY_RGBA: [u8; 4] = [255, 0, 0, 255];
const EYE_RGBA: [u8; 4] = [250, 250, 250, 255];

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

/// 建一个精灵（槽 16 -> 图案格）并放到世界 (x,y)。
fn sprite_at(server: &mut WgpuRenderServer, x: f32, y: f32) -> nes_render_api::ItemHandle {
    let handle = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(handle, Affine2::translation(x, y));
    handle
}

/// 用给定相机提交消费一帧。
fn flush_with(
    consumer: &mut CommandConsumer,
    server: &mut WgpuRenderServer,
    camera: &Camera2DState,
) -> FrameOutcome {
    server.set_camera(camera);
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("消费一帧")
}

/// 不设相机直接提交（T-Camera-07 用）。
fn flush_no_camera(
    consumer: &mut CommandConsumer,
    server: &mut WgpuRenderServer,
) -> FrameOutcome {
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("消费一帧")
}

/// T-Camera-01：单位口径 —— 中心 = 视口半尺寸 -> 视图矩阵恰为单位。
#[test]
fn t_camera_01_identity_convention() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 10.0, 10.0);
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(128.0, 64.0);
    let outcome = flush_with(&mut consumer, &mut server, &camera);
    assert!(outcome.stats.camera_applied);
    let image = &outcome.image;
    assert_eq!(image.pixel(10, 10), Some(BODY_RGBA), "世界 == 像素");
    assert_eq!(image.pixel(13, 13), Some(EYE_RGBA));
    assert_eq!(image.pixel(9, 9), Some(CLEAR_RGBA));
}

/// T-Camera-02：平移 —— 注视点 (16,32) 搬到视口中心 (128,64)。
#[test]
fn t_camera_02_pan_centers_focus() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 16.0, 32.0);
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(16.0, 32.0);
    let outcome = flush_with(&mut consumer, &mut server, &camera);
    assert!(outcome.stats.camera_applied);
    let image = &outcome.image;
    // 视图 = T(112,32)：世界 (16,32) -> (128,64)，眼睛 (19,35) -> (131,67)。
    assert_eq!(image.pixel(128, 64), Some(BODY_RGBA), "注视点居中");
    assert_eq!(image.pixel(131, 67), Some(EYE_RGBA));
    assert_eq!(image.pixel(127, 63), Some(CLEAR_RGBA));
}

/// T-Camera-03：缩放按轴生效；负 zoom 退化为 1（`effective_zoom` 契约）。
#[test]
fn t_camera_03_zoom_per_axis_and_negative() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 32.0, 32.0);
    // zoom = (-1, 2)：x 轴非正 -> 退化为 1；y 轴 2 倍。
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(64.0, 64.0);
    camera.zoom = Vec2::new(-1.0, 2.0);
    let outcome = flush_with(&mut consumer, &mut server, &camera);
    assert!(outcome.stats.camera_applied);
    let image = &outcome.image;
    // 世界 (32,32) -> x=(32-64)+128=96，y=(32-64)*2+64=0；四边形 [96,112)x[0,32)。
    assert_eq!(image.pixel(96, 0), Some(BODY_RGBA), "x 轴单位缩放");
    assert_eq!(image.pixel(111, 31), Some(BODY_RGBA), "y 轴 2 倍（高 32px）");
    assert_eq!(image.pixel(112, 0), Some(CLEAR_RGBA), "x 轴未放大（宽 16px）");
    // 眼睛 [35,36)^2 -> 屏幕 x [99,100) -> 像素 99；y [6,8) -> 像素 6。
    assert_eq!(image.pixel(99, 6), Some(EYE_RGBA));
    assert_eq!(image.pixel(99, 7), Some(EYE_RGBA), "y 方向眼睛拉高 2 像素");
}

/// T-Camera-04：旋转 —— 视图绕视口中心旋转（相机旋转 90°，中心在世界原点）。
#[test]
fn t_camera_04_rotation() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 10.0, 10.0);
    // view = T(128,64) ∘ R(-90°)：(x,y) -> (y,-x) 再平移。
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::rotation(std::f32::consts::FRAC_PI_2);
    let outcome = flush_with(&mut consumer, &mut server, &camera);
    assert!(outcome.stats.camera_applied);
    let image = &outcome.image;
    // 四边形世界 [10,26]^2 -> 屏幕 x∈[138,154)、y∈[38,54)；中心世界 (18,18)->(146,46)。
    assert_eq!(image.pixel(146, 46), Some(BODY_RGBA), "旋转后的四边形中心");
    // 眼睛纹素 [13,14)^2 -> 屏幕 x∈[141,142)、y∈(50,51] -> 像素 (141,50)。
    assert_eq!(image.pixel(141, 50), Some(EYE_RGBA), "旋转后眼睛 (141,50)");
    assert_eq!(image.pixel(137, 55), Some(CLEAR_RGBA), "段外");
    assert_eq!(image.pixel(155, 37), Some(CLEAR_RGBA), "段外");
}

/// T-Camera-05：`enabled == false` -> 帧本地回退单位视图（I9 后半分支）。
#[test]
fn t_camera_05_disabled_falls_back() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 10.0, 10.0);
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(200.0, 200.0); // 若生效，精灵会飞出画布
    camera.enabled = false;
    let outcome = flush_with(&mut consumer, &mut server, &camera);
    assert!(!outcome.stats.camera_applied);
    assert_eq!(outcome.image.pixel(13, 13), Some(EYE_RGBA), "退回单位视图");
}

/// T-Camera-06：视口非法（0）-> 回退单位视图 + 目标尺寸（防除零）。
#[test]
fn t_camera_06_invalid_viewport_falls_back() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 10.0, 10.0);
    let mut camera = Camera2DState::new(Vec2::ZERO);
    camera.transform = Affine2::translation(200.0, 200.0);
    let outcome = flush_with(&mut consumer, &mut server, &camera);
    assert!(!outcome.stats.camera_applied, "非法视口视同未生效");
    assert_eq!(outcome.image.pixel(13, 13), Some(EYE_RGBA), "回退单位视图");
    assert_eq!(outcome.stats.driver_errors, 0, "不得产生除零 NaN 坐标类错误");
}

/// T-Camera-07：无相机 -> 单位视图。
#[test]
fn t_camera_07_no_camera_identity() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 10.0, 10.0);
    let outcome = flush_no_camera(&mut consumer, &mut server);
    assert!(!outcome.stats.camera_applied);
    let image = &outcome.image;
    assert_eq!(image.pixel(10, 10), Some(BODY_RGBA));
    assert_eq!(image.pixel(13, 13), Some(EYE_RGBA));
}

/// T-Camera-08：逐条消费 —— 流内多条 `SetCamera`，最后一条生效。
/// （契约层服务端每帧只发一条；本用例钉的是消费器对命令流的处理语义。）
#[test]
fn t_camera_08_last_write_wins() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = sprite_at(&mut server, 10.0, 10.0);

    // 手工拼流：属性 + 相机A（远处，若生效精灵出画）+ 相机B（单位）+ Submit。
    let far = {
        let mut c = Camera2DState::new(Vec2::new(256.0, 128.0));
        c.transform = Affine2::translation(200.0, 200.0);
        c
    };
    let identity = {
        let mut c = Camera2DState::new(Vec2::new(256.0, 128.0));
        c.transform = Affine2::translation(128.0, 64.0);
        c
    };
    let commands = vec![
        RenderCommand::CreateItem {
            handle,
            key: RenderAssetKey::from_parts(16, 1),
        },
        RenderCommand::SetTransform {
            handle,
            transform: Affine2::translation(10.0, 10.0),
        },
        RenderCommand::SetCamera { camera: far },
        RenderCommand::SetCamera { camera: identity },
        RenderCommand::Submit { frame: frame(0) },
    ];
    let outcome = consumer.consume(&commands).expect("消费一帧");
    assert!(outcome.stats.camera_applied);
    let image = &outcome.image;
    assert_eq!(image.pixel(13, 13), Some(EYE_RGBA), "最后一条相机（单位）生效");
    assert_eq!(image.pixel(10, 10), Some(BODY_RGBA));
}
