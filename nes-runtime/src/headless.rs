//! Headless / 确定性运行（S7.3）。
//!
//! # 架构口径
//!
//! **同一运行时、同一 tick 语义，无 GPU/窗口**（[`crate::NesRuntime::
//! open_headless`]）—— headless 不是"窗口关掉继续跑"，更不是第二套
//! 运行时。装载/输入/信号/tick 与窗口模式逐字节同路径；只有渲染端
//!（帧呈现/纹理上传）如实缺席。
//!
//! # 确定性管线
//!
//! ```text
//! InputTrace（帧号 + 事件批，parse_trace 解析）
//!     ↓ 逐帧：inject_input（与真实消息同一队列）
//! collect_input -> emit_input_signals -> tick_headless
//!     ↓
//! state_fingerprint（语义状态：场景 + 脚本局部 + 按住键）
//!     ↓
//! frame_hashes[0..N] -> trace_hash（差分测试：第一处差异定位到帧）
//! ```

use std::path::Path;

use nes_asset::fnv1a64;
use nes_render_api::input::InputTrace;
use nes_render_wgpu::window::inject_input;
use nes_scene::{ScriptVm, TickStats};

use crate::{BackendError, NesRuntime};

/// headless 运行报告：逐帧指纹 + 轨迹总指纹。
#[derive(Clone, Debug, PartialEq)]
pub struct HeadlessReport {
    /// 每帧末的语义状态指纹（帧 0..N-1 顺序）。
    pub frame_hashes: Vec<u64>,
    /// 全轨迹指纹（由逐帧指纹链式混合）—— 两个运行等价 ⟺ 它相等
    ///（逆否命题：不等 ⟹ 至少一帧不等，`frame_hashes` 定位第一处）。
    pub trace_hash: u64,
}

impl NesRuntime {
    /// headless 推进一帧（**与 `frame*` 共用同一条 `SceneTree::tick` 路径**，
    /// 只是不做提取/渲染/呈现 —— 语义零分支）。
    pub fn tick_headless(
        &mut self,
        delta: f32,
        obs: &mut dyn nes_scene::SceneObserver,
    ) -> TickStats {
        self.tree.tick(delta, obs)
    }

    /// 语义状态指纹：场景（结构/变换/属性/生命周期 + 脚本局部）⊕ 输入
    /// 按住态（影响 `key()` 探针的后续行为，属语义状态）。
    pub fn state_fingerprint(&self, vm: Option<&ScriptVm>) -> u64 {
        let mut h = nes_scene::scene_fingerprint(&self.tree, vm);
        let snap = self.input_state.borrow();
        h = mix(h, b"held");
        for k in &snap.held {
            h = mix(h, k.name().as_bytes());
        }
        h = mix(h, b"buttons");
        let buttons = snap.buttons_held.map(|b| b as u8);
        h = mix(h, &buttons);
        h
    }

    /// 跑一段 headless 确定性运行：加载场景 -> 装载脚本 -> 逐帧
    /// （注入轨迹 -> 收集输入 -> 发 `input/*` 信号 -> tick -> 指纹）。
    ///
    /// 外置 `.nes` 脚本经磁盘读取闭包装载（与窗口宿主同一装载路径）。
    /// `delta` 固定值（如 1/60）—— 确定性口径下时间也是输入。
    pub fn run_headless(
        &mut self,
        scene_rel: &str,
        trace: &[InputTrace],
        frames: u64,
        delta: f32,
    ) -> Result<HeadlessReport, BackendError> {
        // 队列清残留（进程级静态：其他测试/注入的遗留不该进本轨迹）。
        let _ = nes_render_wgpu::window::drain_input();
        self.load_scene(scene_rel)?;
        let mut vm = ScriptVm::new();
        {
            let table = self.resources_mut().clone();
            let root = self.root.clone();
            let issues = vm.attach_all_with_sources(self.tree_mut(), &table, &mut |rel| {
                let text = std::fs::read_to_string(root.join(rel))
                    .map_err(|e| e.to_string())?;
                Ok(text)
            });
            if !issues.is_empty() {
                return Err(BackendError::ConfigMismatch(format!(
                    "脚本装载缺口：{issues:?}"
                )));
            }
        }
        self.mount_input_view(&mut vm);

        let mut frame_hashes = Vec::with_capacity(frames as usize);
        for f in 0..frames {
            for t in trace.iter().filter(|t| t.frame == f) {
                for ev in &t.events {
                    inject_input(*ev); // Copy：与真实消息同一队列
                }
            }
            let snap = self.collect_input();
            self.emit_input_signals(&snap);
            // 与窗口帧路径同一节拍实现（内建 tick + 蓄步 —— 宿主分支为零）。
            let _steps = self.step_headless(delta, &mut vm);
            frame_hashes.push(self.state_fingerprint(Some(&vm)));
        }
        let mut h = fnv1a64(b"NES_TRACE_V1");
        for fh in &frame_hashes {
            h = mix(h, &fh.to_le_bytes());
        }
        Ok(HeadlessReport { frame_hashes, trace_hash: h })
    }
}

/// 链式混合（与 nes-scene 确定性指纹同族）。
fn mix(h: u64, bytes: &[u8]) -> u64 {
    let mut buf = Vec::with_capacity(8 + bytes.len());
    buf.extend_from_slice(&h.to_le_bytes());
    buf.extend_from_slice(bytes);
    fnv1a64(&buf)
}

/// 便捷入口：从资产根直接跑（CLI 与测试共用同一条路径 —— CLI 不是
/// 第二个运行时）。
pub fn run(
    root: &Path,
    scene_rel: &str,
    trace: &[InputTrace],
    frames: u64,
    delta: f32,
) -> Result<HeadlessReport, BackendError> {
    let mut rt = NesRuntime::open_headless(root)?;
    rt.run_headless(scene_rel, trace, frames, delta)
}
