# 阶段二：内核核心（Rust + MMU + 调度 + 系统调用）

## 语言选型：Rust

- **内存安全**：编译期消除 use-after-free、越界，裸机无 libc 也安全。
- **无 GC**：`#![no_std]` + 自定义 global allocator，确定性内存分配。
- **ARMv7 支持**：`thumbv7a-none-eabi` target 官方支持，内联汇编稳定。
- **C 互操作**：`extern "C"` 直接调汇编 stub。
- **对比 C**：无未定义行为陷阱（整数溢出在 debug 模式直接 panic），适合 1GB 内存的小型 OS。

## 异常向量与中断

- ARMv7 短描述符 VBAR 指向 `_vectors`（偏移 0x20）。
- BCM2709 中断控制器（BASE 0x3F00B000）：IRQ basic / IRQ1 / IRQ2 寄存器。
- 系统定时器（0x3F003000，1MHz）：100Hz tick。

## MMU 与页表

- ARMv7-A 短描述符一级页表（4KB 页）。
- 启动阶段用 1:1 映射：
  - RAM `0x00000000–0x3BFFFFFF`（~960MB）
  - 外设 `0x3F000000–0x40100000`
- 后续阶段再切到分页 + 用户/内核空间隔离。

## 物理内存布局（1GB）

| 地址范围 | 用途 | 大小 |
|---|---|---|
| 0x00000000–0x00200000 | 内核镜像（.text/.data/.bss） | 2MB |
| 0x00200000–0x3C000000 | 伙伴分配器堆 | ~959MB |
| 0x3F000000 | BCM2709 外设（UART/GPIO/Timer） | 16MB |
| 0x40000000 | ARM-local（中断控制器、CPU 私有定时器） | — |

## 伙伴分配器

- `crates/kirax-core/src/buddy.rs`，阶语义用**相对阶**（min_order=12 即 4KB 为最小块）。
- 块头：magic@0(u32) + next@8(u64)，next=u64::MAX 表示链尾。
- host 端 `cargo test` 单测覆盖：正常分配、零字节、释放合并、重复释放拒绝、碎片统计。

## 四核启动

- CPU0 跑内核；CPU1-3 在 `_start` 处用 mailbox 协议或自旋锁停在自旋循环。
- 调度器就绪后通过释放自旋变量唤醒从核。

## 系统调用

- ARMv7 SVC 指令；寄存器 r0-r6 传参，r7 为 syscall号。
- 首批：`exit`、`write`、`mmap`、`open`、`read`、`poll`。

## 常见坑

| 症状 | 原因 |
|---|---|
| 启动即卡死 | VBAR 没设或向量表未 32 字节对齐 |
| MMU 开后死机 | 页表基址 TTBR0 未刷 TLB |
| 中断不进 | CPSR IRQ 位未清零，或中断控制器未 unmask |
| buddy 越界 panic | next 哨兵 u64::MAX 被当索引 → idx_to_addr 乘法溢出 |
