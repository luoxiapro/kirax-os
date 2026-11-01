//! 小端读取器 + uleb128/sleb128 + MUTF-8（Android DEX 专用，纯手写无依赖）。

use alloc::string::String;
use alloc::vec::Vec;

/// 小端视图读取器（不持有数据）
pub struct Reader<'a> {
    pub d: &'a [u8],
    pub p: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadErr {
    Oob,
    BadUleb,
    BadUtf8,
}

pub type R<T> = core::result::Result<T, ReadErr>;

impl<'a> Reader<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Reader { d, p: 0 }
    }

    pub fn at(d: &'a [u8], p: usize) -> Self {
        Reader { d, p }
    }

    pub fn len(&self) -> usize {
        self.d.len()
    }

    pub fn rest(&self) -> usize {
        self.d.len().saturating_sub(self.p)
    }

    pub fn seek(&mut self, p: usize) -> R<()> {
        if p > self.d.len() {
            return Err(ReadErr::Oob);
        }
        self.p = p;
        Ok(())
    }

    pub fn u8(&mut self) -> R<u8> {
        if self.p + 1 > self.d.len() {
            return Err(ReadErr::Oob);
        }
        let v = self.d[self.p];
        self.p += 1;
        Ok(v)
    }

    pub fn u16(&mut self) -> R<u16> {
        if self.p + 2 > self.d.len() {
            return Err(ReadErr::Oob);
        }
        let v = u16::from_le_bytes([self.d[self.p], self.d[self.p + 1]]);
        self.p += 2;
        Ok(v)
    }

    pub fn u32(&mut self) -> R<u32> {
        if self.p + 4 > self.d.len() {
            return Err(ReadErr::Oob);
        }
        let v = u32::from_le_bytes([
            self.d[self.p],
            self.d[self.p + 1],
            self.d[self.p + 2],
            self.d[self.p + 3],
        ]);
        self.p += 4;
        Ok(v)
    }

    pub fn i32(&mut self) -> R<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> R<u64> {
        if self.p + 8 > self.d.len() {
            return Err(ReadErr::Oob);
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.d[self.p..self.p + 8]);
        self.p += 8;
        Ok(u64::from_le_bytes(b))
    }

    pub fn bytes(&mut self, n: usize) -> R<&'a [u8]> {
        if self.p + n > self.d.len() {
            return Err(ReadErr::Oob);
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }

    /// uleb128（Android DEX 编码）
    pub fn uleb(&mut self) -> R<u32> {
        let mut result: u32 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.u8()?;
            if shift >= 32 && (b & 0x7f) != 0 {
                return Err(ReadErr::BadUleb);
            }
            result |= ((b & 0x7f) as u32) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        Ok(result)
    }

    /// sleb128
    pub fn sleb(&mut self) -> R<i32> {
        let mut result: i32 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.u8()?;
            result |= ((b & 0x7f) as i32) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 32 && (b & 0x40) != 0 {
                    result |= -1i32 << shift;
                }
                break;
            }
        }
        Ok(result)
    }

    /// MUTF-8 字符串（DEX：无 NUL 终止、modified UTF-8）
    pub fn mutf8(&mut self) -> R<String> {
        let mut out = Vec::new();
        loop {
            let b = self.u8()?;
            if b == 0 {
                break;
            }
            if b < 0x80 {
                out.push(b as char);
            } else if b >> 5 == 0b110 {
                let b2 = self.u8()?;
                let cp = (((b & 0x1f) as u32) << 6) | ((b2 & 0x3f) as u32);
                out.push(char_from_cp(cp)?);
            } else if b >> 4 == 0b1110 {
                let b2 = self.u8()?;
                let b3 = self.u8()?;
                let cp = (((b & 0x0f) as u32) << 12)
                    | (((b2 & 0x3f) as u32) << 6)
                    | ((b3 & 0x3f) as u32);
                out.push(char_from_cp(cp)?);
            } else if b >> 3 == 0b11110 {
                let b2 = self.u8()?;
                let b3 = self.u8()?;
                let b4 = self.u8()?;
                let cp = (((b & 0x07) as u32) << 18)
                    | (((b2 & 0x3f) as u32) << 12)
                    | (((b3 & 0x3f) as u32) << 6)
                    | ((b4 & 0x3f) as u32);
                out.push(char_from_cp(cp)?);
            } else if b == 0xc0 && self.d.get(self.p) == Some(&0x80) {
                // 修正 UTF-8 表示 NUL（0xC0 0x80）
                self.p += 1;
                out.push('\0');
            } else {
                return Err(ReadErr::BadUtf8);
            }
        }
        Ok(out.into_iter().collect())
    }
}

fn char_from_cp(cp: u32) -> R<char> {
    char::from_u32(cp).ok_or(ReadErr::BadUtf8)
}

/// 读取一个 u16[]（DEX CodeItem.insns），返回字节切片（长度=2*n）
pub fn insns_slice<'a>(d: &'a [u8], off: usize, n: usize) -> R<&'a [u8]> {
    let end = off.checked_add(n * 2).ok_or(ReadErr::Oob)?;
    if end > d.len() {
        return Err(ReadErr::Oob);
    }
    Ok(&d[off..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_bytes() {
        let d = [0x34, 0x12, 0x00, 0x00];
        let mut r = Reader::new(&d);
        assert_eq!(r.u16().unwrap(), 0x1234);
        r.seek(0).unwrap();
        assert_eq!(r.u32().unwrap(), 0x1234);
    }

    #[test]
    fn uleb_common() {
        let d = [0xE5, 0x8E, 0x26]; // 0x4_0445? 手工验证: 0x65 | (0x0E<<7) | (0x26<<14)
        let mut r = Reader::new(&d);
        assert_eq!(r.uleb().unwrap(), 0x65 | (0x0E << 7) | (0x26 << 14));
    }

    #[test]
    fn mutf8_ascii_and_ce() {
        let d = b"abc\0";
        let mut r = Reader::new(d);
        assert_eq!(r.mutf8().unwrap(), "abc");

        // "中文" 的 UTF-8 字节 + NUL
        let mut d2 = Vec::new();
        d2.extend_from_slice("中文".as_bytes());
        d2.push(0);
        let mut r2 = Reader::new(&d2);
        assert_eq!(r2.mutf8().unwrap(), "中文");

        // 修正 NUL: C0 80
        let d3 = [0x61, 0xC0, 0x80, 0x62, 0x00];
        let mut r3 = Reader::new(&d3);
        assert_eq!(r3.mutf8().unwrap(), "a\0b");
    }
}
