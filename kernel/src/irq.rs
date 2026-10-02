//! 中断控制器：BCM2836 双控制器。
//!
//! 树莓派 2 的中断有两级：
//! 1. **legacy（BCM2835 中断控制器）**，基址 0x3F00_B200：
//!    管理所有 GPU/外设中断（定时器、USB、SD、GPIO 等），分 3 组 pending 寄存器。
//! 2. **ARM-local（BCM2836 新增）**，基址 0x4000_0000：
//!    管理 4 核本地中断（本地定时器、邮筒、全局/通用定时器），
//!    以及 GPU 中断到具体核的路由。
//!
//! 中断流向：外设 → legacy → GPU Interrupt Routing → ARM-local → CPU IRQ 线。
//! 处理时先读 ARM-local 的核中断源，再读 legacy pending 精确定位。

use crate::sync::SpinLock;
use crate::klog;
use core::sync::atomic::{AtomicBool, Ordering};

/// legacy 中断控制器寄存器
const LEGACY_BASE: usize = 0x3F00_B200;
#[allow(dead_code)]
const IRQ_BASIC_PENDING: *mut u32 = (LEGACY_BASE + 0x200) as *mut u32;
const IRQ_PENDING_1: *mut u32 = (LEGACY_BASE + 0x204) as *mut u32;
const IRQ_PENDING_2: *mut u32 = (LEGACY_BASE + 0x208) as *mut u32;
const IRQ_ENABLE_1: *mut u32 = (LEGACY_BASE + 0x210) as *mut u32;
const IRQ_ENABLE_2: *mut u32 = (LEGACY_BASE + 0x214) as *mut u32;
const IRQ_ENABLE_BASIC: *mut u32 = (LEGACY_BASE + 0x218) as *mut u32;
const IRQ_DISABLE_1: *mut u32 = (LEGACY_BASE + 0x21C) as *mut u32;
const IRQ_DISABLE_2: *mut u32 = (LEGACY_BASE + 0x220) as *mut u32;

/// ARM-local 寄存器
const LOCAL_BASE: usize = 0x4000_0000;
/// 核 0 中断源（读：哪类中断到达本核）
const CORE0_IRQ_SOURCE: *mut u32 = (LOCAL_BASE + 0x20) as *mut u32;
/// GPU 中断路由（每 2 位一组，选择接收核）
const GPU_INTERRUPT_ROUTING: *mut u32 = (LOCAL_BASE + 0x08) as *mut u32;

/// 中断处理函数类型
pub type IrqHandler = fn();

/// 中断处理表（GPU IRQ 0..63；这里只使用第一组 0..31）
static HANDLERS: SpinLock<[Option<IrqHandler>; 64]> = SpinLock::new([None; 64]);

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// 初始化：清中断、注册表清零、路由 GPU 中断到核 0
pub fn init() {
    unsafe {
        // 关闭全部 legacy 中断（安全起点）
        core::ptr::write_volatile(IRQ_DISABLE_1, 0xFFFF_FFFF);
        core::ptr::write_volatile(IRQ_DISABLE_2, 0xFFFF_FFFF);
        core::ptr::write_volatile(IRQ_ENABLE_BASIC, 0);

        // GPU 中断全部路由到核 0
        core::ptr::write_volatile(GPU_INTERRUPT_ROUTING, 0);
    }
    INITIALIZED.store(true, Ordering::Release);
}

/// 注册 GPU 中断处理函数
pub fn register_handler(irq: u32, handler: IrqHandler) {
    let mut h = HANDLERS.lock();
    if (irq as usize) < h.len() {
        h[irq as usize] = Some(handler);
    }
}

/// 使能某个 GPU 中断
pub fn enable_gpu_irq(irq: u32) {
    unsafe {
        match irq {
            0..=31 => {
                // 第一组：IRQ_PENDING_1 对应 enable1
                core::ptr::write_volatile(IRQ_ENABLE_1, 1 << irq)
            }
            32..=63 => {
                core::ptr::write_volatile(IRQ_ENABLE_2, 1 << (irq - 32))
            }
            _ => {}
        }
    }
}

/// 关闭某个 GPU 中断
pub fn disable_gpu_irq(irq: u32) {
    unsafe {
        match irq {
            0..=31 => core::ptr::write_volatile(IRQ_DISABLE_1, 1 << irq),
            32..=63 => core::ptr::write_volatile(IRQ_DISABLE_2, 1 << (irq - 32)),
            _ => {}
        }
    }
}

/// 当前核 IRQ 处理入口（由异常分发调用）
pub fn handle_current_irq() {
    unsafe {
        // 1. 读 ARM-local 核中断源
        let src = core::ptr::read_volatile(CORE0_IRQ_SOURCE);

        // 2. GPU 中断位（bit0..7 对应路由到本核的 GPU IRQ 组 0..7）
        if src & 0xFF != 0 {
            // legacy pending1/pending2 精确定位
            let p1 = core::ptr::read_volatile(IRQ_PENDING_1);
            let p2 = core::ptr::read_volatile(IRQ_PENDING_2);
            for bit in 0..32u32 {
                if p1 & (1 << bit) != 0 {
                    dispatch_gpu(bit);
                }
            }
            for bit in 0..32u32 {
                if p2 & (1 << bit) != 0 {
                    dispatch_gpu(32 + bit);
                }
            }
        }

        // 3. 本地中断（mailbox / 本地定时器）——预留
        //    bit11..14 = mailboxes；后续 USB 键盘鼠标等驱动注册
    }
}

fn dispatch_gpu(irq: u32) {
    let handlers = HANDLERS.lock();
    if let Some(h) = handlers[irq as usize] {
        // 处理函数可能注册/注销，先取再调用（调用时不能持有锁，防重入死锁）
        drop(handlers);
        h();
    } else {
        klog!("[irq] 未注册的 GPU IRQ #{}", irq);
    }
}
