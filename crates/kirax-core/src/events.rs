//! 输入事件定义与无锁环形缓冲。
//!
//! 设计要点：
//! - 内核的 USB 键盘/鼠标驱动把 HID 报文翻译成 [`InputEvent`]，写入“输入环形缓冲”；
//! - 窗口系统/用户态应用从缓冲读取事件，完成命中测试与分发；
//! - [`RingBuffer`] 是单生产者/单消费者的无锁环形缓冲，生产者在中断上下文，
//!   消费者在进程上下文，无需加锁（使用 volatile 读写 + 序号）。

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 修饰键位掩码
pub mod mods {
    pub const NONE: u32 = 0;
    pub const SHIFT: u32 = 1 << 0;
    pub const CTRL: u32 = 1 << 1;
    pub const ALT: u32 = 1 << 2;
    pub const META: u32 = 1 << 3; // Win/Command
    pub const CAPS: u32 = 1 << 4;
}

/// 鼠标按键掩码
pub mod buttons {
    pub const LEFT: u8 = 1 << 0;
    pub const RIGHT: u8 = 1 << 1;
    pub const MIDDLE: u8 = 1 << 2;
}

/// 事件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EventKind {
    /// 鼠标移动（x, y 为绝对屏幕坐标）
    MouseMove = 0,
    /// 鼠标按键按下
    MouseDown = 1,
    /// 鼠标按键抬起
    MouseUp = 2,
    /// 滚轮（wheel_delta 为滚动行数，可正负）
    MouseWheel = 3,
    /// 键盘按下（key 为 scancode）
    KeyDown = 4,
    /// 键盘抬起
    KeyUp = 5,
    /// 文本输入（key 为 Unicode 码点，仅当能映射时）
    TextInput = 6,
    /// 触摸/触控板手势（预留）
    Gesture = 7,
}

/// 一个输入事件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct InputEvent {
    pub kind: EventKind,
    /// 屏幕绝对坐标（鼠标事件有效）
    pub x: u16,
    pub y: u16,
    /// 键盘 scancode 或 Unicode 码点
    pub key: u16,
    /// 修饰键掩码，见 [`mods`]
    pub modifiers: u32,
    /// 滚轮增量
    pub wheel_delta: i16,
    /// 单调时钟 tick（内核定时器提供）
    pub timestamp: u64,
}

impl InputEvent {
    pub const fn new(kind: EventKind) -> Self {
        Self {
            kind,
            x: 0,
            y: 0,
            key: 0,
            modifiers: mods::NONE,
            wheel_delta: 0,
            timestamp: 0,
        }
    }

    pub const fn with_pos(mut self, x: u16, y: u16) -> Self {
        self.x = x;
        self.y = y;
        self
    }

    pub const fn with_key(mut self, key: u16) -> Self {
        self.key = key;
        self
    }
}

/// 单生产者/单消费者无锁环形缓冲。
///
/// 容量必须是 2 的幂。生产者（中断）调用 [`push`](Self::push)，
/// 消费者（进程）调用 [`pop`](Self::pop)。
/// 使用 `head`（生产序号）与 `tail`（消费序号）两个原子计数器实现无锁。
pub struct RingBuffer<T, const N: usize>
where
    T: Copy,
{
    /// 槽位用 MaybeUninit 包裹：const 构造无需 T::Default；
    /// 读取只发生在写入之后（由 head/tail 序号保证），因此是健全的。
    buf: [MaybeUninit<T>; N],
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl<T, const N: usize> RingBuffer<T, N>
where
    T: Copy,
{
    /// 创建空缓冲（槽位未初始化，push 前不可读）
    pub const fn new() -> Self {
        Self {
            buf: [MaybeUninit::uninit(); N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// 已占用条目数
    pub fn len(&self) -> usize {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Relaxed);
        h.wrapping_sub(t) & (N - 1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() == N - 1
    }

    /// 生产者入队。满时返回 None（丢弃最旧策略由调用方决定）。
    pub fn push(&self, item: T) -> Option<()> {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Relaxed);
        if h.wrapping_sub(t) & (N - 1) == N - 1 {
            return None; // 满
        }
        // 关键：必须先写数据，再发布 head（Release 语义）
        let idx = h & (N - 1);
        // &self 下通过裸指针写（MaybeUninit 槽位），与 pop 的序号约束配对保证健全性
        unsafe {
            core::ptr::write(self.buf.as_ptr().add(idx) as *mut T, item);
        }
        self.head
            .store(h.wrapping_add(1), Ordering::Release);
        Some(())
    }

    /// 消费者出队。空时返回 None。
    pub fn pop(&self) -> Option<T> {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if t == h {
            return None;
        }
        let idx = t & (N - 1);
        let item = unsafe { core::ptr::read(self.buf.as_ptr().add(idx) as *const T) };
        self.tail
            .store(t.wrapping_add(1), Ordering::Release);
        Some(item)
    }

    /// 清空（仅消费者可调用）
    pub fn clear(&self) {
        self.tail
            .store(self.head.load(Ordering::Relaxed), Ordering::Relaxed);
    }
}

impl<T, const N: usize> Default for RingBuffer<T, N>
where
    T: Copy,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_roundtrip() {
        let rb = RingBuffer::<u32, 8>::new();
        for i in 0..7 {
            assert!(rb.push(i).is_some());
        }
        assert!(rb.is_full());
        // 满时拒绝
        assert!(rb.push(99).is_none());
        for i in 0..7 {
            assert_eq!(rb.pop(), Some(i));
        }
        assert!(rb.is_empty());
        assert_eq!(rb.pop(), None);
    }

    #[test]
    fn wrap_around() {
        // 验证序号回绕后仍正确
        let rb = RingBuffer::<u32, 4>::new();
        for i in 0..3 {
            rb.push(i);
        }
        assert_eq!(rb.pop(), Some(0));
        assert_eq!(rb.pop(), Some(1));
        rb.push(10);
        rb.push(11);
        assert_eq!(rb.pop(), Some(2));
        assert_eq!(rb.pop(), Some(10));
        assert_eq!(rb.pop(), Some(11));
        assert_eq!(rb.pop(), None);
    }

    #[test]
    fn event_flow() {
        let rb = RingBuffer::<InputEvent, 16>::new();
        let ev = InputEvent::new(EventKind::MouseMove).with_pos(320, 240);
        rb.push(ev);
        let got = rb.pop().unwrap();
        assert_eq!(got.kind, EventKind::MouseMove);
        assert_eq!((got.x, got.y), (320, 240));
    }
}
