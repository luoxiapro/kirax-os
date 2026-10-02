//! # Space OS 内核入口
//!
//! 引导流程（boot/start.S → 本文件）：
//! 1. `kernel_main`：主核入口（0 号核）
//! 2. `kernel_secondary_main`：从核入口（1..3 号核）
//! 3. 完成 Phase 1（UART）→ Phase 2（内存/MMU/中断/调度/系统调用）初始化
//! 4. 运行板载自检（selftest），输出到串口
//!
//! 本内核零第三方依赖，全部自研。

#![no_std]

extern crate alloc;

// 注意：串口驱动必须最先声明（klog! 宏是文本序解析的，后面的模块才能用）
#[cfg(not(target_arch = "aarch64"))]
pub mod uart;
/// PL011 主串口（统一镜像：Pi3 32 位 / AArch64 均使用，Pi2 运行时切 mini UART）
pub mod pl011;
/// SoC 运行时识别（统一镜像：一份内核自动适配 Pi2 / Pi3）
pub mod soc;
/// 控制台适配层：运行时按 SoC 选择串口驱动
pub mod console;
pub mod sync;
pub mod memory;
#[cfg(not(target_arch = "aarch64"))]
pub mod mmu;
#[cfg(not(target_arch = "aarch64"))]
pub mod timer;
#[cfg(not(target_arch = "aarch64"))]
pub mod irq;
#[cfg(not(target_arch = "aarch64"))]
pub mod exception;
#[cfg(not(target_arch = "aarch64"))]
pub mod scheduler;
#[cfg(not(target_arch = "aarch64"))]
pub mod syscall;
#[cfg(not(target_arch = "aarch64"))]
pub mod mailbox;
#[cfg(not(target_arch = "aarch64"))]
pub mod font;
#[cfg(not(target_arch = "aarch64"))]
pub mod framebuffer;
#[cfg(not(target_arch = "aarch64"))]
pub mod graphics;
#[cfg(not(target_arch = "aarch64"))]
pub mod window;
#[cfg(not(target_arch = "aarch64"))]
pub mod desktop;
#[cfg(not(target_arch = "aarch64"))]
pub mod sd;
#[cfg(not(target_arch = "aarch64"))]
pub mod usb;
#[cfg(not(target_arch = "aarch64"))]
pub mod kir;
pub mod selftest;

/// AArch64 启动代码（EL2→EL1、VMSAv8-64 页表），由 rustc/LLVM 汇编。
#[cfg(target_arch = "aarch64")]
mod start64;

use core::fmt::Write;

/// 版本与横幅
pub const OS_NAME: &str = "Space OS";
pub const OS_VERSION: &str = "0.1.0";
#[cfg(not(target_arch = "aarch64"))]
pub const ARCH: &str = "ARMv7-A (AArch32)";
#[cfg(target_arch = "aarch64")]
pub const ARCH: &str = "ARMv8-A (AArch64)";

/// SoC 名：启动时按 CPU 型号运行时识别（统一镜像自动适配 Pi2 / Pi3）。
/// AArch64 仅存在于 Pi3，直接返回 64 位横幅。
pub fn soc_name() -> &'static str {
    #[cfg(target_arch = "aarch64")]
    {
        "BCM2837 (Raspberry Pi 3 Model B, AArch64 mode)"
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        if soc::is_pi3() {
            "BCM2837 (Raspberry Pi 3 Model B, AArch32 mode)"
        } else {
            "BCM2709/BCM2836 (Raspberry Pi 2 Model B)"
        }
    }
}
pub const BUILD_TAG: &str = env!("CARGO_PKG_VERSION");

/// 主核入口（由 start.S 跳转）
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    // ---- Phase 1：点亮串口（console 按 SoC 运行时选择驱动） ----
    let mut console = console::CONSOLE.lock();
    console.init();
    let _ = write!(console, "\n========================================\n");
    let _ = write!(console, " {OS_NAME} v{OS_VERSION}  ({BUILD_TAG})\n");
    let _ = write!(console, " {ARCH} | {}\n", soc_name());
    let _ = write!(console, "========================================\n");
    #[cfg(not(target_arch = "aarch64"))]
    let _ = if soc::use_pl011() {
        write!(console, "[boot] core0 up, PL011 UART0 115200 ready\n")
    } else {
        write!(console, "[boot] core0 up, mini UART 115200 ready\n")
    };
    #[cfg(target_arch = "aarch64")]
    let _ = write!(console, "[boot] core0 up, PL011 UART0 115200 ready (AArch64)\n");
    drop(console);

    // ---- Phase 2：内存管理（伙伴分配器 + 全局分配器） ----
    memory::init();
    klog!("[boot] buddy allocator initialized");
    klog!("[boot] 可用堆: {} MB", memory::heap_size() / (1024 * 1024));

    // ---- Phase 2：MMU 与页表 ----
    #[cfg(not(target_arch = "aarch64"))]
    mmu::init();
    #[cfg(not(target_arch = "aarch64"))]
    klog!("[boot] MMU enabled (1:1 映射, 短描述符)");
    #[cfg(target_arch = "aarch64")]
    klog!("[boot] MMU enabled by start64 (VMSAv8-64, 1GB block)");

    // ---- Phase 2：中断控制器与系统定时器 ----
    #[cfg(not(target_arch = "aarch64"))]
    irq::init();
    #[cfg(not(target_arch = "aarch64"))]
    timer::init();
    #[cfg(not(target_arch = "aarch64"))]
    klog!("[boot] interrupt controller + system timer ready");

    // ---- 板载自检 ----
    selftest::run_all();

    // ---- 从核启动（spin 表） ----
    #[cfg(not(target_arch = "aarch64"))]
    scheduler::smp::wake_secondaries();
    #[cfg(not(target_arch = "aarch64"))]
    klog!("[boot] secondary cores released");

    // ---- 进入调度器空闲循环 ----
    #[cfg(not(target_arch = "aarch64"))]
    scheduler::start_scheduler();
    #[cfg(target_arch = "aarch64")]
    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}

/// 从核入口（由 start.S 的 secondary_entry 跳转，参数为核号）
#[cfg(not(target_arch = "aarch64"))]
#[no_mangle]
pub extern "C" fn kernel_secondary_main(core_id: u32) -> ! {
    klog!("[boot] core{} up", core_id);
    // 从核进入各自的空闲循环（后续调度器接管）
    scheduler::secondary_idle(core_id);
}

/// 内核 panic 处理器：串口打印后挂死
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut c = console::CONSOLE.lock();
    let _ = write!(c, "\n[KERNEL PANIC] ");
    let _ = write!(c, "{}", info);
    let _ = write!(c, "\n[SpaceOS] system halted\n");
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}
