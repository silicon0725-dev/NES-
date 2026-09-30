//! T-Registry 契约回归：纹理注册表的交叉语义逐项钉死（S4.7）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Reg-01 | 键隔离：多键多纹理互不串扰 |
//! | T-Reg-02 | 同键覆写 = 热重载：瓦片号不变、内容更新、下一帧生效 |
//! | T-Reg-03 | 覆写可换尺寸：UV 裁剪矩形随新尺寸更新 |
//! | T-Reg-04 | 扩容保持：超过初始容量后旧纹理逐张仍正确 |
//! | T-Reg-05 | 双次扩容：17 张（4 -> 16 -> 64）全部正确 |
//! | T-Reg-06 | NIL 键注册被拒（NIL 保留给未绑定语义），不留痕 |
//! | T-Reg-07 | 与字体共存：注册表承载字形表 + 精灵纹理 + 扩容互不干扰 |
//! | T-Reg-08 | 未注册键回退内建图集格（与注册方向相反的回退语义） |

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{
    Affine2, Camera2DState, FrameInfo, LabelState, RenderAssetKey, RenderServer, Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FontParams, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
const BODY_RGBA: [u8; 4] = [255, 0, 0, 255];
const EYE_RGBA: [u8; 4] = [250, 250, 250, 255];
const CYAN: [u8; 4] = [0, 255, 255, 255];
const ORANGE: [u8; 4] = [255, 128, 0, 255];

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

fn flush(consumer: &mut CommandConsumer, server: &mut WgpuRenderServer) -> FrameOutcome {
    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(128.0, 64.0);
    server.set_camera(&camera);
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    consumer.consume(&commands).expect("消费一帧")
}

fn sprite_at(server: &mut WgpuRenderServer, slot: u32, x: f32, y: f32) -> nes_render_api::ItemHandle {
    let handle = server.create_item(RenderAssetKey::from_parts(slot, 1));
    server.set_transform(handle, Affine2::translation(x, y));
    handle
}

fn solid(w: u32, h: u32, color: [u8; 4]) -> Vec<u8> {
    color.repeat((w * h) as usize)
}

/// 程序化字形表（每字符一格纯色）。
fn font_sheet() -> (FontParams, Vec<u8>) {
    const CELL: u32 = 16;
    const COLS: u32 = 16;
    const FIRST: u32 = 32;
    const COUNT: u32 = 95;
    let rows = COUNT.div_ceil(COLS);
    let (w, h) = (CELL * COLS, CELL * rows);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for i in 0..COUNT {
        let code = FIRST + i;
        if code == b' ' as u32 {
            continue;
        }
        let color = [
            40 + (code * 3 % 200) as u8,
            40 + (code * 7 % 200) as u8,
            40 + (code * 11 % 200) as u8,
            255,
        ];
        let (cx, cy) = ((i % COLS) * CELL, (i / COLS) * CELL);
        for y in 0..CELL {
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

fn char_color(code: u32) -> [u8; 4] {
    [
        40 + (code * 3 % 200) as u8,
        40 + (code * 7 % 200) as u8,
        40 + (code * 11 % 200) as u8,
        255,
    ]
}

/// T-Reg-01：键隔离 —— 两键两纹理各画各的，互不串扰。
#[test]
fn t_reg_01_key_isolation() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    consumer
        .register_texture(RenderAssetKey::from_parts(16, 1), 16, 16, &solid(16, 16, CYAN))
        .expect("注册青");
    consumer
        .register_texture(RenderAssetKey::from_parts(32, 1), 16, 16, &solid(16, 16, ORANGE))
        .expect("注册橙");
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 16, 0.0, 0.0);
    sprite_at(&mut server, 32, 32.0, 0.0);
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.from_registry, 2);
    let image = &outcome.image;
    assert_eq!(image.pixel(5, 5), Some(CYAN), "键 16 采到青");
    assert_eq!(image.pixel(37, 5), Some(ORANGE), "键 32 采到橙");
}

/// T-Reg-02：同键覆写 = 热重载（瓦片号不变、内容更新、下一帧生效）。
#[test]
fn t_reg_02_overwrite_hot_reload() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let key = RenderAssetKey::from_parts(16, 1);
    let tile0 = consumer
        .register_texture(key, 16, 16, &solid(16, 16, CYAN))
        .expect("首注册");
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 16, 0.0, 0.0);
    let first = flush(&mut consumer, &mut server);
    assert_eq!(first.image.pixel(5, 5), Some(CYAN));

    let tile1 = consumer
        .register_texture(key, 16, 16, &solid(16, 16, ORANGE))
        .expect("覆写");
    assert_eq!(tile0, tile1, "覆写不换瓦片");
    let second = flush(&mut consumer, &mut server);
    assert_eq!(second.image.pixel(5, 5), Some(ORANGE), "下一帧生效");
}

/// T-Reg-03：覆写可换尺寸 —— 16x16 纯青改为 32x8 左红右蓝，UV 裁剪随新尺寸更新。
#[test]
fn t_reg_03_overwrite_resizes() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let key = RenderAssetKey::from_parts(16, 1);
    consumer
        .register_texture(key, 16, 16, &solid(16, 16, CYAN))
        .expect("首注册 16x16");

    // 32x8：左半 16 列红、右半 16 列蓝（行 128 字节 -> 补齐到 256 上传）。
    let mut wide = Vec::new();
    for _ in 0..8 {
        wide.extend_from_slice(&[[255u8, 0, 0, 255]; 16].concat());
        wide.extend_from_slice(&[[0u8, 0, 255, 255]; 16].concat());
    }
    consumer.register_texture(key, 32, 8, &wide).expect("覆写 32x8");

    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 16, 0.0, 0.0);
    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    // 32 纹素宽映射到 16px 四边形：像素 0..7 采左半（红），8..15 采右半（蓝）。
    assert_eq!(image.pixel(3, 3), Some([255, 0, 0, 255]), "左半红");
    assert_eq!(image.pixel(12, 3), Some([0, 0, 255, 255]), "右半蓝");
    assert_eq!(image.pixel(16, 0), Some(CLEAR_RGBA), "四边形外");
}

/// T-Reg-04：扩容保持 —— 第 5 张触发 4->16 扩容，全部 5 张仍正确。
#[test]
fn t_reg_04_growth_preserves() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    const COLORS: [[u8; 4]; 5] = [
        [255, 128, 0, 255],
        [128, 0, 255, 255],
        [0, 255, 128, 255],
        [255, 0, 128, 255],
        [128, 128, 255, 255],
    ];
    for (i, color) in COLORS.iter().enumerate() {
        let key = RenderAssetKey::from_parts(16 * (i as u32 + 1), 1);
        consumer
            .register_texture(key, 16, 16, &solid(16, 16, *color))
            .expect("注册");
    }
    assert_eq!(consumer.registry().len(), 5, "已超过初始容量 4");

    let mut server = WgpuRenderServer::new();
    for i in 0..5u32 {
        sprite_at(&mut server, 16 * (i + 1), (i * 16) as f32, 0.0);
    }
    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    for (i, color) in COLORS.iter().enumerate() {
        assert_eq!(
            image.pixel(i as u32 * 16 + 4, 4),
            Some(*color),
            "第 {i} 张扩容后仍正确"
        );
    }
}

/// T-Reg-05：双次扩容 —— 17 张（4 -> 16 -> 64），逐张核对。
#[test]
fn t_reg_05_double_growth() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let colors: Vec<[u8; 4]> = (0..17)
        .map(|i| [40 + i as u8 * 3, 60 + i as u8 * 7, 90 + i as u8 * 5, 255])
        .collect();
    for (i, color) in colors.iter().enumerate() {
        let key = RenderAssetKey::from_parts(16 * (i as u32 + 1), 1);
        consumer
            .register_texture(key, 16, 16, &solid(16, 16, *color))
            .expect("注册");
    }
    assert_eq!(consumer.registry().len(), 17, "两次扩容（4->16->64）");

    let mut server = WgpuRenderServer::new();
    for i in 0..17u32 {
        let (x, y) = ((i % 13) * 16 + (i / 13) * 8, (i / 13) * 24);
        sprite_at(&mut server, 16 * (i + 1), x as f32, y as f32);
    }
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.drawn, 17);
    let image = &outcome.image;
    for i in 0..17u32 {
        let (x, y) = ((i % 13) * 16 + (i / 13) * 8, (i / 13) * 24);
        assert_eq!(
            image.pixel(x + 4, y + 4),
            Some(colors[i as usize]),
            "第 {i} 张两次扩容后仍正确"
        );
    }
}

/// T-Reg-06：NIL 键注册被拒，且不留痕。
#[test]
fn t_reg_06_nil_key_rejected() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    assert!(matches!(
        consumer.register_texture(RenderAssetKey::NIL, 16, 16, &solid(16, 16, CYAN)),
        Err(BackendError::ConfigMismatch(_))
    ), "NIL 键拒绝");
    assert!(consumer.registry().is_empty(), "被拒注册不留痕");
    assert_eq!(consumer.registry().layer_of(RenderAssetKey::NIL), None);
}

/// T-Reg-07：与字体共存 —— 注册表同时承载字形表 + 精灵纹理并触发扩容，
/// 文本与精灵互不干扰。
#[test]
fn t_reg_07_font_and_textures_coexist() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    let (params, sheet) = font_sheet();
    consumer.set_default_font(params, &sheet).expect("字体（瓦片 0）");
    for i in 0..5u32 {
        let key = RenderAssetKey::from_parts(16 * (i + 10), 1);
        consumer
            .register_texture(key, 16, 16, &solid(16, 16, [i as u8 * 40, 255 - i as u8 * 40, 128, 255]))
            .expect("注册精灵纹理");
    }
    assert_eq!(consumer.registry().len(), 6, "字体 + 5 张 -> 已扩容");

    let mut server = WgpuRenderServer::new();
    let label = server.create_item(RenderAssetKey::NIL);
    server.set_transform(label, Affine2::translation(0.0, 0.0));
    server.set_text(label, &LabelState::new("AB", 16.0));
    sprite_at(&mut server, 160, 48.0, 0.0);
    let outcome = flush(&mut consumer, &mut server);
    let image = &outcome.image;
    assert_eq!(image.pixel(0, 0), Some(char_color(b'A' as u32)), "字形不受精灵纹理干扰");
    assert_eq!(image.pixel(16, 0), Some(char_color(b'B' as u32)));
    assert_eq!(
        image.pixel(52, 4),
        Some([0, 255, 128, 255]),
        "精灵纹理（i=0）不受字体干扰"
    );
}

/// T-Reg-08：未注册键回退内建图集格（与"注册优先"相反的方向）。
#[test]
fn t_reg_08_unregistered_falls_back_to_atlas() {
    let (_guard, consumer) = open_canvas();
    let Some(mut consumer) = consumer else { return };
    // 只注册键 32；键 16 从未注册 -> 走内建格 0（红 + 眼）。
    consumer
        .register_texture(RenderAssetKey::from_parts(32, 1), 16, 16, &solid(16, 16, CYAN))
        .expect("注册键 32");
    let mut server = WgpuRenderServer::new();
    sprite_at(&mut server, 16, 0.0, 0.0);
    sprite_at(&mut server, 32, 32.0, 0.0);
    let outcome = flush(&mut consumer, &mut server);
    assert_eq!(outcome.stats.from_registry, 1, "只有键 32 走注册表");
    let image = &outcome.image;
    assert_eq!(image.pixel(3, 3), Some(EYE_RGBA), "未注册键 -> 内建图案格（眼睛在局部 (3,3)）");
    assert_eq!(image.pixel(10, 10), Some(BODY_RGBA));
    assert_eq!(image.pixel(37, 5), Some(CYAN), "已注册键照常");
}
