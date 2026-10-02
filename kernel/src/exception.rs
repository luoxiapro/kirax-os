//! 异常分发（异常向量表在 boot/start.S，汇编入口保存现场后调用本模块）。
//!
//! 现场布局（与 start.S 的 EXC_ENTRY 宏严格对应）：
//! ```text
//! ctx + 0   .. +48    r0..r12（13 × u32）
//! ctx + 52           lr_exc（返回地址，已按异常类型修正）
//! ctx + 56           spsr_exc（被中断上下文的 CPSR）
//! ```

use crate::scheduler;
use crate::klog;
use crate::syscall;

/// 异常类型编号（与 start.S 的 .equ 一致）
pub const EXC_UNDEF: u32 = 0;
pub const EXC_SVC: u32 = 1;
pub const EXC_PREFETCH: u32 = 2;
pub const EXC_DATA: u32 = 3;
pub const EXC_IRQ: u32 = 4;
pub const EXC_FIQ: u32 = 5;

/// 保存的寄存器现场
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SavedRegs {
    pub r0: u32,
    pub r1: u32,
    pub r2: u32,
    pub r3: u32,
    pub r4: u32,
    pub r5: u32,
    pub r6: u32,
    pub r7: u32,
    pub r8: u32,
    pub r9: u32,
    pub r10: u32,
    pub r11: u32,
    pub r12: u32,
    pub lr_exc: u32,
    pub spsr_exc: u32,
}

impl SavedRegs {
    /// 是否来自用户态（CPSR 模式位 == 0x10 User）
    pub fn from_user(&self) -> bool {
        self.spsr_exc & 0x1F == 0x10
    }
    /// 返回地址
    pub fn pc(&self) -> u32 {
        self.lr_exc
    }
    /// 设置返回值（系统调用结果写 r0）
    pub fn set_ret(&mut self, v: u32) {
        self.r0 = v;
    }
}

/// 异常分发（汇编调用，返回可能被调度器替换的现场指针）
///
/// # Safety
/// - `ctx` 必须指向当前 SVC 栈上由 EXC_ENTRY 保存的现场块
#[no_mangle]
pub extern "C" fn exception_dispatch(kind: u32, ctx: *mut SavedRegs) -> *mut SavedRegs {
    let ctx_ref = unsafe { &mut *ctx };
    match kind {
        EXC_SVC => {
            // 系统调用：r7 = 编号，r0-r5 = 参数
            syscall::handle(ctx_ref);
            ctx
        }
        EXC_IRQ => {
            crate::irq::handle_current_irq();
            // 节拍后可能切换任务（返回新的现场指针）
            scheduler::maybe_reschedule(ctx)
        }
        EXC_UNDEF => {
            if ctx_ref.from_user() {
                klog!("[exc] 用户进程未定义指令 @ pc={:#010x}，终止", ctx_ref.pc());
                scheduler::kill_current();
                ctx
            } else {
                panic!("未定义指令（内核态） pc={:#010x}", ctx_ref.pc());
            }
        }
        EXC_PREFETCH | EXC_DATA => {
            // 读故障地址寄存器
            let far: u32;
            unsafe {
                core::arch::asm!("mrc p15, 0, {0}, c6, c0, 0", out(reg) far); // DFAR/IFAR
            }
            if ctx_ref.from_user() {
                klog!("[exc] 用户进程内存访问故障 @ {:#010x} pc={:#010x}，终止", far, ctx_ref.pc());
                scheduler::kill_current();
                ctx
            } else {
                panic!("内存访问故障（内核态） far={:#010x} pc={:#010x}", far, ctx_ref.pc());
            }
        }
        EXC_FIQ => {
            panic!("FIQ 未实现");
        }
        _ => {
            panic!("未知异常 kind={}", kind);
        }
    }
}
