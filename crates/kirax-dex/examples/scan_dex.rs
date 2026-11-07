//! 真实 DEX 兼容性扫描：解析任意 classes.dex，输出类/方法清单、
//! opcode 分布与解释器未支持指令统计。
//!
//! 用法：cargo run --example scan_dex -- <classes.dex>

use kirax_dex::dex::{code_insns, DexFile};

/// 解释器当前支持的 opcode 集合（与 interp.rs 保持一致）
fn supported_ops() -> Vec<u8> {
    // 与 interp.rs 的分支严格对齐（含显式 UnsupportedOp 的路线图分支）
    let mut v: Vec<u8> = Vec::new();
    v.extend(0x00..=0x00); // nop
    v.extend(0x01..=0x09); // move
    v.extend(0x0a..=0x0c); // move-result
    v.extend(0x0e..=0x11); // return
    v.extend(0x12..=0x19); // const
    v.extend(0x1a..=0x1b); // const-string
    v.extend(0x1c..=0x2c); // const-class..switch（含 no-op 与路线图报错分支）
    v.extend(0x2d..=0x31); // cmp 系
    v.extend(0x32..=0x3d); // if-test / if-testz
    v.extend(0x3e..=0x3f); // unused（显式报错）
    v.extend(0x40..=0x51); // aget / aput
    v.extend(0x52..=0x5f); // iget / iput
    v.extend(0x60..=0x6d); // sget / sput
    v.extend(0x6e..=0x72); // invoke-kind 35c
    v.extend(0x74..=0x78); // invoke-kind/range 3rc
    v.extend(0x7b..=0x90); // 一元 / 转换
    v.extend(0x91..=0xa6); // binop int/long
    v.extend(0xa7..=0xaf); // 浮点 binop（显式报错）
    v.extend(0xb0..=0xcf); // binop/2addr
    v.extend(0xd0..=0xd7); // binop/lit16
    v.extend(0xd8..=0xe2); // binop/lit8
    v
}

fn op_name(op: u8) -> &'static str {
    match op {
        0x00 => "nop",
        0x01..=0x09 => "move",
        0x0a..=0x0c => "move-result",
        0x0d => "move-exception",
        0x0e..=0x11 => "return",
        0x12..=0x19 => "const",
        0x1a..=0x1b => "const-string",
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
        0x2d..=0x30 => "cmp-float/double",
        0x31 => "cmp-long",
        0x32..=0x37 => "if-test",
        0x38..=0x3d => "if-testz",
        0x3e..=0x3f => "unused",
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
        0x60..=0x6d => "sget/sput",
        0x6e..=0x78 => "invoke",
        0x79..=0x7a => "invoke-custom",
        0x7b..=0x90 => "unary",
        0x91..=0xa6 => "binop",
        0xa7..=0xaf => "unused",
        0xb0..=0xcf => "binop/2addr",
        0xd0..=0xd7 => "binop/lit16",
        0xd8..=0xe2 => "binop/lit8",
        0xe3..=0xff => "unused",
        _ => "?",
    }
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("用法: scan_dex <classes.dex>");
        std::process::exit(1);
    });
    let data = std::fs::read(&path).expect("读取 DEX");
    let dex = match DexFile::parse(&data) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("解析失败: {:?}", e);
            std::process::exit(1);
        }
    };
    println!("== DEX {} ({}B) ==", path, data.len());
    println!(
        "strings={} types={} protos={} methods={} classes={}",
        dex.strings.len(),
        dex.types.len(),
        dex.protos.len(),
        dex.methods.len(),
        dex.class_count
    );

    // ---- 类与方法 ----
    let mut with_code = 0usize;
    let mut native = 0usize;
    for m in &dex.methods {
        if m.code_off >= 0 {
            with_code += 1;
        } else {
            native += 1;
        }
    }
    println!("方法：代码 {} / native {}", with_code, native);
    println!("\n-- 类清单（含方法数） --");
    let mut by_class: std::collections::BTreeMap<String, Vec<&kirax_dex::dex::Method>> =
        std::collections::BTreeMap::new();
    for m in &dex.methods {
        by_class.entry(m.class.clone()).or_default().push(m);
    }
    for (c, ms) in &by_class {
        println!("  {}  ({} 方法)", c, ms.len());
        for m in ms.iter().take(8) {
            println!(
                "      {} {} {} {}",
                if m.code_off >= 0 { "[code]" } else { "[native]" },
                m.name,
                m.shorty,
                if m.code_off >= 0 { "" } else { "(native)" }
            );
        }
        if ms.len() > 8 {
            println!("      ... 等 {} 个", ms.len());
        }
    }

    // ---- opcode 分布 ----
    let supported = supported_ops();
    let mut hist: std::collections::BTreeMap<u8, usize> = std::collections::BTreeMap::new();
    let mut missing: std::collections::BTreeMap<u8, usize> = std::collections::BTreeMap::new();
    let mut total_insns = 0usize;
    for m in &dex.methods {
        if m.code_off < 0 {
            continue;
        }
        let Some((insns, _r, _i, _o)) = code_insns(&data, m.code_off as u32) else {
            continue;
        };
        // 逐条解码（只做长度跳过，不解语义，未支持也按规范长度跳）
        // 载荷（fill-array-data/packed/sparse-switch）是数据区：R8 常把多个载荷
        // 集中放在方法尾部，若扫到 0x26/0x2b/0x2c 立即"跳载荷末尾"会跳过中间
        // 真实指令。正确做法：记录载荷区间，顺序扫到该区间时再跳过。
        let mut pc = 0usize;
        let mut pl_ranges: Vec<(usize, usize)> = Vec::new();
        while pc + 2 <= insns.len() {
            if let Some(&(s, e)) = pl_ranges.iter().find(|&&(s, e)| pc >= s && pc < e) {
                pc = e;
                continue;
            }
            let w = u16::from_le_bytes([insns[pc], insns[pc + 1]]);
            let op = (w & 0xff) as u8;
            total_insns += 1;
            *hist.entry(op).or_insert(0) += 1;
            if !supported.contains(&op) {
                *missing.entry(op).or_insert(0) += 1;
            }
            if matches!(op, 0x26 | 0x2b | 0x2c) {
                if let Some((s, e)) = payload_range(&insns, pc, op) {
                    if s >= pc && e <= insns.len() && !pl_ranges.iter().any(|&(a, b)| s < b && a < e)
                    {
                        pl_ranges.push((s, e));
                    }
                }
            }
            let units = inst_units(op, insns, pc);
            if units == 0 {
                break;
            }
            pc += units * 2;
        }
    }
    println!("\n-- 指令统计：共 {} 条 --", total_insns);
    for (op, n) in &hist {
        println!(
            "  0x{:02x} {:<22} x{} {}",
            op,
            op_name(*op),
            n,
            if supported.contains(op) { "" } else { " << 未支持" }
        );
    }

    // ---- 幻影 opcode 实证：dump 0x39/0x3a 的指令字节上下文 ----
    let probe_ops = [0xe3u8, 0x73, 0x79];
    let mut dumped = std::collections::BTreeMap::new();
    for m in &dex.methods {
        if m.code_off < 0 {
            continue;
        }
        let Some((insns, _r, _i, _o)) = code_insns(&data, m.code_off as u32) else {
            continue;
        };
        let mut pc = 0usize;
        let mut pl_ranges: Vec<(usize, usize)> = Vec::new();
        while pc + 2 <= insns.len() {
            if let Some(&(s, e)) = pl_ranges.iter().find(|&&(s, e)| pc >= s && pc < e) {
                pc = e;
                continue;
            }
            let w = u16::from_le_bytes([insns[pc], insns[pc + 1]]);
            let op = (w & 0xff) as u8;
            if probe_ops.contains(&op) && dumped.get(&op).map_or(true, |n: &usize| *n < 4) {
                let ctx_start = pc.saturating_sub(48);
                let ctx_end = (pc + 64).min(insns.len());
                let mut hex = String::new();
                for b in &insns[ctx_start..ctx_end] {
                    hex.push_str(&format!("{:02x} ", b));
                }
                let entry = dumped.entry(op).or_insert(0usize);
                if *entry < 4 {
                    println!(
                        "\n[op=0x{:02x} 证据] {}  pc={}  prev/self/next 字节：{}",
                        op,
                        format!("{}.{}{}", m.class, m.name, m.shorty),
                        pc,
                        hex
                    );
                }
                *entry += 1;
            }
            if matches!(op, 0x26 | 0x2b | 0x2c) {
                if let Some((s, e)) = payload_range(&insns, pc, op) {
                    if s >= pc && e <= insns.len() && !pl_ranges.iter().any(|&(a, b)| s < b && a < e)
                    {
                        pl_ranges.push((s, e));
                    }
                }
            }
            let units = inst_units(op, insns, pc);
            if units == 0 {
                break;
            }
            pc += units * 2;
        }
    }

    if missing.is_empty() {
        println!("\n结论：全部 opcode 均在解释器支持集内 ✓");
    } else {
        println!("\n结论：{} 种 opcode 未支持：", missing.len());
        for (op, n) in &missing {
            println!("  0x{:02x} {} x{}", op, op_name(*op), n);
        }
    }
}

/// switch / fill-array-data 的载荷数据区间（字节）；非载荷指令返回 None。
/// 返回 (载荷起始, 载荷对齐末尾)；off（word1）以 code unit 计、相对指令起始。
fn payload_range(insns: &[u8], pc: usize, op: u8) -> Option<(usize, usize)> {
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
            // packed-switch：ident + size(u16) + first_key(int) + size × u32 targets
            // 头部 8 字节（ident 2 + size 2 + first_key 4）
            let sz = u16::from_le_bytes([insns[start + 2], insns[start + 3]]) as usize;
            8 + sz * 4
        }
        0x0200 => {
            // sparse-switch：ident + size + size × (u32 key, u32 target)
            let sz = u16::from_le_bytes([insns[start + 2], insns[start + 3]]) as usize;
            4 + sz * 8
        }
        0x0300 => {
            // fill-array-data：ident + element_width(u16) + size(u32) + data
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

/// 指令长度（code units）；未知返回 0
fn inst_units(op: u8, insns: &[u8], pc: usize) -> usize {
    let w1 = |p: usize| -> u16 {
        if p + 2 <= insns.len() {
            u16::from_le_bytes([insns[p], insns[p + 1]])
        } else {
            0
        }
    };
    match op {
        0x00 => 1,            // nop
        0x01..=0x09 => {
            // move：03/06/09 是 32x(3)；02/05/08 是 22x(2)；其余 12x(1)
            match op {
                0x03 | 0x06 | 0x09 => 3,
                0x02 | 0x05 | 0x08 => 2,
                _ => 1,
            }
        }
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
        0x1c => 2, // const-class 21c
        0x1d | 0x1e => 1, // monitor-enter/exit 11x
        0x1f => 2, // check-cast 21c
        0x20 => 2, // instance-of（22c 2 units：A=word0 低 4、B=高 4，word1=type_idx）
        0x21 => 1, // array-length 12x
        0x22 => 2, // new-instance 21c
        0x23 => 2, // new-array（22c 2 units：A=word0 低 4、B=高 4，word1=type_idx）
        0x24 => 3, // filled-new-array 35c
        0x25 => 3, // filled-new-array/range 3rc
        0x26 => 3, // fill-array-data 31t
        0x27 => 1, // throw 11x
        0x28 => 1, // goto 10t
        0x29 => 2, // goto/16 20t
        0x2a => 3, // goto/32 30t
        0x2b | 0x2c => 3, // packed/sparse-switch 31t
        0x2d..=0x31 => 2, // cmp（23x 2 units：A=word0 低 8，B=word1 低字节、C=高字节）
        0x32..=0x37 => 2, // if-test 22t
        0x38..=0x3d => 2, // if-testz 21t
        0x3e..=0x3f => 1, // unused
        0x40..=0x51 => 2, // aget/aput（23x 2 units：A=word0 低 8，B=word1 低字节、C=高字节）
        0x52..=0x5f => 2, // iget/iput（22c 2 units：A=word0 低 4、B=高 4，word1=field_idx 16）
        0x60..=0x6d => 2, // sget/sput 21c
        0x6e..=0x72 => 3, // invoke-kind 35c
        0x73 => 1,        // unused
        0x74..=0x78 => 3, // invoke-kind/range 3rc
        0x79..=0x7a => 4, // invoke-custom 45cc / 4rc
        0x7b..=0x8f => 1, // 一元/转换 12x
        0x90 => 2,        // add-int（23x 2 units：A=word0 低 8，B=word1 低字节、C=高字节）
        0x91..=0xaf => 2, // binop（23x 2 units：A=word0 低 8，B=word1 低字节、C=高字节）
        0xb0..=0xcf => 1, // binop/2addr 12x
        0xd0..=0xd7 => 2, // lit16 22s（官方：A 4 位、B 4 位、C 16 位 = 2 code units）
        0xd8..=0xe2 => 2, // lit8 22b
        0xe3..=0xff => 1, // unused / 040 新增（const-method-* 等 2 units）
        _ => {
            let _ = w1;
            0
        }
    }
}
