//! 系统调用接口（SVC #0）。
//!
//! 调用约定（用户态 mini libc 封装）：
//! ```text
//! r7 = 系统调用号
//! r0-r3 = 参数 0..3
//! 返回值在 r0；负数 = 错误码
//! ```
//!
//! 系统调用表：
//! | 号 | 名称              | 说明 |
//! |----|-------------------|------|
//! | 0  | exit             | 退出当前进程 |
//! | 1  | write            | fd=1 写串口 |
//! | 2  | read             | fd=0 读串口 |
//! | 3  | time             | 单调 tick 数 |
//! | 4  | fb_info          | 获取 framebuffer 尺寸/地址 |
//! | 5  | fb_flush         | 刷新脏矩形到显存 |
//! | 6  | sbrk             | 扩展堆（简化） |
//! | 7  | events_poll      | 读取输入事件（返回事件数） |
//! | 8  | window_create    | 创建窗口（Phase 4） |
//! | 9  | window_update    | 更新窗口内容 |
//! | 10 | kir_load         | 加载 .kir 应用（Phase 5） |

use crate::exception::SavedRegs;
use crate::scheduler;

/// 系统调用号
pub mod nr {
    pub const EXIT: u32 = 0;
    pub const WRITE: u32 = 1;
    pub const READ: u32 = 2;
    pub const TIME: u32 = 3;
    pub const FB_INFO: u32 = 4;
    pub const FB_FLUSH: u32 = 5;
    pub const SBRK: u32 = 6;
    pub const EVENTS_POLL: u32 = 7;
    pub const WINDOW_CREATE: u32 = 8;
    pub const WINDOW_UPDATE: u32 = 9;
    pub const KIR_LOAD: u32 = 10;
}

/// 错误码
pub mod err {
    pub const OK: u32 = 0;
    pub const EINVAL: u32 = -22i32 as u32;
    pub const ENOENT: u32 = -2i32 as u32;
    pub const ENOMEM: u32 = -12i32 as u32;
    pub const ENOSYS: u32 = -38i32 as u32;
}

/// 处理系统调用（修改 ctx.r0 为返回值）
pub fn handle(ctx: &mut SavedRegs) {
    let nr = ctx.r7;
    let ret = dispatch(nr, ctx.r0, ctx.r1, ctx.r2, ctx.r3);
    ctx.set_ret(ret);
}

fn dispatch(nr: u32, a0: u32, a1: u32, a2: u32, _a3: u32) -> u32 {
    match nr {
        nr::EXIT => {
            scheduler::kill_current();
            err::OK
        }
        nr::WRITE => {
            // write(fd, buf, len)：fd=1 → 串口
            if a0 == 1 {
                let buf = a1 as *const u8;
                let len = a2 as usize;
                let console = crate::console::CONSOLE.lock();
                let bytes = unsafe { core::slice::from_raw_parts(buf, len) };
                // 逐字节写入（避免借用问题）
                for &b in bytes {
                    if b == b'\n' {
                        console.putc(b'\r');
                    }
                    console.putc(b);
                }
                len as u32
            } else {
                err::EINVAL
            }
        }
        nr::READ => {
            // read(fd, buf, len)：fd=0 → 串口非阻塞读
            if a0 == 0 {
                let buf = a1 as *mut u8;
                let len = a2 as usize;
                let console = crate::console::CONSOLE.lock();
                let mut n = 0usize;
                while n < len {
                    match console.getc_nowait() {
                        Some(b) => unsafe {
                            *buf.add(n) = b;
                            n += 1;
                        },
                        None => break,
                    }
                }
                n as u32
            } else {
                err::EINVAL
            }
        }
        nr::TIME => crate::timer::ticks() as u32,
        nr::FB_INFO => {
            // fb_info(*info)：返回 framebuffer 信息结构（见 framebuffer.rs）
            let info = crate::framebuffer::fb_info();
            if a0 != 0 {
                unsafe {
                    let p = a0 as *mut crate::framebuffer::FbInfo;
                    core::ptr::write_volatile(p, info);
                }
                err::OK
            } else {
                err::EINVAL
            }
        }
        nr::FB_FLUSH => {
            // fb_flush(x, y, w, h)：刷新脏矩形
            crate::framebuffer::flush_rect(a0 as i32, a1 as i32, a2 as i32, _a3 as i32);
            err::OK
        }
        nr::SBRK => {
            // sbrk(size)：简化——从内核堆分配（仅调试用，不做进程地址空间）
            let size = a0 as usize;
            match crate::memory::alloc_pages(kirax_core::align_up(size, 4096).trailing_zeros() as u32 - 12) {
                Some(addr) => addr as u32,
                None => err::ENOMEM,
            }
        }
        nr::EVENTS_POLL => {
            // events_poll(buf, max)：从输入环形缓冲读取事件
            crate::window::poll_events(a0 as *mut kirax_core::events::InputEvent, a1 as usize) as u32
        }
        nr::KIR_LOAD => {
            // kir_load(path)：加载并启动 .kir 应用（Phase 5 实现）
            crate::kir::load_app(a0 as *const u8)
        }
        _ => err::ENOSYS,
    }
}
