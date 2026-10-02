//! MMU 初始化：ARMv7 短描述符格式，1:1 恒等映射。
//!
//! 页表结构（short-descriptor）：
//! - L1 表：4096 项 × 4 字节 = 16KB（16KB 对齐），每项覆盖 1MB（section 描述符）
//! - 本阶段使用 section 映射（1MB 粒度），把 1GB RAM 与 16MB 外设恒等映射
//!
//! Section 描述符布局（ARMv7-A，短描述符）：
//! ```text
//! [31:20] 基地址 | [19] 0 | [18] nG | [17] S | [16] AP[2]
//! [15:14] TEX | [13] 0 | [12] 0 | [11:10] AP[1:0] | [9:5] 0 | [4] XN | [3:2] Domain | [1:0] 0b10
//! ```
//! 使用业界验证过的常量（众多树莓派裸机项目通用）：
//! - 外设/设备内存：0x00000406
//! - 普通可缓存内存：0x00000C0E
//! DACR 全部域设为 manager，规避早期权限问题（后续细化 AP 权限）。

use crate::sync::SpinLock;
use crate::klog;

/// L1 页表容器（16KB 对齐：ARMv7 要求 TTBR 16KB 对齐，用结构体保证）
#[repr(C, align(16384))]
struct AlignedL1([u32; 4096]);

/// L1 页表（4096 个 section 描述符，覆盖 4GB 地址空间）
static mut L1_TABLE: AlignedL1 = AlignedL1([0; 4096]);

/// 取页表可变引用（内核页表全局唯一，允许多处取用）
#[allow(static_mut_refs)]
fn l1() -> &'static mut [u32; 4096] {
    unsafe { &mut L1_TABLE.0 }
}

/// section 描述符：设备内存（外设，强序/设备属性，特权访问）
pub const SECT_DEVICE: u32 = 0x0000_0406;
/// section 描述符：普通可缓存内存（RAM）
pub const SECT_NORMAL: u32 = 0x0000_0C0E;

/// 锁（MMU 配置期使用；运行期页表修改需停核或使用 break-before-make）
static MMU_LOCK: SpinLock<()> = SpinLock::new(());

extern "C" {
    static __kernel_end: u8;
}

/// 初始化 MMU：建立 1:1 映射并启用
pub fn init() {
    let _g = MMU_LOCK.lock();
    // 清空页表
    l1().iter_mut().for_each(|e| *e = 0);

        // 1) RAM：0x0000_0000 .. 0x3C00_0000 全部映射为普通可缓存内存
        //    （VideoCore 保留区 0x3C00_0000 起不动，保持不可映射，避免踩到 framebuffer）
        for va in (0x0000_0000..0x3C00_0000).step_by(0x100_000) {
            map_section(va, va, SECT_NORMAL);
        }

        // 2) 外设：0x3F00_0000 .. 0x4000_0000 设备内存
        for va in (0x3F00_0000..0x4000_0000).step_by(0x100_000) {
            map_section(va, va, SECT_DEVICE);
        }

        // 3) ARM-local（BCM2836 中断/邮筒）：0x4000_0000 .. 0x4010_0000
        for va in (0x4000_0000..0x4010_0000).step_by(0x100_000) {
            map_section(va, va, SECT_DEVICE);
        }

        enable_mmu();
    klog!("[mmu] 1:1 mapping: RAM 0x00000000-0x3BFFFFFF, PERIPH 0x3F000000-0x40100000");
}

/// 映射一个 1MB section
pub fn map_section(va: usize, pa: usize, flags: u32) {
    unsafe {
        let idx = (va >> 20) as usize;
        l1()[idx] = ((pa & 0xFFF0_0000) as u32) | flags;
        // 无效化对应 TLB 项
        core::arch::asm!(
            "mcr p15, 0, {0}, c8, c7, 1", // TLBIMVA
            in(reg) va,
            options(nostack, preserves_flags)
        );
    }
}

/// 启用 MMU（SCTLR.M/C/I）
fn enable_mmu() {
    unsafe {
        // 1. TTBR0 = L1 表基址
        let ttbr0 = l1().as_ptr() as usize;
        core::arch::asm!(
            "mcr p15, 0, {0}, c2, c0, 0", // TTBR0
            in(reg) ttbr0,
            options(nostack, preserves_flags)
        );
        // 2. DACR = 所有域 manager（0b11）
        let dacr: u32 = 0xFFFF_FFFF;
        core::arch::asm!(
            "mcr p15, 0, {0}, c3, c0, 0", // DACR
            in(reg) dacr,
            options(nostack, preserves_flags)
        );
        // 3. 无效化 TLB 与缓存
        core::arch::asm!(
            "mcr p15, 0, {0}, c8, c7, 0", // TLBIALL
            in(reg) 0u32,
            options(nostack, preserves_flags)
        );
        core::arch::asm!(
            "mcr p15, 0, {0}, c7, c5, 0", // ICIALLU
            in(reg) 0u32,
            options(nostack, preserves_flags)
        );
        // 4. 启用 MMU + D-Cache + I-Cache
        core::arch::asm!(
            "mrc p15, 0, r0, c1, c0, 0
             orr r0, r0, #(1 << 0)
             orr r0, r0, #(1 << 2)
             orr r0, r0, #(1 << 12)
             mcr p15, 0, r0, c1, c0, 0
             isb",
            options(nostack)
        );
    }
}

/// 读取当前页表项（调试用）
pub fn read_section(va: usize) -> u32 {
    unsafe { L1_TABLE.0[va >> 20] }
}
