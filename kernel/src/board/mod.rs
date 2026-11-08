//! # board —— 板级支持包（BSP）抽象层
//!
//! KiraX OS 从 2.0.0 起不再绑定树莓派：内核主体（调度/内存/文件系统/桌面/
//! syscall）与板级外设解耦，所有硬件地址集中在本模块，新增平台 = 新增一个
//! `pub mod <board>` 并在底部 `current` 别名切换。
//!
//! 当前板级：
//! - [`raspi`]：Raspberry Pi 2 / 3（BCM2836 / BCM2837），生产配置，已在 QEMU
//!   raspi2b 与真机上验证。
//! - [`virt`]：QEMU ARM `virt` 机器（`qemu-system-arm -M virt`），PC 虚拟机
//!   通用平台骨架，地址表已就绪，驱动适配在进行中。
//!
//! 切换平台：`cargo build --features board_virt`。

pub mod raspi {
    /// ARM 外设总线基址（Pi2/Pi3 32 位视图）。
    pub const PERIPHERAL_BASE: usize = 0x3F00_0000;
    /// PL011 UART0（主调试串口）。
    pub const PL011_BASE: usize = PERIPHERAL_BASE + 0x201_000;
    /// GPIO（用于 ALT 复用切换 UART）。
    pub const GPIO_BASE: usize = PERIPHERAL_BASE + 0x200_000;
    /// CM_PLL 时钟管理器（UART 时钟开关与分频）。
    pub const CPRMAN_BASE: usize = PERIPHERAL_BASE + 0x101_000;
    /// PL011 输入时钟（真机 48 MHz；QEMU raspi 模拟为 3 MHz，乱码时改之）。
    pub const PL011_CLOCK_HZ: u32 = 19_200_000;
    /// VC 邮箱（framebuffer/电源）。
    pub const MAILBOX_BASE: usize = PERIPHERAL_BASE + 0x00B8_8000;
    /// SD/MMC 控制器（SD 卡）。
    pub const SDHCI_BASE: usize = PERIPHERAL_BASE + 0x0030_0000;
}

pub mod virt {
    //! QEMU ARM virt（`-M virt`）地址表 —— 骨架。
    //!
    //! virt 与 raspi 的关键差异：
    //! - 无 BCM GPIO/CPRMAN：PL011 上电即可用，无需 ALT 复用与时钟开关；
    //! - 中断控制器为 GIC（raspi 为中断控制器 + 局部定时器）；
    //! - 块设备走 virtio-mmio 而非 SDHCI。
    //!
    //! 本表用于 `--features board_virt` 编译时地址切换；驱动层适配
    //! （GIC 接管、virtio-mmio、无 GPIO 初始化）在 1.x 路线图。

    /// virt 外设总片区基址。
    pub const PERIPHERAL_BASE: usize = 0x0800_0000;
    /// PL011 UART0（virt 上唯一串口）。
    pub const PL011_BASE: usize = 0x0900_0000;
    /// virt PL011 固定 24 MHz。
    pub const PL011_CLOCK_HZ: u32 = 24_000_000;
    /// GIC 分发器。
    pub const GIC_DIST_BASE: usize = PERIPHERAL_BASE + 0x10_0000;
    /// GIC CPU 接口。
    pub const GIC_CPU_BASE: usize = PERIPHERAL_BASE + 0x10_1000;
    /// virtio-mmio 块设备起点。
    pub const VIRTIO_MMIO_BASE: usize = PERIPHERAL_BASE + 0x02_0000;
}

#[cfg(not(feature = "board_virt"))]
pub use raspi as current;

#[cfg(feature = "board_virt")]
pub use virt as current;
