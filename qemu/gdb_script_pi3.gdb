# QEMU + GDB 调试 Space OS 内核（树莓派 3B，AArch32 构建）
# 用法:
#   终端1: make pi3-qemu   (或 qemu-system-arm -M raspi2b -kernel build/pi3/qemu-kernel.elf -display none -serial stdio -s -S)
#   终端2: gdb-multiarch -x qemu/gdb_script_pi3.gdb build/pi3/qemu-kernel.elf

set pagination off
set confirm off
set architecture arm
target remote :1234

# 加载符号（Pi3 QEMU 重定位版 ELF，0x10000）
symbol-file build/pi3/qemu-kernel.elf

# 在内核 main 入口下断点
break kernel_main
# 在 panic 处理函数下断点
break rust_begin_unwind

# 复位并继续到第一条指令
continue

# 常用命令（在 gdb 中手动执行）:
#   info registers            查看寄存器
#   x/16xw 0x10000           查看内存
#   stepi / nexti             单步
#   break *0x10100            地址断点
#   layout asm                汇编窗口
#   monitor info registers    QEMU 监控
#   continue / ctrl+c         继续 / 中断
