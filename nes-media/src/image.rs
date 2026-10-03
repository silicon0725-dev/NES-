//! 图像解码（`image` 0.25 系）：任意 `image` 支持的格式 -> RGBA8 DTO。
//!
//! # 为什么是适配层
//!
//! 引擎核心的纹理口径是 **BMP**（`nes-render-wgpu::bmp` 手写解析，零依赖
//! 纪律）。外部交付物却是 PNG/JPEG/GIF/WebP —— 本模块把 `image` 库收口
//! 在这里：入口只有 [`decode_image`] 一个函数，出口只有 [`DecodedImage`]
//! 一个结构体（width/height/RGBA8），`image::DynamicImage` 等第三方类型
//! **不出本 crate** —— 上层（nes-runtime / 编辑器）对解码器零感知。
//!
//! # 格式探测与动图
//!
//! * 格式按**内容**探测（`guess_format` 读文件头魔数），不看扩展名 ——
//!   资产管线里扩展名会说谎，字节不会；
//! * GIF 取**首帧**（`load_from_memory` 语义），动图帧数经 GIF 解码器
//!   如实数出记 [`DecodedImage::frame_count`]（`Some(n)`；静态图为
//!   `None` —— 上层据此决定将来做帧动画还是首帧静态）；
//! * 失败面三态：[`MediaError::UnsupportedFormat`]（文件头不识别）、
//!   [`MediaError::Decode`]（数据损坏/截断）、[`MediaError::Io`]（缓冲
//!   层 IO）—— `image::ImageError` 在边界处一次性归一。

#![forbid(unsafe_code)]

use ::image::AnimationDecoder as _;
use crate::MediaError;

/// 解码完成的图像：尺寸 + RGBA8 像素（行主序、每像素 4 字节）。
///
/// 与 `nes-render-wgpu` 纹理注册表的入参同构（`register_texture(key, w,
/// h, rgba)`）—— 上层拿到即可上传，无需任何转换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    /// 宽（像素）。
    pub width: u32,
    /// 高（像素）。
    pub height: u32,
    /// RGBA8 像素，长度恒为 `width * height * 4`。
    pub rgba: Vec<u8>,
    /// 动图帧数（GIF 如实数出；静态图 `None`）。首帧已解码进 `rgba`。
    pub frame_count: Option<u32>,
}

/// 从内存字节解码一张图像为 RGBA8（格式按内容探测）。
///
/// 只认字节流，不做文件 IO（装载属上层职责）。GIF 动图取首帧并记录
/// 总帧数（[`DecodedImage::frame_count`]）。
pub fn decode_image(data: &[u8]) -> Result<DecodedImage, MediaError> {
    // 文件头魔数探测（失败不提前报错 —— 交给 decode 给出更准的原因）。
    let format = ::image::guess_format(data).ok();
    let img = ::image::load_from_memory(data).map_err(map_image_error)?;
    // GIF：首帧已在上面的解码里；动图帧数单独如实数出（损坏的帧序列
    // 不算失败 —— 数到几帧报几帧，首帧静态可用优先）。
    let frame_count = match format {
        Some(::image::ImageFormat::Gif) => {
            ::image::codecs::gif::GifDecoder::new(std::io::Cursor::new(data))
                .ok()
                .map(|dec| dec.into_frames().filter_map(Result::ok).count() as u32)
        }
        _ => None,
    };
    let rgba = img.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    Ok(DecodedImage { width, height, rgba: rgba.into_raw(), frame_count })
}

/// `image::ImageError` -> [`MediaError`] 的一次性归一（边界处收口，
/// 第三方错误类型不出本模块）。
fn map_image_error(e: ::image::ImageError) -> MediaError {
    match e {
        ::image::ImageError::Unsupported(_) => MediaError::UnsupportedFormat,
        ::image::ImageError::IoError(io) => MediaError::Io(io.to_string()),
        other => MediaError::Decode(other.to_string()),
    }
}

// ------------------------------------------------------------ 单元测试
//
// 合成字节夹具：用同一个 image 库现场编码（测试里允许 —— 仓库不提交
// 二进制资产，与 WAV 蜂鸣/BMP 演示纹理同一条家法）。真实媒体文件的
// 契约测试在 tests/real_media.rs（skip-if-missing）。

#[cfg(test)]
mod tests {
    use super::*;

    /// 用 image 库现场编码一张 RGBA8 PNG。
    fn encode_png(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
        let img = ::image::RgbaImage::from_raw(w, h, px.to_vec()).expect("像素缓冲");
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ::image::ImageFormat::Png).expect("编码 PNG");
        out.into_inner()
    }

    /// 用 image 库现场编码一张 RGBA8 JPEG（有损 —— 只断尺寸不断像素；
    /// JPEG 编码器只收 RGB8：编码前转一次，不影响解码面）。
    fn encode_jpeg(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
        let img = ::image::RgbaImage::from_raw(w, h, px.to_vec()).expect("像素缓冲");
        let mut out = std::io::Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgba8(img)
            .to_rgb8()
            .write_to(&mut out, ::image::ImageFormat::Jpeg)
            .expect("编码 JPEG");
        out.into_inner()
    }

    #[test]
    fn t_img01_png_roundtrip_size_and_pixels() {
        // 3x2 渐变：解码往返后尺寸与像素逐字节保真（PNG 无损）。
        let mut px = Vec::new();
        for i in 0..3 * 2 {
            px.extend_from_slice(&[(i * 7) as u8, (i * 13) as u8, (i * 29) as u8, 255]);
        }
        let png = encode_png(3, 2, &px);
        let img = decode_image(&png).expect("PNG 必须可解码");
        assert_eq!(img.width, 3);
        assert_eq!(img.height, 2);
        assert_eq!(img.rgba.len(), 3 * 2 * 4, "RGBA8 恒为 w*h*4");
        assert_eq!(img.rgba, px, "PNG 无损往返逐字节保真");
        assert_eq!(img.frame_count, None, "静态图无帧数");
    }

    #[test]
    fn t_img02_jpeg_dimensions_preserved() {
        // JPEG 有损：像素不做逐字节断言（量化误差），尺寸与缓冲形状必须准。
        let px = vec![128u8; 8 * 4 * 4];
        let jpeg = encode_jpeg(8, 4, &px);
        let img = decode_image(&jpeg).expect("JPEG 必须可解码");
        assert_eq!(img.width, 8);
        assert_eq!(img.height, 4);
        assert_eq!(img.rgba.len(), 8 * 4 * 4);
        assert_eq!(img.frame_count, None);
    }

    #[test]
    fn t_img03_gif_reports_frame_count_first_frame_decoded() {
        // 现场编码两帧 GIF：frame_count = Some(2)，首帧像素可用。
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut enc = ::image::codecs::gif::GifEncoder::new(&mut out);
            let frame_px = |v: u8| vec![v; 2 * 2 * 4];
            enc.encode_frame(
                ::image::Frame::from_parts(
                    ::image::RgbaImage::from_raw(2, 2, frame_px(10)).unwrap(),
                    0,
                    0,
                    ::image::Delay::from_saturating_duration(
                        std::time::Duration::from_millis(100),
                    ),
                ),
            )
            .expect("编码第一帧");
            enc.encode_frame(
                ::image::Frame::from_parts(
                    ::image::RgbaImage::from_raw(2, 2, frame_px(200)).unwrap(),
                    0,
                    0,
                    ::image::Delay::from_saturating_duration(
                        std::time::Duration::from_millis(100),
                    ),
                ),
            )
            .expect("编码第二帧");
            // enc 在此 drop：GIF 尾部帧元数据随 drop 收尾。
        }
        let gif = out.into_inner();
        let img = decode_image(&gif).expect("GIF 必须可解码");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.frame_count, Some(2), "动图帧数如实报告");
        assert_eq!(img.rgba.len(), 2 * 2 * 4, "解码产物 = 首帧 RGBA8");
        assert_eq!(img.rgba[0], 10, "首帧像素（第一帧的值 10）");
    }

    #[test]
    fn t_img04_garbage_reports_unsupported_format() {
        // 明确不是任何已收录格式的字节：如实报 UnsupportedFormat。
        let err = decode_image(b"\x00this is not an image at all\x01\x02").expect_err("垃圾字节必须被拒");
        assert_eq!(err, MediaError::UnsupportedFormat);
        assert!(err.to_string().contains("格式不支持"), "Display 中文指名：{err}");
    }

    #[test]
    fn t_img05_truncated_png_reports_named_error() {
        // 合法 PNG 头 + 被裁断的数据体：截断是"数据不完整"，必须报出
        // 指名错误（经内存游标落地为 Io(EOF)，语义与 Decode 同类 ——
        // 关键是不许静默、不许误报"格式不支持"）。
        let mut png = encode_png(2, 2, &[9u8; 2 * 2 * 4]);
        png.truncate(png.len() / 2);
        let err = decode_image(&png).expect_err("截断 PNG 必须被拒");
        assert!(
            matches!(err, MediaError::Decode(_) | MediaError::Io(_)),
            "截断数据报 Decode/Io（非 Unsupported）：{err:?}"
        );
    }

    #[test]
    fn t_img06_error_display_is_chinese_named() {
        assert!(MediaError::UnsupportedFormat.to_string().contains("格式不支持"));
        let d = MediaError::Decode("像素流损坏".into());
        assert!(d.to_string().contains("媒体解码失败") && d.to_string().contains("像素流损坏"));
        let io = MediaError::Io("disk full".into());
        assert!(io.to_string().contains("媒体读取失败") && io.to_string().contains("disk full"));
    }
}
