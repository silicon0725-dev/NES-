//! 稳定身份与代际 arena。
//!
//! 纪律与 TWN 既有资产同构（`RenderAssetKey` vs `TextureHandle`）：
//!
//! - [`NodeId`] 是**稳定身份**：可序列化、跨帧不变；一经删除，其 `(slot, gen)`
//!   永不复用。存档、脚本引用、信号连接表里只允许出现它。
//! - [`NodeHandle`] 是**临时句柄**：只允许出现在帧内缓存与热路径索引中，
//!   不得进入任何持久化结构。
//!
//! 把这条纪律保持住的收益：悬垂引用会变成 `None` 而不是访问到"另一个恰好
//! 复用了同一槽位的节点"。后者是最难查的一类 bug。

use core::fmt;

/// 稳定身份。`(slot, gen)` 全局唯一且永不复用。
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    slot: u32,
    gen: u32,
}

impl NodeId {
    /// 槽位下标。仅用于确定性兜底排序，业务代码不应依赖其含义。
    pub fn slot(self) -> u32 {
        self.slot
    }

    /// 代际号。每删除一次 +1。
    pub fn generation(self) -> u32 {
        self.gen
    }

    /// 压成 u64，供外部索引 / FFI / 序列化使用。
    pub fn to_bits(self) -> u64 {
        ((self.gen as u64) << 32) | (self.slot as u64)
    }

    /// 从 [`Self::to_bits`] 的产物还原。
    pub fn from_bits(bits: u64) -> Self {
        Self {
            slot: (bits & 0xFFFF_FFFF) as u32,
            gen: (bits >> 32) as u32,
        }
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 刻意用紧凑格式：日志与断言失败信息里会大量出现。
        write!(f, "#{}v{}", self.slot, self.gen)
    }
}

/// 脚本可见的弱句柄（S8.2b-1，v1.1 口径）：**可跨帧持有；不保证目标
/// 跨帧存活；每次解引用重新验证**（gen 防"悬垂撞上复用槽位的另一个
/// 节点"）。gen 是 allocator 安全机制，**不是游戏语义** —— 语义指纹
/// 哈希 resolve 的结果（规范语义身份 / 规范 Dead 态），不哈希位形。
/// 不序列化（场景内节点引用 = 路径，两者不混）。
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NodeHandle(u64);

impl NodeHandle {
    /// 由稳定身份派生句柄。
    pub fn of(id: NodeId) -> Self {
        Self(id.to_bits())
    }

    /// 还原稳定身份。因为句柄只是 id 的按位副本，还原永远有效。
    pub fn to_id(self) -> NodeId {
        NodeId::from_bits(self.0)
    }
}

struct Slot<T> {
    gen: u32,
    value: Option<T>,
}

/// 代际 arena。删除即 `gen` 递增，保证 `(slot, gen)` 不复用。
///
/// 不依赖外部 arena crate，理由见 crate 根注释；替换面收敛在此文件。
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Arena<T> {
    /// 空 arena。
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }

    /// 插入并返回稳定身份。
    pub fn insert(&mut self, value: T) -> NodeId {
        if let Some(slot) = self.free.pop() {
            let s = &mut self.slots[slot as usize];
            debug_assert!(s.value.is_none(), "空闲槽位必须为空");
            s.value = Some(value);
            self.len += 1;
            NodeId { slot, gen: s.gen }
        } else {
            let slot = self.slots.len() as u32;
            self.slots.push(Slot {
                gen: 0,
                value: Some(value),
            });
            self.len += 1;
            NodeId { slot, gen: 0 }
        }
    }

    /// 删除。代际号不匹配（即传入已失效的 id）时返回 `None`。
    pub fn remove(&mut self, id: NodeId) -> Option<T> {
        let s = self.slots.get_mut(id.slot as usize)?;
        if s.gen != id.gen {
            return None;
        }
        let v = s.value.take()?;
        // wrapping 仅在 2^32 次回收同一槽位后才会回绕，实际不可达；即便如此也不 panic。
        s.gen = s.gen.wrapping_add(1);
        self.free.push(id.slot);
        self.len -= 1;
        Some(v)
    }

    /// 代际校验读取。失效 id 返回 `None`。
    pub fn get(&self, id: NodeId) -> Option<&T> {
        let s = self.slots.get(id.slot as usize)?;
        if s.gen != id.gen {
            return None;
        }
        s.value.as_ref()
    }

    /// 代际校验可变读取。失效 id 返回 `None`。
    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut T> {
        let s = self.slots.get_mut(id.slot as usize)?;
        if s.gen != id.gen {
            return None;
        }
        s.value.as_mut()
    }

    /// 是否存在且代际有效。
    pub fn contains(&self, id: NodeId) -> bool {
        self.get(id).is_some()
    }

    /// 存活元素数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 遍历全部存活元素。顺序为槽位下标升序 —— **这是确定性的**，可依赖。
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &T)> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.value.as_ref().map(|v| {
                (
                    NodeId {
                        slot: i as u32,
                        gen: s.gen,
                    },
                    v,
                )
            })
        })
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generations_never_alias() {
        let mut a: Arena<u32> = Arena::new();
        let first = a.insert(1);
        assert_eq!(a.remove(first), Some(1));

        let second = a.insert(2);
        // 槽位复用，但身份必须不同。
        assert_eq!(first.slot(), second.slot());
        assert_ne!(first, second);
        // 旧身份失效，不会读到新值 —— 这正是双身份的收益。
        assert!(a.get(first).is_none());
        assert_eq!(a.get(second), Some(&2));
    }

    #[test]
    fn remove_is_idempotent_and_gen_checked() {
        let mut a: Arena<u32> = Arena::new();
        let id = a.insert(7);
        assert_eq!(a.remove(id), Some(7));
        assert_eq!(a.remove(id), None);
        assert_eq!(a.len(), 0);
    }

    #[test]
    fn handle_roundtrip_is_lossless() {
        let mut a: Arena<u32> = Arena::new();
        let id = a.insert(1);
        let _ = a.insert(2);
        _ = a.remove(id);
        let id3 = a.insert(3);
        assert_eq!(NodeHandle::of(id3).to_id(), id3);
        assert_eq!(NodeId::from_bits(id3.to_bits()), id3);
    }

    #[test]
    fn iter_is_slot_ascending() {
        let mut a: Arena<u32> = Arena::new();
        let x = a.insert(10);
        let _ = x;
        let _y = a.insert(20);
        let _z = a.insert(30);
        let slots: Vec<u32> = a.iter().map(|(id, _)| id.slot()).collect();
        let mut sorted = slots.clone();
        sorted.sort_unstable();
        assert_eq!(slots, sorted);
    }
}
