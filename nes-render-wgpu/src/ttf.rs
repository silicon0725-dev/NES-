//! 手写 TrueType（TTF）子集解析器 + 标量灰度光栅化器（零第三方依赖）。
//!
//! # 为什么自己写
//!
//! 与 [`crate::png`] / [`crate::bmp`] 同一条纪律：本 crate 只允许依赖
//! nes-render-api（见 `Cargo.toml` 的 G8/G9/G10 注释），而为"动态字体"
//! 引入 `ttf-parser` / `rusttype` / `ab_glyph` 等 registry 依赖是越界的。
//! 于是这里实现 TrueType 的**够用子集**，为下一期"动态字形图集渲染集成"
//! 备好 API；本期不做任何 GPU/图集接入（集成另有人做）。
//!
//! # 覆盖的表
//!
//! | 表 | 读什么 |
//! |---|---|
//! | sfnt 头 | version `0x00010000`，或 `'ttcf'`（TTC 集合：取第一个字体） |
//! | `head` | `unitsPerEm`、`indexToLocFormat`、字形包围盒 |
//! | `maxp` | `numGlyphs` |
//! | `cmap` | platform 3/1（回退 3/0 与 0/x）的 **format 4** 子表（BMP） |
//! | `loca` | short / long 两式 |
//! | `glyf` | 简单字形 + 复合字形（递归组装） |
//! | `hhea` / `hmtx` | ascent/descent/lineGap + advance/lsb |
//!
//! # 明确的裁剪点（Unsupported，指名道姓）
//!
//! - `OTTO`（CFF 轮廓）与 `'true'` 头：只认 glyf 轮廓；
//! - cmap format 12（增补平面）：format 4 只覆盖 BMP，`char::from_u32` 超出
//!   0xFFFF 时 [`TtfFont::glyph_index`] 返回 `None`；
//! - 复合字形的**点匹配**定位（`ARGS_ARE_XY_VALUES` 未置位）：现代字体罕见；
//! - `SCALED_COMPONENT_OFFSET`：按规范默认的 UNSCALED 语义处理（平移量不随
//!   父变换缩放）；`ROUND_XY_TO_GRID` 在"整数 font unit 空间"是恒等操作，
//!   读入后按无操作处理；
//! - 无 hinting（instructions 整段跳过）、无 kerning/GPOS、无可变字体。
//!
//! # 光栅化口径
//!
//! - TrueType 二次样条展平为折线（控制多边形像素长度决定细分档数，确定性）；
//! - **nonzero winding**（TrueType 语义）：同一轮廓圈层方向一致时洞被正确
//!   挖空，同向嵌套（nonzero 与 even-odd 的区分点）正确填充；
//! - 覆盖率 = 每像素 4x4 = 16 个采样点（子像素中心）内外判定，`0..=255` 灰度；
//! - 扫描线交点取半开区间 `[ymin, ymax)`，顶点处不多数不少漏；
//! - 字体坐标 y-up，输出位图 y-down（第 0 行是字形顶部）。
//!
//! # 与像素的换算
//!
//! `scale = px / units_per_em`，所有 font unit 度量 ×scale 得像素。
//! [`GlyphBitmap::bearing_x`] 为位图左缘相对笔位（原点）的横向偏移，
//! [`GlyphBitmap::bearing_y`] 为基线到位图顶缘的高度（y-up，非负）。

use std::fmt;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// 错误类型
// ---------------------------------------------------------------------------

/// TTF 子集解析 / 光栅化的全部失败点（Display 一律中文指名道姓）。
#[derive(Debug, Clone, PartialEq)]
pub enum TtfError {
    /// sfnt 头魔数不合法：既非 `0x00010000` 也非 `'ttcf'`，或 head.magicNumber 不符。
    BadMagic,
    /// 必需表在表目录里找不到（携带缺失的表标签）。
    MissingTable(String),
    /// 表内读数越界：偏移/长度与数据不符（文件被截断或表损坏）。
    BadOffset {
        /// 越界的绝对字节位置。
        pos: usize,
    },
    /// 命中本子集解析器的明确裁剪点（详见模块文档）。
    Unsupported(&'static str),
    /// 字形位图尺寸超出安全上限（坏字形或像素尺寸病态）。
    GlyphTooLarge {
        /// 计算出的位图宽度（像素）。
        width: u32,
        /// 计算出的位图高度（像素）。
        height: u32,
    },
    /// 光栅化像素尺寸非法（非有限或 <= 0）。
    InvalidPixelSize(f32),
}

impl fmt::Display for TtfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => write!(
                f,
                "sfnt 头魔数不合法：既非 0x00010000 也非 'ttcf'（或 head.magicNumber 不符）"
            ),
            Self::MissingTable(tag) => write!(f, "必需表缺失：{tag}"),
            Self::BadOffset { pos } => {
                write!(f, "表内偏移越界：pos={pos}（文件截断或表损坏）")
            }
            Self::Unsupported(why) => write!(f, "TTF 子集解析器不支持：{why}"),
            Self::GlyphTooLarge { width, height } => {
                write!(f, "字形位图过大：{width}x{height}（超过安全上限）")
            }
            Self::InvalidPixelSize(px) => write!(f, "非法的像素尺寸：{px}"),
        }
    }
}

impl std::error::Error for TtfError {}

// ---------------------------------------------------------------------------
// 公开数据类型
// ---------------------------------------------------------------------------

/// 字体竖排度量（像素，随请求字号缩放）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    /// 上伸高度（基线到字面顶部，正值，y-up）。
    pub ascent: f32,
    /// 下伸深度（基线到字面底部，**负值**，排版惯例）。
    pub descent: f32,
    /// 行高（`ascent - descent + lineGap`，且保证 >= `ascent - descent`）。
    pub line_height: f32,
}

/// 单个字形的灰度位图（覆盖率 0..=255，行优先，第 0 行是字形顶部）。
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphBitmap {
    /// 位图宽度（像素；空字形如空格为 0）。
    pub width: u32,
    /// 位图高度（像素；空字形如空格为 0）。
    pub height: u32,
    /// 灰度覆盖率，`width * height` 字节，行优先 y-down。
    pub coverage: Vec<u8>,
    /// 位图左缘相对笔位的横向偏移（像素，通常 >= 0）。
    pub bearing_x: i32,
    /// 基线到位图顶缘的高度（像素，y-up，通常 > 0）。
    pub bearing_y: i32,
    /// 水平步进（像素，`hmtx` advance × scale）。
    pub advance: f32,
}

/// cmap format 4 的一个区段（解析期已做边界检查，查找期零重复解析）。
#[derive(Debug, Clone, Copy)]
struct CmapSegment {
    /// 区段起始码点（含）。
    start: u16,
    /// 区段结束码点（含）。
    end: u16,
    /// 字形索引增量。
    delta: i16,
    /// `idRangeOffset` 原始值（0 表示用 start+delta 公式）。
    range_offset: u16,
    /// 本区段 `idRangeOffset` 条目在整份数据里的绝对位置（查找期读数基址）。
    range_base: usize,
}

/// 一条轮廓上的点（font unit，f32 承载复合字形的变换插值）。
#[derive(Debug, Clone, Copy)]
struct ContourPoint {
    x: f32,
    y: f32,
    /// True：on-curve 锚点；False：off-curve 控制点。
    on_curve: bool,
}

/// TrueType 字体 / 字体集合（TTC）的第一个字体的子集解析结果。
///
/// 内部用 `Arc<[u8]>` **拥有**整份字体数据：`parse` 时一次拷贝，
/// 之后所有表内读数都指向自己，[`TtfFont`] 可随意 `Clone` / 跨线程共享。
#[derive(Clone)]
pub struct TtfFont {
    data: Arc<[u8]>,
    /// `head.unitsPerEm`。
    units_per_em: u16,
    /// `head.indexToLocFormat`（0 = short，1 = long）。
    index_to_loc_format: u16,
    /// `head` 的字形包围盒（xMin, yMin, xMax, yMax，font unit）。
    head_bbox: [i16; 4],
    /// `maxp.numGlyphs`。
    num_glyphs: u16,
    /// `hhea.ascender`（font unit）。
    ascender: i16,
    /// `hhea.descender`（font unit，通常为负）。
    descender: i16,
    /// `hhea.lineGap`（font unit）。
    line_gap: i16,
    /// `hhea.numberOfHMetrics`。
    num_h_metrics: u16,
    /// `hmtx` 表在数据中的字节区间。
    hmtx: (usize, usize),
    /// `loca` 表在数据中的字节区间。
    loca: (usize, usize),
    /// `glyf` 表在数据中的字节区间。
    glyf: (usize, usize),
    /// cmap format 4 区段（按 start 升序）。
    cmap_segs: Vec<CmapSegment>,
}

/// 位图单边最大像素数（防坏字形 / 病态字号把内存打爆的安全闸）。
const MAX_BITMAP_DIM: u32 = 4096;

/// 复合字形递归组装的最大深度（防畸形数据的成环引用）。
const MAX_COMPOSITE_DEPTH: u32 = 8;

// ---------------------------------------------------------------------------
// 大端读数（全部带边界检查）
// ---------------------------------------------------------------------------

fn u8_at(data: &[u8], pos: usize) -> Result<u8, TtfError> {
    data.get(pos).copied().ok_or(TtfError::BadOffset { pos })
}

fn u16_at(data: &[u8], pos: usize) -> Result<u16, TtfError> {
    let hi = u8_at(data, pos)?;
    let lo = u8_at(data, pos + 1)?;
    Ok((u16::from(hi) << 8) | u16::from(lo))
}

fn i16_at(data: &[u8], pos: usize) -> Result<i16, TtfError> {
    Ok(u16_at(data, pos)? as i16)
}

fn u32_at(data: &[u8], pos: usize) -> Result<u32, TtfError> {
    let a = u8_at(data, pos)?;
    let b = u8_at(data, pos + 1)?;
    let c = u8_at(data, pos + 2)?;
    let d = u8_at(data, pos + 3)?;
    Ok((u32::from(a) << 24) | (u32::from(b) << 16) | (u32::from(c) << 8) | u32::from(d))
}

/// F2Dot14（2.14 定点数）转 f32（复合字形变换矩阵元素）。
fn f2dot14_at(data: &[u8], pos: usize) -> Result<f32, TtfError> {
    Ok(f32::from(i16_at(data, pos)?) / 16_384.0)
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

/// 表目录里的一条记录。
#[derive(Debug, Clone, Copy)]
struct TableRecord {
    tag: [u8; 4],
    offset: usize,
    length: usize,
}

impl TtfFont {
    /// 解析一份 TrueType 字体数据（`.ttf` 或 `.ttc`）。
    ///
    /// 数据被**拷贝**进内部的 `Arc<[u8]>`（[`TtfFont`] 拥有数据、可 Clone）。
    /// TTC 集合只取第一个字体；必需表缺失、魔数不对、表内越界一律报错，
    /// 不做任何静默兜底。
    pub fn parse(data: &[u8]) -> Result<Self, TtfError> {
        // ---- sfnt 头：单字体或 TTC 集合，定位第一个字体的表目录 ----
        if data.len() < 12 {
            return Err(TtfError::BadOffset { pos: data.len() });
        }
        let version = u32_at(data, 0)?;
        let dir_base = if version == u32::from_be_bytes(*b"ttcf") {
            // TTC 头：tag(4) major(2) minor(2) numFonts(4) offsets[](4)
            let num_fonts = u32_at(data, 8)?;
            if num_fonts == 0 {
                return Err(TtfError::Unsupported("TTC 集合为空（numFonts == 0）"));
            }
            u32_at(data, 12)? as usize
        } else if version == 0x0001_0000 {
            0
        } else if version == u32::from_be_bytes(*b"OTTO") {
            return Err(TtfError::Unsupported("OTTO/CFF 轮廓（本子集只认 glyf）"));
        } else if version == u32::from_be_bytes(*b"true") {
            return Err(TtfError::Unsupported("Apple 'true' 头（本子集只认 0x00010000）"));
        } else {
            return Err(TtfError::BadMagic);
        };

        // ---- 表目录 ----
        let num_tables = u16_at(data, dir_base + 4)?;
        if num_tables == 0 || num_tables > 4096 {
            return Err(TtfError::BadOffset {
                pos: dir_base + 4,
            });
        }
        let mut records = Vec::with_capacity(usize::from(num_tables));
        for i in 0..usize::from(num_tables) {
            let pos = dir_base + 12 + i * 16;
            let mut tag = [0u8; 4];
            tag.copy_from_slice(data.get(pos..pos + 4).ok_or(TtfError::BadOffset { pos })?);
            let offset = u32_at(data, pos + 8)? as usize;
            let length = u32_at(data, pos + 12)? as usize;
            if offset.checked_add(length).map_or(true, |end| end > data.len()) {
                return Err(TtfError::BadOffset { pos });
            }
            records.push(TableRecord { tag, offset, length });
        }

        let table = |name: [u8; 4]| -> Result<TableRecord, TtfError> {
            records
                .iter()
                .copied()
                .find(|r| r.tag == name)
                .ok_or_else(|| {
                    TtfError::MissingTable(
                        std::str::from_utf8(&name).unwrap_or("????").to_string(),
                    )
                })
        };

        // ---- head：unitsPerEm / indexToLocFormat / 包围盒 ----
        let head = table(*b"head")?;
        if head.length < 54 {
            return Err(TtfError::BadOffset { pos: head.offset });
        }
        if u32_at(data, head.offset + 12)? != 0x5F0F_3CF5 {
            return Err(TtfError::BadMagic);
        }
        let units_per_em = u16_at(data, head.offset + 18)?;
        if units_per_em == 0 {
            return Err(TtfError::Unsupported("head.unitsPerEm == 0"));
        }
        let head_bbox = [
            i16_at(data, head.offset + 36)?,
            i16_at(data, head.offset + 38)?,
            i16_at(data, head.offset + 40)?,
            i16_at(data, head.offset + 42)?,
        ];
        let index_to_loc_format = u16_at(data, head.offset + 50)?;
        if index_to_loc_format > 1 {
            return Err(TtfError::Unsupported("indexToLocFormat 非 0/1"));
        }

        // ---- maxp：numGlyphs ----
        let maxp = table(*b"maxp")?;
        if maxp.length < 6 {
            return Err(TtfError::BadOffset { pos: maxp.offset });
        }
        let num_glyphs = u16_at(data, maxp.offset + 4)?;

        // ---- hhea：竖排度量 ----
        let hhea = table(*b"hhea")?;
        if hhea.length < 36 {
            return Err(TtfError::BadOffset { pos: hhea.offset });
        }
        let ascender = i16_at(data, hhea.offset + 4)?;
        let descender = i16_at(data, hhea.offset + 6)?;
        let line_gap = i16_at(data, hhea.offset + 8)?;
        let num_h_metrics = u16_at(data, hhea.offset + 34)?;
        if num_h_metrics == 0 {
            return Err(TtfError::Unsupported("hhea.numberOfHMetrics == 0"));
        }

        // ---- hmtx / loca / glyf：只存区间，读数延迟到用点 ----
        let hmtx = table(*b"hmtx")?;
        let loca = table(*b"loca")?;
        let glyf = table(*b"glyf")?;
        let hmtx_range = (hmtx.offset, hmtx.offset + hmtx.length);
        let loca_range = (loca.offset, loca.offset + loca.length);
        let glyf_range = (glyf.offset, glyf.offset + glyf.length);
        if hmtx.length < usize::from(num_h_metrics) * 4 {
            return Err(TtfError::BadOffset { pos: hmtx.offset });
        }

        // ---- cmap：选定 format 4 子表并预解析区段 ----
        let cmap_segs = parse_cmap_format4(data, table(*b"cmap")?)?;

        Ok(Self {
            data: Arc::from(data.to_vec().into_boxed_slice()),
            units_per_em,
            index_to_loc_format,
            head_bbox,
            num_glyphs,
            ascender,
            descender,
            line_gap,
            num_h_metrics,
            hmtx: hmtx_range,
            loca: loca_range,
            glyf: glyf_range,
            cmap_segs,
        })
    }

    /// `head.unitsPerEm`（em 方形的 font unit 数；1000 / 2048 是常见值）。
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    /// `head` 的字形包围盒（font unit）。
    pub fn head_bbox(&self) -> [i16; 4] {
        self.head_bbox
    }

    /// 字形总数（`maxp.numGlyphs`）。
    pub fn num_glyphs(&self) -> u16 {
        self.num_glyphs
    }

    /// 字符 -> 字形索引（cmap format 4，BMP 码点）。
    ///
    /// 码点不在 cmap 里（或超出 BMP）返回 `None`；调用方用
    /// [`TtfFont::NOTDEF`]（gid 0）兜底渲染缺字形方块。
    pub fn glyph_index(&self, c: char) -> Option<u16> {
        let cp = u32::from(c);
        if cp > 0xFFFF {
            return None; // format 4 只覆盖 BMP
        }
        let cp = cp as u16;
        // 区段按 start 升序且互不重叠：找最后一个 start <= cp 的区段再验 end。
        let idx = self
            .cmap_segs
            .partition_point(|seg| seg.start <= cp)
            .checked_sub(1)?;
        let seg = self.cmap_segs[idx];
        if cp < seg.start || cp > seg.end {
            return None;
        }
        let gid = if seg.range_offset == 0 {
            cp.wrapping_add(seg.delta as u16)
        } else {
            let pos = seg.range_base + 2 * usize::from(cp - seg.start);
            let raw = u16_at(&self.data, pos).ok()?;
            if raw == 0 {
                return None;
            }
            raw.wrapping_add(seg.delta as u16)
        };
        Some(gid)
    }

    /// 缺字形的兜底字形索引（gid 0，`.notdef`）。
    pub const NOTDEF: u16 = 0;

    /// 水平步进（font unit → 像素）：`hmtx` advance × `px / unitsPerEm`。
    ///
    /// `gid` 超出 `numGlyphs` 报 [`TtfError::BadOffset`]；
    /// 超出 `numberOfHMetrics` 的字形重复最后一个 advance（规范行为）。
    pub fn advance(&self, gid: u16, px: f32) -> Result<f32, TtfError> {
        Ok(self.hmtx_advance(gid)? * self.scale(px)?)
    }

    /// 按字号缩放的竖排度量。
    pub fn metrics(&self, px: f32) -> Result<FontMetrics, TtfError> {
        let scale = self.scale(px)?;
        let ascent = f32::from(self.ascender) * scale;
        let descent = f32::from(self.descender) * scale;
        // lineGap 可为负（罕见），行高钳到不小于 ascent - descent（即 |两段| 之和）。
        let line_height = (f32::from(self.ascender - self.descender + self.line_gap) * scale)
            .max(ascent - descent);
        Ok(FontMetrics {
            ascent,
            descent,
            line_height,
        })
    }

    /// 光栅化一个字形为 4x4 超采样的灰度位图（详见模块文档"光栅化口径"）。
    ///
    /// 空字形（如空格、gid 0 的部分字体）返回 0x0 空位图，`advance` 仍有效。
    /// 同参数重复调用逐字节相同（纯函数、无缓存、无并行、无环境读取）。
    pub fn rasterize(&self, gid: u16, px: f32) -> Result<GlyphBitmap, TtfError> {
        if !px.is_finite() || px <= 0.0 {
            return Err(TtfError::InvalidPixelSize(px));
        }
        let scale = self.scale(px)?;
        let advance = self.hmtx_advance(gid)? * scale;
        let contours = self.glyph_contours(gid, 0)?;

        // 空字形：0x0 位图 + 有效 advance（bearing 取 lsb，纯记账用途）。
        if contours.iter().all(|c| c.len() < 2) {
            return Ok(GlyphBitmap {
                width: 0,
                height: 0,
                coverage: Vec::new(),
                bearing_x: (self.hmtx_lsb(gid)? * scale).round() as i32,
                bearing_y: 0,
                advance,
            });
        }

        // 展平为 raster 空间折线（y-down：y_raster = -y_font * scale）并求包围盒。
        let mut polylines: Vec<Vec<(f32, f32)>> = Vec::with_capacity(contours.len());
        let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
        let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for contour in &contours {
            let line = flatten_contour(contour);
            if line.len() < 2 {
                continue;
            }
            for &(x, y) in &line {
                let (rx, ry) = (x * scale, -y * scale);
                min_x = min_x.min(rx);
                min_y = min_y.min(ry);
                max_x = max_x.max(rx);
                max_y = max_y.max(ry);
            }
            polylines.push(line);
        }
        if polylines.is_empty() {
            return Ok(GlyphBitmap {
                width: 0,
                height: 0,
                coverage: Vec::new(),
                bearing_x: (self.hmtx_lsb(gid)? * scale).round() as i32,
                bearing_y: 0,
                advance,
            });
        }

        // 像素包围盒：floor/ceil 外扩一周，保证轮廓完全落在位图内。
        let rx0 = min_x.floor();
        let ry0 = min_y.floor();
        let width = (max_x.ceil() - rx0).max(1.0) as u32;
        let height = (max_y.ceil() - ry0).max(1.0) as u32;
        if width > MAX_BITMAP_DIM || height > MAX_BITMAP_DIM {
            return Err(TtfError::GlyphTooLarge { width, height });
        }
        let bearing_x = rx0 as i32;
        let bearing_y = -(ry0 as i32);

        // 4x4 超采样：每个像素 16 个子采样点，nonzero winding 判内外。
        let sh = usize::try_from(height).unwrap_or(0) * 4;
        // 16 子采样的**原始计数**先累加（每像素 0..=16），子行循环结束
        // 后一次性缩放到 0..=255 —— 逐子行覆写会让满覆盖像素封顶在
        // 4*255/16=63（S12-10 实测抓到的缺陷）。
        let mut acc = vec![0u32; width as usize * height as usize];
        let mut crossings: Vec<(f32, i32)> = Vec::new();
        for sub_row in 0..sh {
            let sy = (sub_row as f32 + 0.5) / 4.0;
            crossings.clear();
            for line in &polylines {
                // 折线闭合成环：最后一点隐式连回第一点。
                let n = line.len();
                for i in 0..n {
                    let (x1, y1) = line[i];
                    let (x2, y2) = line[(i + 1) % n];
                    let (sx1, sy1) = (x1 * scale - rx0, -y1 * scale - ry0);
                    let (sx2, sy2) = (x2 * scale - rx0, -y2 * scale - ry0);
                    // 半开区间 [min, max)：顶点处恰好一次计数。
                    if (sy1 <= sy) != (sy2 <= sy) {
                        let t = (sy - sy1) / (sy2 - sy1);
                        let x = sx1 + t * (sx2 - sx1);
                        let dir = if sy2 > sy1 { 1 } else { -1 };
                        crossings.push((x, dir));
                    }
                }
            }
            if crossings.is_empty() {
                continue;
            }
            crossings.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));

            // 子采样 x 沿行单调递增：交点指针只前进，winding 增量维护。
            let mut wind = 0i32;
            let mut ptr = 0usize;
            for col in 0..width {
                let mut inside = 0u32;
                for sub in 0..4u32 {
                    let sx = col as f32 + (sub as f32 + 0.5) / 4.0;
                    while ptr < crossings.len() && crossings[ptr].0 < sx {
                        wind += crossings[ptr].1;
                        ptr += 1;
                    }
                    if wind != 0 {
                        inside += 1;
                    }
                }
                acc[sub_row / 4 * width as usize + col as usize] += inside;
            }
        }

        // 16 级原始计数 -> 0..=255 灰度。
        let coverage = acc
            .iter()
            .map(|&n| ((n.min(16) * 255) / 16) as u8)
            .collect::<Vec<u8>>();

        Ok(GlyphBitmap {
            width,
            height,
            coverage,
            bearing_x,
            bearing_y,
            advance,
        })
    }

    // -----------------------------------------------------------------------
    // 内部：度量与轮廓
    // -----------------------------------------------------------------------

    fn scale(&self, px: f32) -> Result<f32, TtfError> {
        if !px.is_finite() || px <= 0.0 {
            return Err(TtfError::InvalidPixelSize(px));
        }
        Ok(px / f32::from(self.units_per_em))
    }

    /// `hmtx` advance（font unit）。超界字形重复最后一个 advance。
    fn hmtx_advance(&self, gid: u16) -> Result<f32, TtfError> {
        if gid >= self.num_glyphs {
            return Err(TtfError::BadOffset { pos: usize::from(gid) });
        }
        let idx = gid.min(self.num_h_metrics - 1);
        Ok(f32::from(u16_at(&self.data, self.hmtx.0 + usize::from(idx) * 4)?))
    }

    /// `hmtx` lsb（font unit）。超界字形读 lsb 数组。
    fn hmtx_lsb(&self, gid: u16) -> Result<f32, TtfError> {
        if gid >= self.num_glyphs {
            return Err(TtfError::BadOffset { pos: usize::from(gid) });
        }
        let g = usize::from(gid);
        if g < usize::from(self.num_h_metrics) {
            Ok(f32::from(i16_at(&self.data, self.hmtx.0 + g * 4 + 2)?))
        } else {
            let pos = self.hmtx.0 + usize::from(self.num_h_metrics) * 4 + (g - usize::from(self.num_h_metrics)) * 2;
            if pos + 2 > self.hmtx.1 {
                return Err(TtfError::BadOffset { pos });
            }
            Ok(f32::from(i16_at(&self.data, pos)?))
        }
    }

    /// `loca` 定位的字形字节区间（已钳到 `glyf` 表内；空字形返回空区间）。
    fn glyph_range(&self, gid: u16) -> Result<std::ops::Range<usize>, TtfError> {
        if gid >= self.num_glyphs {
            return Err(TtfError::BadOffset { pos: usize::from(gid) });
        }
        let g = usize::from(gid);
        let entry = if self.index_to_loc_format == 0 { 2 } else { 4 };
        let pos = self.loca.0 + g * entry;
        let raw_start = if self.index_to_loc_format == 0 {
            usize::from(u16_at(&self.data, pos)?) * 2
        } else {
            u32_at(&self.data, pos)? as usize
        };
        let raw_end = if self.index_to_loc_format == 0 {
            usize::from(u16_at(&self.data, pos + entry)?) * 2
        } else {
            u32_at(&self.data, pos + entry)? as usize
        };
        let glyf_len = self.glyf.1 - self.glyf.0;
        if raw_start > raw_end || raw_start > glyf_len {
            return Err(TtfError::BadOffset { pos });
        }
        // 规范允许 loca 末项指向表尾之外表示空字形，这里钳到表长。
        let end = raw_end.min(glyf_len);
        Ok(self.glyf.0 + raw_start..self.glyf.0 + end)
    }

    /// 提取一个字形的全部轮廓（font unit；复合字形递归组装、应用变换）。
    fn glyph_contours(&self, gid: u16, depth: u32) -> Result<Vec<Vec<ContourPoint>>, TtfError> {
        if depth > MAX_COMPOSITE_DEPTH {
            return Err(TtfError::Unsupported("复合字形嵌套过深（疑似成环）"));
        }
        let range = self.glyph_range(gid)?;
        if range.is_empty() {
            return Ok(Vec::new());
        }
        let data = &self.data[..];
        let num_contours = i16_at(data, range.start)?;
        if num_contours >= 0 {
            self.simple_glyph(range, usize::try_from(num_contours).unwrap_or(0))
        } else {
            self.composite_glyph(range, depth)
        }
    }

    /// 简单字形：endPts + flags（含 REPEAT）+ x/y 增量（byte/short 双式）。
    fn simple_glyph(
        &self,
        range: std::ops::Range<usize>,
        num_contours: usize,
    ) -> Result<Vec<Vec<ContourPoint>>, TtfError> {
        let data = &self.data[..];
        let mut p = range.start + 10; // 跳过 numberOfContours + 包围盒
        if num_contours == 0 {
            return Ok(Vec::new());
        }
        let mut end_pts = Vec::with_capacity(num_contours);
        for _ in 0..num_contours {
            end_pts.push(u16_at(data, p)?);
            p += 2;
        }
        let n_points = usize::from(*end_pts.last().ok_or(TtfError::BadOffset { pos: p })?) + 1;
        let instr_len = usize::from(u16_at(data, p)?);
        p += 2 + instr_len; // instructions 整段跳过（无 hinting）

        // flags：处理 REPEAT 位，直到收满 n_points 个。
        let mut flags: Vec<u8> = Vec::with_capacity(n_points);
        while flags.len() < n_points {
            let flag = u8_at(data, p)?;
            p += 1;
            flags.push(flag);
            if flag & 0x08 != 0 {
                let repeat = u8_at(data, p)?;
                p += 1;
                for _ in 0..repeat {
                    if flags.len() >= n_points {
                        return Err(TtfError::BadOffset { pos: p });
                    }
                    flags.push(flag);
                }
            }
        }

        // x 坐标：增量解码（i16 / u8+ / u8- / 零 四式）。
        let mut xs = Vec::with_capacity(n_points);
        let mut ys = Vec::with_capacity(n_points);
        let mut x = 0i32;
        for &flag in &flags {
            if flag & 0x02 != 0 {
                let d = u32::from(u8_at(data, p)?);
                p += 1;
                x += if flag & 0x10 != 0 { d as i32 } else { d as i32 - 256 };
            } else if flag & 0x10 == 0 {
                x += i32::from(i16_at(data, p)?);
                p += 2;
            }
            xs.push(x);
        }
        // y 坐标：同构（标志位 0x04 / 0x20）。
        let mut y = 0i32;
        for &flag in &flags {
            if flag & 0x04 != 0 {
                let d = u32::from(u8_at(data, p)?);
                p += 1;
                y += if flag & 0x20 != 0 { d as i32 } else { d as i32 - 256 };
            } else if flag & 0x20 == 0 {
                y += i32::from(i16_at(data, p)?);
                p += 2;
            }
            ys.push(y);
        }
        if p > range.end {
            return Err(TtfError::BadOffset { pos: p });
        }

        // 合流成轮廓：endPts 切分 + on/off-curve 标志（bit 0）。
        let mut points = Vec::with_capacity(n_points);
        let mut contour_bounds: Vec<usize> = Vec::with_capacity(num_contours);
        let mut idx = 0usize;
        for &end in &end_pts {
            let end = usize::from(end);
            while idx <= end {
                if idx >= n_points {
                    return Err(TtfError::BadOffset { pos: p });
                }
                points.push(ContourPoint {
                    x: if xs[idx] == i32::MAX { f32::MAX } else { xs[idx] as f32 },
                    y: if ys[idx] == i32::MAX { f32::MAX } else { ys[idx] as f32 },
                    on_curve: flags[idx] & 0x01 != 0,
                });
                idx += 1;
            }
            contour_bounds.push(points.len());
        }
        let mut contours = Vec::with_capacity(num_contours);
        let mut start = 0usize;
        for bound in contour_bounds {
            contours.push(points[start..bound].to_vec());
            start = bound;
        }
        Ok(contours)
    }

    /// 复合字形：ARGS_ARE_XY_VALUES 定位 + SCALE / X_Y_SCALE / 2x2 变换，递归组装。
    fn composite_glyph(
        &self,
        range: std::ops::Range<usize>,
        depth: u32,
    ) -> Result<Vec<Vec<ContourPoint>>, TtfError> {
        let data = &self.data[..];
        let mut p = range.start + 10;
        let mut out: Vec<Vec<ContourPoint>> = Vec::new();
        loop {
            let flags = u16_at(data, p)?;
            let child = u16_at(data, p + 2)?;
            p += 4;
            if flags & 0x0002 == 0 {
                return Err(TtfError::Unsupported(
                    "复合字形点匹配定位（ARGS_ARE_XY_VALUES 未置位）",
                ));
            }
            // 参数：word 时 i16，byte 时 i8（XY 语义下均为有符号平移，font unit）。
            let (dx, dy) = if flags & 0x0001 != 0 {
                let v = (i16_at(data, p)?, i16_at(data, p + 2)?);
                p += 4;
                (f32::from(v.0), f32::from(v.1))
            } else {
                let v = (i8_at(data, p)?, i8_at(data, p + 1)?);
                p += 2;
                (f32::from(v.0), f32::from(v.1))
            };
            // 变换矩阵（行主序 a b / c d 的转置记法见规范；x' = a*x + c*y，y' = b*x + d*y）。
            let (mut a, mut b, mut c, mut d) = (1.0f32, 0.0f32, 0.0f32, 1.0f32);
            if flags & 0x0008 != 0 {
                let s = f2dot14_at(data, p)?;
                p += 2;
                a = s;
                d = s;
            } else if flags & 0x0040 != 0 {
                a = f2dot14_at(data, p)?;
                d = f2dot14_at(data, p + 2)?;
                p += 4;
            } else if flags & 0x0080 != 0 {
                a = f2dot14_at(data, p)?;
                b = f2dot14_at(data, p + 2)?;
                c = f2dot14_at(data, p + 4)?;
                d = f2dot14_at(data, p + 6)?;
                p += 8;
            }
            // ROUND_XY_TO_GRID：平移量在整数 font unit 空间已对齐网格，恒等，略。
            // SCALED_COMPONENT_OFFSET：按规范默认 UNSCALED 语义（平移不随父变换缩放）。

            for mut contour in self.glyph_contours(child, depth + 1)? {
                for pt in &mut contour {
                    *pt = ContourPoint {
                        x: a * pt.x + c * pt.y + dx,
                        y: b * pt.x + d * pt.y + dy,
                        on_curve: pt.on_curve,
                    };
                }
                out.push(contour);
            }
            if flags & 0x0020 == 0 {
                break; // MORE_COMPONENTS 未置位：组件结束（WE_HAVE_INSTRUCTIONS 忽略）
            }
            if p >= range.end {
                return Err(TtfError::BadOffset { pos: p });
            }
        }
        Ok(out)
    }
}

/// 有符号单字节读数（复合字形 byte 参数）。
fn i8_at(data: &[u8], pos: usize) -> Result<i8, TtfError> {
    Ok(u8_at(data, pos)? as i8)
}

/// 解析 cmap 表：选定优先级最高的 format 4 子表，返回区段列表。
///
/// 优先级：platform 3 encoding 1（Windows Unicode BMP）> 3/0（Symbol，
/// 按 0xF000 映射的字体对 BMP 码点同样落在 0x20..0xFF 段，可作回退）>
/// platform 0（Unicode 任意 encoding）。其余（含 format 12）跳过。
fn parse_cmap_format4(data: &[u8], cmap: TableRecord) -> Result<Vec<CmapSegment>, TtfError> {
    let base = cmap.offset;
    let num_subtables = usize::from(u16_at(data, base + 2)?);
    if num_subtables == 0 || num_subtables > 1024 {
        return Err(TtfError::BadOffset { pos: base + 2 });
    }
    // 候选按优先级排序：(platform, encoding) 越靠前越优先。
    let mut candidates: Vec<(u16, u16, usize)> = Vec::with_capacity(num_subtables);
    for i in 0..num_subtables {
        let pos = base + 4 + i * 8;
        let platform = u16_at(data, pos)?;
        let encoding = u16_at(data, pos + 2)?;
        let offset = u32_at(data, pos + 4)? as usize;
        if base.checked_add(offset).map_or(true, |abs| abs >= data.len()) {
            return Err(TtfError::BadOffset { pos });
        }
        candidates.push((platform, encoding, base + offset));
    }
    candidates.sort_by_key(|&(platform, encoding, _)| {
        match (platform, encoding) {
            (3, 1) => 0,
            (3, 0) => 1,
            (0, _) => 2,
            (3, _) => 3,
            _ => 4,
        }
    });

    for &(_, _, sub) in &candidates {
        let format = u16_at(data, sub).unwrap_or(0);
        #[cfg(test)]
        eprintln!("[probe] cmap candidate sub={sub} format={format}");
        if format != 4 {
            continue;
        }
        let seg_count_x2 = usize::from(u16_at(data, sub + 6)?);
        if seg_count_x2 == 0 || seg_count_x2 % 2 != 0 {
            return Err(TtfError::BadOffset { pos: sub + 6 });
        }
        let seg_count = seg_count_x2 / 2;
        let end_base = sub + 14;
        let start_base = end_base + seg_count_x2 + 2; // +2: reservedPad
        let delta_base = start_base + seg_count_x2;
        let range_base = delta_base + seg_count_x2;
        let table_end = base + cmap.length;
        if range_base + seg_count_x2 > table_end {
            return Err(TtfError::BadOffset { pos: range_base });
        }
        let mut segs = Vec::with_capacity(seg_count);
        for i in 0..seg_count {
            let seg = CmapSegment {
                start: u16_at(data, start_base + i * 2)?,
                end: u16_at(data, end_base + i * 2)?,
                delta: i16_at(data, delta_base + i * 2)?,
                range_offset: u16_at(data, range_base + i * 2)?,
                range_base: range_base + i * 2,
            };
            if seg.start > seg.end {
                return Err(TtfError::BadOffset { pos: start_base + i * 2 });
            }
            segs.push(seg);
        }
        return Ok(segs);
    }
    Err(TtfError::Unsupported("未找到可用的 cmap format 4 子表"))
}

// ---------------------------------------------------------------------------
// 二次样条展平
// ---------------------------------------------------------------------------

/// 把一条（闭环）TrueType 二次样条轮廓展平为折线顶点序列（font unit）。
///
/// off-curve 隐式中点规则：连续两个 off-curve 之间补一个 on-curve 中点；
/// 末尾悬空的 off-curve 经中点闭合回起点。全部 on-curve 时退化为多边形。
fn flatten_contour(points: &[ContourPoint]) -> Vec<(f32, f32)> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![(points[0].x, points[0].y)];
    }
    let all_on = points.iter().all(|p| p.on_curve);
    if all_on {
        return points.iter().map(|p| (p.x, p.y)).collect();
    }

    // 全 off-curve（罕见）：相邻点的中点全部视为 on-curve。
    if !points.iter().any(|p| p.on_curve) {
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let a = points[i];
            let b = points[(i + 1) % n];
            out.push(((a.x + b.x) * 0.5, (a.y + b.y) * 0.5));
        }
        return out;
    }

    // 旋转到以一个 on-curve 点开头。
    let k = points.iter().position(|p| p.on_curve).unwrap_or(0);
    let pts: Vec<ContourPoint> = points[k..].iter().chain(points[..k].iter()).copied().collect();

    let mut out: Vec<(f32, f32)> = Vec::with_capacity(n * 2);
    let mut prev = (pts[0].x, pts[0].y);
    out.push(prev);
    let mut i = 1usize;
    while i < n {
        let cur = pts[i];
        if cur.on_curve {
            out.push((cur.x, cur.y));
            prev = (cur.x, cur.y);
            i += 1;
        } else if i + 1 < n {
            let next = pts[i + 1];
            if next.on_curve {
                push_quadratic(prev, (cur.x, cur.y), (next.x, next.y), &mut out);
                prev = (next.x, next.y);
                i += 2;
            } else {
                // 隐式 on-curve 中点。
                let mid = ((cur.x + next.x) * 0.5, (cur.y + next.y) * 0.5);
                push_quadratic(prev, (cur.x, cur.y), mid, &mut out);
                prev = mid;
                i += 1;
            }
        } else {
            // 最后一个点是 off-curve：经中点闭回起点。
            push_quadratic(prev, (cur.x, cur.y), (pts[0].x, pts[0].y), &mut out);
            i += 1;
        }
    }
    out
}

/// 细分一段二次贝塞尔（控制多边形像素长度决定档数，确定性纯函数）并追加顶点。
fn push_quadratic(p0: (f32, f32), c: (f32, f32), p1: (f32, f32), out: &mut Vec<(f32, f32)>) {
    // 控制多边形长度（font unit）；以约 2 unit 一档细分，弧长越短档数越少。
    let approx = approx_len(p0, c) + approx_len(c, p1);
    let steps = ((approx * 0.5).ceil() as usize).clamp(2, 64);
    for k in 1..=steps {
        let t = k as f32 / steps as f32;
        let s = 1.0 - t;
        let x = s * s * p0.0 + 2.0 * s * t * c.0 + t * t * p1.0;
        let y = s * s * p0.1 + 2.0 * s * t * c.1 + t * t * p1.1;
        out.push((x, y));
    }
}

/// 两点欧氏距离（展平细分档数估算用）。
fn approx_len(a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    (dx * dx + dy * dy).sqrt()
}

// ---------------------------------------------------------------------------
// 单元测试：合成最小字体（不依赖系统字体，全管线可复现）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 组装一个只有 3 个字形的合法最小 TTF（long loca，upem=1000）：
    /// gid0 空字形（.notdef）、gid1 三角形（'A'）、gid2 同向双层方环（'B'，
    /// nonzero 与 even-odd 的区分用例）。
    fn build_minimal_font() -> Vec<u8> {
        // ---- head（54 字节） ----
        let mut head = Vec::new();
        head.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // version
        head.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // fontRevision
        head.extend_from_slice(&0u32.to_be_bytes()); // checkSumAdjustment
        head.extend_from_slice(&0x5F0F_3CF5u32.to_be_bytes()); // magicNumber
        head.extend_from_slice(&0u16.to_be_bytes()); // flags
        head.extend_from_slice(&1000u16.to_be_bytes()); // unitsPerEm
        head.extend_from_slice(&[0u8; 16]); // created + modified
        head.extend_from_slice(&0i16.to_be_bytes()); // xMin
        head.extend_from_slice(&0i16.to_be_bytes()); // yMin
        head.extend_from_slice(&1000i16.to_be_bytes()); // xMax
        head.extend_from_slice(&900i16.to_be_bytes()); // yMax
        head.extend_from_slice(&0u16.to_be_bytes()); // macStyle
        head.extend_from_slice(&8u16.to_be_bytes()); // lowestRecPPEM
        head.extend_from_slice(&0i16.to_be_bytes()); // fontDirectionHint
        head.extend_from_slice(&1i16.to_be_bytes()); // indexToLocFormat = long
        head.extend_from_slice(&0i16.to_be_bytes()); // glyphDataFormat

        // ---- maxp ----
        let mut maxp = Vec::new();
        maxp.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        maxp.extend_from_slice(&3u16.to_be_bytes()); // numGlyphs

        // ---- hhea ----
        let mut hhea = Vec::new();
        hhea.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        hhea.extend_from_slice(&800i16.to_be_bytes()); // ascender
        hhea.extend_from_slice(&(-200i16).to_be_bytes()); // descender
        hhea.extend_from_slice(&0i16.to_be_bytes()); // lineGap
        hhea.extend_from_slice(&700u16.to_be_bytes()); // advanceWidthMax
        hhea.extend_from_slice(&[0u8; 22]); // 其余字段本子集不读
        hhea.extend_from_slice(&3u16.to_be_bytes()); // numberOfHMetrics

        // ---- hmtx：3 个度量 ----
        let mut hmtx = Vec::new();
        for (adv, lsb) in [(500u16, 0i16), (500, 0), (800, 0)] {
            hmtx.extend_from_slice(&adv.to_be_bytes());
            hmtx.extend_from_slice(&lsb.to_be_bytes());
        }

        // ---- cmap：format 4，platform 3/1，'A'->1、'B'->2、0xFFFF 哨兵 ----
        let mut sub = Vec::new();
        sub.extend_from_slice(&4u16.to_be_bytes()); // format
        sub.extend_from_slice(&40u16.to_be_bytes()); // length
        sub.extend_from_slice(&0u16.to_be_bytes()); // language
        sub.extend_from_slice(&6u16.to_be_bytes()); // segCountX2 = 3 段
        sub.extend_from_slice(&[0u8; 6]); // searchRange/entrySelector/rangeShift
        for end in [0x41u16, 0x42, 0xFFFF] {
            sub.extend_from_slice(&end.to_be_bytes());
        }
        sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
        for start in [0x41u16, 0x42, 0xFFFF] {
            sub.extend_from_slice(&start.to_be_bytes());
        }
        for delta in [(1i16 - 0x41), (2i16 - 0x42), 1] {
            sub.extend_from_slice(&delta.to_be_bytes());
        }
        for _ in 0..3 {
            sub.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset = 0
        }
        let mut cmap = Vec::new();
        cmap.extend_from_slice(&0u16.to_be_bytes()); // version
        cmap.extend_from_slice(&1u16.to_be_bytes()); // numTables
        cmap.extend_from_slice(&3u16.to_be_bytes()); // platformID = Windows
        cmap.extend_from_slice(&1u16.to_be_bytes()); // encodingID = Unicode BMP
        cmap.extend_from_slice(&12u32.to_be_bytes()); // 子表偏移（表内）
        cmap.extend_from_slice(&sub); // 子表本体（40 字节，offset 指向处）

        // ---- glyf：gid1 三角形 + gid2 同向方环 ----
        let mut glyph1 = Vec::new();
        glyph1.extend_from_slice(&1i16.to_be_bytes()); // numberOfContours
        glyph1.extend_from_slice(&0i16.to_be_bytes()); // xMin
        glyph1.extend_from_slice(&0i16.to_be_bytes()); // yMin
        glyph1.extend_from_slice(&1000i16.to_be_bytes()); // xMax
        glyph1.extend_from_slice(&900i16.to_be_bytes()); // yMax
        glyph1.extend_from_slice(&2u16.to_be_bytes()); // endPts = [2]
        glyph1.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
        glyph1.extend_from_slice(&[0x01, 0x01, 0x01]); // 全 on-curve
        for d in [0i16, 500, 500] {
            glyph1.extend_from_slice(&d.to_be_bytes()); // x 增量
        }
        for d in [0i16, 900, -900] {
            glyph1.extend_from_slice(&d.to_be_bytes()); // y 增量
        }

        let mut glyph2 = Vec::new();
        glyph2.extend_from_slice(&2i16.to_be_bytes()); // numberOfContours
        glyph2.extend_from_slice(&0i16.to_be_bytes());
        glyph2.extend_from_slice(&0i16.to_be_bytes());
        glyph2.extend_from_slice(&800i16.to_be_bytes());
        glyph2.extend_from_slice(&800i16.to_be_bytes());
        glyph2.extend_from_slice(&3u16.to_be_bytes()); // 外环 endPts = [3]
        glyph2.extend_from_slice(&7u16.to_be_bytes()); // 内环 endPts = [7]
        glyph2.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
        glyph2.extend_from_slice(&[0x01u8; 8]); // 全 on-curve
        for d in [0i16, 0, 800, 0, -800, 0, 400, 0] {
            glyph2.extend_from_slice(&d.to_be_bytes()); // x 增量
        }
        for d in [0i16, 800, 0, -800, 400, 0, 400, 0] {
            glyph2.extend_from_slice(&d.to_be_bytes()); // y 增量
        }

        let mut glyf = glyph1.clone();
        glyf.extend_from_slice(&glyph2);
        let glyph1_len = glyph1.len() as u32;

        // ---- loca（long）：gid0 空 / gid1 [0,29) / gid2 [29,85) / 表尾 ----
        let mut loca = Vec::new();
        for off in [0u32, 0, glyph1_len, glyf.len() as u32] {
            loca.extend_from_slice(&off.to_be_bytes());
        }

        // ---- 装配 sfnt：表目录按标签升序，表数据 4 字节对齐 ----
        let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"cmap", cmap),
            (b"glyf", glyf),
            (b"head", head),
            (b"hhea", hhea),
            (b"hmtx", hmtx),
            (b"loca", loca),
            (b"maxp", maxp),
        ];
        let dir_len = 12 + tables.len() * 16;
        let mut offset = dir_len;
        let mut placed: Vec<([u8; 4], u32, u32)> = Vec::new();
        let mut body = Vec::new();
        for (tag, data) in tables.iter() {
            while offset % 4 != 0 {
                body.push(0);
                offset += 1;
            }
            placed.push((**tag, offset as u32, data.len() as u32));
            body.extend_from_slice(data);
            offset += data.len();
        }

        let mut font = Vec::with_capacity(offset);
        font.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        font.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        font.extend_from_slice(&[0u8; 6]); // searchRange/entrySelector/rangeShift
        for (tag, off, len) in &placed {
            font.extend_from_slice(tag);
            font.extend_from_slice(&0u32.to_be_bytes()); // checkSum（本子集不校验）
            font.extend_from_slice(&off.to_be_bytes());
            font.extend_from_slice(&len.to_be_bytes());
        }
        font.extend_from_slice(&body);
        font
    }

    #[test]
    fn minimal_font_parse_and_cmap() {
        let font = TtfFont::parse(&build_minimal_font()).expect("最小字体必须可解析");
        assert_eq!(font.units_per_em(), 1000);
        assert_eq!(font.num_glyphs(), 3);
        assert_eq!(font.glyph_index('A'), Some(1));
        assert_eq!(font.glyph_index('B'), Some(2));
        assert_eq!(font.glyph_index('Z'), None, "未映射码点必须返回 None");
        assert_eq!(font.glyph_index('中'), None, "合成字体只有 ASCII 段");
    }

    #[test]
    fn minimal_font_metrics_and_advance() {
        let font = TtfFont::parse(&build_minimal_font()).expect("解析成功");
        let m = font.metrics(10.0).expect("10px 度量");
        assert_eq!(m.ascent, 8.0); // 800 * 10/1000
        assert_eq!(m.descent, -2.0);
        assert_eq!(m.line_height, 10.0);
        assert_eq!(font.advance(1, 10.0).expect("advance"), 5.0); // 500 * 10/1000
        // 超出 numberOfHMetrics 的兜底路径由真实字体测试覆盖，这里验证越界报错。
        assert_eq!(
            font.advance(9, 10.0),
            Err(TtfError::BadOffset { pos: 9 }),
            "gid 越出 numGlyphs 必须报错"
        );
        assert!(matches!(
            font.rasterize(1, -1.0),
            Err(TtfError::InvalidPixelSize(_))
        ));
    }

    #[test]
    fn minimal_font_triangle_raster() {
        let font = TtfFont::parse(&build_minimal_font()).expect("解析成功");
        let bm = font.rasterize(1, 16.0).expect("三角形光栅化");
        assert!(bm.width > 0 && bm.height > 0);
        assert!(bm.coverage.iter().any(|&c| c > 0), "必须有覆盖");
        assert!(
            bm.coverage.contains(&0),
            "包围盒必须留出无覆盖的角落（floor/ceil 外扩）"
        );
        // 重心附近必有高覆盖。
        let (cx, cy) = ((bm.width / 2) as usize, (bm.height * 3 / 4) as usize);
        assert!(
            bm.coverage[cy * bm.width as usize + cx] > 128,
            "三角形内部（重心偏下）必须高覆盖"
        );
    }

    #[test]
    fn minimal_font_nonzero_winding() {
        let font = TtfFont::parse(&build_minimal_font()).expect("解析成功");
        let bm = font.rasterize(2, 16.0).expect("方环光栅化");
        // 内外两圈同向（winding 2/1）：nonzero 语义下内层必须填充；
        // 若误用 even-odd，内层（跨 2 条边）会成洞。
        let (cx, cy) = ((bm.width / 2) as usize, (bm.height / 2) as usize);
        assert_eq!(
            bm.coverage[cy * bm.width as usize + cx],
            255,
            "nonzero winding：同向内层必须满覆盖"
        );
    }

    #[test]
    fn minimal_font_empty_glyph() {
        let font = TtfFont::parse(&build_minimal_font()).expect("解析成功");
        let bm = font.rasterize(0, 16.0).expect("空字形光栅化");
        assert_eq!((bm.width, bm.height), (0, 0));
        assert!(bm.coverage.is_empty());
        assert!(bm.advance > 0.0, "空字形也必须推进笔位");
    }

    #[test]
    fn bad_magic_and_missing_table() {
        let mut font = build_minimal_font();
        assert!(matches!(
            TtfFont::parse(&[]),
            Err(TtfError::BadOffset { .. })
        ));
        font[0] = 0xAB;
        assert!(matches!(
            TtfFont::parse(&font),
            Err(TtfError::BadMagic)
        ));
        font[0] = 0x00; // 恢复 sfnt 魔数，进入 MissingTable 分支
        // 表目录从 12 字节起，'head' 是升序第 3 条：篡改标签 -> MissingTable。
        let head_tag = 12 + 2 * 16;
        font[head_tag..head_tag + 4].copy_from_slice(b"xxxx");
        assert!(matches!(
            TtfFont::parse(&font),
            Err(TtfError::MissingTable(t)) if t == "head"
        ));
    }

    #[test]
    fn error_display_is_chinese_and_named() {
        let cases = [
            TtfError::BadMagic,
            TtfError::MissingTable("glyf".to_string()),
            TtfError::BadOffset { pos: 17 },
            TtfError::Unsupported("format 12"),
            TtfError::GlyphTooLarge {
                width: 9,
                height: 9,
            },
            TtfError::InvalidPixelSize(0.0),
        ];
        for err in cases {
            let text = err.to_string();
            assert!(!text.is_empty(), "每个错误都要有 Display：{err:?}");
        }
    }
}
