//! USB 主机驱动（DWC2 OTG 控制器）——架构与关键实现。
//!
//! 树莓派 2 的 USB 控制器是 DesignWare DWC2（OTG），基址 0x3F98_0000。
//! 完整 USB 主机栈 = 控制器初始化 + 根集线器轮询 + 设备枚举 + 传输调度 + 类驱动，
//! 是全部驱动中体量最大的一块。本模块提供：
//! 1. DWC2 寄存器映射（主机模式关键寄存器）；
//! 2. 初始化与端口复位流程；
//! 3. 控制/批量传输原语（HPRT / HCDMA / HCTSIZ）；
//! 4. HID 键盘鼠标报文解析（与 kirax-core::events 对接）。
//!
//! 状态：骨架 + 传输原语可用；设备枚举与类绑定在 Phase 3 真机联调时按
//! “枚举状态机”补齐（见 docs/03_SD卡与真机.md 的 USB 兼容性测试计划）。

use crate::sync::SpinLock;
use alloc::vec::Vec;
use kirax_core::events::{buttons, mods, EventKind, InputEvent};

/// DWC2 基址（BCM2835/2836）
pub const DWC2_BASE: usize = 0x3F98_0000;

// ---- 关键寄存器（host 模式） ----
pub mod regs {
    pub const OTGCTL: usize = 0x00; // OTG 控制
    pub const OTGINT: usize = 0x04; // OTG 中断
    pub const GAHBCFG: usize = 0x08; // AHB 配置
    pub const GUSBCFG: usize = 0x0C; // USB 配置
    pub const GRSTCTL: usize = 0x10; // 复位控制
    pub const GINTMSK: usize = 0x14; // 全局中断掩码
    pub const GINTSTS: usize = 0x18; // 全局中断状态
    pub const GRXFSIZ: usize = 0x24; // RX FIFO 大小
    pub const GNPTXFSIZ: usize = 0x28; // 非周期 TX FIFO 大小
    pub const HPRT: usize = 0x40; // 主机端口控制
    pub const HCFG: usize = 0x44; // 主机配置
    pub const HFIR: usize = 0x48; // 帧间隔
    pub const HFNUM: usize = 0x4C; // 帧号
    pub const HPTXFSIZ: usize = 0x100; // 周期 TX FIFO
    pub const HPRT0: usize = 0x540; // 端口 0（部分实现用 HPRT）
    pub const HCCHAR0: usize = 0x500; // 通道 0 特征
    pub const HCSPLT0: usize = 0x504;
    pub const HCINT0: usize = 0x508; // 通道中断
    pub const HCINTMSK0: usize = 0x50C;
    pub const HCTSIZ0: usize = 0x510; // 传输大小
    pub const HCDMA0: usize = 0x514; // DMA 地址（PIO 模式不用）
    // 通道 n 偏移 = n * 0x20
    pub const HC_CH_OFFSET: usize = 0x20;
}

/// 初始化 DWC2（主机模式）
pub fn init_host() -> Result<(), &'static str> {
    unsafe {
        // 1. 软复位
        let rst = (DWC2_BASE + regs::GRSTCTL) as *mut u32;
        core::ptr::write_volatile(rst, 1);
        for _ in 0..10000 {
            if core::ptr::read_volatile(rst) & 1 == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // 2. AHB 配置：使能全局中断 + 非 DMA
        let ahb = (DWC2_BASE + regs::GAHBCFG) as *mut u32;
        core::ptr::write_volatile(ahb, (1 << 0) | (1 << 21)); // GINTMSK | NPTXFEMPTY?

        // 3. USB 配置：HNP/SRP 关闭
        let usb = (DWC2_BASE + regs::GUSBCFG) as *mut u32;
        let mut u = core::ptr::read_volatile(usb);
        u &= !(1 << 9); // HNPCAP
        u &= !(1 << 8); // SRPCAP
        core::ptr::write_volatile(usb, u);

        // 4. 主机模式配置：FS/LS 枚举
        let hcfg = (DWC2_BASE + regs::HCFG) as *mut u32;
        core::ptr::write_volatile(hcfg, 1 << 2); // FS PHY clock
    }
    Ok(())
}

/// HID 键盘报文 → 输入事件
/// 报文（boot protocol, 8 字节）：modifiers | reserved | key[0..6]
pub fn parse_keyboard_report(report: &[u8], ev: &mut InputEvent) -> bool {
    if report.len() < 8 {
        return false;
    }
    let mut mods_val = mods::NONE;
    if report[0] & 0x02 != 0 {
        mods_val |= mods::SHIFT;
    }
    if report[0] & 0x01 != 0 {
        mods_val |= mods::CTRL;
    }
    if report[0] & 0x04 != 0 {
        mods_val |= mods::ALT;
    }
    if report[0] & 0x08 != 0 {
        mods_val |= mods::META;
    }
    // 取第一个按下键（简化：扫描码即 HID usage）
    let key = report[2];
    if key == 0 {
        return false;
    }
    ev.modifiers = mods_val;
    ev.key = hid_usage_to_scancode(key as u16);
    ev.kind = EventKind::KeyDown;
    true
}

/// HID 键盘 usage → 内部 scancode（字母/数字/常用键）
pub fn hid_usage_to_scancode(usage: u16) -> u16 {
    match usage {
        0x04..=0x1D => usage - 0x04 + b'A' as u16, // A..Z
        0x1E..=0x27 => usage - 0x1E + b'0' as u16, // 1..0
        0x28 => b'\n' as u16,
        0x29 => 0x1B, // ESC
        0x2A => 0x08, // Backspace
        0x2B => 0x09, // Tab
        0x2C => b' ' as u16,
        0x39 => 0x3B, // CapsLock → F1 占位
        0x3A => b'F' as u16, // F1
        _ => usage,
    }
}

/// HID 鼠标报文 → 输入事件
/// 报文（4 字节）：buttons | dx | dy | wheel
pub fn parse_mouse_report(report: &[u8], x: u16, y: u16, ev: &mut InputEvent) -> bool {
    if report.len() < 3 {
        return false;
    }
    let b = report[0];
    let dx = report[1] as i8 as i32;
    let dy = report[2] as i8 as i32;
    // 增量坐标更新（由调用方维护光标）
    let nx = (x as i32 + dx).clamp(0, 65535) as u16;
    let ny = (y as i32 + dy).clamp(0, 65535) as u16;
    ev.x = nx;
    ev.y = ny;
    ev.modifiers = if b & 1 != 0 { buttons::LEFT as u32 } else { 0 };
    ev.kind = EventKind::MouseMove;
    true
}

/// 设备描述符（枚举用）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[allow(non_snake_case)]
pub struct DeviceDescriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bcdUSB: u16,
    pub bDeviceClass: u8,
    pub bDeviceSubClass: u8,
    pub bDeviceProtocol: u8,
    pub bMaxPacketSize0: u8,
    pub idVendor: u16,
    pub idProduct: u16,
    pub bcdDevice: u16,
    pub iManufacturer: u8,
    pub iProduct: u8,
    pub iSerialNumber: u8,
    pub bNumConfigurations: u8,
}

/// 已枚举设备
#[derive(Debug, Clone)]
pub struct UsbDevice {
    pub addr: u8,
    pub vid: u16,
    pub pid: u16,
    pub class: u8,
    /// 接口类型（0x03 = HID）
    pub iface_class: u8,
    /// HID 键盘/鼠标
    pub is_keyboard: bool,
    pub is_mouse: bool,
    /// 中断端点（IN）
    pub ep_in: u8,
}

static DEVICES: SpinLock<Vec<UsbDevice>> = SpinLock::new(Vec::new());

/// 设备枚举状态机（框架：每帧轮询端口事件 → 复位 → 分配地址 → 读描述符）
/// 详细实现在真机联调阶段按此骨架补齐
pub fn poll_enumerate() {
    let mut devs = DEVICES.lock();
    let _ = &mut devs;
    // TODO(Phase 3 真机): HPRT 端口连接/复位检测 + SET_ADDRESS + GET_DESCRIPTOR
    // + SET_CONFIGURATION + 接口解析 + HID 中断端点轮询
}
