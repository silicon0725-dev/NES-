//! 视觉基线（S4.7）：把一帧**确定性复合场景**的 PNG 字节锚定成哈希常量。
//!
//! 与逐像素锚点的分工：锚点钉**契约**（语义正确性），基线钉**漂移**
//! （任何未预期的像素变化——驱动更新、着色器改动、打包顺序变化——即使不违反
//! 任何契约也会在此显形）。
//!
//! # 场景（全程序化资产，无外部文件）
//!
//! 清屏 + 内建精灵 + 注册表纹理精灵 + 控件边框 + 程序化字体文本 "NES 2.0"，
//! 单位相机，256x128 画布。
//!
//! # 换基线（bless）
//!
//! 基线与 GPU/驱动相关（光栅取整等）。有意变更或换机后重录：
//!
//! ```text
//! NES_RENDER_BLESS_BASELINE=1 cargo test --test criterion_visual_baseline
//! ```
//!
//! bless 模式打印新哈希并直接通过；把打印值填回 `BASELINE_FNV1A` 即完成重录。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{
    Affine2, Camera2DState, ControlState, FrameInfo, LabelState, RenderAssetKey, RenderServer,
    Vec2,
};
use nes_render_wgpu::{
    BackendError, CommandConsumer, FontParams, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

/// 本机（Intel Iris Xe / Vulkan / wgpu-native v29.0.1.1）录制的基线 FNV-1a 64。
const BASELINE_FNV1A: u64 = 0xf6cf_1788_9417_60b7;

fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 程序化字形表（与契约矩阵同款：每字符一格纯色）。
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

/// 渲染基线复合场景并返回 PNG 字节。
fn render_baseline_png() -> Result<Vec<u8>, BackendError> {
    let ctx = GpuContext::open()?;
    let target = RenderTarget::with_size(&ctx, 256, 128)?;
    let atlas = SpriteAtlas::new(&ctx)?;
    let mut consumer = CommandConsumer::new(ctx, target, atlas)?;

    let (params, sheet) = font_sheet();
    consumer.set_default_font(params, &sheet)?;
    // 注册表纹理：16x16 青色（键 32）。
    consumer.register_texture(
        RenderAssetKey::from_parts(32, 1),
        16,
        16,
        &[0, 255, 255, 255].repeat(16 * 16),
    )?;

    let mut server = WgpuRenderServer::new();
    // 内建精灵（红 + 眼）。
    let sprite = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(sprite, Affine2::translation(16.0, 16.0));
    server.set_z(sprite, 0, 0);
    // 注册表精灵（青）。
    let tex = server.create_item(RenderAssetKey::from_parts(32, 1));
    server.set_transform(tex, Affine2::translation(48.0, 16.0));
    server.set_z(tex, 0, 1);
    // 控件边框。
    let control = server.create_item(RenderAssetKey::NIL);
    server.set_z(control, 1, 0);
    server.set_rect(control, &ControlState::new([0.0; 4], [16.0, 48.0, 112.0, 80.0]));
    // 文本。
    let label = server.create_item(RenderAssetKey::NIL);
    server.set_z(label, 2, 0);
    server.set_transform(label, Affine2::translation(140.0, 16.0));
    server.set_text(label, &LabelState::new("NES 2.0", 16.0));

    let mut camera = Camera2DState::new(Vec2::new(256.0, 128.0));
    camera.transform = Affine2::translation(128.0, 64.0);
    server.set_camera(&camera);

    let frame = FrameInfo::new(0, 0.0, 0.0, Vec2::new(256.0, 128.0));
    let mut commands = Vec::new();
    server.submit_into(&frame, &mut commands);
    let outcome = consumer.consume(&commands)?;
    assert_eq!(outcome.stats.driver_errors, 0);

    // 编码 PNG（不落盘：基线只锚字节，产物证据由各示例负责）。
    nes_render_wgpu::png::encode_rgba8(256, 128, &outcome.image.rgba)
}

#[test]
fn visual_baseline_png_hash() {
    let _guard = gpu_lock();
    let png = match render_baseline_png() {
        Ok(png) => png,
        Err(BackendError::NoLibraryCandidates(tried)) => {
            eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库，已尝试：{tried}");
            return;
        }
        Err(err) => panic!("基线渲染失败（应如实暴露）：{err}"),
    };
    let hash = fnv1a64(&png);
    let blessing = std::env::var_os("NES_RENDER_BLESS_BASELINE").is_some();
    if blessing {
        eprintln!(
            "\n[bless] 新基线 FNV-1a = 0x{hash:016x}\n[bless] 把 BASELINE_FNV1A 更新为 0x{hash:016x} 后移除环境变量重跑。"
        );
        return;
    }
    assert_eq!(
        hash, BASELINE_FNV1A,
        "视觉基线漂移：实际 0x{hash:016x}，基线 0x{BASELINE_FNV1A:016x}。\
         若为有意变更请重录（NES_RENDER_BLESS_BASELINE=1，见模块文档）。"
    );
}
