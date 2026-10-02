//! # Space OS AArch64 启动代码（global_asm! 内嵌，由 rustc/LLVM 汇编）
//!
//! 树莓派 3 (BCM2837, Cortex-A53, AArch64)。
//! 职责：EL2→EL1 降级、核识别、VMSAv8-64 页表初始化、开 MMU、清 BSS、跳 kernel_main。
//!
//! 入口基址：0x80000（树莓派 AArch64 固件 / QEMU raspi3b FIRMWARE_ADDR_3）。

use core::arch::global_asm;

global_asm!(
    // ------------------------------------------------------------------
    // 常量
    // ------------------------------------------------------------------
    ".set    SPSR_EL1_EL2t, 0x3C5",          // D/A=1, IRQ=1, FIQ=1, EL1t

    ".section .text._start",
    ".balign 4",
    ".global _start",
    "_start:",
    // 1) CurrentEL 判断
    "mrs     x0, CurrentEL",
    "lsr     x0, x0, #2",
    "cmp     x0, #2",
    "b.ne    2f",

    // 2) EL2 -> EL1
    "mov     x0, #(1<<10)",
    "msr     SCTLR_EL2, x0",
    "msr     CNTVOFF_EL2, xzr",
    "mov     x0, #0x3C5",
    "msr     SPSR_EL2, x0",
    "adr     x0, 2f",
    "msr     ELR_EL2, x0",
    "eret",

    "2:",
    "msr     SCTLR_EL1, xzr",                 // 关 MMU/Cache
    // 3) 核识别
    "mrs     x0, MPIDR_EL1",
    "and     x0, x0, #0xFF",
    "cbz     x0, 3f",
    "4: wfe",
    "   b     4b",

    "3:",
    // 4) 向量表
    "adr     x0, _vectors",
    "msr     VBAR_EL1, x0",

    // 5) 页表初始化
    "bl      mmu_init_64",

    // 6) 栈
    "ldr     x0, =__stack_top",
    "mov     sp, x0",
    "msr     SP_EL1, x0",

    // 7) 清 BSS
    "ldr     x0, =__bss_start",
    "ldr     x1, =__bss_end",
    "10: cmp     x0, x1",
    "   b.ge    11f",
    "   str     xzr, [x0], #8",
    "   b       10b",
    "11:",

    // 8) 开 MMU
    "bl      mmu_enable_64",

    // 9) 跳 Rust
    "bl      kernel_main",
    "12: b       12b",

    // ------------------------------------------------------------------
    // 异常向量表（VBAR_EL1，2KB 对齐，16 项 × 32 字节）
    // ------------------------------------------------------------------
    ".balign 2048",
    ".global _vectors",
    "_vectors:",
    "b       .",
    "b       .",
    "b       .",
    "b       .",
    ".balign 32",
    "b       .",
    "b       .",
    "b       .",
    "b       .",
    ".balign 32",
    "b       .",
    "b       .",
    "b       .",
    "b       .",
    ".balign 32",
    "b       .",
    "b       .",
    "b       .",
    "b       .",

    // ------------------------------------------------------------------
    // 页表（4KB 对齐，L1 512 项 + L2 512 项）
    // 映射设计（BCM2837 1GB RAM + 外设）：
    //   L1[0] → L2 页表（0x00000000-0x3FFFFFFF，2MB 块粒度）
    //          · 0x00000000-0x3EFFFFFF : Normal（RAM）
    //          · 0x3F000000-0x40000000 : Device（外设，不缓存）
    //   L1[1] → Normal 1GB block（1GB-2GB，备用）
    //   L1[2] → Normal 1GB block（2GB-3GB，备用）
    //   L1[3] → Device 1GB block（3GB-4GB，备用）
    // 注：外设区必须 Device（禁止缓存），否则真机上 UART/定时器等
    // 寄存器读写会被 Cache 延迟或合并，QEMU 不模拟 Cache 所以测不出来。
    // ------------------------------------------------------------------
    ".balign 4096",
    ".global _page_l1",
    "_page_l1:",
    ".skip 8 * 512",
    ".balign 4096",
    ".global _page_l2",
    "_page_l2:",
    ".skip 8 * 512",

    // void mmu_init_64(void)
    ".global mmu_init_64",
    ".type   mmu_init_64, %function",
    "mmu_init_64:",
    // 清空 L1（512 项）
    "adr     x0, _page_l1",
    "mov     x1, #0",
    "mov     x2, #512",
    "1: str     xzr, [x0], #8",
    "   subs    x2, x2, #1",
    "   b.ne    1b",
    // 清空 L2（512 项）
    "adr     x0, _page_l2",
    "mov     x2, #512",
    "2: str     xzr, [x0], #8",
    "   subs    x2, x2, #1",
    "   b.ne    2b",

    // MAIR_EL1: index0=Device(0x00), index1=Normal(0xFF)
    "mov     x1, #0xFF",
    "lsl     x1, x1, #8",
    "mov     x2, #0x00",
    "orr     x1, x1, x2",
    "msr     MAIR_EL1, x1",

    // 填 L2：512 项，每项 2MB block；i < 504 为 Normal，i >= 504 为 Device
    //（504 * 2MB = 0x3F000000 = 外设基址）
    "adr     x0, _page_l2",
    "mov     x2, #0",
    "mov     x3, #512",
    "20:",
    "   lsl     x4, x2, #21",               // PA = i * 2MB
    "   mov     x5, x4",
    "   orr     x5, x5, #(1<<0)",           // block
    "   orr     x5, x5, #(1<<10)",          // AF
    "   orr     x5, x5, #(3<<8)",           // SH=OS
    "   cmp     x2, #504",
    "   b.ge    21f",
    "   orr     x5, x5, #(1 << 2)",         // AttrIndx=1 → Normal
    "21:",
    "   str     x5, [x0, x2, lsl #3]",
    "   add     x2, x2, #1",
    "   cmp     x2, x3",
    "   b.lo    20b",

    // 填 L1：L1[0] = L2 页表描述符；L1[1..3] = 1GB block
    "adr     x0, _page_l1",
    "adr     x1, _page_l2",
    "orr     x1, x1, #(1<<0)",              // table
    "orr     x1, x1, #(1<<1)",              // table
    "str     x1, [x0, #0]",                 // L1[0] → L2
    "mov     x2, #1",
    "mov     x3, #4",
    "22:",
    "   lsl     x4, x2, #30",               // PA = n*1GB
    "   mov     x5, x4",
    "   orr     x5, x5, #(1<<0)",           // block
    "   orr     x5, x5, #(1<<10)",          // AF
    "   orr     x5, x5, #(3<<8)",           // SH=OS
    "   cmp     x2, #3",
    "   b.eq    23f",
    "   orr     x5, x5, #(1 << 2)",         // Normal（1GB/2GB 备用）
    "23:",
    // 3GB 项保持 Device（MAIR index=0）
    "   str     x5, [x0, x2, lsl #3]",
    "   add     x2, x2, #1",
    "   cmp     x2, x3",
    "   b.lo    22b",

    // TCR_EL1: 4KB granule, T0SZ=0
    "mov     x1, #0",                        // T0SZ=0
    "orr     x1, x1, #(1 << 14)",           // ORGN=WB
    "orr     x1, x1, #(1 << 16)",           // IRGN=WB
    "orr     x1, x1, #(3 << 8)",            // SH=OS
    "orr     x1, x1, #(1 << 30)",           // TG0=4KB
    "msr     TCR_EL1, x1",

    "adr     x0, _page_l1",
    "msr     TTBR0_EL1, x0",

    "dsb     sy",
    "isb",
    "ret",

    // void mmu_enable_64(void)
    ".global mmu_enable_64",
    ".type   mmu_enable_64, %function",
    "mmu_enable_64:",
    "dsb     sy",
    "isb",
    "mrs     x0, SCTLR_EL1",
    "orr     x0, x0, #(1 << 0)",           // M
    "orr     x0, x0, #(1 << 2)",           // C (DCache)
    "orr     x0, x0, #(1 << 12)",          // I (ICache)
    "msr     SCTLR_EL1, x0",
    "dsb     sy",
    "isb",
    "ret",
);
