//! 资源身份：[`AssetKey`] 与资源分类 [`AssetKind`]。
//!
//! 纪律与 `nes-scene` 的 `NodeId` 完全同构：`(slot, gen)` 一经分配即全局唯一，
//! 删除后 `gen` 递增，**永不复用**。悬垂引用因此变成 `None`，而不是"访问到
//! 恰好复用同一槽位的另一个资源"。

use core::fmt;

/// 资源分类。
///
/// 分类是**身份的一部分**：同一路径以不同 kind 注册会得到不同的 [`AssetKey`]，
/// 因为它们走的是不同的加载/解码通路。
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum AssetKind {
    /// 纹理（`Textures/`、png 等）。
    Texture,
    /// 音频（ogg/wav）。
    Audio,
    /// 视频（S15：amv/avi —— 容器经 nes-media 解析，当前帧经渲染侧
    /// 同键逐帧覆写上 GPU；`is_render_facing` = true，与纹理共用
    /// RenderAssetKey 命名空间机制）。
    Video,
    /// 字体（ttf/otf）。
    Font,
    /// 子场景（ron）。
    Scene,
    /// 脚本（将来的可视化脚本 / JS 扩展入口）。
    Script,
    /// 着色器（wgsl）。
    Shader,
    /// 数据（json/csv/二进制等）。
    Data,
}

impl AssetKind {
    /// 全部分类，顺序与 [`AssetKind::as_str`] 的声明顺序一致。
    pub const ALL: [AssetKind; 8] = [
        Self::Texture,
        Self::Audio,
        Self::Video,
        Self::Font,
        Self::Scene,
        Self::Script,
        Self::Shader,
        Self::Data,
    ];

    /// 稳定字符串名。用于序列化、日志与编辑器，**不得**随重构改名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Texture => "Texture",
            Self::Audio => "Audio",
            Self::Video => "Video",
            Self::Font => "Font",
            Self::Scene => "Scene",
            Self::Script => "Script",
            Self::Shader => "Shader",
            Self::Data => "Data",
        }
    }

    /// 从稳定字符串名还原。
    pub fn from_str_exact(s: &str) -> Option<Self> {
        Some(match s {
            "Texture" => Self::Texture,
            "Audio" => Self::Audio,
            "Video" => Self::Video,
            "Font" => Self::Font,
            "Scene" => Self::Scene,
            "Script" => Self::Script,
            "Shader" => Self::Shader,
            "Data" => Self::Data,
            _ => return None,
        })
    }

    /// 该分类是否面向渲染后端（即是否拥有 `RenderAssetKey` 视图）。
    ///
    /// 草案第 11 节：`RenderAssetKey` 视为 AssetRegistry 中 **Texture 类**的一面视图。
    /// S15 起 **Video 类同面**：视频资源的"当前帧"就是一张纹理 —— 每帧
    /// 经渲染注册表同键覆写（字形页/纹理热重载已验证的路径），Sprite2D
    /// 的 texture 属性经提取层 `RenderKeySource` 解析到同一个键。键位
    /// 编码只含 `(slot, gen)`，两类资源天然不同槽位，命名空间不冲突。
    pub const fn is_render_facing(self) -> bool {
        matches!(self, Self::Texture | Self::Video)
    }
}

/// 稳定资源身份。`(slot, gen, kind)` 全局唯一且永不复用。
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetKey {
    slot: u32,
    gen: u32,
    kind: AssetKind,
}

impl AssetKey {
    /// 空键（`slot == 0`）。槽位 0 保留给"未绑定"，因此它天然映射
    /// `nes-scene` 里 `Value::Resource(0)` 的占位语义。
    pub const NIL: AssetKey = AssetKey {
        slot: 0,
        gen: 0,
        kind: AssetKind::Data,
    };

    /// 指定分类的空键。
    pub const fn nil_of(kind: AssetKind) -> Self {
        Self {
            slot: 0,
            gen: 0,
            kind,
        }
    }

    /// 构造（供注册表内部使用；外部只应通过 [`crate::AssetRegistry::register`] 获得）。
    pub(crate) const fn new(slot: u32, gen: u32, kind: AssetKind) -> Self {
        Self { slot, gen, kind }
    }

    /// 是否为空键（未绑定）。
    pub const fn is_nil(self) -> bool {
        self.slot == 0
    }

    /// 槽位下标。仅用于确定性排序与诊断，业务代码不应依赖其含义。
    pub const fn slot(self) -> u32 {
        self.slot
    }

    /// 代际号。每删除一次 +1。
    pub const fn generation(self) -> u32 {
        self.gen
    }

    /// 资源分类。
    pub const fn kind(self) -> AssetKind {
        self.kind
    }

    /// 压成 u64，供外部索引 / FFI / 与 `nes-scene` 的 `Value::Resource(u64)` 互操作。
    ///
    /// **注意**：编码只覆盖 `(slot, gen)`，不含 `kind`。分类由调用方提供
    /// （在场景里由属性 schema 的 `EditorHint::Resource { kind }` 声明），
    /// 这是刻意设计——`Value` 只有一个 u64 的宽度，而分类是 schema 的知识。
    pub const fn to_bits(self) -> u64 {
        ((self.gen as u64) << 32) | (self.slot as u64)
    }

    /// 从 [`Self::to_bits`] 的产物还原（分类须由调用方给出）。
    pub const fn from_bits(bits: u64, kind: AssetKind) -> Self {
        Self {
            slot: (bits & 0xFFFF_FFFF) as u32,
            gen: (bits >> 32) as u32,
            kind,
        }
    }

    /// 取渲染侧视图。非渲染类资源返回 `None`（保持"一套身份"的纪律）。
    pub fn as_render_key(self) -> Option<RenderAssetKeyView> {
        if self.kind.is_render_facing() && !self.is_nil() {
            Some(RenderAssetKeyView::from_bits(self.to_bits()))
        } else {
            None
        }
    }

    /// 由渲染侧视图还原。分类必须是渲染类的（即 [`AssetKind::Texture`]）。
    pub fn from_render_key(view: RenderAssetKeyView, kind: AssetKind) -> Option<Self> {
        if !kind.is_render_facing() {
            return None;
        }
        Some(Self::from_bits(view.to_bits(), kind))
    }
}

impl fmt::Debug for AssetKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 紧凑格式：日志与断言失败信息里会大量出现。
        write!(f, "{}#{}v{}", self.kind.as_str(), self.slot, self.gen)
    }
}

/// `twn-render-resources::RenderAssetKey` 的**镜像视图**。
///
/// 为什么不直接依赖那个 crate：它位于已 DRIVE SEALED 的 TWN 包内，且其
/// native 后端锁在 `target_os = "linux"`，本机无法编译（见 NES 2.0 阻塞清单）。
/// 因此 M3 采用**位编码层面统一**：两侧都只认一个 `u64`，且都遵守
/// 「稳定身份 vs 易变后端句柄」的双身份纪律。将来移植 `twn-render-resources`
/// 时，只需把本视图换成真类型，`AssetRegistry` 一行不改。
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RenderAssetKeyView {
    bits: u64,
}

impl RenderAssetKeyView {
    /// 由位编码构造。
    pub const fn from_bits(bits: u64) -> Self {
        Self { bits }
    }

    /// 位编码。
    pub const fn to_bits(self) -> u64 {
        self.bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names_roundtrip() {
        for kind in AssetKind::ALL {
            assert_eq!(AssetKind::from_str_exact(kind.as_str()), Some(kind));
        }
        assert_eq!(AssetKind::from_str_exact("texture"), None);
    }

    #[test]
    fn bits_roundtrip_keeps_slot_and_gen() {
        let k = AssetKey::new(7, 3, AssetKind::Texture);
        let restored = AssetKey::from_bits(k.to_bits(), AssetKind::Texture);
        assert_eq!(restored, k);
        assert_eq!(k.slot(), 7);
        assert_eq!(k.generation(), 3);
    }

    #[test]
    fn nil_maps_to_zero_bits() {
        assert!(AssetKey::NIL.is_nil());
        assert_eq!(AssetKey::NIL.to_bits(), 0);
        assert!(AssetKey::from_bits(0, AssetKind::Texture).is_nil());
    }

    #[test]
    fn render_view_is_texture_and_video_only() {
        let tex = AssetKey::new(1, 0, AssetKind::Texture);
        let view = tex.as_render_key().expect("texture 应有渲染视图");
        assert_eq!(view.to_bits(), tex.to_bits());
        assert_eq!(AssetKey::from_render_key(view, AssetKind::Texture), Some(tex));

        // S15：Video 类与 Texture 同为渲染面（视频当前帧 = 同键覆写的
        // 纹理载体）；其余类别仍然无渲染视图。
        let video = AssetKey::new(2, 0, AssetKind::Video);
        let vview = video.as_render_key().expect("video 应有渲染视图");
        assert_eq!(vview.to_bits(), video.to_bits());
        assert_eq!(AssetKey::from_render_key(vview, AssetKind::Video), Some(video));

        let audio = AssetKey::new(1, 0, AssetKind::Audio);
        assert_eq!(audio.as_render_key(), None);
        assert_eq!(AssetKey::from_render_key(view, AssetKind::Audio), None);
        assert_eq!(AssetKey::NIL.as_render_key(), None);
    }
}
