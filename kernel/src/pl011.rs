//! PL011 串口驱动（UART0）—— 树莓派 3B (BCM2837) 主串口
//!
//! 树莓派 2 使用 mini UART（AUX 外设），而树莓派 3B 的 GPIO14/15 默认接到
//! PL011（UART0，基址 0x3F201000），这是 Pi3 的推荐调试串口：
//! - 挂载在 ARM 外设总线上，时钟为 UART 时钟（本驱动按 48 MHz 计算波特率分频；
//!   QEMU 的 raspi 模拟板 PL011 时钟为 3 MHz，若 QEMU 下乱码，将 UART_CLOCK_HZ
//!   改为 3_000_000 即可）
//! - 寄存器（相对 0x3F201000）：
//!   0x00 DR     数据寄存器（写=发送，读=接收）
//!   0x18 FR     标志寄存器（bit5 TXFE 发送 FIFO 空，bit7 TXFF 发送 FIFO 满）
//!   0x24 IBRD   整数波特率分频
//!   0x28 FBRD   小数波特率分频
//!   0x2C LCRH   线控（bit4-5 字长，bit6 FIFO 使能，bit7 停止位）
//!   0x30 CR     控制（bit0 UARTEN，bit8 TXE，bit9 RXE）
//!   0x44 ICR    中断清除（写任意 1 清零）
//!
//! 初始化序列（ARM PrimeCell 手册 + 树莓派官方 BCM2837 外设手册）：
//!   1. GPIO14/15 复用 ALT0 并上拉
//!   2. CR=0 关串口 → ICR=0x7FF 清中断
//!   3. 写 IBRD/FBRD（波特率分频）
//!   4. LCRH=0x70（8 位无校验、FIFO 使能）→ 先写 LCRH 再使能
//!   5. IMSC=0 关中断 → CR=0x301 使能 TX/RX
//!   6. 发送前等待 FR.TXFF=0

use core::fmt::{self, Write};

/// 板级地址表（raspi / virt 经 `--features board_virt` 切换，见 crate::board）。
use crate::board::current as b;

/// 外设基址（BCM2837 与 BCM2709 相同，AArch32 模式；raspi 专属，
/// virt 平台无此总线，GPIO/CPRMAN 初始化路径在 virt 下为已知待适配项）
const PERIPHERAL_BASE: usize = 0x3F00_0000;
/// PL011 (UART0) 基址 —— 来自板级地址表
const PL011_BASE: usize = b::PL011_BASE;
/// GPIO 基址
const GPIO_BASE: usize = PERIPHERAL_BASE + 0x200_000;
/// CPRMAN 时钟管理器基址（配置 UART 时钟，QEMU 默认时钟树未使能）
const CPRMAN_BASE: usize = PERIPHERAL_BASE + 0x101_000;

/// UART 时钟：来自板级地址表（raspi 由本驱动切到 19.2 MHz 晶振源；
/// virt 为固定 24 MHz，无需 CPRMAN 开关）。
const UART_CLOCK_HZ: u32 = b::PL011_CLOCK_HZ;
/// 目标波特率
const BAUD: u32 = 115_200;

const REG_DR: *mut u32 = (PL011_BASE + 0x00) as *mut u32;
const REG_FR: *mut u32 = (PL011_BASE + 0x18) as *mut u32;
const REG_IBRD: *mut u32 = (PL011_BASE + 0x24) as *mut u32;
const REG_FBRD: *mut u32 = (PL011_BASE + 0x28) as *mut u32;
const REG_LCRH: *mut u32 = (PL011_BASE + 0x2C) as *mut u32;
const REG_CR: *mut u32 = (PL011_BASE + 0x30) as *mut u32;
const REG_IMSC: *mut u32 = (PL011_BASE + 0x38) as *mut u32;
const REG_ICR: *mut u32 = (PL011_BASE + 0x44) as *mut u32;

// CPRMAN：CM_UARTCTL (0x78) / CM_UARTDIV (0x7C)
const REG_CM_UARTCTL: *mut u32 = (CPRMAN_BASE + 0x78) as *mut u32;
const REG_CM_UARTDIV: *mut u32 = (CPRMAN_BASE + 0x7C) as *mut u32;

// GPIO 寄存器（用于 14/15 引脚复用）
const REG_GPFSEL0: *mut u32 = (GPIO_BASE + 0x00) as *mut u32;
const REG_GPPUD: *mut u32 = (GPIO_BASE + 0x94) as *mut u32;
const REG_GPPUDCLK0: *mut u32 = (GPIO_BASE + 0x98) as *mut u32;

/// FR 寄存器标志
const FR_TXFF: u32 = 1 << 7; // 发送 FIFO 满
#[allow(dead_code)]
const FR_TXFE: u32 = 1 << 5; // 发送 FIFO 空（保留：状态读取调试用）

/// 简单的忙等待延时（GPIO 上拉时序用，周期数足够）
#[inline]
fn delay_cycles(n: u32) {
    let mut i = n;
    while i > 0 {
        unsafe { core::arch::asm!("nop") }
        i -= 1;
    }
}

#[inline]
fn read32(addr: *mut u32) -> u32 {
    unsafe { core::ptr::read_volatile(addr) }
}

#[inline]
fn write32(addr: *mut u32, val: u32) {
    unsafe { core::ptr::write_volatile(addr, val) }
}

/// 配置 UART 时钟：使能 CM_UARTCTL，源 = osc（19.2 MHz），分频 = 1。
/// QEMU 的 cprman 时钟树在复位后 uart mux 未使能（时钟为 0），PL011 会卡在
/// TXFF 等待；真机固件虽已配时钟，但自配后两平台行为一致，波特率恒按 19.2MHz 计算。
/// 注意：QEMU 的 cprman 将 BUSY 位在使能后恒置 1（模拟简化），故这里不等待
/// BUSY 清除，改用短延时保证写操作稳定，真机同样适用。
fn enable_uart_clock() {
    // 1) 关时钟（清 ENABLE）
        write32(REG_CM_UARTCTL, 0);
        // 2) 分频 = 1（DIVI=1）
        write32(REG_CM_UARTDIV, 1);
        // 3) 使能：ENABLE=1，SRC=0（osc 19.2MHz）
        write32(REG_CM_UARTCTL, 1);
        // 4) 短延时等待时钟稳定（不依赖 BUSY 位；QEMU 下 BUSY 恒 1，真机下 10µs 足够）
        delay_cycles(300);
}

/// PL011 串口
pub struct Pl011 {
    initialized: bool,
}

impl Pl011 {
    pub const fn new() -> Self {
        Pl011 { initialized: false }
    }

    /// 初始化 PL011（幂等）
    pub fn init(&mut self) {
        if self.initialized {
            return;
        }

        // ---- 0. 配置 UART 时钟（cprman，19.2MHz osc） ----
        enable_uart_clock();

        // ---- 1. GPIO14/15 → ALT0（UART0 TXD/RXD） ----
        // GPFSEL0 bit14-19 清零（ALT0 编码 000）
            let fsel = read32(REG_GPFSEL0);
            write32(REG_GPFSEL0, fsel & !(0b111_111 << 14));

            // 上拉序列：GPPUD=2（上拉）→ 等 150 周期 → GPPUDCLK0 bit14/15=1
            //          → 等 150 周期 → GPPUDCLK0=0 → GPPUD=0
            write32(REG_GPPUD, 2);
            delay_cycles(150);
            write32(REG_GPPUDCLK0, (1 << 14) | (1 << 15));
            delay_cycles(150);
            write32(REG_GPPUDCLK0, 0);
            write32(REG_GPPUD, 0);

        // ---- 2. 关串口 + 清中断 ----
        write32(REG_CR, 0);
        write32(REG_ICR, 0x7FF);

        // ---- 3. 波特率分频：IBRD/FBRD = UART_CLOCK/(16·baud) ----
        //   整数部分 = 商；小数部分 = round(余数/除数 × 64)
        let divisor = UART_CLOCK_HZ / (16 * BAUD);
        let remainder = UART_CLOCK_HZ % (16 * BAUD);
        let ibrd = divisor;
        let fbrd = ((remainder * 64) + (16 * BAUD) / 2) / (16 * BAUD);
        write32(REG_IBRD, ibrd);
        write32(REG_FBRD, fbrd);

        // ---- 4. 线控：8 位、无校验、FIFO 使能 ----
        write32(REG_LCRH, 0x70); // WLEN=11 (8bit), FEN=1

        // ---- 5. 关中断，使能 TX/RX ----
        write32(REG_IMSC, 0);
        write32(REG_CR, (1 << 0) | (1 << 8) | (1 << 9)); // UARTEN | TXE | RXE

        self.initialized = true;
    }

    /// 发送单字节（忙等待 FIFO 空，带超时保护防止硬件异常挂死）
    pub fn putc(&self, c: u8) {
        // 等待发送 FIFO 不满；QEMU 时钟未就绪时 TXFF 可能恒 1，超时兜底。
        // 用普通 nop 而不是 yield：TCG 模拟下 yield 会让步导致超时极慢。
        let mut spin: u32 = 0;
        while read32(REG_FR) & FR_TXFF != 0 {
            spin += 1;
            if spin > 200_000 {
                break;
            }
            unsafe { core::arch::asm!("nop") }
        }
        write32(REG_DR, c as u32);
    }

    /// 发送字符串
    pub fn puts(&self, s: &str) {
        for &b in s.as_bytes() {
            // 换行前补 \r，避免终端只进不退行
            if b == b'\n' {
                self.putc(b'\r');
            }
            self.putc(b);
        }
    }

    /// 阻塞读单字节
    pub fn getc(&self) -> u8 {
        // RXFE = bit4：接收 FIFO 空
        while read32(REG_FR) & (1 << 4) != 0 {
            core::hint::spin_loop();
        }
        (read32(REG_DR) & 0xFF) as u8
    }

    /// 非阻塞读
    pub fn getc_nowait(&self) -> Option<u8> {
        if read32(REG_FR) & (1 << 4) != 0 {
            None
        } else {
            Some((read32(REG_DR) & 0xFF) as u8)
        }
    }
}

impl Write for Pl011 {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.puts(s);
        Ok(())
    }
}


