//! T-Sync 契约回归：表面随窗同步（S12-4 —— "最大化横向拉长"的根修）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Sync-01 | 无窗口运行时：`sync_surface_to_window` 恒 `Ok(false)`（离屏与 headless）；`window_client_size` = 装配的离屏目标尺寸（headless = (0,0)） |
//! | T-Sync-02 | 窗口运行时：`SetWindowPos` 改客户区后 sync 返回 `true` 且 `window_client_size` 实测新客户区；再 sync 幂等 `false`；离屏 `frame_with` 读回尺寸不受表面重配影响（FrameOutcome 仍是装配值） |
//! | T-Sync-03 | 窗口帧路径：resize 后 `frame_windowed_with` 帧首自动同步并照常出帧（帧后再 sync 已无事可做） |
//!
//! 程序化改窗用 `FindWindowW` + `SetWindowPos`（与 nes-render-wgpu 测试的
//! 手写 user32 FFI 同纪律；引擎侧 `NesRuntime` 不暴露 hwnd，按标题寻窗）。
//! GPU 用例沿用跳过纪律：无库跳过，有库失败即失败。

use nes_render_api::{FrameInfo, Vec2};
use nes_runtime::NesRuntime;
use nes_scene::NoObserver;

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

const W: u32 = 256;
const H: u32 = 128;

fn frame(index: u64, vw: f32, vh: f32) -> FrameInfo {
    FrameInfo::new(index, 0.0, 0.0, Vec2::new(vw, vh))
}

fn tmp_root(name: &str) -> PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_sync")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// T-Sync-01：无窗口（离屏带 GPU / headless 无 GPU）两条装配路径。
#[test]
fn t_sync_01_no_window_returns_false() {
    let root = tmp_root("s01");
    // headless：无窗口无 GPU。
    let mut rt = NesRuntime::open_headless(&root).expect("headless 装配");
    assert!(
        !rt.sync_surface_to_window().unwrap(),
        "headless 恒 Ok(false)"
    );
    assert_eq!(rt.window_client_size(), (0, 0), "headless 无离屏目标 = (0,0)");

    // 离屏（带 GPU）：无窗口 → false；client_size = 装配的离屏目标尺寸。
    let Ok(mut rt2) = NesRuntime::open_with_root(&root, W, H) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    assert!(!rt2.sync_surface_to_window().unwrap(), "离屏恒 Ok(false)");
    assert_eq!(rt2.window_client_size(), (W, H), "离屏 = 装配目标尺寸");
}

/// T-Sync-02：窗口运行时的显式同步（幂等 + 离屏读回不受扰）。
#[test]
fn t_sync_02_windowed_sync_tracks_resize() {
    let root = tmp_root("s02");
    let Ok(mut rt) = NesRuntime::open_windowed_with_root(&root, "t-sync-02", W, H) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    assert_eq!(rt.window_client_size(), (W, H), "开窗客户区 = 请求尺寸");
    assert!(!rt.sync_surface_to_window().unwrap(), "尺寸相同无事可做");

    // 拉大整窗（outer 480x320）→ 客户区实测变化 → sync 返回 true。
    resize_window("t-sync-02", 480, 320);
    let (cw, ch) = rt.window_client_size();
    assert!((cw, ch) != (W, H), "客户区已变：{cw}x{ch}");
    assert!(rt.sync_surface_to_window().unwrap(), "首次同步返回 true");
    assert!(!rt.sync_surface_to_window().unwrap(), "再同步幂等 false");
    assert_eq!(rt.window_client_size(), (cw, ch), "client_size 实测新客户区");

    // 离屏读回路径（frame_with -> FrameOutcome）不受表面重配影响：
    // 离屏目标尺寸独立于 surface，仍是装配值。
    let out = rt
        .frame_with(&frame(1, W as f32, H as f32), &mut NoObserver)
        .expect("离屏一帧");
    assert_eq!((out.image.width, out.image.height), (W, H), "读回尺寸不变");
    assert_eq!(out.stats.driver_errors, 0);
}

/// T-Sync-03：帧路径自动同步 —— resize 后第一帧照常出帧，帧首的
/// 自动 sync 已把表面追到新客户区（帧后再 sync 无事可做）。
#[test]
fn t_sync_03_frame_windowed_auto_syncs() {
    let root = tmp_root("s03");
    let Ok(mut rt) = NesRuntime::open_windowed_with_root(&root, "t-sync-03", W, H) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    resize_window("t-sync-03", 480, 320);

    let stats = rt
        .frame_windowed_with(&frame(0, W as f32, H as f32), &mut NoObserver)
        .expect("帧循环不因 resize 失败")
        .expect("窗口未关闭，出帧");
    assert_eq!(stats.driver_errors, 0, "重配不引入驱动错误");
    // 帧首已自动同步：帧后再 sync 无事可做。
    assert!(!rt.sync_surface_to_window().unwrap(), "帧内已同步");
    let (cw, ch) = rt.window_client_size();
    assert!((cw, ch) != (W, H), "表面/客户区已随窗变大：{cw}x{ch}");
}

// ---- 测试侧 user32 FFI（程序化改窗；与被测引擎同纪律的手写绑定）----

const SWP_FLAGS: u32 = 0x0002 | 0x0004; // SWP_NOMOVE | SWP_NOZORDER

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// 按标题寻窗并拉大整窗（`FindWindowW(null, title)` + `SetWindowPos`）。
fn resize_window(title: &str, outer_w: i32, outer_h: i32) {
    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class: *const u16, window: *const u16) -> *mut core::ffi::c_void;
        fn SetWindowPos(
            hwnd: *mut core::ffi::c_void,
            after: *mut core::ffi::c_void,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
    }
    let title_w = wide(title);
    let hwnd = unsafe { FindWindowW(std::ptr::null(), title_w.as_ptr()) };
    assert!(!hwnd.is_null(), "按标题寻窗：{title}");
    assert!(
        unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, outer_w, outer_h, SWP_FLAGS) } != 0,
        "改窗成功"
    );
}
