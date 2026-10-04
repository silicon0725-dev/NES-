//! IMA ADPCM 解码（AMV 音轨）—— **S14.3 音轨有声化**（FFmpeg
//! `ADPCM_IMA_AMV` 语义的零依赖手写移植）。
//!
//! # 这个模块是什么
//!
//! AMV（山寨 MP4 播放器录像格式）的音轨载荷是**分块 IMA ADPCM**：容器
//! （nes-media 的 `amv` 模块）把每个 `'01wb'` 块体原样交进来，本模块
//! 展开成 16-bit PCM 样本。解码语义逐条对照 FFmpeg `libavcodec/adpcm.c`
//! 的 `ADPCM_IMA_AMV` 分支（本仓库 `ruference/ffmpeg` 本地参照）：
//!
//! * **块头 8 字节**（FFmpeg：读 3 字节后 `skipu(5)`，尼布流从偏移 8 起）：
//!   ```text
//!   +0  i16 predictor（LE，初始预测值）
//!   +2  u8  step_index（步长表下标，> 88 拒绝 —— FFmpeg 同款检查）
//!   +3  u8  reserved（FFmpeg 跳过不看）
//!   +4  u32 frame_size（LE，本块声明样本数）
//!   ```
//!   实际展开样本数 = `min(尼布数 × 2, frame_size)` —— FFmpeg 的
//!   `FFMIN((buf_size - 8) * 2, coded_samples)` 同款；声明少则按声明截断，
//!   声明多则按实际尼布数兜底。
//! * **尼布序：高半字节在前**（每字节先 `byte >> 4` 后 `byte & 0x0F`，
//!   各产一个样本）。这是 FFmpeg `ADPCM_IMA_AMV` 的解码序，且与其编码侧
//!   （`adpcmenc.c`：`compress(样本0) << 4 | compress(样本1)`）自洽 ——
//!   与同文件 IMA WAV 分支（低半字节在前）**相反**，移植时不可混用。
//! * **展开公式**（`adpcm_ima_expand_nibble`，shift=3）：
//!   ```text
//!   diff = ((2 * (code & 7) + 1) * step) >> 3
//!   predictor += (code & 8) ? -diff : diff   // 再 clamp 到 i16
//!   step_index += index_table[code]          // 再 clamp 到 0..=88
//!   ```
//! * **逐块状态重置**：每块头自带 predictor/step_index，块间互不延续
//!   —— 解码器对每个块从块头重新起步，块序拼接即整条音轨。
//!
//! # 失败面（与 crate 错误哲学同一条：指名道姓、不 panic）
//!
//! * 块不足 8 字节头：[`AdpcmError::BadHeader`]；
//! * 块头 `step_index > 88`：[`AdpcmError::StepIndex`]（携带实际值）。
//!   上层（nes-media 的 `amv::AmvVideo::audio`）对任一块失败如实返回
//!   `None` —— 音轨缺席、视频照常可用，不给半截音轨。
//!
//! # 家法
//!
//! 零依赖（本 crate 是依赖树纯叶子）：两张常量表手抄自 FFmpeg
//! `libavcodec/adpcm_data.c` 的 `ff_adpcm_step_table[89]` /
//! `ff_adpcm_index_table[16]` —— 出自公开的 IMA ADPCM 参考实现，非
//! FFmpeg 私有数据。`#![forbid(unsafe_code)]` 与 crate 各模块一致。

#![forbid(unsafe_code)]

use std::fmt;

/// 步长表（FFmpeg `adpcm_data.c` 的 `ff_adpcm_step_table[89]`；出自公开
/// IMA ADPCM 参考实现 —— 各家实现偶有微差，本表与 FFmpeg 逐项一致）。
const STEP_TABLE: [i16; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, //
    19, 21, 23, 25, 28, 31, 34, 37, 41, 45, //
    50, 55, 60, 66, 73, 80, 88, 97, 107, 118, //
    130, 143, 157, 173, 190, 209, 230, 253, 279, 307, //
    337, 371, 408, 449, 494, 544, 598, 658, 724, 796, //
    876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, //
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, //
    5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, //
    15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// 索引调整表（FFmpeg `adpcm_data.c` 的 `ff_adpcm_index_table[16]`，同源
/// 公开 IMA 表；下标 = 4-bit 尼布码，值加进 step_index 后再 clamp）。
const INDEX_TABLE: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// IMA ADPCM 解码可报告的失败点。`Display` 全中文（与 crate 各错误同一家法）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdpcmError {
    /// 块长度不足 8 字节头（截断或非本格式数据）。
    BadHeader,
    /// 块头 step_index 越界（> 88），携带实际值 —— FFmpeg 同款拒绝线。
    StepIndex(u8),
}

impl fmt::Display for AdpcmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdpcmError::BadHeader => {
                write!(f, "IMA ADPCM 解码失败：块头不足 8 字节（数据截断）")
            }
            AdpcmError::StepIndex(v) => {
                write!(f, "IMA ADPCM 解码失败：头部 step_index = {v} 越界（上限 88）")
            }
        }
    }
}

impl std::error::Error for AdpcmError {}

/// 解码一整条音轨：全部 `'01wb'` 块体逐块展开后顺序拼接。
///
/// 每块自含状态（块头 predictor/step_index），块间不延续；任一块解码
/// 失败即整体报错（调用方拿到 [`AdpcmError`]，不会拿到半截样本）。
pub fn decode_ima_amv(chunks: &[&[u8]]) -> Result<Vec<i16>, AdpcmError> {
    let mut out = Vec::new();
    for chunk in chunks {
        out.extend_from_slice(&decode_ima_amv_block(chunk)?);
    }
    Ok(out)
}

/// 解码单个 IMA ADPCM 块（8 字节头 + 尼布流，状态从块头起步）。
///
/// 语义逐条对照 FFmpeg `adpcm.c` 的 `ADPCM_IMA_AMV` 分支：实际展开数 =
/// `min((块长 - 8) × 2, frame_size)`；样本数为奇数时，末字节只取高半
/// 字节（低半字节按 FFmpeg 裁决跳过 —— 真实文件该位恒 0）。
pub fn decode_ima_amv_block(block: &[u8]) -> Result<Vec<i16>, AdpcmError> {
    if block.len() < 8 {
        return Err(AdpcmError::BadHeader);
    }
    let predictor0 = i16::from_le_bytes([block[0], block[1]]);
    let step_index0 = block[2];
    if step_index0 > 88 {
        return Err(AdpcmError::StepIndex(step_index0));
    }
    let frame_size =
        u32::from_le_bytes(block[4..8].try_into().expect("头内切片固定 4 字节")) as usize;
    let nibbles = &block[8..];
    let count = nibbles.len().saturating_mul(2).min(frame_size);
    if count == 0 {
        return Ok(Vec::new());
    }

    let mut predictor = i32::from(predictor0);
    let mut step_index = usize::from(step_index0);
    let mut out = Vec::with_capacity(count);
    for &byte in nibbles {
        // 高半字节在前（FFmpeg ADPCM_IMA_AMV 解码序；见模块文档）。
        for nibble in [byte >> 4, byte & 0x0F] {
            let step = i32::from(STEP_TABLE[step_index]);
            let delta = i32::from(nibble & 0x07);
            let diff = ((2 * delta + 1) * step) >> 3;
            predictor += if nibble & 0x08 != 0 { -diff } else { diff };
            predictor = predictor.clamp(i32::from(i16::MIN), i32::from(i16::MAX));
            let next = step_index as i32 + i32::from(INDEX_TABLE[usize::from(nibble)]);
            step_index = next.clamp(0, 88) as usize;
            out.push(predictor as i16);
            if out.len() == count {
                return Ok(out); // frame_size 截断：声明之外的尼布不再展开。
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------ 单元测试
//
// 家法：仓库不提交二进制资产，夹具现场手编（字节字面量全 ASCII）；期望
// 值全部按展开公式手算写死（不拿实现算实现）。表值另以"首项 + 尾项 +
// 长度"抽查钉住手抄错误。

#[cfg(test)]
mod tests {
    use super::*;

    /// 装配一个块：predictor / step_index / frame_size + 尼布字节。
    fn block(predictor: i16, step_index: u8, frame_size: u32, nibbles: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + nibbles.len());
        out.extend_from_slice(&predictor.to_le_bytes());
        out.push(step_index);
        out.push(0); // reserved（FFmpeg 跳过不看）
        out.extend_from_slice(&frame_size.to_le_bytes());
        out.extend_from_slice(nibbles);
        out
    }

    /// 手算基线夹具：predictor=0、step_index=0、尼布码 [4, 7, 8, 8]
    /// （字节 0x47、0x88）。逐样本手算：
    ///   s0: code=4, step=7,  diff=(9*7)>>3=7    -> 0+7  = 7   (idx 0+2=2)
    ///   s1: code=7, step=9,  diff=(15*9)>>3=16  -> 7+16 = 23  (idx 2+8=10)
    ///   s2: code=8, step=19, diff=(1*19)>>3=2   -> 23-2 = 21  (idx 10-1=9)
    ///   s3: code=8, step=17, diff=(1*17)>>3=2   -> 21-2 = 19  (idx 9-1=8)
    const BASE_EXPECTED: [i16; 4] = [7, 23, 21, 19];

    #[test]
    fn t_adpcm01_hand_computed_baseline() {
        // 手编尼布流 + 手算期望值：展开公式、步长/索引表、clamp 全链路。
        let b = block(0, 0, 4, &[0x47, 0x88]);
        assert_eq!(decode_ima_amv_block(&b), Ok(BASE_EXPECTED.to_vec()));
    }

    #[test]
    fn t_adpcm02_high_nibble_first() {
        // 尼布序钉死：字节 0x47 高半字节（code 4 -> 样本 7）在前，低半
        // 字节（code 7 -> 样本 23）在后。若是低半字节在前，首样本应为
        // code 7 的展开值 13（(15*7)>>3=13）—— 断言 [7, ..] 即排除了它。
        let b = block(0, 0, 2, &[0x47]);
        assert_eq!(decode_ima_amv_block(&b), Ok(vec![7, 23]));
    }

    #[test]
    fn t_adpcm03_header_predictor_and_step_honored() {
        // 块头 predictor/step_index 必须真实起步：predictor=-100、
        // step_index=5（step=12）、code=0 -> diff=(1*12)>>3=1 -> -99。
        let b = block(-100, 5, 2, &[0x00]);
        assert_eq!(decode_ima_amv_block(&b), Ok(vec![-99, -98]));
    }

    #[test]
    fn t_adpcm04_step_index_out_of_range_named() {
        // FFmpeg 同款拒绝线：> 88 拒绝（89 与 255 都报），88 本身合法。
        assert_eq!(
            decode_ima_amv_block(&block(0, 89, 2, &[0x00])),
            Err(AdpcmError::StepIndex(89))
        );
        assert_eq!(
            decode_ima_amv_block(&block(0, 255, 2, &[0x00])),
            Err(AdpcmError::StepIndex(255))
        );
        let ok = block(0, 88, 2, &[0x00]);
        assert!(decode_ima_amv_block(&ok).is_ok(), "88 是合法上界");
    }

    #[test]
    fn t_adpcm05_bad_header() {
        // 不足 8 字节头 -> BadHeader；恰好 8 字节（无尼布）-> 空样本。
        assert_eq!(decode_ima_amv_block(&[0; 7]), Err(AdpcmError::BadHeader));
        assert_eq!(decode_ima_amv_block(&[]), Err(AdpcmError::BadHeader));
        assert_eq!(decode_ima_amv_block(&block(0, 0, 0, &[])), Ok(vec![]));
    }

    #[test]
    fn t_adpcm06_frame_size_truncates() {
        // frame_size 声明 2 < 实际尼布 8：只展开 2 个样本（FFmpeg 的
        // FFMIN 裁决），多出的尼布字节不消费。
        let b = block(0, 0, 2, &[0x47, 0x88, 0x88, 0x88]);
        assert_eq!(decode_ima_amv_block(&b), Ok(vec![7, 23]));

        // frame_size 声明 3（奇数）：末字节只取高半字节（code 8 ->
        // step=19，diff=2，23-2=21），低半字节按 FFmpeg 裁决跳过。
        let odd = block(0, 0, 3, &[0x47, 0x88]);
        assert_eq!(decode_ima_amv_block(&odd), Ok(vec![7, 23, 21]));
    }

    #[test]
    fn t_adpcm07_predictor_and_step_index_clamps() {
        // predictor 上 clamp：32700 + 28666（code 7 @ idx 80，step=15289，
        // diff=(15*15289)>>3=28666）= 61366 -> 32767。
        let up = block(32700, 80, 1, &[0x07]);
        assert_eq!(decode_ima_amv_block(&up), Ok(vec![32767]));

        // 双向下探：idx=88（step=32767）连续 code 0xF -> diff=(15*32767)>>3
        // = 61438 -> 0-61438 clamp -32768；且 idx 88+8 clamp 回 88（第二
        // 个样本仍用 32767 步长，仍是 -32768）。
        let down = block(0, 88, 2, &[0xFF]);
        assert_eq!(decode_ima_amv_block(&down), Ok(vec![-32768, -32768]));
    }

    #[test]
    fn t_adpcm08_multi_chunk_reset_and_concat() {
        // 逐块状态重置 + 拼接：块 B 从自己块头（predictor=1000、
        // step_index=30、step=130）重新起步，不延续块 A 的 19：
        //   s0: code=0, diff=(1*130)>>3=16  -> 1000+16 = 1016 (idx 30-1=29)
        //   s1: code=0, step=118, diff=(1*118)>>3=14 -> 1016+14 = 1030
        let a = block(0, 0, 4, &[0x47, 0x88]);
        let b = block(1000, 30, 2, &[0x00]);
        let refs = [&a[..], &b[..]];
        assert_eq!(
            decode_ima_amv(&refs),
            Ok(vec![7, 23, 21, 19, 1016, 1030]),
            "块 B 必须从块头状态重新起步"
        );
        // 任一块失败 -> 整体 Err（不给半截）。
        let bad = block(0, 200, 2, &[0x00]);
        let refs = [&a[..], &bad[..]];
        assert_eq!(decode_ima_amv(&refs), Err(AdpcmError::StepIndex(200)));
    }

    #[test]
    fn t_adpcm09_tables_transcribed_correctly() {
        // 手抄表抽查：长度 + 首尾项 + 两个中间锚点，与 FFmpeg
        // adpcm_data.c 逐项核对过的值一致。
        assert_eq!(STEP_TABLE.len(), 89);
        assert_eq!(STEP_TABLE[0], 7);
        assert_eq!(STEP_TABLE[40], 337);
        assert_eq!(STEP_TABLE[80], 15289);
        assert_eq!(STEP_TABLE[88], 32767);
        assert_eq!(INDEX_TABLE.len(), 16);
        assert_eq!(INDEX_TABLE[0], -1);
        assert_eq!(INDEX_TABLE[4], 2);
        assert_eq!(INDEX_TABLE[7], 8);
        assert_eq!(INDEX_TABLE[15], 8);
    }
}
