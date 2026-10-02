//! 板载自检（对应“每个内核模块实现后立即编写单元测试”的裸机侧验证）。
//!
//! 主机侧 `cargo test` 覆盖 space-core 全部算法；本模块在**真机/QEMU**上
//! 复跑关键路径（内存分配、模糊、混合、.kir、FAT32），全部通过输出
//! `SELFTEST PASS`（供 tests/run_tests.sh 抓取判定）。

use crate::klog;
use alloc::vec;
use alloc::vec::Vec;

/// 运行全部自检
pub fn run_all() {
    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! check {
        ($name:expr, $cond:expr) => {
            if $cond {
                klog!("[test] PASS  {}", $name);
                passed += 1;
            } else {
                klog!("[test] FAIL  {}", $name);
                failed += 1;
            }
        };
    }

    // ---- 1. 伙伴分配器（通过全局分配器间接验证 + 直接 API） ----
    // 容错处理：QEMU 某些配置下分配可能失败，不 panic，记录 FAIL 供测试脚本判定。
    {
        let a = crate::memory::alloc_pages(0);
        let b = crate::memory::alloc_pages(0);
        match (a, b) {
            (Some(a), Some(b)) => {
                check!("buddy 正常分配", a != b);
                let _ = crate::memory::free_pages(a, 0);
                let _ = crate::memory::free_pages(b, 0);
                check!("buddy 释放合并", crate::memory::free_memory() > 0);
                check!("buddy 重复释放拒绝", crate::memory::free_pages(a, 0).is_err());
            }
            _ => check!("buddy 正常分配", false),
        }
    }

    // ---- 2. 全局分配器（Vec 分配/释放） ----
    {
        let mut v: Vec<u8> = Vec::with_capacity(1024);
        for i in 0..1024 {
            v.push((i % 251) as u8);
        }
        let sum: u32 = v.iter().map(|&x| x as u32).sum();
        check!("global allocator Vec 读写", v.len() == 1024 && sum > 0);
        drop(v);
    }

    // ---- 3. 模糊（三次盒式近似） ----
    {
        use space_core::blur::{box_width_for_sigma, gaussian_blur_3box};
        check!("box 宽度为奇数", box_width_for_sigma(4.0, 3) % 2 == 1);
        // 纯色图模糊后不变
        let w = 32;
        let h = 32;
        let mut src = vec![0u8; w * h * 4];
        for px in src.chunks_mut(4) {
            px.copy_from_slice(&[200, 100, 50, 255]);
        }
        let mut dst = vec![0u8; w * h * 4];
        gaussian_blur_3box(&src, w, h, 4.0, &mut dst);
        check!("模糊保持纯色", dst == src);
    }

    // ---- 4. Alpha 混合 ----
    {
        use space_core::blend::blend_pixel;
        let mut d = [10u8, 20, 30, 255];
        blend_pixel(&mut d, [200, 100, 50, 255]);
        check!("不透明覆盖", d == [200, 100, 50, 255]);
        let mut d2 = [10u8, 20, 30, 255];
        blend_pixel(&mut d2, [200, 100, 50, 0]);
        check!("全透明无变化", d2 == [10, 20, 30, 255]);
    }

    // ---- 5. 圆角 SDF ----
    {
        use space_core::roundrect::{coverage, Rect};
        let r = Rect::new(10, 10, 100, 60);
        check!("圆角中心覆盖率=1", (coverage(60.0, 40.0, &r, 10.0) - 1.0).abs() < 0.01);
        check!("圆角外部覆盖率=0", coverage(5.0, 5.0, &r, 10.0) == 0.0);
    }

    // ---- 6. .kir 打包/解析 ----
    {
        use space_core::kir::{extract_code, pack, KirHeader};
        let code: Vec<u8> = (0u8..64).collect();
        let icon = vec![0x11u8; 16 * 16 * 4];
        let kir = pack(&code, &icon, 16, 16, "Selftest", "Space", "0.1.0", 4096, 8192, true, false);
        let h = KirHeader::parse(&kir).unwrap();
        check!("kir 头部解析", h.is_valid() && h.code_size == 64);
        check!("kir 代码提取", extract_code(&kir).unwrap() == code);
        // 错误魔数拒绝
        let mut bad = kir.clone();
        bad[0] = 0;
        check!("kir 魔数校验", KirHeader::parse(&bad).is_err());
    }

    // ---- 7. FAT32（内存盘） ----
    {
        use space_core::fat32::{BlockDevice, Fat32Fs};
        struct RamDisk(Vec<u8>);
        impl BlockDevice for RamDisk {
            fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
                let s = lba as usize * 512;
                if s + buf.len() > self.0.len() {
                    return Err("越界");
                }
                buf.copy_from_slice(&self.0[s..s + buf.len()]);
                Ok(())
            }
            fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
                let s = lba as usize * 512;
                if s + buf.len() > self.0.len() {
                    return Err("越界");
                }
                self.0[s..s + buf.len()].copy_from_slice(buf);
                Ok(())
            }
            fn sector_size(&self) -> u16 {
                512
            }
        }
        let img = build_fat32_image();
        let mut fs = Fat32Fs::mount(RamDisk(img)).unwrap();
        let mut buf = [0u8; 32];
        let n = fs.read_file("/HELLO.TXT", 0, &mut buf).unwrap();
        check!("FAT32 挂载并读取", n == 16 && buf[..16].starts_with(b"Hello, Space OS"));
    }

    if failed == 0 {
        klog!("========================================");
        klog!("[test] SELFTEST PASS  ({} checks)", passed);
        klog!("========================================");
    } else {
        klog!("========================================");
        klog!("[test] SELFTEST FAIL  ({} passed, {} failed)", passed, failed);
        klog!("========================================");
    }
}

/// 构造迷你 FAT32 镜像（供裸机自检）
fn build_fat32_image() -> Vec<u8> {
    let mut img = vec![0u8; 64 * 512];
    // 引导扇区
    img[0] = 0xEB;
    img[1] = 0x3C;
    img[2] = 0x90;
    img[3..11].copy_from_slice(b"SPACEOS!");
    img[11] = 0x00;
    img[12] = 0x02; // 512 字节扇区
    img[13] = 0x01; // 1 扇区/簇
    img[14] = 0x01; // 保留 1 扇区
    img[15] = 0x00;
    img[16] = 0x02; // 2 个 FAT
    img[20] = 0xF8;
    img[32..36].copy_from_slice(&(32u32).to_le_bytes()); // 总扇区
    img[36..40].copy_from_slice(&(4u32).to_le_bytes()); // FAT 大小 4 扇区
    img[44..48].copy_from_slice(&(2u32).to_le_bytes()); // 根簇 2
    img[71..82].copy_from_slice(b"SPACE OS   ");
    img[510] = 0x55;
    img[511] = 0xAA;
    // FAT1（LBA 1..5）
    let fat = 1 * 512usize;
    let set = |img: &mut [u8], c: usize, v: u32| {
        img[fat + c * 4..fat + c * 4 + 4].copy_from_slice(&(v & 0x0FFF_FFFF).to_le_bytes());
    };
    set(&mut img, 0, 0x0FFF_FFF8);
    set(&mut img, 1, 0x0FFF_FFFF);
    set(&mut img, 2, 0x0FFF_FFFF); // 根目录
    set(&mut img, 3, 0x0FFF_FFFF); // HELLO.TXT 单簇
    // FAT2 镜像
    img.copy_within(1 * 512..5 * 512, 5 * 512);
    // 根目录（簇 2 = LBA 9）
    let root = 9 * 512;
    let content = b"Hello, Space OS!";
    // HELLO.TXT 目录项
    let mut sfn = [b' '; 11];
    sfn[0..5].copy_from_slice(b"HELLO");
    sfn[8..11].copy_from_slice(b"TXT");
    img[root..root + 11].copy_from_slice(&sfn);
    img[root + 11] = 0x20; // archive
    img[root + 26..root + 28].copy_from_slice(&3u16.to_le_bytes()); // 簇 3
    img[root + 28..root + 32].copy_from_slice(&(content.len() as u32).to_le_bytes());
    // 内容（簇 3 = LBA 10）
    let data = 10 * 512;
    img[data..data + content.len()].copy_from_slice(content);
    img
}
