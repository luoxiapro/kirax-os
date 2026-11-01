//! APK 兼容器 —— 主机单测
//!
//! 构造真实的 ZIP/APK 字节（Store 方法）与二进制 AndroidManifest.xml，
//! 验证解析器的完整链路：EOCD → 中心目录 → Local Header → 文件提取
//! → AXML package/label 提取。

use apkcompat::{inspect_apk, Apk, ApkError};

// ============================================================================
// ZIP 构造工具（测试用，Store 方法）
// ============================================================================

/// 构造一个 Store 方法 ZIP 文件
fn make_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    let mut offset = 0u32;

    for (name, data) in files {
        // ---- Local File Header ----
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes()); // sig
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method = store
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0u16.to_le_bytes()); // date
        out.extend_from_slice(&0u32.to_le_bytes()); // crc
        out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // csize
        out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // usize
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        // ---- 中心目录条目 ----
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes()); // sig
        central.extend_from_slice(&20u16.to_le_bytes()); // version made
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&0u16.to_le_bytes()); // method
        central.extend_from_slice(&0u16.to_le_bytes()); // time
        central.extend_from_slice(&0u16.to_le_bytes()); // date
        central.extend_from_slice(&0u32.to_le_bytes()); // crc
        central.extend_from_slice(&(data.len() as u32).to_le_bytes()); // csize
        central.extend_from_slice(&(data.len() as u32).to_le_bytes()); // usize
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk start
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attr
        central.extend_from_slice(&0u32.to_le_bytes()); // external attr
        central.extend_from_slice(&offset.to_le_bytes()); // local header offset
        central.extend_from_slice(name.as_bytes());

        offset += 30 + name.len() as u32 + data.len() as u32;
    }

    // ---- EOCD ----
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes()); // sig
    out.extend_from_slice(&0u16.to_le_bytes()); // disk
    out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
    out.extend_from_slice(&(files.len() as u16).to_le_bytes()); // disk entries
    out.extend_from_slice(&(files.len() as u16).to_le_bytes()); // total
    out.extend_from_slice(&(central.len() as u32).to_le_bytes()); // cd size
    out.extend_from_slice(&cd_offset.to_le_bytes()); // cd offset
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len

    out
}

// ============================================================================
// 二进制 AndroidManifest.xml 构造工具
// ============================================================================

/// 构造一个最小 AXML：StringPool（UTF-16）+ manifest StartElement（package 属性）
fn make_manifest_axml(package: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    // 文件头（ResChunk_header）：type(u16)=0x0003 + headerSize(u16)=8 + size(u32)
    out.extend_from_slice(&0x0003u16.to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // file_size（后面补）

    // ---- StringPool ----
    // 字符串：0="manifest" 1=package 2=<package 值> 3="application"
    let strs: Vec<&str> = vec!["manifest", "package", package, "application"];
    let mut pool: Vec<u8> = Vec::new();
    let mut offsets: Vec<u32> = Vec::new();
    let mut cur = 0u32;
    let mut utf16_data: Vec<u16> = Vec::new();
    for s in &strs {
        offsets.push(cur);
        let v: Vec<u16> = s.encode_utf16().collect();
        let l = v.len() as u32;
        utf16_data.push(l as u16); // 长度前缀
        utf16_data.extend_from_slice(&v); // 字符
        utf16_data.push(0); // null 结尾
        cur += 1 + l + 1; // len + chars + null
    }
    let strings_start = 28 + offsets.len() * 4;
    // 布局：type@0(4) headerSize@4(2)=28 size@6(4) stringCount@10(4)
    //       styleCount@14(4) flags@18(4) stringsStart@22(4) stylesStart@26(4) offsets@28
    pool.extend_from_slice(&CHUNK_STRING.to_le_bytes()); // type@0 (u16)
    pool.extend_from_slice(&28u16.to_le_bytes()); // header_size@2
    // size = 头(8) + 字段(20: count/style/flags/stringsStart/stylesStart) + offsets + 数据
    pool.extend_from_slice(&((28 + offsets.len() * 4 + utf16_data.len() * 2) as u32).to_le_bytes()); // size@4
    pool.extend_from_slice(&(strs.len() as u32).to_le_bytes()); // string_count@10
    pool.extend_from_slice(&0u32.to_le_bytes()); // style_count@14
    pool.extend_from_slice(&0u32.to_le_bytes()); // flags@18 (UTF-16)
    pool.extend_from_slice(&(strings_start as u32).to_le_bytes()); // stringsStart@22
    pool.extend_from_slice(&0u32.to_le_bytes()); // stylesStart@26
    for o in &offsets {
        pool.extend_from_slice(&o.to_le_bytes());
    }
    for c in &utf16_data {
        pool.extend_from_slice(&c.to_le_bytes());
    }
    out.extend_from_slice(&pool);

    // ---- manifest StartElement ----
    // ResXMLTree_startElement（标准布局）：
    //   type@0(u16)=0x0102 headerSize@2(u16)=16 size@4(u32)
    //   lineNumber@8(4) comment@12(4) ns@16(4) name@20(4)
    //   attributeStart@24(2) attributeSize@26(2) attributeCount@28(2)
    //   idIndex@30(2) classIndex@32(2) styleIndex@34(2) 属性表@36
    let attr_count: u16 = 1;
    let attr_start: u16 = 36; // 属性表相对 element 起始的偏移
    let el_chunk_size: u32 = 36 + attr_count as u32 * 20;
    out.extend_from_slice(&0x0102u16.to_le_bytes()); // type@0
    out.extend_from_slice(&16u16.to_le_bytes()); // headerSize@2
    out.extend_from_slice(&el_chunk_size.to_le_bytes()); // size@4
    out.extend_from_slice(&0u32.to_le_bytes()); // lineNumber@8
    out.extend_from_slice(&0u32.to_le_bytes()); // comment@12
    out.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // ns@16
    out.extend_from_slice(&0u32.to_le_bytes()); // name@20 = "manifest"
    out.extend_from_slice(&attr_start.to_le_bytes()); // attributeStart@24
    out.extend_from_slice(&20u16.to_le_bytes()); // attributeSize@26
    out.extend_from_slice(&attr_count.to_le_bytes()); // attributeCount@28
    out.extend_from_slice(&0u16.to_le_bytes()); // idIndex@30
    out.extend_from_slice(&0u16.to_le_bytes()); // classIndex@32
    out.extend_from_slice(&0u16.to_le_bytes()); // styleIndex@34

    // 属性 0：ns=0xFFFFFFFF，name=1("package")，rawValue=2(<package>)，typedValue 8 字节
    out.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // ns
    out.extend_from_slice(&1u32.to_le_bytes()); // name idx
    out.extend_from_slice(&2u32.to_le_bytes()); // rawValue idx
    out.extend_from_slice(&0x0100_0003u32.to_le_bytes()); // typedValue: size=1,dataType=3(string)
    out.extend_from_slice(&2u32.to_le_bytes()); // data = string idx 2
    // 属性共 20 字节（ns+name+rawValue+typedValue8）

    // 补 file_size
    let size = out.len() as u32;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    out
}

const CHUNK_STRING: u16 = 0x0001;

// ============================================================================
// 测试
// ============================================================================

#[test]
fn zip_eocd_and_extract() {
    let zip = make_zip(&[("hello.txt", b"Hello, Space OS!"), ("res/mipmap/ic.png", b"PNGDATA")]);
    let apk = Apk::parse(&zip).unwrap();
    let names = apk.names();
    assert!(names.contains(&"hello.txt"));
    assert!(names.contains(&"res/mipmap/ic.png"));
    let e = apk.find("hello.txt").unwrap();
    let data = apk.extract(e).unwrap();
    assert_eq!(data, b"Hello, Space OS!");
}

#[test]
fn zip_bad_magic() {
    assert_eq!(Apk::parse(b"not a zip at all").err(), Some(ApkError::NoEocd));
}

#[test]
fn zip_entry_not_found() {
    let zip = make_zip(&[("a.txt", b"x")]);
    let apk = Apk::parse(&zip).unwrap();
    assert_eq!(apk.find("missing.txt").err(), Some(ApkError::EntryNotFound));
}

#[test]
fn manifest_package_extracted() {
    let axml = make_manifest_axml("com.spaceos.demo");
    let info = apkcompat::parse_manifest_xml(&axml).unwrap();
    assert_eq!(info.package, "com.spaceos.demo");
}

#[test]
fn inspect_full_apk() {
    // 构造一个最小 APK：AndroidManifest.xml + 图标
    let axml = make_manifest_axml("com.example.hello");
    let zip = make_zip(&[
        ("AndroidManifest.xml", &axml),
        ("classes.dex", b"dex\n035\x00".to_vec().as_slice()),
        ("res/mipmap-mdpi/ic_launcher.png", b"\x89PNGfake"),
    ]);
    let app = inspect_apk(&zip).unwrap();
    assert_eq!(app.package, "com.example.hello");
    assert!(app.icon_png.starts_with(b"\x89PNG"));
    // label 无资源引用时回退到 package
    assert_eq!(app.label, "com.example.hello");
}

#[test]
fn inspect_missing_manifest() {
    let zip = make_zip(&[("classes.dex", b"x")]);
    assert!(inspect_apk(&zip).is_err());
}
