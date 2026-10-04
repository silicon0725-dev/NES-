//! S16.3（精灵锚点 pivot）渲染侧契约：`SetPivot` 的像素事实。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-P-01 | pivot (0,0) 对照：显式零记录与无记录的整帧像素**逐位相同**（既有行为不变的渲染侧根；零平移乘上去的 ±0.0 加法逐位精确） |
//! | T-P-02 | pivot (0.5,0.5) 的精灵放 (32,32)：像素断言精灵占 (24..40)（中心锚定 —— 位置即精灵中心） |
//! | T-P-03 | pivot (0.5,0.5) + 旋转 90°：像素断言绕锚点旋转（四象限色块按 90° 重排，包围盒不动） |
//! | T-P-04 | 图集帧 + pivot 中心：帧切换只换采样色、几何不动（pivot 相对当前帧矩形，换帧不换语义） |
//! | T-P-05 | 设非零回调 (0,0)：补推零向量清除后整帧像素回基线**逐位** |
//! | T-P-06 | scale + pivot 中心：缩放绕中心（四角对称外扩，(16..48)） |
//!
//! 手法与 `criterion_frame_contract` 同源：直接驱动契约层
//!（WgpuRenderServer -> 命令流 -> CommandConsumer），离屏消费 + 读回断言。
//! 几何口径：SPRITE_PX = 16 世界单位，pivot 平移发生在**局部空间**
//!（`world ∘ translation(-pivot x 16)`）—— 缩放 1 时屏幕矩形 =
//! `pos - pivot*16 .. pos + (1-pivot)*16`。GPU 用例沿用跳过纪律：无库跳过，
//! 有库失败即失败。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{
    Affine2, FrameInfo, ItemHandle, RenderAssetKey, RenderCommand, RenderServer, Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];

/// 四象限色块（可见载体）：32x32、每象限 16x16。
/// 行主序帧格：0=左上红、1=右上绿、2=左下蓝、3=右下黄。
const QUAD_TEXEL_COLORS: [([u8; 4], usize, usize); 4] = [
    ([255, 0, 0, 255], 0, 0),
    ([0, 255, 0, 255], 16, 0),
    ([0, 0, 255, 255], 0, 16),
    ([255, 255, 0, 255], 16, 16),
];

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

fn quadrant_texture() -> Vec<u8> {
    let mut rgba = vec![0u8; 32 * 32 * 4];
    for (color, cx, cy) in QUAD_TEXEL_COLORS {
        for y in cy..cy + 16 {
            for x in cx..cx + 16 {
                let i = (y * 32 + x) * 4;
                rgba[i..i + 4].copy_from_slice(&color);
            }
        }
    }
    rgba
}

/// 行主序帧号 -> 归一化象限矩形（2x2 sheet 特例，与提取层同算式）。
fn quadrant_rect(frame: i64) -> [f32; 4] {
    let f = frame.rem_euclid(4);
    let col = f % 2;
    let row = f / 2;
    [col as f32 * 0.5, row as f32 * 0.5, 0.5, 0.5]
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

/// 装配 256x128 消费器 + 已注册的 32x32 四象限纹理（守卫护住整个测试体）。
fn open_canvas() -> (MutexGuard<'static, ()>, Option<CommandConsumer>) {
    let guard = gpu_lock();
    let consumer = match GpuContext::open() {
        Ok(ctx) => {
            let target = RenderTarget::with_size(&ctx, 256, 128).expect("256x128 目标");
            let atlas = SpriteAtlas::new(&ctx).expect("图集");
            match CommandConsumer::new(ctx, target, atlas) {
                Ok(mut consumer) => {
                    consumer
                        .register_texture(RenderAssetKey::from_parts(7, 1), 32, 32, &quadrant_texture())
                        .expect("注册四象限纹理");
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

/// 一个 16px 屏上精灵（缩放 1 = 16px 单元格 1:1），可带旋转。
fn sprite_at(server: &mut WgpuRenderServer, x: f32, y: f32, rotation: f32) -> ItemHandle {
    let h = server.create_item(RenderAssetKey::from_parts(7, 1));
    let local = if rotation == 0.0 {
        Affine2::IDENTITY
    } else {
        Affine2::rotation(rotation)
    };
    server.set_transform(h, Affine2::translation(x, y).mul(&local));
    h
}

/// T-P-01：pivot (0,0) 对照 —— 显式零记录与无记录整帧**逐位相同**；
/// 且两者都与"精灵贴原点"的既有像素一致（24..40 探针）。
#[test]
fn t_p_01_zero_pivot_is_bit_identical_baseline() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = sprite_at(&mut server, 32.0, 32.0, 0.0);

    // 无记录（= 生产路径的缺省：提取层对 (0,0) 不推 SetPivot）。
    let baseline = flush(&mut consumer, &mut server);
    assert_eq!(baseline.stats.drawn, 1);
    // 既有位置事实：缺省锚点 (0,0) = 无平移，四边形从 pos 向 +x/+y 展开
    //（顶角贴原点的既有行为）—— 占 (32..48)。
    assert_eq!(baseline.image.pixel(33, 33), Some(RED), "缺省顶角贴 pos");
    assert_eq!(baseline.image.pixel(47, 47), Some(YELLOW), "缺省展开到 +16");

    // 显式零记录（= 迁移帧"补推清除"后的快照重发路径）。
    server.set_pivot(h, [0.0, 0.0]);
    let zeroed = flush(&mut consumer, &mut server);
    assert_eq!(
        zeroed.image.rgba, baseline.image.rgba,
        "pivot (0,0) 与无记录整帧逐位相同（零平移 = 恒等）"
    );

    server.destroy_item(h);
    let _ = flush(&mut consumer, &mut server); // 销毁帧落地（清簿记）。
}

/// T-P-02：pivot (0.5,0.5) 的精灵放 (32,32) —— 像素断言精灵占 (24..40)
///（中心锚定：位置即精灵中心；对比 T-P-01 的顶角贴 pos）。
#[test]
fn t_p_02_center_pivot_anchors_position() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = sprite_at(&mut server, 32.0, 32.0, 0.0);
    server.set_pivot(h, [0.5, 0.5]);

    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 1);
    let image = &outcome.image;
    // 四边形从 pos-(8,8) 展开到 pos+(8,8) —— 探针离边界 >= 2px。
    assert_eq!(image.pixel(26, 26), Some(RED), "左上象限随锚点左移");
    assert_eq!(image.pixel(37, 37), Some(YELLOW), "右下象限随锚点右移");
    // 包围盒之外是清屏色（原点侧与远端对称退让各 8px）。
    assert_eq!(image.pixel(22, 22), Some(CLEAR_RGBA), "原点侧退让");
    assert_eq!(image.pixel(42, 42), Some(CLEAR_RGBA), "远端同样退让（位置=中心）");

    server.destroy_item(h);
    let _ = flush(&mut consumer, &mut server);
}

/// T-P-03：pivot (0.5,0.5) + 旋转 90° —— 像素断言绕锚点旋转：四象限色块
/// 重排（红从左上转到右上），包围盒不动（绕中心转 90° 的包围盒不变）。
///
/// 几何：world = T(32,32) ∘ R(90°) ∘ T(-8,-8)，局部 (x,y) -> 屏幕
/// (40-y, 24+x)（y 向下坐标系里 +90° 的引擎旋向）。
#[test]
fn t_p_03_center_pivot_rotates_around_anchor() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = sprite_at(&mut server, 32.0, 32.0, std::f32::consts::FRAC_PI_2);
    server.set_pivot(h, [0.5, 0.5]);

    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 1);
    let image = &outcome.image;
    // 旋转后的象限重排（对照 T-P-02 的未旋转布局逐一错位）。
    assert_eq!(image.pixel(26, 26), Some(BLUE), "左上 <- 原 左下（蓝）");
    assert_eq!(image.pixel(37, 26), Some(RED), "右上 <- 原 左上（红）");
    assert_eq!(image.pixel(26, 37), Some(YELLOW), "左下 <- 原 右下（黄）");
    assert_eq!(image.pixel(37, 37), Some(GREEN), "右下 <- 原 右上（绿）");
    // 包围盒不动：绕锚点转，位置语义不被旋转带走。
    assert_eq!(image.pixel(22, 22), Some(CLEAR_RGBA));
    assert_eq!(image.pixel(42, 42), Some(CLEAR_RGBA));

    server.destroy_item(h);
    let _ = flush(&mut consumer, &mut server);
}

/// T-P-04：图集帧 + pivot 中心 —— 帧切换只换采样色、几何不动
///（pivot 归一化相对当前帧矩形；帧采样只影响 uv 不影响几何）。
#[test]
fn t_p_04_frame_switch_keeps_pivot_geometry() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = server.create_item(RenderAssetKey::from_parts(7, 1));
    // 缩放 2：四边形 32px，中心锚定在 (32,32) -> 占 (16..48)。
    server.set_transform(h, Affine2::translation(32.0, 32.0).mul(&Affine2::scale(2.0, 2.0)));
    server.set_pivot(h, [0.5, 0.5]);

    // 帧 0（红象限铺满四边形）。
    server.set_uv(h, quadrant_rect(0));
    let f0 = flush(&mut consumer, &mut server);
    assert_eq!(f0.image.pixel(20, 20), Some(RED), "帧 0 全红");
    assert_eq!(f0.image.pixel(46, 46), Some(RED));
    assert_eq!(f0.image.pixel(14, 14), Some(CLEAR_RGBA), "锚定包围盒外");
    assert_eq!(f0.image.pixel(50, 50), Some(CLEAR_RGBA));

    // 切帧 2（蓝象限）：同一锚点、同一包围盒，只有颜色换。
    server.set_uv(h, quadrant_rect(2));
    let f2 = flush(&mut consumer, &mut server);
    assert_eq!(f2.image.pixel(20, 20), Some(BLUE), "帧 2 全蓝");
    assert_eq!(f2.image.pixel(46, 46), Some(BLUE));
    assert_eq!(f2.image.pixel(14, 14), Some(CLEAR_RGBA), "换帧不动几何");
    assert_eq!(f2.image.pixel(50, 50), Some(CLEAR_RGBA), "换帧不动几何");

    server.destroy_item(h);
    let _ = flush(&mut consumer, &mut server);
}

/// T-P-05：设非零回调 (0,0) —— 补推零向量（提取层 `pivot_active` 迁移
/// 义务的服务端消费侧）后，整帧像素回基线**逐位**。
#[test]
fn t_p_05_clear_push_returns_to_baseline_bit_exact() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };

    // 基线：从未设过 pivot 的同位精灵。
    let mut server = WgpuRenderServer::new();
    let baseline_h = sprite_at(&mut server, 32.0, 32.0, 0.0);
    let baseline = flush(&mut consumer, &mut server);
    server.destroy_item(baseline_h);
    let _ = flush(&mut consumer, &mut server);

    // 主角：设非零锚点 -> 消费（锚定生效）-> 回调 (0,0)（迁移帧补推清除）。
    let mut server = WgpuRenderServer::new();
    let h = sprite_at(&mut server, 32.0, 32.0, 0.0);
    server.set_pivot(h, [0.5, 0.5]);
    let anchored = flush(&mut consumer, &mut server);
    // 哨兵：中心锚定必须先产生可见位移（否则后面的"逐位回基线"断言空洞）。
    assert_ne!(anchored.image.rgba, baseline.image.rgba, "锚定位移可见");

    server.set_pivot(h, [0.0, 0.0]);
    let cleared = flush(&mut consumer, &mut server);
    assert_eq!(
        cleared.image.rgba, baseline.image.rgba,
        "回调 (0,0) 后整帧逐位回基线（零向量清除 = 恒等）"
    );

    server.destroy_item(h);
    let _ = flush(&mut consumer, &mut server);
}

/// T-P-06：scale + pivot 中心 —— 缩放绕中心：四角像素对称外扩到 (16..48)，
/// 与中心 (32,32) 等距（对照：缺省锚点下缩放会从 pos 向 +x/+y 扩到
/// (32..96)，(18,18) 处必为清屏色 —— 本测试的断言在那一侧恰好相反）。
#[test]
fn t_p_06_center_pivot_scales_around_anchor() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = server.create_item(RenderAssetKey::from_parts(7, 1));
    server.set_transform(h, Affine2::translation(32.0, 32.0).mul(&Affine2::scale(2.0, 2.0)));
    server.set_pivot(h, [0.5, 0.5]);

    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 1);
    let image = &outcome.image;
    // 对称外扩的四角象限（局部象限 8px x 缩放 2 = 16px 屏上一格）。
    assert_eq!(image.pixel(18, 18), Some(RED), "左上角随缩放外扩");
    assert_eq!(image.pixel(46, 18), Some(GREEN), "右上角");
    assert_eq!(image.pixel(18, 46), Some(BLUE), "左下角");
    assert_eq!(image.pixel(46, 46), Some(YELLOW), "右下角");
    // 与中心等距的两侧都在包围盒外 —— 对称性 = 绕中心缩放的像素事实。
    assert_eq!(image.pixel(14, 32), Some(CLEAR_RGBA), "左缘对称");
    assert_eq!(image.pixel(50, 32), Some(CLEAR_RGBA), "右缘对称");
    assert_eq!(image.pixel(32, 14), Some(CLEAR_RGBA), "上缘对称");
    assert_eq!(image.pixel(32, 50), Some(CLEAR_RGBA), "下缘对称");

    server.destroy_item(h);
    let _ = flush(&mut consumer, &mut server);
}

/// 簿记卫生：同键覆写、未知句柄静默忽略、销毁随条目清理（wgpu 侧与
/// null 严格同构的像素面抽查；输出序 SetPivot 恒在 SetUv 之后）。
#[test]
fn pivot_bookkeeping_semantics_on_wgpu_server() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = sprite_at(&mut server, 32.0, 32.0, 0.0);

    // 同键覆写：后写者生效；未知句柄静默忽略。
    server.set_pivot(h, [0.5, 0.5]);
    server.set_pivot(h, [0.0, 1.0]);
    server.set_pivot(ItemHandle::from_raw(999), [1.0, 1.0]);
    // 同帧叠加 tint/uv：输出序 SetPivot 恒在 SetUv 之后（契约冻结序）。
    server.set_tint(h, [255, 255, 255, 255]);
    server.set_uv(h, [0.0, 0.0, 1.0, 1.0]);

    let mut commands = Vec::new();
    server.submit_into(&frame(1), &mut commands);
    let uv_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetUv { .. }));
    let pivot_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetPivot { .. }));
    assert!(uv_at.is_some() && pivot_at.is_some(), "两命令都在流里");
    assert!(pivot_at > uv_at, "SetPivot 恒在 SetUv 之后");
    assert!(
        commands
            .iter()
            .all(|c| c.handle() != Some(ItemHandle::from_raw(999))),
        "未知句柄不产生命令"
    );

    let outcome = consumer.consume(&commands).expect("消费首帧");
    // 覆写后写者生效：pivot (0,1) = 底边中点锚定 —— 局部平移 (0,-16)，
    // 四边形占 (32..48) x (16..32)（对比缺省的 (32..48) x (32..48)）。
    assert_eq!(outcome.image.pixel(34, 18), Some(RED), "底边锚定的顶角");
    assert_eq!(outcome.image.pixel(46, 30), Some(YELLOW), "底边锚定的远角");
    assert_eq!(outcome.image.pixel(34, 34), Some(CLEAR_RGBA), "旧锚定位已空");

    // 销毁：pivot 随条目消亡 —— 命令流里不再出现 SetPivot、像素消失。
    server.destroy_item(h);
    let mut commands = Vec::new();
    server.submit_into(&frame(2), &mut commands);
    assert!(
        commands
            .iter()
            .all(|c| !matches!(c, RenderCommand::SetPivot { .. })),
        "销毁后命令流不再出现 SetPivot"
    );
    let outcome2 = consumer.consume(&commands).expect("消费销毁帧");
    assert_eq!(outcome2.image.pixel(34, 18), Some(CLEAR_RGBA), "条目已消失");
}
