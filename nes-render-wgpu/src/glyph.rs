//! TTF 动态字形图集：贪心 shelf 装箱 + (char, 字号) 进程内缓存（S12-11 第 2 期）。
//!
//! # 职责与边界
//!
//! 本模块只做**纯 CPU 的装箱与缓存簿记**：给一个字形位图（[`crate::ttf::
//! TtfFont::rasterize`] 的产物）在 256x256 的字形页里分配一个矩形，并把灰度
//! 覆盖率转成"白色 x 覆盖 alpha"的 RGBA 像素写进页缓冲。GPU 上传、采样 UV
//! 折算都留在 [`crate::renderer::CommandConsumer`]（走 [`crate::gpu::
//! TextureRegistry`] 的既有注册路径），本模块不碰任何句柄。
//!
//! # 页键命名空间（0x7000_0000 段专用）
//!
//! 字形第 `n` 页的资源键 = [`page_key`]`(n)` =
//! `RenderAssetKey::from_parts(0x7000_0000 | n, 1)`。与现有三类键互不相撞：
//!
//! - 资源类键：`slot` 从资源 arena 的 0 起单调增长（S2 提取层），永远到不了
//!   2^30；`gen` 任意 —— `slot` 段不同即不撞；
//! - 非资源类键：`slot` 带 `0x8000_0000` 命名空间标记（S2 `NON_RESOURCE_KEY_
//!   TAG`，`node_key`），与本段差一整个位；
//! - 后端保留键：默认字体表 `DEFAULT_FONT_KEY = from_parts(u32::MAX, 1)`，
//!   slot 全 1，同样不落在本段。
//!
//! 页号上限 2^28（0x1000_0000）：远超注册表的容量天花板（纹理边长 8192px
//! = 每边 32 瓦片 = 1024 页，见 `TextureRegistry::grow` 的上限报错），页号
//! 永远不会溢出进 `gen` 段或撞上 0x8000_0000 段。`gen` 固定取 1：页内容一旦
//! 写入就不迁移（见下"缓存语义"），不存在换代。
//!
//! # 装箱策略（贪心 shelf）
//!
//! 只在**最后一页**顺序装箱，不回头找旧页空洞：
//!
//! 1. 当前行放不下 → 关闭本行（行高 = 本行出现过的最高字形），另起一行；
//! 2. 竖向也放不下 → 整页作废（已装箱的字形不迁移），开新页；
//! 3. 单边超过 256px 的字形不装箱（返回 `None`，调用方只推笔位不画）——
//!    字号契约 clamp 8..128 后，正常字体不会触到这条。
//!
//! 编辑器场景字形是"先冷后稳"的：头几帧装箱 + 整页上传，稳态零分配零上传。
//!
//! # 缓存语义（进程内不淘汰）
//!
//! 缓存键 `(char, size_px)`，命中直接返回页内矩形与排版度量。**不淘汰**：
//! 编辑器人可输入的字符集有限（CJK 常用数千字 x 少数字号），内存上限
//! = 页数 x 256 KiB，规模可控；淘汰机制（LRU / 页回收）需要处理"GPU 纹理
//! 里还引用着旧页内容"的失效同步，在出现真实需求前不引入。页内容只增不改
//! （已有字形的矩形永不复用），因此同键重复上传整页是安全的覆写。

use nes_render_api::RenderAssetKey;

/// 字形页资源键的命名空间标记（slot 高位段专用，见模块文档的冲突面核查）。
pub(crate) const GLYPH_PAGE_TAG: u32 = 0x7000_0000;

/// 字形页边长（像素）。与 `TextureRegistry::TILE_PX` 同值：一页恰好占注册表
/// 一个瓦片，256 KiB RGBA8，上传的 256 字节行对齐天然满足。
pub(crate) const GLYPH_PAGE_PX: u32 = 256;

/// 字形第 `n` 页的注册表资源键（`gen` 固定 1，见模块文档）。
pub(crate) fn page_key(page: u32) -> RenderAssetKey {
    RenderAssetKey::from_parts(GLYPH_PAGE_TAG | page, 1)
}

/// 一个已装箱字形的完整落位记录（缓存值）。
#[derive(Copy, Clone, Debug)]
pub(crate) struct GlyphSlot {
    /// 所在页号（配 [`page_key`] 查注册表）。
    pub(crate) page: u32,
    /// 页内矩形左上角（像素）。
    pub(crate) x: u32,
    /// 页内矩形左上角（像素）。
    pub(crate) y: u32,
    /// 位图宽（像素；0 = 空字形 / 缺字形，只推笔位不画）。
    pub(crate) w: u32,
    /// 位图高（像素）。
    pub(crate) h: u32,
    /// 位图左缘相对笔位的横向偏移（像素，透传 [`crate::ttf::GlyphBitmap`]）。
    pub(crate) bearing_x: f32,
    /// 基线到位图顶缘的高度（像素，y-up，透传）。
    pub(crate) bearing_y: f32,
    /// 水平步进（像素；缺字形 = `.notdef` 的 advance，见 [`GlyphAtlas::slot`]）。
    pub(crate) advance: f32,
}

/// 一张字形页：RGBA8 像素缓冲 + shelf 装箱游标。
struct ShelfPage {
    /// 页像素（RGBA8，行优先；未写纹素全透明，采样侧按 alpha 丢弃）。
    rgba: Vec<u8>,
    /// 当前行装箱游标（下一字形的 x）。
    pen_x: u32,
    /// 当前行基线 y（下一字形的 y）。
    pen_y: u32,
    /// 当前行高度（本行出现过的最高字形）。
    shelf_h: u32,
}

impl ShelfPage {
    fn new() -> Self {
        Self {
            rgba: vec![0; (GLYPH_PAGE_PX * GLYPH_PAGE_PX * 4) as usize],
            pen_x: 0,
            pen_y: 0,
            shelf_h: 0,
        }
    }
}

/// 字形图集：页序列 + 进程内缓存（语义见模块文档）。
#[derive(Default)]
pub(crate) struct GlyphAtlas {
    pages: Vec<ShelfPage>,
    cache: std::collections::BTreeMap<(char, i32), GlyphSlot>,
}

impl GlyphAtlas {
    /// 缓存查询（命中返回落位记录）。
    pub(crate) fn cached(&self, ch: char, size_px: i32) -> Option<GlyphSlot> {
        self.cache.get(&(ch, size_px)).copied()
    }

    /// 贪心 shelf 装箱：返回 `Some((页号, x, y))`；`None` = 单边超页（调用方
    /// 只推笔位不画，见模块文档"装箱策略"）。
    pub(crate) fn place(&mut self, w: u32, h: u32) -> Option<(u32, u32, u32)> {
        if w == 0 || h == 0 || w > GLYPH_PAGE_PX || h > GLYPH_PAGE_PX {
            return None;
        }
        if self.pages.is_empty() {
            self.pages.push(ShelfPage::new());
        }
        {
            let page = self.pages.last_mut().expect("上面刚保证非空");
            if page.pen_x + w > GLYPH_PAGE_PX {
                // 当前行放不下：关闭本行（行高 = 本行最高字形），另起一行。
                page.pen_y += page.shelf_h;
                page.pen_x = 0;
                page.shelf_h = 0;
            }
            if page.pen_y + h > GLYPH_PAGE_PX {
                // 竖向也满：开新页（本页已装箱字形不迁移）。
                self.pages.push(ShelfPage::new());
            }
        }
        let page_no = self.pages.len() as u32 - 1;
        let page = self.pages.last_mut().expect("上面刚保证非空");
        let (x, y) = (page.pen_x, page.pen_y);
        page.pen_x += w;
        page.shelf_h = page.shelf_h.max(h);
        Some((page_no, x, y))
    }

    /// 把一份灰度覆盖率写进页内矩形（白 RGB x 覆盖 alpha；着色由实例 tint 承担）。
    ///
    /// `coverage` 是 `w * h` 字节的行优先灰度（[`crate::ttf::GlyphBitmap::coverage`]）。
    pub(crate) fn blit(&mut self, page: u32, x: u32, y: u32, w: u32, h: u32, coverage: &[u8]) {
        let page = &mut self.pages[page as usize];
        for row in 0..h {
            let src = (row * w) as usize;
            let dst = ((y + row) * GLYPH_PAGE_PX + x) as usize;
            for col in 0..w {
                let a = coverage[src + col as usize];
                let px = (dst + col as usize) * 4;
                page.rgba[px..px + 4].copy_from_slice(&[255, 255, 255, a]);
            }
        }
    }

    /// 某页的整页 RGBA 像素（注册上传用）。
    pub(crate) fn page_rgba(&self, page: u32) -> &[u8] {
        &self.pages[page as usize].rgba
    }

    /// 缓存写入（装箱成功后调用；键重复 = 覆写，调用方只在未命中时走装箱）。
    pub(crate) fn cache_insert(&mut self, ch: char, size_px: i32, slot: GlyphSlot) {
        self.cache.insert((ch, size_px), slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot_of(page: u32, x: u32, y: u32, w: u32, h: u32) -> GlyphSlot {
        GlyphSlot {
            page,
            x,
            y,
            w,
            h,
            bearing_x: 0.0,
            bearing_y: 0.0,
            advance: 0.0,
        }
    }

    #[test]
    fn shelf_packs_rows_then_new_page() {
        let mut atlas = GlyphAtlas::default();
        // 256 宽的页一行装 16 个 16x16：第 17 个换行，第 257 个开新页。
        for i in 0..260u32 {
            let placed = atlas.place(16, 16).expect("16x16 必然装得下");
            let expect_page = i / 256;
            let idx = i % 256;
            let (row, col) = (idx / 16, idx % 16);
            assert_eq!(placed, (expect_page, col * 16, row * 16), "第 {i} 个落位");
        }
    }

    #[test]
    fn shelf_row_height_is_tallest_glyph() {
        let mut atlas = GlyphAtlas::default();
        // 8 高 + 20 高 + 8 高：第三个应落在 y=20（行高由本行最高的 20 决定）。
        assert_eq!(atlas.place(10, 8), Some((0, 0, 0)));
        assert_eq!(atlas.place(10, 20), Some((0, 10, 0)));
        assert_eq!(atlas.place(10, 8), Some((0, 20, 0)));
    }

    #[test]
    fn oversize_glyph_is_rejected() {
        let mut atlas = GlyphAtlas::default();
        assert_eq!(atlas.place(257, 16), None, "超页宽不装箱");
        assert_eq!(atlas.place(16, 0), None, "空位图不装箱");
        assert!(atlas.place(256, 256).is_some(), "整页大小恰好可装箱");
    }

    #[test]
    fn blit_writes_white_rgb_with_coverage_alpha() {
        let mut atlas = GlyphAtlas::default();
        let (page, x, y) = atlas.place(2, 2).expect("装箱");
        atlas.blit(page, x, y, 2, 2, &[0, 128, 255, 7]);
        let rgba = atlas.page_rgba(page);
        let at = |row: u32, col: u32| {
            let base = (((y + row) * 256 + (x + col)) as usize) * 4;
            [rgba[base], rgba[base + 1], rgba[base + 2], rgba[base + 3]]
        };
        assert_eq!(at(0, 0), [255, 255, 255, 0]);
        assert_eq!(at(0, 1), [255, 255, 255, 128]);
        assert_eq!(at(1, 0), [255, 255, 255, 255]);
        assert_eq!(at(1, 1), [255, 255, 255, 7]);
    }

    #[test]
    fn page_key_namespace_is_disjoint() {
        // 页键落 0x7000_0000 段、gen=1：与默认字体键（u32::MAX）、非资源键
        // （0x8000_0000 段）、资源 arena 键（slot 从 0 增长）位空间互斥。
        let k = page_key(0);
        assert_eq!(k.slot() & GLYPH_PAGE_TAG, GLYPH_PAGE_TAG);
        assert_eq!(k.generation(), 1);
        assert_ne!(k.slot(), u32::MAX);
        assert_eq!(k.slot() & 0x8000_0000, 0);
        // 页号不会吃掉命名空间位（上限 2^28）。
        let max = page_key((1 << 28) - 1);
        assert_eq!(max.slot() & !GLYPH_PAGE_TAG, (1 << 28) - 1);
    }

    #[test]
    fn cache_roundtrip() {
        let mut atlas = GlyphAtlas::default();
        assert!(atlas.cached('A', 16).is_none(), "未写入前必未命中");
        atlas.cache_insert('A', 16, slot_of(0, 3, 5, 9, 12));
        let slot = atlas.cached('A', 16).expect("写入后命中");
        assert_eq!((slot.page, slot.x, slot.y, slot.w, slot.h), (0, 3, 5, 9, 12));
        assert!(atlas.cached('A', 32).is_none(), "字号是键的一部分");
        assert!(atlas.cached('B', 16).is_none(), "字符是键的一部分");
    }
}
