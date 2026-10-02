//! S11-2 第三完整项目：**哨塔防线**（Sentinel Line）—— 波次防御。
//!
//! 窗口宿主（与 dungeon_game 同款四件事：装配、装载、逐帧、瞬态容忍）。
//! 场景与行为全部在 `examples/assets/tower_defense.ron`：四个具名
//! Script 节点（game / spawner / tower_ctl / hud_ui）经 F-1 只读共享面
//! 协作（S11.1），状态只住 game 的 locals。
//!
//! 玩法：敌人从顶部按波次下行，到底扣 1 生命；**鼠标左键建塔**（花
//! 10 金，初始 20 金）；塔按 timer 周期攻击射程内敌人（距离平方判定）；
//! 每杀 +5 金；撑住一波推进下一波（配额/速度递增），生命归零 LOSE。
//!
//! 便携分发形态：exe 同目录 `assets/` 优先（双击即玩），否则回落仓库
//! 开发目录。纹理缺失自动生成（纯色 16x16）。
//!
//! **显示 2x**：场景世界是 384x216（引擎基准坐标系），窗口开 768x432、
//! 相机局部 scale 覆写为 2 —— 同一世界区域放大一倍渲染；鼠标坐标经
//! [`ScaledView`] 除以 2 映射回世界系（场景脚本零改动、语义不变）。
//!
//! 确定性验证走 headless CLI（与本宿主同一场景文件）：
//!
//! ```text
//! ./target/release/nes.exe --headless examples/assets/tower_defense.ron \
//!     --frames 900 --trace examples/assets/tower_defense_trace.txt
//! ```

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use nes_render_api::input::{InputSnapshot, MouseButton};
use nes_render_api::{FrameInfo, Vec2};
use nes_render_wgpu::bmp;
use nes_render_wgpu::FontParams;
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::{InputView, ScriptVm, Transform2D};

/// 单色 16x16 纹理。
fn solid_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    [r, g, b, 255].repeat(16 * 16)
}

/// 缩放输入视图（显示 2x）：鼠标坐标除以缩放比映回世界系；键/按钮/
/// 文本原样透传。快照由宿主每帧从 `collect_input()` 灌入。
struct ScaledView(Rc<RefCell<InputSnapshot>>, f32);

impl InputView for ScaledView {
    fn key(&self, name: &str) -> bool {
        self.0.borrow().is_down(name)
    }
    fn mouse(&self) -> (f32, f32) {
        let s = self.0.borrow();
        (s.mouse.x / self.1, s.mouse.y / self.1)
    }
    fn mouse_delta(&self) -> (f32, f32) {
        let s = self.0.borrow();
        (s.mouse_delta.x / self.1, s.mouse_delta.y / self.1)
    }
    fn button(&self, name: &str) -> bool {
        let s = self.0.borrow();
        MouseButton::from_name(name)
            .map(|b| s.buttons_held[b.index()])
            .unwrap_or(false)
    }
    fn text_len(&self) -> usize {
        self.0.borrow().text.len()
    }
}

/// 显示缩放（世界 384x216 -> 窗口 768x432）。
const VIEW_SCALE: f32 = 2.0;

fn main() {
    // 资产根：exe 同目录 assets/ 优先（便携包），否则仓库开发目录。
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let portable = exe_dir
        .as_ref()
        .map(|d| d.join("assets").join("tower_defense.ron").is_file())
        .unwrap_or(false);
    let root = if portable {
        exe_dir.unwrap().join("assets")
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets")
    };
    let tex = root.join("Textures");
    std::fs::create_dir_all(&tex).unwrap();
    // enemy.bmp = 敌人（红）；bullet.bmp 在本场景兼作塔身（青）。
    for (name, rgb) in [
        ("enemy.bmp", (255, 80, 80)),
        ("bullet.bmp", (80, 200, 255)),
    ] {
        if !tex.join(name).exists() {
            let (r, g, b) = rgb;
            write_bmp_rgba(&tex.join(name), 16, 16, &solid_rgba(r, g, b)).expect("写纹理");
        }
    }

    let mut rt = NesRuntime::open_windowed_with_root(
        &root,
        "NES 2.0 - Sentinel Line (S11-2)",
        (384.0 * VIEW_SCALE) as u32,
        (216.0 * VIEW_SCALE) as u32,
    )
    .expect("窗口装配");
    for t in ["enemy", "bullet"] {
        let _ = rt
            .declare_texture(&format!("Textures/{t}.bmp"))
            .expect("声明纹理");
    }
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 2, "纹理绑定：{report:?}");
    assert_eq!(rt.upload_pending_textures().expect("上传"), 2);
    {
        // 默认字体（HUD Label）：便携包带字体则用包内，否则仓库开发目录。
        let font_dir = if root.join("font_atlas.bmp").is_file() {
            root.clone()
        } else {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../nes-render-wgpu/examples/assets")
        };
        let (w, h, sheet) =
            bmp::load_rgba(&std::fs::read(font_dir.join("font_atlas.bmp")).expect("读字形表"))
                .expect("解码字形表");
        let metrics =
            std::fs::read_to_string(font_dir.join("font_metrics.txt")).expect("读字形表参数");
        let field = |k: &str| -> f32 {
            metrics
                .split_whitespace()
                .find_map(|t| t.strip_prefix(&format!("{k}=")))
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("font_metrics 缺 {k}"))
        };
        let cell = metrics
            .split_whitespace()
            .find_map(|t| t.strip_prefix("cell="))
            .and_then(|c| c.split_once('x'))
            .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
            .expect("cell 格式");
        rt.consumer_mut()
            .expect("GPU 消费器")
            .set_default_font(
                FontParams {
                    width: w,
                    height: h,
                    cell_w: cell.0,
                    cell_h: cell.1,
                    cols: field("cols") as u32,
                    first_char: field("first") as u32,
                    count: field("count") as u32,
                    advance: field("advance"),
                    line_height: field("line_height"),
                },
                &sheet,
            )
            .expect("登记默认字体");
    }

    rt.load_scene("tower_defense.ron").expect("加载场景");
    // 显示 2x：相机局部 scale 覆写（宿主层展示选择 —— 场景文件与
    // headless 语义不动）。cam(192,108) 是世界中心 -> 屏幕中心，
    // scale 2 后世界 (0,0) 仍落屏幕 (0,0)，整场 384x216 放大一倍。
    {
        let cam = rt
            .tree_mut()
            .find_by_name("cam")
            .expect("相机节点 cam");
        rt.tree_mut().set_local(
            cam,
            Transform2D {
                pos: nes_scene::Vec2::new(192.0, 108.0),
                rot: 0.0,
                scale: nes_scene::Vec2::new(VIEW_SCALE, VIEW_SCALE),
                skew: 0.0,
            },
        );
    }
    let mut vm = ScriptVm::new();
    {
        let table = rt.resources_mut().clone();
        let issues = vm.attach_all_with_sources(rt.tree_mut(), &table, &mut |rel| {
            std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())
        });
        assert!(issues.is_empty(), "脚本装载：{issues:?}");
    }
    // 缩放输入视图（替 mount_input_view）：鼠标窗口坐标 / 2 -> 世界系。
    let shared_snap = Rc::new(RefCell::new(InputSnapshot::default()));
    vm.set_input_view(Rc::new(ScaledView(shared_snap.clone(), VIEW_SCALE)));

    // NES_GAME_FRAMES=N 自动退出（打包冒烟用）；缺省无限玩到关窗。
    let total: u64 = std::env::var("NES_GAME_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
    let mut transient = 0u64;
    const TRANSIENT_LIMIT: u64 = 120;
    for index in 0..total {
        let snap = rt.collect_input();
        *shared_snap.borrow_mut() = snap.clone();
        let _ = rt.emit_input_signals(&snap);
        // tick 由引擎内建发射（S8.1）—— 宿主不再手发。
        let frame = FrameInfo::new(
            index,
            1.0 / 60.0,
            index as f64 / 60.0,
            Vec2::new(384.0 * VIEW_SCALE, 216.0 * VIEW_SCALE),
        );
        match rt.frame_windowed_with(&frame, &mut vm) {
            Ok(Some(stats)) => {
                if stats.driver_errors > 0 {
                    eprintln!("[帧 {index}] driver_errors={}", stats.driver_errors);
                }
                transient = 0;
            }
            Ok(None) => {
                println!("[帧 {index}] 窗口已关闭，退出");
                break;
            }
            Err(err) => {
                transient += 1;
                eprintln!("[帧 {index}] 失败（{transient}/{TRANSIENT_LIMIT}）：{err}");
                if transient >= TRANSIENT_LIMIT {
                    std::process::exit(1);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    println!("[完成] Sentinel Line 退出");
}
