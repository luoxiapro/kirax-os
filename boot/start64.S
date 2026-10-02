// ============================================================================
// Space OS —— 树莓派 3 (BCM2837, Cortex-A53, AArch64) 裸机启动汇编
// ============================================================================
// 职责：
//   1. 从 EL2（QEMU/固件进入的异常级别）降级到 EL1
//   2. 识别核（MPIDR_EL1），主核继续，从核 wfe 等待
//   3. 配置 AArch64 内存管理（VMSAv8-64）：MAIR/TCR/TTBR0/SCTLR，开 MMU
//   4. 设置 SP_EL1、向量表 VBAR_EL1
//   5. 跳转到 Rust kernel_main
//
// 入口基址：0x80000（树莓派 AArch64 固件加载地址，QEMU raspi3b FIRMWARE_ADDR_3）
// ============================================================================

.set    SPSR_EL1_EL2t,     0x3C5   // EL2 -> EL1 (t, D/A, IRQ, FIQ masked)
.set    REG_CPUACTLR_EL1,  S3_1_C15_C2_1
.set    SCTLR_EL1_RES1,     (1<<11 | 1<<20 | 3<<22 | 1<<11)

.section .text._start
.balign 4

.global _start
_start:
    // -----------------------------------------------------------------------
    // 1) 判断当前异常级别（MRS CurrentEL）
    // -----------------------------------------------------------------------
    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #2
    b.ne    1f                       // 已经在 EL1，直接继续

    // -----------------------------------------------------------------------
    // 2) EL2 -> EL1：
    //    设置 SPSR_EL2 / ELR_EL2，eret 降级
    // -----------------------------------------------------------------------
    mov     x0, #(1<<10)             // RES1 bit
    msr     SCTLR_EL2, x0           // 关 MMU/Cache（降级前清干净）

    mrs     x0, CNTFRQ_EL0           // 随便取个值占位
    msr     CNTVOFF_EL2, xzr         // 清虚拟定时器偏移

    mov     x0, #0x3C5               // D/A=1, IRQ=1, FIQ=1, EL1t（AArch64）
    msr     SPSR_EL2, x0

    adr     x0, 1f
    msr     ELR_EL2, x0
    eret                             // -> EL1, PC=1f

1:
    // -----------------------------------------------------------------------
    // 3) 关 MMU/Cache（EL1 下），识别核
    // -----------------------------------------------------------------------
    msr     SCTLR_EL1, xzr           // 关 MMU、ICache、DCache

    mrs     x0, MPIDR_EL1
    and     x0, x0, #0xFF            // 核 ID
    cbz     x0, 2f                   // 主核继续

    // 从核：wfe 等待（后续 SMP 唤醒）
3:  wfe
    b       3b

2:
    // -----------------------------------------------------------------------
    // 4) 设置异常向量基址
    // -----------------------------------------------------------------------
    adr     x0, _vectors
    msr     VBAR_EL1, x0

    // -----------------------------------------------------------------------
    // 5) 初始化页表（L1 1GB block 映射）
    //    0x00000000 - 0x3FFFFFFF : 普通内存（Write-Back Write-Allocate）
    //    0x3F000000 - 0x40000000 : 设备（nGnRnE）
    //    （1GB 粒度：0x00000000-0x3FFFFFFF 整段做普通内存即可，
    //     外设 0x3F000000 落在 1GB 块内，下面单独改）
    // -----------------------------------------------------------------------
    bl      mmu_init_64

    // -----------------------------------------------------------------------
    // 6) 设置主核栈（栈顶 0x40000000 下方，16KB 对齐）
    // -----------------------------------------------------------------------
    ldr     x0, =__stack_top
    mov     sp, x0
    msr     SP_EL1, x0

    // -----------------------------------------------------------------------
    // 7) 清 BSS
    // -----------------------------------------------------------------------
    ldr     x0, =__bss_start
    ldr     x1, =__bss_end
10: cmp     x0, x1
    b.ge    11f
    str     xzr, [x0], #8
    b       10b
11:
    // -----------------------------------------------------------------------
    // 8) 开 MMU
    // -----------------------------------------------------------------------
    bl      mmu_enable_64

    // -----------------------------------------------------------------------
    // 9) 跳 Rust
    // -----------------------------------------------------------------------
    bl      kernel_main
12: b       12b                       // 不应返回

// ============================================================================
// 异常向量表（VBAR_EL1 指向此处，2KB 对齐；每个 entry 32 字节 = 8 条指令）
// ============================================================================
.balign 2048
.global _vectors
_vectors:
    b       .                        // Synchronous, CurrentEL SP0
    b       .                        // IRQ
    b       .                        // FIQ
    b       .                        // SError
    .balign 32
    b       .                        // Synchronous, CurrentEL SPx
    b       .                        // IRQ
    b       .                        // FIQ
    b       .                        // SError
    .balign 32
    b       .                        // Synchronous, LowerEL AArch64
    b       .
    b       .
    b       .
    .balign 32
    b       .                        // Synchronous, LowerEL AArch32
    b       .
    b       .
    b       .

// ============================================================================
// 页表初始化与启用（VMSAv8-64, 4KB granule, L1 1GB block）
// ============================================================================
.set    MAIR_ATTR_DEVICE,   0x00    // nGnRnE
.set    MAIR_ATTR_NORMAL,   0xFF    // Write-Back Write-Allocate RW-Internal

// L1 描述符：block, AF=1(bit10), SH=OS(bit9,8=11), AF(bit10), PXN(bit5), UXN(bit5 in PXN?)
// 简化：AP=00 (RW EL1), NS=1 (bit0=block), attr index 来自 MAIR
.set    PTE_BLOCK,          (1<<0)                // Table/Block 位
.set    PTE_AF,             (1<<10)               // Access Flag
.set    PTE_SH_OS,          (3<<8)                // Outer Shareable
.set    PTE_NG,             (0)                  // Not Global
.set    PTE_AP_RW,          (0<<6)               // EL1 RW
.set    PTE_ATTR_NORMAL,    (0 << 2)             // MAIR index 1 (Normal)
.set    PTE_ATTR_DEVICE,    (1 << 2)             // MAIR index 0 (Device)

.balign 4096
.global _page_l1
_page_l1:
    .skip 8 * 512                   // L1: 512 entries × 8 bytes = 4KB

.macro  msv_barrier
    dsb     sy
    isb
.endm

// void mmu_init_64(void)
.global mmu_init_64
.type   mmu_init_64, %function
mmu_init_64:
    adr     x0, _page_l1
    mov     x1, #0
    mov     x2, #512
1:  str     xzr, [x0], #8
    subs    x2, x2, #1
    b.ne    1b

    // MAIR_EL1: index0 = Device(0x00), index1 = Normal(0xFF)
    mov     x1, #MAIR_ATTR_NORMAL
    lsl     x1, x1, #8              // index1
    mov     x2, #MAIR_ATTR_DEVICE
    orr     x1, x1, x2              // index0
    msr     MAIR_EL1, x1

    // 填充 L1：每 1GB block
    // entry n 对应 PA = n*1GB
    adr     x0, _page_l1
    mov     x2, #0
    mov     x3, #4                   // 覆盖 0~4GB（4 个 1GB 块）
2:
    lsl     x4, x2, #30             // PA = n*1GB
    mov     x5, x4
    orr     x5, x5, #PTE_BLOCK
    orr     x5, x5, #PTE_AF
    orr     x5, x5, #PTE_SH_OS
    // 0~0x3FFFFFFF：Normal；外设 0x3F000000 落在 block3（3~4GB），用 Device
    cmp     x2, #3
    b.eq    3f
    orr     x5, x5, #PTE_ATTR_NORMAL
    b       4f
3:
    orr     x5, x5, #PTE_ATTR_DEVICE
4:
    str     x5, [x0, x2, lsl #3]
    add     x2, x2, #1
    cmp     x2, x3
    b.lo    2b

    // TCR_EL1: 4KB granule, T0SZ=0 (48bit)
    mov     x1, #(0 << 0)           // T0SZ = 0 (4GB)
    orr     x1, x1, #(1 << 14)     // ORGN=Write-Back
    orr     x1, x1, #(1 << 16)     // IRGN=Write-Back
    orr     x1, x1, #(1 << 30)     // SH=Inner Shareable
    orr     x1, x1, #(1 << 32)     // TG0=4KB
    msr     TCR_EL1, x1

    // TTBR0_EL1 = L1 table
    adr     x0, _page_l1
    msr     TTBR0_EL1, x0

    msv_barrier
    ret

// void mmu_enable_64(void)
.global mmu_enable_64
.type   mmu_enable_64, %function
mmu_enable_64:
    msv_barrier
    mrs     x0, SCTLR_EL1
    orr     x0, x0, #(1 << 0)      // M = MMU enable
    orr     x0, x0, #(1 << 2)     // C = DCache enable
    orr     x0, x0, #(1 << 12)    // I = ICache enable
    msr     SCTLR_EL1, x0
    msv_barrier
    ret
