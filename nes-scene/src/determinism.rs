//! 确定性指纹（S7.3）：场景**语义状态**的稳定哈希。
//!
//! # 口径（什么进哈希、什么绝不进）
//!
//! 进哈希的只有**语义状态**：树级（帧号/暂停/时间缩放）、按**前序**的
//! 节点序列（名字/类型/父（前序下标）/本地变换（逐字段 f32 位形）/
//! process_mode/生命周期位/属性表（BTreeMap 名序））、脚本局部
//!（按节点前序、局部名 BTree 序）、以及**位置补间登记表**（S16 起，
//! 条件混入：登记表非空才摺进 —— 目标 uid + from/to/elapsed/duration
//! 位形；无补间场景的指纹与旧口径逐位相同）。
//!
//! **绝不进**：GPU 句柄、指针、分配地址、HashMap 迭代布局、HWND、
//! 时间戳 —— 那些是"机器一样才相同"的伪确定性。世界变换缓存不进
//!（它是本地变换的派生，冲洗语义已由帧循环钉死）。分组表暂不进
//!（v1 尚无按组分派的行为面；出现时再裁剪口径）。
//!
//! 哈希函数：FNV-1a 64（与 nes-asset 内容戳同族），链式混合。


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
        // 数组：元素逐个语义化（调用点的 semantic_value 已展开 Node，
        // 这里只见非句柄元素 —— 防御臂与 Node 同理）。
        Value::Array(items) => {
            let mut hh = mix(h, &items.len().to_le_bytes());
            for item in items {
                hh = mix_value(hh, item);
            }
            hh
        }
    }
}

/// 局部值的语义化（S8.2b v1.1/b-2）：句柄 -> resolve 结果（前序身份 /
/// Dead=-1）；数组 -> 元素逐个语义化（嵌套数组递归）。位形/gen 不进指纹。
fn semantic_value(v: &Value, tree: &SceneTree) -> Value {
    match v {
        Value::Node(hd) => {
            // 句柄语义指纹 = resolve 的 **uid**（S9-1：与前序身份同步切换；
            // 死句柄规范 Dead 态 = 全 1 位形 —— 与合法 uid 区分）。
            let id = hd.to_id();
            if tree.contains(id) {
                match tree.uid_of(id) {
                    Some(u) => {
                        let b = u.bits();
                        let lo = i64::from_le_bytes(b[0..8].try_into().unwrap());
                        let hi = i64::from_le_bytes(b[8..16].try_into().unwrap());
                        Value::Array(vec![Value::I64(lo), Value::I64(hi)])
                    }
                    None => Value::I64(-2),
                }
            } else {
                Value::I64(-1)
            }
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|i| semantic_value(i, tree)).collect())
        }
        other => other.clone(),
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
    h = mix(h, &order.len().to_le_bytes());
    for &node in order.iter() {
        let name = tree.name(node).unwrap_or("");
        // **canonical semantic identity = Persistent uid**（S9-1 一次性
        // 切换，S9-0 Q9：不留双轨）。前序 i 只作遍历序不再作身份。
        h = mix(h, b"uid");
        h = mix(h, &tree.uid_of(node).map(|u| u.bits()).unwrap_or([0u8; 16]));
        h = mix(h, name.as_bytes());
        if let Some(tag) = tree.kind_tag(node) {
            h = mix_kind(h, tag);
        }
        // 父引用：父的 **uid**（canonical 身份切换的完整性 —— 结构引用
        // 与节点身份同源；根的父 = 全 1 位形哨兵）。
        let parent_uid: [u8; 16] = match tree.parent(node).and_then(|p| tree.uid_of(p)) {
            Some(u) => u.bits(),
            None => [0xFF; 16],
        };
        h = mix(h, &parent_uid);
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
                    h = mix_value(h, &semantic_value(v, tree));
                }
            }
        }
        h = mix(h, b"|"); // 节点分隔
    }

    // 补间登记表（S16 第 1 期）—— **条件混入**：补间是游戏可见状态
    //（每 tick 直写节点 local），登记表本身必须可复现、进指纹；但采样
    // 面做成"有补间才摺进" —— 无补间的场景（登记表空）零混入，既有
    // 基线指纹逐位不变（S16 冻结：基线漂移即为实现错误）。
    // 字段口径：目标锚定 **uid**（与节点身份同源 —— 句柄位形/gen 是
    // allocator 历史不进指纹；死目标按全 1 位形规范 Dead 态如实混入，
    // 推进阶段理应已自动清，这里是防御口径）；from/to/elapsed/duration
    // 取位形（f32/f64 逐位 —— 与本地变换同一口径）。
    let tweens = tree.tweens();
    if !tweens.is_empty() {
        h = mix(h, b"tweens");
        h = mix(h, &tweens.len().to_le_bytes());
        for tw in tweens {
            let target_uid: [u8; 16] = tree
                .uid_of(tw.target.to_id())
                .map(|u| u.bits())
                .unwrap_or([0xFF; 16]);
            h = mix(h, &target_uid);
            h = mix(h, &tw.from.x.to_bits().to_le_bytes());
            h = mix(h, &tw.from.y.to_bits().to_le_bytes());
            h = mix(h, &tw.to.x.to_bits().to_le_bytes());
            h = mix(h, &tw.to.y.to_bits().to_le_bytes());
            h = mix(h, &tw.elapsed_ms.to_bits().to_le_bytes());
            h = mix(h, &tw.duration_ms.to_bits().to_le_bytes());
        }
    }
    h
}
