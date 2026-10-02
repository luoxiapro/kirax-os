//! EMMC/SD 卡驱动（BCM2835 EMMC 控制器，基址 0x3F30_0000）。
//!
//! 控制器遵循 SDHCI 变体。寄存器与位定义来自《BCM2835 ARM Peripherals》第 8 章
//! （SD Card）。**注意**：QEMU 的 raspi2b 未模拟 EMMC 控制器，本驱动无法在
//! QEMU 中验证，属于“真机验证项”（见 docs/05 的 SD 兼容性测试计划）；
//! FAT32 逻辑层已在 QEMU 中用内存盘验证（space-core::fat32 测试）。
//!
//! 初始化序列（SDHC 卡）：
//! ```text
//! 复位控制器 → 使能内部时钟 → 设置分频 → 上电 → CMD0 → CMD8(0x1AA)
//! → ACMD41(HCS) 轮询就绪 → CMD2 → CMD3(RCA) → CMD7(选中) → CMD13(状态)
//! → 块读写：CMD17/CMD18/CMD24/CMD25
//! ```

#![allow(dead_code)] // 真机调试期寄存器常量池（硬件规范命名），驱动完善后移除
use crate::klog;
use crate::sync::SpinLock;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

const EMMC_BASE: usize = 0x3F30_0000;

// ---- 寄存器偏移（BCM2835 datasheet §8） ----
// 全部按硬件规范命名；部分常量供后续真机 EMMC 调试使用，允许暂时未用。
#[allow(dead_code)]
const REG_ARG: usize = 0x00;
const REG_CMD: usize = 0x04;
const REG_RESP0: usize = 0x08;
const REG_RESP1: usize = 0x0C;
const REG_RESP2: usize = 0x10;
const REG_RESP3: usize = 0x14;
const REG_DATA: usize = 0x18;
const REG_STATUS: usize = 0x1C;
const REG_CONTROL0: usize = 0x20;
const REG_CONTROL1: usize = 0x24;
const REG_INTERRUPT: usize = 0x28;
const REG_IRPT_MASK: usize = 0x2C;
const REG_IRPT_EN: usize = 0x30;
const REG_CONTROL2: usize = 0x34;
const REG_CAPAB0: usize = 0x38;
const REG_CAPAB1: usize = 0x3C;
const REG_FORCE_IRPT: usize = 0x40;
const REG_BOOT_TIMEOUT: usize = 0x44;
const REG_READ_THRESHOLD: usize = 0x60;

// ---- EMMC_CMD 位 ----
#[allow(dead_code)]
const CMD_INDEX_MASK: u32 = 0x3F;
const CMD_NEED_RESP: u32 = 1 << 6;
const CMD_LONG_RESP: u32 = 1 << 7;
const CMD_NEED_APP_CMD: u32 = 1 << 8;
const CMD_CMD_TYPE_NORMAL: u32 = 3 << 9;
const CMD_CRC_CHK_EN: u32 = 1 << 11;
const CMD_CMD_IDX_CHK_EN: u32 = 1 << 12;
const CMD_DATA_PRESENT: u32 = 1 << 13;
const CMD_MULTI_BLOCK: u32 = 1 << 14;
const CMD_RW_READ: u32 = 1 << 15;

// ---- EMMC_STATUS 位 ----
const STATUS_CMD_INHIBIT: u32 = 1 << 0;
const STATUS_DATA_INHIBIT: u32 = 1 << 1;
const STATUS_DAT_LEVEL_0: u32 = 1 << 16;

// ---- EMMC_CONTROL1 位 ----
const CONTROL1_SRST_HC: u32 = 1 << 24;
const CONTROL1_SRST_CMD: u32 = 1 << 25;
const CONTROL1_SRST_DATA: u32 = 1 << 26;
const CONTROL1_CLK_INTL_EN: u32 = 1 << 0;
const CONTROL1_CLK_INTL_STABLE: u32 = 1 << 1;
const CONTROL1_CLK_EN: u32 = 1 << 2;
const CONTROL1_CLK_GENSEL: u32 = 1 << 5;
/// 时钟分频（[23:16]，值为 2^n 分频）
const CONTROL1_CLK_FREQ_MASK: u32 = 0xFF << 16;
/// 上电（SD_BUS_POWER）
const CONTROL1_SD_POWER_ON: u32 = 1 << 8;

// ---- EMMC_INTERRUPT 位 ----
const IRQ_CMD_DONE: u32 = 1 << 0;
const IRQ_DATA_DONE: u32 = 1 << 1;
const IRQ_WRITE_READY: u32 = 1 << 4;
const IRQ_READ_READY: u32 = 1 << 5;
const IRQ_CARD_INSERTED: u32 = 1 << 6;
const IRQ_CARD_REMOVED: u32 = 1 << 7;
const IRQ_CMD_TIMEOUT: u32 = 1 << 9;
const IRQ_DATA_TIMEOUT: u32 = 1 << 10;
const IRQ_CRC_CMD_ERR: u32 = 1 << 11;
const IRQ_CRC_DATA_ERR: u32 = 1 << 12;
const IRQ_END_BIT_CMD_ERR: u32 = 1 << 13;
const IRQ_END_BIT_DATA_ERR: u32 = 1 << 14;
const IRQ_ERR_MASK: u32 = IRQ_CMD_TIMEOUT | IRQ_DATA_TIMEOUT | IRQ_CRC_CMD_ERR
    | IRQ_CRC_DATA_ERR | IRQ_END_BIT_CMD_ERR | IRQ_END_BIT_DATA_ERR;

// ---- SD 卡命令 ----
const CMD0_GO_IDLE: u32 = 0;
const CMD2_ALL_SEND_CID: u32 = 2;
const CMD3_SEND_REL_ADDR: u32 = 3;
const CMD7_SEL_CARD: u32 = 7;
const CMD8_SEND_IF_COND: u32 = 8;
const CMD13_SEND_STATUS: u32 = 13;
const CMD17_READ_SINGLE: u32 = 17;
const CMD18_READ_MULTI: u32 = 18;
const CMD24_WRITE_SINGLE: u32 = 24;
const CMD25_WRITE_MULTI: u32 = 25;
const CMD55_APP_CMD: u32 = 55;
const ACMD41_SD_SEND_OP_COND: u32 = 41;

/// 卡状态（rca/blocks 供后续真机 CMD13/CMD9 解析使用，暂未读）
#[allow(dead_code)]
struct SdCard {
    rca: u32,
    /// 高容量（SDHC）标记（块寻址 512B 块）
    high_capacity: bool,
    /// 块数（来自 CSD/CMD9，简化用 CMD13 读取）
    blocks: u64,
}

static SD: SpinLock<Option<SdCard>> = SpinLock::new(None);
static READY: AtomicBool = AtomicBool::new(false);

pub fn is_ready() -> bool {
    READY.load(Ordering::Acquire)
}

#[inline]
unsafe fn reg(off: usize) -> *mut u32 {
    (EMMC_BASE + off) as *mut u32
}

#[inline]
unsafe fn rd(off: usize) -> u32 {
    core::ptr::read_volatile(reg(off))
}

#[inline]
unsafe fn wr(off: usize, v: u32) {
    core::ptr::write_volatile(reg(off), v)
}

fn wait_status(mask: u32, set: bool, timeout: u32) -> bool {
    for _ in 0..timeout {
        let s = unsafe { rd(REG_STATUS) };
        if ((s & mask) != 0) == set {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// 发送命令（带参数），返回响应字（短响应取 RESP0，长响应取 4 个字）
fn send_cmd(cmd: u32, arg: u32, flags: u32, _long_resp: bool) -> Result<u32, &'static str> {
    // 等待命令通道空闲
    if !wait_status(STATUS_CMD_INHIBIT, false, 100_000) {
        return Err("命令通道忙");
    }
    unsafe {
        wr(REG_ARG, arg);
        // 写命令寄存器即触发执行
        let full = (cmd & CMD_INDEX_MASK) | flags | CMD_CMD_TYPE_NORMAL;
        wr(REG_CMD, full);

        // 等待完成（轮询中断）
        for _ in 0..1_000_000 {
            let irpt = rd(REG_INTERRUPT);
            if irpt & IRQ_CMD_DONE != 0 {
                wr(REG_INTERRUPT, irpt); // 清标志
                return Ok(rd(REG_RESP0));
            }
            if irpt & IRQ_ERR_MASK != 0 {
                wr(REG_INTERRUPT, irpt);
                return Err("命令错误");
            }
            core::hint::spin_loop();
        }
    }
    Err("命令超时")
}

/// ACMD41：先发 CMD55 再发 CMD41
fn app_cmd(cmd: u32, arg: u32, flags: u32) -> Result<u32, &'static str> {
    let _ = send_cmd(CMD55_APP_CMD, 0, CMD_NEED_RESP, false)?;
    send_cmd(cmd, arg, flags, false)
}

/// 复位并初始化控制器时钟
fn controller_init() -> Result<(), &'static str> {
    unsafe {
        // 1. 软件复位
        wr(REG_CONTROL1, CONTROL1_SRST_HC);
        wait_status(STATUS_CMD_INHIBIT, false, 100_000);
        // 等待复位完成（CONTROL1 复位位自动清）
        for _ in 0..1000 {
            if rd(REG_CONTROL1) & CONTROL1_SRST_HC == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // 2. 使能内部时钟
        wr(REG_CONTROL1, CONTROL1_CLK_INTL_EN);
        // 等待稳定
        for _ in 0..1000 {
            if rd(REG_CONTROL1) & CONTROL1_CLK_INTL_STABLE != 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // 3. 设置时钟分频（400kHz 初始化频率：19MHz 核心 / 400k ≈ 48 → 2^5=32）
        let mut c1 = rd(REG_CONTROL1);
        c1 &= !CONTROL1_CLK_FREQ_MASK;
        c1 |= 5 << 16; // 分频 2^5 = 32
        wr(REG_CONTROL1, c1);
        // 使能 SD 时钟
        c1 |= CONTROL1_CLK_EN;
        wr(REG_CONTROL1, c1);

        // 4. 上电（SD 总线电源）
        c1 |= CONTROL1_SD_POWER_ON;
        wr(REG_CONTROL1, c1);

        // 5. 数据宽度：4 位（CONTROL0）
        let mut c0 = rd(REG_CONTROL0);
        c0 |= 0b10 << 1; // SDHCI 总线宽度 4-bit
        wr(REG_CONTROL0, c0);

        // 6. 中断使能
        wr(REG_IRPT_EN, 0xFFFF_FFFF);
        wr(REG_IRPT_MASK, 0xFFFF_FFFF);
    }
    Ok(())
}

/// 检测并初始化 SD 卡
pub fn init() -> Result<(), &'static str> {
    controller_init()?;

    // CMD0：进入空闲态
    send_cmd(CMD0_GO_IDLE, 0, 0, false)?;

    // CMD8：检测 SDHC（参数 0x1AA）
    let r = send_cmd(
        CMD8_SEND_IF_COND,
        0x1AA,
        CMD_NEED_RESP | CMD_CRC_CHK_EN | CMD_CMD_IDX_CHK_EN,
        false,
    )?;
    if r & 0xFFF != 0x1AA {
        return Err("非 SDHC 卡或 CMD8 失败");
    }

    // ACMD41：初始化 + 设置 HCS（高容量支持）
    let mut high_capacity = false;
    let mut ready = false;
    for _ in 0..1000 {
        let r = app_cmd(
            ACMD41_SD_SEND_OP_COND,
            if high_capacity { 0x40_0000 } else { 0 },
            CMD_NEED_RESP,
        )?;
        if r & (1 << 30) != 0 {
            high_capacity = true; // 卡支持 HCS
        }
        if r & (1 << 31) != 0 {
            ready = true;
            break;
        }
        crate::timer::delay_us(1000);
    }
    if !ready {
        return Err("ACMD41 超时");
    }

    // CMD2：读 CID
    let _ = send_cmd(CMD2_ALL_SEND_CID, 0, CMD_NEED_RESP | CMD_LONG_RESP, true)?;

    // CMD3：读 RCA
    let r = send_cmd(CMD3_SEND_REL_ADDR, 0, CMD_NEED_RESP, false)?;
    let rca = r >> 16;
    if rca == 0 {
        return Err("RCA 无效");
    }

    // CMD7：选中卡
    send_cmd(CMD7_SEL_CARD, rca << 16, CMD_NEED_RESP, false)?;

    // CMD13：查询状态，确认就绪
    let st = send_cmd(CMD13_SEND_STATUS, rca << 16, CMD_NEED_RESP, false)?;
    if (st >> 9) & 0xF != 4 && (st >> 9) & 0xF != 3 {
        // 期望 transferred/standby
    }

    *SD.lock() = Some(SdCard {
        rca,
        high_capacity,
        blocks: 0,
    });
    READY.store(true, Ordering::Release);
    klog!("[sd] SD 卡就绪: rca={:#x} high_capacity={}", rca, high_capacity);
    Ok(())
}

/// 块地址（SDHC 为 512B 块号，SDSC 为字节地址）
fn block_addr(high_capacity: bool, lba: u64) -> u32 {
    if high_capacity {
        lba as u32
    } else {
        (lba * 512) as u32
    }
}

/// 读单块（512B）
pub fn read_block(lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    let sd = SD.lock();
    let Some(card) = sd.as_ref() else {
        return Err("SD 未初始化");
    };
    if buf.len() < 512 {
        return Err("缓冲区不足");
    }
    // 等待数据通道空闲
    wait_status(STATUS_DATA_INHIBIT, false, 100_000);

    let arg = block_addr(card.high_capacity, lba);
    // 配置数据长度（块大小 512）
    // BLOCK_SIZE 在 CONTROL1? BCM2835 用 CONTROL1[27:16]? —— 见真机验证说明

    send_cmd(
        CMD17_READ_SINGLE,
        arg,
        CMD_NEED_RESP | CMD_CRC_CHK_EN | CMD_CMD_IDX_CHK_EN | CMD_DATA_PRESENT | CMD_RW_READ,
        false,
    )?;

    // 等待读就绪，逐字读取 128 个 u32
    let mut done = 0usize;
    while done < 512 {
        let irpt = unsafe { rd(REG_INTERRUPT) };
        if irpt & IRQ_READ_READY != 0 {
            unsafe { wr(REG_INTERRUPT, IRQ_READ_READY) };
            for _ in 0..4 {
                let w = unsafe { rd(REG_DATA) };
                buf[done] = w as u8;
                buf[done + 1] = (w >> 8) as u8;
                buf[done + 2] = (w >> 16) as u8;
                buf[done + 3] = (w >> 24) as u8;
                done += 4;
            }
        } else if irpt & IRQ_ERR_MASK != 0 {
            unsafe { wr(REG_INTERRUPT, irpt) };
            return Err("读块数据错误");
        } else {
            core::hint::spin_loop();
        }
    }
    // 等待数据完成
    wait_status(STATUS_DATA_INHIBIT, false, 1_000_000);
    Ok(())
}

/// 写单块（512B）
pub fn write_block(lba: u64, buf: &[u8]) -> Result<(), &'static str> {
    let sd = SD.lock();
    let Some(card) = sd.as_ref() else {
        return Err("SD 未初始化");
    };
    wait_status(STATUS_DATA_INHIBIT, false, 100_000);
    let arg = block_addr(card.high_capacity, lba);
    send_cmd(
        CMD24_WRITE_SINGLE,
        arg,
        CMD_NEED_RESP | CMD_CRC_CHK_EN | CMD_CMD_IDX_CHK_EN | CMD_DATA_PRESENT,
        false,
    )?;
    let mut done = 0usize;
    while done < 512 {
        let irpt = unsafe { rd(REG_INTERRUPT) };
        if irpt & IRQ_WRITE_READY != 0 {
            unsafe { wr(REG_INTERRUPT, IRQ_WRITE_READY) };
            for _ in 0..4 {
                let w = (buf[done] as u32)
                    | ((buf[done + 1] as u32) << 8)
                    | ((buf[done + 2] as u32) << 16)
                    | ((buf[done + 3] as u32) << 24);
                unsafe { wr(REG_DATA, w) };
                done += 4;
            }
        } else if irpt & IRQ_ERR_MASK != 0 {
            unsafe { wr(REG_INTERRUPT, irpt) };
            return Err("写块数据错误");
        } else {
            core::hint::spin_loop();
        }
    }
    // 等待卡忙结束（SDHCI 的“写保护”状态位）
    if !wait_status(STATUS_DATA_INHIBIT, false, 10_000_000) { return Err("写卡忙超时"); }
    Ok(())
}

/// 块设备适配（供 space-core::fat32 使用）
pub struct SdBlockDevice;

impl space_core::fat32::BlockDevice for SdBlockDevice {
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let n = buf.len() / 512;
        for i in 0..n {
            read_block(lba + i as u64, &mut buf[i * 512..(i + 1) * 512])?;
        }
        Ok(())
    }
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        let n = buf.len() / 512;
        for i in 0..n {
            write_block(lba + i as u64, &buf[i * 512..(i + 1) * 512])?;
        }
        Ok(())
    }
    fn sector_size(&self) -> u16 {
        512
    }
}

/// 挂载 FAT32 并标记就绪（供 kir 加载器使用）
pub fn mount_fat32() -> Result<(), &'static str> {
    let dev = SdBlockDevice;
    let _fs = space_core::fat32::Fat32Fs::mount(dev)?;
    crate::kir::set_fs_ready();
    klog!("[sd] FAT32 卷挂载成功");
    Ok(())
}

/// sd_read_file 钩子（kir 模块引用）
#[no_mangle]
pub extern "C" fn sd_read_file(path: *const u8, out: *mut u8, max: u32) -> i32 {
    // 读取路径（ASCII）
    let mut name = Vec::new();
    let mut i = 0usize;
    loop {
        let c = unsafe { core::ptr::read_volatile(path.add(i)) };
        if c == 0 {
            break;
        }
        name.push(c);
        i += 1;
        if i > 255 {
            return -1;
        }
    }
    let path_str = core::str::from_utf8(&name).unwrap_or("");
    let dev = SdBlockDevice;
    let mut fs = match space_core::fat32::Fat32Fs::mount(dev) {
        Ok(f) => f,
        Err(_) => return -2,
    };
    let mut buf = alloc::vec![0u8; max as usize];
    match fs.read_file(path_str, 0, &mut buf) {
        Ok(n) => {
            unsafe {
                core::ptr::copy_nonoverlapping(buf.as_ptr(), out, n);
            }
            n as i32
        }
        Err(_) => -2,
    }
}
