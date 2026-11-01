//! 测试支撑：手工构造最小 DEX 文件（无工具链依赖），验证解析器与解释器。
//!
//! 布局遵循 DEX 规范：header(0x70) → 各 *_ids 表 → string_data → type_list →
//! class_data → code_item。全部小端。

use alloc::vec;
use alloc::vec::Vec;

/// 追加 uleb128
fn uleb(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

fn u16le(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn u32le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// string_data：utf16_size + MUTF-8 + NUL
fn string_data(out: &mut Vec<u8>, s: &str) -> usize {
    let off = out.len();
    let chars = s.chars().count();
    uleb(out, chars as u32);
    out.extend_from_slice(s.as_bytes());
    out.push(0);
    off
}

/// 组装 DEX。`strings` 的每一项是字符串文本；各表按约定构造。
struct DexLayout {
    strings: Vec<&'static str>,
    /// (descriptor_string_idx)
    types: Vec<u32>,
    /// (shorty_idx, return_type_idx, params_type_idxs)
    protos: Vec<(u32, u32, Vec<u32>)>,
    /// (class_type_idx, proto_idx, name_string_idx)
    methods: Vec<(u32, u32, u32)>,
    /// (class_type_idx, direct_methods: (method_idx_diff 起点 0, flags, insns_units), virtual…)
    classes: Vec<(u32, Vec<(u32, u32, Vec<u16>)>)>,
}

fn build(l: &DexLayout) -> Vec<u8> {
    let mut d: Vec<u8> = vec![0u8; 0x70];
    // magic + version
    d[0..8].copy_from_slice(b"dex\n035\0");

    // ---- 表偏移占位（最后回填） ----
    let s_off = d.len();
    for _ in 0..l.strings.len() * 4 {
        d.push(0);
    }
    let t_off = d.len();
    for _ in 0..l.types.len() * 4 {
        d.push(0);
    }
    let p_off = d.len();
    for _ in 0..l.protos.len() * 12 {
        d.push(0);
    }
    let f_off = d.len();
    for _ in 0..0 * 8 {
        d.push(0);
    }
    let m_off = d.len();
    for _ in 0..l.methods.len() * 8 {
        d.push(0);
    }
    let c_off = d.len();
    for _ in 0..l.classes.len() * 32 {
        d.push(0);
    }
    let data_off = d.len();

    // ---- string_data ----
    let mut str_offs = Vec::new();
    for s in &l.strings {
        str_offs.push(string_data(&mut d, s));
    }

    // ---- type_list（proto 参数） ----
    let mut proto_param_offs = Vec::new();
    for (_sh, _rt, params) in &l.protos {
        if params.is_empty() {
            proto_param_offs.push(0u32);
            continue;
        }
        let off = d.len() as u32;
        u32le(&mut d, params.len() as u32);
        for t in params {
            u16le(&mut d, *t as u16);
        }
        proto_param_offs.push(off);
    }

    // ---- class_data + code_item ----
    // ---- class_data + code_item ----
    // 注意：class_data 内 code_off 占位固定写 2 字节 uleb（0x80 0x00=0），
    // 回填时也固定写 2 字节，避免占位 1 字节因偏移>=0x80 覆盖 class_data 尾部。
    let mut class_data_offs = Vec::new();
    for (_cls, dmethods) in &l.classes {
        let cd_off = d.len() as u32;
        class_data_offs.push(cd_off);
        uleb(&mut d, 0); // static_fields
        uleb(&mut d, 0); // instance_fields
        uleb(&mut d, dmethods.len() as u32);
        uleb(&mut d, 0); // virtual_methods
        let mut last = 0u32;
        for (idx, flags, _insns) in dmethods {
            uleb(&mut d, idx - last);
            last = *idx;
            uleb(&mut d, *flags);
            d.push(0x80); // code_off 占位（2 字节 uleb 形式，值 0）
            d.push(0x00);
        }
    }

    // ---- code_item（紧随 class_data；此时偏移已可确定） ----
    let mut per_class_codes: Vec<Vec<u32>> = Vec::new();
    for (_cls, dmethods) in &l.classes {
        let mut codes = Vec::new();
        for (_idx, _flags, insns) in dmethods {
            let off = d.len() as u32;
            codes.push(off);
            // 16B 头
            u16le(&mut d, 4); // registers_size
            u16le(&mut d, 1); // ins_size
            u16le(&mut d, 0); // outs_size
            u16le(&mut d, 0); // tries_size
            u32le(&mut d, 0); // debug_info_off
            u32le(&mut d, insns.len() as u32); // insns_size（units）
            for u in insns {
                u16le(&mut d, *u);
            }
        }
        per_class_codes.push(codes);
    }

    // ---- 回填 ----
    // string_ids
    for (i, off) in str_offs.iter().enumerate() {
        let p = s_off + i * 4;
        d[p..p + 4].copy_from_slice(&(*off as u32).to_le_bytes());
    }
    // type_ids
    for (i, t) in l.types.iter().enumerate() {
        let p = t_off + i * 4;
        d[p..p + 4].copy_from_slice(&t.to_le_bytes());
    }
    // proto_ids
    for (i, (sh, rt, _params)) in l.protos.iter().enumerate() {
        let p = p_off + i * 12;
        d[p..p + 4].copy_from_slice(&sh.to_le_bytes());
        d[p + 4..p + 8].copy_from_slice(&rt.to_le_bytes());
        d[p + 8..p + 12].copy_from_slice(&proto_param_offs[i].to_le_bytes());
    }
    // method_ids
    for (i, (cl, pr, nm)) in l.methods.iter().enumerate() {
        let p = m_off + i * 8;
        d[p..p + 2].copy_from_slice(&(*cl as u16).to_le_bytes());
        d[p + 2..p + 4].copy_from_slice(&(*pr as u16).to_le_bytes());
        d[p + 4..p + 8].copy_from_slice(&nm.to_le_bytes());
    }
    // class_defs（class_data_off 回填；code_off 需按方法展开回填到 class_data）
    for (i, (_cls, dmethods)) in l.classes.iter().enumerate() {
        let p = c_off + i * 32;
        d[p..p + 4].copy_from_slice(&l.classes[i].0.to_le_bytes()); // class_idx
        d[p + 4..p + 8].copy_from_slice(&0u32.to_le_bytes()); // access_flags
        d[p + 8..p + 12].copy_from_slice(&0xffff_ffffu32.to_le_bytes()); // superclass NO_INDEX
        d[p + 12..p + 16].copy_from_slice(&0u32.to_le_bytes()); // interfaces
        d[p + 16..p + 20].copy_from_slice(&0xffff_ffffu32.to_le_bytes()); // source_file
        d[p + 20..p + 24].copy_from_slice(&0u32.to_le_bytes()); // annotations
        d[p + 24..p + 28].copy_from_slice(&class_data_offs[i].to_le_bytes());
        d[p + 28..p + 32].copy_from_slice(&0u32.to_le_bytes()); // static_values
        // 回填 class_data 内的 code_off（跳 4 个 uleb 头 + 每方法 diff/flags 两 uleb）
        let mut pos = class_data_offs[i] as usize;
        pos = skip_uleb(&d, pos);
        pos = skip_uleb(&d, pos);
        pos = skip_uleb(&d, pos);
        pos = skip_uleb(&d, pos);
        let mut ci = 0usize;
        for (_idx, _flags, _insns) in dmethods {
            pos = skip_uleb(&d, pos); // method_idx_diff
            pos = skip_uleb(&d, pos); // access_flags
            let co = per_class_codes[i][ci];
            // 固定写 2 字节 uleb（co < 0x4000，两字节足够）
            d[pos] = (co & 0x7f) as u8 | 0x80;
            d[pos + 1] = (co >> 7) as u8;
            ci += 1;
            pos += 2;
        }
    }

    // ---- header 回填 ----
    let file_size = d.len() as u32;
    d[32..36].copy_from_slice(&file_size.to_le_bytes());
    d[36..40].copy_from_slice(&0x70u32.to_le_bytes()); // header_size
    d[40..44].copy_from_slice(&0x1234_5678u32.to_le_bytes()); // endian_tag
    // link
    d[44..48].copy_from_slice(&0u32.to_le_bytes());
    d[48..52].copy_from_slice(&0u32.to_le_bytes());
    // map_off = 0
    d[52..56].copy_from_slice(&0u32.to_le_bytes());
    d[56..60].copy_from_slice(&(l.strings.len() as u32).to_le_bytes());
    d[60..64].copy_from_slice(&(s_off as u32).to_le_bytes());
    d[64..68].copy_from_slice(&(l.types.len() as u32).to_le_bytes());
    d[68..72].copy_from_slice(&(t_off as u32).to_le_bytes());
    d[72..76].copy_from_slice(&(l.protos.len() as u32).to_le_bytes());
    d[76..80].copy_from_slice(&(p_off as u32).to_le_bytes());
    d[80..84].copy_from_slice(&0u32.to_le_bytes()); // field_ids_size
    d[84..88].copy_from_slice(&(f_off as u32).to_le_bytes());
    d[88..92].copy_from_slice(&(l.methods.len() as u32).to_le_bytes());
    d[92..96].copy_from_slice(&(m_off as u32).to_le_bytes());
    d[96..100].copy_from_slice(&(l.classes.len() as u32).to_le_bytes());
    d[100..104].copy_from_slice(&(c_off as u32).to_le_bytes());
    let total = d.len() - data_off;
    d[104..108].copy_from_slice(&(total as u32).to_le_bytes()); // data_size
    d[108..112].copy_from_slice(&(data_off as u32).to_le_bytes());
    let _ = f_off;

    d
}

fn skip_uleb(d: &[u8], mut p: usize) -> usize {
    while d[p] & 0x80 != 0 {
        p += 1;
    }
    p + 1
}

/// Main.compute(int)->int = x*x+1
pub fn build_mini_dex() -> Vec<u8> {
    // strings: 0=LMain; 1=compute 2=I 3=(I)I
    // types: 0=LMain;(str0) 1=I(str2)
    // proto: shorty=(I)I(str3), ret=I(type1), params=[I(type1)]
    // method: class=type0, proto=0, name=str1
    // class: type0, direct=[method0: flags=0x9, code]
    // code (registers=2: v0 参数, v1 临时):
    //   const/4 v1, #1        -> 12x: [0x12, 0x11]
    //   mul-int v1, v0, v0    -> 23x: [0x93, 0x01, 0x00, 0x00, 0x00, 0x00]
    //   add-int/lit8 v1,v1,#1 -> 22b: [0xe0, 0x01, 0x01, 0x01]
    //   return v1             -> 11x: [0x0f, 0x01]
    let insns = build_insns(&[
        &[0x12u8, 0x11],
        &[0x93, 0x01, 0x00, 0x00, 0x00, 0x00],
        &[0xe0, 0x01, 0x01, 0x01],
        &[0x0f, 0x01],
    ]);
    let l = DexLayout {
        strings: vec!["LMain;", "compute", "I", "(I)I"],
        types: vec![0, 2],
        protos: vec![(3, 1, vec![1])],
        methods: vec![(0, 0, 1)],
        classes: vec![(0, vec![(0, 0x9, insns)])],
    };
    build(&l)
}

/// Main.fib(int)->int 递归斐波那契
pub fn build_fib_dex() -> Vec<u8> {
    // registers=5: v0=n, v1=1, v2,v3 临时, v4 未用
    // units 布局（字节）：
    // u0  const/4 v1,#1                      [0x12,0x11]
    // u1  if-le v0,v1,+24                    [0x32,0x00,0x01,0x18,0x00,0x00]  (22t: A=0,B=1,off=24)
    // u2  sub-int v2,v0,v1                   [0x92,0x02,0x00,0x01,0x00,0x00]
    // u3  sub-int v3,v0,v1                   [0x92,0x03,0x00,0x01,0x00,0x00]
    // u4  sub-int v3,v3,v1                   [0x92,0x03,0x03,0x01,0x00,0x00]
    // u5  invoke-static {v2}, fib           [0x71,0x01,0x00,0x00,0x02,0x00]  (35c: A=1,G=0,method=0,C=v2)
    // u6  move-result v2                     [0x0a,0x02]
    // u7  invoke-static {v3}, fib           [0x71,0x01,0x00,0x00,0x03,0x00]
    // u8  move-result v3                     [0x0a,0x03]
    // u9  add-int v2,v2,v3                   [0x91,0x02,0x02,0x03,0x00,0x00]
    // u10 return v2                          [0x0f,0x02]
    // u11 return v0                          [0x0f,0x00]
    // if-le 的 off：u1 位于 unit1（字节 2），目标 u11 位于 unit25（字节 50）。
    // DEX 分支偏移相对指令起始：off = (50 - 2) / 2 = 24 = 0x18
    let insns = build_insns(&[
        &[0x12, 0x11],
        &[0x32, 0x00, 0x01, 0x18, 0x00, 0x00],
        &[0x92, 0x02, 0x00, 0x01, 0x00, 0x00],
        &[0x92, 0x03, 0x00, 0x01, 0x00, 0x00],
        &[0x92, 0x03, 0x03, 0x01, 0x00, 0x00],
        &[0x71, 0x01, 0x00, 0x00, 0x02, 0x00],
        &[0x0a, 0x02],
        &[0x71, 0x01, 0x00, 0x00, 0x03, 0x00],
        &[0x0a, 0x03],
        &[0x91, 0x02, 0x02, 0x03, 0x00, 0x00],
        &[0x0f, 0x02],
        &[0x0f, 0x00],
    ]);
    // strings: 0=LMain; 1=fib 2=I 3=(I)I
    let l = DexLayout {
        strings: vec!["LMain;", "fib", "I", "(I)I"],
        types: vec![0, 2],
        protos: vec![(3, 1, vec![1])],
        methods: vec![(0, 0, 1)],
        classes: vec![(0, vec![(0, 0x9, insns)])],
    };
    build(&l)
}

/// 把字节段拼成 u16 序列（小端）
fn build_insns(parts: &[&[u8]]) -> Vec<u16> {
    let mut out = Vec::new();
    for p in parts {
        assert!(p.len() % 2 == 0, "指令段必须偶数长度");
        let mut i = 0;
        while i < p.len() {
            out.push(u16::from_le_bytes([p[i], p[i + 1]]));
            i += 2;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dex::DexFile;
    use crate::interp::{Interp, InterpErr};

    #[test]
    fn mini_dex_parses_and_computes() {
        let bytes = build_mini_dex();
        let d = DexFile::parse(&bytes).unwrap();
        assert_eq!(d.methods.len(), 1);
        let m = &d.methods[0];
        assert_eq!(m.name, "compute");
        assert_eq!(m.shorty, "(I)I");
        assert!(m.code_off >= 0);

        let mut ip = Interp::new(&d);
        // 7*7+1 = 50
        assert_eq!(ip.invoke_static("LMain;", "compute", "(I)I", &[7]).unwrap(), 50);
        assert_eq!(ip.invoke_static("Main", "compute", "(I)I", &[0]).unwrap(), 1);
        assert_eq!(ip.invoke_static("Main", "compute", "(I)I", &[-3i32 as u32]).unwrap(), 10);
    }

    #[test]
    fn fib_recursion() {
        let bytes = build_fib_dex();
        let d = DexFile::parse(&bytes).unwrap();
        let mut ip = Interp::new(&d);
        assert_eq!(ip.invoke_static("Main", "fib", "(I)I", &[0]).unwrap(), 0);
        assert_eq!(ip.invoke_static("Main", "fib", "(I)I", &[1]).unwrap(), 1);
        assert_eq!(ip.invoke_static("Main", "fib", "(I)I", &[10]).unwrap(), 55);
    }

    #[test]
    fn no_such_method() {
        let bytes = build_mini_dex();
        let d = DexFile::parse(&bytes).unwrap();
        let mut ip = Interp::new(&d);
        assert_eq!(
            ip.invoke_static("Main", "nope", "(I)I", &[1]),
            Err(InterpErr::NoSuchMethod)
        );
    }

    #[test]
    fn reject_bad_magic() {
        let mut b = build_mini_dex();
        b[0] = b'X';
        assert!(DexFile::parse(&b).is_err());
    }

    #[test]
    fn reject_bad_version() {
        let mut b = build_mini_dex();
        b[4..7].copy_from_slice(b"999");
        assert!(DexFile::parse(&b).is_err());
    }
}
