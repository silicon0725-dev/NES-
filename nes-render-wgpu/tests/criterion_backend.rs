//! S4.1 出口准则测试：后端 crate 的失败路径与最小可视闭环。
//!
//! 覆盖面（对照封口计划 5.5）：
//!
//! | 用例 | 依赖 GPU | 验证什么 |
//! |---|---|---|
//! | `library_not_found` | 否 | 库定位失败路径：指名道姓报 `LibraryNotFound`，不静默换库 |
//! | `missing_symbol_probe` | 否 | 资产版本探针：加载了"不是 wgpu"的 DLL 时报首个缺失符号 |
//! | `offscreen_target_geometry` | 是 | 离屏目标创建：默认尺寸/行跨度、非法尺寸与未对齐宽度的拒绝 |
//! | `clear_frame_and_row_alignment` | 是 | 空帧清屏 + 读回行对齐（行打包错位会让逐像素比对立刻显形） |
//! | `malformed_stream_rejected` | 是 | 命令流不合法：缺 `Submit` 结尾被拒，且不碰 GPU 状态 |
//! | `sprite_frame_and_png_roundtrip` | 是 | 精灵帧锚点像素 + PNG 落盘回读 |
//!
//! # GPU 用例的跳过纪律（如实报告）
//!
//! 本机找不到 wgpu-native 动态库时，GPU 用例**跳过**并打印说明（`NoLibraryCandidates`）；
//! 但只要库存在，装配或渲染失败就**直接判失败** —— "有库但跑不通"与"没有库"
//! 是两种不同的事实，不许互相伪装。

use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

use nes_render_api::command::RenderCommand;
use nes_render_api::{
    Affine2, Camera2DState, ControlState, FrameInfo, RenderAssetKey, RenderServer, Vec2, Flip,
};
use nes_render_wgpu::renderer::CLEAR_COLOR;
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    WgpuRenderServer,
};

/// 背景色的字节形态（`ClearColor` 是 f64 通道，像素读回后是 u8）。
const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
/// 精灵底色（红）。
const BODY_RGBA: [u8; 4] = [255, 0, 0, 255];
/// 精灵眼睛（近白）。
const EYE_RGBA: [u8; 4] = [250, 250, 250, 255];
/// 图集哨兵色（品红）：正常画面绝不应出现。
const FILLER_RGBA: [u8; 4] = [255, 0, 255, 255];
/// 控件边框色（绿）。
const CONTROL_RGBA: [u8; 4] = [0, 255, 0, 255];

/// 生成一张纯色 RGBA 纹理。
fn solid_rgba(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        out.extend_from_slice(&color);
    }
    out
}

/// GPU 用例串行锁。
///
/// 四个 GPU 用例各自装配独立的 Vulkan 实例/设备，本机实测并发跑会**偶发**把
/// 测试进程直接崩掉（两例：整轮 exit 139 / 127；单跑与串行轮次全部稳定）。
/// 锁只作用于本测试二进制内的 GPU 用例；无 GPU 用例（失败路径、锚点互锁）不受影响。
fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 0.0, 0.0, Vec2::new(64.0, 64.0))
}

/// 打开 GPU 消费器；无库时跳过（返回 `None`），有库但失败时如实失败。
fn open_consumer() -> Option<CommandConsumer> {
    match CommandConsumer::open() {
        Ok(consumer) => Some(consumer),
        Err(BackendError::NoLibraryCandidates(tried)) => {
            eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库，已尝试：{tried}");
            None
        }
        Err(err) => panic!("GPU 装配失败（库存在但上下文不成立，应如实暴露）：{err}"),
    }
}

// ------------------------------------------------------------ 失败路径（无 GPU）

#[test]
fn criterion_backend_library_not_found() {
    let missing = Path::new("Z:/nes-render-wgpu/definitely-missing.dll");
    assert!(
        !missing.is_file(),
        "测试前提：路径确实不存在（选一个不可能存在的盘符/路径）"
    );
    match GpuContext::open_at(missing) {
        Err(BackendError::LibraryNotFound(path)) => {
            assert_eq!(path, missing, "错误里必须指名道姓给出尝试过的路径");
        }
        Err(other) => panic!("期望 LibraryNotFound，实际 {other}"),
        Ok(_) => panic!("期望 LibraryNotFound，实际装配成功"),
    }
}

#[test]
fn criterion_backend_missing_symbol_probe() {
    // 用系统自带的 kernel32.dll 当"不是 wgpu 的库"：它能被加载，但不含任何
    // wgpu 导出 —— 正是"资产版本与本绑定不匹配"的探针场景。
    let sys = Path::new(r"C:\Windows\System32\kernel32.dll");
    assert!(sys.is_file(), "测试前提：Windows 系统 DLL 存在");
    match GpuContext::open_at(sys) {
        // WgpuApi::load 按字段序解析，第一个就是 wgpuCreateInstance。
        Err(BackendError::MissingSymbol(name)) => {
            assert_eq!(name, "wgpuCreateInstance", "应报出首个缺失符号（资产版本探针）");
        }
        Err(other) => panic!("期望 MissingSymbol，实际 {other}"),
        Ok(_) => panic!("期望 MissingSymbol，实际装配成功（kernel32 不含 wgpu 导出）"),
    }
}

// ------------------------------------------------------------ GPU 用例

#[test]
fn criterion_backend_offscreen_target_geometry() {
    let _guard = gpu_lock();
    let Some(consumer) = open_consumer() else {
        return;
    };
    let ctx = consumer.ctx();

    // 默认 64x64：行跨度 256 字节，正好卡在对齐边界上。
    let target = RenderTarget::new(ctx).expect("默认离屏目标创建");
    assert_eq!(target.size(), (64, 64));
    assert_eq!(target.padded_bytes_per_row(), 256);

    // 128 宽：512 字节/行，仍是 256 的倍数。
    let wide = RenderTarget::with_size(ctx, 128, 64).expect("128x64 目标创建");
    assert_eq!(wide.padded_bytes_per_row(), 512);

    // 尺寸为 0：装配参数自相矛盾，与"驱动故障"分开报。
    assert!(matches!(
        RenderTarget::with_size(ctx, 0, 64),
        Err(BackendError::ConfigMismatch(_))
    ));
    assert!(matches!(
        RenderTarget::with_size(ctx, 64, 0),
        Err(BackendError::ConfigMismatch(_))
    ));
    // 60px 宽 = 240 字节/行，不满足 256 对齐：创建前自检拒绝。
    assert!(matches!(
        RenderTarget::with_size(ctx, 60, 1),
        Err(BackendError::ConfigMismatch(_))
    ));
    // 63px 宽 = 252 字节/行，同样不满足。
    assert!(matches!(
        RenderTarget::with_size(ctx, 63, 1),
        Err(BackendError::ConfigMismatch(_))
    ));
}

#[test]
fn criterion_backend_clear_frame_and_row_alignment() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };

    // 空帧：只有 Submit 终止标记（契约 I3 的最小形态）。
    let commands = vec![RenderCommand::Submit { frame: frame(1) }];
    let outcome = consumer.consume(&commands).expect("空帧消费");

    let stats = &outcome.stats;
    assert_eq!(stats.commands, 1);
    assert_eq!(stats.drawn, 0, "空帧不绘制精灵");
    assert!(!stats.camera_applied, "未推相机时应退回单位视图");
    assert_eq!(stats.driver_errors, 0, "驱动侧不应有未捕获错误");

    let image = &outcome.image;
    assert_eq!((image.width, image.height), (64, 64));
    assert_eq!(image.storage_format(), "RGBA8Unorm");
    // 行对齐自检：紧凑行长 = 宽 * 4；声明跨度是对齐后的值且为 256 的倍数。
    for y in 0..image.height {
        assert_eq!(image.row(y).map(<[u8]>::len), Some(256), "第 {y} 行紧凑长度");
    }
    assert_eq!(image.bytes_per_row % 256, 0);
    // 清屏帧：全部像素 == 清屏色，且只有这一种颜色。
    // 若读回的行打包错位，逐像素比对会立刻显形（列剪切、行串色）。
    assert_eq!(image.distinct_colors(), 1);
    for y in 0..image.height {
        for x in 0..image.width {
            assert_eq!(image.pixel(x, y), Some(CLEAR_RGBA), "px({x},{y}) 应为清屏色");
        }
    }
}

#[test]
fn criterion_backend_malformed_stream_rejected() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };

    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::from_parts(16, 1));
    let no_submit = vec![RenderCommand::SetVisible {
        handle,
        visible: true,
    }];
    match consumer.consume(&no_submit) {
        Err(BackendError::MalformedCommandStream(why)) => {
            assert!(!why.is_empty(), "拒绝理由必须指名道姓");
        }
        other => panic!("期望 MalformedCommandStream，实际 {other:?}"),
    }
    // 空缓冲同样不合法（末条必须是 Submit）。
    assert!(matches!(
        consumer.consume(&[]),
        Err(BackendError::MalformedCommandStream(_))
    ));
}

#[test]
fn criterion_backend_sprite_frame_and_png_roundtrip() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };

    // 与原型 s41_probe3.log 同口径：一个精灵，世界平移 (10,10)，单位视图相机。
    let mut server = WgpuRenderServer::new();
    let key = RenderAssetKey::from_parts(16, 1); // slot 16 -> 图集格 0
    let sprite = server.create_item(key);
    server.set_transform(sprite, Affine2::translation(10.0, 10.0));
    server.set_z(sprite, 0, 0);
    server.set_visible(sprite, true);
    let mut camera = Camera2DState::new(Vec2::new(64.0, 64.0));
    camera.transform = Affine2::translation(32.0, 32.0);
    server.set_camera(&camera);

    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("精灵帧消费");
    assert_eq!(outcome.stats.drawn, 1);
    assert!(outcome.stats.camera_applied);
    assert_eq!(outcome.stats.driver_errors, 0);

    // 锚点像素（原型日志的两个断言 + 背景/边界）。
    let image = &outcome.image;
    assert_eq!(image.pixel(9, 9), Some(CLEAR_RGBA), "精灵外(左上)应为背景");
    assert_eq!(image.pixel(10, 10), Some(BODY_RGBA), "精灵左上应为红");
    assert_eq!(image.pixel(13, 13), Some(EYE_RGBA), "精灵眼睛应为近白");
    assert_eq!(image.pixel(25, 25), Some(BODY_RGBA), "精灵右下应为红");
    assert_eq!(image.pixel(26, 26), Some(CLEAR_RGBA), "精灵外(右下)应为背景");
    assert_eq!(image.distinct_colors(), 3, "背景 + 红 + 近白，仅此三种");
    for y in 0..image.height {
        for x in 0..image.width {
            assert_ne!(
                image.pixel(x, y),
                Some(FILLER_RGBA),
                "px({x},{y}) 出现哨兵品红：UV 采到格 0 之外"
            );
        }
    }

    // PNG 落盘回读：合法签名 + 非空（外部解码器交叉验证见示例文档）。
    let path = std::env::temp_dir().join("nes_render_wgpu_criterion_backend.png");
    let written = outcome.write_png(&path).expect("PNG 落盘");
    assert!(written > 0);
    let bytes = std::fs::read(&path).expect("PNG 回读");
    assert_eq!(
        &bytes[..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        "必须是合法 PNG 签名"
    );
    let _ = std::fs::remove_file(&path);

    // 稳态确定性：同一状态再消费一帧，锚点不变（属性流是全量幂等快照）。
    let mut again = Vec::new();
    server.submit_into(&frame(1), &mut again);
    let outcome2: FrameOutcome = consumer.consume(&again).expect("第二帧消费");
    assert_eq!(outcome2.stats.drawn, 1);
    assert_eq!(outcome2.image.pixel(13, 13), Some(EYE_RGBA));
    assert_eq!(outcome2.image.distinct_colors(), 3);
}

// ------------------------------------------------------------ S4.2：批量、变换与控件

/// 批量绘制 + 实例缓冲扩容：100 个精灵（初始容量 64），覆盖满画布的 4x4 块
/// 加重叠的 84 个，触发 `ensure_capacity` 的倍增重建路径。
#[test]
fn criterion_backend_capacity_growth_and_batch() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };

    let mut server = WgpuRenderServer::new();
    for i in 0..100u32 {
        // slot = 16*(i+1) → 采样格恒为 0（真实图案格）。
        let key = RenderAssetKey::from_parts(16 * (i + 1), 1);
        let handle = server.create_item(key);
        server.set_transform(
            handle,
            Affine2::translation((i % 4) as f32 * 16.0, ((i / 4) % 4) as f32 * 16.0),
        );
        server.set_z(handle, 0, i as u64);
    }

    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("批量帧消费");

    assert_eq!(outcome.stats.drawn, 100);
    assert_eq!(outcome.stats.driver_errors, 0);
    assert!(
        consumer.pipeline().capacity() > 64,
        "100 个精灵应触发实例缓冲扩容（容量实际 {}）",
        consumer.pipeline().capacity()
    );
    // 4x4 块恰好铺满 64x64：画面只剩红与眼白两种颜色，无背景、无品红。
    let image = &outcome.image;
    assert_eq!(image.distinct_colors(), 2);
    assert_eq!(image.pixel(3, 3), Some(EYE_RGBA), "首块眼睛");
    assert_eq!(image.pixel(19, 19), Some(EYE_RGBA), "次块眼睛（块起点 16,16）");
    assert_eq!(image.pixel(0, 0), Some(BODY_RGBA));
    for y in 0..image.height {
        for x in 0..image.width {
            assert_ne!(image.pixel(x, y), Some(FILLER_RGBA), "px({x},{y}) 品红哨兵");
        }
    }
}

/// flip 的像素级实证（契约 I8）：`world = transform ∘ flip`，翻转**绕渲染物
/// 原点**（本后端的绘制锚点 = 左上角）发生 —— 水平翻转把四边形镜像到锚点
/// 左侧（16px 精灵平移 (10,10) 后占据 x ∈ [-6,10]），平移分量逐位不变。
/// M5 Scratch 兼容层若需"绕中心翻转"，由兼容层在翻转时补偿半个 extents。
#[test]
fn criterion_backend_flip_mirrors_eye() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };

    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(handle, Affine2::translation(10.0, 10.0));
    server.set_flip(handle, Flip::new(true, false));

    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("翻转帧消费");
    assert_eq!(outcome.stats.drawn, 1);

    let image = &outcome.image;
    // y 不受水平翻转影响：精灵仍占 y ∈ [10,26)。
    assert_eq!(image.pixel(9, 9), Some(CLEAR_RGBA), "精灵上方仍是背景");
    // 锚点 (10,10) 成为镜像后的右边缘：像素 10 在精灵外。
    assert_eq!(image.pixel(10, 10), Some(CLEAR_RGBA));
    // 镜像主体：可见部分 x ∈ [0,10)。
    assert_eq!(image.pixel(9, 13), Some(BODY_RGBA), "镜像精灵主体");
    assert_eq!(image.pixel(7, 25), Some(BODY_RGBA));
    // 原精灵区域（x > 10）已空出。
    assert_eq!(image.pixel(13, 13), Some(CLEAR_RGBA), "原精灵区域已空出");
    assert_eq!(image.pixel(25, 25), Some(CLEAR_RGBA));
    // 眼睛：纹素 3（连续 [3,4]）镜像到世界 [6,7]，覆盖像素中心 6.5 -> (6,13)。
    assert_eq!(image.pixel(6, 13), Some(EYE_RGBA), "眼睛移到镜像点");
    assert_eq!(image.pixel(7, 13), Some(BODY_RGBA), "镜像点右侧一格是红底");
}

/// 相机 zoom 的像素级实证（契约 I9）：zoom=2、中心 (16,16) 时，
/// 世界 (10,10)-(26,26) 的精灵映射到视口 (20,20)-(52,52)，尺寸翻倍。
#[test]
fn criterion_backend_camera_zoom_scales_sprite() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };

    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(handle, Affine2::translation(10.0, 10.0));

    let mut camera = Camera2DState::new(Vec2::new(64.0, 64.0));
    camera.transform = Affine2::translation(16.0, 16.0);
    camera.zoom = Vec2::new(2.0, 2.0);
    server.set_camera(&camera);

    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("缩放帧消费");
    assert_eq!(outcome.stats.drawn, 1);
    assert!(outcome.stats.camera_applied);

    let image = &outcome.image;
    assert_eq!(image.pixel(19, 19), Some(CLEAR_RGBA), "放大后左上之外是背景");
    assert_eq!(image.pixel(20, 20), Some(BODY_RGBA), "世界 (10,10) -> 视口 (20,20)");
    assert_eq!(image.pixel(26, 26), Some(EYE_RGBA), "眼睛 (13,13) -> 视口 (26,26)");
    assert_eq!(image.pixel(51, 51), Some(BODY_RGBA), "精灵右下边缘内");
    assert_eq!(image.pixel(52, 52), Some(CLEAR_RGBA), "精灵之外是背景");
}

/// 控件 HUD 边框（S4.2）：FULL_RECT 铺满视口、子矩形按视口解析锚点，
/// 边框格内部透明（alpha 丢弃）—— 下层精灵与背景透过控件可见。
#[test]
fn criterion_backend_control_frame_hud() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };

    let mut server = WgpuRenderServer::new();
    // 下层精灵（z=0）：在控件内部应当"透出"。
    let sprite = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(sprite, Affine2::translation(30.0, 30.0));
    server.set_z(sprite, 0, 0);
    // 全幅控件（z=1，NIL 键 —— 控件不需要纹理键）。
    let full = server.create_item(RenderAssetKey::NIL);
    server.set_z(full, 1, 0);
    server.set_rect(full, &ControlState::FULL_RECT);
    // 子矩形控件：锚点全 0，偏移 (8,8,40,40) -> 视口矩形 (8,8,40,40)。
    let sub = server.create_item(RenderAssetKey::NIL);
    server.set_z(sub, 1, 1);
    server.set_rect(sub, &ControlState::new([0.0, 0.0, 0.0, 0.0], [8.0, 8.0, 40.0, 40.0]));

    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("控件帧消费");
    // E-1（S12.1）：每控件 = 四条 1px 边框条实例（填充透明不发）
    // —— 1 精灵 + 2 控件 x 4 条 = 9 实例。
    assert_eq!(outcome.stats.drawn, 9);
    assert_eq!(outcome.stats.controls, 2, "两个控件边框");
    assert_eq!(outcome.stats.driver_errors, 0);

    let image = &outcome.image;
    // FULL_RECT 边框：像素精确 1px 平直条（E-1），内部透明。
    assert_eq!(image.pixel(0, 0), Some(CONTROL_RGBA), "全幅边框左上");
    assert_eq!(image.pixel(3, 3), Some(CLEAR_RGBA), "边框恰 1px（内一格透明）");
    assert_eq!(image.pixel(63, 63), Some(CONTROL_RGBA), "全幅边框右下");
    assert_eq!(image.pixel(5, 5), Some(CLEAR_RGBA), "全幅内部透明 -> 背景");
    // 子矩形 (8,8,40,40)：1px 平直边框。
    assert_eq!(image.pixel(9, 9), Some(CLEAR_RGBA), "子矩形边框恰 1px");
    assert_eq!(image.pixel(12, 12), Some(CLEAR_RGBA), "子矩形内部透明");
    // 下层精灵透过控件内部可见。
    assert_eq!(image.pixel(30, 30), Some(BODY_RGBA), "精灵透过控件可见");
    assert_eq!(image.pixel(33, 33), Some(EYE_RGBA), "精灵眼睛透过控件可见");
}

// ------------------------------------------------------------ S4.3：纹理注册表

/// 注册表采样：16x16 四象限纹理按图层采样，与未注册键（内建图集格）
/// 同帧混画；同键覆写 = 热重载（图层号不变、内容更新）。
#[test]
fn criterion_backend_texture_registry_sampling() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };

    // 四象限纹理：蓝 / 黄 / 青 / 灰，各占 8x8。
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const YELLOW: [u8; 4] = [255, 255, 0, 255];
    const CYAN: [u8; 4] = [0, 255, 255, 255];
    const GRAY: [u8; 4] = [64, 64, 64, 255];
    let mut quad = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            let color = if x < 8 {
                if y < 8 { BLUE } else { CYAN }
            } else if y < 8 {
                YELLOW
            } else {
                GRAY
            };
            quad.extend_from_slice(&color);
        }
    }
    let key = RenderAssetKey::from_parts(32, 1); // 未注册时 slot 32 -> 图集格 0
    let layer = consumer
        .register_texture(key, 16, 16, &quad)
        .expect("注册四象限纹理");
    assert_eq!(layer, 0, "首张纹理分到图层 0");

    let mut server = WgpuRenderServer::new();
    let registered = server.create_item(key);
    server.set_transform(registered, Affine2::translation(0.0, 0.0));
    let builtin = server.create_item(RenderAssetKey::from_parts(16, 1));
    server.set_transform(builtin, Affine2::translation(32.0, 0.0));

    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("注册表帧消费");
    assert_eq!(outcome.stats.drawn, 2);
    assert_eq!(outcome.stats.from_registry, 1, "一个精灵从注册表采样");

    let image = &outcome.image;
    assert_eq!(image.pixel(0, 0), Some(BLUE), "注册表纹理左上象限");
    assert_eq!(image.pixel(15, 0), Some(YELLOW), "右上象限");
    assert_eq!(image.pixel(0, 15), Some(CYAN), "左下象限");
    assert_eq!(image.pixel(15, 15), Some(GRAY), "右下象限");
    // 未注册键仍走内建图集（同帧两路采样混画）。
    assert_eq!(image.pixel(32, 0), Some(BODY_RGBA), "内建格精灵照常");
    assert_eq!(image.pixel(35, 3), Some(EYE_RGBA), "内建格眼睛照常");
    assert_eq!(outcome.stats.driver_errors, 0);

    // 覆写：同键注册纯橙色，图层号不变，下一帧生效。
    const ORANGE: [u8; 4] = [255, 128, 0, 255];
    let again_layer = consumer
        .register_texture(key, 16, 16, &solid_rgba(16, 16, ORANGE))
        .expect("覆写注册");
    assert_eq!(again_layer, layer, "覆写不换图层");
    let mut commands = Vec::new();
    server.submit_into(&frame(1), &mut commands);
    let outcome = consumer.consume(&commands).expect("覆写帧消费");
    assert_eq!(outcome.image.pixel(5, 5), Some(ORANGE), "覆写内容生效");
    assert_eq!(outcome.image.pixel(32, 0), Some(BODY_RGBA), "内建路径不受影响");
}

/// 源行补齐上传：2x1 纹理（红|蓝）的行字节只有 8，须补齐到 256 再上传；
/// 拉伸到 16px 四边形后左半红、右半蓝。
#[test]
fn criterion_backend_texture_registry_padded_upload() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    let two_texels = [RED, BLUE].concat();
    let key = RenderAssetKey::from_parts(48, 1);
    consumer
        .register_texture(key, 2, 1, &two_texels)
        .expect("注册 2x1 纹理（触发源行补齐）");

    let mut server = WgpuRenderServer::new();
    let handle = server.create_item(key);
    server.set_transform(handle, Affine2::translation(0.0, 0.0));
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("补齐上传帧消费");

    let image = &outcome.image;
    assert_eq!(image.pixel(0, 0), Some(RED), "左半是第一纹素");
    assert_eq!(image.pixel(7, 15), Some(RED));
    assert_eq!(image.pixel(8, 0), Some(BLUE), "右半是第二纹素");
    assert_eq!(image.pixel(15, 15), Some(BLUE));
    assert_eq!(image.distinct_colors(), 3, "红 + 蓝 + 背景三种");
}

/// 多图层 + 倍增扩容：注册 5 张纯色纹理（初始容量 4），第 5 张触发扩容
/// 重建与既有图层重传 —— 5 个精灵各采样各的颜色，全部正确才算过。
#[test]
fn criterion_backend_texture_registry_growth() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };
    const COLORS: [[u8; 4]; 5] = [
        [255, 128, 0, 255], // 橙
        [128, 0, 255, 255], // 紫
        [0, 255, 128, 255], // 薄荷
        [255, 0, 128, 255], // 玫红
        [128, 128, 255, 255], // 长春花
    ];
    let keys: Vec<RenderAssetKey> = (0..5)
        .map(|i| RenderAssetKey::from_parts(64 + i as u32 * 16, 1))
        .collect();
    for (i, key) in keys.iter().enumerate() {
        let layer = consumer
            .register_texture(*key, 16, 16, &solid_rgba(16, 16, COLORS[i]))
            .expect("注册纯色纹理");
        assert_eq!(layer, i as u32, "第 {i} 张应分到图层 {i}");
    }

    let mut server = WgpuRenderServer::new();
    let positions = [(0.0, 0.0), (16.0, 0.0), (32.0, 0.0), (48.0, 0.0), (0.0, 16.0)];
    for (key, (x, y)) in keys.iter().zip(positions) {
        let handle = server.create_item(*key);
        server.set_transform(handle, Affine2::translation(x, y));
    }
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("扩容帧消费");
    assert_eq!(outcome.stats.drawn, 5);
    assert_eq!(outcome.stats.from_registry, 5);
    assert_eq!(outcome.stats.driver_errors, 0);

    let image = &outcome.image;
    for (i, (x, y)) in positions.iter().enumerate() {
        let (px, py) = (*x as u32 + 4, *y as u32 + 4);
        assert_eq!(
            image.pixel(px, py),
            Some(COLORS[i]),
            "第 {i} 张纹理应采到自己的颜色"
        );
    }
    assert_eq!(consumer.registry().len(), 5);
}

/// 注册表的拒绝路径：尺寸超限 / 字节数不符 / 零尺寸，全部指名道姓报错。
#[test]
fn criterion_backend_texture_registry_rejects() {
    let _guard = gpu_lock();
    let Some(mut consumer) = open_consumer() else {
        return;
    };
    let key = RenderAssetKey::from_parts(80, 1);
    assert!(matches!(
        consumer.register_texture(key, 257, 1, &solid_rgba(257, 1, [255, 0, 0, 255])),
        Err(BackendError::ConfigMismatch(_))
    ), "超过图层上限应拒绝");
    assert!(matches!(
        consumer.register_texture(key, 16, 16, &[0u8; 10]),
        Err(BackendError::PixelBufferSize { .. })
    ), "字节数不符应拒绝");
    assert!(matches!(
        consumer.register_texture(key, 0, 0, &[]),
        Err(BackendError::InvalidImageSize { .. })
    ), "零尺寸应拒绝");
    assert!(consumer.registry().is_empty(), "被拒的注册不留痕");
    assert_eq!(consumer.registry().layer_of(key), None);
}

// ------------------------------------------------------------ S4.4：文本光栅化

/// 程序化字形表：每字符一格纯色（颜色由码点确定），不需要外部资产即可
/// 精确断言字形的落位、多行、空格与表外字符行为。
fn procedural_font_sheet() -> (nes_render_wgpu::FontParams, Vec<u8>) {
    const CELL: u32 = 16;
    const COLS: u32 = 16;
    const FIRST: u32 = 32;
    const COUNT: u32 = 95; // ASCII 32..=126
    let rows = COUNT.div_ceil(COLS);
    let (w, h) = (CELL * COLS, CELL * rows); // 256x96
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for i in 0..COUNT {
        if FIRST + i == b' ' as u32 {
            continue; // 空格无墨（与真实字形表一致）
        }
        let color = char_color(FIRST + i);
        let (cx, cy) = ((i % COLS) * CELL, (i / COLS) * CELL);
        for y in 0..CELL {
            for x in 0..CELL {
                let at = (((cy + y) * w + cx + x) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&color);
            }
        }
    }
    (
        nes_render_wgpu::FontParams {
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

/// 字符码点 -> 测试用纯色（可区分且不与既有锚点色冲突）。
fn char_color(code: u32) -> [u8; 4] {
    [40 + (code * 3 % 200) as u8, 40 + (code * 7 % 200) as u8, 40 + (code * 11 % 200) as u8, 255]
}

/// 文本光栅化（S4.4）：字形落位（字距推进）、多行（行高 + line_spacing）、
/// 空格与表外字符只推进笔位、NIL 键的文本渲染物照常绘制。
#[test]
fn criterion_backend_label_text_raster() {
    let _guard = gpu_lock();
    // 128x128 画布：多行与后续标签的落位超出默认 64x64。
    let mut consumer = match GpuContext::open() {
        Ok(ctx) => {
            let target = RenderTarget::with_size(&ctx, 128, 128).expect("128x128 目标");
            let atlas = SpriteAtlas::new(&ctx).expect("图集");
            CommandConsumer::new(ctx, target, atlas).expect("消费器")
        }
        Err(BackendError::NoLibraryCandidates(tried)) => {
            eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库，已尝试：{tried}");
            return;
        }
        Err(err) => panic!("GPU 装配失败（应如实暴露）：{err}"),
    };
    let (params, sheet) = procedural_font_sheet();
    let tile = consumer.set_default_font(params, &sheet).expect("设置默认字体");
    assert_eq!(tile, 0);

    let color_of = |ch: char| char_color(ch as u32);
    let mut server = WgpuRenderServer::new();

    // "AB"：A 在 (0,0)-(15,15)，B 在 (16,0)-(31,15)。
    let ab = server.create_item(RenderAssetKey::NIL); // 文本不依赖纹理键
    server.set_transform(ab, Affine2::translation(0.0, 0.0));
    server.set_text(ab, &nes_render_api::LabelState::new("AB", 16.0));

    // "A\nB" + 额外行距 4：第二行 y = 16 + 4 = 20。
    let multi = server.create_item(RenderAssetKey::NIL);
    server.set_transform(multi, Affine2::translation(0.0, 40.0));
    let mut multiline = nes_render_api::LabelState::new("A\nB", 16.0);
    multiline.line_spacing = 4.0;
    server.set_text(multi, &multiline);

    // "A B"：空格只推进笔位，B 落在 x = 32。
    let spaced = server.create_item(RenderAssetKey::NIL);
    server.set_transform(spaced, Affine2::translation(0.0, 80.0));
    server.set_text(spaced, &nes_render_api::LabelState::new("A B", 16.0));

    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = consumer.consume(&commands).expect("文本帧消费");
    assert_eq!(outcome.stats.glyphs, 6, "AB 两个 + 多行两个 + A B 两个 = 6 个字形");
    assert_eq!(outcome.stats.drawn, 6);
    assert_eq!(outcome.stats.driver_errors, 0);

    let image = &outcome.image;
    // "AB"：字距推进 16px。
    let (ca, cb) = (color_of('A'), color_of('B'));
    assert_eq!(image.pixel(0, 0), Some(ca), "A 字格左上");
    assert_eq!(image.pixel(15, 15), Some(ca), "A 字格右下");
    assert_eq!(image.pixel(16, 0), Some(cb), "B 字格左上（推进 16px）");
    assert_eq!(image.pixel(31, 15), Some(cb), "B 字格右下");
    assert_eq!(image.pixel(32, 0), Some(CLEAR_RGBA), "文本外是背景");
    // 多行：第二行按 line_height + line_spacing 落位。
    assert_eq!(image.pixel(0, 40), Some(ca), "多行首行 A");
    assert_eq!(image.pixel(0, 55), Some(ca), "首行 A 字格底部");
    assert_eq!(image.pixel(0, 59), Some(CLEAR_RGBA), "行距间隙是背景");
    assert_eq!(image.pixel(0, 60), Some(cb), "第二行 B 落在 40+20");
    assert_eq!(image.pixel(15, 75), Some(cb), "第二行 B 字格底部");
    // 空格：x=16..31 区域背景，B 落在 x=32。
    assert_eq!(image.pixel(16, 80), Some(CLEAR_RGBA), "空格区域透明");
    assert_eq!(image.pixel(31, 95), Some(CLEAR_RGBA));
    assert_eq!(image.pixel(32, 80), Some(cb), "B 越过空格落在 x=32");
    assert_eq!(image.pixel(47, 95), Some(cb));

    // 表外字符（'€' = U+20AC > 126）：只推进笔位不画。
    //（消费器条目表跨帧持有：第一帧的三个标签仍在原位重画，这里放到空闲区 (0,112)。）
    let mut commands = Vec::new();
    server.set_transform(spaced, Affine2::translation(0.0, 112.0));
    server.set_text(spaced, &nes_render_api::LabelState::new("A\u{20AC}B", 16.0));
    server.submit_into(&frame(1), &mut commands);
    let outcome = consumer.consume(&commands).expect("表外字符帧消费");
    let image = &outcome.image;
    assert_eq!(image.pixel(0, 112), Some(ca), "A 照常");
    assert_eq!(image.pixel(16, 112), Some(CLEAR_RGBA), "表外字符不画");
    assert_eq!(image.pixel(32, 112), Some(cb), "B 仍按笔位落在 x=32");
    assert_eq!(outcome.stats.glyphs, 6, "AB 2 + 多行 2 + A€B 的 A/B 2（€ 跳过）");

    // 未设置字体前的另一消费器：文本静默不画（如实记账，不误报）。
    drop(consumer);
    let Some(mut plain) = open_consumer() else {
        return;
    };
    let mut server = WgpuRenderServer::new();
    let label = server.create_item(RenderAssetKey::NIL);
    server.set_transform(label, Affine2::translation(0.0, 0.0));
    server.set_text(label, &nes_render_api::LabelState::new("AB", 16.0));
    let mut commands = Vec::new();
    server.submit_into(&frame(0), &mut commands);
    let outcome = plain.consume(&commands).expect("无字体帧消费");
    assert_eq!(outcome.stats.glyphs, 0, "未设字体时不产生字形");
    assert_eq!(outcome.image.pixel(0, 0), Some(CLEAR_RGBA));
}

/// 编译期锚点：`CLEAR_COLOR` 的 f64 通道必须与 `CLEAR_RGBA` 的字节一致
/// （两者分别在清屏与断言两侧使用，漂移会让所有像素断言静默失效）。
#[test]
fn clear_color_channels_match_rgba_bytes() {
    let expect = |channel: f64, byte: u8| {
        assert!((channel * 255.0 - byte as f64).abs() < 0.5, "通道 {channel} 与 {byte} 不一致");
    };
    expect(CLEAR_COLOR.r, CLEAR_RGBA[0]);
    expect(CLEAR_COLOR.g, CLEAR_RGBA[1]);
    expect(CLEAR_COLOR.b, CLEAR_RGBA[2]);
    expect(CLEAR_COLOR.a, CLEAR_RGBA[3]);
}
