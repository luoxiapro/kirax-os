//! 进程/线程调度器：固定容量任务表 + 轮转调度（RR）+ 抢占。
//!
//! 设计要点：
//! - 内核线程模型：每个任务拥有独立内核栈（伙伴分配器分配 16KB）；
//! - 抢占：系统定时器 100Hz 节拍 → `on_tick` 递减时间片 → `maybe_reschedule` 切换；
//! - 切换机制：异常入口把现场保存在**当前任务自己的内核栈**上，
//!   `maybe_reschedule` 返回下一个任务的现场指针，汇编 `mov sp, r0` 完成栈切换；
//! - 空闲任务：主核的引导栈上下文（无 TCB），任务队列为空时返回空闲现场。
//!
//! 多核说明：本版本调度器为单队列（核 0 调度），从核进入空闲循环；
//! 每核独立调度队列是后续优化项（文档见 docs/05_测试与长稳.md）。

#![allow(static_mut_refs)] // RUN_QUEUE 为 static mut，访问均在 RUN_HEAD/RUN_TAIL 自旋锁临界区内（调度器既定设计）
use crate::exception::SavedRegs;
use crate::klog;
use crate::memory;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicU32, Ordering};

/// 任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TaskState {
    Ready = 0,
    Running = 1,
    Blocked = 2,
    Dead = 3,
}

/// 任务控制块
#[derive(Debug, Clone, Copy)]
pub struct Task {
    pub id: u32,
    pub name: &'static str,
    pub state: TaskState,
    /// 现场指针（位于任务内核栈顶附近）
    pub ctx: *mut SavedRegs,
    /// 内核栈基址（用于回收）
    pub stack_base: usize,
    /// 剩余时间片（单位：tick）
    pub quantum_left: u32,
}

/// 最大任务数（固定容量，避免堆依赖）
pub const MAX_TASKS: usize = 64;
/// 默认时间片（tick 数，100Hz 下 = 20ms）
pub const DEFAULT_QUANTUM: u32 = 2;
/// 任务内核栈大小
const TASK_STACK_SIZE: usize = 16 * 1024;

/// 任务表
static mut TASKS: [Option<Task>; MAX_TASKS] = [None; MAX_TASKS];

/// 就绪队列（固定容量环形，存任务 id）
static mut RUN_QUEUE: [u32; MAX_TASKS] = [0; MAX_TASKS];
static RUN_HEAD: SpinLock<usize> = SpinLock::new(0);
static RUN_TAIL: SpinLock<usize> = SpinLock::new(0);

/// 当前运行任务（usize::MAX = 空闲）
static CURRENT: AtomicU32 = AtomicU32::new(u32::MAX);
/// 下一个任务 id 分配器
static NEXT_ID: AtomicU32 = AtomicU32::new(0);

/// 空闲任务现场（第一次切换时记录）
static mut IDLE_CTX: *mut SavedRegs = core::ptr::null_mut();

fn next_id() -> u32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// 创建内核线程
pub fn create_kernel_task(name: &'static str, entry: extern "C" fn()) -> u32 {
    unsafe {
        // 分配任务栈
        let stack_base = memory::alloc_pages(2).expect("任务栈分配失败"); // 16KB = 4 页
        let stack_top = stack_base + TASK_STACK_SIZE;

        // 在栈顶合成“被中断现场”：spsr = SVC 模式、IRQ 开启
        // 布局（自高向低）：spsr, lr(entry), r0..r12
        let spsr_off = stack_top - 4;
        let lr_off = stack_top - 8;
        let ctx_off = stack_top - 60;
        *(spsr_off as *mut u32) = 0x13; // SVC 模式
        *(lr_off as *mut u32) = entry as usize as u32;
        let ctx = ctx_off as *mut SavedRegs;
        (*ctx) = SavedRegs {
            r0: 0,
            r1: 0,
            r2: 0,
            r3: 0,
            r4: 0,
            r5: 0,
            r6: 0,
            r7: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            lr_exc: entry as usize as u32,
            spsr_exc: 0x13,
        };

        let id = next_id();
        let task = Task {
            id,
            name,
            state: TaskState::Ready,
            ctx,
            stack_base,
            quantum_left: DEFAULT_QUANTUM,
        };
        TASKS[id as usize % MAX_TASKS] = Some(task);
        push_ready(id);
        klog!("[sched] task#{} \"{}\" created", id, name);
        id
    }
}

fn push_ready(id: u32) {
    let h = RUN_HEAD.lock();
    let mut t = RUN_TAIL.lock();
    unsafe {
        let capacity = RUN_QUEUE.len();
        if (*h + 1) % capacity == *t {
            klog!("[sched] 就绪队列已满，丢弃 task#{}", id);
            return;
        }
        RUN_QUEUE[*t] = id;
        *t = (*t + 1) % capacity;
    }
}

fn pop_ready() -> Option<u32> {
    let mut h = RUN_HEAD.lock();
    let t = RUN_TAIL.lock();
    unsafe {
        if *h == *t {
            return None;
        }
        let id = RUN_QUEUE[*h];
        *h = (*h + 1) % RUN_QUEUE.len();
        Some(id)
    }
}

/// 定时器节拍：递减当前任务时间片，耗尽则标记需要调度
pub fn on_tick() {
    let cur = CURRENT.load(Ordering::Relaxed);
    if cur == u32::MAX {
        return;
    }
    unsafe {
        if let Some(t) = &mut TASKS[cur as usize % MAX_TASKS] {
            if t.state == TaskState::Running && t.quantum_left > 0 {
                t.quantum_left -= 1;
            }
        }
    }
}

/// 节拍/中断后的调度决策：返回应恢复的现场（可能切换到另一任务）
pub fn maybe_reschedule(ctx: *mut SavedRegs) -> *mut SavedRegs {
    let cur = CURRENT.load(Ordering::Relaxed);

    // 记录空闲现场（首次从空闲切走时）
    unsafe {
        if cur == u32::MAX {
            IDLE_CTX = ctx;
        }
    }

    // 当前任务时间片耗尽 → 入队尾
    if cur != u32::MAX {
        unsafe {
            if let Some(t) = &mut TASKS[cur as usize % MAX_TASKS] {
                if t.state == TaskState::Running && t.quantum_left == 0 {
                    t.ctx = ctx;
                    t.state = TaskState::Ready;
                    t.quantum_left = DEFAULT_QUANTUM;
                    push_ready(cur);
                    return switch_to_next(ctx);
                }
            }
        }
    } else {
        // 空闲态：有任务就切换
        if !queue_empty() {
            return switch_to_next(ctx);
        }
    }

    // 不切换：恢复原现场
    ctx
}

fn queue_empty() -> bool {
    let h = *RUN_HEAD.lock();
    let t = *RUN_TAIL.lock();
    h == t
}

fn switch_to_next(_cur_ctx: *mut SavedRegs) -> *mut SavedRegs {
    loop {
        let Some(id) = pop_ready() else {
            // 无任务：回到空闲现场
            unsafe {
                if !IDLE_CTX.is_null() {
                    CURRENT.store(u32::MAX, Ordering::Relaxed);
                    return IDLE_CTX;
                }
            }
            return _cur_ctx;
        };
        unsafe {
            let Some(t) = &mut TASKS[id as usize % MAX_TASKS] else {
                continue;
            };
            if t.state == TaskState::Dead {
                continue;
            }
            t.state = TaskState::Running;
            t.quantum_left = DEFAULT_QUANTUM;
            CURRENT.store(id, Ordering::Relaxed);
            return t.ctx;
        }
    }
}

/// 终止当前任务
pub fn kill_current() {
    let cur = CURRENT.load(Ordering::Relaxed);
    if cur == u32::MAX {
        return;
    }
    unsafe {
        if let Some(t) = &mut TASKS[cur as usize % MAX_TASKS] {
            t.state = TaskState::Dead;
            // 回收内核栈
            let _ = memory::free_pages(t.stack_base, 2);
            klog!("[sched] task#{} \"{}\" exited", t.id, t.name);
        }
        CURRENT.store(u32::MAX, Ordering::Relaxed);
    }
}

/// 当前任务 id（u32::MAX = 空闲）
pub fn current_id() -> u32 {
    CURRENT.load(Ordering::Relaxed)
}

/// 启动调度器（使能 IRQ 后进入空闲循环）
pub fn start_scheduler() -> ! {
    klog!("[sched] scheduler started, entering idle");
    unsafe {
        // 使能 IRQ（CPSR.I 清位）
        core::arch::asm!("cpsie i", options(nostack));
    }
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}

/// 从核空闲循环
pub fn secondary_idle(_core_id: u32) -> ! {
    unsafe {
        core::arch::asm!("cpsie i", options(nostack));
    }
    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}

// ===========================================================================
// 多核启动（spin 表）
// ===========================================================================
pub mod smp {
    use crate::klog;

    /// spin 表：物理 0x00000000 + core*0x10 + 4（Linux BCM2836 约定）
    const fn pen_addr(core: u32) -> usize {
        0x0000_0000 + core as usize * 0x10 + 4
    }

    /// 唤醒从核 1..3：写入入口地址后 sev
    pub fn wake_secondaries() {
        extern "C" {
            fn secondary_entry();
        }
        // 入口地址写入各核 spin 表（内核无 ASLR，地址固定；
        // 函数项 → 裸指针 → u32，绕开 fn→int 直接 cast 的 lint）
        let entry: u32 = secondary_entry as *const () as usize as u32;
        for core in 1..4u32 {
            unsafe {
                core::ptr::write_volatile(pen_addr(core) as *mut u32, entry);
            }
        }
        // DSB + SEV 唤醒
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
            core::arch::asm!("sev", options(nostack));
        }
        klog!("[smp] spin table written: entry={:#010x}", entry);
    }
}
