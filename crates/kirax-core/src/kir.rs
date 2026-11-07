//! # .kir 应用二进制格式（SKIR，KIR2 线格式）
//!
//! 与 tools/kirax-cli、tools/kirpm、scripts/pack-apkcompat.py、
//! kirax-studio/src-tauri/kirpack.rs 产出的真实产物一致（kernel 加载器亦按此格式校验）。
//! 全部小端，32 字节定长头：
//!
//! ```text
//! 偏移   大小  字段
//! 0x00   4    魔数 "SKIR" (0x52494B53)
//! 0x04   4    头部版本 (u32 LE，当前 1)
//! 0x08   4    代码段偏移（相对文件起始）
//! 0x0C   4    代码段大小
//! 0x10   4    图标偏移（PNG 或 BGRA 位图原始字节）
//! 0x14   4    图标大小
//! 0x18   4    manifest 偏移（JSON 文本）
//! 0x1C   4    manifest 大小
//! 0x20   ...  代码段 → 图标 → manifest（顺序拼接，无对齐填充）
//! ```
//!
//! 说明：历史上本模块曾实现过另一套 KIR1（魔数 0x4B495231 + 128B 头 + key=value
//! manifest），与生态实际产物不符，已废弃。当前为位置相关的平坦二进制装载格式。

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

/// .kir 魔数 "SKIR"（小端 u32）
pub const KIR_MAGIC: u32 = 0x5249_4B53;
/// 头部大小（32 字节定长）
pub const KIR_HEADER_SIZE: usize = 32;
/// 当前头部版本
pub const KIR_VERSION: u32 = 1;

/// 兼容标志位（SKIR 不再编码这些位，保留字段仅为与旧调用方字段对齐，恒 0）。
pub const FLAG_NEEDS_GLASS: u16 = 1 << 0;
pub const FLAG_FULLSCREEN: u16 = 1 << 1;

/// .kir 头部（解析后的视图）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct KirHeader {
    pub magic: u32,
    pub version: u32,
    pub code_offset: u32,
    pub code_size: u32,
    pub icon_offset: u32,
    pub icon_size: u32,
    pub manifest_offset: u32,
    pub manifest_size: u32,
    /// BSS 大小（SKIR 未编码，恒 0；kernel 日志/未来装载用）
    pub bss_size: u32,
    /// 栈大小（SKIR 未编码，恒 0）
    pub stack_size: u32,
    /// 标志位（SKIR 未编码，恒 0）
    pub flags: u16,
}

impl KirHeader {
    /// 校验并解析头部；失败返回可读错误串。
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < KIR_HEADER_SIZE {
            return Err("文件过短，无法容纳 32 字节头");
        }
        let magic = u32_at(bytes, 0);
        if magic != KIR_MAGIC {
            return Err("魔数错误：不是 .kir(SKIR) 文件");
        }
        let version = u32_at(bytes, 4);
        if version != KIR_VERSION {
            return Err("版本不兼容");
        }
        let code_offset = u32_at(bytes, 8);
        let code_size = u32_at(bytes, 12);
        let icon_offset = u32_at(bytes, 16);
        let icon_size = u32_at(bytes, 20);
        let manifest_offset = u32_at(bytes, 24);
        let manifest_size = u32_at(bytes, 28);

        // 三段边界检查（不要求相邻，但不得越界）
        let end = |o: u32, s: u32| -> Result<(), &'static str> {
            let e = o as usize + s as usize;
            if e > bytes.len() {
                return Err("段越界：文件被截断");
            }
            Ok(())
        };
        end(code_offset, code_size)?;
        end(icon_offset, icon_size)?;
        end(manifest_offset, manifest_size)?;

        Ok(Self {
            magic,
            version,
            code_offset,
            code_size,
            icon_offset,
            icon_size,
            manifest_offset,
            manifest_size,
            bss_size: 0,
            stack_size: 0,
            flags: 0,
        })
    }

    pub fn is_valid(&self) -> bool {
        self.magic == KIR_MAGIC && self.version == KIR_VERSION
    }
}

/// 应用 manifest（JSON 文本解析后的关键字段；无第三方 JSON 库，做最小提取）
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub name: String,
    pub author: String,
    pub version: String,
    /// 图标宽（JSON 无此字段时为 0）
    pub icon_w: u16,
    pub icon_h: u16,
    pub glass: bool,
}

impl Manifest {
    /// 解析 manifest 文本（容忍 JSON `{"name":".."}` 与旧式 `key=value`）。
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        let text = core::str::from_utf8(bytes).map_err(|_| "manifest 不是 UTF-8")?;
        let mut m = Manifest::default();
        m.name = json_string_field(text, "name");
        m.author = json_string_field(text, "author");
        m.version = json_string_field(text, "version");
        Ok(m)
    }

    /// 有序序列化为 JSON manifest 文本（打包用）。
    pub fn to_bytes(&self) -> Vec<u8> {
        alloc::format!(
            "{{\"name\":\"{}\",\"author\":\"{}\",\"version\":\"{}\"}}",
            self.name, self.author, self.version
        )
        .into_bytes()
    }
}

/// 最小提取：在 `{"name":"x"}`（容忍空白）里取 key 对应的字符串值；找不到返回空串。
fn json_string_field(text: &str, key: &str) -> String {
    let needle = alloc::format!("\"{}\"", key);
    let Some(pos) = text.find(&needle) else {
        return String::new();
    };
    let rest = &text[pos + needle.len()..];
    let Some(colon) = rest.find(':') else {
        return String::new();
    };
    let after = &rest[colon + 1..];
    let Some(open) = after.find('"') else {
        return String::new();
    };
    let val = &after[open + 1..];
    let Some(close) = val.find('"') else {
        return String::new();
    };
    String::from(&val[..close])
}

/// 打包：把代码/图标/manifest 按 SKIR 布局组装成一个 .kir 文件。
///
/// 参数保持与历史调用方（kernel selftest）一致：`icon_w/icon_h/bss_size/stack_size/
/// glass/fullscreen` 在 SKIR 线格式中不再编码，仅接收以兼容旧签名；manifest 由
/// name/author/version 自动生成 JSON。
pub fn pack(
    code: &[u8],
    icon: &[u8],
    _icon_w: u16,
    _icon_h: u16,
    name: &str,
    author: &str,
    version: &str,
    _bss_size: u32,
    _stack_size: u32,
    _glass: bool,
    _fullscreen: bool,
) -> Vec<u8> {
    let manifest = Manifest {
        name: name.into(),
        author: author.into(),
        version: version.into(),
        ..Default::default()
    };
    let manifest_bytes = manifest.to_bytes();

    let code_off: u32 = KIR_HEADER_SIZE as u32;
    let icon_off: u32 = code_off + code.len() as u32;
    let man_off: u32 = icon_off + icon.len() as u32;

    let mut out = Vec::with_capacity(KIR_HEADER_SIZE + code.len() + icon.len() + manifest_bytes.len());
    out.extend_from_slice(b"SKIR");
    out.extend_from_slice(&KIR_VERSION.to_le_bytes());
    out.extend_from_slice(&code_off.to_le_bytes());
    out.extend_from_slice(&(code.len() as u32).to_le_bytes());
    out.extend_from_slice(&icon_off.to_le_bytes());
    out.extend_from_slice(&(icon.len() as u32).to_le_bytes());
    out.extend_from_slice(&man_off.to_le_bytes());
    out.extend_from_slice(&(manifest_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(code);
    out.extend_from_slice(icon);
    out.extend_from_slice(&manifest_bytes);
    out
}

#[inline]
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// 从完整文件提取代码段
pub fn extract_code(bytes: &[u8]) -> Result<&[u8], &'static str> {
    let h = KirHeader::parse(bytes)?;
    let s = h.code_offset as usize;
    let e = s + h.code_size as usize;
    Ok(&bytes[s..e])
}

/// 从完整文件提取图标原始字节
pub fn extract_icon(bytes: &[u8]) -> Result<&[u8], &'static str> {
    let h = KirHeader::parse(bytes)?;
    let s = h.icon_offset as usize;
    let e = s + h.icon_size as usize;
    Ok(&bytes[s..e])
}

/// 从完整文件提取 manifest 文本
pub fn extract_manifest(bytes: &[u8]) -> Result<&[u8], &'static str> {
    let h = KirHeader::parse(bytes)?;
    let s = h.manifest_offset as usize;
    let e = s + h.manifest_size as usize;
    Ok(&bytes[s..e])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 仓库内真实打包产物（由 scripts/pack-apkcompat.py 生成）。
    /// 相对 crate 根（crates/kirax-core）解析。
    const REAL_KIR: &str = "../../apps/apkcompat/apkcompat.kir";

    fn sample_code() -> Vec<u8> {
        (0u8..64).collect()
    }

    fn sample_icon() -> Vec<u8> {
        vec![0x11u8; 16 * 16 * 4]
    }

    #[test]
    fn pack_parse_roundtrip() {
        let kir = pack(
            &sample_code(),
            &sample_icon(),
            16,
            16,
            "About KiraX OS",
            "Space Team",
            "1.0.0",
            4096,
            8192,
            true,
            false,
        );
        let h = KirHeader::parse(&kir).unwrap();
        assert!(h.is_valid());
        assert_eq!(h.code_size, 64);
        assert_eq!(h.icon_size, sample_icon().len() as u32);
        // 段偏移顺序：头 32 → code → icon → manifest
        assert_eq!(h.code_offset, 32);
        assert_eq!(h.icon_offset, 32 + 64);
        // 代码原样取回
        assert_eq!(extract_code(&kir).unwrap(), sample_code());
        assert_eq!(extract_icon(&kir).unwrap(), sample_icon().as_slice());
        // manifest 是我们生成的 JSON
        let man = extract_manifest(&kir).unwrap();
        let m = Manifest::parse(man).unwrap();
        assert_eq!(m.name, "About KiraX OS");
        assert_eq!(m.version, "1.0.0");
    }

    #[test]
    fn reject_bad_magic() {
        let mut kir = pack(&sample_code(), &sample_icon(), 8, 8, "x", "y", "1", 0, 0, false, false);
        kir[0] = 0x00;
        assert!(KirHeader::parse(&kir).is_err());
    }

    #[test]
    fn reject_truncated() {
        let kir = pack(&sample_code(), &sample_icon(), 8, 8, "x", "y", "1", 0, 0, false, false);
        assert!(KirHeader::parse(&kir[..kir.len() - 10]).is_err());
        assert!(KirHeader::parse(&kir[..20]).is_err());
    }

    #[test]
    fn reject_wrong_version() {
        let mut kir = pack(&sample_code(), &sample_icon(), 8, 8, "x", "y", "1", 0, 0, false, false);
        kir[4..8].copy_from_slice(&99u32.to_le_bytes());
        assert!(KirHeader::parse(&kir).is_err());
    }

    #[test]
    fn empty_code_ok() {
        let kir = pack(&[], &[], 0, 0, "e", "a", "1", 0, 0, false, false);
        let h = KirHeader::parse(&kir).unwrap();
        assert_eq!(h.code_size, 0);
        assert_eq!(extract_code(&kir).unwrap().len(), 0);
    }

    // ---- 真实产物解析验证 ----

    #[test]
    fn real_apkcompat_header() {
        let bytes = std::fs::read(REAL_KIR).expect("读取真实 apkcompat.kir");
        let h = KirHeader::parse(&bytes).expect("真实产物应通过校验");
        assert_eq!(h.magic, KIR_MAGIC);
        assert_eq!(h.version, 1);
        assert!(h.is_valid());
        // 与 pack-apkcompat.py 自验证值一致：code=162566 icon=775 manifest=1013
        assert_eq!(h.code_offset, 32);
        assert_eq!(h.code_size, 162566);
        assert_eq!(h.icon_offset, 32 + 162566);
        assert_eq!(h.icon_size, 775);
        assert_eq!(h.manifest_offset, 32 + 162566 + 775);
        assert_eq!(h.manifest_size, 1013);
    }

    #[test]
    fn real_apkcompat_extract_icon_is_png() {
        let bytes = std::fs::read(REAL_KIR).unwrap();
        let icon = extract_icon(&bytes).unwrap();
        assert_eq!(&icon[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn real_apkcompat_manifest_name() {
        let bytes = std::fs::read(REAL_KIR).unwrap();
        let man = extract_manifest(&bytes).unwrap();
        let m = Manifest::parse(man).unwrap();
        assert_eq!(m.name, "apkcompat");
        assert_eq!(m.version, "1.0.0");
    }

    #[test]
    fn real_apkcompat_rejects_garbage() {
        let bytes = std::fs::read(REAL_KIR).unwrap();
        // 截断到只剩头，manifest 段越界 → 拒绝
        assert!(KirHeader::parse(&bytes[..33]).is_err());
        // 篡改魔数 → 拒绝
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(KirHeader::parse(&bad).is_err());
    }
}
