//! 手写 PNG 编码器（零第三方依赖）。
//!
//! # 为什么自己写
//!
//! 本 crate 的依赖纪律只允许 `nes-render-api`（见 `Cargo.toml` 的 G8/G9/G10 注释），
//! 而 S4.1 的验收要求"把读回的像素落成一张 PNG 截图作为可视证据"。
//! 为一张 64x64 的图引入 `image` / `png` 两个 registry 依赖，代价大于收益。
//!
//! 于是这里实现 PNG 的最小标准子集：
//!
//! - 位深 8、颜色类型 6（RGBA，无调色板、无隔行）；
//! - 每条扫描线的过滤器固定为 0（None）；
//! - IDAT 内是一段合法的 zlib 流，但只用**存储块**（`BTYPE = 00`，不压缩），
//!   外加 zlib 尾部的 Adler-32 与每个块的 CRC-32。
//!
//! # 为什么用存储块而不是真压缩
//!
//! 真压缩要写 deflate 的霍夫曼编码与匹配搜索，出错面远大于收益。存储块只在
//! 每 64KB 数据前加 5 字节头部，**没有熵编码路径**：一旦长度、反码长度或 CRC
//! 写错，解码器会直接报错。也就是说，这个选择把"写错"从"像素悄悄变花"
//! 退化成了"解码器当场拒绝" —— 对一份要当证据的截图来说，这个交换是划算的。
//!
//! # 交叉验证
//!
//! 本模块**不做**自我解码（自证只能证明自洽）。交叉验证放在
//! `examples/s41_visual_closure.rs` 的验证小节：用外部解码器（`System.Drawing`）
//! 打开落盘的 PNG，回读像素并与内存中的期望值比对。

use std::path::Path;
use std::sync::OnceLock;

use crate::error::BackendError;

/// PNG 文件签名（8 字节魔数）。
const SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// zlib 头：`CMF = 0x78`（deflate、32K 窗口）、`FLG = 0x01`（最快压缩级别、
/// 无预置字典）。`(CMF << 8 | FLG) % 31 == 0` 是 zlib 的强制校验位，这里成立。
const ZLIB_HEADER: [u8; 2] = [0x78, 0x01];

/// 存储块单块能装的最大字节数（`LEN` 是 `u16`）。
const STORED_BLOCK_MAX: usize = 65_535;

/// 把 RGBA8 像素编码为 PNG 字节流。
///
/// `rgba` 必须恰好是 `width * height * 4` 字节，行优先、**不含**行首过滤器字节
/// （过滤器字节由本函数按 0 补齐）。尺寸为 0 或长度不符时不写文件、直接报错，
/// 避免把一张来历不明的图当成证据落盘。
pub fn encode_rgba8(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, BackendError> {
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidImageSize { width, height });
    }
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(BackendError::PixelBufferSize {
            expected,
            actual: rgba.len(),
        });
    }

    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity(height as usize * (1 + stride));
    for row in rgba.chunks_exact(stride) {
        raw.push(0); // 过滤器类型 0：None
        raw.extend_from_slice(row);
    }

    let idat = deflate_stored(&raw);

    let mut out = Vec::with_capacity(idat.len() + 64);
    out.extend_from_slice(&SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // 位深 8 / 颜色类型 6(RGBA) / 压缩方法 0 / 过滤器方法 0 / 隔行 0
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    push_chunk(&mut out, b"IHDR", &ihdr);
    push_chunk(&mut out, b"IDAT", &idat);
    push_chunk(&mut out, b"IEND", &[]);

    Ok(out)
}

/// 编码并写入 `path`（父目录不存在时自动创建），返回实际写入的字节数。
pub fn write_rgba8_png(
    path: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<usize, BackendError> {
    let bytes = encode_rgba8(width, height, rgba)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, &bytes)?;
    Ok(bytes.len())
}

/// zlib 流：`头 + 存储块序列 + Adler-32`（输入为空时也产出合法流）。
pub fn deflate_stored(raw: &[u8]) -> Vec<u8> {
    let total = raw.len();
    let mut out = Vec::with_capacity(total + total / STORED_BLOCK_MAX * 5 + 16);
    out.extend_from_slice(&ZLIB_HEADER);

    let mut offset = 0usize;
    loop {
        let end = core::cmp::min(offset + STORED_BLOCK_MAX, total);
        let is_last = end == total;
        let chunk = &raw[offset..end];
        // BFINAL(1 bit) + BTYPE(2 bit, 00 = 存储) + 补齐到字节边界 → 单字节
        out.push(u8::from(is_last));
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
        offset = end;
        if is_last {
            break;
        }
    }

    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// CRC-32（IEEE 802.3，反射多项式 `0xEDB88320`），PNG 每个块的校验字段。
pub fn crc32(bytes: &[u8]) -> u32 {
    let table = crc32_table();
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        let index = ((crc ^ u32::from(*byte)) & 0xFF) as usize;
        crc = table[index] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// Adler-32（zlib 流尾校验）。
pub fn adler32(bytes: &[u8]) -> u32 {
    /// 最大的 `n`，使 `MOD` 为模时 `b` 不会在 8 位累加中溢出。
    const MOD: u32 = 65_521;
    let mut a = 1u32;
    let mut b = 0u32;
    for chunk in bytes.chunks(5_552) {
        for byte in chunk {
            a += u32::from(*byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// CRC 查表（首次使用时构建 256 项，之后复用）。
fn crc32_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (index, slot) in table.iter_mut().enumerate() {
            let mut value = index as u32;
            for _ in 0..8 {
                value = if value & 1 == 1 {
                    0xEDB8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                };
            }
            *slot = value;
        }
        table
    })
}

/// 追加一个 PNG 块：`长度 + 类型 + 数据 + CRC-32`（CRC 覆盖类型与数据）。
fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);

    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_check_vector() {
        // 标准检查值（CRC-32/ISO-HDLC）：crc32("123456789") == 0xCBF43926
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0x0000_0000);
    }

    #[test]
    fn adler32_matches_check_vector() {
        // 标准检查值：adler32("Wikipedia") == 0x11E60398
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn encode_writes_well_formed_chunk_sequence() {
        let pixels: Vec<u8> = (0u8..16).collect();
        let png = encode_rgba8(2, 2, &pixels).expect("2x2 编码成功");
        assert_eq!(&png[..8], &SIGNATURE);

        let mut kinds: Vec<Vec<u8>> = Vec::new();
        let mut pos = 8usize;
        while pos < png.len() {
            let len = u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]])
                as usize;
            let end = pos + 12 + len;
            assert!(end <= png.len(), "块长度不得越过文件末尾");
            let stored =
                u32::from_be_bytes([png[end - 4], png[end - 3], png[end - 2], png[end - 1]]);
            assert_eq!(stored, crc32(&png[pos + 4..end - 4]), "块 CRC 必须自洽");
            kinds.push(png[pos + 4..pos + 8].to_vec());
            pos = end;
        }
        assert_eq!(pos, png.len(), "块序列必须正好铺满整个文件");
        assert_eq!(
            kinds,
            vec![b"IHDR".to_vec(), b"IDAT".to_vec(), b"IEND".to_vec()]
        );
    }

    #[test]
    fn encode_rejects_wrong_pixel_count() {
        let err = encode_rgba8(2, 2, &[0u8; 15]).expect_err("长度不符必须报错");
        assert_eq!(
            err,
            BackendError::PixelBufferSize {
                expected: 16,
                actual: 15
            }
        );
        assert_eq!(
            encode_rgba8(0, 4, &[]).expect_err("零宽必须报错"),
            BackendError::InvalidImageSize { width: 0, height: 4 }
        );
    }
}
