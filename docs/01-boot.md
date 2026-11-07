# 阶段一：环境搭建与裸机点亮 UART Hello World

## 目标
在树莓派 2（BCM2709，ARMv7-A）上用 ARMv7 汇编完成：
- 设置 SVC 模式与栈指针
- mini UART 初始化
- 输出 "Hello, KiraX OS!"
- 链接脚本 + Makefile，先 QEMU 后真机

## 工具链

```bash
sudo apt-get install -y qemu-system-arm gcc-arm-none-eabi gdb-multiarch
rustup target add thumbv7a-none-eabi
export PATH="$HOME/.cargo/bin:$PATH"
```

- `arm-none-eabi-gcc`：汇编 start.S（Rust 内核链接时需要 start.o）
- `qemu-system-arm -M raspi2b`：仿真
- `gdb-multiarch`：远程调试
- Rust `thumbv7a-none-eabi`：裸机 target

## 关键代码

### boot/start.S（双态启动 stub）
树莓派的 GPU 在 0x8000 加载内核，但 QEMU 把 `-kernel` ELF 加载到 0x10000。
用双态 stub：前 8 字节是两个跳变指令，分别在 0x8000 和 0x10000 都能跳到 _vectors。

```armasm
.global _start
_start:
    // 双态 stub：0x8000 和 0x10000 都能正确跳转
    .byte 0x00, 0x00, 0x08, 0xA0   // add r0, pc, #0x8000（真机）
    .byte 0x00, 0x47, 0x00, 0x00   // bx r0
_vectors:
    ldr pc, =kernel_main            // 复位向量
    // ... 其余异常向量占位
```

### 链接脚本
- 真机 `linker.ld`：`. = 0x8000`
- QEMU `linker-qemu.ld`：`. = 0x10000`

## 构建与运行

```bash
make qemu        # QEMU 运行
make all         # 生成 build/kernel7.img（真机）
```

## 常见坑

| 症状 | 原因 | 修复 |
|---|---|---|
| QEMU 无任何输出 | QEMU 加载到 0x10000 但链接在 0x8000 | 用 linker-qemu.ld |
| UART 一直等 LSR | LSR bit5=TX FIFO 空（不是 bit6） | 等 bit5 |
| putc 卡死 | 没等 LSR 直接写 DR | 先查 LSR |
| 真机无输出 | GPU 需要 kernel7.img 而非 ELF | `objcopy -O binary` |

## GDB 调试

```bash
# 终端1
qemu-system-arm -M raspi2b -kernel build/qemu-kernel.elf -display none -serial stdio -s -S
# 终端2
gdb-multiarch -x qemu/gdb_script.gdb build/qemu-kernel.elf
```
