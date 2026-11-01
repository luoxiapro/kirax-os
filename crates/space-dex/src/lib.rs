//! # space-dex — Android DEX 解析器与字节码解释器
//!
//! Space OS APK 兼容层（参考卓易通）的核心执行引擎。目标：在 `no_std` 环境
//! （thumbv7a-none-eabi 可编入 .kir）解析并执行 APK 的 `classes.dex` 字节码，
//! 同时在主机可直接 `cargo test`。
//!
//! 当前实现（MVP，真实可运行）：
//! - DEX 文件解析：header / string_ids / type_ids / proto_ids / method_ids /
//!   field_ids / class_defs / class_data / code_item（MUTF-8 + uleb128 + 小端）
//! - 寄存器虚拟机解释器：算术/逻辑/移位、常量加载、move、goto、条件分支、
//!   方法调用（static/direct/virtual/interface + /range）、返回、monitor no-op、
//!   静态字段（sget/sput）与实例字段（iget/iput，句柄对象模型）
//! - Native 桩表：`java.lang.Math` 等少量真实实现 + 其余报明确错误
//!
//! 未实现（路线图，按依赖排序）：
//! - `tries` 异常处理表（throw 直接终止执行）
//! - 数组 aget/aput、new-array、fill-array-data（部分指令已识别未执行）
//! - Deflate（APK 内 classes.dex 常为 deflate 压缩，需先实现 inflate）
//! - Binder / Activity 生命周期 / View 系统 / ART 对象模型
//! - DEX → .kir AOT 转译（解释器稳定后可做）
//!
//! 用法：
//! ```rust,ignore
//! use space_dex::dex::DexFile;
//! use space_dex::interp::{Interp, Options};
//!
//! let dex = DexFile::parse(&bytes)?;
//! let mut ip = Interp::new(&dex);
//! let ret = ip.invoke_static("LMain;", "compute", "(I)I", &[5])?;
//! ```

#![no_std]

extern crate alloc;

// host 构建/测试（cargo build / cargo test）：分配器与 panic_handler 由 std
// 测试 harness 或最终宿主提供；thumbv7a 作为依赖编译时无需 std。
// 独立打包（--features kir-standalone）时由下方 standalone 模块提供
// 最小 allocator + panic handler（仅用于链接通过，Space OS 内核内运行时
// 已提供全局分配器，勿在真机启用本 feature）。

#[cfg(feature = "kir-standalone")]
#[allow(static_mut_refs)]
mod standalone {
    use core::alloc::{GlobalAlloc, Layout};
    use core::panic::PanicInfo;

    static mut HEAP: [u8; 128 * 1024] = [0u8; 128 * 1024];
    static mut OFFSET: usize = 0;

    pub struct Bump;

    unsafe impl GlobalAlloc for Bump {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let start = OFFSET;
            let aligned = (start + layout.align() - 1) & !(layout.align() - 1);
            if aligned + layout.size() > HEAP.len() {
                return core::ptr::null_mut();
            }
            OFFSET = aligned + layout.size();
            HEAP.as_mut_ptr().add(aligned)
        }
        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
    }

    #[global_allocator]
    static GLOBAL: Bump = Bump;

    #[panic_handler]
    fn panic(_info: &PanicInfo) -> ! {
        loop {}
    }
}

pub mod dex;
pub mod interp;
pub mod reader;

#[cfg(test)]
pub mod tests;
