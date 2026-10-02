//! # space-core —— Space OS 纯逻辑内核库
//!
//! 设计目标：
//! 1. 与硬件完全解耦：所有算法（内存分配、模糊、混合、圆角、FAT32、.kir 打包）都在这里实现，
//!    内核只负责把硬件寄存器转换成对这些函数的调用。
//! 2. 可测试：`#![no_std]` 但测试时链接 `std`，因此 `cargo test` 可以在主机上直接跑全部单元测试，
//!    满足“每个内核模块实现后立即编写单元测试”的要求。
//! 3. 零依赖：不引入任何 crates.io 第三方库，符合“全程自研、无第三方库”的约束。
//!
//! 模块一览：
//! - [`buddy`]      二进制伙伴内存分配器（页帧分配）
//! - [`blur`]       三次盒式模糊近似高斯模糊（含降采样/升采样）
//! - [`blend`]      Alpha 混合（source-over）
//! - [`roundrect`]  圆角矩形 SDF 覆盖率（抗锯齿填充/描边/高光）
//! - [`fat32`]      FAT32 文件系统逻辑（块设备抽象之上）
//! - [`kir`]        .kir 应用二进制格式（打包/解析）
//! - [`glass`]      Glass 设计规范令牌（色值/透明度/模糊半径参数表）
//! - [`events`]     输入事件定义与无锁环形缓冲

#![no_std]

#[cfg(test)]
extern crate std;

// alloc 宏（vec!/format!）全库可用；内核侧由全局分配器提供支撑
#[macro_use]
extern crate alloc;

pub mod blend;
pub mod blur;
pub mod buddy;
pub mod events;
pub mod fat32;
pub mod fmath;
pub mod glass;
pub mod kir;
pub mod roundrect;

/// 通用工具：向上对齐
#[inline]
pub const fn align_up(x: usize, align: usize) -> usize {
    (x + align - 1) & !(align - 1)
}

/// 通用工具：向下对齐
#[inline]
pub const fn align_down(x: usize, align: usize) -> usize {
    x & !(align - 1)
}

/// 判断是否 2 的幂
#[inline]
pub const fn is_pow2(x: usize) -> bool {
    x != 0 && (x & (x - 1)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("rr_check.rs");

    #[test]
    fn align_works() {
        assert_eq!(align_up(1, 4096), 4096);
        assert_eq!(align_up(4096, 4096), 4096);
        assert_eq!(align_up(4095, 16), 4096);
        assert_eq!(align_down(4097, 4096), 4096);
        assert!(is_pow2(1));
        assert!(is_pow2(4096));
        assert!(!is_pow2(4095));
        assert!(!is_pow2(0));
    }
}
