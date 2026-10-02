//! framebuffer：通过 VideoCore 邮筒分配显存，提供双缓冲与脏矩形刷新。
//!
//! 流程：
//! 1. 邮筒 tag `ALLOCATE_BUFFER` 向 GPU 申请显存（默认 1280×720×32bpp）；
//! 2. 获得物理地址后，内核把该区域作为“前缓冲”；
//! 3. 合成器在“后缓冲”（内核堆）绘制，`flush_rect` 把脏矩形拷到前缓冲；
//! 4. 树莓派的 GPU 持续扫描前缓冲输出到 HDMI/DSI。
//!
//! 注意：framebuffer 地址由 VideoCore 分配，位于其保留区（默认 gpu_mem=64MB，
//! 即 0x3C00_0000 以上），内核伙伴分配器已避开该区域。

use crate::mailbox;
use crate::klog;
use crate::sync::SpinLock;
use alloc::vec;
use alloc::vec::Vec;
use space_core::align_up;

/// 默认分辨率（树莓派 2 性能友好的中间档）
pub const DEFAULT_W: u32 = 1280;
pub const DEFAULT_H: u32 = 720;
pub const DEFAULT_BPP: u32 = 32;

/// framebuffer 信息（用户态通过 sys_fb_info 获取）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FbInfo {
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub addr: u32,
    pub bpp: u32,
}

/// 全局 framebuffer 状态
struct FbState {
    info: FbInfo,
    /// 后缓冲（合成器绘制目标）
    back: Vec<u8>,
    initialized: bool,
}

static FB: SpinLock<FbState> = SpinLock::new(FbState {
    info: FbInfo {
        width: 0,
        height: 0,
        pitch: 0,
        addr: 0,
        bpp: 0,
    },
    back: Vec::new(),
    initialized: false,
});

/// 初始化 framebuffer（邮筒申请显存）
pub fn init(w: u32, h: u32) -> Result<(), &'static str> {
    let mut fb = FB.lock();
    if fb.initialized {
        return Ok(());
    }

    // 1. 分配显存（对齐 256 字节，大小 = 帧缓冲 + 1MB 余量）
    let req = w * h * (DEFAULT_BPP / 8) + 1024 * 1024;
    let v = mailbox::call_tag(mailbox::tag::ALLOCATE_BUFFER, &[256, req], 8)?;
    let (addr, size) = (v[0], v[1]);
    if addr == 0 {
        return Err("GPU 拒绝分配 framebuffer");
    }

    // 2. 设置物理/虚拟分辨率与位深
    let _ = mailbox::call_tag(mailbox::tag::SET_PHYSICAL_W_H, &[w, h], 8)?;
    let _ = mailbox::call_tag(mailbox::tag::SET_VIRTUAL_W_H, &[w, h], 8)?;
    let _ = mailbox::call_tag(mailbox::tag::SET_DEPTH, &[DEFAULT_BPP], 4)?;

    // 3. 读取实际 pitch
    let pitch_v = mailbox::call_tag(mailbox::tag::GET_PITCH, &[], 4)?;
    let pitch = pitch_v[0];

    let _ = size;
    fb.info = FbInfo {
        width: w,
        height: h,
        pitch,
        addr,
        bpp: DEFAULT_BPP,
    };
    fb.back = vec![0u8; (pitch * h) as usize];
    fb.initialized = true;

    klog!("[fb] framebuffer {w}x{h} bpp={DEFAULT_BPP} addr={:#010x} pitch={pitch}", addr);
    Ok(())
}

/// 获取 framebuffer 信息（系统调用）
pub fn fb_info() -> FbInfo {
    FB.lock().info
}

/// 后缓冲可变访问（合成器）
pub fn back_buffer_mut() -> &'static mut [u8] {
    let fb = FB.lock();
    unsafe {
        let ptr = fb.back.as_ptr() as *mut u8;
        core::slice::from_raw_parts_mut(ptr, fb.back.len())
    }
}

/// 后缓冲只读访问
pub fn back_buffer() -> &'static [u8] {
    let fb = FB.lock();
    unsafe { core::slice::from_raw_parts(fb.back.as_ptr(), fb.back.len()) }
}

/// 脏矩形刷新：把后缓冲的 (x,y,w,h) 区域拷贝到显存
pub fn flush_rect(x: i32, y: i32, w: i32, h: i32) {
    let fb = FB.lock();
    if !fb.initialized {
        return;
    }
    let info = &fb.info;
    let x0 = x.max(0) as u32;
    let y0 = y.max(0) as u32;
    let x1 = (x + w).max(0) as u32;
    let y1 = (y + h).max(0) as u32;
    if x0 >= info.width || y0 >= info.height || x1 <= x0 || y1 <= y0 {
        return;
    }
    let x1 = x1.min(info.width);
    let y1 = y1.min(info.height);

    // 行拷贝（back: pitch 每行；fb: pitch 每行；bpp=32 → 4 字节/像素）
    let bpp = (info.bpp / 8) as u32;
    let row_bytes = ((x1 - x0) * bpp) as usize;
    for row in y0..y1 {
        let src_off = (row * info.pitch + x0 * bpp) as usize;
        let dst_off = (row * info.pitch + x0 * bpp) as usize;
        let dst = info.addr as usize + dst_off;
        let src = fb.back.as_ptr() as usize + src_off;
        unsafe {
            core::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, row_bytes);
        }
    }
}

/// 全屏刷新（启动/壁纸变化时）
pub fn flush_all() {
    let fb = FB.lock();
    if !fb.initialized {
        return;
    }
    let info = &fb.info;
    let total = (info.pitch * info.height) as usize;
    unsafe {
        core::ptr::copy_nonoverlapping(
            fb.back.as_ptr(),
            info.addr as *mut u8,
            total,
        );
    }
}

/// 需要的对齐（mailbox 要求 16 字节）
pub fn mailbox_aligned(size: usize) -> usize {
    align_up(size, 16)
}
