//! T-Sprite 契约回归：精灵渲染的契约面逐项钉死（S4.5，与 T-Text 同一方法论）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Sprite-01 | 绘制锚点 = 局部原点（左上角），四边形 16px |
//! | T-Sprite-02 | `key.slot % 16` 确定性选格：格 0 图案 / 格 1 控件框 / 格 2+ 品红哨兵**可见** |
//! | T-Sprite-03 | NIL 键（未绑定）不渲染，计入 `skipped` |
//! | T-Sprite-04 | `visible = false` 跳过但不销毁，属性保留、恢复可见即回来 |
//! | T-Sprite-05 | DrawKey 三级全序：z -> order -> handle，高层相同时低层决胜 |
//! | T-Sprite-06 | flip 三态（h/v/hv）绕渲染物原点镜像，平移分量不变 |
//! | T-Sprite-07 | 世界变换原样生效：旋转 90° 与缩放 2x 的精确落位 |
//! | T-Sprite-08 | 生命周期：销毁即消失（跨帧，条目注销） |
//!
//! GPU 用例沿用跳过纪律：无库跳过，有库失败即失败。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{Affine2, Camera2DState, Flip, FrameInfo, RenderAssetKey, RenderServer, Vec2};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
const BODY_RGBA: [u8; 4] = [255, 0, 0, 255];
const EYE_RGBA: [u8; 4] = [250, 250, 250, 255];
const FILLER_RGBA: [u8; 4] = [255, 0, 255, 255];
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

/// 装配 256x128 消费器（守卫须绑定到测试作用域，护住整个测试体）。
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

/// 单位相机 + 提交消费一帧。
fn flush(consumer: &mut CommandConsumer, server: &mut WgpuRenderServer) -> FrameOutcome {
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(128.0, 64.0);
    server.set_camera(&camera);
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("消费一帧")
}

/// 建一个精灵并摆放。
fn sprite_at(server: &mut WgpuRenderServer, slot: u32, x: f32, y: f32) -> nes_render_api::ItemHandle {
    let handle = server.create_item(RenderAssetKey::from_parts(slot, 1));
    server.set_transform(handle, Affine2::translation(x, y));
    handle
}

/// 纯色纹理字节。
fn solid(color: [u8; 4]) -> Vec<u8> {
    color.repeat(16 * 16)
}

/// T-Sprite-01：锚点 = 局部原点，四边形 16px，眼睛在局部 (3,3)。
#[test]
fn t_sprite_01_anchor_and_extent() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 16, 10.0, 20.0);
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 1);
    let image = &outcome.image;
    assert_eq!(image.pixel(10, 20), Some(BODY_RGBA), "原点 = 首像素");
    assert_eq!(image.pixel(13, 23), Some(EYE_RGBA), "眼睛在 (13,23)");
    assert_eq!(image.pixel(25, 35), Some(BODY_RGBA), "右下角内");
    assert_eq!(image.pixel(9, 19), Some(CLEAR_RGBA), "原点外");
    assert_eq!(image.pixel(26, 36), Some(CLEAR_RGBA), "16px 之外");
}

/// T-Sprite-02：slot -> 格确定性映射；哨兵格（品红）在画面上**可见**——
/// 采样错格不是静默错误，是可判定的像素事实。
#[test]
fn t_sprite_02_slot_to_cell_mapping() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 16, 0.0, 0.0); // 16 % 16 = 0 -> 图案格
    sprite_at(&mut server, 17, 20.0, 0.0); // 17 % 16 = 1 -> 控件边框格
    sprite_at(&mut server, 2, 40.0, 0.0); // 2 -> 品红哨兵格
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 3);
    let image = &outcome.image;
    assert_eq!(image.pixel(5, 5), Some(BODY_RGBA), "slot 16 -> 格 0（红）");
    assert_eq!(image.pixel(20, 0), Some(CONTROL_RGBA), "slot 17 -> 格 1（边框角）");
    assert_eq!(image.pixel(28, 8), Some(CLEAR_RGBA), "格 1 内部透明");
    assert_eq!(image.pixel(45, 8), Some(FILLER_RGBA), "slot 2 -> 品红哨兵可见");
}

/// T-Sprite-03：NIL 键 = 未绑定，不渲染（契约："未绑定不渲染"不是错误）。
#[test]
fn t_sprite_03_nil_key_skipped() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::NIL);
    server.set_transform(handle, Affine2::translation(10.0, 10.0));
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 0);
    assert_eq!(outcome.stats.skipped, 1, "未绑定计入 skipped");
    assert_eq!(outcome.image.pixel(10, 10), Some(CLEAR_RGBA));
}

/// T-Sprite-04：不可见跳过但属性保留；恢复可见后原地回来（不可见 != 销毁）。
#[test]
fn t_sprite_04_invisible_retains_state() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = sprite_at(&mut server, 16, 10.0, 10.0);
    server.set_visible(handle, false);
    let hidden = flush(&mut consumer, &mut server);
    assert_eq!(hidden.stats.drawn, 0);
    assert_eq!(hidden.stats.skipped, 1);
    assert_eq!(hidden.image.pixel(13, 13), Some(CLEAR_RGBA));

    server.set_visible(handle, true);
    let shown = flush(&mut consumer, &mut server);
    assert_eq!(shown.stats.drawn, 1, "恢复可见即回来");
    assert_eq!(shown.image.pixel(13, 13), Some(EYE_RGBA), "属性（位置）保留");
}

/// T-Sprite-05：DrawKey 三级全序 —— z 不同看 z；z 同看 order；再同看 handle。
/// 三组同位遮挡各验一级，上层者覆盖下层。
#[test]
fn t_sprite_05_drawkey_total_order() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    const CYAN: [u8; 4] = [0, 255, 255, 255];
    const ORANGE: [u8; 4] = [255, 128, 0, 255];
    consumer
        .register_texture(RenderAssetKey::from_parts(160, 1), 16, 16, &solid(CYAN))
        .expect("注册青色");
    consumer
        .register_texture(RenderAssetKey::from_parts(176, 1), 16, 16, &solid(ORANGE))
        .expect("注册橙色");

    let mut server = WgpuRenderServer::new();
    // 第一级（z）：青 z=0、橙 z=1 -> 橙在上。
    let cyan = sprite_at(&mut server, 160, 0.0, 0.0);
    server.set_z(cyan, 0, 0);
    let orange = sprite_at(&mut server, 176, 0.0, 0.0);
    server.set_z(orange, 1, 0);
    // 第二级（order）：z 同为 0，橙 order=1、青 order=5 -> 青在上。
    let orange2 = sprite_at(&mut server, 176, 0.0, 32.0);
    server.set_z(orange2, 0, 1);
    let cyan2 = sprite_at(&mut server, 160, 0.0, 32.0);
    server.set_z(cyan2, 0, 5);
    // 第三级（handle）：z/order 全同，先建橙（句柄小）、后建青（句柄大）-> 青在上。
    let orange3 = sprite_at(&mut server, 176, 0.0, 64.0);
    server.set_z(orange3, 0, 0);
    let cyan3 = sprite_at(&mut server, 160, 0.0, 64.0);
    server.set_z(cyan3, 0, 0);
    let _ = (orange3, cyan3);

    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 6);
    let image = &outcome.image;
    assert_eq!(image.pixel(5, 5), Some(ORANGE), "z 高者在上");
    assert_eq!(image.pixel(5, 37), Some(CYAN), "z 同看 order：大者在后画");
    assert_eq!(image.pixel(5, 69), Some(CYAN), "z/order 同看 handle：大者在上");
}

/// T-Sprite-06：flip 三态绕渲染物原点镜像（契约 I8 + 左上锚点约定），
/// 平移分量逐位不变。
#[test]
fn t_sprite_06_flip_three_modes() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = sprite_at(&mut server, 16, 30.0, 30.0);
    server.set_flip(h, Flip::new(true, false));
    let v = sprite_at(&mut server, 16, 30.0, 30.0);
    server.set_flip(v, Flip::new(false, true));
    let hv = sprite_at(&mut server, 16, 30.0, 30.0);
    server.set_flip(hv, Flip::new(true, true));
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 3, "三个翻转精灵");
    let image = &outcome.image;
    // h：镜像到锚点左侧 x∈[14,30)，眼睛纹素 [3,4) 镜像到 [26,27)。
    assert_eq!(image.pixel(26, 33), Some(EYE_RGBA), "h 翻转眼睛 (26,33)");
    assert_eq!(image.pixel(20, 35), Some(BODY_RGBA), "h 主体");
    assert_eq!(image.pixel(31, 35), Some(CLEAR_RGBA), "h：原区域空出");
    // v：镜像到锚点上方 y∈[14,30)。
    assert_eq!(image.pixel(33, 26), Some(EYE_RGBA), "v 翻转眼睛 (33,26)");
    assert_eq!(image.pixel(35, 20), Some(BODY_RGBA), "v 主体");
    assert_eq!(image.pixel(35, 31), Some(CLEAR_RGBA), "v：原区域空出");
    // hv：双镜像到 [14,30)²。
    assert_eq!(image.pixel(26, 26), Some(EYE_RGBA), "hv 翻转眼睛 (26,26)");
    assert_eq!(image.pixel(20, 20), Some(BODY_RGBA), "hv 主体");
    assert_eq!(image.pixel(31, 31), Some(CLEAR_RGBA), "hv：原区域空出");
}

/// T-Sprite-07：世界变换原样生效 —— 旋转 90° 与放大 2x 的精确落位。
#[test]
fn t_sprite_07_world_transform_rotation_and_scale() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    // 旋转 90°（逆时针）：局部 (x,y)->(-y,x)，四边形落到 x∈[4,20)、y∈[20,36)。
    let rot = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(
        rot,
        Affine2::translation(20.0, 20.0).mul(&Affine2::rotation(std::f32::consts::FRAC_PI_2)),
    );
    // 放大 2x：四边形 [40,72)²，眼睛纹素 3 落到像素 6..8。
    let scaled = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(
        scaled,
        Affine2::translation(40.0, 0.0).mul(&Affine2::scale(2.0, 2.0)),
    );
    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    // 眼睛纹素区间 [3,4) 经 90° 旋转映射到世界 x∈(16,17]，覆盖像素中心 16.5 -> (16,23)。
    assert_eq!(image.pixel(16, 23), Some(EYE_RGBA), "旋转后眼睛 (16,23)");
    assert_eq!(image.pixel(12, 28), Some(BODY_RGBA), "旋转后主体");
    assert_eq!(image.pixel(3, 19), Some(CLEAR_RGBA), "旋转段外");
    assert_eq!(image.pixel(21, 21), Some(CLEAR_RGBA), "旋转段外（右上）");
    assert_eq!(image.pixel(46, 6), Some(EYE_RGBA), "2x 后眼睛 (46,6)");
    assert_eq!(image.pixel(40, 0), Some(BODY_RGBA), "2x 原点");
    assert_eq!(image.pixel(71, 31), Some(BODY_RGBA), "2x 右下内");
    assert_eq!(image.pixel(72, 32), Some(CLEAR_RGBA), "2x 段外");
}

/// T-Sprite-08：生命周期 —— 销毁即消失（一次性事件跨帧生效，条目注销）。
#[test]
fn t_sprite_08_destroy_removes() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = sprite_at(&mut server, 16, 10.0, 10.0);
    let alive = flush(&mut consumer, &mut server);
    assert_eq!(alive.stats.drawn, 1);
    assert_eq!(alive.image.pixel(13, 13), Some(EYE_RGBA));

    server.destroy_item(handle);
    let dead = flush(&mut consumer, &mut server);
    assert_eq!(dead.stats.drawn, 0, "销毁后不再绘制");
    assert_eq!(dead.stats.destroys, 1);
    assert_eq!(dead.image.pixel(13, 13), Some(CLEAR_RGBA));
}
