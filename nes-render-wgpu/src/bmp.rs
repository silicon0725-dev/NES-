//! 极简 BMP 装载器（32bpp / 24bpp，BI_RGB 无压缩，自底向上行序）。
//!
//! # 为什么进库
//!
//! 外部预处理工具（System.Drawing 一类）把任意格式图片 / 字体栅格统一转成
//! BMP 后，宿主侧需要把它读回 RGBA 才能走 [`crate::TextureRegistry`] 上传。
//! BMP 是唯一**无熵编码**的常见位图格式（行序与像素布局固定），手写解析的
//! 出错面极小 —— 与本 crate 零依赖、手写 PNG 编码器是同一条纪律：
//! "解码交给外部工具，但无压缩的搬运格式可以自己读"。
//!
//! # 不支持（如实报错，不猜）
//!
//! RLE / JPEG/PNG 压缩的 BMP、顶朝下（负高度）、调色板（bpp <= 8）、
//! BITMAPCOREHEADER —— 预处理工具不会产出这些形态，出现即按配置错误上报。

use crate::error::BackendError;

/// 解析 BMP 字节流，返回 `(宽, 高, RGBA8 紧凑像素)`（行优先、顶朝下）。
pub fn load_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), BackendError> {
    let invalid = |why: &str| BackendError::ConfigMismatch(format!("BMP 不合法：{why}"));
    if bytes.len() < 54 || &bytes[0..2] != b"BM" {
        return Err(invalid("魔数不符或文件过短"));
    }
    let u16at = |o: usize| u16::from_le_bytes(bytes[o..o + 2].try_into().unwrap());
    let u32at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let i32at = |o: usize| i32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());

    let data_off = u32at(10) as usize;
    if u32at(14) < 40 {
        return Err(invalid("只支持 BITMAPINFOHEADER 及其扩展（DIB 头 < 40 字节）"));
    }
    let (w, h) = (i32at(18), i32at(22));
    let (bpp, compression) = (u16at(28), u32at(30));
    if w <= 0 || h <= 0 {
        return Err(invalid(&format!(
            "尺寸 {w}x{h} 非法（顶朝下 BMP 不在支持形态内）"
        )));
    }
    if compression != 0 {
        return Err(invalid(&format!("压缩方式 {compression} 不支持（仅 BI_RGB）")));
    }
    let (w, h) = (w as u32, h as u32);
    let bytes_per_px = match bpp {
        32 => 4,
        24 => 3,
        other => return Err(invalid(&format!("位深 {other} 不支持（仅 24/32bpp）"))),
    };
    // 尺寸算术用 checked（收束阶段）：恶构头（w/h 近 i32 上限）会让
    // 乘加在 debug 下溢出 panic、release 下回绕 —— 先算出可寻址总量，
    // 溢出即按不合法头报错，再与实际长度比对（分配因此也受文件长度约束）。
    let row = (w as usize * bytes_per_px).div_ceil(4) * 4; // 行按 4 字节对齐补齐
    let pixels = row
        .checked_mul(h as usize)
        .ok_or_else(|| invalid("像素总量超出可寻址范围（头尺寸字段非法）"))?;
    let expected = data_off
        .checked_add(pixels)
        .ok_or_else(|| invalid("数据偏移 + 像素总量溢出（头尺寸字段非法）"))?;
    if bytes.len() < expected {
        return Err(invalid(&format!(
            "像素数据截断：需要 {expected} 字节，实际 {}",
            bytes.len()
        )));
    }

    // 容量按 usize 算（w*h*4 在 u32 域会回绕 —— 大图合法文件也中招）；
    // 此时 expected <= len 已成立，分配受文件长度约束。
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for y in (0..h).rev() {
        // 自底向上：文件里第 0 行是图像最后一行。
        let start = data_off + y as usize * row;
        let line = &bytes[start..start + row];
        for px in line.chunks_exact(bytes_per_px) {
            // BMP 存储序 BGR(A) -> RGBA。
            rgba.extend_from_slice(&[px[2], px[1], px[0], if bpp == 32 { px[3] } else { 255 }]);
        }
    }
    Ok((w, h, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一张 2x2、32bpp、自底向上的最小 BMP（像素经 `bgra` 行序给出）。
    fn tiny_bmp(bgra_rows_top_down: [[u8; 4]; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&70u32.to_le_bytes()); // 文件大小
        out.extend_from_slice(&[0; 4]); // 保留
        out.extend_from_slice(&54u32.to_le_bytes()); // 像素数据偏移
        out.extend_from_slice(&40u32.to_le_bytes()); // DIB 头大小
        out.extend_from_slice(&2i32.to_le_bytes()); // 宽
        out.extend_from_slice(&2i32.to_le_bytes()); // 高（正 = 自底向上）
        out.extend_from_slice(&1u16.to_le_bytes()); // 平面数
        out.extend_from_slice(&32u16.to_le_bytes()); // 位深
        out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        out.extend_from_slice(&16u32.to_le_bytes()); // 图像字节数
        out.extend_from_slice(&[0; 16]); // 分辨率/调色板
        // 像素：文件行序 = 自底向上（图像最后一行在前），行内顺序不变。
        for row in bgra_rows_top_down.chunks_exact(2).rev() {
            for px in row {
                out.extend_from_slice(px);
            }
        }
        out
    }

    #[test]
    fn parses_bottom_up_and_swaps_channels() {
        // 数组按 **BGRA 存储序**、行主序（顶朝下）给出。
        let bmp = tiny_bmp([
            [255, 0, 0, 255], // 图像 (0,0)：存储蓝 -> 语义蓝 [0,0,255]
            [0, 255, 0, 255], // (1,0)：绿（对称，交换不变）
            [0, 0, 255, 255], // (0,1)：存储红 -> 语义红 [255,0,0]
            [255, 255, 255, 128],
        ]);
        let (w, h, rgba) = load_rgba(&bmp).expect("解析");
        assert_eq!((w, h), (2, 2));
        assert_eq!(&rgba[0..4], &[0, 0, 255, 255], "(0,0) 存储蓝 -> 语义蓝");
        assert_eq!(&rgba[4..8], &[0, 255, 0, 255], "(1,0) 绿");
        assert_eq!(&rgba[8..12], &[255, 0, 0, 255], "(0,1) 存储红 -> 语义红");
        assert_eq!(&rgba[12..16], &[255, 255, 255, 128], "(1,1) 透明度直传");
    }

    #[test]
    fn rejects_malformed() {
        assert!(matches!(load_rgba(b"nope"), Err(BackendError::ConfigMismatch(_))));
        // 压缩位非零。
        let mut bmp = tiny_bmp([[0; 4]; 4]);
        bmp[30..34].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(load_rgba(&bmp), Err(BackendError::ConfigMismatch(_))));
        // 像素截断。
        let mut bmp = tiny_bmp([[0; 4]; 4]);
        bmp.truncate(60);
        assert!(matches!(load_rgba(&bmp), Err(BackendError::ConfigMismatch(_))));
    }
}
