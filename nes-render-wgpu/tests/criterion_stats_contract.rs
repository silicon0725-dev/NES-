//! T-Stats 契约回归：帧统计是条目行为的**完备对账**（S4.7）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Stats-01 | 空帧（仅 Submit）：全部计数为零 |
//! | T-Stats-02 | 忽略计数：空句柄 / 未知句柄的命令逐条计入 `ignored` |
//! | T-Stats-03 | 混合帧全量对账：commands/creates/destroys/updates/drawn/controls/ |
//! |            | glyphs/from_registry/skipped 每个字段与手工账面严格一致 |
//! | T-Stats-04 | 统计逐帧重置：上一帧的数字不泄漏到下一帧 |
//! | T-Stats-05 | `frame_index` 透传自 Submit 携带的 FrameInfo |
//! | T-Stats-06 | `drawn == 0` 当且仅当画面 == 清屏色（计数与像素互证） |

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::command::RenderCommand;
use nes_render_api::{
    Affine2, Camera2DState, ControlState, FrameInfo, ItemHandle, LabelState, RenderAssetKey,
    RenderServer, Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

const CYAN: [u8; 4] = [0, 255, 255, 255];

fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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

fn flush(consumer: &mut CommandConsumer, server: &mut WgpuRenderServer) -> FrameOutcome {
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(128.0, 64.0);
    server.set_camera(&camera);
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("消费一帧")
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 0.0, 0.0, Vec2::new(256.0, 128.0))
}

/// T-Stats-01：空帧（仅 Submit）全部为零。
#[test]
fn t_stats_01_empty_frame_zero_counts() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let commands = vec![RenderCommand::Submit { frame: frame(0) }];
    let outcome = consumer.consume(&commands).expect("消费空帧");
    let s = &outcome.stats;
    assert_eq!((s.commands, s.creates, s.destroys, s.updates, s.ignored), (1, 0, 0, 0, 0));
    assert_eq!((s.drawn, s.controls, s.glyphs, s.from_registry, s.skipped), (0, 0, 0, 0, 0));
    assert!(!s.camera_applied);
    assert_eq!(s.driver_errors, 0);
}

/// T-Stats-02：空句柄与未知句柄逐条计入 `ignored`（契约 I1 的可观测面）。
#[test]
fn t_stats_02_ignored_accounting() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let unknown = ItemHandle::from_raw(999);
    let commands = vec![
        RenderCommand::SetVisible {
            handle: ItemHandle::NIL,
            visible: true,
        },
        RenderCommand::SetTransform {
            handle: unknown,
            transform: Affine2::IDENTITY,
        },
        RenderCommand::DestroyItem { handle: unknown },
        RenderCommand::Submit { frame: frame(0) },
    ];
    let outcome = consumer.consume(&commands).expect("消费");
    assert_eq!(outcome.stats.commands, 4);
    assert_eq!(outcome.stats.ignored, 3, "NIL 1 + 未知 2");
    assert_eq!(outcome.stats.drawn, 0);
}

/// T-Stats-03（旗舰对账）：混合帧的每个统计字段与手工账面严格一致。
///
/// 账面：
/// - 命令 = 7 创建（含即建即毁）+ 1 销毁 + 1 相机 + 属性 4x6=24 + SetRect + SetText + Submit = 36；
/// - updates = 24 + 1 + 1 = 26；
/// - drawn = 精灵 2 + 控件 1 + 字形 2 = 5；
/// - skipped = 不可见 1 + NIL 键无状态 1 = 2。
#[test]
fn t_stats_03_mixed_frame_full_reconciliation() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    // 字形表：2x2 单色字格即可（本测试只看计数，不看字形像素）。
    let sheet = [200u8, 180, 160, 255].repeat(4 * 4);
    consumer
        .set_default_font(
            nes_render_wgpu::FontParams {
                width: 4,
                height: 4,
                cell_w: 2,
                cell_h: 2,
                cols: 2,
                first_char: b'A' as u32,
                count: 2,
                advance: 2.0,
                line_height: 2.0,
            },
            &sheet,
        )
        .expect("设置最小字形表");
    consumer
        .register_texture(RenderAssetKey::from_parts(160, 1), 16, 16, &CYAN.repeat(16 * 16))
        .expect("注册青色纹理");

    let mut server = WgpuRenderServer::new();
    // 1) 内建精灵（画）。
    let builtin = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(builtin, Affine2::translation(0.0, 0.0));
    // 2) 注册表精灵（画）。
    let registered = server.create_item(RenderAssetKey::from_parts(160, 1));
    server.set_transform(registered, Affine2::translation(32.0, 0.0));
    // 3) 控件（画）。
    let control = server.create_item(RenderAssetKey::NIL);
    server.set_rect(control, &ControlState::new([0.0; 4], [64.0, 32.0, 96.0, 64.0]));
    // 4) 文本 "AB"（画 2 字形）。
    let label = server.create_item(RenderAssetKey::NIL);
    server.set_transform(label, Affine2::translation(0.0, 96.0));
    server.set_text(label, &LabelState::new("AB", 16.0));
    // 5) 不可见精灵（跳过）。
    let invisible = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(invisible, Affine2::translation(0.0, 0.0));
    server.set_visible(invisible, false);
    // 6) NIL 键且无 SetRect/SetText（跳过：未绑定且无状态）。
    let unbound = server.create_item(RenderAssetKey::NIL);
    server.set_transform(unbound, Affine2::translation(0.0, 0.0));
    // 7) 即建即毁（不进绘制也不进 skipped）。
    let temp = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.destroy_item(temp);

    let outcome = flush(&mut consumer, &mut server);
    let s = &outcome.stats;
    assert_eq!(s.commands, 36, "7 创建 + 1 销毁 + 1 相机 + 24 属性 + SetRect + SetText + Submit");
    assert_eq!(s.creates, 7);
    assert_eq!(s.destroys, 1);
    assert_eq!(s.updates, 26, "属性 24 + SetRect 1 + SetText 1");
    assert_eq!(s.ignored, 0);
    assert_eq!(s.drawn, 5, "精灵 2 + 控件 1 + 字形 2");
    assert_eq!(s.controls, 1);
    assert_eq!(s.glyphs, 2);
    assert_eq!(s.from_registry, 1);
    assert_eq!(s.skipped, 2, "不可见 + 未绑定");
    assert!(s.camera_applied);
    assert_eq!(s.driver_errors, 0);
}

/// T-Stats-04：统计逐帧重置 —— 上一帧的数字不泄漏。
#[test]
fn t_stats_04_per_frame_reset() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(handle, Affine2::translation(0.0, 0.0));

    let rich = flush(&mut consumer, &mut server);
    assert_eq!(rich.stats.drawn, 1);
    assert_eq!(rich.stats.commands, 1 + 1 + 4 + 1); // 创建 + 相机 + 属性 4 + Submit

    // 第二帧：属性流照常（服务端全量快照），但统计从零起算。
    let again = flush(&mut consumer, &mut server);
    let s = &again.stats;
    assert_eq!(s.commands, 1 + 4 + 1, "无生命周期事件：相机 + 属性 4 + Submit");
    assert_eq!(s.creates, 0, "生命周期事件一次性，不重复计数");
    assert_eq!(s.drawn, 1);
}

/// T-Stats-05：`frame_index` 透传。
#[test]
fn t_stats_05_frame_index_passthrough() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    for index in [0u64, 7, 12345] {
        let commands = vec![RenderCommand::Submit { frame: frame(index) }];
        let outcome = consumer.consume(&commands).expect("消费");
        assert_eq!(outcome.stats.frame_index, index);
    }
}

/// T-Stats-06：`drawn == 0` 当且仅当画面 == 清屏色（计数与像素互证）。
#[test]
fn t_stats_06_drawn_zero_iff_clear_canvas() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let empty = vec![RenderCommand::Submit { frame: frame(0) }];
    let empty_outcome = consumer.consume(&empty).expect("空帧");
    assert_eq!(empty_outcome.stats.drawn, 0);
    assert_eq!(empty_outcome.image.distinct_colors(), 1, "画面 == 清屏色单色");

    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(handle, Affine2::translation(0.0, 0.0));
    let rich = flush(&mut consumer, &mut server);
    assert_eq!(rich.stats.drawn, 1);
    assert_eq!(rich.image.distinct_colors(), 3, "背景 + 红 + 眼白");
}
