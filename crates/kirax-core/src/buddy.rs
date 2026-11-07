//! 二进制伙伴内存分配器（页帧级）。
//!
//! 经典 buddy 算法：
//! - 整块内存按 2 的幂划分为块；每个“阶(order)”对应一种块大小；
//! - 空闲块通过“侵入式链表”串起来：空闲块的首字直接存放下一个空闲块的索引，**不需要额外元数据内存**；
//! - 分配：找 >= 请求阶的最小空闲块，反复对半分裂，把多出的半块挂回低一阶；
//! - 释放：检查伙伴是否空闲，若空闲则合并到高一阶，递归向上。
//!
//! 边界语义（与测试对应）：
//! - 0 字节分配 → 按最小块（1 页）处理，返回合法地址；
//! - 超过池容量 → 返回 `None`；
//! - 重复释放 → 检测到 magic 已被破坏/块不在空闲表，返回 `Err`（不破坏内存）。

use crate::align_down;

/// 最大阶（32 阶覆盖 4KB → 16TB，足够 1GB 内存）
pub const MAX_ORDER: usize = 32;
/// 页大小（树莓派 2 物理内存按 4KB 页管理）
pub const PAGE_SIZE: usize = 4096;
/// 空闲块头部的 magic，用于检测双重释放
const FREE_MAGIC: u32 = 0xB0DDFEED;

/// 伙伴分配器（元数据只有 32 个链表头，可放进静态区）
#[derive(Debug, Clone)]
pub struct BuddyAllocator {
    /// 内存池起始物理地址
    base: usize,
    /// 内存池大小（字节）
    size: usize,
    /// 最小块阶（页大小对应的阶，4KB → 12）
    min_order: u32,
    /// 最大可用阶
    max_order: u32,
    /// 每阶空闲链表头（存块索引；块索引 = (地址-base)/最小块大小）
    free_heads: [Option<usize>; MAX_ORDER],
    /// 每阶空闲块计数（便于统计）
    free_counts: [u32; MAX_ORDER],
}

impl BuddyAllocator {
    pub const fn new() -> Self {
        Self {
            base: 0,
            size: 0,
            min_order: 0,
            max_order: 0,
            free_heads: [None; MAX_ORDER],
            free_counts: [0; MAX_ORDER],
        }
    }

    /// 初始化内存池。
    /// - `base` 必须按最小块大小对齐（**不要求**是 2 的幂——堆起点通常不是）；
    /// - `size` 会被向下取整到 2 的幂（仅保留最大完整块）。
    ///
    /// 阶语义统一：`free_heads[]` / `alloc_blocks()` / `free_blocks()` 中的阶
    /// 均为**相对最小块**的阶（0 = 1 个最小块）；`min_order` 字段保留绝对阶，
    /// 仅用于计算 `min_size()`。`max_order` 存相对最大阶。
    pub fn init(&mut self, base: usize, size: usize, min_order: u32) -> Result<(), &'static str> {
        let min_size = 1usize << min_order;
        if base % min_size != 0 {
            return Err("base 未按最小块对齐");
        }
        if size < min_size {
            return Err("size 小于最小块");
        }
        let size = align_down(size, min_size);
        // 取最大 2 的幂块（相对阶）：块大小 = min_size << rel_order，必须 <= size
        let mut rel_order: u32 = 0;
        while (min_size << (rel_order + 1)) <= size && rel_order + 1 < MAX_ORDER as u32 {
            rel_order += 1;
        }
        self.base = base;
        self.size = min_size << rel_order; // 绝对字节
        self.min_order = min_order;
        self.max_order = rel_order; // 相对最大阶
        self.free_heads = [None; MAX_ORDER];
        self.free_counts = [0; MAX_ORDER];
        // 整个池作为一个空闲大块入链
        let idx = 0usize;
        self.push_free(idx, rel_order)?;
        Ok(())
    }

    // ---- 内部：地址 ↔ 索引 ----
    #[inline]
    fn min_size(&self) -> usize {
        1usize << self.min_order
    }

    #[inline]
    fn idx_to_addr(&self, idx: usize) -> usize {
        self.base + idx * self.min_size()
    }

    #[inline]
    fn addr_to_idx(&self, addr: usize) -> Result<usize, &'static str> {
        if addr < self.base || addr >= self.base + self.size || addr % self.min_size() != 0 {
            return Err("地址不在池内或未对齐");
        }
        Ok((addr - self.base) / self.min_size())
    }

    /// 块索引 → 伙伴索引
    #[inline]
    fn buddy_idx(&self, idx: usize, order: u32) -> usize {
        idx ^ (1usize << order)
    }

    /// 空闲块的“下一块”指针存于块首（侵入式），并写入 magic。
    /// 块头布局：offset 0 = u32 magic，offset 8 = u64 next（p 已是 4KB 对齐，
    /// p+8 在 32/64 位均自然对齐；next 用 u64 宽度，兼容 ARMv7 32 位与 host 64 位测试）。
    unsafe fn block_next(&self, idx: usize) -> Option<usize> {
        let p = self.idx_to_addr(idx) as *const u8;
        let magic = core::ptr::read_volatile(p as *const u32);
        if magic != FREE_MAGIC {
            return None; // 块不在空闲表（或被破坏）
        }
        let next = core::ptr::read_volatile(p.add(8) as *const u64);
        if next == u64::MAX {
            None // 链尾哨兵
        } else {
            Some(next as usize)
        }
    }

    unsafe fn set_block_next(&self, idx: usize, next: Option<usize>) {
        let p = self.idx_to_addr(idx) as *mut u8;
        core::ptr::write_volatile(p as *mut u32, FREE_MAGIC);
        let next = next.unwrap_or(usize::MAX) as u64;
        core::ptr::write_volatile(p.add(8) as *mut u64, next);
    }

    /// 把块推入空闲链（头插）
    fn push_free(&mut self, idx: usize, order: u32) -> Result<(), &'static str> {
        let o = order as usize;
        unsafe {
            let next = self.free_heads[o];
            // 双重释放检测：magic==FREE_MAGIC 表示该块已在空闲状态
            // （分配时已清 magic=0，合并时 remove_from_chain 也清 magic）
            let p = self.idx_to_addr(idx) as *const u32;
            if core::ptr::read_volatile(p) == FREE_MAGIC {
                return Err("双重释放：块已在空闲链中");
            }
            self.set_block_next(idx, next);
        }
        self.free_heads[o] = Some(idx);
        self.free_counts[o] += 1;
        Ok(())
    }

    /// 从链头弹出块
    fn pop_free(&mut self, order: u32) -> Option<usize> {
        let o = order as usize;
        let head = self.free_heads[o]?;
        unsafe {
            let next = self.block_next(head).unwrap_or(usize::MAX);
            self.free_heads[o] = if next == usize::MAX { None } else { Some(next) };
        }
        self.free_counts[o] -= 1;
        Some(head)
    }

    fn chain_contains(&self, idx: usize, order: u32) -> bool {
        let mut cur = self.free_heads[order as usize];
        while let Some(c) = cur {
            if c == idx {
                return true;
            }
            cur = unsafe { self.block_next(c) };
        }
        false
    }

    // ---- 对外 API ----

    /// 分配 `2^order` 个最小块，返回物理地址（order 为相对阶，0 = 1 个最小块）
    pub fn alloc_blocks(&mut self, order: u32) -> Option<usize> {
        if order > self.max_order {
            return None; // 超过最大可分配阶
        }
        let order = order.min(self.max_order);
        // 找 >= order 的最小空闲阶
        let mut o = order;
        while o <= self.max_order && self.free_heads[o as usize].is_none() {
            o += 1;
        }
        if o > self.max_order {
            return None; // 内存耗尽
        }
        // 分裂到目标阶
        while o > order {
            let block = self.pop_free(o)?;
            // 弹出后清 magic，避免下方 push_free 误判双重释放
            unsafe {
                core::ptr::write_volatile(self.idx_to_addr(block) as *mut u32, 0);
            }
            let buddy = self.buddy_idx(block, o - 1);
            // 低半块留下（block），高半块（buddy）挂回 o-1 阶
            self.push_free(buddy, o - 1).ok()?;
            // 继续分裂 block
            self.push_free(block, o - 1).ok()?;
            o -= 1;
        }
        let idx = self.pop_free(order)?;
        // 清除块头 magic，标记该块已分配（非空闲）
        unsafe {
            let p = self.idx_to_addr(idx) as *mut u32;
            core::ptr::write_volatile(p, 0);
        }
        Some(self.idx_to_addr(idx))
    }

    /// 分配约 `size` 字节（向上取整到最小块），返回地址
    pub fn alloc_bytes(&mut self, size: usize) -> Option<usize> {
        // 0 字节：按 1 个最小块处理（保证返回唯一地址）
        let size = size.max(1);
        let blocks = (size + self.min_size() - 1) / self.min_size();
        let order = (blocks.next_power_of_two().trailing_zeros()) as u32;
        self.alloc_blocks(order)
    }

    /// 释放 `2^order` 个最小块的块（地址必须由 alloc_blocks 返回，order 为相对阶）
    pub fn free_blocks(&mut self, addr: usize, order: u32) -> Result<(), &'static str> {
        let idx = self.addr_to_idx(addr)?;
        let order = order.min(self.max_order);
        let mut idx = idx;
        let mut order = order;
        // 向上合并
        while order < self.max_order {
            let buddy = self.buddy_idx(idx, order);
            if self.chain_contains(buddy, order) {
                // 从链中移除 buddy
                self.remove_from_chain(buddy, order)?;
                // 合并：取两块中地址较低者
                idx = idx.min(buddy);
                order += 1;
            } else {
                break;
            }
        }
        self.push_free(idx, order)
    }

    fn remove_from_chain(&mut self, idx: usize, order: u32) -> Result<(), &'static str> {
        let o = order as usize;
        let mut prev: Option<usize> = None;
        let mut cur = self.free_heads[o];
        while let Some(c) = cur {
            if c == idx {
                // 清掉该块 magic，标记它已不在空闲链（被合并）
                unsafe {
                    core::ptr::write_volatile(self.idx_to_addr(c) as *mut u32, 0);
                }
                let next = unsafe { self.block_next(c).unwrap_or(usize::MAX) };
                match prev {
                    Some(p) => unsafe { self.set_block_next(p, if next == usize::MAX { None } else { Some(next) }) },
                    None => {
                        self.free_heads[o] = if next == usize::MAX { None } else { Some(next) }
                    }
                }
                self.free_counts[o] -= 1;
                return Ok(());
            }
            prev = Some(c);
            cur = unsafe { self.block_next(c) };
        }
        Err("伙伴不在空闲链中（重复释放或地址错误）")
    }

    /// 释放约 `size` 字节（需与 alloc_bytes 相同的向上取整语义）
    pub fn free_bytes(&mut self, addr: usize, size: usize) -> Result<(), &'static str> {
        let size = size.max(1);
        let blocks = (size + self.min_size() - 1) / self.min_size();
        let order = (blocks.next_power_of_two().trailing_zeros()) as u32;
        self.free_blocks(addr, order)
    }

    // ---- 统计 ----

    /// 池总字节数
    pub fn total_bytes(&self) -> usize {
        self.size
    }

    /// 空闲字节数（近似：各阶空闲块 × 块大小）
    pub fn free_bytes_total(&self) -> usize {
        let mut sum = 0usize;
        for o in 0..MAX_ORDER {
            let cnt = self.free_counts[o] as usize;
            sum += cnt * (self.min_size() << o);
        }
        sum
    }

    pub fn free_blocks_of(&self, order: u32) -> u32 {
        let order = order.min(self.max_order);
        self.free_counts[order as usize]
    }
}

impl Default for BuddyAllocator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::align_up;
    use std::vec::Vec;

    /// 测试用内存池：在 host 上分配真实 Vec，base = Vec 内部指针。
    /// 裸机上 base 直接指向物理 RAM；host 测试必须用真实可写缓冲，
    /// 否则 set_block_next 写假物理地址会段错误。
    const POOL_SIZE: usize = 8 * 1024 * 1024;

    struct TestPool {
        #[allow(dead_code)]
        buf: Vec<u8>,
        b: BuddyAllocator,
        base: usize,
    }

    fn new_pool() -> TestPool {
        let mut buf = vec![0u8; POOL_SIZE];
        // 按最小块对齐 Vec 起点
        let raw = buf.as_mut_ptr() as usize;
        let aligned = align_up(raw, PAGE_SIZE);
        let off = aligned - raw;
        let usable = buf.len() - off;
        let mut b = BuddyAllocator::new();
        b.init(aligned, usable, 12).unwrap();
        TestPool { buf, b, base: aligned }
    }

    #[test]
    fn normal_alloc_free_merge() {
        let mut t = new_pool();
        let base = t.base;
        let total = t.b.free_bytes_total();
        let a = t.b.alloc_bytes(4096).unwrap();
        let b2 = t.b.alloc_bytes(4096).unwrap();
        let c = t.b.alloc_bytes(8192).unwrap();
        assert!(a >= base && a < base + POOL_SIZE);
        assert!(a != b2 && a != c && b2 != c);
        t.b.free_bytes(a, 4096).unwrap();
        assert!(t.b.free_bytes_total() > total - 4096 * 2 - 8192);
        t.b.free_bytes(b2, 4096).unwrap();
        t.b.free_bytes(c, 8192).unwrap();
        assert_eq!(t.b.free_bytes_total(), total);
    }

    #[test]
    fn zero_byte_alloc() {
        let mut t = new_pool();
        let p = t.b.alloc_bytes(0).unwrap();
        assert_eq!(p % PAGE_SIZE, 0);
        t.b.free_bytes(p, 0).unwrap();
        assert_eq!(t.b.free_bytes_total(), t.b.total_bytes());
    }

    #[test]
    fn oversized_alloc() {
        let mut t = new_pool();
        assert!(t.b.alloc_bytes(POOL_SIZE * 2).is_none());
        // 请求超过最大阶 → None
        assert!(t.b.alloc_blocks(MAX_ORDER as u32).is_none());
    }

    #[test]
    fn double_free_detected() {
        let mut t = new_pool();
        let p = t.b.alloc_bytes(4096).unwrap();
        t.b.free_bytes(p, 4096).unwrap();
        assert!(t.b.free_bytes(p, 4096).is_err());
        assert!(t.b.free_bytes(p, 4096).is_err());
        let q = t.b.alloc_bytes(4096).unwrap();
        assert_eq!(q % PAGE_SIZE, 0);
        t.b.free_bytes(q, 4096).unwrap();
    }

    #[test]
    fn exhaustion_then_reclaim() {
        let mut t = new_pool();
        let mut addrs = std::vec![];
        loop {
            match t.b.alloc_bytes(PAGE_SIZE) {
                Some(a) => addrs.push(a),
                None => break,
            }
        }
        assert_eq!(t.b.free_bytes_total(), 0);
        assert_eq!(t.b.alloc_bytes(PAGE_SIZE), None);
        for a in addrs.iter() {
            t.b.free_bytes(*a, PAGE_SIZE).unwrap();
        }
        assert_eq!(t.b.free_bytes_total(), t.b.total_bytes());
    }

    #[test]
    fn fragmentation_stats() {
        let mut t = new_pool();
        // 连续分配 4 个 4096 块（idx 0,1,2,3）
        let mut addrs = vec![];
        for _ in 0..4 {
            addrs.push(t.b.alloc_bytes(4096).unwrap());
        }
        // 释放 addrs[0] 与 addrs[2]：二者被 addrs[1]/addrs[3] 隔开，无法向上合并 → 碎片
        t.b.free_bytes(addrs[0], 4096).unwrap();
        t.b.free_bytes(addrs[2], 4096).unwrap();
        assert_eq!(t.b.free_blocks_of(0), 2);
        // 全部释放 → 相邻块逐级合并，整池回到单一最大阶块
        t.b.free_bytes(addrs[1], 4096).unwrap();
        t.b.free_bytes(addrs[3], 4096).unwrap();
        assert_eq!(t.b.free_bytes_total(), t.b.total_bytes());
        assert_eq!(t.b.free_blocks_of(t.b.max_order), 1);
    }

    #[test]
    fn misaligned_addr_rejected() {
        let mut t = new_pool();
        let base = t.base;
        let p = t.b.alloc_bytes(4096).unwrap();
        t.b.free_bytes(p, 4096).unwrap();
        assert!(t.b.free_bytes(base - 1, 4096).is_err());
        assert!(t.b.free_bytes(base + POOL_SIZE, 4096).is_err());
        assert!(t.b.free_bytes(base + 1, 4096).is_err());
    }
}
