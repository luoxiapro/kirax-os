//! .kir 打包器 — 纯 Rust，无任何系统依赖，跨平台可单测。
//!
//! .kir 二进制格式（与 crates/kirax-core/src/kir.rs 解析端一致）：
//! ```text
//! 偏移   大小   字段
//! 0      4      魔数 "SKIR" (0x52494B53)
//! 4      4      头部版本 (u32 LE)
//! 8      4      代码段偏移 (u32 LE)
//! 12     4      代码段大小 (u32 LE)
//! 16     4      图标偏移 (u32 LE)
//! 20     4      图标大小 (u32 LE)
//! 24     4      manifest 偏移 (u32 LE)
//! 28     4      manifest 大小 (u32 LE)
//! 32     ...    代码段
//! ...          图标
//! ...          manifest
//! ```

/// 打包 .kir 二进制
pub fn pack(code: &[u8], icon: &[u8], manifest: &[u8]) -> Vec<u8> {
    const HDR: usize = 32;
    let mut out = Vec::with_capacity(HDR + code.len() + icon.len() + manifest.len());
    out.extend_from_slice(b"SKIR");
    out.extend_from_slice(&1u32.to_le_bytes()); // 版本

    let code_off = HDR as u32;
    let icon_off = (HDR + code.len()) as u32;
    let man_off = (HDR + code.len() + icon.len()) as u32;

    out.extend_from_slice(&code_off.to_le_bytes());
    out.extend_from_slice(&(code.len() as u32).to_le_bytes());
    out.extend_from_slice(&icon_off.to_le_bytes());
    out.extend_from_slice(&(icon.len() as u32).to_le_bytes());
    out.extend_from_slice(&man_off.to_le_bytes());
    out.extend_from_slice(&(manifest.len() as u32).to_le_bytes());

    out.extend_from_slice(code);
    out.extend_from_slice(icon);
    out.extend_from_slice(manifest);
    out
}

/// 解包 .kir（供校验/预览）
pub fn unpack(kir: &[u8]) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), &'static str> {
    if kir.len() < 32 || &kir[0..4] != b"SKIR" {
        return Err("无效 .kir：魔数或长度错误");
    }
    let code_off = u32::from_le_bytes(kir[8..12].try_into().unwrap()) as usize;
    let code_len = u32::from_le_bytes(kir[12..16].try_into().unwrap()) as usize;
    let icon_off = u32::from_le_bytes(kir[16..20].try_into().unwrap()) as usize;
    let icon_len = u32::from_le_bytes(kir[20..24].try_into().unwrap()) as usize;
    let man_off = u32::from_le_bytes(kir[24..28].try_into().unwrap()) as usize;
    let man_len = u32::from_le_bytes(kir[28..32].try_into().unwrap()) as usize;
    if code_off + code_len + icon_len + man_len > kir.len() {
        return Err("无效 .kir：段越界");
    }
    let code = kir[code_off..code_off + code_len].to_vec();
    let icon = kir[icon_off..icon_off + icon_len].to_vec();
    let man = kir[man_off..man_off + man_len].to_vec();
    Ok((code, icon, man))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_roundtrip() {
        let code = vec![0x00, 0x01, 0x02, 0xFF, 0xFE];
        let icon = b"PNGICON".to_vec();
        let man = br#"{"name":"demo"}"#.to_vec();
        let kir = pack(&code, &icon, &man);
        assert_eq!(&kir[0..4], b"SKIR");
        let (c, i, m) = unpack(&kir).unwrap();
        assert_eq!(c, code);
        assert_eq!(i, icon);
        assert_eq!(m, man);
    }

    #[test]
    fn bad_magic_rejected() {
        assert!(unpack(b"NOTSKIR................").is_err());
    }

    #[test]
    fn empty_code_ok() {
        let kir = pack(&[], &[], b"{}");
        assert!(unpack(&kir).is_ok());
    }
}
