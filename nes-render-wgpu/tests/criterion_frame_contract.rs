//! S16.2（图集帧动画）渲染侧契约：`SetUv` 的像素事实。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-F-01 | sheet 2x2（纹理 32x32、帧 16x16）frame=0/1/2/3 -> 四个象限 uv：像素级断言（四帧内容四象限色块，逐帧消费断言对应象限色） |
//! | T-F-02 | frame 越界回绕（5 -> 1、-1 -> 3）；cols=0 整图模式：无记录与恒等矩形 `[0,0,1,1]` 的整帧像素**逐位相同**（既有行为不变的渲染侧根） |
//! | T-F-03 | 簿记语义：同键覆写、未知句柄静默忽略、销毁随条目清理；输出序恒在 `SetTint` 之后（null 与 wgpu 严格同序） |
//!
//! 手法与 `criterion_alpha_contract` 同源：直接驱动契约层
//!（WgpuRenderServer -> 命令流 -> CommandConsumer），离屏消费 + 读回断言。
//! GPU 用例沿用跳过纪律：无库跳过，有库失败即失败。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{
    Affine2, FrameInfo, ItemHandle, RenderAssetKey, RenderCommand, RenderServer, Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];

/// 四象限色块（T-F-01 的可见载体）：32x32、每象限 16x16。
/// 行主序帧格：0=左上红、1=右上绿、2=左下蓝、3=右下黄。
const QUAD_TEXEL_COLORS: [([u8; 4], usize, usize); 4] = [
    ([255, 0, 0, 255], 0, 0),
    ([0, 255, 0, 255], 16, 0),
    ([0, 0, 255, 255], 0, 16),
    ([255, 255, 0, 255], 16, 16),
];

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

/// 行主序帧号 -> 归一化象限矩形（与提取层 `sprite_sheet_uv_rect` 同一算式
/// 的 2x2 特例：col = frame % 2、row = frame / 2）。
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

/// 一个 32x32 屏上精灵（world scale 2 = 16px 单元格 x 2 —— 与 alpha 用例
/// 同款折算：着色器常量 16 世界单位一格）。
fn sheet_sprite(server: &mut WgpuRenderServer) -> ItemHandle {
    let h = server.create_item(RenderAssetKey::from_parts(7, 1));
    server.set_transform(
        h,
        Affine2::translation(0.0, 0.0).mul(&Affine2::scale(2.0, 2.0)),
    );
    h
}

/// T-F-01：2x2 sheet 逐帧消费 —— 每帧整块四边形只显示对应象限色
///（探针全部取象限内部点，离色块边界 >= 2 纹素，线性过滤不越界）。
#[test]
fn t_f_01_quadrant_frames_pixel_exact() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    for f in 0..4i64 {
        let mut server = WgpuRenderServer::new();
        let h = sheet_sprite(&mut server);
        server.set_uv(h, quadrant_rect(f));
        let outcome = flush(&mut consumer, &mut server);
        assert_eq!(outcome.stats.drawn, 1, "frame {f}");
        let image = &outcome.image;
        let want = QUAD_TEXEL_COLORS[f as usize].0;
        // 四个探针 = 四边形四分位内部点：都落在该帧象限色块内部。
        for (px, py) in [(8u32, 8u32), (24, 8), (8, 24), (24, 24)] {
            assert_eq!(
                image.pixel(px, py),
                Some(want),
                "frame {f} 探针 ({px},{py})"
            );
        }
        // 背景未被动过（四边形恰 32x32，之外是清屏色）。
        assert_eq!(image.pixel(40, 8), Some(CLEAR_RGBA));
    }
}

/// T-F-02：越界回绕 + 整图模式逐位不变。
///
/// 纪律：消费器的条目/簿记表**跨帧持久**，而句柄从每个 server 的 1 号
/// 重新分配 —— 每个用例用完即销毁条目（销毁同时清簿记），用例间互不
/// 串味（与 t_alpha_04 的销毁先例同一条卫生纪律）。
#[test]
fn t_f_02_wrap_and_whole_image_identity() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();

    // frame=5（5 % 4 = 1 -> 右上绿）、frame=-1（rem_euclid -> 3 -> 右下黄）。
    for (frame_no, want) in [(5i64, [0u8, 255, 0, 255]), (-1, [255, 255, 0, 255])] {
        let h = sheet_sprite(&mut server);
        server.set_uv(h, quadrant_rect(frame_no));
        let outcome = flush(&mut consumer, &mut server);
        let image = &outcome.image;
        for (px, py) in [(8u32, 8u32), (24, 8), (8, 24), (24, 24)] {
            assert_eq!(
                image.pixel(px, py),
                Some(want),
                "frame {frame_no} 回绕探针 ({px},{py})"
            );
        }
        server.destroy_item(h);
        let _ = flush(&mut consumer, &mut server); // 销毁帧落地（清簿记）。
    }

    // cols=0 整图模式：无记录 = 全瓦片。四象限同屏可见。
    let h = sheet_sprite(&mut server);
    let whole = flush(&mut consumer, &mut server);
    assert_eq!(whole.image.pixel(8, 8), Some([255, 0, 0, 255]));
    assert_eq!(whole.image.pixel(24, 8), Some([0, 255, 0, 255]));
    assert_eq!(whole.image.pixel(8, 24), Some([0, 0, 255, 255]));
    assert_eq!(whole.image.pixel(24, 24), Some([255, 255, 0, 255]));

    // 恒等矩形 `[0,0,1,1]` vs 无记录：整帧像素**逐位相同**（既有行为
    // 不变的渲染侧根 —— 提取层迁移帧补推恒等矩形的折算零漂移）。
    server.set_uv(h, [0.0, 0.0, 1.0, 1.0]);
    let ident_frame = flush(&mut consumer, &mut server);
    assert_eq!(
        ident_frame.image.rgba, whole.image.rgba,
        "恒等矩形与无记录整帧逐位相同"
    );
    server.destroy_item(h);
}

/// T-F-03：簿记语义 —— 同键覆写、未知句柄静默忽略、销毁随条目清理、
/// 输出序恒在 SetTint 之后。
#[test]
fn t_f_03_bookkeeping_semantics() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let h = sheet_sprite(&mut server);

    // 同键覆写：后写者生效（frame 0 红 -> frame 2 蓝）。
    server.set_uv(h, quadrant_rect(0));
    server.set_uv(h, quadrant_rect(2));
    // 未知句柄：静默忽略（不 panic、不进命令流）。
    server.set_uv(ItemHandle::from_raw(999), [0.25, 0.25, 0.5, 0.5]);
    // 同帧叠加 tint：输出序 SetUv 恒在 SetTint 之后（契约冻结序）。
    server.set_tint(h, [255, 255, 255, 255]);

    // 首条命令流 = 生命周期 + 全量属性（含 CreateItem —— 消费器靠它建条目）。
    let mut commands = Vec::new();
    server.submit_into(&frame(1), &mut commands);
    let tint_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetTint { .. }));
    let uv_at = commands
        .iter()
        .position(|c| matches!(c, RenderCommand::SetUv { .. }));
    assert!(tint_at.is_some() && uv_at.is_some(), "两命令都在流里");
    assert!(uv_at > tint_at, "SetUv 恒在 SetTint 之后");
    assert!(
        commands
            .iter()
            .all(|c| c.handle() != Some(ItemHandle::from_raw(999))),
        "未知句柄不产生命令"
    );

    let outcome = consumer.consume(&commands).expect("消费首帧");
    assert_eq!(outcome.image.pixel(8, 8), Some([0, 0, 255, 255]), "覆写后写者生效");

    // 销毁：uv 随条目消亡 —— 命令流里不再出现 SetUv、像素消失。
    server.destroy_item(h);
    let mut commands = Vec::new();
    server.submit_into(&frame(2), &mut commands);
    assert!(
        commands.iter().all(|c| !matches!(c, RenderCommand::SetUv { .. })),
        "销毁后命令流不再出现 SetUv"
    );
    let outcome2 = consumer.consume(&commands).expect("消费销毁帧");
    assert_eq!(outcome2.image.pixel(8, 8), Some(CLEAR_RGBA), "条目已消失");
}
