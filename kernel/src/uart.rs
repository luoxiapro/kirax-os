//! mini UART 串口驱动（BCM2835/2836 外设基址 0x3F000000 的 AUX 外设）。
//!
//! 树莓派 2 上 mini UART（UART1）挂在 AUX 外设上：
//! - 数据寄存器 AUX_MU_IO（8 位）
//! - 时钟固定 250 MHz（BCM2836），波特率寄存器 = 250_000_000/(8·baud) - 1
//! - 引脚 GPIO14(TXD) / GPIO15(RXD)，复用功能 5（ALT5）
//!
//! 初始化顺序（官方文档约定）：
//!   1. GPIO14/15 切到 ALT5 并上拉
//!   2. 使能 AUX
//!   3. 清零 IER / CNTL
//!   4. LCR=3（8 位），MCR=0
//!   5. 清 FIFO（IIR=0xC6）
//!   6. 设波特率 115200
//!   7. 使能 TX+RX（CNTL=3）
//!
//! 本驱动同时实现 `core::fmt::Write`，panic 与内核日志均走这里。

use core::fmt::{self, Write};

/// BCM2835 外设基址（树莓派 2）
pub const PERIPHERAL_BASE: usize = 0x3F00_0000;
/// AUX 外设基址
const AUX_BASE: usize = PERIPHERAL_BASE + 0x21_5000;
/// GPIO 基址
const GPIO_BASE: usize = PERIPHERAL_BASE + 0x20_0000;

const AUX_ENABLE: *mut u32 = (AUX_BASE + 0x04) as *mut u32;
const AUX_MU_IO: *mut u32 = (AUX_BASE + 0x40) as *mut u32;
const AUX_MU_IER: *mut u32 = (AUX_BASE + 0x44) as *mut u32;
const AUX_MU_IIR: *mut u32 = (AUX_BASE + 0x48) as *mut u32;
const AUX_MU_LCR: *mut u32 = (AUX_BASE + 0x4C) as *mut u32;
const AUX_MU_MCR: *mut u32 = (AUX_BASE + 0x50) as *mut u32;
const AUX_MU_LSR: *mut u32 = (AUX_BASE + 0x54) as *mut u32;
const AUX_MU_CNTL: *mut u32 = (AUX_BASE + 0x60) as *mut u32;
const AUX_MU_BAUD: *mut u32 = (AUX_BASE + 0x68) as *mut u32;

const GPFSEL1: *mut u32 = (GPIO_BASE + 0x04) as *mut u32;
const GPPUD: *mut u32 = (GPIO_BASE + 0x94) as *mut u32;
const GPPUDCLK0: *mut u32 = (GPIO_BASE + 0x98) as *mut u32;

/// mini UART 时钟（BCM2836 固定 250 MHz；BCM2835 随核心时钟变化，见坑点文档）
const AUX_CLOCK: u32 = 250_000_000;
/// 默认波特率
const BAUD: u32 = 115_200;

#[inline]
unsafe fn read32(p: *mut u32) -> u32 {
    core::ptr::read_volatile(p)
}
#[inline]
unsafe fn write32(p: *mut u32, v: u32) {
    core::ptr::write_volatile(p, v)
}

/// 简易忙等待（编译期常量循环，供初始化时序用）
#[inline]
fn delay(cycles: u32) {
    let mut i = cycles;
    while i > 0 {
        core::hint::spin_loop();
        i -= 1;
    }
}

/// mini UART 驱动
#[derive(Debug, Clone, Copy, Default)]
pub struct MiniUart {
    initialized: bool,
}

impl MiniUart {
    pub const fn new() -> Self {
        Self {
            initialized: false,
        }
    }

    /// 初始化（GPIO → 使能 → 配置 → 波特率）
    pub fn init(&mut self) {
        unsafe {
            // 1. GPIO14/15 → ALT5（mini UART）
            let mut sel = read32(GPFSEL1);
            sel &= !(0b111 << 12); // GPIO14 清位
            sel |= 0b010 << 12; // ALT5
            sel &= !(0b111 << 15); // GPIO15 清位
            sel |= 0b010 << 15; // ALT5
            write32(GPFSEL1, sel);

            // 2. GPIO14/15 上拉（mini UART 空闲高电平）
            write32(GPPUD, 0b10);
            delay(150);
            write32(GPPUDCLK0, (1 << 14) | (1 << 15));
            delay(150);
            write32(GPPUDCLK0, 0);
            write32(GPPUD, 0);

            // 3. 使能 mini UART
            let en = read32(AUX_ENABLE);
            write32(AUX_ENABLE, en | 1);

            // 4. 配置：先关闭 TX/RX
            write32(AUX_MU_IER, 0);
            write32(AUX_MU_CNTL, 0);
            write32(AUX_MU_LCR, 3); // 8 位
            write32(AUX_MU_MCR, 0);
            write32(AUX_MU_IIR, 0xC6); // 清空收发 FIFO

            // 5. 波特率：reg = 时钟/(8·波特) - 1
            write32(AUX_MU_BAUD, AUX_CLOCK / (8 * BAUD) - 1);

            // 6. 使能 TX + RX
            write32(AUX_MU_CNTL, 3);

            // 清空残留
            while read32(AUX_MU_LSR) & 1 != 0 {
                read32(AUX_MU_IO);
            }
            self.initialized = true;
        }
    }

    /// 发送单字节（等待 TX FIFO 可写）
    pub fn putc(&self, c: u8) {
        unsafe {
            // LSR bit5 = TX FIFO 空（可写）；bit6 = TX IDLE（传输完成，空闲）。
            // 写入前必须等 bit5 置 1。注意：若误等 bit6，在 QEMU 与真机上
            // 都会因 bit6 长时间保持 1 而进入死循环。
            while read32(AUX_MU_LSR) & (1 << 5) == 0 {
                core::hint::spin_loop();
            }
            write32(AUX_MU_IO, c as u32);
        }
    }

    /// 发送字符串（逐个字符，'\n' 补 '\r' 便于串口终端换行）
    pub fn puts(&self, s: &str) {
        for &b in s.as_bytes() {
            if b == b'\n' {
                self.putc(b'\r');
            }
            self.putc(b);
        }
    }

    /// 接收单字节（阻塞等待）
    pub fn getc(&self) -> u8 {
        unsafe {
            while read32(AUX_MU_LSR) & 1 == 0 {
                core::hint::spin_loop();
            }
            read32(AUX_MU_IO) as u8
        }
    }

    /// 非阻塞接收
    pub fn getc_nowait(&self) -> Option<u8> {
        unsafe {
            if read32(AUX_MU_LSR) & 1 != 0 {
                Some(read32(AUX_MU_IO) as u8)
            } else {
                None
            }
        }
    }
}

impl Write for MiniUart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.puts(s);
        Ok(())
    }
}


