//! 确定性指纹（S7.3）：场景**语义状态**的稳定哈希。
//!
//! # 口径（什么进哈希、什么绝不进）
//!
//! 进哈希的只有**语义状态**：树级（帧号/暂停/时间缩放）、按**前序**的
//! 节点序列（名字/类型/父（前序下标）/本地变换（逐字段 f32 位形）/
//! process_mode/生命周期位/属性表（BTreeMap 名序））、以及脚本局部
//!（按节点前序、局部名 BTree 序）。
//!
//! **绝不进**：GPU 句柄、指针、分配地址、HashMap 迭代布局、HWND、
//! 时间戳 —— 那些是"机器一样才相同"的伪确定性。世界变换缓存不进
//!（它是本地变换的派生，冲洗语义已由帧循环钉死）。分组表暂不进
//!（v1 尚无按组分派的行为面；出现时再裁剪口径）。
//!
//! 哈希函数：FNV-1a 64（与 nes-asset 内容戳同族），链式混合。

use std::collections::HashMap;

use nes_asset::fnv1a64;

use crate::script::ScriptVm;
use crate::identity::NodeId;
use crate::node::NodeKindTag;
use crate::tree::{ProcessMode, SceneTree};
use crate::value::Value;

/// 域标签（版本化：口径变更时改标签，旧哈希自然失效不误判相等）。
const DOMAIN: &[u8] = b"NES_SCENE_FP_V1";

/// 链式混合：把数据摺进当前哈希。
fn mix(h: u64, bytes: &[u8]) -> u64 {
    let mut buf = Vec::with_capacity(8 + bytes.len());
    buf.extend_from_slice(&h.to_le_bytes());
    buf.extend_from_slice(bytes);
    fnv1a64(&buf)
}

/// 值指纹（f32 取**位形** —— `-0.0` 与 `0.0` 位形不同是如实口径；
/// 位形相同的浮点值在 IEEE 语义下不可区分）。
fn mix_value(h: u64, v: &Value) -> u64 {
    match v {
        Value::F32(f) => mix(h, &f.to_bits().to_le_bytes()),
        Value::I64(i) => mix(h, &i.to_le_bytes()),
        Value::Bool(b) => mix(h, &[*b as u8]),
        Value::Str(s) => mix(h, s.as_bytes()),
        Value::Vec2(p) => mix(
            h,
            &[p.x.to_bits().to_le_bytes(), p.y.to_bits().to_le_bytes()].concat(),
        ),
        Value::Resource(slot) => mix(h, &slot.to_le_bytes()),
        // 节点句柄的**语义**指纹在调用点做（S8.2b v1.1：哈希 resolve
        // 结果 —— 活 -> 前序身份，悬垂 -> 规范 Dead 态；位形/gen 不进
        // 指纹）。此臂只作防御：属性表按 schema 不含 Node（走到这里
        // 说明口径被破坏，哈希标记形以便察觉）。
        Value::Node(_) => mix(h, b"<handle:unresolved>"),
    }
}

fn mix_process_mode(h: u64, m: ProcessMode) -> u64 {
    let tag: u8 = match m {
        ProcessMode::Inherit => 0,
        ProcessMode::Pausable => 1,
        ProcessMode::WhenPaused => 2,
        ProcessMode::Always => 3,
        ProcessMode::Disabled => 4,
    };
    mix(h, &[tag])
}

fn mix_kind(h: u64, tag: NodeKindTag) -> u64 {
    mix(h, tag.as_str().as_bytes())
}

/// 场景指纹：`tree` 的语义状态 +（可选）脚本 VM 的局部状态。
///
/// 相同场景 + 相同输入轨迹 + 相同帧数 ⇒ 逐帧指纹相同（S7.3 确定性
/// 契约，T-HR 系钉死）。前序下标是节点的哈希身份（slot/代际是内部
/// 实现细节，语义身份 = 结构位置 + 名字）。
pub fn scene_fingerprint(tree: &SceneTree, vm: Option<&ScriptVm>) -> u64 {
    let mut h = fnv1a64(DOMAIN);
    h = mix(h, &tree.frame().to_le_bytes());
    h = mix(h, &[tree.paused() as u8]);
    h = mix(h, &tree.time_scale().to_bits().to_le_bytes());

    let order: Vec<NodeId> = tree.preorder();
    // 节点 -> 前序下标（父的引用用下标表达；根的父 = usize::MAX）。
    let index: HashMap<NodeId, usize> =
        order.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    h = mix(h, &order.len().to_le_bytes());
    for (i, &node) in order.iter().enumerate() {
        let name = tree.name(node).unwrap_or("");
        h = mix(h, &(i as u64).to_le_bytes());
        h = mix(h, name.as_bytes());
        if let Some(tag) = tree.kind_tag(node) {
            h = mix_kind(h, tag);
        }
        // 父（前序下标）。
        let parent_idx: u64 = match tree.parent(node) {
            Some(p) => index.get(&p).copied().map(|i| i as u64).unwrap_or(u64::MAX),
            None => u64::MAX,
        };
        h = mix(h, &parent_idx.to_le_bytes());
        // 生命周期位。
        let flags = [tree.is_entered(node) as u8, tree.is_ready(node) as u8];
        h = mix(h, &flags);
        if let Some(mode) = tree.process_mode(node) {
            h = mix_process_mode(h, mode);
        }
        // 本地变换（逐字段位形）。
        if let Some(t) = tree.local(node) {
            let bytes = [
                t.pos.x.to_bits().to_le_bytes(),
                t.pos.y.to_bits().to_le_bytes(),
                t.rot.to_bits().to_le_bytes(),
                t.skew.to_bits().to_le_bytes(),
                t.scale.x.to_bits().to_le_bytes(),
                t.scale.y.to_bits().to_le_bytes(),
            ]
            .concat();
            h = mix(h, &bytes);
        }
        // 属性表（名字 BTree 序 —— PropStore 本身有序）。
        if let Some(props) = tree.props(node) {
            h = mix(h, &props.len().to_le_bytes());
            for (name, v) in props.iter() {
                h = mix(h, name.as_bytes());
                h = mix_value(h, v);
            }
        }
        // 脚本局部（同节点序；局部名 BTree 序）。
        if let Some(vm) = vm {
            if let Some(locals) = vm.locals(node) {
                h = mix(h, &locals.len().to_le_bytes());
                for (name, v) in &locals {
                    h = mix(h, name.as_bytes());
                    // 句柄的语义指纹 = **resolve 结果**（S8.2b v1.1）：
                    // 活 -> 所指节点的前序身份（v1 规范语义身份；Persistent
                    // NodeId 引入时升格）；悬垂 -> 规范 Dead 态（-1）。
                    // 位形/gen 是 allocator 历史，不进指纹 —— 换回收策略
                    // 指纹不变。别名（两个句柄指同一节点）自然折叠。
                    let v = match v {
                        Value::Node(hd) => {
                            let id = hd.to_id();
                            if tree.contains(id) {
                                Value::I64(index.get(&id).copied().map(|i| i as i64).unwrap_or(-2))
                            } else {
                                Value::I64(-1)
                            }
                        }
                        other => other.clone(),
                    };
                    h = mix_value(h, &v);
                }
            }
        }
        h = mix(h, b"|"); // 节点分隔
    }
    h
}
