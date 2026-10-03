//! 设备冒烟测试：waveOut 开/关 + 静音填充 + 反复开合无泄漏。
//!
//! 覆盖面：
//!
//! | 用例 | 依赖设备 | 验证什么 |
//! |---|---|---|
//! | `open_fill_silence_close` | 是 | 打开 → ~1 秒静音填充（空混音器）→ 关闭，全程无 panic |
//! | `reopen_ten_times_idempotent` | 是 | open-close 反复 10 次：占位释放、无 AlreadyOpen 残留、Drop 幂等 |
//! | `already_open_rejected` | 是 | 双开被指名拒绝（`AlreadyOpen`），首设备不受影响 |
//! | `error_display_named` | 否 | 错误 Display 中文指名道姓（无设备也可跑） |
//!
//! # 设备用例的跳过纪律（照 GPU 用例惯例，如实报告）
//!
//! `waveout_device_count() == 0`（CI / 无声卡环境）时设备用例**跳过**并打印
//! 说明；只要系统有设备，打开失败就**直接判失败**——"没有设备"与
//! "有设备但跑不通"是两种不同的事实，不许互相伪装。
//!
//! 混音数学的全部测试在 `src/mixer.rs` / `src/wav.rs`，不需要设备。

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use nes_audio::device::waveout_device_count;
use nes_audio::{AudioDevice, AudioError, Mixer};

/// 设备用例串行锁：本 crate 进程级只允许一个 `AudioDevice`，
/// 两个设备用例并发跑会互相顶出 `AlreadyOpen`，必须串行。
fn device_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 没有可用输出设备时打印跳过说明并返回 true（调用方直接 return）。
fn no_device_skip() -> bool {
    if waveout_device_count() == 0 {
        eprintln!(
            "[跳过设备用例] 本机没有可用的 waveOut 输出设备（waveOutGetNumDevs = 0），混音数学用例不受影响"
        );
        return true;
    }
    false
}

fn empty_mixer() -> Arc<Mutex<Mixer>> {
    Arc::new(Mutex::new(Mixer::new()))
}

#[test]
fn t_dev01_open_fill_silence_one_second_close() {
    let _guard = device_lock();
    if no_device_skip() {
        return;
    }
    let device = AudioDevice::open(empty_mixer(), 44100, 2)
        .expect("系统有 waveOut 设备时打开必须成功");
    assert!(device.is_open());
    assert_eq!(device.device_rate(), 44100);
    assert_eq!(device.channels(), 2);

    // ~1 秒静音填充冒烟：空混音器的 mix_into 全程写 0，线程循环不 panic 即通过。
    std::thread::sleep(std::time::Duration::from_millis(1000));

    device.close();
    // close 消费了 self；Drop 再跑一次必须是空操作（若不幂等这里会 panic/减两次占位）。
}

#[test]
fn t_dev02_reopen_ten_times_idempotent() {
    let _guard = device_lock();
    if no_device_skip() {
        return;
    }
    for i in 0..10 {
        let device = AudioDevice::open(empty_mixer(), 22050, 1)
            .expect("反复开合必须稳定成功（占位随 close 释放）");
        assert!(device.is_open(), "第 {} 次 open 后应处于打开态", i + 1);
        // close 消费 self；其后 Drop 再跑一遍必须是空操作（否则这里会 panic）。
        device.close();
    }
}

#[test]
fn t_dev03_double_open_rejected_named() {
    let _guard = device_lock();
    if no_device_skip() {
        return;
    }
    let first = AudioDevice::open(empty_mixer(), 44100, 2).expect("首次打开必须成功");
    let second = AudioDevice::open(empty_mixer(), 44100, 2).unwrap_err();
    assert_eq!(second, AudioError::AlreadyOpen, "双开必须被指名拒绝");
    assert!(first.is_open(), "被拒绝的第二次 open 不得影响首个设备");
    first.close();
    // 关闭后占位应已释放：再开一次必须成功（也是 t_dev02 的补充证明）。
    let again = AudioDevice::open(empty_mixer(), 44100, 2);
    assert!(again.is_ok(), "close 释放占位后必须能再开");
    again.unwrap().close();
}

#[test]
fn t_dev04_error_display_named_chinese() {
    // 无设备也可跑：错误类型必须指名道姓（与后端 error.rs 同一条纪律）。
    assert_eq!(
        AudioError::NoDevice.to_string(),
        "音频设备：没有可用的 waveOut 输出设备（waveOutGetNumDevs 返回 0）"
    );
    assert!(AudioError::OpenFailed { code: 32 }.to_string().contains("32"));
    assert!(AudioError::OpenFailed { code: 32 }.to_string().contains("waveOutOpen"));
    assert!(AudioError::AlreadyOpen.to_string().contains("AudioDevice"));
}

#[test]
fn t_dev05_bad_params_rejected_before_ffi() {
    // 无设备也可跑：参数检查发生在 FFI 之前，任何环境都应得到一致结果。
    let _guard = device_lock();
    let err = AudioDevice::open(empty_mixer(), 0, 2).unwrap_err();
    assert_eq!(err, AudioError::OpenFailed { code: 0 });
    let err = AudioDevice::open(empty_mixer(), 44100, 3).unwrap_err();
    assert_eq!(err, AudioError::OpenFailed { code: 0 });
    // 失败路径必须已释放占位：下面这次（若本机有设备）不得报 AlreadyOpen。
    if no_device_skip() {
        return;
    }
    let probe = AudioDevice::open(empty_mixer(), 44100, 2);
    assert!(probe.is_ok(), "参数失败后占位必须已释放");
    probe.unwrap().close();
}
