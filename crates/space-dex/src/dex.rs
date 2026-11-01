//! DEX 文件解析（Android dex 格式，手写无依赖）。
//!
//! 覆盖：header、string_ids、type_ids、proto_ids、field_ids、method_ids、
//! class_defs、class_data、code_item。表项解析为轻量视图，CodeItem 指令流
//! 以字节切片保留（解释器直接读）。

use crate::reader::{insns_slice, Reader, ReadErr};
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

#[cfg(test)]
use alloc::vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DexErr {
    BadMagic,
    BadVersion,
    BadEndian,
    BadHeader,
    Truncated,
    BadUleb,
    BadUtf8,
    BadTable,
    NotCode,
}

pub type R<T> = core::result::Result<T, DexErr>;

const DEX_MAGIC: &[u8; 8] = b"dex\n035\0";
const HEADER_SIZE: usize = 0x70;

/// 方法 id（全局 method_ids 表中的一项，解析后具体化）
#[derive(Debug, Clone)]
pub struct Method {
    /// 方法所属类描述符（如 "LMain;"）
    pub class: String,
    /// 方法名（如 "compute"）
    pub name: String,
    /// 参数类型描述符列表（如 ["I"]）
    pub params: Vec<String>,
    /// 返回类型描述符（如 "I" / "V"）
    pub ret: String,
    /// 简写原型（如 "(I)I"）
    pub shorty: String,
    /// 方法在 method_ids 表中的索引
    pub idx: u32,
    /// code 偏移；-1 = abstract/native（无代码）
    pub code_off: i32,
    pub access_flags: u32,
}

impl Method {
    /// 完整签名："LMain;->compute(I)I"
    pub fn signature(&self) -> String {
        alloc::format!("{};->{}{}", self.class, self.name, self.shorty)
    }
}

/// 解析后的 DEX 视图（不持有数据）
#[derive(Debug, Clone)]
pub struct DexFile<'a> {
    pub data: &'a [u8],
    pub version: u32,
    pub strings: Vec<String>,
    pub types: Vec<String>,
    /// proto 的短原型（含返回）
    pub protos: Vec<String>,
    /// 所有方法（class_data 聚合，idx 递增排序）
    pub methods: Vec<Method>,
    pub class_count: u32,
}

fn err_of(e: ReadErr) -> DexErr {
    match e {
        ReadErr::Oob => DexErr::Truncated,
        ReadErr::BadUleb => DexErr::BadUleb,
        ReadErr::BadUtf8 => DexErr::BadUtf8,
    }
}

impl<'a> DexFile<'a> {
    pub fn parse(data: &'a [u8]) -> R<Self> {
        if data.len() < HEADER_SIZE {
            return Err(DexErr::BadHeader);
        }
        if &data[0..8] != DEX_MAGIC {
            return Err(DexErr::BadMagic);
        }
        // 版本：035 / 036 / 037 / 038 / 039 / 040
        let v = core::str::from_utf8(&data[4..7]).map_err(|_| DexErr::BadVersion)?;
        if !["035", "036", "037", "038", "039", "040"].contains(&v) {
            return Err(DexErr::BadVersion);
        }
        let version: u32 = v.parse().map_err(|_| DexErr::BadVersion)?;
        if data[7] != 0 {
            return Err(DexErr::BadMagic);
        }
        let mut r = Reader::new(data);
        r.seek(32).map_err(err_of)?;
        let file_size = r.u32().map_err(err_of)?;
        if file_size as usize > data.len() {
            return Err(DexErr::Truncated);
        }
        let header_size = r.u32().map_err(err_of)?;
        if header_size != HEADER_SIZE as u32 {
            return Err(DexErr::BadHeader);
        }
        let endian = r.u32().map_err(err_of)?;
        if endian != 0x1234_5678 {
            return Err(DexErr::BadEndian);
        }
        // 表头：string/type/proto/field/method/class_defs
        let mut r = Reader::new(data);
        r.seek(56).map_err(err_of)?;
        let string_size = r.u32().map_err(err_of)?;
        let string_off = r.u32().map_err(err_of)?;
        let type_size = r.u32().map_err(err_of)?;
        let type_off = r.u32().map_err(err_of)?;
        let proto_size = r.u32().map_err(err_of)?;
        let proto_off = r.u32().map_err(err_of)?;
        let _field_size = r.u32().map_err(err_of)?;
        let _field_off = r.u32().map_err(err_of)?;
        let method_size = r.u32().map_err(err_of)?;
        let method_off = r.u32().map_err(err_of)?;
        let class_size = r.u32().map_err(err_of)?;
        let class_off = r.u32().map_err(err_of)?;

        // 字符串池
        let mut strings = Vec::with_capacity(string_size as usize);
        for i in 0..string_size as usize {
            let mut r2 = Reader::new(data);
            r2.seek((string_off + (i as u32) * 4) as usize)
                .map_err(err_of)?;
            let off = r2.u32().map_err(err_of)? as usize;
            let mut r3 = Reader::at(data, off);
            let _utf16_len = r3.uleb().map_err(err_of)?;
            let s = r3.mutf8().map_err(err_of)?;
            strings.push(s);
        }

        // 类型描述符
        let mut types = Vec::with_capacity(type_size as usize);
        for i in 0..type_size as usize {
            let mut r2 = Reader::new(data);
            r2.seek((type_off + (i as u32) * 4) as usize)
                .map_err(err_of)?;
            let di = r2.u32().map_err(err_of)? as usize;
            types.push(strings.get(di).cloned().unwrap_or_default());
        }

        // 原型（短原型含返回：(params)ret）
        let mut protos = Vec::with_capacity(proto_size as usize);
        for i in 0..proto_size as usize {
            let mut r2 = Reader::new(data);
            r2.seek((proto_off + (i as u32) * 12) as usize)
                .map_err(err_of)?;
            let shorty_idx = r2.u32().map_err(err_of)? as usize;
            let ret_idx = r2.u32().map_err(err_of)? as usize;
            let params_off = r2.u32().map_err(err_of)? as usize;
            let ret = types.get(ret_idx).cloned().unwrap_or_default();
            let mut ps = Vec::new();
            if params_off != 0 {
                let mut r3 = Reader::at(data, params_off);
                let n = r3.u32().map_err(err_of)? as usize;
                for _ in 0..n {
                    let ti = r3.u16().map_err(err_of)? as usize;
                    ps.push(types.get(ti).cloned().unwrap_or_default());
                }
            }
            let params_join = ps.join("");
            let _ = shorty_idx; // shorty 字符串保留字段，MVP 用拼接结果
            protos.push(alloc::format!("({}){}", params_join, ret));
        }

        // 方法 id（具体化类/名/原型）
        let mut methods = Vec::new();
        if method_size > 0 && method_off > 0 {
            for i in 0..method_size as usize {
                let mut r2 = Reader::new(data);
                r2.seek((method_off + (i as u32) * 8) as usize)
                    .map_err(err_of)?;
                let class_idx = r2.u16().map_err(err_of)? as usize;
                let proto_idx = r2.u16().map_err(err_of)? as usize;
                let name_idx = r2.u32().map_err(err_of)? as usize;
                let class = types.get(class_idx).cloned().unwrap_or_default();
                let shorty = protos.get(proto_idx).cloned().unwrap_or_default();
                let name = strings.get(name_idx).cloned().unwrap_or_default();
                // 从短原型拆参数与返回
                let (params, ret) = split_shorty(&shorty);
                methods.push(Method {
                    class,
                    name,
                    params,
                    ret,
                    shorty,
                    idx: i as u32,
                    code_off: -1,
                    access_flags: 0,
                });
            }
        }

        // class_defs → class_data → 方法 code 填充
        if class_size > 0 && class_off > 0 {
            for i in 0..class_size as usize {
                let mut r2 = Reader::new(data);
                r2.seek((class_off + (i as u32) * 32) as usize)
                    .map_err(err_of)?;
                let class_idx = r2.u32().map_err(err_of)?;
                let access_flags = r2.u32().map_err(err_of)?;
                let _super_idx = r2.u32().map_err(err_of)?;
                let _interfaces_off = r2.u32().map_err(err_of)?;
                let _source_idx = r2.u32().map_err(err_of)?;
                let _annotations_off = r2.u32().map_err(err_of)?;
                let class_data_off = r2.u32().map_err(err_of)?;
                let _static_values_off = r2.u32().map_err(err_of)?;
                let _ = class_idx;
                let _ = access_flags;
                if class_data_off == 0 {
                    continue; // 无方法体
                }
                // 解析 direct + virtual 方法，回填 method_ids 的 code/access
                let ms = parse_class_data_methods(data, class_data_off)?;
                for (mid, flags, code_off) in ms {
                    if let Some(m) = methods.get_mut(mid as usize) {
                        m.code_off = code_off;
                        m.access_flags = flags;
                    }
                }
            }
        }

        Ok(DexFile {
            data,
            version,
            strings,
            types,
            protos,
            methods,
            class_count: class_size,
        })
    }

    /// 取方法（按 method id）；越界返回 None
    pub fn method(&self, id: u32) -> Option<&Method> {
        self.methods.get(id as usize)
    }
}

/// 从短原型 "(III)I" 拆分参数与返回
pub fn split_shorty(shorty: &str) -> (Vec<String>, String) {
    let mut ps = Vec::new();
    let mut ret = String::new();
    let b = shorty.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i] == b'(' {
        i += 1;
        let mut cur = String::new();
        while i < b.len() && b[i] != b')' {
            match b[i] {
                b'[' => {
                    cur.push('[');
                    i += 1;
                }
                b'L' => {
                    // 类名直到 ';'（可能带数组前缀）
                    let start = i;
                    while i < b.len() && b[i] != b';' {
                        i += 1;
                    }
                    if i < b.len() {
                        i += 1;
                    }
                    cur.push_str(&shorty[start..i]);
                    ps.push(core::mem::take(&mut cur));
                }
                c => {
                    // 基元（可能带数组前缀）
                    cur.push(c as char);
                    ps.push(core::mem::take(&mut cur));
                    i += 1;
                }
            }
        }
        i += 1; // ')'
    }
    if i < b.len() {
        ret = shorty[i..].to_string();
    }
    (ps, ret)
}

/// 校验并解析 class_data 的全部方法（direct + virtual，共用递增的 method_id 序列）。
/// 返回 (method_id, access_flags, code_off)。
pub fn parse_class_data_methods(data: &[u8], class_data_off: u32) -> R<Vec<(u32, u32, i32)>> {
    let mut r = Reader::at(data, class_data_off as usize);
    let sfields = r.uleb().map_err(err_of)?;
    let ifields = r.uleb().map_err(err_of)?;
    let dmethods = r.uleb().map_err(err_of)?;
    let vmethods = r.uleb().map_err(err_of)?;
    // 跳过字段编码（encoded_field：field_idx_diff + access_flags）
    for _ in 0..sfields + ifields {
        let _ = r.uleb().map_err(err_of)?;
        let _ = r.uleb().map_err(err_of)?;
    }
    let mut out = Vec::new();
    let mut last: u32 = 0;
    for _ in 0..dmethods + vmethods {
        let diff = r.uleb().map_err(err_of)?;
        last += diff;
        let flags = r.uleb().map_err(err_of)?;
        let code_off = r.uleb().map_err(err_of)?;
        out.push((last, flags, if code_off == 0 { -1 } else { code_off as i32 }));
    }
    Ok(out)
}

/// 取 CodeItem 的指令流字节切片；无代码返回 None
pub fn code_insns<'a>(data: &'a [u8], code_off: u32) -> Option<(&'a [u8], u16, u16, u16)> {
    if code_off == 0 || code_off as usize + 16 > data.len() {
        return None;
    }
    let mut r = Reader::at(data, code_off as usize);
    let registers = r.u16().ok()?;
    let ins = r.u16().ok()?;
    let outs = r.u16().ok()?;
    let _tries = r.u16().ok()?;
    let _debug_off = r.u32().ok()?;
    let insns_size = r.u32().ok()?;
    let insns = insns_slice(data, r.p, insns_size as usize).ok()?;
    Some((insns, registers, ins, outs))
}

/// 类描述符 → 显示短名（"Lcom/foo/Bar;" → "Bar"；"I" → "I"）
pub fn type_short(desc: &str) -> &str {
    if let Some(rest) = desc.strip_prefix('L') {
        let end = rest.find(';').unwrap_or(rest.len());
        let inner = &rest[..end];
        inner.rsplit('/').next().unwrap_or(inner)
    } else if let Some(arr) = desc.strip_prefix('[') {
        type_short(arr)
    } else {
        desc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_proto() {
        let (p, r) = split_shorty("(III)I");
        assert_eq!(p, vec!["I", "I", "I"]);
        assert_eq!(r, "I");
        let (p2, r2) = split_shorty("(Ljava/lang/String;[I)J");
        assert_eq!(p2, vec!["Ljava/lang/String;", "[I"]);
        assert_eq!(r2, "J");
        let (p3, r3) = split_shorty("()V");
        assert!(p3.is_empty());
        assert_eq!(r3, "V");
    }

    #[test]
    fn short_names() {
        assert_eq!(type_short("Lcom/foo/Bar;"), "Bar");
        assert_eq!(type_short("I"), "I");
        assert_eq!(type_short("[I"), "I");
        assert_eq!(type_short("Ljava/lang/String;"), "String");
    }

    #[test]
    fn magic_rejected() {
        let bad = [0u8; 128];
        assert!(DexFile::parse(&bad).is_err());
    }
}
