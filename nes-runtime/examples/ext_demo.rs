//! S17 扩展运行时第 1 期 harness：**ext_demo** —— JS 扩展驱动引擎状态。
//!
//! 画面（384x216）：蓝色方块 `obj1` 由 **hello.js**（QuickJS 扩展）驱动
//! 转圈 —— 扩展只能经 `nes` 能力对象操作（`nes.scene.find` /
//! `nes.node.setPos` / `nes.input.isPressed` / `nes.audio.play`），按住
//! 空格时经混音器播 `Audio/beep`（0.5）。
//!
//! 运行：`cargo run --release --example ext_demo`（窗口；空格出声）。
//! 冒烟：`NES_GAME_FRAMES=300 cargo run --release --example ext_demo`，
//! 或纯 headless：`cargo run --release --example ext_demo -- --headless`
//!（300 帧 + 树断言 + 无 JS 异常，无 GPU 依赖）。
//!
//! 帧序（S17 契约）：`update_extensions` 在 **simulate 之后**调用 ——
//! 扩展看到的是当 tick 后状态；写队列当帧落地、下一帧呈现。

use std::path::Path;
use std::time::Duration;

use nes_render_api::input::{InputEvent, Key};
use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::PROP_TEXTURE;
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::{NodeKind, ScriptVm, Transform2D};

/// 单色 16x16 纹理。
fn solid_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    [r, g, b, 255].repeat(16 * 16)
}

/// 440Hz 短蜂鸣采样（与 editor_shell 同族）。
fn beep_samples(duration_ms: u32, amplitude: i16) -> Vec<i16> {
    let rate = 22_050u32;
    let n = (rate * duration_ms / 1000) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let fade = 1.0 - (i as f64 / n as f64);
            ((t * 440.0).sin() * amplitude as f64 * fade) as i16
        })
        .collect()
}

fn main() {
    let headless = std::env::args().any(|a| a == "--headless");

    // 资产根 = 仓库内的示例目录（扩展文件在 Extensions/hello.js，入库）。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    let tex = root.join("Textures");
    std::fs::create_dir_all(&tex).unwrap();
    if !tex.join("ext_box.bmp").exists() {
        write_bmp_rgba(&tex.join("ext_box.bmp"), 16, 16, &solid_rgba(90, 130, 255))
            .expect("写方块纹理");
    }
    let audio_dir = root.join("Audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let beep = audio_dir.join("beep.wav");
    if !beep.exists() {
        let wav = nes_audio::Wav { sample_rate: 22050, channels: 1, samples: beep_samples(250, 8000) };
        nes_audio::wav::write_wav(&beep, &wav).expect("写蜂鸣 WAV");
    }

    let mut rt = if headless {
        NesRuntime::open_headless(&root).expect("headless 装配")
    } else {
        NesRuntime::open_windowed_with_root(&root, "NES 2.0 - Extension demo (S17)", 384, 216)
            .expect("窗口装配")
    };
    let box_res = rt.declare_texture("Textures/ext_box.bmp").expect("声明方块纹理");
    let _sound = rt.declare_sound("Audio/beep.wav").expect("声明蜂鸣");
    let report = rt.bind_assets();
    assert!(!report.loaded.is_empty(), "资产绑定：{report:?}");
    if !headless {
        let uploaded = rt.upload_pending_textures().expect("上传纹理");
        assert!(uploaded >= 1, "纹理上传：{uploaded}");
    }
    rt.open_audio().expect("开音频（headless 也能开 —— 同一运行时）");

    // 代码搭树：相机 + 转圈的方块（obj1 是扩展的操作对象）。
    {
        let tree = rt.tree_mut();
        let root_node = tree.root();
        let cam = tree.add_node(root_node, "cam", NodeKind::Camera2D);
        tree.set_local(cam, Transform2D::from_pos(192.0, 108.0));
        let obj1 = tree.add_node(root_node, "obj1", NodeKind::Sprite2D);
        tree.set_prop(obj1, PROP_TEXTURE, box_res.to_value()).expect("挂纹理属性");
        tree.set_local(obj1, Transform2D::from_pos(192.0, 108.0));
    }

    // 装载扩展（读 Extensions/hello.js -> QuickJS -> registerExtension）。
    let ext_id = rt
        .load_extension_file(root.join("Extensions/hello.js"))
        .expect("装载 hello.js");
    println!("[扩展] 已装载：{ext_id}（共 {} 个）", rt.extension_count());
    assert_eq!(ext_id, "hello", "hello.js 应自报 id=hello");
    assert_eq!(rt.extension_count(), 1);

    // 冒烟口：NES_GAME_FRAMES 限制帧数；headless 缺省即 300（无窗口可关，
    // 跑满断言）；窗口模式缺省跑到窗口关闭。
    let total: u64 = std::env::var("NES_GAME_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if headless { 300 } else { u64::MAX });
    let mut vm = ScriptVm::new();
    let mut initial: Option<(f32, f32)> = None;
    let mut js_error_frames = 0u64;
    for index in 0..total {
        // 冒烟脚本：帧 100 按下空格、帧 130 抬起（输入能力 + 音频路径）。
        if index == 100 {
            nes_render_wgpu::window::inject_input(InputEvent::Key { key: Key::Space, down: true });
        }
        if index == 130 {
            nes_render_wgpu::window::inject_input(InputEvent::Key { key: Key::Space, down: false });
        }
        let snap = rt.collect_input();
        let _ = rt.emit_input_signals(&snap);
        if headless {
            rt.step_headless(1.0 / 60.0, &mut vm);
        } else {
            let frame = FrameInfo::new(
                index,
                1.0 / 60.0,
                index as f64 / 60.0,
                Vec2::new(384.0, 216.0),
            );
            match rt.frame_windowed_with(&frame, &mut vm) {
                Ok(Some(_stats)) => {}
                Ok(None) => {
                    println!("[帧 {index}] 窗口已关闭，退出");
                    break;
                }
                Err(err) => eprintln!("[帧 {index}] 渲染失败（如实上报）：{err}"),
            }
            std::thread::sleep(Duration::from_millis(16));
        }
        // S17 帧序契约：扩展 update 在 simulate 之后（当 tick 后状态）。
        let errors = rt.update_extensions();
        if !errors.is_empty() {
            js_error_frames += 1;
            for e in &errors {
                eprintln!("[帧 {index}] JS 异常：{e}");
            }
        }
        if initial.is_none() {
            let tree = rt.tree_mut();
            let obj1 = tree.find_by_name("obj1").expect("obj1 在场");
            let t = tree.local(obj1).expect("obj1 变换");
            initial = Some((t.pos.x, t.pos.y));
        }
    }

    // 冒烟断言：方块动过（树状态断言）+ 无 JS 异常。
    let tree = rt.tree_mut();
    let obj1 = tree.find_by_name("obj1").expect("obj1 在场");
    let t = tree.local(obj1).expect("obj1 变换");
    let final_pos = (t.pos.x, t.pos.y);
    let init = initial.expect("至少跑了一帧");
    let dist = ((final_pos.0 - init.0).powi(2) + (final_pos.1 - init.1).powi(2)).sqrt();
    println!(
        "[断言] obj1 初始 {init:?} -> 终态 {final_pos:?}（位移 {dist:.2}px）；JS 异常帧 {js_error_frames}"
    );
    assert!(dist > 5.0, "扩展必须动过方块（位移 {dist}）");
    assert_eq!(js_error_frames, 0, "不允许 JS 异常");
    println!("[OK] ext_demo 冒烟通过");
}
