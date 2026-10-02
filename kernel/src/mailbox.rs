//! VideoCore 邮筒（Mailbox）接口：与 GPU 通信获取 framebuffer、内存信息等。
//!
//! 寄存器（0x3F00_B880）：
//! - 0x00 READ    读数据（低 4 位 = 通道）
//! - 0x04 POLL    非阻塞读
//! - 0x08 SENDER  发送方
//! - 0x0C STATUS  状态（bit31 满 / bit30 空）
//! - 0x20 WRITE   写数据（低 4 位 = 通道）
//!
//! 协议：属性通道（channel 8）。请求缓冲区需 16 字节对齐；
//! 缓冲区格式：u32 总大小 | u32 请求码 0 | tag 序列 | 0 结束。
//! 每个 tag：u32 tagID | u32 值大小 | u32 值个数 | 值 | 填充到 4 字节。

/// 外设基址
const PERIPHERAL_BASE: usize = 0x3F00_0000;
const MAILBOX_BASE: usize = PERIPHERAL_BASE + 0x00B880;

use alloc::vec::Vec;

const MAILBOX_READ: *mut u32 = (MAILBOX_BASE + 0x00) as *mut u32;
const MAILBOX_STATUS: *mut u32 = (MAILBOX_BASE + 0x0C) as *mut u32;
const MAILBOX_WRITE: *mut u32 = (MAILBOX_BASE + 0x20) as *mut u32;

/// 属性通道
const CHANNEL_PROP: u32 = 8;

const STATUS_FULL: u32 = 1 << 31;
const STATUS_EMPTY: u32 = 1 << 30;

/// 常用 tag
pub mod tag {
    /// 分配 framebuffer（值：对齐字节 | 请求的字节数，返回：帧缓冲地址 | 大小）
    pub const ALLOCATE_BUFFER: u32 = 0x0004_8001;
    /// 释放 framebuffer
    pub const RELEASE_BUFFER: u32 = 0x0004_8002;
    /// 物理显示宽高（返回：宽 | 高）
    pub const GET_PHYSICAL_W_H: u32 = 0x0004_0003;
    /// 设置物理显示宽高（值：宽 | 高）
    pub const SET_PHYSICAL_W_H: u32 = 0x0004_8003;
    /// 虚拟宽高（返回：宽 | 高）
    pub const GET_VIRTUAL_W_H: u32 = 0x0004_0004;
    /// 设置虚拟宽高
    pub const SET_VIRTUAL_W_H: u32 = 0x0004_8004;
    /// 位深（返回：bpp）
    pub const GET_DEPTH: u32 = 0x0004_0005;
    /// 设置位深
    pub const SET_DEPTH: u32 = 0x0004_8005;
    /// 行字节数（pitch）
    pub const GET_PITCH: u32 = 0x0004_0008;
    /// 获取 ARM 内存
    pub const GET_ARM_MEMORY: u32 = 0x0001_0005;
    /// 获取 VC 内存
    pub const GET_VC_MEMORY: u32 = 0x0001_0006;
    /// 获取板卡序列号
    pub const GET_BOARD_SERIAL: u32 = 0x0001_0004;
}

/// 发送属性请求；`buf` 必须 16 字节对齐，包含 tag 序列并以 0 结束。
/// 成功返回 buf 中第一个 tag 的响应；失败返回 None。
pub fn property_request(buf: &mut [u32]) -> Result<(), &'static str> {
    unsafe {
        // 写请求码
        buf[1] = 0;
        // 总大小（前两个 u32 之后的所有字节，含结束 0）
        let total = (buf.len() * 4) as u32;
        buf[0] = total;

        // 发送
        let msg = (buf.as_mut_ptr() as u32 & !0xF) | CHANNEL_PROP;
        while core::ptr::read_volatile(MAILBOX_STATUS) & STATUS_FULL != 0 {
            core::hint::spin_loop();
        }
        core::ptr::write_volatile(MAILBOX_WRITE, msg);

        // 等待响应
        loop {
            while core::ptr::read_volatile(MAILBOX_STATUS) & STATUS_EMPTY != 0 {
                core::hint::spin_loop();
            }
            let resp = core::ptr::read_volatile(MAILBOX_READ);
            if resp & 0xF == CHANNEL_PROP {
                break;
            }
        }

        // 检查响应状态（每个 tag 的字节 2：0 = 成功）
        if buf[2 + 2] == 0x8000_0000 {
            return Err("mailbox tag 请求失败");
        }
        Ok(())
    }
}

/// 执行单个 tag 请求：`[tag, value_size, num, value...]`
/// 返回 value 区（不含 tag 头）
pub fn call_tag(tag_id: u32, values: &[u32], in_size: usize) -> Result<Vec<u32>, &'static str> {
    // 构造：header(2) + tag(3+values) + 0 结束
    let mut buf: Vec<u32> = Vec::with_capacity(5 + values.len());
    buf.push(0); // 总大小（占位）
    buf.push(0); // 请求码
    buf.push(tag_id);
    buf.push(in_size as u32); // 值区大小（字节）
    buf.push(values.len() as u32); // 值个数
    buf.extend_from_slice(values);
    buf.push(0); // 结束标记
    // 16 字节对齐（Vec 分配天然对齐，这里确保长度）
    while buf.len() % 4 != 0 {
        buf.push(0);
    }

    property_request(&mut buf)?;

    // 返回 value 区
    let val_start = 5;
    let n = buf[val_start - 1] as usize; // 值个数
    Ok(buf[val_start..val_start + n].to_vec())
}

/// 获取板卡序列号（自检/标识用）
pub fn board_serial() -> u64 {
    let v = call_tag(tag::GET_BOARD_SERIAL, &[], 8).unwrap_or_default();
    if v.len() >= 2 {
        (v[1] as u64) << 32 | v[0] as u64
    } else {
        0
    }
}
