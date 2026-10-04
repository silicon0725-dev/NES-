//! winmm waveOut 设备输出：手写 FFI + `CALLBACK_NULL` 轮询 + 专属填充线程。
//!
//! # 为什么是 waveOut + `CALLBACK_NULL`
//!
//! * **waveOut** 是 Windows 上最老的可用 PCM 输出面（winmm.dll，Windows SDK
//!   自带 import 库，`#[link(name = "winmm")]` 开箱即链，无任何第三方依赖）；
//! * **`CALLBACK_NULL`**（无回调）+ 轮询是最简可靠形态：回调形态要求回调里
//!   只能碰"异步信号安全"的 API，容易写出自锁；轮询把时序完全收回我们手里。
//!
//! # 线程模型（全图）
//!
//! ```text
//! open()                              设备线程（唯一碰 winmm 句柄的线程）
//! ──────                              ────────────────────────────────
//! timeBeginPeriod(1)（RAII 守卫）      SetThreadPriority(HIGHEST)（自提）
//! waveOutGetNumDevs → 0 ⇒ NoDevice    loop {
//! waveOutOpen(WAVE_MAPPER,              对 4 个 20ms WAVEHDR 轮转：
//!   CALLBACK_NULL) ⇒ OpenFailed?          跳过仍在播的（dwFlags 无 WHDR_DONE）
//! Prepare × 4 → spawn 线程                全部 DONE ⇒ underruns()+1（诊断）
//!                                         mixer.lock().mix_into(缓冲, rate, ch)
//!                                       waveOutWrite 提交
//! close()/Drop                        } sleep(10ms)
//! ────────────                        } // 退出后，同线程串行：
//! stop=true → join                    waveOutReset（召回在播缓冲）
//! （守卫 Drop → timeEndPeriod）        waveOutUnprepareHeader × 4
//!                                     waveOutClose
//! ```
//!
//! 把 Reset / Unprepare / Close 放在**设备线程的收尾段**而不是 close() 里，
//! 是刻意的：所有 winmm 调用因此单线程串行，"句柄在 waveOutWrite 中途被
//! 另一线程 close"一类竞态从根上不存在；close() 只负责置停机位 + join。
//!
//! # 设备线程硬化（对 Windows 定时器分辨率陷阱）
//!
//! 用户实测症状：waveOut 播放间歇性停顿/爆音。根因链：Windows 默认系统
//! 定时器节拍 ~15.6ms ⇒ `thread::sleep(10ms)` 实际睡 10~15.6ms ⇒ 回填
//! 节拍与 4×10ms 队列同量级，任何调度抖动都吃穿余量，队列周期性打干
//! （soak 实测：旧参数 12 秒欠载 6 次）；系统节拍是否处于 1ms 取决于
//! **其它进程**（浏览器等）恰好拉高过分辨率——所以症状"间歇性"。
//! 三项组合修复（取舍细节在各定义处注释）：
//!
//! 1. **`timeBeginPeriod(1)`**（RAII 配对守卫，open 提升 / close·Drop 恢复）
//!    —— 把 sleep 抖动从 +5.6ms 压到 ~+1ms；
//! 2. **设备线程自提 `THREAD_PRIORITY_HIGHEST`**（线程函数体首行，伪句柄
//!    自提；不用实时档 TIME_CRITICAL 的理由见函数注释）—— 消除普通优先
//!    级下被宿主 CPU 脉冲拖后一个时间片的一项；
//! 3. **缓冲 4×10ms → 4×20ms**（80ms 排队深度）—— 10ms 节拍下单次抖动
//!    最多吃 10ms，余量 60ms（机制性 3-4 倍）；代价是音效触发延迟上界从
//!    ~10ms 变为 ~20ms（新声部从"下一个头"起混，最坏多等 10ms；首响差异
//!    人耳阈值 ~20-30ms，可感知性低）。
//!
//! 诊断：[`underruns()`] 统计队列打干次数，硬化回归门要求连续播放为 0
//! （`cargo run --release --example soak`）。
//!
//! # 为什么不升级 CALLBACK_EVENT 精确唤醒
//!
//! 事件等待（waveOutOpen 传事件句柄 + `WaitForSingleObject`）能消掉
//! sleep 本身的离散抖动，但要新增 CreateEvent/SetEvent/WaitForSingleObject
//! 三个 FFI 与跨线程事件语义。上面 1+3 落地后机制性余量已达 3-4 倍且
//! soak 实测 60 秒 0 欠载，收益不再覆盖改动面——保持"盲睡 + 肥余量"
//! 最简形态；若未来 soak 再现欠载，这是既定的下一刀。
//!
//! # 单设备约束
//!
//! 本 crate 同一时刻只允许一个 `AudioDevice`（进程级 `AtomicBool` 占位），
//! 第二次 open 报 [`AudioError::AlreadyOpen`]。引擎只需要一个输出口；
//! 占位在 close/Drop 时释放，open-close 反复配对不泄漏。
//!
//! # 失败纪律（照 GPU 用例惯例）
//!
//! 无设备 / 打开失败一律返回 [`AudioError`]，**不 panic、不静默换路**；
//! 测试在 `waveOutGetNumDevs() == 0` 的环境（CI / 无声卡）打印跳过说明，
//! 有设备时打开失败则直接判失败——"没有设备"与"有设备但跑不通"不许互装。

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::mixer::Mixer;

/// 累计欠载（underrun）计数：设备队列被打干的回填轮数（口径见
/// [`underruns`]）。进程级诊断计数，随设备开关累加、不重置。
static UNDERRUNS: AtomicU64 = AtomicU64::new(0);

/// 进程启动以来的欠载次数（诊断）。
///
/// 口径：回填轮询中发现**全部轮转头都已 DONE**（= 播放位置已越过待填
/// 数据、输出跨过一段最多一个回填节拍的静默）计 1 次。硬化的回归门：
/// 连续播放下必须为 0（见模块 doc「设备线程硬化」）。
pub fn underruns() -> u64 {
    UNDERRUNS.load(Ordering::Relaxed)
}

// ------------------------------------------------------------ winmm FFI
//
// 手写 `#[repr(C)]`（MSDN 口径），命名保留 Windows 原名以便对照文档；
// 全部 winmm 调用点集中在本模块，每个 unsafe 块带 SAFETY 注释。

#[allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]
mod winmm {
    use std::ffi::c_void;

    /// `MMRESULT`：winmm 的通用返回码，0 = 成功。
    pub type MMRESULT = u32;
    /// `HWAVEOUT`：wave 输出句柄（不透明指针）。
    pub type HWAVEOUT = *mut c_void;

    pub const MMSYSERR_NOERROR: MMRESULT = 0;
    /// 设备枚举占位：让系统挑选合适的输出设备。
    pub const WAVE_MAPPER: u32 = u32::MAX;
    /// 无回调：本 crate 用轮询写头，不需要窗口/线程/事件回调。
    pub const CALLBACK_NULL: u32 = 0x0000_0000;
    /// `WAVEHDR.dwFlags`：缓冲已播完（或从未提交），可安全回填重发。
    pub const WHDR_DONE: u32 = 0x0000_0001;
    /// `WAVEFORMATEX.wFormatTag`：未压缩 PCM。
    pub const WAVE_FORMAT_PCM: u16 = 1;

    /// MSDN `WAVEFORMATEX`（18 字节原样，不合并字段）。
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct WAVEFORMATEX {
        pub wFormatTag: u16,
        pub nChannels: u16,
        pub nSamplesPerSec: u32,
        pub nAvgBytesPerSec: u32,
        pub nBlockAlign: u16,
        pub wBitsPerSample: u16,
        pub cbSize: u16,
    }

    /// MSDN `WAVEHDR`（`dwUser`/`dwReserved` 是指针宽的 `DWORD_PTR`）。
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct WAVEHDR {
        pub lpData: *mut u8,
        pub dwBufferLength: u32,
        pub dwBytesRecorded: u32,
        pub dwUser: usize,
        pub dwFlags: u32,
        pub dwLoops: u32,
        pub lpNext: *mut WAVEHDR,
        pub dwReserved: usize,
    }

    #[link(name = "winmm")]
    extern "system" {
        pub fn waveOutGetNumDevs() -> u32;
        pub fn waveOutOpen(
            phwo: *mut HWAVEOUT,
            uDeviceID: u32,
            lpFormat: *const WAVEFORMATEX,
            dwCallback: usize,
            dwInstance: usize,
            fdwOpen: u32,
        ) -> MMRESULT;
        pub fn waveOutPrepareHeader(
            hwo: HWAVEOUT,
            pwh: *mut WAVEHDR,
            cbwh: u32,
        ) -> MMRESULT;
        pub fn waveOutWrite(hwo: HWAVEOUT, pwh: *mut WAVEHDR, cbwh: u32) -> MMRESULT;
        pub fn waveOutUnprepareHeader(
            hwo: HWAVEOUT,
            pwh: *mut WAVEHDR,
            cbwh: u32,
        ) -> MMRESULT;
        pub fn waveOutReset(hwo: HWAVEOUT) -> MMRESULT;
        pub fn waveOutClose(hwo: HWAVEOUT) -> MMRESULT;
        /// 系统定时器分辨率请求（毫秒）：Windows 默认节拍 ~15.6ms，把
        /// `thread::sleep` 的实际粒度从 15.6ms 压到 ~1ms（见模块 doc「设备
        /// 线程硬化」）。进程级引用计数由 OS 管：begin/end 必须配对。
        pub fn timeBeginPeriod(uPeriod: u32) -> MMRESULT;
        /// [`timeBeginPeriod`] 的配对撤销。
        pub fn timeEndPeriod(uPeriod: u32) -> MMRESULT;
    }

    // 结构体尺寸自检：改字段/改对齐立刻在编译期显形。
    // WAVEFORMATEX 按 MSDN 是 18 字节，repr(C) 自然对齐（含 u32）后尾补到 20；
    // 多出的 2 字节是纯尾部填充，Windows 侧只读前 18 字节。
    const _: () = assert!(size_of::<WAVEHDR>() == size_of::<usize>() * 4 + 16);
    const _: () = assert!(size_of::<WAVEFORMATEX>() == 20);
}

// ------------------------------------------------------------ kernel32 FFI
//
// 设备线程自提优先级（照 winmm 模块同一手写模式，零依赖）。

#[allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]
mod kernel32 {
    use std::ffi::c_void;

    /// `HANDLE`：内核对象句柄（本模块只用到线程伪句柄）。
    pub type HANDLE = *mut c_void;

    /// 线程优先级"最高"（= 2，普通档顶格，比 NORMAL 高两档）。注意**不是**
    /// 实时档 `THREAD_PRIORITY_TIME_CRITICAL`（= 15）——取舍见线程函数注释。
    pub const THREAD_PRIORITY_HIGHEST: i32 = 2;

    #[link(name = "kernel32")]
    extern "system" {
        /// 返回**调用线程**的伪句柄（内部常量 -2，只能在本线程内使用——
        /// 本 crate 恰好只在设备线程内自提优先级，窗口合法）。
        pub fn GetCurrentThread() -> HANDLE;
        /// 设置线程优先级：成功返回非 0。失败尽力而为（不分支、不报错）。
        pub fn SetThreadPriority(hThread: HANDLE, nPriority: i32) -> i32;
    }
}

/// 当前系统可见的 waveOut 输出设备数（0 = 无设备，测试据此跳过冒烟用例）。
pub fn waveout_device_count() -> u32 {
    // SAFETY: waveOutGetNumDevs 无前置条件，纯查询。
    unsafe { winmm::waveOutGetNumDevs() }
}

/// 设备层可报告的失败点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    /// 系统没有任何 waveOut 输出设备（`waveOutGetNumDevs() == 0`）。
    NoDevice,
    /// 打开失败：`code` 是 winmm `MMRESULT`（32 = WAVERR_BADFORMAT，
    /// 6 = MMSYSERR_NODRIVER，4 = MMSYSERR_ALLOCATED；0 表示在 FFI 之前
    /// 就被参数检查拒绝）。
    OpenFailed { code: u32 },
    /// 进程内已有一个 `AudioDevice` 未关闭（本 crate 同一时刻只允许一个）。
    AlreadyOpen,
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoDevice => {
                write!(f, "音频设备：没有可用的 waveOut 输出设备（waveOutGetNumDevs 返回 0）")
            }
            AudioError::OpenFailed { code } => {
                write!(f, "音频设备：waveOutOpen 失败（MMRESULT = {code}）")
            }
            AudioError::AlreadyOpen => {
                write!(f, "音频设备：进程内已有一个 AudioDevice 未关闭（同一时刻只允许一个）")
            }
        }
    }
}

impl std::error::Error for AudioError {}

/// 进程级单设备占位：false = 空闲，true = 已被某个 `AudioDevice` 持有。
static DEVICE_HELD: AtomicBool = AtomicBool::new(false);

/// 单头时长（毫秒）：4 头 × 20ms = **80ms 排队深度**。硬化取值（原 10ms
/// 的取舍见模块 doc「设备线程硬化」）：10ms 回填节拍下最坏调度抖动至多吃
/// 掉 10ms，80ms 深度仍有 ~60ms 余量（3-4 倍机制性裕度）；代价是音效
/// 触发延迟上界 +20ms（新声部最晚等一个 20ms 头起混），可感知性低。
const BUFFER_MS: u64 = 20;
/// 轮转头数：4 头环形；10ms 节拍下每两拍必有一头播完，不空转。
const HEADER_COUNT: usize = 4;
/// 填充节拍（毫秒）：单头时长的一半，回填探测延迟上界可控、占用低。
const TICK: Duration = Duration::from_millis(10);

// ------------------------------------------------------------ 硬化设施

/// 系统定时器分辨率请求（RAII 配对守卫）：构造即 `timeBeginPeriod(1)`，
/// Drop 即 `timeEndPeriod(1)`。
///
/// Windows 默认系统定时器节拍 ~15.6ms：`thread::sleep(10ms)` 实际睡
/// 15.6ms，旧 4×10ms 缓冲在该节拍下回填追不上播放（结构性欠载，soak
/// 实测 12 秒 6 次）。`timeBeginPeriod(1)` 把节拍压到 ~1ms，sleep 抖动
/// 从 +5.6ms 收敛到 +1ms 量级。**配对纪律**：引用计数进程级由 OS 管，
/// [`AudioDevice::open`] 构造守卫、close/Drop 经字段 Drop 撤销——即便
/// 设备线程中途 panic，字段清理照样恢复系统分辨率。
#[derive(Debug)]
struct TimerResolution;

impl TimerResolution {
    /// 请求 1ms 分辨率。失败（`TIMERR_NOCANDO`）尽力而为忽略：播放只是
    /// 维持系统默认分辨率（欠载风险回升），不算 open 失败。
    fn request() -> Self {
        // SAFETY: 1 是 MSDN 允许范围（1..=15）内的合法值；begin 与 Drop 中
        // 的 end 严格配对，多出的 end（begin 失败时）对 OS 引用计数无害。
        unsafe {
            winmm::timeBeginPeriod(1);
        }
        TimerResolution
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        // SAFETY: 与 request() 的 timeBeginPeriod(1) 配对；即便 begin 失败
        // 也无害（未持有引用计数时 end 只是空转返回）。
        unsafe {
            winmm::timeEndPeriod(1);
        }
    }
}

// ------------------------------------------------------------ 设备线程

/// `HWAVEOUT` 的独占所有权包装：把句柄从 `open()` 转移给设备线程。
///
/// # SAFETY（Send 的依据）
///
/// wave 句柄按 MSDN 可在任意线程使用（`CALLBACK_NULL` 无线程亲和性）；
/// 本类型只此一处：`open()` 之后主线程**不再触碰**句柄，全部 winmm 调用
/// 都发生在设备线程上，因此跨线程转移的是独占所有权，无共享可竞争。
struct SendHandle(winmm::HWAVEOUT);
unsafe impl Send for SendHandle {}

/// 一个轮转槽：已 Prepare 的头 + 它指向的样本缓冲 + 是否已提交过。
struct Slot {
    hdr: Box<winmm::WAVEHDR>,
    data: Box<[i16]>,
    submitted: bool,
}

/// `open()` 移交给设备线程的全套运行参数。
struct ThreadMsg {
    handle: SendHandle,
    mixer: Arc<Mutex<Mixer>>,
    device_rate: u32,
    channels: u16,
    samples_per_buffer: usize,
}

/// 设备线程主循环 + 收尾关闭序列。**所有 winmm 调用都在本线程串行发生**。
///
/// # SAFETY（调用约定）
///
/// `handle` 必须来自 `open()` 里成功的 `waveOutOpen`，且调用本函数后
/// 原线程不得再使用它（所有权已转移，见 [`SendHandle`]）。
/// `slots` 的样本缓冲在线程内一次分配、永不移动（`Box` 堆址稳定），
/// `hdr.lpData` 因此在 Prepare → Write → Unprepare 全程有效。
fn device_thread(
    stop: Arc<AtomicBool>,
    handle: winmm::HWAVEOUT,
    mixer: Arc<Mutex<Mixer>>,
    device_rate: u32,
    channels: u16,
    samples_per_buffer: usize,
) {
    // 自提优先级：本线程每 ~10ms 醒一次、每次只干几十微秒的活；普通优先级
    // 下宿主/渲染线程的 CPU 脉冲会把单次唤醒拖后一个时间片，与 sleep 抖动
    // 复合叠加吃队列余量。提到底（HIGHEST = 2，普通档顶格）消除这一项。
    // **有意不用 TIME_CRITICAL（= 15，实时档）**：它能让本线程在出 bug
    // 忙旋时饿死全进程其余线程；缓冲硬化后机制性余量已有 3-4 倍，优先级
    // 只需兜单次调度抖动，HIGHEST 的收益/风险比更优。
    // SAFETY: GetCurrentThread 返回本线程伪句柄，只在调用线程内有效——
    // 这里取到即用、同线程消费，无跨线程传递；失败（返回 0）尽力而为忽略。
    unsafe {
        kernel32::SetThreadPriority(
            kernel32::GetCurrentThread(),
            kernel32::THREAD_PRIORITY_HIGHEST,
        );
    }

    let hdr_bytes = size_of::<winmm::WAVEHDR>() as u32;
    let mut slots: Vec<Slot> = (0..HEADER_COUNT)
        .map(|_| {
            let mut data = vec![0i16; samples_per_buffer].into_boxed_slice();
            let mut hdr = Box::new(winmm::WAVEHDR {
                lpData: data.as_mut_ptr().cast::<u8>(),
                dwBufferLength: (data.len() * 2) as u32,
                dwBytesRecorded: 0,
                dwUser: 0,
                dwFlags: 0,
                dwLoops: 0,
                lpNext: std::ptr::null_mut(),
                dwReserved: 0,
            });
            // SAFETY: handle 有效；hdr 指向的缓冲在本线程存活到 Unprepare 之后。
            unsafe {
                winmm::waveOutPrepareHeader(handle, &mut *hdr, hdr_bytes);
            }
            Slot { hdr, data, submitted: false }
        })
        .collect();

    // 填充循环：跳过仍在播的头，回填播完的头并重提交。
    // 欠载口径：本轮 4 个头**全部** DONE（队列已干，此前输出跨过静默）
    // 且曾经提交过（排除启动首轮的必然全空），计 1 次 —— 见 [`underruns`]。
    let mut ever_submitted = false;
    while !stop.load(Ordering::Relaxed) {
        let mut refillable = 0usize;
        for slot in slots.iter_mut() {
            if slot.submitted && slot.hdr.dwFlags & winmm::WHDR_DONE == 0 {
                continue; // 该缓冲还在设备队列里
            }
            refillable += 1;
            // 毒化容忍：音频不该因为别处 panic 时恰好握着锁而整条哑掉。
            let mut guard = mixer.lock().unwrap_or_else(PoisonError::into_inner);
            guard.mix_into(&mut slot.data, device_rate, channels);
            drop(guard);
            slot.hdr.dwBufferLength = (slot.data.len() * 2) as u32;
            // SAFETY: handle 有效；hdr 已 Prepare，且不在设备队列中（WHDR_DONE
            // 或从未提交），按 MSDN 此时重填长度再 waveOutWrite 合法。
            unsafe {
                winmm::waveOutWrite(handle, &mut *slot.hdr, hdr_bytes);
            }
            slot.submitted = true;
        }
        if ever_submitted && refillable == HEADER_COUNT {
            UNDERRUNS.fetch_add(1, Ordering::Relaxed);
        }
        ever_submitted = true;
        thread::sleep(TICK);
    }

    // 收尾关闭序列（与模块 doc 的全图一一对应）：Reset 召回在播缓冲 →
    // Unprepare × N → Close。同线程串行，无竞态窗口。
    // SAFETY: handle 有效且此后立即 Close，本线程不再使用。
    unsafe {
        winmm::waveOutReset(handle);
        for slot in slots.iter_mut() {
            winmm::waveOutUnprepareHeader(handle, &mut *slot.hdr, hdr_bytes);
        }
        winmm::waveOutClose(handle);
    }
}

// ------------------------------------------------------------ 公开设备

/// 一个打开的 waveOut 输出设备：内部线程每 ~10ms 用 [`Mixer::mix_into`]
/// 轮转填充 4 个 20ms 环形缓冲并提交（硬化参数，见模块 doc）。
///
/// `close(self)`（或 `Drop`）置停机位并 join 线程；Reset / Unprepare / Close
/// 由设备线程收尾段执行（见模块 doc 的线程模型）。重复 close 安全：
/// `Drop` 看到已关闭即空操作。
#[derive(Debug)]
pub struct AudioDevice {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    device_rate: u32,
    channels: u16,
    open: bool,
    /// 定时器分辨率配对守卫：open 时提升、close/Drop（先 join 线程、
    /// 后字段清理）时恢复 —— panic 路径也由 Drop 保证撤销。字段只为
    /// Drop 副作用存在，读取无意义。
    #[allow(dead_code)]
    timer: TimerResolution,
}

impl AudioDevice {
    /// 打开默认输出设备并启动填充线程。
    ///
    /// * `mixer`：与调用方共享的混音器（设备线程每个 tick 锁一次、填一个缓冲）；
    /// * `device_rate`：设备采样率（>0）；`channels`：1 或 2；非法参数在
    ///   FFI 之前拒绝（[`AudioError::OpenFailed`] 的 `code = 0` 口径）。
    ///
    /// 失败路径（[`AudioError`]）不 panic；同一时刻只允许一个设备
    /// （[`AudioError::AlreadyOpen`]）。
    pub fn open(
        mixer: Arc<Mutex<Mixer>>,
        device_rate: u32,
        channels: u16,
    ) -> Result<Self, AudioError> {
        if DEVICE_HELD
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(AudioError::AlreadyOpen);
        }
        // 失败路径统一从这里退出（释放占位）。
        let fail = |code: u32| {
            DEVICE_HELD.store(false, Ordering::SeqCst);
            AudioError::OpenFailed { code }
        };

        if device_rate == 0 || (channels != 1 && channels != 2) {
            return Err(fail(0));
        }
        if waveout_device_count() == 0 {
            DEVICE_HELD.store(false, Ordering::SeqCst);
            return Err(AudioError::NoDevice);
        }

        // 先起线程、再开设备：spawn 失败发生在句柄存在之前，免去回收路径；
        // 句柄与混音器随后经通道独占移交（SendError 会把失败的消息原样弹回）。
        let (tx, rx) = mpsc::channel::<ThreadMsg>();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let spawned = thread::Builder::new()
            .name("nes-audio-waveout".into())
            .spawn(move || {
                // 通道关闭（open 提前失败）则无事可做，直接退出。
                if let Ok(msg) = rx.recv() {
                    // SAFETY: 句柄所有权经 SendHandle 独占转移给本线程。
                    device_thread(
                        worker_stop,
                        msg.handle.0,
                        msg.mixer,
                        msg.device_rate,
                        msg.channels,
                        msg.samples_per_buffer,
                    );
                }
            });
        let worker: JoinHandle<()> = match spawned {
            Ok(w) => w,
            // 此刻尚无句柄可泄漏。
            Err(_) => return Err(fail(0)),
        };

        let block_align = channels * 2; // 16-bit PCM
        let format = winmm::WAVEFORMATEX {
            wFormatTag: winmm::WAVE_FORMAT_PCM,
            nChannels: channels,
            nSamplesPerSec: device_rate,
            nAvgBytesPerSec: device_rate * u32::from(block_align),
            nBlockAlign: block_align,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut handle: winmm::HWAVEOUT = std::ptr::null_mut();
        // SAFETY: format/句柄指针都是本栈上的有效对象；CALLBACK_NULL 不注册回调。
        let rc = unsafe {
            winmm::waveOutOpen(
                &mut handle,
                winmm::WAVE_MAPPER,
                &format,
                0,
                0,
                winmm::CALLBACK_NULL,
            )
        };
        if rc != winmm::MMSYSERR_NOERROR || handle.is_null() {
            return Err(fail(rc));
        }

        // ~20ms/头（[`BUFFER_MS`]）：48000 → 960 帧；极小采样率至少 1 帧防 0 长度。
        let frames_per_buffer = ((u64::from(device_rate) * BUFFER_MS) / 1000).max(1) as usize;
        let samples_per_buffer = frames_per_buffer * usize::from(channels);

        let msg = ThreadMsg {
            handle: SendHandle(handle),
            mixer,
            device_rate,
            channels,
            samples_per_buffer,
        };
        if let Err(bounced) = tx.send(msg) {
            // 设备线程没能接管（意外退出）：在当前线程完成收尾（与线程收尾同序）。
            // SAFETY: handle 有效，此后不再使用。
            let SendHandle(handle) = bounced.0.handle;
            unsafe {
                winmm::waveOutReset(handle);
                winmm::waveOutClose(handle);
            }
            return Err(fail(0));
        }

        Ok(Self {
            stop,
            worker: Some(worker),
            device_rate,
            channels,
            open: true,
            // 定时器提升守卫在移交成功后才构造：send 失败路径（设备线程
            // 没接住）不经过这里，begin/end 天然配对、无泄漏。
            timer: TimerResolution::request(),
        })
    }

    /// 设备是否处于打开状态（close 之后为 false；Drop 后对象不复存在）。
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 打开时约定的设备采样率。
    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// 打开时约定的设备声道数。
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// 关闭设备：置停机位 → 设备线程执行 Reset + Unprepare × N + Close → join。
    /// 消费 `self`；之后的 `Drop` 是空操作。
    pub fn close(mut self) {
        self.shutdown();
    }

    /// close/Drop 共用的停机序列；幂等（`open=false` 后不再动作）。
    fn shutdown(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            // 线程收尾段保证在 join 返回前完成全部 winmm 清理。
            let _ = worker.join();
        }
        DEVICE_HELD.store(false, Ordering::SeqCst);
    }
}

impl Drop for AudioDevice {
    fn drop(&mut self) {
        self.shutdown();
    }
}
