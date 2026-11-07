//! 物理内存管理：二进制伙伴分配器 + Rust 全局分配器。
//!
//! 内存规划（1GB，树莓派 2）：
//! ```text
//! 0x0000_0000 - 0x0000_03FF   启动桩 / 多核 spin 表（保留）
//! 0x0000_8000 - 0x001F_FFFF   内核镜像（代码/数据/BSS/4 核栈，约 2MB）
//! 0x0020_0000 - 0x3BFF_FFFF   内核堆（伙伴分配器管理，约 950MB）
//! 0x3C00_0000 - 0x3EFF_FFFF   VideoCore 保留（gpu_mem=64MB，含 framebuffer）
//! 0x3F00_0000 - 0x3FFF_FFFF   外设（legacy）
//! 0x4000_0000 - 0x4000_FFFF   ARM-local（BCM2836 中断）
//! ```
//! 说明：内核直接使用物理地址（早期 1:1 映射），虚拟地址空间在 Phase 2
//! 的 MMU 模块中建立，用户态进程从 0x2000_0000 起映射。

use crate::sync::SpinLock;
use core::alloc::{GlobalAlloc, Layout};
use kirax_core::buddy::{BuddyAllocator, PAGE_SIZE};

// 内核镜像结束地址（链接脚本符号）
extern "C" {
    static __kernel_end: u8;
}

/// 堆起始（内核镜像结束，4KB 对齐）
fn heap_start() -> usize {
    let end = unsafe { &__kernel_end as *const u8 as usize };
    kirax_core::align_up(end, PAGE_SIZE)
}

/// 堆结束：0x3C00_0000（VideoCore 保留区之下）
const HEAP_END: usize = 0x3C00_0000;

/// 全局伙伴分配器实例
static ALLOCATOR: SpinLock<BuddyAllocator> = SpinLock::new(BuddyAllocator::new());

/// 初始化内存管理（必须在任何堆分配之前调用）
pub fn init() {
    let mut a = ALLOCATOR.lock();
    a.init(heap_start(), HEAP_END - heap_start(), 12)
        .expect("buddy init failed");
}

/// 堆总大小
pub fn heap_size() -> usize {
    HEAP_END - heap_start()
}

/// 当前空闲内存
pub fn free_memory() -> usize {
    ALLOCATOR.lock().free_bytes_total()
}

/// 内核页帧分配（物理地址）
pub fn alloc_pages(order: u32) -> Option<usize> {
    ALLOCATOR.lock().alloc_blocks(order)
}

/// 释放页帧
pub fn free_pages(addr: usize, order: u32) -> Result<(), &'static str> {
    ALLOCATOR.lock().free_blocks(addr, order)
}

// ---------------------------------------------------------------------------
// Rust 全局分配器（供 `alloc` crate 的 Vec / String 使用）
// ---------------------------------------------------------------------------
pub struct KernelAllocator;

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let addr = ALLOCATOR
            .lock()
            .alloc_bytes(layout.size())
            .unwrap_or(0);
        // 返回的地址天然按最小块（4KB）对齐，满足任意 layout 对齐
        addr as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = ALLOCATOR.lock().free_bytes(ptr as usize, layout.size());
    }
}

/// 注册为 Rust 全局分配器
#[global_allocator]
static GLOBAL_ALLOC: KernelAllocator = KernelAllocator;

// 分配失败默认走 alloc 提供的 abort 处理器（Rust 稳定版不再允许自定义
// #[alloc_error_handler]；如需更详细的失败信息，可在 panic 处理器里补打）
