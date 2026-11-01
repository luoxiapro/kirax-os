//! DEX 兼容解码与 .kir 转译骨架 —— apkcompat 接入 space-dex 引擎。
//!
//! 把 APK 内（deflate 压缩的）`classes.dex` 提取、解压后交给
//! `space_dex::dex::DexFile` 解析，输出：
//!   1. `DexInfo`：头表规模 / 方法构成 / 指令总数 / opcode 分布 / 未支持清单
//!   2. `dex_to_kir`：方法级 .kir 转译骨架（AOT 方向；每行 = 地址 | 助记符 | 原始 code units）
//!
//! 指令长度与载荷区间跳过的口径与 `crates/space-dex/examples/scan_dex.rs`
//! 完全一致（经 037 真实 DEX 对 baksmali 审计定稿：257909 vs 257910，差 1
//! 为载荷对齐填充口径）。未支持 = 解释器支持集之外的 opcode（037 样本仅
//! `0x0d move-exception`，属 try/catch 合理缺口）。

use crate::ApkError;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use space_dex::dex::{code_insns, DexFile};

/// 一条 opcode 的统计
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DexOpcodeStat {
    pub opcode: u8,
    pub mnemonic: &'static str,
    pub count: u32,
    pub supported: bool,
}

/// DEX 解码报告
#[derive(Debug, Clone)]
pub struct DexInfo {
    /// DEX 版本号（如 35 / 37 / 38 / 39）
    pub version: u32,
    pub strings: usize,
    pub types: usize,
    pub protos: usize,
    pub methods: usize,
    pub classes: u32,
    /// 有 code_item 的方法数（代码）
    pub code_methods: usize,
    /// native/abstract（无代码）方法数
    pub native_methods: usize,
    /// 指令总数（载荷数据区间已跳过）
    pub insn_total: u32,
    /// 未支持 opcode（opcode, 助记符, 条数）
    pub unsupported: Vec<(u8, &'static str, u32)>,
    /// 全部 opcode 直方图（按 opcode 升序）
    pub opcodes: Vec<DexOpcodeStat>,
}

/// 解析 DEX 字节并扫描指令（与 scan_dex 审计口径一致）
pub fn inspect_dex(dex_bytes: &[u8]) -> Result<DexInfo, ApkError> {
    let dex = DexFile::parse(dex_bytes).map_err(|_| ApkError::BadDex)?;
    let mut code_methods = 0usize;
    let mut native_methods = 0usize;
    for m in &dex.methods {
        if m.code_off >= 0 {
            code_methods += 1;
        } else {
            native_methods += 1;
        }
    }
    let mut hist: BTreeMap<u8, u32> = BTreeMap::new();
    let mut total = 0u32;
    for m in &dex.methods {
        if m.code_off < 0 {
            continue;
        }
        let Some((insns, _r, _i, _o)) = code_insns(dex_bytes, m.code_off as u32) else {
            continue;
        };
        let mut pc = 0usize;
        let mut pl: Vec<(usize, usize)> = Vec::new();
        while pc + 2 <= insns.len() {
            if let Some(&(_, e)) = pl.iter().find(|&&(s, e)| pc >= s && pc < e) {
                pc = e;
                continue;
            }
            let w = u16::from_le_bytes([insns[pc], insns[pc + 1]]);
            let op = (w & 0xff) as u8;
            *hist.entry(op).or_insert(0) += 1;
            total += 1;
            if matches!(op, 0x26 | 0x2b | 0x2c) {
                if let Some((s, e)) = payload_range(&insns, pc) {
                    if s >= pc && e <= insns.len() && !pl.iter().any(|&(a, b)| s < b && a < e) {
                        pl.push((s, e));
                    }
                }
            }
            let units = inst_units(op);
            if units == 0 {
                break; // 未知 opcode：停止本方法（不误判后续）
            }
            pc += units * 2;
        }
    }
    let mut opcodes = Vec::new();
    let mut unsupported = Vec::new();
    for (op, n) in &hist {
        let m = mnemonic(*op);
        let supported = supported_ops().contains(op);
        opcodes.push(DexOpcodeStat {
            opcode: *op,
            mnemonic: m,
            count: *n,
            supported,
        });
        if !supported {
            unsupported.push((*op, m, *n));
        }
    }
    Ok(DexInfo {
        version: dex.version,
        strings: dex.strings.len(),
        types: dex.types.len(),
        protos: dex.protos.len(),
        methods: dex.methods.len(),
        classes: dex.class_count,
        code_methods,
        native_methods,
        insn_total: total,
        unsupported,
        opcodes,
    })
}

/// 端到端：解析 APK → 提取 classes.dex（Store/Deflate）→ DEX 解码报告
pub fn inspect_apk_dex(apk_bytes: &[u8]) -> Result<DexInfo, ApkError> {
    let apk = crate::Apk::parse(apk_bytes)?;
    let entry = apk.find("classes.dex")?;
    let dex = apk.extract(entry)?;
    inspect_dex(&dex)
}

/// 方法级 .kir 转译骨架：AOT 方向的第一步真实产物。
/// 每行 = 地址 | 助记符 | 原始 code units（与 baksmali dump `addr: hex | smali` 同构）。
pub fn dex_to_kir(dex_bytes: &[u8], kir_name: &str) -> Result<String, ApkError> {
    let dex = DexFile::parse(dex_bytes).map_err(|_| ApkError::BadDex)?;
    let mut out = String::new();
    out.push_str(&format!(
        "; Space OS .kir —— DEX AOT 转译骨架（{}，dex\\n0{}）\n",
        kir_name, dex.version
    ));
    out.push_str(&format!(
        "; strings={} types={} protos={} methods={} classes={}\n\n",
        dex.strings.len(),
        dex.types.len(),
        dex.protos.len(),
        dex.methods.len(),
        dex.class_count
    ));
    // 按类分组输出
    let mut by_class: BTreeMap<String, Vec<&space_dex::dex::Method>> = BTreeMap::new();
    for m in &dex.methods {
        by_class.entry(m.class.clone()).or_default().push(m);
    }
    for (cls, ms) in &by_class {
        out.push_str(&format!(".class {}\n", cls));
        for m in ms {
            if m.code_off < 0 {
                out.push_str(&format!(
                    "  ; [native] {}({}){}\n",
                    m.name,
                    m.params.join(""),
                    m.ret
                ));
                continue;
            }
            let Some((insns, regs, ins, ots)) = code_insns(dex_bytes, m.code_off as u32) else {
                continue;
            };
            out.push_str(&format!(
                "  .method {}({}){}  regs={} ins={} outs={} insns={}u\n",
                m.name,
                m.params.join(""),
                m.ret,
                regs,
                ins,
                ots,
                insns.len() / 2
            ));
            let mut pc = 0usize;
            let mut pl: Vec<(usize, usize)> = Vec::new();
            while pc + 2 <= insns.len() {
                if let Some(&(s, e)) = pl.iter().find(|&&(s, e)| pc >= s && pc < e) {
                    out.push_str(&format!(
                        "    ; {:04x}: [payload 数据区间 {:04x}..{:04x}]\n",
                        pc, s, e
                    ));
                    pc = e;
                    continue;
                }
                let w = u16::from_le_bytes([insns[pc], insns[pc + 1]]);
                let op = (w & 0xff) as u8;
                // 原始 code units hex
                let units = inst_units(op);
                let nbytes = units.saturating_mul(2).min(insns.len() - pc);
                let mut hex = String::new();
                for b in &insns[pc..pc + nbytes] {
                    hex.push_str(&format!("{:02x}", b));
                }
                out.push_str(&format!("    ; {:04x}  {:<24} {}\n", pc, mnemonic(op), hex));
                if matches!(op, 0x26 | 0x2b | 0x2c) {
                    if let Some((s, e)) = payload_range(&insns, pc) {
                        if s >= pc && e <= insns.len() && !pl.iter().any(|&(a, b)| s < b && a < e) {
                            pl.push((s, e));
                        }
                    }
                }
                if units == 0 {
                    break;
                }
                pc += units * 2;
            }
            out.push_str("  .end method\n");
        }
        out.push_str("\n");
    }
    Ok(out)
}

// ----------------------------------------------------------------------------
// 指令长度 / 载荷区间 / 支持集 / 助记符 —— 与 scan_dex 审计定稿口径一致
// ----------------------------------------------------------------------------

/// 指令长度（code units）；未知返回 0
pub fn inst_units(op: u8) -> usize {
    match op {
        0x00 => 1,
        0x01..=0x09 => match op {
            0x03 | 0x06 | 0x09 => 3, // move/16 系列
            0x02 | 0x05 | 0x08 => 2, // move/from16 系列
            _ => 1,
        },
        0x0a..=0x0d => 1,
        0x0e..=0x11 => 1,
        0x12 => 1,
        0x13 => 2,
        0x14 => 3,
        0x15 => 2,
        0x16 => 2,
        0x17 => 3,
        0x18 => 5,
        0x19 => 2,
        0x1a => 2,
        0x1b => 3,
        0x1c => 2,
        0x1d | 0x1e => 1,
        0x1f => 2,
        0x20 => 2,
        0x21 => 1,
        0x22 => 2,
        0x23 => 2,
        0x24 => 3,
        0x25 => 3,
        0x26 => 3,
        0x27 => 1,
        0x28 => 1,
        0x29 => 2,
        0x2a => 3,
        0x2b | 0x2c => 3,
        0x2d..=0x31 => 2,
        0x32..=0x37 => 2,
        0x38..=0x3d => 2,
        0x3e..=0x3f => 1,
        0x40..=0x51 => 2,
        0x52..=0x5f => 2,
        0x60..=0x6d => 2,
        0x6e..=0x72 => 3,
        0x73 => 1,
        0x74..=0x78 => 3,
        0x79..=0x7a => 4,
        0x7b..=0x8f => 1,
        0x90..=0xaf => 2,
        0xb0..=0xcf => 1,
        0xd0..=0xd7 => 2, // lit16（A/B 各 4 位）
        0xd8..=0xe2 => 2, // lit8
        0xe3..=0xff => 1, // unused / 040 新增
    }
}

/// switch / fill-array-data 载荷数据区间（字节）；非载荷指令返回 None。
/// packed-switch 载荷头 8 字节（ident+size+first_key），对齐到 4。
pub fn payload_range(insns: &[u8], pc: usize) -> Option<(usize, usize)> {
    if pc + 4 > insns.len() {
        return None;
    }
    let w = u16::from_le_bytes([insns[pc], insns[pc + 1]]);
    let op = (w & 0xff) as u8;
    if !matches!(op, 0x26 | 0x2b | 0x2c) {
        return None;
    }
    let off = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
    let start = pc + off * 2;
    if start + 4 > insns.len() {
        return None;
    }
    let ident = u16::from_le_bytes([insns[start], insns[start + 1]]);
    let body = match ident {
        0x0100 => {
            let sz = u16::from_le_bytes([insns[start + 2], insns[start + 3]]) as usize;
            8 + sz * 4
        }
        0x0200 => {
            let sz = u16::from_le_bytes([insns[start + 2], insns[start + 3]]) as usize;
            4 + sz * 8
        }
        0x0300 => {
            let ew = u16::from_le_bytes([insns[start + 2], insns[start + 3]]) as usize;
            if start + 8 > insns.len() {
                return None;
            }
            let sz = u32::from_le_bytes([
                insns[start + 4],
                insns[start + 5],
                insns[start + 6],
                insns[start + 7],
            ]) as usize;
            8 + sz * ew
        }
        _ => return None,
    };
    let end = ((start + body) + 3) & !3;
    Some((start, end))
}

/// 解释器支持集（与 scan_dex 口径一致；0x0d move-exception 属路线图缺口）
fn supported_ops() -> Vec<u8> {
    let mut v: Vec<u8> = Vec::new();
    v.extend(0x00..=0x00);
    v.extend(0x01..=0x09);
    v.extend(0x0a..=0x0c);
    v.extend(0x0e..=0x11);
    v.extend(0x12..=0x19);
    v.extend(0x1a..=0x1b);
    v.extend(0x1c..=0x2c);
    v.extend(0x2d..=0x31);
    v.extend(0x32..=0x3d);
    v.extend(0x3e..=0x3f);
    v.extend(0x40..=0x51);
    v.extend(0x52..=0x5f);
    v.extend(0x60..=0x6d);
    v.extend(0x6e..=0x72);
    v.extend(0x74..=0x78);
    v.extend(0x7b..=0x90);
    v.extend(0x91..=0xa6);
    v.extend(0xa7..=0xaf);
    v.extend(0xb0..=0xcf);
    v.extend(0xd0..=0xd7);
    v.extend(0xd8..=0xe2);
    v
}

/// 精确 smali 助记符（官方 Dalvik 指令表）
pub fn mnemonic(op: u8) -> &'static str {
    match op {
        0x00 => "nop",
        0x01 => "move",
        0x02 => "move/from16",
        0x03 => "move/16",
        0x04 => "move-wide",
        0x05 => "move-wide/from16",
        0x06 => "move-wide/16",
        0x07 => "move-object",
        0x08 => "move-object/from16",
        0x09 => "move-object/16",
        0x0a => "move-result",
        0x0b => "move-result-wide",
        0x0c => "move-result-object",
        0x0d => "move-exception",
        0x0e => "return-void",
        0x0f => "return",
        0x10 => "return-wide",
        0x11 => "return-object",
        0x12 => "const/4",
        0x13 => "const/16",
        0x14 => "const",
        0x15 => "const/high16",
        0x16 => "const-wide/16",
        0x17 => "const-wide/32",
        0x18 => "const-wide",
        0x19 => "const-wide/high16",
        0x1a => "const-string",
        0x1b => "const-string/jumbo",
        0x1c => "const-class",
        0x1d => "monitor-enter",
        0x1e => "monitor-exit",
        0x1f => "check-cast",
        0x20 => "instance-of",
        0x21 => "array-length",
        0x22 => "new-instance",
        0x23 => "new-array",
        0x24 => "filled-new-array",
        0x25 => "filled-new-array/range",
        0x26 => "fill-array-data",
        0x27 => "throw",
        0x28 => "goto",
        0x29 => "goto/16",
        0x2a => "goto/32",
        0x2b => "packed-switch",
        0x2c => "sparse-switch",
        0x2d => "cmpl-float",
        0x2e => "cmpg-float",
        0x2f => "cmpl-double",
        0x30 => "cmpg-double",
        0x31 => "cmp-long",
        0x32 => "if-eq",
        0x33 => "if-ne",
        0x34 => "if-lt",
        0x35 => "if-ge",
        0x36 => "if-gt",
        0x37 => "if-le",
        0x38 => "if-eqz",
        0x39 => "if-nez",
        0x3a => "if-ltz",
        0x3b => "if-gez",
        0x3c => "if-gtz",
        0x3d => "if-lez",
        0x3e => "unused-3e",
        0x3f => "unused-3f",
        0x44 => "aget",
        0x45 => "aget-wide",
        0x46 => "aget-object",
        0x47 => "aget-boolean",
        0x48 => "aget-byte",
        0x49 => "aget-char",
        0x4a => "aget-short",
        0x4b => "aput",
        0x4c => "aput-wide",
        0x4d => "aput-object",
        0x4e => "aput-boolean",
        0x4f => "aput-byte",
        0x50 => "aput-char",
        0x51 => "aput-short",
        0x52 => "iget",
        0x53 => "iget-wide",
        0x54 => "iget-object",
        0x55 => "iget-boolean",
        0x56 => "iget-byte",
        0x57 => "iget-char",
        0x58 => "iget-short",
        0x59 => "iput",
        0x5a => "iput-wide",
        0x5b => "iput-object",
        0x5c => "iput-boolean",
        0x5d => "iput-byte",
        0x5e => "iput-char",
        0x5f => "iput-short",
        0x60 => "sget",
        0x61 => "sget-wide",
        0x62 => "sget-object",
        0x63 => "sget-boolean",
        0x64 => "sget-byte",
        0x65 => "sget-char",
        0x66 => "sget-short",
        0x67 => "sput",
        0x68 => "sput-wide",
        0x69 => "sput-object",
        0x6a => "sput-boolean",
        0x6b => "sput-byte",
        0x6c => "sput-char",
        0x6d => "sput-short",
        0x6e => "invoke-virtual",
        0x6f => "invoke-super",
        0x70 => "invoke-direct",
        0x71 => "invoke-static",
        0x72 => "invoke-interface",
        0x73 => "unused-73",
        0x74 => "invoke-virtual/range",
        0x75 => "invoke-super/range",
        0x76 => "invoke-direct/range",
        0x77 => "invoke-static/range",
        0x78 => "invoke-interface/range",
        0x79 => "unused-79",
        0x7a => "unused-7a",
        0x7b => "neg-int",
        0x7c => "not-int",
        0x7d => "neg-long",
        0x7e => "not-long",
        0x7f => "neg-float",
        0x80 => "neg-double",
        0x81 => "int-to-long",
        0x82 => "int-to-float",
        0x83 => "int-to-double",
        0x84 => "long-to-int",
        0x85 => "long-to-float",
        0x86 => "long-to-double",
        0x87 => "float-to-int",
        0x88 => "float-to-long",
        0x89 => "float-to-double",
        0x8a => "double-to-int",
        0x8b => "double-to-long",
        0x8c => "double-to-float",
        0x8d => "int-to-byte",
        0x8e => "int-to-char",
        0x8f => "int-to-short",
        0x90 => "add-int",
        0x91 => "sub-int",
        0x92 => "mul-int",
        0x93 => "div-int",
        0x94 => "rem-int",
        0x95 => "and-int",
        0x96 => "or-int",
        0x97 => "xor-int",
        0x98 => "shl-int",
        0x99 => "shr-int",
        0x9a => "ushr-int",
        0x9b => "add-long",
        0x9c => "sub-long",
        0x9d => "mul-long",
        0x9e => "div-long",
        0x9f => "rem-long",
        0xa0 => "and-long",
        0xa1 => "or-long",
        0xa2 => "xor-long",
        0xa3 => "shl-long",
        0xa4 => "shr-long",
        0xa5 => "ushr-long",
        0xa6 => "add-float",
        0xa7 => "sub-float",
        0xa8 => "mul-float",
        0xa9 => "div-float",
        0xaa => "rem-float",
        0xab => "add-double",
        0xac => "sub-double",
        0xad => "mul-double",
        0xae => "div-double",
        0xaf => "rem-double",
        0xb0 => "add-int/2addr",
        0xb1 => "sub-int/2addr",
        0xb2 => "mul-int/2addr",
        0xb3 => "div-int/2addr",
        0xb4 => "rem-int/2addr",
        0xb5 => "and-int/2addr",
        0xb6 => "or-int/2addr",
        0xb7 => "xor-int/2addr",
        0xb8 => "shl-int/2addr",
        0xb9 => "shr-int/2addr",
        0xba => "ushr-int/2addr",
        0xbb => "add-long/2addr",
        0xbc => "sub-long/2addr",
        0xbd => "mul-long/2addr",
        0xbe => "div-long/2addr",
        0xbf => "rem-long/2addr",
        0xc0 => "and-long/2addr",
        0xc1 => "or-long/2addr",
        0xc2 => "xor-long/2addr",
        0xc3 => "shl-long/2addr",
        0xc4 => "shr-long/2addr",
        0xc5 => "ushr-long/2addr",
        0xc6 => "add-float/2addr",
        0xc7 => "sub-float/2addr",
        0xc8 => "mul-float/2addr",
        0xc9 => "div-float/2addr",
        0xca => "rem-float/2addr",
        0xcb => "add-double/2addr",
        0xcc => "sub-double/2addr",
        0xcd => "mul-double/2addr",
        0xce => "div-double/2addr",
        0xcf => "rem-double/2addr",
        0xd0 => "add-int/lit16",
        0xd1 => "rsub-int",
        0xd2 => "mul-int/lit16",
        0xd3 => "div-int/lit16",
        0xd4 => "rem-int/lit16",
        0xd5 => "and-int/lit16",
        0xd6 => "or-int/lit16",
        0xd7 => "xor-int/lit16",
        0xd8 => "add-int/lit8",
        0xd9 => "rsub-int/lit8",
        0xda => "mul-int/lit8",
        0xdb => "div-int/lit8",
        0xdc => "rem-int/lit8",
        0xdd => "and-int/lit8",
        0xde => "or-int/lit8",
        0xdf => "xor-int/lit8",
        0xe0 => "shl-int/lit8",
        0xe1 => "shr-int/lit8",
        0xe2 => "ushr-int/lit8",
        0xe3..=0xff => "unused",
        _ => "?",
    }
}
