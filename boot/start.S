// ============================================================================
// Space OS —— 树莓派 2 (BCM2709/BCM2836, ARMv7-A, 4×Cortex-A7) 裸机启动汇编
// ============================================================================
// 职责：
//   1. 双态入口（0x8000）：兼容 QEMU（以 Thumb 状态进入）与真机（ARM 状态）
//   2. 异常向量表（0x8020，32 字节对齐，VBAR 指向此处）
//   3. 处理 HYP→SVC 模式切换（防御性：树莓派固件可能从 HYP 启动）
//   4. 识别当前核（MPIDR），主核初始化，从核进入 pen 等待（兼容 QEMU 与真机固件）
//   5. 设置每核 SVC 栈、清零 BSS、设置 VBAR
//   6. 跳转到 Rust 内核 kernel_main
// 说明：全程不依赖任何现有系统，指令均为 ARMv7-A 通用指令。
// ============================================================================

.syntax unified
.arm                                    @ 向量表必须为 ARM 状态指令

.section .text._start

// ---------------------------------------------------------------------------
// 双态入口（0x8000）：
//   真机固件以 ARM 状态跳入：下面的机器码在 ARM 下是无害 NOP，顺序执行到 0x8020
//   的向量表第一条（b .Lreset_vector），正常引导。
//   QEMU(6.x) 的 raspi2b 模拟固件以 Thumb 状态跳入：机器码的低半字恰好是
//   Thumb 指令序列（lsls → adds r0,pc,#0x20 → bx r0），把 CPU 切回 ARM 状态
//   并跳到 0x8020 向量表。两种启动方式殊途同归。
// ---------------------------------------------------------------------------
.global _start
_start:
    .byte 0x00, 0x00          @ Thumb: lsls r0, r0, #0        | ARM: ANDGE r8, r0, r0（NOP）
    .byte 0x08, 0xA0          @ Thumb: adds r0, pc, #0x20     |      （r0 = 0x8020）
    .byte 0x00, 0x47          @ Thumb: bx r0（切 ARM，PC=0x8020）| ARM: ANDEQ r4, r0, r0（NOP）
    .byte 0x00, 0x00          @ 填充
    .space 24, 0              @ 0x8008..0x801F：全零 NOP（双态下均无害）

// ---------------------------------------------------------------------------
// 异常向量表（0x8020，32 字节对齐，共 8 个入口，每个 4 字节）
// ---------------------------------------------------------------------------
.balign 32
.global _vectors
_vectors:
    b .Lreset_vector                    @ 0x00 Reset
    b .Lundef_vector                    @ 0x04 未定义指令
    b .Lsvc_vector                      @ 0x08 SVC 系统调用
    b .Lprefetch_vector                 @ 0x0C 预取中止
    b .Ldata_vector                     @ 0x10 数据中止
    b .Lreserved_vector                 @ 0x14 保留
    b .Lirq_vector                      @ 0x18 IRQ
    b .Lfiq_vector                      @ 0x1C FIQ

// ---------------------------------------------------------------------------
// 主核引导入口
// ---------------------------------------------------------------------------
.Lreset_vector:
    // ---- 防御：关闭 MMU / D-Cache / I-Cache（固件一般已关闭，双保险） ----
    mrc p15, 0, r0, c1, c0, 0           // SCTLR
    bic r0, r0, #(1 << 0)               // M = 0
    bic r0, r0, #(1 << 2)               // C = 0
    bic r0, r0, #(1 << 12)              // I = 0
    mcr p15, 0, r0, c1, c0, 0
    // 使指令预取缓冲失效
    mcr p15, 0, r0, c7, c5, 4           // ICIALLU（r0 值无关紧要）

    // ---- 检测并退出 HYP 模式（若固件以 HYP 启动） ----
    mrs r0, cpsr
    and r0, r0, #0x1F                   // 取模式位
    cmp r0, #0x1A                       // 0x1A = HYP
    bne .Lno_hyp
    // 在 HYP 模式下：把返回模式设为 SVC，然后 hvc 返回
    mrs r0, cpsr
    bic r0, r0, #0x1F
    orr r0, r0, #0x13                   // SVC 模式
    msr spsr_cxsf, r0                   // 写 HYP 的 SPSR
    adr lr, .Lhyp_return
    hvc #0                              // 返回 SVC，PC=LR，CPSR=SPSR
.Lhyp_return:
.Lno_hyp:

    // ---- 确保处于 SVC 模式且关中断 ----
    mrs r0, cpsr
    bic r0, r0, #0x1F                   // 清模式位
    orr r0, r0, #0x13                   // SVC
    orr r0, r0, #0xC0                   // 禁用 IRQ|FIQ
    msr cpsr_c, r0

    // ---- 识别当前核 ----
    mrc p15, 0, r4, c0, c0, 5           // MPIDR
    and r4, r4, #0x3                    // 核号 0..3
    cmp r4, #0
    beq .Lcore0_boot

    // ---- 从核（1..3）：等待主核写入 spin 表（兼容 QEMU 全核同入口的情况） ----
    // 真实固件下从核由固件 pen 直接跳转 secondary_entry，不会走到这里
.Lsecondary_spin:
    mov r0, #0x10
    mul r0, r4, r0
    add r0, r0, #4                      // 0x14 / 0x24 / 0x34（Linux BCM2836 约定）
    ldr r1, [r0]
    cmp r1, #0
    wfe
    beq .Lsecondary_spin
    bx r1                               // 跳转到主核写入的入口

// ---------------------------------------------------------------------------
// 主核正式引导
// ---------------------------------------------------------------------------
.Lcore0_boot:
    // 设置 VBAR = 0x8020（本异常向量表基址）
    ldr r0, =_vectors
    mcr p15, 0, r0, c12, c0, 0          // VBAR

    // 设置主核 SVC 栈（链接脚本定义）
    ldr sp, =__stack_top_core0

    // 清零 BSS
    ldr r0, =__bss_start
    ldr r1, =__bss_end
    mov r2, #0
.Lbss_loop:
    cmp r0, r1
    bhs .Lbss_done
    str r2, [r0], #4
    b .Lbss_loop
.Lbss_done:

    // ---- 跳转 Rust 内核（ldr pc 自动处理 ARM→Thumb 切换） ----
    ldr pc, =kernel_main
.Lhang:
    wfi
    b .Lhang

// ---------------------------------------------------------------------------
// 从核正式入口（主核通过 pen 写入本地址唤醒；或固件直接跳转）
// ---------------------------------------------------------------------------
.global secondary_entry
secondary_entry:
    // 此时 r4 可能未设置，重新读取核号
    mrc p15, 0, r4, c0, c0, 5
    and r4, r4, #0x3

    // 每核独立 SVC 栈
    cmp r4, #1
    ldreq sp, =__stack_top_core1
    cmp r4, #2
    ldreq sp, =__stack_top_core2
    cmp r4, #3
    ldreq sp, =__stack_top_core3

    // 设置 VBAR（与主核一致）
    ldr r0, =_start
    mcr p15, 0, r0, c12, c0, 0

    mov r0, r4                          // 参数：核号
    ldr pc, =kernel_secondary_main

// ---------------------------------------------------------------------------
// 异常入口：现场保存 + 分发到 Rust
// ---------------------------------------------------------------------------
// 宏说明：
//   srsdb sp!, #0x13  把异常 LR 与 SPSR 压入 SVC 栈
//   cps #0x13         切到 SVC 模式（使用 SVC 栈与 SVC LR）
//   stmdb {r0-r12}    保存普通寄存器
//   rfeia sp!         从 SVC 栈恢复 PC/CPSR（返回被中断的上下文）
.macro EXC_ENTRY kind, adjust
    sub lr, lr, #\adjust                // 修正返回地址（IRQ/-4, DABT/-8, SVC/0）
    srsdb sp!, #0x13                    // [sp_svc]  ← lr_exc; [sp_svc+4] ← spsr_exc
    cps #0x13                           // 切换到 SVC 模式
    stmdb sp!, {r0-r12}                 // 保存 r0..r12（52 字节）
    mov r0, sp                          // r0 = 现场指针
    mov r1, #\kind                      // r1 = 异常类型
    ldr r2, =exception_dispatch         // Rust: fn(kind, ctx)
    blx r2
    ldmia sp!, {r0-r12}                 // 恢复 r0..r12
    rfeia sp!                           // PC←[sp], CPSR←[sp+4]，返回
.endm

// 异常类型编号（与 kernel/src/exception.rs 保持一致）
.equ EXC_UNDEF,     0
.equ EXC_SVC,       1
.equ EXC_PREFETCH,  2
.equ EXC_DATA,      3
.equ EXC_IRQ,       4
.equ EXC_FIQ,       5

.Lundef_vector:    EXC_ENTRY EXC_UNDEF, 4
.Lsvc_vector:      EXC_ENTRY EXC_SVC, 0
.Lprefetch_vector: EXC_ENTRY EXC_PREFETCH, 4
.Ldata_vector:     EXC_ENTRY EXC_DATA, 8
.Lreserved_vector:
    // 保留向量：不应触发；挂死
    b .
.Lirq_vector:      EXC_ENTRY EXC_IRQ, 4
.Lfiq_vector:      EXC_ENTRY EXC_FIQ, 4

// ---------------------------------------------------------------------------
// 展开表占位（消除对 libgcc 展开函数的引用）
// ---------------------------------------------------------------------------
.section .text
.weak __aeabi_unwind_cpp_pr0
.type __aeabi_unwind_cpp_pr0, %function
__aeabi_unwind_cpp_pr0:
    bx lr
.weak __aeabi_unwind_cpp_pr1
.type __aeabi_unwind_cpp_pr1, %function
__aeabi_unwind_cpp_pr1:
    bx lr

// 字面量池
.ltorg

.end
