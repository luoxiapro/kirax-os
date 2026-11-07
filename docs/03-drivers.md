# 阶段三：驱动开发（全部手写）

## 驱动清单

| 驱动 | 控制器 | 作用 |
|---|---|---|
| mini UART | 0x3F001000 | 调试串口 115200 |
| System Timer | 0x3F003000 | 1MHz 时基、调度 tick |
| GPIO | 0x3F200000 | UART 引脚复用、LED |
| Interrupt Controller | 0x3F00B000 | IRQ 分发 |
| Framebuffer | VideoCore GPU mailbox | 显示输出 |
| EMMC/SD 卡 | 0x3F300000 | 持久存储 |
| USB (DWC2) | 0x3F980000 | 键盘鼠标 |

## Framebuffer（通过 VideoCore mailbox）

- mailbox 通道 8：请求 GPU 分配 framebuffer。
- 流程：物理分辨率 → 虚拟分辨率 → 位深（32bpp BGRA）→ 拿帧缓冲物理地址和 pitch。
- 直接写内存像素即可，CPU 渲染。

## FAT32 文件系统

- `crates/kirax-core/src/fat32.rs`：只读挂载 + 读文件。
- 挂载校验：引导签名 0x55AA、BPB 解析（每簇扇区数、FAT 大小、根簇）。
- 根目录 LBA = reserved + num_fats * sectors_per_fat。
- 文件内容按 FAT 链读簇。

## USB HID（DWC2）

- 枚举设备 → 找 HID 接口 → 中断传输读键盘/鼠标报告。
- 键位映射表 + 鼠标位移累加。

## 常见坑

| 症状 | 原因 |
|---|---|
| framebuffer 黑屏 | mailbox 通道号错，或没等 GPU 响应 |
| SD 卡读超时 | EMMC 时钟频率过高，先降频到 400kHz 初始化 |
| USB 键盘无反应 | DWC2 需要先 reset PHY，枚举超时 |
