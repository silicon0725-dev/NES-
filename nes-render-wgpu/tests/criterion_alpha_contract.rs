//! T-A-01（S16.1 alpha 通道）渲染侧契约：`SetTint` 的像素事实。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Alpha-01 | 无 tint 簿记 = 中性恒等：精灵像素与 E-1 之前逐位相同 |
//! | T-Alpha-02 | `SetTint` A 通道乘进精灵：alpha 0.5 -> 读回像素 RGB 不变（255 白）、A = 127 |
//! | T-Alpha-03 | RGB 相乘路径：tint RGB 128 -> 255 x (128/255) = 128（相乘语义 = E-1 同族） |
//! | T-Alpha-04 | 簿记语义：同键覆写、未知句柄静默忽略、销毁随条目清理 |
//!
//! 本文件直接驱动契约层（WgpuRenderServer -> 命令流 -> CommandConsumer），
//! 像素手法与 `criterion_sprite_contract` 同源；提取层（alpha 属性 ->
//! SetTint）的簿记断言在 nes-render-extract 的 `alpha_channel_pushes_set_tint`。
//! GPU 用例沿用跳过纪律：无库跳过，有库失败即失败。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{
    Affine2, Camera2DState, FrameInfo, ItemHandle, RenderAssetKey, RenderCommand, RenderServer,
    Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];

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

/// 装配 256x128 消费器 + 已注册的 16x16 纯白纹理（守卫护住整个测试体）。
fn open_canvas() -> (MutexGuard<'static, ()>, Option<CommandConsumer>) {
    let guard = gpu_lock();
    let consumer = match GpuContext::open() {
        Ok(ctx) => {
            let target = RenderTarget::with_size(&ctx, 256, 128).expect("256x128 目标");
            let atlas = SpriteAtlas::new(&ctx).expect("图集");
            match CommandConsumer::new(ctx, target, atlas) {
                Ok(mut consumer) => {
                    // 注册表键 (7,1)：16x16 纯白纹理 —— tint 相乘的可见载体。
                    let white = [255u8; 4].repeat(16 * 16);
                    consumer
                        .register_texture(RenderAssetKey::from_parts(7, 1), 16, 16, &white)
                        .expect("注册白纹理");
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

/// 单位相机 + 提交消费一帧。
fn flush(consumer: &mut CommandConsumer, server: &mut WgpuRenderServer) -> FrameOutcome {
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(128.0, 64.0);
    server.set_camera(&camera);
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("消费一帧")
}

/// T-Alpha-01/02：无簿记 = 中性恒等（逐位不变）；A 通道乘进精灵读回像素。
#[test]
fn t_alpha_01_neutral_identity_and_alpha_multiply() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();

    // 精灵 A：无 tint 簿记 —— 恒等（基线逐位不变的渲染侧根）。
    let neutral = server.create_item(RenderAssetKey::from_parts(7, 1));
    server.set_transform(neutral, Affine2::translation(0.0, 0.0));

    // 精灵 B：alpha 0.5（提取层对 (0.5 * 255.0) 截断 -> 127）。
    let half = server.create_item(RenderAssetKey::from_parts(7, 1));
    server.set_transform(half, Affine2::translation(20.0, 0.0));
    server.set_tint(half, [255, 255, 255, 127]);

    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 2);
    let image = &outcome.image;
    // 中性：RGB 与 A 全 255（与无 SetTint 命令的旧路径逐位相同）。
    assert_eq!(image.pixel(5, 5), Some([255, 255, 255, 255]), "无簿记 = 恒等");
    // 半透明：采样白 x tint A(127/255) -> RGB 仍 255、A = 127（写入直通）。
    assert_eq!(
        image.pixel(25, 5),
        Some([255, 255, 255, 127]),
        "alpha 0.5 -> A = 127"
    );
    // 背景未被动过。
    assert_eq!(image.pixel(250, 120), Some(CLEAR_RGBA));
}

/// T-Alpha-03：RGB 相乘（255 x 128/255 = 128）—— 与 E-1 相乘色同族。
#[test]
fn t_alpha_03_rgb_multiply() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = server.create_item(RenderAssetKey::from_parts(7, 1));
    server.set_transform(h, Affine2::translation(0.0, 0.0));
    server.set_tint(h, [128, 255, 255, 255]);
    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(image.pixel(5, 5), Some([128, 255, 255, 255]), "R 通道相乘");
}

/// T-Alpha-04：簿记语义 —— 同键覆写生效；未知句柄静默忽略（不出现在
/// 命令流）；销毁随条目清理（命令流里不再出现 SetTint）。
#[test]
fn t_alpha_04_bookkeeping_semantics() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = server.create_item(RenderAssetKey::from_parts(7, 1));
    server.set_transform(h, Affine2::translation(0.0, 0.0));

    // 同键覆写：后写者生效。
    server.set_tint(h, [255, 255, 255, 127]);
    server.set_tint(h, [255, 255, 255, 64]);
    // 未知句柄：静默忽略（不 panic、不进命令流）。
    let ghost = ItemHandle::from_raw(999);
    server.set_tint(ghost, [9, 9, 9, 9]);

    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(image.pixel(5, 5), Some([255, 255, 255, 64]), "覆写后写者生效");

    // 销毁：tint 随条目消亡 —— 同一条命令流里既无 SetTint、也无像素。
    server.destroy_item(h);
    let mut commands = Vec::new();
    server.submit_into(&frame(1), &mut commands);
    assert!(
        commands.iter().all(|c| !matches!(c, RenderCommand::SetTint { .. })),
        "销毁后命令流不再出现 SetTint"
    );
    let outcome2 = consumer.consume(&commands).expect("消费销毁帧");
    assert_eq!(outcome2.image.pixel(5, 5), Some(CLEAR_RGBA), "条目已消失");

    // 未绑定纹理键的图集格路径同样查 tint（slot 3 = 品红哨兵格可见性照旧）。
    let mut server2 = WgpuRenderServer::new();
    let h2 = server2.create_item(RenderAssetKey::from_parts(3, 1));
    server2.set_transform(h2, Affine2::translation(40.0, 0.0));
    server2.set_tint(h2, [255, 255, 255, 127]);
    let outcome3 = flush(&mut consumer, &mut server2);
    assert_eq!(outcome3.image.pixel(45, 5).map(|p| p[3]), Some(127), "图集格路径同查 tint");
}
