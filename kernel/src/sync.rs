//! 极简自旋锁（无第三方依赖）。
//!
//! 树莓派 2 为多核（4×Cortex-A7），共享结构必须加锁。
//! 使用 ARMv7 的 `exclusive` 原子指令实现：
//! - `ldrex` 读取并独占监视
//! - `strex` 条件写入；失败则重试
//! - 等待时用 `wfe` 降低功耗（配合 `sev` 唤醒）
//!
//! 注意：本锁为**内核态互斥**（不睡眠），临界区必须极短；
//! 调度器持有的“睡眠锁”在 Phase 2 的调度器模块另行实现。

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// 只有 T 自身线程安全时锁才安全（锁保证互斥，但 T 必须 Send）
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// 获取锁（忙等）
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // ARMv7: WFE 等待事件（其他核释放时 sev 唤醒）
            unsafe { core::arch::asm!("wfe") };
        }
        SpinLockGuard { lock: self }
    }

    /// 尝试获取锁，失败返回 None（非阻塞）
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinLockGuard { lock: self })
        } else {
            None
        }
    }

    /// 直接访问（仅用于初始化阶段/单核上下文）
    pub unsafe fn get_unchecked(&self) -> &T {
        &*self.data.get()
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<'a, T> core::ops::Deref for SpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> core::ops::DerefMut for SpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        // 释放并唤醒等待者（ARMv7: sev）
        self.lock.locked.store(false, Ordering::Release);
        unsafe { core::arch::asm!("sev") };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_roundtrip() {
        let l = SpinLock::new(42u32);
        {
            let mut g = l.lock();
            *g += 1;
        }
        assert_eq!(*l.lock(), 43);
    }

    #[test]
    fn try_lock_exclusive() {
        let l = SpinLock::new(0u32);
        let _g1 = l.try_lock().unwrap();
        assert!(l.try_lock().is_none());
    }
}
