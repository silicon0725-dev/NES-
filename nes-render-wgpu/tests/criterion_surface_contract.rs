//! T-Surf 契约回归：窗口表面生命周期（S6.1）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Surf-01 | 客户区精确等于请求尺寸（AdjustWindowRect 契约），表面尺寸同步，格式 RGBA8Unorm |
//! | T-Surf-02 | 命令流经 `consume_to_surface` 呈现：计数与离屏路径同账、`frame_index` 透传、零 driver errors |
//! | T-Surf-03 | 同一表面连续多帧 acquire/present 复用：每帧独立、计数不串帧 |
//! | T-Surf-04 | 析构序：表面先于消费器与窗口释放，进程干净存活（drop 即断言） |
//!
//! 注意：这些用例会**短暂弹出真实窗口**（每条约 1 秒）——窗口是表面契约的
//! 物理组成部分，无法离屏替身。GPU 用例沿用跳过纪律：无库跳过，有库失败即失败。
//! surface 像素不读回（呈现目标是交换链），断言止于 FrameStats 与无错误，
//! 画面正确性由示例 + 程序化截屏像素校验承担（S6 文档 §验证）。

use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::{Affine2, Camera2DState, FrameInfo, RenderAssetKey, RenderServer, Vec2};
use nes_render_wgpu::window::Window;
use nes_render_wgpu::{
    BackendError, CommandConsumer, GpuContext, RenderTarget, SpriteAtlas, SurfaceTarget,
    WgpuRenderServer,
};

/// webgpu.h 的 `WGPUTextureFormat_RGBA8Unorm`（表面唯一装配格式）。
const RGBA8_UNORM: i32 = 22;

/// 宽度须为 64 的倍数（离屏读回通道的 256 字节行对齐；表面路径本身无此约束，
/// 但消费器始终携带一个同尺寸离屏目标用于 `frame()` 断言）。
/// 且须高于 Win32 标题栏最小宽度（~162px 客户区）：过窄时系统为容纳
/// 标题按钮强制加宽，客户区精确性契约只在下限之上成立（示例 512 宽无此问题）。
const W: u32 = 256;
const H: u32 = 128;

fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 0.0, 0.0, Vec2::new(W as f32, H as f32))
}

/// 装配窗口三件套：消费器（含离屏目标）+ 窗口 + 表面。
/// 守卫须绑定到测试作用域，护住整个测试体。
fn open_surface(
    title: &str,
) -> (
    MutexGuard<'static, ()>,
    Option<(CommandConsumer, SurfaceTarget, Window)>,
) {
    let guard = gpu_lock();
    let ctx = match GpuContext::open() {
        Ok(ctx) => ctx,
        Err(BackendError::NoLibraryCandidates(tried)) => {
            eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库，已尝试：{tried}");
            return (guard, None);
        }
        Err(err) => panic!("GPU 装配失败（应如实暴露）：{err}"),
    };
    let window = Window::open(title, W, H).expect("窗口");
    let target = RenderTarget::with_size(&ctx, W, H).expect("离屏目标");
    let atlas = SpriteAtlas::new(&ctx).expect("图集");
    let consumer = CommandConsumer::new(ctx, target, atlas).expect("消费器");
    let surface = SurfaceTarget::new(consumer.ctx(), &window).expect("表面");
    (guard, Some((consumer, surface, window)))
}

/// 单位相机 + 一张已注册纹理上的精灵，提交并呈现到表面。
fn present_sprite(
    consumer: &mut CommandConsumer,
    surface: &SurfaceTarget,
    index: u64,
) -> nes_render_wgpu::FrameStats {
    let mut server = WgpuRenderServer::new();
    let mut camera = Camera2DState::new(Vec2::new(W as f32, H as f32));
    camera.transform = Affine2::translation(W as f32 / 2.0, H as f32 / 2.0);
    server.set_camera(&camera);
    let handle = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(handle, Affine2::translation(8.0, 8.0));
    let mut commands = Vec::new();
    server.submit_into(&frame(index), &mut commands);
    consumer
        .consume_to_surface(&commands, surface)
        .expect("呈现一帧")
}

/// T-Surf-01：客户区精确等于请求尺寸（AdjustWindowRect 按真实系统度量外扩），
/// 表面按客户区配置，格式恒 RGBA8Unorm。
#[test]
fn t_surf_01_client_size_and_surface_config() {
    let (_guard, canvas) = open_surface("t-surf-01");
    let Some((consumer, surface, window)) = canvas else {
        return;
    };
    assert_eq!(window.client_size(), (W, H), "客户区 = 请求尺寸");
    assert_eq!(surface.size(), (W, H), "表面尺寸 = 客户区");
    assert_eq!(surface.format(), RGBA8_UNORM, "表面格式 = RGBA8Unorm");
    drop(consumer);
}

/// T-Surf-02：注册表纹理上的精灵经表面路径呈现，计数与离屏路径同一本账
/// （drawn=1、from_registry=1、ignored=0），`frame_index` 从 Submit 透传。
#[test]
fn t_surf_02_present_matches_offscreen_ledger() {
    let (_guard, canvas) = open_surface("t-surf-02");
    let Some((mut consumer, surface, _window)) = canvas else {
        return;
    };
    consumer
        .register_texture(RenderAssetKey::from_parts(16, 1), 16, 16, &[200, 120, 40, 255].repeat(16 * 16))
        .expect("注册纹理");
    let stats = present_sprite(&mut consumer, &surface, 7);
    assert_eq!(stats.drawn, 1, "一个精灵");
    assert_eq!(stats.from_registry, 1, "纹理来自注册表");
    assert_eq!(stats.ignored, 0);
    assert_eq!(stats.frame_index, 7, "frame_index 透传自 Submit");
    assert_eq!(stats.driver_errors, 0, "驱动零错误");
}

/// T-Surf-03：同一表面连续三帧 acquire/present，帧间状态独立、计数不串帧。
#[test]
fn t_surf_03_multi_frame_reuse() {
    let (_guard, canvas) = open_surface("t-surf-03");
    let Some((mut consumer, surface, _window)) = canvas else {
        return;
    };
    consumer
        .register_texture(RenderAssetKey::from_parts(16, 1), 16, 16, &[120, 40, 200, 255].repeat(16 * 16))
        .expect("注册纹理");
    for index in 0..3u64 {
        let stats = present_sprite(&mut consumer, &surface, index);
        assert_eq!(stats.drawn, 1, "第 {index} 帧仍是一个精灵");
        assert_eq!(stats.frame_index, index, "第 {index} 帧序号不串帧");
        assert_eq!(stats.driver_errors, 0);
    }
}

/// T-Surf-04：显式析构序 —— 表面先释放（unconfigure + release），
/// 再消费器（其 Drop 会拆除管线与离屏目标），最后窗口。走完即干净存活。
#[test]
fn t_surf_04_clean_teardown_order() {
    let (_guard, canvas) = open_surface("t-surf-04");
    let Some((mut consumer, surface, window)) = canvas else {
        return;
    };
    consumer
        .register_texture(RenderAssetKey::from_parts(16, 1), 16, 16, &[40, 200, 120, 255].repeat(16 * 16))
        .expect("注册纹理");
    let stats = present_sprite(&mut consumer, &surface, 0);
    assert_eq!(stats.drawn, 1, "析构前最后一帧正常呈现");
    drop(surface);
    drop(consumer);
    drop(window);
    // 走到这里没有崩溃/挂起：析构序契约由测试进程干净存活到收尾证明。
}
