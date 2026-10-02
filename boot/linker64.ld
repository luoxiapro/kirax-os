/*
 * Space OS AArch64 (BCM2837, Raspberry Pi 3) 链接脚本
 * 入口基址 0x80000（树莓派 AArch64 固件 / QEMU raspi3b FIRMWARE_ADDR_3）
 */

ENTRY(_start)

MEMORY
{
    RAM (rwx) : ORIGIN = 0x00080000, LENGTH = 1024M
}

SECTIONS
{
    . = 0x00080000;

    .text : {
        KEEP(*(.text._start))
        *(.text .text.*)
    } > RAM

    .rodata : {
        *(.rodata .rodata.*)
    } > RAM

    .data : {
        *(.data .data.*)
    } > RAM

    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    } > RAM

    /* 内核结束符号（space-core 堆边界用） */
    __kernel_end = .;

    /* 栈：主核 16KB，放在物理 RAM 顶部之下（0x3FFC_0000）。
       - 必须用绝对符号赋值，不能用「空节 + location counter」：
         rust-lld 会丢弃空节并把符号解析到 .bss 之后的低地址，
         导致栈向下增长覆盖静态区与内核堆（64 位 buddy 分配失败的根因）；
       - 0x3FFC0000 在物理 RAM 有效范围（< 0x4000_0000），且落在
         MMU L1[0] 的 Normal 1GB block 内。 */
    __stack_top = 0x3FFC0000;

    /DISCARD/ : {
        *(.ARM.exidx*)
        *(.comment)
    }
}
