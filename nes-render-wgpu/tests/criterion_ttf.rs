//! T-TTF 契约回归：真字体解析 + 灰度光栅化（S12-10 第 1 期）。
//!
//! 系统字体路径（本机 Windows 自带）：`C:\Windows\Fonts\simhei.ttf`
//!（单 TTF，GBK 覆盖）与 `msyh.ttc`（TTC 集合）。字体缺失时照 GPU
//! 用例惯例 skip——契约在字体在场的机器上验证。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-TTF-01 | simhei.ttf 解析：unitsPerEm 合理、'A'/'中' 都有字形、'Z' 缺映射回 None（SimHei 覆盖 Latin）|
//! | T-TTF-02 | 光栅化：'A' 16px 灰度位图非全零非全满、包围盒留空角；同参数两次逐字节相同（确定性）|
//! | T-TTF-03 | CJK：'中' 24px 非零覆盖、advance > 0；metrics 线性（2x px 行高 ≈ 2x）|
//! | T-TTF-04 | TTC：msyh.ttc 解析成功（第一个字体）、'中' 有字形 |
//! | T-TTF-05 | 比例字宽：'i' advance <= 'W' advance（同字号）|

use nes_render_wgpu::ttf::TtfFont;

fn simhei() -> Option<Vec<u8>> {
    let p = std::path::Path::new("C:/Windows/Fonts/simhei.ttf");
    std::fs::read(p).ok()
}

fn msyh() -> Option<Vec<u8>> {
    let p = std::path::Path::new("C:/Windows/Fonts/msyh.ttc");
    std::fs::read(p).ok()
}

/// T-TTF-01：simhei 解析 + cmap 映射。
#[test]
fn t_ttf_01_simhei_parse_and_cmap() {
    let Some(data) = simhei() else {
        eprintln!("[skip] C:/Windows/Fonts/simhei.ttf not found");
        return;
    };
    let font = TtfFont::parse(&data).expect("simhei.ttf 必须可解析");
    let upem = font.units_per_em();
    assert!(
        (16..=16384).contains(&upem),
        "unitsPerEm 合理值域（SimHei 实测 256，老 GB 字体常见）：{upem}"
    );
    assert!(font.glyph_index('A').is_some(), "Latin 'A' 有字形");
    assert!(font.glyph_index('Z').is_some(), "Latin 'Z' 有字形");
    let zh = font.glyph_index('中');
    assert!(zh.is_some(), "CJK '中' 有字形（GBK 覆盖）");
    assert_ne!(zh, Some(0), "'中' 不是 .notdef");
}

/// T-TTF-02：'A' 16px 光栅化——灰度形态 + 确定性。
#[test]
fn t_ttf_02_rasterize_deterministic() {
    let Some(data) = simhei() else {
        eprintln!("[skip] simhei not found");
        return;
    };
    let font = TtfFont::parse(&data).expect("解析");
    let gid = font.glyph_index('A').expect("'A' 字形");
    let bm = font.rasterize(gid, 16.0).expect("光栅化");
    assert!(bm.width > 0 && bm.height > 0);
    assert!(bm.advance > 0.0);
    assert!(bm.coverage.iter().any(|&c| c > 0), "有覆盖");
    assert!(bm.coverage.iter().any(|&c| c < 255), "包围盒留空（AA）");
    let bm2 = font.rasterize(gid, 16.0).expect("第二次光栅化");
    assert_eq!(bm.coverage, bm2.coverage, "同参数逐字节相同");
}

/// T-TTF-03：CJK 光栅化 + 度量线性。
#[test]
fn t_ttf_03_cjk_raster_and_metrics() {
    let Some(data) = simhei() else {
        eprintln!("[skip] simhei not found");
        return;
    };
    let font = TtfFont::parse(&data).expect("解析");
    let gid = font.glyph_index('中').expect("'中' 字形");
    let bm = font.rasterize(gid, 24.0).expect("24px 光栅化");
    assert!(bm.coverage.iter().any(|&c| c > 0), "'中' 有覆盖");
    assert!(bm.advance > 0.0);
    let m16 = font.metrics(16.0).expect("16px 度量");
    let m32 = font.metrics(32.0).expect("32px 度量");
    assert!(
        (m32.line_height - m16.line_height * 2.0).abs() < 1.0,
        "行高随字号线性：{} vs {}",
        m16.line_height,
        m32.line_height
    );
}

/// T-TTF-04：TTC 集合（msyh.ttc）取第一个字体可解析。
#[test]
fn t_ttf_04_ttc_collection() {
    let Some(data) = msyh() else {
        eprintln!("[skip] msyh.ttc not found");
        return;
    };
    let font = TtfFont::parse(&data).expect("TTC 第一个字体可解析");
    assert!(font.units_per_em() > 0);
    assert!(font.glyph_index('中').is_some(), "雅黑覆盖 CJK");
}

/// T-TTF-05：比例字宽序（同字号 'i' <= 'W'）。
#[test]
fn t_ttf_05_proportional_widths() {
    let Some(data) = simhei() else {
        eprintln!("[skip] simhei not found");
        return;
    };
    let font = TtfFont::parse(&data).expect("解析");
    let i = font
        .glyph_index('i')
        .and_then(|g| font.advance(g, 16.0).ok())
        .expect("'i' advance");
    let w = font
        .glyph_index('W')
        .and_then(|g| font.advance(g, 16.0).ok())
        .expect("'W' advance");
    assert!(i <= w, "'i' ({i}) <= 'W' ({w})");
}
