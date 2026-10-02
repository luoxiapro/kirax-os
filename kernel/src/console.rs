//! 控制台适配层：运行时按 SoC 自动选择串口驱动。
//!
//! - 树莓派 2（Cortex-A7）：mini UART（AUX 外设）
//! - 树莓派 3B（Cortex-A53）：PL011（UART0，GPIO14/15 主串口）
//! - AArch64（仅 Pi3）：PL011
//!
//! 一份内核镜像同时携带两个驱动，启动时通过 `soc::use_pl011()` 判定
//! 输出走哪个串口；两个驱动的 `init()` 都会执行（AUX 与 UART0 是不同
//! 外设，互不干扰），之后所有输出按选定端口转发。
//!
//! 内核其它模块一律通过 `console::CONSOLE` 与 `klog!` 输出，不感知硬件差异。

use crate::pl011::Pl011;
use crate::soc;
use crate::sync::SpinLock;
use core::fmt;

#[cfg(not(target_arch = "aarch64"))]
use crate::uart::MiniUart;

/// 串口端口选择
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SerialKind {
    Mini,
    Pl011,
}

/// 统一控制台：同时持有 mini UART 与 PL011 驱动，按 `kind` 转发输出。
pub struct Console {
    kind: SerialKind,
    #[cfg(not(target_arch = "aarch64"))]
    mini: MiniUart,
    pl011: Pl011,
}

impl Console {
    pub const fn new() -> Self {
        Console {
            kind: SerialKind::Pl011, // 初始占位，init() 时按 SoC 修正
            #[cfg(not(target_arch = "aarch64"))]
            mini: MiniUart::new(),
            pl011: Pl011::new(),
        }
    }

    /// 初始化全部串口驱动，并按 SoC 选定输出端口。
    pub fn init(&mut self) {
        #[cfg(not(target_arch = "aarch64"))]
        self.mini.init();
        self.pl011.init();
        self.kind = if soc::use_pl011() {
            SerialKind::Pl011
        } else {
            SerialKind::Mini
        };
    }

    #[inline]
    pub fn putc(&self, c: u8) {
        match self.kind {
            SerialKind::Mini => {
                #[cfg(not(target_arch = "aarch64"))]
                self.mini.putc(c);
            }
            SerialKind::Pl011 => self.pl011.putc(c),
        }
    }

    #[inline]
    pub fn puts(&self, s: &str) {
        match self.kind {
            SerialKind::Mini => {
                #[cfg(not(target_arch = "aarch64"))]
                self.mini.puts(s);
            }
            SerialKind::Pl011 => self.pl011.puts(s),
        }
    }

    /// 阻塞读一个字节
    #[inline]
    pub fn getc(&self) -> u8 {
        if self.kind == SerialKind::Pl011 {
            self.pl011.getc()
        } else {
            #[cfg(not(target_arch = "aarch64"))]
            {
                self.mini.getc()
            }
            #[cfg(target_arch = "aarch64")]
            {
                unreachable!("AArch64 恒为 PL011")
            }
        }
    }

    /// 非阻塞读（无数据返回 None）
    #[inline]
    pub fn getc_nowait(&self) -> Option<u8> {
        if self.kind == SerialKind::Pl011 {
            self.pl011.getc_nowait()
        } else {
            #[cfg(not(target_arch = "aarch64"))]
            {
                self.mini.getc_nowait()
            }
            #[cfg(target_arch = "aarch64")]
            {
                unreachable!("AArch64 恒为 PL011")
            }
        }
    }
}

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.puts(s);
        Ok(())
    }
}

/// 全局控制台（内核各模块经 klog!/console::CONSOLE 使用）
pub static CONSOLE: SpinLock<Console> = SpinLock::new(Console::new());

/// 打印格式化日志（内核各模块使用）：`klog!("...", args)`
/// 输出目标由 `console` 模块按 SoC 运行时转发。
#[macro_export]
macro_rules! klog {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let mut c = $crate::console::CONSOLE.lock();
        let _ = write!(c, "[SpaceOS] ");
        let _ = write!(c, $($arg)*);
        let _ = c.write_str("\n");
    }};
}
