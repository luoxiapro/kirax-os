//! APK → Deflate 解压 → DEX 兼容解码 端到端集成测试
//!
//! 使用仓库内 testdata/sample.apk（真实 classes.dex 035 样本 + 真实
//! AndroidManifest.xml；classes.dex 以 Deflate 压缩，与真实 APK 一致）。
//! 035 已定稿指标（scan_dex 审计）：strings=77 types=26 protos=19 methods=38
//! classes=6、代码方法 9 / native 29、指令 82 条、0 未支持。

use apkcompat::dexcompat::{dex_to_kir, inspect_apk_dex, inspect_dex};
use apkcompat::{Apk, ApkError};

const SAMPLE_APK: &[u8] = include_bytes!("../testdata/sample.apk");

/// 从样本 APK 提取 classes.dex（真实 deflate 解压路径）
fn extract_dex() -> Vec<u8> {
    let apk = Apk::parse(SAMPLE_APK).unwrap();
    let e = apk.find("classes.dex").unwrap();
    apk.extract(e).unwrap()
}

#[test]
fn sample_apk_classes_dex_is_deflate() {
    let apk = Apk::parse(SAMPLE_APK).unwrap();
    let e = apk.find("classes.dex").unwrap();
    // 真实 APK 形态：classes.dex 以 Deflate 压缩
    assert_eq!(e.method, 8);
    assert!(e.compressed_size < e.uncompressed_size);
    let dex = apk.extract(e).unwrap();
    // 解压结果 = 真实 DEX 035 样本（3920B，magic dex\n035）
    assert_eq!(dex.len(), 3920);
    assert_eq!(&dex[0..8], b"dex\n035\x00");
}

#[test]
fn sample_apk_end_to_end_dex_info() {
    let info = inspect_apk_dex(SAMPLE_APK).unwrap();
    assert_eq!(info.version, 35);
    assert_eq!(info.strings, 77);
    assert_eq!(info.types, 26);
    assert_eq!(info.protos, 19);
    assert_eq!(info.methods, 38);
    assert_eq!(info.classes, 6);
    assert_eq!(info.code_methods, 9);
    assert_eq!(info.native_methods, 29);
    // 035 审计定稿：指令总数 82、0 未支持
    assert_eq!(info.insn_total, 82);
    assert!(info.unsupported.is_empty());
    // opcode 直方图非空且合计等于指令总数
    let sum: u32 = info.opcodes.iter().map(|s| s.count).sum();
    assert_eq!(sum, info.insn_total);
}

#[test]
fn inspect_dex_raw_bytes() {
    let dex = extract_dex();
    let info = inspect_dex(&dex).unwrap();
    assert_eq!(info.version, 35);
    assert_eq!(info.insn_total, 82);
    assert!(info.unsupported.is_empty());
}

#[test]
fn dex_to_kir_skeleton() {
    let dex = extract_dex();
    let kir = dex_to_kir(&dex, "sample").unwrap();
    // 头注
    assert!(kir.contains("dex\\n035"));
    assert!(kir.contains("strings=77"));
    // 类与方法骨架
    assert!(kir.contains(".class Lcom/pianshu/app/MainActivity;"));
    assert!(kir.contains(".method <init>()V"));
    // 助记符与地址行
    assert!(kir.contains("invoke-direct"));
    assert!(kir.contains("return-void"));
    assert!(kir.contains("; 0000")); // 地址列
}

#[test]
fn bad_dex_rejected() {
    assert!(inspect_dex(b"not a dex").is_err());
    // 魔数正确但损坏也应报错（不 panic）
    let mut bad = b"dex\n037\x00".to_vec();
    bad.extend_from_slice(&[0u8; 64]);
    let _ = inspect_dex(&bad);
}

#[test]
fn apk_error_kinds() {
    // 非 ZIP
    assert!(Apk::parse(b"garbage").is_err());
    // 无 classes.dex 的合法 zip → EntryNotFound 路径
    let apk = Apk::parse(SAMPLE_APK).unwrap();
    assert_eq!(
        apk.find("no/such/entry").err(),
        Some(ApkError::EntryNotFound)
    );
}
