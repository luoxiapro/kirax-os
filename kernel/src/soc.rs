//! SoC 运行时识别（统一镜像支持 Pi2 / Pi3 的关键）。
//!
//! 背景：早期按编译期 feature（`--features pi3`）选择串口驱动，导致 Pi2 与
//! Pi3 必须各自构建一份 kernel7.img。本模块改为**启动时读 CPU 型号寄存器
//! MIDR/MIDR_EL1** 判断 SoC，一份 32 位内核即可在 Pi2 / Pi3 上自动选择
//! mini UART 或 PL011，配合 AArch64 的 kernel8.img 实现「一张 SD 卡、
//! 一个镜像」覆盖全部机型。
//!
//! 型号判定（ARMv7/ARMv8 通用）：
//! - Cortex-A7  （Pi2，BCM2709/BCM2836）：MIDR PartNum = 0xC07
//! - Cortex-A53 （Pi3，BCM2837）：MIDR PartNum = 0xD03
//! 掩码 `(midr >> 4) & 0xFFF` 提取 PartNum（ARM ARM：MIDR[15:4]）。

/// 读 CPU 主标识寄存器（32 位：MRC p15,0,c0,c0,0；64 位：MRS MIDR_EL1）。
/// 两个模式下该寄存器的值布局一致，PartNum 都在 [15:4]。
#[cfg(target_arch = "aarch64")]
#[inline]
fn read_midr() -> u32 {
    let v: u32;
    // SAFETY: MIDR_EL1 为只读系统寄存器，任何 EL ≥ 1 均可访问。
    unsafe {
        core::arch::asm!("mrs {0:w}, MIDR_EL1", out(reg) v, options(nomem, nostack));
    }
    v
}

/// 读 CPU 主标识寄存器（ARMv7-A：协处理器 CP15 指令）。
#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn read_midr() -> u32 {
    let v: u32;
    // SAFETY: MIDR 为只读，SVC 模式下 MRC 合法。
    unsafe {
        core::arch::asm!("mrc p15, 0, {0}, c0, c0, 0", out(reg) v, options(nomem, nostack));
    }
    v
}

/// CPU 型号 PartNum（MIDR[15:4]）
#[inline]
fn part_num() -> u32 {
    (read_midr() >> 4) & 0xFFF
}

/// 是否为树莓派 3（Cortex-A53 → BCM2837）。
/// 树莓派 2（Cortex-A7 → BCM2709/BCM2836）为 false。
#[inline]
pub fn is_pi3() -> bool {
    part_num() == 0xD03
}

/// 是否为树莓派 2（Cortex-A7 → BCM2709/BCM2836）。
#[inline]
pub fn is_pi2() -> bool {
    part_num() == 0xC07
}

/// 串口选型：Pi3 用 PL011（UART0，GPIO14/15 主串口），Pi2 用 mini UART（AUX）。
/// 默认按 SoC 运行时自动检测（统一镜像）；`--features pi3` 强制 PL011，
/// 供 QEMU raspi2b（仅模拟 A7）验证 32 位 PL011 驱动路径。
/// AArch64 仅存在于 Pi3，恒为 PL011。
#[cfg(not(target_arch = "aarch64"))]
#[inline]
pub fn use_pl011() -> bool {
    #[cfg(feature = "pi3")]
    {
        return true; // 强制 PL011（调试 / 路径验证）
    }
    #[cfg(not(feature = "pi3"))]
    {
        is_pi3()
    }
}

#[cfg(target_arch = "aarch64")]
#[inline]
pub fn use_pl011() -> bool {
    true
}
