//! `nes` CLI（S7.3）：headless 确定性运行。
//!
//! ```text
//! nes --headless <场景.ron> [--frames N] [--trace <轨迹>] [--delta F]
//! ```
//!
//! 输出：逐帧指纹 + 轨迹总指纹（差分测试口径 —— 两个实现的输出
//! 逐行 diff，第一处差异即定位到帧）。CLI 只是 [`nes_runtime::headless`]
//! 的薄壳：与测试共用同一条运行路径，不是第二个运行时。

use std::path::Path;

use nes_render_api::input::parse_trace;
use nes_runtime::headless::run;

fn main() {
    let mut scene: Option<String> = None;
    let mut frames: u64 = 300;
    let mut trace_path: Option<String> = None;
    let mut delta: f32 = 1.0 / 60.0;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--headless" => scene = args.next(),
            "--frames" => frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(frames),
            "--trace" => trace_path = args.next(),
            "--delta" => delta = args.next().and_then(|v| v.parse().ok()).unwrap_or(delta),
            other => {
                eprintln!("未知参数 {other}（用法：nes --headless <场景.ron> [--frames N] [--trace <文件>] [--delta F]）");
                std::process::exit(2);
            }
        }
    }
    let Some(scene) = scene else {
        eprintln!("用法：nes --headless <场景.ron> [--frames N] [--trace <文件>] [--delta F]");
        std::process::exit(2);
    };
    let trace = match &trace_path {
        Some(p) => parse_trace(
            &std::fs::read_to_string(p).unwrap_or_else(|e| {
                eprintln!("读轨迹 {p} 失败：{e}");
                std::process::exit(1);
            }),
        )
        .unwrap_or_else(|e| {
            eprintln!("轨迹 {p} 解析失败：{e}");
            std::process::exit(1);
        }),
        None => Vec::new(),
    };

    // 场景路径相对资产根：根 = 场景文件所在目录（单文件即根的惯用法）。
    let scene_path = Path::new(&scene);
    let root = scene_path.parent().unwrap_or(Path::new("."));
    let rel = scene_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let report = run(root, &rel, &trace, frames, delta).unwrap_or_else(|e| {
        eprintln!("headless 运行失败：{e}");
        std::process::exit(1);
    });
    for (i, h) in report.frame_hashes.iter().enumerate() {
        println!("frame {i} hash {h:016x}");
    }
    println!("trace hash {:016x}", report.trace_hash);
}
