//! T-Panel-R 契约回归：**编辑器脚本面板**接入运行时帧循环（S6.34）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Panel-R1 | 窗口运行时：注入字符事件（S7.2 起走中性事件队列）-> `collect_input` 快照 text -> 宿主编辑解释 -> 回车提交 -> `poll_reloads` 热重载 -> 下一帧精灵像素换位；坏脚本 last-good（旧行为保持）；面板 Label 字形有墨 |

use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::{PROP_LABEL_TEXT, PROP_TEXTURE};
use nes_render_api::input::InputEvent;
use nes_render_wgpu::window::inject_input;
use nes_render_wgpu::{bmp, FontParams};
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::{NodeKind, ScriptVm, Transform2D, Value};

const CLEAR_RGBA: [u8; 4] = [13, 13, 25, 255];
const CHECK_A: [u8; 4] = [0, 200, 120, 255]; // 棋盘亮格
const W: u32 = 160;
const H: u32 = 160;
/// 精灵 A 位（初始脚本钉住）与 B 位（注入脚本钉住）。
const A: (f32, f32) = (16.0, 48.0);
const B: (f32, f32) = (40.0, 120.0);

fn checker_rgba() -> Vec<u8> {
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            rgba.extend_from_slice(if (x / 4 + y / 4) % 2 == 0 {
                &CHECK_A
            } else {
                &[20, 30, 50, 255]
            });
        }
    }
    rgba
}

fn frame(index: u64) -> FrameInfo {
    FrameInfo::new(index, 1.0 / 60.0, index as f64 / 60.0, Vec2::new(W as f32, H as f32))
}

/// 登记默认字体（nes-render-wgpu 示例烘焙产物；Label 文本的渲染前提）。
fn register_font(rt: &mut NesRuntime) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../nes-render-wgpu/examples/assets");
    let (w, h, sheet) =
        bmp::load_rgba(&std::fs::read(dir.join("font_atlas.bmp")).expect("读字形表")).unwrap();
    let metrics = std::fs::read_to_string(dir.join("font_metrics.txt")).unwrap();
    let field = |k: &str| -> f32 {
        metrics
            .split_whitespace()
            .find_map(|t| t.strip_prefix(&format!("{k}=")))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("font_metrics 缺 {k}"))
    };
    let (cw, ch) = metrics
        .split_whitespace()
        .find_map(|t| t.strip_prefix("cell="))
        .and_then(|c| c.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap();
    rt.consumer_mut()
        .set_default_font(
            FontParams {
                width: w,
                height: h,
                cell_w: cw,
                cell_h: ch,
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

/// 面板编辑缓冲（与示例 script_panel 同一套解释规则）。
struct Panel {
    input: String,
}

impl Panel {
    fn feed(&mut self, code: u32) -> bool {
        match code {
            0x0D => true,
            0x08 => {
                self.input.pop();
                false
            }
            c if (32..127).contains(&c) => {
                self.input.push(c as u8 as char);
                false
            }
            _ => false,
        }
    }
}

/// T-Panel-R1：编辑器面板全链（窗口模式）。键入经 WM_CHAR 队列
/// （`inject_input` 与真实消息落进同一事件队列，S7.2 口径），
/// 回车提交把 `source` 属性换成缓冲文本，`poll_reloads` 重编译重挂载，
/// 下一次 "step" 精灵钉在新位 —— 窗口表面呈现、离屏读数断言（同一
/// 消费器语义）。坏脚本如实失败且旧行为保持（last-good）。
#[test]
fn t_panel_r1_typed_script_hot_reloads_to_pixels() {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("nes_runtime_panel")
        .join("r1");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("grid.bmp"), 16, 16, &checker_rgba())
        .expect("写棋盘");

    let Ok(mut rt) = NesRuntime::open_windowed_with_root(&root, "T-Panel-R1", W, H) else {
        eprintln!("[跳过 GPU 用例] 本机未找到 wgpu-native 动态库");
        return;
    };
    let res = rt.declare_texture("Textures/grid.bmp").expect("声明纹理");
    assert_eq!(rt.bind_assets().loaded.len(), 1);
    assert_eq!(rt.upload_pending_textures().unwrap(), 1);
    register_font(&mut rt);

    // 场景：相机 + 精灵（初始脚本钉在 A）+ 脚本 + 面板状态 Label。
    let (sprite, brain, label) = {
        let tree = rt.tree_mut();
        let root_node = tree.root();
        let cam = tree.add_node(root_node, "cam", NodeKind::Camera2D);
        tree.set_local(cam, Transform2D::from_pos(80.0, 80.0));
        let sprite = tree.add_node(root_node, "sprite", NodeKind::Sprite2D);
        tree.set_prop(sprite, PROP_TEXTURE, res.to_value()).unwrap();
        tree.set_local(sprite, Transform2D::from_pos(A.0, A.1));
        let brain = tree.add_node(root_node, "brain", NodeKind::Script);
        tree.set_prop(
            brain,
            "source",
            Value::Str(format!("on \"step\" {{ sprite.pos = ({}, {}) }}", A.0, A.1)),
        )
        .unwrap();
        let label = tree.add_node(root_node, "label", NodeKind::Label);
        tree.set_local(label, Transform2D::from_pos(0.0, 0.0));
        tree.set_prop(label, PROP_LABEL_TEXT, Value::Str("st> boot".into()))
            .unwrap();
        tree.apply_pending();
        (sprite, brain, label)
    };

    let mut vm = ScriptVm::new();
    assert!(vm.attach_all(rt.tree_mut()).is_empty(), "初始装载");

    // 窗口两帧：初始脚本生效（钉 A）。
    let mut panel = Panel { input: String::new() };
    for i in 0..2 {
        rt.tree_mut().emit_signal("step", Value::I64(0));
        let _ = rt.collect_input(); // 清干净（无键入）
        rt.frame_windowed_with(&frame(i), &mut vm).unwrap().unwrap();
    }
    let f0 = rt.frame_with(&frame(2), &mut vm).unwrap();
    assert_eq!(f0.image.pixel(A.0 as u32 + 1, A.1 as u32 + 1), Some(CHECK_A), "A 位有棋盘");

    // 键入 B 脚本 + 回车（生产路径：字符队列）。
    for ch in format!("on \"step\" {{ sprite.pos = ({}, {}) }}", B.0, B.1).chars() {
        inject_input(InputEvent::Char(ch as u32));
    }
    inject_input(InputEvent::Char(0x0D));
    let mut committed = None;
    for i in 3..5 {
        rt.tree_mut().emit_signal("step", Value::I64(0));
        for code in rt.collect_input().text {
            if panel.feed(code) {
                let src = panel.input.clone();
                rt.tree_mut()
                    .set_prop(brain, "source", Value::Str(src))
                    .unwrap();
                committed = Some(vm.poll_reloads(rt.tree_mut()));
            }
        }
        rt.frame_windowed_with(&frame(i), &mut vm).unwrap().unwrap();
    }
    let (reloaded, failed) = committed.expect("回车已提交");
    assert_eq!(reloaded, vec![brain], "热重载恰命中 brain");
    assert!(failed.is_empty(), "无编译错误：{failed:?}");

    let f1 = rt.frame_with(&frame(5), &mut vm).unwrap();
    assert_eq!(f1.image.pixel(B.0 as u32 + 1, B.1 as u32 + 1), Some(CHECK_A), "B 位有棋盘");
    assert_eq!(f1.image.pixel(A.0 as u32 + 1, A.1 as u32 + 1), Some(CLEAR_RGBA), "A 位已空");

    // last-good：坏脚本（编译失败）如实报错、行为保持 B。
    panel.input.clear();
    for ch in "on \"step\" { sprite.pos = ) }".chars() {
        inject_input(InputEvent::Char(ch as u32));
    }
    inject_input(InputEvent::Char(0x0D));
    rt.tree_mut().emit_signal("step", Value::I64(0));
    for code in rt.collect_input().text {
        if panel.feed(code) {
            rt.tree_mut()
                .set_prop(brain, "source", Value::Str(panel.input.clone()))
                .unwrap();
        }
    }
    let (reloaded2, failed2) = vm.poll_reloads(rt.tree_mut());
    assert!(reloaded2.is_empty(), "坏脚本不应装载：{reloaded2:?}");
    assert_eq!(failed2.len(), 1, "编译错误如实上报：{failed2:?}");
    rt.frame_windowed_with(&frame(6), &mut vm).unwrap().unwrap();
    let f2 = rt.frame_with(&frame(7), &mut vm).unwrap();
    assert_eq!(
        f2.image.pixel(B.0 as u32 + 1, B.1 as u32 + 1),
        Some(CHECK_A),
        "last-good：精灵仍在 B"
    );

    // 面板文本有墨（默认字体 + Label 生效）。
    rt.tree_mut()
        .set_prop(label, PROP_LABEL_TEXT, Value::Str("st> OK".into()))
        .unwrap();
    let f3 = rt.frame_with(&frame(8), &mut vm).unwrap();
    let ink = (0..W)
        .flat_map(|x| (0..16u32).map(move |y| (x, y)))
        .filter(|&(x, y)| f3.image.pixel(x, y) == Some([255, 255, 255, 255]))
        .count();
    assert!(ink >= 8, "Label 字形有墨：{ink} px");
    assert_eq!(f3.stats.driver_errors, 0);
    let _ = sprite;
}
