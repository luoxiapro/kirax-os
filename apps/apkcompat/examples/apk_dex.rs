//! APK/DEX 兼容解码演示入口（host 可运行，真实可复核）
//!
//! 用法：
//!   cargo run --example apk_dex -- <sample.apk|classes.dex> [--kir]
//!
//! 对 APK：ZIP → Deflate 解压 classes.dex → DEX 解码报告
//! 对 DEX：直接 DEX 解码报告
//! 加 --kir：另输出方法级 .kir 转译骨架（AOT 方向）

use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("用法: apk_dex <apk|dex> [--kir]");
    let want_kir = args.iter().any(|a| a == "--kir");
    let data = fs::read(path).expect("读取文件失败");

    let is_dex = data.len() >= 8 && &data[0..4] == b"dex\n";
    let info = if is_dex {
        println!("== DEX {} ({}B) ==", path, data.len());
        apkcompat::dexcompat::inspect_dex(&data).expect("DEX 解析失败")
    } else {
        println!("== APK {} ({}B) ==", path, data.len());
        let apk = apkcompat::Apk::parse(&data).expect("APK 解析失败");
        println!("-- APK 条目（前 12） --");
        for n in apk.names().iter().take(12) {
            println!("  {n}");
        }
        println!("\n-- classes.dex 解码 --");
        apkcompat::dexcompat::inspect_apk_dex(&data).expect("APK DEX 解析失败")
    };

    println!(
        "DEX v{}  strings={} types={} protos={} methods={} classes={}",
        info.version, info.strings, info.types, info.protos, info.methods, info.classes
    );
    println!(
        "方法：代码 {} / native {}    指令总数 {}",
        info.code_methods, info.native_methods, info.insn_total
    );
    if info.unsupported.is_empty() {
        println!("结论：全部 opcode 均在解释器支持集内 ✓");
    } else {
        println!("结论：{} 种 opcode 未支持：", info.unsupported.len());
        for (op, m, n) in &info.unsupported {
            println!("  0x{op:02x} {m} x{n}");
        }
    }
    println!("\n-- opcode 直方图 --");
    for s in &info.opcodes {
        println!(
            "  0x{:02x} {:<24} x{}{}",
            s.opcode,
            s.mnemonic,
            s.count,
            if s.supported { "" } else { "  << 未支持" }
        );
    }

    if want_kir {
        let kir_data = if is_dex {
            data.clone()
        } else {
            let apk = apkcompat::Apk::parse(&data).expect("APK 解析失败");
            let e = apk.find("classes.dex").expect("APK 中无 classes.dex");
            apk.extract(e).expect("classes.dex 提取/解压失败")
        };
        let kir = apkcompat::dexcompat::dex_to_kir(&kir_data, path).expect(".kir 转译骨架失败");
        println!("\n== .kir 转译骨架（前 80 行，共 {} 行） ==", kir.lines().count());
        for l in kir.lines().take(80) {
            println!("{l}");
        }
    }
}
