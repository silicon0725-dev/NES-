//! 输入契约（S7.2）：**平台输入 → 帧输入快照**的纯数据层。
//!
//! # 分层（与渲染契约同一条纪律）
//!
//! ```text
//! Platform Input（nes-render-wgpu：Win32 消息 → 中性 InputEvent）
//!       ↓
//! Input Collector（本模块：事件流 → 帧边缘/状态折叠，纯函数）
//!       ↓
//! Frame Input Snapshot（InputSnapshot：held/pressed/released/mouse/text）
//!       ↓
//! Runtime（collect_input + input/* 信号 + 脚本 key() 探针）
//!       ↓
//! Script / UI / Game
//! ```
//!
//! 四路**分开**：Keyboard（键集 + 边缘）/ Mouse（位置 + 增量 + 按钮边缘）/
//! TextInput（WM_CHAR 提交的字符流，与键语义分离 —— IME 合成不走键）/ 
//! Window（尺寸变化，输入性质的事件单列）。**WM_CHAR 不是引擎 API**：
//! 平台字符码只到这里为止，宿主与脚本消费的是快照的 `text` 字段。
//!
//! 零依赖、无平台概念：`InputEvent` 已是中性事件（虚拟键 → `Key` 的
//! 映射在平台层完成）。宿主可不经平台直接注入事件（自动化/回放/
//! headless 合成输入）。

use std::collections::BTreeSet;

use crate::math::Vec2;

/// 中性键位（平台虚拟键的引擎口径；`Other` 保留未列举的原码）。
#[allow(missing_docs)] // 逐键自明（名字即文档，见 `name()`）
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    // 字母（A..Z）
    A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    // 数字行（0..9）
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    // 方向
    ArrowLeft, ArrowRight, ArrowUp, ArrowDown,
    // 编辑/控制
    Space, Enter, Escape, Backspace, Tab,
    // 修饰（左右分开 —— 需要合并语义的宿主自行 or）
    LShift, RShift, LCtrl, RCtrl, LAlt, RAlt,
    /// 未列举键（保留平台原码；名字 `Other(码)`，`from_name` 不解析）。
    Other(u32),
}

impl Key {
    /// 稳定名（`input/*` 信号载荷与脚本 `key("名")` 探针共用这一口径）。
    pub fn name(&self) -> String {
        let s = match self {
            Key::A => "A", Key::B => "B", Key::C => "C", Key::D => "D", Key::E => "E",
            Key::F => "F", Key::G => "G", Key::H => "H", Key::I => "I", Key::J => "J",
            Key::K => "K", Key::L => "L", Key::M => "M", Key::N => "N", Key::O => "O",
            Key::P => "P", Key::Q => "Q", Key::R => "R", Key::S => "S", Key::T => "T",
            Key::U => "U", Key::V => "V", Key::W => "W", Key::X => "X", Key::Y => "Y",
            Key::Z => "Z",
            Key::Num0 => "Num0", Key::Num1 => "Num1", Key::Num2 => "Num2",
            Key::Num3 => "Num3", Key::Num4 => "Num4", Key::Num5 => "Num5",
            Key::Num6 => "Num6", Key::Num7 => "Num7", Key::Num8 => "Num8",
            Key::Num9 => "Num9",
            Key::ArrowLeft => "ArrowLeft", Key::ArrowRight => "ArrowRight",
            Key::ArrowUp => "ArrowUp", Key::ArrowDown => "ArrowDown",
            Key::Space => "Space", Key::Enter => "Enter", Key::Escape => "Escape",
            Key::Backspace => "Backspace", Key::Tab => "Tab",
            Key::LShift => "LShift", Key::RShift => "RShift",
            Key::LCtrl => "LCtrl", Key::RCtrl => "RCtrl",
            Key::LAlt => "LAlt", Key::RAlt => "RAlt",
            Key::Other(v) => return format!("Other({v})"),
        };
        s.to_string()
    }

    /// 名字反解（探针口径；未知名返回 `None` —— 未列举即未按）。
    pub fn from_name(name: &str) -> Option<Key> {
        Some(match name {
            "A" => Key::A, "B" => Key::B, "C" => Key::C, "D" => Key::D, "E" => Key::E,
            "F" => Key::F, "G" => Key::G, "H" => Key::H, "I" => Key::I, "J" => Key::J,
            "K" => Key::K, "L" => Key::L, "M" => Key::M, "N" => Key::N, "O" => Key::O,
            "P" => Key::P, "Q" => Key::Q, "R" => Key::R, "S" => Key::S, "T" => Key::T,
            "U" => Key::U, "V" => Key::V, "W" => Key::W, "X" => Key::X, "Y" => Key::Y,
            "Z" => Key::Z,
            "Num0" => Key::Num0, "Num1" => Key::Num1, "Num2" => Key::Num2,
            "Num3" => Key::Num3, "Num4" => Key::Num4, "Num5" => Key::Num5,
            "Num6" => Key::Num6, "Num7" => Key::Num7, "Num8" => Key::Num8,
            "Num9" => Key::Num9,
            "ArrowLeft" => Key::ArrowLeft, "ArrowRight" => Key::ArrowRight,
            "ArrowUp" => Key::ArrowUp, "ArrowDown" => Key::ArrowDown,
            "Space" => Key::Space, "Enter" => Key::Enter, "Escape" => Key::Escape,
            "Backspace" => Key::Backspace, "Tab" => Key::Tab,
            "LShift" => Key::LShift, "RShift" => Key::RShift,
            "LCtrl" => Key::LCtrl, "RCtrl" => Key::RCtrl,
            "LAlt" => Key::LAlt, "RAlt" => Key::RAlt,
            _ => return None,
        })
    }
}

/// 鼠标按钮（三键最小集；滚轮属后续）。
#[allow(missing_docs)] // 逐键自明
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    /// 稳定名（信号载荷口径）。
    pub fn name(&self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
        }
    }

    /// 名字反解。
    pub fn from_name(name: &str) -> Option<MouseButton> {
        match name {
            "left" => Some(MouseButton::Left),
            "right" => Some(MouseButton::Right),
            "middle" => Some(MouseButton::Middle),
            _ => None,
        }
    }

    /// 按钮在 `[bool; 3]` 里的下标（Left/Right/Middle 顺序）。
    pub fn index(&self) -> usize {
        match self {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
        }
    }
}

/// 中性输入事件（平台层映射产物；宿主可直注 —— 自动化/headless）。
#[allow(missing_docs)] // 变体字段自明（见变体文档）
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputEvent {
    /// 键按下/抬起（自动重发的 down 由集合语义自然幂等 —— 快照边缘按
    /// 集合差算，重复 down 不产生第二次 pressed）。
    Key { key: Key, down: bool },
    /// 文本输入字符（UTF-16 单元原码；与键语义分离）。
    Char(u32),
    /// 鼠标移动（客户区像素）。
    MouseMove { x: f32, y: f32 },
    /// 鼠标按钮按下/抬起。
    MouseButton { button: MouseButton, down: bool },
    /// 客户区尺寸变化（输入性质的单列事件；表面重配置另属里程碑）。
    Resize { w: u32, h: u32 },
}

/// 一帧的输入快照：**边缘（本帧）+ 状态（按住）** 两类口径并存 ——
/// 事件式消费者读边缘（或消费 `input/*` 信号），轮询式消费者读状态
///（脚本 `key("W")` 探针）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputSnapshot {
    /// 按住中的键（含本帧刚按下的）。
    pub held: BTreeSet<Key>,
    /// 本帧按下（边缘 = curr − prev；自动重发不重复计）。
    pub pressed: BTreeSet<Key>,
    /// 本帧抬起（边缘 = prev − curr）。
    pub released: BTreeSet<Key>,
    /// 鼠标位置（客户区像素；尚无移动事件时 (0,0)）。
    pub mouse: Vec2,
    /// 本帧鼠标位移（相对上一快照；首次为 (0,0)）。
    pub mouse_delta: Vec2,
    /// 按钮按住（Left/Right/Middle 顺序）。
    pub buttons_held: [bool; 3],
    /// 按钮本帧按下。
    pub buttons_pressed: [bool; 3],
    /// 按钮本帧抬起。
    pub buttons_released: [bool; 3],
    /// 本帧提交的文本字符（UTF-16 单元序；快照取走即清）。
    pub text: Vec<u32>,
    /// 本帧客户区尺寸变化（快照取走即清）。
    pub resized: Option<(u32, u32)>,
}

impl InputSnapshot {
    /// 键是否按住（探针口径：未列举名 = 未按，不猜）。
    pub fn is_down(&self, name: &str) -> bool {
        Key::from_name(name).is_some_and(|k| self.held.contains(&k))
    }
}

/// 输入折叠器：事件流 → 帧快照。纯状态机，无平台/线程概念
///（平台队列在 nes-render-wgpu，宿主每帧 drain 后喂进来）。
///
/// **边缘口径（T-In-C01 实证修正）**：pressed 在 down 事件上**闩锁**
///（同帧内按下又抬起 = 一次脉冲，不因帧边界错过而消失）；重发的 down
/// （键已按住）不重复闩锁；released 只记**上帧末按住**的键的抬起
///（凭空 up 不算，同帧脉冲不产生 released —— 消费者看到的是
/// `pressed=true, released=false, held=false` 的干净单帧脉冲）。
#[derive(Default)]
pub struct InputCollector {
    prev_keys: BTreeSet<Key>,
    curr_keys: BTreeSet<Key>,
    pressed_latch: BTreeSet<Key>,
    released_latch: BTreeSet<Key>,
    mouse: Vec2,
    have_mouse: bool,
    /// 上一帧快照已有位置基准（首次观测只建基准，不算增量 ——
    /// 窗口收到的第一个移动事件前鼠标"本来就在那"，跳变是伪影）。
    have_baseline: bool,
    last_mouse: Vec2,
    prev_buttons: [bool; 3],
    curr_buttons: [bool; 3],
    buttons_pressed_latch: [bool; 3],
    buttons_released_latch: [bool; 3],
    text: Vec<u32>,
    resized: Option<(u32, u32)>,
}

impl InputCollector {
    /// 空折叠器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 折叠一个事件（平台 drain 或宿主直注）。
    pub fn push(&mut self, ev: InputEvent) {
        match ev {
            InputEvent::Key { key, down } => {
                if down {
                    if !self.curr_keys.contains(&key) {
                        self.pressed_latch.insert(key); // 新按下（非重发）
                    }
                    self.curr_keys.insert(key);
                } else {
                    self.curr_keys.remove(&key);
                    if self.prev_keys.contains(&key) {
                        self.released_latch.insert(key); // 上帧末按住的真抬起
                    }
                }
            }
            InputEvent::Char(c) => self.text.push(c),
            InputEvent::MouseMove { x, y } => {
                self.mouse = Vec2::new(x, y);
                self.have_mouse = true;
            }
            InputEvent::MouseButton { button, down } => {
                let i = button.index();
                if down {
                    if !self.curr_buttons[i] {
                        self.buttons_pressed_latch[i] = true;
                    }
                    self.curr_buttons[i] = true;
                } else {
                    self.curr_buttons[i] = false;
                    if self.prev_buttons[i] {
                        self.buttons_released_latch[i] = true;
                    }
                }
            }
            InputEvent::Resize { w, h } => self.resized = Some((w, h)),
        }
    }

    /// 结帧：取走闩锁边缘、清一次性数据，返回本帧快照（`prev = curr`
    /// 前移）。`held` 是**帧末状态**（同帧按下又抬起 → held=false，
    /// 但 pressed 仍如实记录这次脉冲）。
    pub fn frame(&mut self) -> InputSnapshot {
        let mouse_delta = if self.have_mouse && self.have_baseline {
            Vec2::new(self.mouse.x - self.last_mouse.x, self.mouse.y - self.last_mouse.y)
        } else {
            Vec2::new(0.0, 0.0)
        };
        let snap = InputSnapshot {
            held: self.curr_keys.clone(),
            pressed: std::mem::take(&mut self.pressed_latch),
            released: std::mem::take(&mut self.released_latch),
            mouse: self.mouse,
            mouse_delta,
            buttons_held: self.curr_buttons,
            buttons_pressed: std::mem::take(&mut self.buttons_pressed_latch),
            buttons_released: std::mem::take(&mut self.buttons_released_latch),
            text: std::mem::take(&mut self.text),
            resized: self.resized.take(),
        };
        self.prev_keys = self.curr_keys.clone();
        self.have_baseline |= self.have_mouse;
        self.last_mouse = self.mouse;
        self.prev_buttons = self.curr_buttons;
        snap
    }
}
