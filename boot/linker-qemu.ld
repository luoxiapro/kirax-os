/* ============================================================================
 * Space OS 链接脚本
 * 树莓派 2 固件把 kernel7.img（原始二进制）加载到 0x8000 并跳转。
 * 布局：
 *   0x00008000        _start（异常向量表 + 启动代码）
 *   ...               .text / .rodata / .data / .bss
 *   ...               .stacks（4 核 × 64KB 内核栈，NOBITS 不占文件空间）
 *   __kernel_end      内核镜像结束（伙伴分配器从此处开始）
 * ==========================================================================*/

ENTRY(_start)

/* 栈区布局：每核 64KB，共 256KB */
__STACK_SIZE = 64K;

SECTIONS
{
    . = 0x00010000;

    .text : ALIGN(4) {
        KEEP(*(.text._start))          /* 保证 _start 位于 0x8000（向量表基址） */
        *(.text .text.*)
    }

    .rodata : ALIGN(4) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4) {
        __data_start = .;
        *(.data .data.*)
        __data_end = .;
    }

    .bss (NOLOAD) : ALIGN(4) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* 4 核内核栈（NOBITS：不占 kernel7.img 文件空间） */
    .stacks (NOLOAD) : ALIGN(16) {
        __stacks_start = .;
        . += 4 * __STACK_SIZE;
        __stacks_end = .;
    }

    . = ALIGN(4096);
    __kernel_end = .;

    /* 丢弃 ARM 展开表（panic=abort，无需栈展开） */
    /DISCARD/ : {
        *(.ARM.exidx*)
        *(.ARM.extab*)
    }
}

/* 每核栈顶（栈向下增长） */
__stack_top_core0 = __stacks_start + 1 * __STACK_SIZE;
__stack_top_core1 = __stacks_start + 2 * __STACK_SIZE;
__stack_top_core2 = __stacks_start + 3 * __STACK_SIZE;
__stack_top_core3 = __stacks_start + 4 * __STACK_SIZE;
