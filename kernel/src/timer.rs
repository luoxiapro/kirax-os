//! BCM2835 系统定时器（1 MHz 递增计数器）。
//!
//! 寄存器（外设基址 0x3F00_0000）：
//! - 0x3000 + 0x00  CS   控制/状态（写 1 清匹配中断）
//! - 0x3000 + 0x04  CLO  计数器低 32 位
//! - 0x3000 + 0x08  CHI  计数器高 32 位
//! - 0x3000 + 0x0C/0x10/0x14/0x18  C0..C3 比较寄存器
//!
//! 中断：System Timer Compare 1 → 中断控制器 IRQ #1（GPU 中断，路由到 core0）。

use crate::irq;
use crate::klog;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicBool, Ordering};

const TIMER_BASE: usize = 0x3F00_3000;
const REG_CS: *mut u32 = (TIMER_BASE + 0x00) as *mut u32;
const REG_CLO: *mut u32 = (TIMER_BASE + 0x04) as *mut u32;
const REG_CHI: *mut u32 = (TIMER_BASE + 0x08) as *mut u32;
const REG_C1: *mut u32 = (TIMER_BASE + 0x10) as *mut u32;

/// 系统定时器 IRQ 号（BCM2835 中断控制器 IRQ1）
pub const TIMER_IRQ: u32 = 1;

/// 节拍间隔：10ms（100Hz）
pub const TICK_INTERVAL_US: u64 = 10_000;

/// 单调 tick 计数
static TICKS: SpinLock<u64> = SpinLock::new(0);

/// 是否已启用周期中断
static ENABLED: AtomicBool = AtomicBool::new(false);

/// 读取 64 位单调时间（微秒）
pub fn time_us() -> u64 {
    unsafe {
        // 高 32 位读两次防回绕
        let hi1 = core::ptr::read_volatile(REG_CHI) as u64;
        let lo = core::ptr::read_volatile(REG_CLO) as u64;
        let hi2 = core::ptr::read_volatile(REG_CHI) as u64;
        if hi1 == hi2 {
            (hi1 << 32) | lo
        } else {
            (hi2 << 32) | lo
        }
    }
}

/// 系统 tick 数（由定时器中断累加）
pub fn ticks() -> u64 {
    *TICKS.lock()
}

/// 忙等微秒
pub fn delay_us(us: u64) {
    let start = time_us();
    while time_us().wrapping_sub(start) < us {
        core::hint::spin_loop();
    }
}

/// 初始化定时器：注册中断 + 设置周期比较
pub fn init() {
    irq::register_handler(TIMER_IRQ, timer_irq_handler);
    unsafe {
        // 清匹配中断
        core::ptr::write_volatile(REG_CS, 1 << 1);
        // 设置第一次比较
        core::ptr::write_volatile(REG_C1, (time_us() + TICK_INTERVAL_US) as u32);
    }
    irq::enable_gpu_irq(TIMER_IRQ);
    ENABLED.store(true, Ordering::Release);
    klog!("[timer] system timer @1MHz, tick 10ms (100Hz)");
}

/// 定时器中断处理（在 IRQ 上下文调用）
fn timer_irq_handler() {
    *TICKS.lock() += 1;
    unsafe {
        // 清中断标志
        core::ptr::write_volatile(REG_CS, 1 << 1);
        // 安排下一次
        let next = core::ptr::read_volatile(REG_C1).wrapping_add(TICK_INTERVAL_US as u32);
        core::ptr::write_volatile(REG_C1, next);
    }
    // 调度器节拍
    crate::scheduler::on_tick();
}
