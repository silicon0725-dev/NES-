//! S7 引擎收束（stabilization）契约：不横向加功能，用现有行为面反向
//! 审出的不变量钉死（本文件全部**无 GPU 依赖** —— 纯函数/静态队列口径）。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Stab-02 | 表面获取状态**命名化**：wgpu 六个状态码 <-> 名字如实映射（未知码报"未知"不猜）；错误信息不再裸数字 |
//! | T-Stab-03 | 字符输入队列**有界**：超限丢弃新字符（FIFO 头保留）、drain 清空 |
//! | T-Stab-04 | BMP 解码**checked 算术**：恶构头（尺寸近 i32 上限）报"头尺寸字段非法"错误而非溢出 panic/回绕；正常路径不受影响 |

use nes_render_wgpu::bmp::load_rgba;
use nes_render_wgpu::gpu::surface_status_name;
use nes_render_wgpu::window::{drain_chars, inject_char};

/// T-Stab-02：状态名映射（收束动机：实测一次 status=3 —— wgpu 的
/// **Timeout**，瞬态可重试 —— 曾被报成"配置参数自相矛盾"并让示例进程
/// 退出；命名 + 分类后错误自带语义）。
#[test]
fn t_stab_02_surface_status_names() {
    assert_eq!(surface_status_name(1), "SUCCESS_OPTIMAL");
    assert_eq!(surface_status_name(2), "SUCCESS_SUBOPTIMAL");
    assert_eq!(surface_status_name(3), "TIMEOUT");
    assert_eq!(surface_status_name(4), "OUTDATED");
    assert_eq!(surface_status_name(5), "LOST");
    assert_eq!(surface_status_name(6), "OUT_OF_MEMORY");
    assert_eq!(surface_status_name(99), "未知状态");
}

/// T-Stab-03：队列容量上限（4096）—— 宿主不排空时内存也不无界增长；
/// 满时丢新保旧（键盘语义），drain 取走清空。
#[test]
fn t_stab_03_typed_queue_bounded() {
    let _ = drain_chars(); // 清前置状态（测试进程共享静态队列）
    for i in 0..5000u32 {
        inject_char(32 + i % 90);
    }
    let drained = drain_chars();
    assert_eq!(drained.len(), 4096, "封顶在 4096");
    assert_eq!(drained[0], 32, "FIFO 头保留（最早的字符在队首）");
    assert_eq!(
        drained.last().copied(),
        Some(32 + 4095 % 90),
        "丢的是后到的溢出字符"
    );
    assert!(drain_chars().is_empty(), "drain 取走清空");
}

/// T-Stab-04：BMP 尺寸算术 checked + usize 域 —— 恶构头（尺寸近 i32
/// 上限）不 panic（debug 溢出 panic / release 回绕都是错的面），如实
/// 报"BMP 不合法"族错误（极端尺寸落在"截断"分支：可寻址总量远超实际
/// 长度）；正常路径不受影响。
#[test]
fn t_stab_04_bmp_extreme_header_errors_not_panics() {
    // 54 字节头 + 极端尺寸（w = h = i32::MAX，32bpp）。
    let mut evil = vec![0u8; 54];
    evil[0..2].copy_from_slice(b"BM");
    evil[10..14].copy_from_slice(&54u32.to_le_bytes()); // data_off
    evil[14..18].copy_from_slice(&40u32.to_le_bytes()); // DIB 头长
    evil[18..22].copy_from_slice(&i32::MAX.to_le_bytes()); // w
    evil[22..26].copy_from_slice(&i32::MAX.to_le_bytes()); // h
    evil[26..28].copy_from_slice(&1u16.to_le_bytes()); // planes
    evil[28..30].copy_from_slice(&32u16.to_le_bytes()); // bpp
    evil[30..34].copy_from_slice(&0u32.to_le_bytes()); // BI_RGB
    let err = load_rgba(&evil).expect_err("极端尺寸必须报错");
    assert!(
        err.to_string().contains("BMP 不合法"),
        "如实报不合法族错误：{err}"
    );

    // 对照：尺寸平凡但数据截断的普通小头 —— 同样走 Err（非 panic）。
    let mut small = evil;
    small[18..22].copy_from_slice(&4u32.to_le_bytes()); // w=4
    small[22..26].copy_from_slice(&4u32.to_le_bytes()); // h=4
    let err2 = load_rgba(&small).expect_err("截断必须报错");
    assert!(err2.to_string().contains("截断"), "指名截断：{err2}");
}
