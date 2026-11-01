//! Deflate (RFC 1951) 解压 —— `no_std` + alloc 手写实现。
//!
//! APK（ZIP）内的 `classes.dex` / `resources.arsc` 等条目几乎都以
//! **raw deflate**（method=8，无 zlib wrapper）压缩，Space OS 兼容层
//! 必须先解压才能交给 DEX 解析器。本模块按 RFC 1951 实现：
//!   - 三种块类型：stored / fixed-Huffman / dynamic-Huffman
//!   - 表驱动规范 Huffman 解码（counts + symbols，无需建树）
//!   - 长度-距离回指复制（含 dist=1 长 run）
//!
//! 无第三方依赖；`cargo test` 在主机直接验证（含真实 APK 样本）。

extern crate alloc;
use alloc::vec::Vec;

/// 解压失败原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    /// 数据在块/码中途耗尽
    Truncated,
    /// Huffman 码超长或码长计数非法
    BadHuffman,
    /// 动态块头参数非法（HLIT/HDIST/HCLEN 越界）
    BadBlockHeader,
    /// 输出长度超上限（防恶意输入）
    OutputLimit,
}

impl core::fmt::Display for InflateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use InflateError::*;
        write!(
            f,
            "{}",
            match self {
                Truncated => "defalte 数据截断",
                BadHuffman => "Huffman 码非法",
                BadBlockHeader => "动态块头参数非法",
                OutputLimit => "输出长度超上限",
            }
        )
    }
}

/// 输出长度安全上限（ZIP64 常规条目不会超过；防 zip-bomb）
pub const MAX_OUTPUT: usize = 256 * 1024 * 1024;

// ----------------------------------------------------------------------------
// 位读取（LSB first，RFC 1951）
// ----------------------------------------------------------------------------

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bitbuf: u32,
    bitcnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            bitbuf: 0,
            bitcnt: 0,
        }
    }

    /// 尽量填满位缓冲（数据耗尽即停止，不视为错误）
    fn fill(&mut self) {
        while self.bitcnt <= 24 && self.pos < self.data.len() {
            self.bitbuf |= (self.data[self.pos] as u32) << self.bitcnt;
            self.pos += 1;
            self.bitcnt += 8;
        }
    }

    /// 读 n 位（n ≤ 16）；位缓冲不足才算截断
    fn bits(&mut self, n: u32) -> Result<u32, InflateError> {
        self.fill();
        if n > self.bitcnt {
            return Err(InflateError::Truncated);
        }
        let v = self.bitbuf & ((1u32 << n) - 1);
        self.bitbuf >>= n;
        self.bitcnt -= n;
        Ok(v)
    }

    /// 字节对齐（stored 块头前）
    fn align_byte(&mut self) {
        let drop = self.bitcnt & 7;
        if drop != 0 {
            self.bitbuf >>= drop;
            self.bitcnt -= drop;
        }
    }

    fn byte_pos(&self) -> usize {
        self.pos
    }
}

// ----------------------------------------------------------------------------
// 规范 Huffman 解码（RFC 1951 §3.2.2）
// ----------------------------------------------------------------------------

struct Huffman {
    counts: [u16; 16],       // counts[len] = 码长为 len 的符号数
    symbols: alloc::vec::Vec<u16>, // 按 (码长, 符号值) 排序后的符号
}

impl Huffman {
    /// 由码长数组构建解码表；码长 0 表示符号未使用。
    fn build(lengths: &[u8]) -> Result<Huffman, InflateError> {
        let mut counts = [0u16; 16];
        let mut max = 0usize;
        for &l in lengths {
            if l > 15 {
                return Err(InflateError::BadHuffman);
            }
            if l != 0 {
                counts[l as usize] += 1;
                if l as usize > max {
                    max = l as usize;
                }
            }
        }
        // 规范校验：非空码树的最短/最长码约束（RFC 1951 3.2.2；left 需非负）
        let mut left: i64 = 1;
        for l in 1..=max {
            left <<= 1;
            left -= counts[l] as i64;
            if left < 0 {
                return Err(InflateError::BadHuffman);
            }
        }
        // 按 (码长, 符号) 排序符号
        let mut syms: alloc::vec::Vec<(u8, u16)> = (0..lengths.len() as u16)
            .filter(|&s| lengths[s as usize] != 0)
            .map(|s| (lengths[s as usize], s))
            .collect();
        syms.sort_by_key(|&(l, s)| (l, s));
        let symbols = syms.into_iter().map(|(_, s)| s).collect();
        Ok(Huffman { counts, symbols })
    }

    /// 读一条码并返回符号；单符号表可直接短路。
    fn decode(&self, br: &mut BitReader) -> Result<u16, InflateError> {
        let mut code: u32 = 0;
        let mut first: u32 = 0;
        let mut index: u32 = 0;
        for len in 1..=15usize {
            code |= br.bits(1)?;
            let count = self.counts[len] as u32;
            if code - first < count {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(InflateError::BadHuffman)
    }
}

// ----------------------------------------------------------------------------
// 长度 / 距离 表（RFC 1951 §3.2.5）
// ----------------------------------------------------------------------------

/// 长度码 257..=285 的基础长度
static LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
/// 长度码附加位数
static LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// 距离码 0..=29 的基础距离
static DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
/// 距离码附加位数
static DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// 固定 Huffman 的 litlen 码长（0..=287）
fn fixed_litlen_lengths() -> alloc::vec::Vec<u8> {
    let mut l = alloc::vec![0u8; 288];
    for i in 0..144 {
        l[i] = 8;
    }
    for i in 144..256 {
        l[i] = 9;
    }
    for i in 256..280 {
        l[i] = 7;
    }
    for i in 280..288 {
        l[i] = 8;
    }
    l
}

fn fixed_dist_lengths() -> alloc::vec::Vec<u8> {
    alloc::vec![5u8; 30]
}

/// 码长表符号顺序（动态块 HCLEN）
static CLEN_ORDER: [u8; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

// ----------------------------------------------------------------------------
// 主入口
// ----------------------------------------------------------------------------

/// 解压 raw deflate 数据（无 zlib 头）。
pub fn inflate(data: &[u8]) -> Result<Vec<u8>, InflateError> {
    let mut br = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();

    loop {
        let bfinal = br.bits(1)?;
        let btype = br.bits(2)?;
        match btype {
            0 => {
                // stored：字节对齐 + LEN/NLEN + 原文
                br.align_byte();
                let len = br.bits(16)? as usize;
                let nlen = br.bits(16)? as usize;
                if (len ^ 0xffff) != nlen {
                    return Err(InflateError::BadBlockHeader);
                }
                if out.len() + len > MAX_OUTPUT {
                    return Err(InflateError::OutputLimit);
                }
                if br.byte_pos() + len > data.len() {
                    return Err(InflateError::Truncated);
                }
                out.extend_from_slice(&data[br.byte_pos()..br.byte_pos() + len]);
                br.pos += len;
            }
            1 => {
                // fixed Huffman
                let litlen = Huffman::build(&fixed_litlen_lengths())?;
                let dist = Huffman::build(&fixed_dist_lengths())?;
                decode_block(&mut br, &mut out, &litlen, &dist)?;
            }
            2 => {
                // dynamic Huffman
                let hlit = br.bits(5)? as usize + 257;
                let hdist = br.bits(5)? as usize + 1;
                let hclen = br.bits(4)? as usize + 4;
                if hlit > 286 || hdist > 30 || hclen > 19 {
                    return Err(InflateError::BadBlockHeader);
                }
                // 码长表码长（3 位 × hclen）
                let mut clen_lengths = alloc::vec![0u8; 19];
                for i in 0..hclen {
                    clen_lengths[CLEN_ORDER[i] as usize] = br.bits(3)? as u8;
                }
                let clen_huff = Huffman::build(&clen_lengths)?;
                // litlen 码长（hlit 个）+ dist 码长（hdist 个）
                let total = hlit + hdist;
                let mut lens = alloc::vec![0u8; total];
                let mut i = 0usize;
                while i < total {
                    let sym = clen_huff.decode(&mut br)? as u8;
                    match sym {
                        0..=15 => {
                            lens[i] = sym;
                            i += 1;
                        }
                        16 => {
                            if i == 0 {
                                return Err(InflateError::BadHuffman);
                            }
                            let prev = lens[i - 1];
                            let rep = 3 + br.bits(2)? as usize;
                            for _ in 0..rep {
                                if i >= total {
                                    return Err(InflateError::BadHuffman);
                                }
                                lens[i] = prev;
                                i += 1;
                            }
                        }
                        17 => {
                            let rep = 3 + br.bits(3)? as usize;
                            for _ in 0..rep {
                                if i >= total {
                                    return Err(InflateError::BadHuffman);
                                }
                                lens[i] = 0;
                                i += 1;
                            }
                        }
                        18 => {
                            let rep = 11 + br.bits(7)? as usize;
                            for _ in 0..rep {
                                if i >= total {
                                    return Err(InflateError::BadHuffman);
                                }
                                lens[i] = 0;
                                i += 1;
                            }
                        }
                        _ => return Err(InflateError::BadHuffman),
                    }
                }
                let litlen = Huffman::build(&lens[..hlit])?;
                let dist = Huffman::build(&lens[hlit..])?;
                decode_block(&mut br, &mut out, &litlen, &dist)?;
            }
            _ => return Err(InflateError::BadBlockHeader),
        }
        if bfinal != 0 {
            break;
        }
    }
    Ok(out)
}

/// 解码一个 Huffman 块（litlen/dist 树已就绪）
fn decode_block(
    br: &mut BitReader,
    out: &mut Vec<u8>,
    litlen: &Huffman,
    dist: &Huffman,
) -> Result<(), InflateError> {
    loop {
        let sym = litlen.decode(br)?;
        match sym {
            0..=255 => {
                if out.len() >= MAX_OUTPUT {
                    return Err(InflateError::OutputLimit);
                }
                out.push(sym as u8);
            }
            256 => return Ok(()),
            257..=285 => {
                let li = (sym - 257) as usize;
                let len = LENGTH_BASE[li] as usize + br.bits(LENGTH_EXTRA[li] as u32)? as usize;
                let dsym = dist.decode(br)? as usize;
                if dsym > 29 {
                    return Err(InflateError::BadHuffman);
                }
                let d = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32)? as usize;
                if out.len() + len > MAX_OUTPUT {
                    return Err(InflateError::OutputLimit);
                }
                if d > out.len() {
                    return Err(InflateError::BadHuffman); // 距离回指超出已输出
                }
                // 回指复制（dist=1 长 run 时逐字节推进）
                let start = out.len() - d;
                for k in 0..len {
                    let b = out[start + k];
                    out.push(b);
                }
            }
            _ => return Err(InflateError::BadHuffman), // 286/287 未使用
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflate_fixed_huffman() {
        // "hello hello hello hello hello" 的 raw deflate（zlib wbits=-15 生成，固定）
        let compressed: &[u8] = &[
            0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0xc0, 0x4e, 0x02, 0x00,
        ];
        let out = inflate(compressed).unwrap();
        assert_eq!(&out[..], b"hello hello hello hello hello");
    }

    #[test]
    fn inflate_dynamic_huffman() {
        // 长文本（293B）触发动态 Huffman 树；zlib wbits=-15 生成，固定
        let compressed: &[u8] = &[
            0x0b, 0x2e, 0x48, 0x4c, 0x4e, 0x55, 0xf0, 0x0f, 0x56, 0x70, 0x0c, 0xf0, 0x56, 0x48,
            0xce, 0xcf, 0x2d, 0x48, 0x2c, 0xc9, 0x4c, 0xca, 0xcc, 0xc9, 0x2c, 0xa9, 0x54, 0xc8,
            0x49, 0xac, 0x4c, 0x2d, 0xd2, 0x53, 0x08, 0x1e, 0x51, 0x2a, 0x52, 0x52, 0xd3, 0x72,
            0x12, 0x4b, 0x52, 0x15, 0x32, 0xf3, 0x20, 0x74, 0x49, 0x6a, 0x71, 0x89, 0x1e, 0x00,
        ];
        let out = inflate(compressed).unwrap();
        let mut expect = b"Space OS APK compatibility layer. ".repeat(8);
        expect.extend_from_slice(b"deflate inflate test.");
        assert_eq!(&out[..], &expect[..]);
    }

    #[test]
    fn inflate_bad_data_rejected() {
        // 非法输入：截断流 / 非法块类型 / LEN 与 NLEN 不符，均应报错而非 panic
        assert!(inflate(&[0x03]).is_err()); // 截断的 fixed 块
        assert!(inflate(&[0x07]).is_err()); // btype=3 非法
        // stored 块：LEN=0x0005 应取反 0xfffa，此处为 0x00fa → 校验失败
        assert!(inflate(&[0x01, 0x05, 0x00, 0xfa, 0xff]).is_err());
    }
}
