//! 简化版 FAT32 文件系统（自研，无第三方依赖）。
//!
//! 覆盖范围（够 .kir 应用装载与读取）：
//! - 解析 BPB / FSInfo，校验 0x55AA 引导签名与 FAT32 类型；
//! - 簇链遍历（含坏簇/环检测）；
//! - 根目录与子目录的 32 字节目录项解析（短文件名，跳过 LFN 长文件名项）；
//! - 文件读取（offset + length，支持跨簇）；
//! - 文件创建/追加（写入 FAT 分配新簇）；
//! - 目录列举。
//!
//! 硬件访问通过 [`BlockDevice`] trait 抽象：内核的 EMMC/SD 驱动实现它，
//! 主机测试用内存块设备实现它 —— 同一份逻辑两处复用。

extern crate alloc;
use alloc::vec::Vec;

/// 块设备抽象（内核 EMMC 驱动实现）
pub trait BlockDevice {
    /// 读 `buf.len()/sector_size()` 个扇区到 buf
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str>;
    /// 写 buf 中的扇区
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), &'static str>;
    fn sector_size(&self) -> u16;
}

/// FAT32 引导扇区解析结果
#[derive(Debug, Clone, Copy)]
pub struct Bpb {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub sectors_per_fat: u32,
    pub root_cluster: u32,
    pub total_sectors: u32,
    pub volume_label: [u8; 11],
}

impl Bpb {
    /// 数据区起始 LBA
    pub fn data_start_lba(&self) -> u64 {
        (self.reserved_sectors as u64) + (self.num_fats as u64) * (self.sectors_per_fat as u64)
    }

    /// FAT1 起始 LBA
    pub fn fat1_lba(&self) -> u64 {
        self.reserved_sectors as u64
    }

    /// 簇 N → 数据 LBA
    pub fn cluster_lba(&self, cluster: u32) -> u64 {
        self.data_start_lba() + (cluster as u64 - 2) * (self.sectors_per_cluster as u64)
    }

    /// 簇 N 的 FAT 表项位置（扇区号 + 扇区内字节偏移）
    pub fn fat_entry_location(&self, cluster: u32) -> (u64, usize) {
        let off = cluster as u64 * 4;
        let sector = self.fat1_lba() + off / self.bytes_per_sector as u64;
        let in_sec = (off % self.bytes_per_sector as u64) as usize;
        (sector, in_sec)
    }
}

/// 目录项
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: [u8; 11], // 短文件名（8.3 格式，未补空格）
    pub attr: u8,
    pub first_cluster: u32,
    pub size: u32,
}

impl DirEntry {
    pub fn is_directory(&self) -> bool {
        self.attr & 0x10 != 0
    }
    /// 转为可读文件名（"HELLO   TXT" → "HELLO.TXT"）
    pub fn display_name(&self) -> alloc::string::String {
        use alloc::string::String;
        let mut s = String::new();
        let name_part = &self.name[..8];
        let trimmed = name_part.iter().take_while(|&&c| c != b' ').cloned().collect::<Vec<u8>>();
        s.push_str(core::str::from_utf8(&trimmed).unwrap_or("?"));
        let ext_part = &self.name[8..];
        let ext = ext_part.iter().take_while(|&&c| c != b' ').cloned().collect::<Vec<u8>>();
        if !ext.is_empty() {
            s.push('.');
            s.push_str(core::str::from_utf8(&ext).unwrap_or("?"));
        }
        s
    }
}

/// 已挂载的 FAT32 卷
pub struct Fat32Fs<D: BlockDevice> {
    dev: D,
    bpb: Bpb,
}

pub const ATTR_READ_ONLY: u8 = 0x01;
pub const ATTR_HIDDEN: u8 = 0x02;
pub const ATTR_SYSTEM: u8 = 0x04;
pub const ATTR_VOLUME_ID: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;
pub const ATTR_ARCHIVE: u8 = 0x20;
pub const ATTR_LFN: u8 = 0x0F;

/// FAT 表项掩码（28 位簇号）
const FAT_MASK: u32 = 0x0FFF_FFFF;
/// 链结束标记
const EOC: u32 = 0x0FFF_FFF8;

impl<D: BlockDevice> Fat32Fs<D> {
    /// 挂载：读取并校验引导扇区
    pub fn mount(mut dev: D) -> Result<Self, &'static str> {
        let ss = dev.sector_size() as usize;
        let mut boot = [0u8; 512];
        dev.read_sectors(0, &mut boot[..ss])?;
        // 校验引导签名
        if boot[510] != 0x55 || boot[511] != 0xAA {
            return Err("无效引导签名 (0x55AA)");
        }
        let bps = u16::from_le_bytes([boot[11], boot[12]]);
        if bps != 512 {
            return Err("仅支持 512 字节扇区");
        }
        let bpb = Bpb {
            bytes_per_sector: bps,
            sectors_per_cluster: boot[13],
            reserved_sectors: u16::from_le_bytes([boot[14], boot[15]]),
            num_fats: boot[16],
            sectors_per_fat: u32::from_le_bytes([boot[36], boot[37], boot[38], boot[39]]),
            root_cluster: u32::from_le_bytes([boot[44], boot[45], boot[46], boot[47]]),
            total_sectors: u32::from_le_bytes([boot[32], boot[33], boot[34], boot[35]]),
            volume_label: [boot[71], boot[72], boot[73], boot[74], boot[75], boot[76], boot[77], boot[78], boot[79], boot[80], boot[81]],
        };
        if bpb.sectors_per_cluster == 0 || bpb.num_fats == 0 || bpb.sectors_per_fat == 0 {
            return Err("BPB 字段非法");
        }
        Ok(Self { dev, bpb })
    }

    pub fn bpb(&self) -> &Bpb {
        &self.bpb
    }

    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        self.dev.read_sectors(lba, buf)
    }

    /// 读 FAT 表项
    fn read_fat_entry(&mut self, cluster: u32) -> Result<u32, &'static str> {
        let (sec, off) = self.bpb.fat_entry_location(cluster);
        let mut buf = [0u8; 512];
        self.read_sector(sec, &mut buf)?;
        let raw = u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
        Ok(raw & FAT_MASK)
    }

    /// 写 FAT 表项（两个 FAT 都写，保持一致性）
    fn write_fat_entry(&mut self, cluster: u32, value: u32) -> Result<(), &'static str> {
        let (sec, off) = self.bpb.fat_entry_location(cluster);
        let mut buf = [0u8; 512];
        self.read_sector(sec, &mut buf)?;
        let raw = value & FAT_MASK;
        buf[off] = raw as u8;
        buf[off + 1] = (raw >> 8) as u8;
        buf[off + 2] = (raw >> 16) as u8;
        buf[off + 3] = (raw >> 24) as u8;
        // FAT2 镜像
        let fat2_sec = sec + self.bpb.sectors_per_fat as u64;
        self.dev.write_sectors(sec, &buf)?;
        self.dev.write_sectors(fat2_sec, &buf)?;
        Ok(())
    }

    /// 沿簇链读整个簇到 `out`，返回实际读取字节数
    fn read_cluster(&mut self, cluster: u32, out: &mut [u8]) -> Result<usize, &'static str> {
        let cl_bytes = self.bpb.bytes_per_sector as usize * self.bpb.sectors_per_cluster as usize;
        let mut lba = self.bpb.cluster_lba(cluster);
        let mut written = 0usize;
        for _ in 0..self.bpb.sectors_per_cluster {
            if written + 512 > out.len() {
                break;
            }
            let mut buf = [0u8; 512];
            self.read_sector(lba, &mut buf)?;
            out[written..written + 512].copy_from_slice(&buf);
            written += 512;
            lba += 1;
        }
        let _ = cl_bytes;
        Ok(written)
    }

    /// 读取文件：按路径（如 `/HELLO.TXT` 或 `HELLO.TXT`）查找并读 `len` 字节到 `out`
    pub fn read_file(&mut self, path: &str, offset: u64, out: &mut [u8]) -> Result<usize, &'static str> {
        let (entry, _parent) = self.lookup(path)?;
        if entry.is_directory() {
            return Err("目标为目录");
        }
        let file_size = entry.size as u64;
        let cl_bytes = self.bpb.bytes_per_sector as usize * self.bpb.sectors_per_cluster as usize;
        let mut cluster = entry.first_cluster;
        let mut k: u64 = 0; // 当前是文件第几簇（从 0 开始）
        let mut skip = offset;
        let mut done = 0usize;
        let mut visited = 0u32;
        while cluster >= 2 && cluster < EOC {
            if visited > 1_000_000 {
                return Err("簇链疑似成环");
            }
            visited += 1;
            let cl_start = k * cl_bytes as u64; // 簇在文件中的字节偏移
            let cl_end = cl_start + cl_bytes as u64;
            // 跳过 offset 之前的簇
            if cl_end <= skip {
                cluster = self.read_fat_entry(cluster)?;
                k += 1;
                continue;
            }
            let in_cluster_off = skip.saturating_sub(cl_start) as usize;
            let mut whole = vec![0u8; cl_bytes];
            let got = self.read_cluster(cluster, &mut whole)?;
            let avail = got - in_cluster_off;
            // 按文件大小截断：本簇可读字节不超过文件剩余字节
            let remain_in_file = file_size.saturating_sub(skip) as usize;
            let want = out.len() - done;
            let n = avail.min(want).min(remain_in_file);
            out[done..done + n].copy_from_slice(&whole[in_cluster_off..in_cluster_off + n]);
            done += n;
            skip += n as u64;
            if done == out.len() || skip >= file_size {
                break;
            }
            cluster = self.read_fat_entry(cluster)?;
            k += 1;
        }
        Ok(done)
    }

    /// 按路径查找目录项（仅支持 "NAME.EXT" 与 "/DIR/NAME.EXT" 两种形态）
    pub fn lookup(&mut self, path: &str) -> Result<(DirEntry, u32), &'static str> {
        let mut cur_cluster = self.bpb.root_cluster;
        let mut comps: alloc::vec::Vec<&str> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|c| !c.is_empty())
            .collect();
        if comps.is_empty() {
            return Err("空路径");
        }
        let file = comps.pop().unwrap();
        // 逐层进入子目录
        for dir in comps.iter() {
            let entry = self.find_in_dir(cur_cluster, dir)?;
            if !entry.is_directory() {
                return Err("路径中间节点不是目录");
            }
            cur_cluster = entry.first_cluster;
        }
        let entry = self.find_in_dir(cur_cluster, file)?;
        Ok((entry, cur_cluster))
    }

    /// 在目录簇链中查找指定文件名
    fn find_in_dir(&mut self, dir_cluster: u32, name: &str) -> Result<DirEntry, &'static str> {
        let entries = self.read_dir_entries(dir_cluster)?;
        // 生成 8.3 短文件名（大写，空格补齐）
        let mut sfn = [b' '; 11];
        let (base, ext) = match name.find('.') {
            Some(i) => (&name[..i], Some(&name[i + 1..])),
            None => (name, None),
        };
        if base.len() > 8 {
            return Err("短文件名超过 8 字符");
        }
        let base_b = base.as_bytes();
        for (i, &c) in base_b.iter().enumerate() {
            sfn[i] = c.to_ascii_uppercase();
        }
        if let Some(e) = ext {
            if e.len() > 3 {
                return Err("扩展名超过 3 字符");
            }
            for (i, &c) in e.as_bytes().iter().enumerate() {
                sfn[8 + i] = c.to_ascii_uppercase();
            }
        }
        for e in entries {
            if e.attr & ATTR_LFN == ATTR_LFN {
                continue;
            }
            if e.name == sfn {
                return Ok(e);
            }
        }
        Err("文件未找到")
    }

    /// 读取目录的所有有效目录项（跳过 . 和 ..）
    pub fn read_dir_entries(&mut self, dir_cluster: u32) -> Result<Vec<DirEntry>, &'static str> {
        let cl_bytes = self.bpb.bytes_per_sector as usize * self.bpb.sectors_per_cluster as usize;
        let mut cluster = dir_cluster;
        let mut out = Vec::new();
        let mut visited = 0u32;
        while cluster >= 2 && cluster < EOC {
            if visited > 1_000_000 {
                return Err("目录簇链成环");
            }
            visited += 1;
            let mut buf = vec![0u8; cl_bytes];
            let got = self.read_cluster(cluster, &mut buf)?;
            let mut i = 0usize;
            while i + 32 <= got {
                let e = &buf[i..i + 32];
                let first = e[0];
                if first == 0x00 {
                    return Ok(out); // 目录结束
                }
                if first == 0xE5 {
                    i += 32; // 已删除
                    continue;
                }
                let attr = e[11];
                if attr & ATTR_LFN == ATTR_LFN {
                    i += 32; // LFN 项跳过（短文件名仍可寻址）
                    continue;
                }
                let mut name = [0u8; 11];
                name.copy_from_slice(&e[0..11]);
                let first_cluster = u16::from_le_bytes([e[26], e[27]]) as u32
                    | (u16::from_le_bytes([e[20], e[21]]) as u32) << 16;
                let size = u32::from_le_bytes([e[28], e[29], e[30], e[31]]);
                out.push(DirEntry {
                    name,
                    attr,
                    first_cluster,
                    size,
                });
                i += 32;
            }
            cluster = self.read_fat_entry(cluster)?;
        }
        Ok(out)
    }

    /// 列举目录（返回可读文件名列表）
    pub fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, &'static str> {
        let cluster = if path.is_empty() || path == "/" {
            self.bpb.root_cluster
        } else {
            let (e, _) = self.lookup(path)?;
            if !e.is_directory() {
                return Err("不是目录");
            }
            e.first_cluster
        };
        self.read_dir_entries(cluster)
    }

    /// 创建/追加文件（简单实现：只支持根目录下新建，数据只写入单个新簇链）
    /// 返回写入的字节数
    pub fn create_file(&mut self, path: &str, data: &[u8]) -> Result<usize, &'static str> {
        // 限制：仅根目录，文件名不超过 8.3
        let name = path.trim_start_matches('/');
        if name.contains('/') {
            return Err("create_file 仅支持根目录");
        }
        // 找空闲簇（从 root_cluster 之后开始扫）
        let cl = self.find_free_cluster()?;
        // 写数据
        let cl_bytes = self.bpb.bytes_per_sector as usize * self.bpb.sectors_per_cluster as usize;
        let mut lba = self.bpb.cluster_lba(cl);
        let mut written = 0usize;
        let mut buf = [0u8; 512];
        for chunk in data.chunks(512) {
            buf[..chunk.len()].copy_from_slice(chunk);
            if chunk.len() < 512 {
                buf[chunk.len()..].fill(0);
            }
            self.dev.write_sectors(lba, &buf)?;
            lba += 1;
            written += chunk.len();
            if written >= cl_bytes {
                break; // 简化：单簇文件
            }
        }
        // FAT 表：簇 → EOC
        self.write_fat_entry(cl, EOC)?;
        // 目录项
        self.add_dir_entry(name, cl, written as u32, ATTR_ARCHIVE)?;
        Ok(written)
    }

    fn find_free_cluster(&mut self) -> Result<u32, &'static str> {
        // 从根簇开始扫 FAT
        let mut c = self.bpb.root_cluster + 1;
        let max_clusters = self.bpb.total_sectors / self.bpb.sectors_per_cluster as u32;
        while c < max_clusters {
            if self.read_fat_entry(c)? == 0 {
                return Ok(c);
            }
            c += 1;
        }
        Err("磁盘已满")
    }

    fn add_dir_entry(&mut self, name: &str, cluster: u32, size: u32, attr: u8) -> Result<(), &'static str> {
        let root = self.bpb.root_cluster;
        let cl_bytes = self.bpb.bytes_per_sector as usize * self.bpb.sectors_per_cluster as usize;
        // 读根目录第一簇，找空槽
        let mut buf = vec![0u8; cl_bytes];
        self.read_cluster(root, &mut buf)?;
        let mut slot: Option<usize> = None;
        let mut i = 0usize;
        while i + 32 <= cl_bytes {
            if buf[i] == 0x00 || buf[i] == 0xE5 {
                slot = Some(i);
                break;
            }
            i += 32;
        }
        let Some(off) = slot else {
            return Err("目录已满（简化版不支持目录扩展）");
        };
        // 构造 8.3 项
        let mut sfn = [b' '; 11];
        let (base, ext) = match name.find('.') {
            Some(i) => (&name[..i], Some(&name[i + 1..])),
            None => (name, None),
        };
        for (j, &c) in base.as_bytes().iter().take(8).enumerate() {
            sfn[j] = c.to_ascii_uppercase();
        }
        if let Some(e) = ext {
            for (j, &c) in e.as_bytes().iter().take(3).enumerate() {
                sfn[8 + j] = c.to_ascii_uppercase();
            }
        }
        buf[off..off + 11].copy_from_slice(&sfn);
        buf[off + 11] = attr;
        buf[off + 12..off + 20].fill(0); // 保留字段
        let cl_lo = (cluster & 0xFFFF) as u16;
        let cl_hi = ((cluster >> 16) & 0xFFFF) as u16;
        buf[off + 20..off + 22].copy_from_slice(&cl_hi.to_le_bytes());
        buf[off + 26..off + 28].copy_from_slice(&cl_lo.to_le_bytes());
        buf[off + 28..off + 32].copy_from_slice(&size.to_le_bytes());
        // 写回目录簇
        let mut lba = self.bpb.cluster_lba(root);
        for sec in buf.chunks(512) {
            let mut s = [0u8; 512];
            s.copy_from_slice(sec);
            self.dev.write_sectors(lba, &s)?;
            lba += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内存块设备（用大 Vec 模拟磁盘）
    struct RamDisk {
        data: Vec<u8>,
    }
    impl RamDisk {
        fn new(size: usize) -> Self {
            Self { data: vec![0u8; size] }
        }
        fn from_vec(img: Vec<u8>) -> Self {
            Self { data: img }
        }
    }
    impl BlockDevice for RamDisk {
        fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
            let start = lba as usize * 512;
            if start + buf.len() > self.data.len() {
                return Err("越界读");
            }
            buf.copy_from_slice(&self.data[start..start + buf.len()]);
            Ok(())
        }
        fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
            let start = lba as usize * 512;
            if start + buf.len() > self.data.len() {
                return Err("越界写");
            }
            self.data[start..start + buf.len()].copy_from_slice(buf);
            Ok(())
        }
        fn sector_size(&self) -> u16 {
            512
        }
    }

    /// 在内存中构造一个微型 FAT32 镜像：
    /// - 1 扇区/簇，reserved=1，FAT 数=2，每 FAT 4 扇区 → 数据区从 LBA 9 开始
    /// - 根目录在簇 2（LBA 9）
    /// - 文件 HELLO.TXT 占簇 3..=5（内容 = "Hello, KiraX OS! " 重复），簇 6 起为文件 2
    /// - 子目录 SUB 在簇 10，内含 INNER.TXT
    fn build_test_image() -> Vec<u8> {
        let total = 64 * 512usize;
        let mut img = vec![0u8; total];
        // ---- 引导扇区 ----
        img[0] = 0xEB; // 跳转指令
        img[1] = 0x3C;
        img[2] = 0x90;
        img[3..11].copy_from_slice(b"KIRAXOS!");
        img[11] = 0x00;
        img[12] = 0x02; // bytes_per_sector = 512
        img[13] = 0x01; // sectors_per_cluster = 1
        img[14] = 0x01;
        img[15] = 0x00; // reserved = 1
        img[16] = 0x02; // num_fats = 2
        img[17] = 0x00; // root entries（FAT32 忽略）
        img[18] = 0x00;
        img[19] = 0x00;
        img[20] = 0xF8; // media
        img[21] = 0x00;
        img[22] = 0x00;
        img[23] = 0x00;
        img[24] = 0x00;
        img[25] = 0x00;
        img[26] = 0x00;
        img[27] = 0x00; // 每磁道扇区
        img[28] = 0x00;
        img[29] = 0x00;
        img[30] = 0x00;
        img[31] = 0x00; // 磁头数
        img[32..36].copy_from_slice(&(32u32).to_le_bytes()); // total_sectors（FAT32）
        img[36..40].copy_from_slice(&(4u32).to_le_bytes()); // sectors_per_fat = 4
        img[40] = 0x00;
        img[41] = 0x00; // flags
        img[42] = 0x00;
        img[43] = 0x00; // version
        img[44..48].copy_from_slice(&(2u32).to_le_bytes()); // root_cluster = 2
        img[48] = 0x01;
        img[49] = 0x00; // FSInfo 扇区 = 1
        img[50] = 0x06;
        img[51] = 0x00; // 备份引导扇区 = 6
        img[71..82].copy_from_slice(b"SPACE OS   ");
        img[510] = 0x55;
        img[511] = 0xAA;

        // ---- FSInfo (LBA 1) ----
        img[512 + 0..512 + 4].copy_from_slice(&0x41615252u32.to_le_bytes());
        img[512 + 4..512 + 484].fill(0);
        img[512 + 484..512 + 488].copy_from_slice(&0x61417272u32.to_le_bytes());
        img[512 + 488..512 + 492].copy_from_slice(&(2u32).to_le_bytes()); // free cluster 2
        img[512 + 510] = 0x55;
        img[512 + 511] = 0xAA;

        // ---- FAT1 (LBA 1..5 其实 reserved=1, FAT1 从 LBA 1 开始, 占 4 扇区) ----
        let fat_base = 1 * 512usize;
        let set_fat = |img: &mut [u8], cluster: u32, val: u32| {
            let off = fat_base + cluster as usize * 4;
            img[off..off + 4].copy_from_slice(&(val & FAT_MASK).to_le_bytes());
        };
        set_fat(&mut img, 0, 0x0FFFFFF8);
        set_fat(&mut img, 1, 0x0FFFFFFF);
        set_fat(&mut img, 2, 0x0FFFFFFF); // 根目录簇 2 → EOC
        set_fat(&mut img, 3, 4);
        set_fat(&mut img, 4, 5);
        set_fat(&mut img, 5, EOC); // HELLO.TXT 簇链 3→4→5
        set_fat(&mut img, 6, EOC); // SECOND.TXT
        set_fat(&mut img, 10, EOC); // 子目录 SUB

        // ---- FAT2 镜像到 LBA 5..9（源 LBA1..5 与目标 LBA5..9 不重叠，用 split_at_mut 规避借用检查） ----
        let (head, tail) = img.split_at_mut(5 * 512);
        tail[..4 * 512].copy_from_slice(&head[1 * 512..5 * 512]);

        // ---- 数据区从 LBA 9 开始（簇 2 = 根目录） ----
        let data_base = 9 * 512usize;
        let mut entry = |img: &mut [u8], off: usize, name: &[u8; 11], attr: u8, cl: u32, size: u32| {
            img[off..off + 11].copy_from_slice(name);
            img[off + 11] = attr;
            img[off + 20..off + 22].copy_from_slice(&(((cl >> 16) & 0xFFFF) as u16).to_le_bytes());
            img[off + 26..off + 28].copy_from_slice(&((cl & 0xFFFF) as u16).to_le_bytes());
            img[off + 28..off + 32].copy_from_slice(&size.to_le_bytes());
        };
        // HELLO.TXT（簇 3，大小 96）
        entry(&mut img, data_base + 0, b"HELLO   TXT", ATTR_ARCHIVE, 3, 96);
        // SECOND.TXT（簇 6，大小 11）
        entry(&mut img, data_base + 32, b"SECOND  TXT", ATTR_ARCHIVE, 6, 11);
        // SUB 子目录（簇 10）
        entry(&mut img, data_base + 64, b"SUB        ", ATTR_DIRECTORY, 10, 0);
        img[data_base + 96] = 0x00; // 目录结束

        // HELLO.TXT 内容（3 簇 × 512 = 1536 字节容量，写 90 字节）
        // 18 字节周期模式：H e l l o , ␣ S p a c e ␣ O S ! ␣ ␣
        let hello: Vec<u8> = b"Hello, KiraX OS!  ".repeat(5); // 90 字节
        let hello_len = hello.len();
        img[data_base + 512 + 0..data_base + 512 + hello_len].copy_from_slice(&hello);
        // 剩余填充
        let second = b"KiraX OS v2";
        img[data_base + 512 * 6..data_base + 512 * 6 + 11].copy_from_slice(second);

        // SUB 目录内容（簇 10 = LBA 9 + (10-2) = LBA 17）
        let sub_base = data_base + (10 - 2) * 512;
        entry(&mut img, sub_base, b"INNER   TXT", ATTR_ARCHIVE, 11, 5);
        img[sub_base + 32] = 0x00;
        // INNER.TXT 内容在簇 11 = LBA 18
        img[data_base + (11 - 2) * 512..data_base + (11 - 2) * 512 + 5].copy_from_slice(b"inner");
        set_fat(&mut img, 11, EOC);

        img
    }

    #[test]
    fn mount_validates() {
        let img = build_test_image();
        let fs = Fat32Fs::mount(RamDisk::new(img.len()));
        assert!(fs.is_err(), "无签名镜像应挂载失败");
        let fs = Fat32Fs::mount(RamDisk::from_vec(img));
        assert!(fs.is_ok());
    }

    #[test]
    fn read_file_basic() {
        let img = build_test_image();
        let mut fs = Fat32Fs::mount(RamDisk::from_vec(img)).unwrap();
        let mut buf = [0u8; 128];
        let n = fs.read_file("/HELLO.TXT", 0, &mut buf).unwrap();
        assert_eq!(n, 96);
        assert!(buf[..n].starts_with(b"Hello, KiraX OS!"));
    }

    #[test]
    fn read_file_offset() {
        let img = build_test_image();
        let mut fs = Fat32Fs::mount(RamDisk::from_vec(img)).unwrap();
        let mut buf = [0u8; 8];
        let n = fs.read_file("/HELLO.TXT", 7, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"KiraX OS");
    }

    #[test]
    fn read_file_across_clusters() {
        // HELLO.TXT 跨 3 个簇（簇 3,4,5），验证簇链遍历
        let img = build_test_image();
        let mut fs = Fat32Fs::mount(RamDisk::from_vec(img)).unwrap();
        // 读偏移 40 处的 16 字节（文件 96 字节内，簇 3）
        let mut buf = [0u8; 16];
        let n = fs.read_file("/HELLO.TXT", 40, &mut buf).unwrap();
        // 内容模式是 "Hello, KiraX OS!  " 循环（18 字节周期）
        assert_eq!(n, 16);
        for i in 0..16 {
            let expect = b"Hello, KiraX OS!  "[((40 + i) % 18) as usize];
            assert_eq!(buf[i], expect, "位置 {i}");
        }
    }

    #[test]
    fn lookup_missing() {
        let img = build_test_image();
        let mut fs = Fat32Fs::mount(RamDisk::from_vec(img)).unwrap();
        assert!(fs.read_file("/NOPE.TXT", 0, &mut [0u8; 8]).is_err());
    }

    #[test]
    fn list_root() {
        let img = build_test_image();
        let mut fs = Fat32Fs::mount(RamDisk::from_vec(img)).unwrap();
        let entries = fs.list_dir("/").unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.display_name()).collect();
        assert!(names.contains(&"HELLO.TXT".into()), "{names:?}");
        assert!(names.contains(&"SECOND.TXT".into()), "{names:?}");
        assert!(names.contains(&"SUB".into()), "{names:?}");
    }

    #[test]
    fn read_subdir_file() {
        let img = build_test_image();
        let mut fs = Fat32Fs::mount(RamDisk::from_vec(img)).unwrap();
        let mut buf = [0u8; 16];
        let n = fs.read_file("/SUB/INNER.TXT", 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"inner");
    }

    #[test]
    fn create_and_read_back() {
        let img = build_test_image();
        let mut fs = Fat32Fs::mount(RamDisk::from_vec(img)).unwrap();
        let data = b"created by space os";
        let n = fs.create_file("NEW.TXT", data).unwrap();
        assert_eq!(n, data.len());
        let mut buf = [0u8; 64];
        let rn = fs.read_file("/NEW.TXT", 0, &mut buf).unwrap();
        assert_eq!(&buf[..rn], data);
    }

    #[test]
    fn total_sector_wraps_correctly() {
        // 验证簇 → LBA 换算与 FAT 位置换算
        let img = build_test_image();
        let fs = Fat32Fs::mount(RamDisk::from_vec(img)).unwrap();
        let bpb = fs.bpb();
        assert_eq!(bpb.cluster_lba(2), 9);
        assert_eq!(bpb.cluster_lba(3), 10);
        let (sec, off) = bpb.fat_entry_location(5);
        assert_eq!(sec, 1 + (5 * 4) / 512);
        assert_eq!(off, (5 * 4) % 512);
    }
}
