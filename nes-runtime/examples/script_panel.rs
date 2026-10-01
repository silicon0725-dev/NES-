//! S6.34 演示：**编辑器脚本面板** —— 在引擎自己的窗口里实时编辑脚本。
//!
//! 布局（768x432，字形 16px 等宽）：
//! - 上半：精灵演示区（动画由 brain 脚本驱动 —— 面板正在编辑的那份行为）；
//! - 下半（HUD 边框面板）：三行 Label ——
//!   - `src>` 当前生效源码（编译通过的最新版）；
//!   - `in >` 输入行（键入即回显 + 光标 `_`；退格删除；回车提交）；
//!   - `st >` 状态行（OK 重载 / 错误信息 —— 出错时精灵保持旧行为）。
//!
//! 输入链（S7.2 升级）：平台消息 -> 中性事件队列 -> `rt.collect_input`
//! 帧快照 -> text 字段 -> 宿主编辑解释（0x08 退格 / 0x0D 提交 /
//! 可打印追加）-> set_prop source -> `vm.poll_reloads` -> 下一帧
//! 行为变化。**引擎用自己的渲染管线显示自己的编辑器**。
//!
//! 运行：`cargo run --example script_panel`
//! 自动化钩子：`NES_PANEL_FRAMES=N`（N 帧后退出）、
//! `NES_PANEL_TYPE=<脚本>`（第 80 帧注入为键入 + 回车）。

use std::time::Duration;

use nes_render_api::{FrameInfo, Vec2};
use nes_render_extract::{
    PROP_CONTROL_ANCHOR, PROP_CONTROL_OFFSET, PROP_CONTROL_SIZE, PROP_LABEL_TEXT, PROP_TEXTURE,
};
use nes_render_api::input::InputEvent;
use nes_render_wgpu::window::inject_input;
use nes_render_wgpu::{bmp, FontParams};
use nes_runtime::{write_bmp_rgba, NesRuntime};
use nes_scene::{NodeKind, NodeId, ScriptVm, Transform2D, Value};

/// 16x16 棋盘（演示精灵纹理）。
fn checker_rgba() -> Vec<u8> {
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            rgba.extend_from_slice(if (x / 4 + y / 4) % 2 == 0 {
                &[0, 200, 120, 255]
            } else {
                &[20, 30, 50, 255]
            });
        }
    }
    rgba
}

/// 显示文本手动折行（面板 46 字符宽；Label 的 \n 分行，空格无墨只推进）。
fn wrap(text: &str, prefix: &str, width: usize) -> String {
    assert!(width > prefix.len());
    let mut out = String::new();
    let cap = width - prefix.len();
    let mut line = String::from(prefix);
    for ch in text.chars() {
        if line.chars().count() >= cap {
            out.push_str(&line);
            out.push('\n');
            line.clear();
        }
        line.push(ch);
    }
    out.push_str(&line);
    out
}

/// 面板状态（编辑缓冲 + 最近一次提交结果）。
struct Panel {
    input: String,
    status: String,
}

impl Panel {
    fn feed(&mut self, code: u32) -> bool {
        match code {
            0x0D => true, // 回车：提交
            0x08 => {
                self.input.pop();
                false
            }
            c if (32..127).contains(&c) => {
                if self.input.chars().count() < 120 {
                    self.input.push(c as u8 as char);
                }
                false
            }
            _ => false, // 其他控制/非 ASCII 忽略（字体表只覆盖 32..127）
        }
    }
}

fn main() {
    let root = std::env::temp_dir().join("nes_runtime_panel");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Textures")).unwrap();
    write_bmp_rgba(&root.join("Textures").join("grid.bmp"), 16, 16, &checker_rgba())
        .expect("写棋盘");

    let mut rt = NesRuntime::open_windowed_with_root(
        &root,
        "NES 2.0 - script panel (S6.34)",
        768,
        432,
    )
    .expect("窗口装配");
    let res = rt.declare_texture("Textures/grid.bmp").expect("声明纹理");
    let report = rt.bind_assets();
    assert_eq!(report.loaded.len(), 1, "纹理绑定：{report:?}");
    assert_eq!(rt.upload_pending_textures().expect("上传"), 1);

    // 默认字体：外部烘焙字形表（nes-render-wgpu 示例资产，与 s44 同一产物；
    // 编辑器要显示自己的文本，前提是 Label 有字形可排）。
    {
        let font_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../nes-render-wgpu/examples/assets");
        let (w, h, sheet) =
            bmp::load_rgba(&std::fs::read(font_dir.join("font_atlas.bmp")).expect("读字形表"))
                .expect("解码字形表");
        let metrics = std::fs::read_to_string(font_dir.join("font_metrics.txt"))
            .expect("读字形表参数");
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
            .expect("font_metrics 缺 cell");
        let (cw, ch) = cell
            .split_once('x')
            .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
            .expect("cell 格式");
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

    // 初始脚本：精灵向右漂移（Vec2 字面量仅数字 —— `(t/2, 48)` 是 Pack
    // 缺口，不用）。
    /// 初始脚本：信号入口（process 只许写自身 —— T-VM-05 契约）。
    const INITIAL: &str = "on \"step\" { sprite.pos += (2.0, 0.0) }";

    // 场景：相机 + 演示精灵 + 脚本 + 面板（HUD 边框 + 三行 Label）。
    let (sprite, brain, l_src, l_in, l_st) = {
        let tree = rt.tree_mut();
        let root_node = tree.root();
        let cam = tree.add_node(root_node, "cam", NodeKind::Camera2D);
        tree.set_local(cam, Transform2D::from_pos(384.0, 216.0));

        let sprite = tree.add_node(root_node, "sprite", NodeKind::Sprite2D);
        tree.set_prop(sprite, PROP_TEXTURE, res.to_value()).unwrap();
        tree.set_local(sprite, Transform2D::from_pos(16.0, 48.0));

        let brain = tree.add_node(root_node, "brain", NodeKind::Script);
        tree.set_prop(brain, "source", Value::Str(INITIAL.into())).unwrap();

        // 面板背景（HUD 边框）：锚左上，offset(8,200) size(752,224)。
        let panel = tree.add_node(root_node, "panel", NodeKind::Control);
        tree.set_local(panel, Transform2D::from_pos(0.0, 0.0));
        tree.set_prop(panel, PROP_CONTROL_ANCHOR, Value::Vec2(nes_scene::Vec2::new(0.0, 0.0)))
            .unwrap();
        tree.set_prop(panel, PROP_CONTROL_OFFSET, Value::Vec2(nes_scene::Vec2::new(8.0, 200.0)))
            .unwrap();
        tree.set_prop(panel, PROP_CONTROL_SIZE, Value::Vec2(nes_scene::Vec2::new(752.0, 224.0)))
            .unwrap();

        // 三行 Label（面板内 16px 行高）。
        let mut mk = |name: &str, y: f32| -> NodeId {
            let l = tree.add_node(root_node, name, NodeKind::Label);
            tree.set_local(l, Transform2D::from_pos(16.0, y));
            tree.set_prop(l, PROP_LABEL_TEXT, Value::Str(String::new())).unwrap();
            l
        };
        let l_src = mk("l_src", 216.0);
        let l_in = mk("l_in", 280.0);
        let l_st = mk("l_st", 344.0);
        tree.apply_pending();
        (sprite, brain, l_src, l_in, l_st)
    };

    let mut vm = ScriptVm::new();
    let issues = vm.attach_all(rt.tree_mut());
    assert!(issues.is_empty(), "初始装载：{issues:?}");

    let mut panel = Panel {
        input: INITIAL.to_string(),
        status: "st> OK initial script loaded".to_string(),
    };

    // 自动化钩子。
    let total: u64 = std::env::var("NES_PANEL_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);
    let typed: Option<String> = std::env::var("NES_PANEL_TYPE").ok();

    let mut rendered = 0u64;
    // 瞬态容错（收束阶段）：表面获取 Timeout 一类瞬态错跳过该帧重试
    // （引擎按可重试类如实分类了）；连续失败超限才退出 —— 不让一次
    // 遮挡/合成器停顿杀死编辑器。
    let mut transient = 0u64;
    const TRANSIENT_LIMIT: u64 = 120;
    for index in 0..total {
        if index == 80 {
            if let Some(script) = typed.as_deref() {
                panel.input.clear();
                for ch in script.chars() {
                    inject_input(InputEvent::Char(ch as u32));
                }
                inject_input(InputEvent::Char(0x0D));
            }
        }
        // 宿主节拍：每帧发 step（脚本行为的驱动源）。
        rt.tree_mut().emit_signal("step", Value::I64(0));
        // 输入解释：帧输入快照的 text 字段（WM_CHAR 在这层已经不是 API）
        // -> 编辑缓冲；回车 -> 提交热重载。
        for code in rt.collect_input().text {
            if panel.feed(code) {
                let src = panel.input.clone();
                rt.tree_mut().set_prop(brain, "source", Value::Str(src)).unwrap();
                let (re, fa) = vm.poll_reloads(rt.tree_mut());
                if fa.is_empty() {
                    panel.status = format!("st> OK reloaded {} (behavior updated)", re.len());
                } else {
                    // last-good：旧行为继续跑，错误如实显示。
                    panel.status = format!("st> ERR {} (kept last-good)", fa[0].1);
                }
            }
        }
        // 面板刷新（Label 文本即渲染内容）。
        {
            let cur = match rt.tree_mut().prop(brain, "source") {
                Some(Value::Str(s)) => s.clone(),
                _ => String::new(),
            };
            let tree = rt.tree_mut();
            tree.set_prop(l_src, PROP_LABEL_TEXT, Value::Str(wrap(&cur, "src> ", 46)))
                .unwrap();
            tree.set_prop(
                l_in,
                PROP_LABEL_TEXT,
                Value::Str(format!("in> {}_", panel.input)),
            )
            .unwrap();
            tree.set_prop(l_st, PROP_LABEL_TEXT, Value::Str(panel.status.clone()))
                .unwrap();
        }

        let frame = FrameInfo::new(
            index,
            1.0 / 60.0,
            index as f64 / 60.0,
            Vec2::new(768.0, 432.0),
        );
        match rt.frame_windowed_with(&frame, &mut vm) {
            Ok(Some(stats)) => {
                if stats.driver_errors > 0 {
                    eprintln!("[帧 {index}] driver_errors={}", stats.driver_errors);
                }
                rendered += 1;
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
                    eprintln!("连续瞬态失败超限，退出");
                    std::process::exit(1);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    println!("[完成] 共呈现 {rendered} 帧；编辑器面板退出");
    let _ = sprite;
}
