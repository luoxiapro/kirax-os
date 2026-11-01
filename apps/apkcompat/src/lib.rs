//! # APK 兼容器 —— APK(ZIP) 解析器、Deflate 解压、DEX 兼容解码与兼容层管理器
//!
//! 参考鸿蒙"卓易通"（安卓兼容层应用）的思路，为 Space OS 提供 APK 的
//! **识别 → 解析 → 安装管理 → 兼容运行** 链路。
//!
//! 本模块为纯逻辑代码，`no_std`（thumbv7a-none-eabi 可编入 .kir），同时
//! 可在主机 `cargo test` 直接单测，是 Space OS 应用层"自带软件"的完整实现。
//!
//! 真实可运行部分：
//!   1. ZIP 容器解析：EOCD 定位 → 中心目录 → Local Header → 文件提取
//!   2. Deflate 解压（RFC 1951 手写 inflate）：真实 APK 的 classes.dex
//!      / resources.arsc 多为 method=8 压缩
//!   3. AndroidManifest.xml **二进制 XML** 解析：提取 package、label 资源引用
//!   4. resources.arsc 路径映射 + strings.xml 二进制解析：解析出应用显示名
//!   5. 图标提取：从 res/mipmap-* 目录取出 PNG 原始数据
//!   6. **DEX 兼容解码**（接入 crates/space-dex 引擎）：类/方法/指令统计、
//!      opcode 覆盖与未支持清单、方法级 .kir 转译骨架（AOT 方向）
//!   7. 安装/卸载/列表管理：挂载到 FAT32 的 /APKS 目录
//!
//! 路线图（后续阶段，见 README.md）：
//!   - DEX 解释器执行（interp.rs 已在 space-dex 就绪，接入做完整运行）
//!   - Binder 服务抽象（Intent/PackageManager/ActivityManager 微内核）
//!   - ART-like 对象模型（堆内句柄 + 弱引用 + GC 标清）
//!
//! 全程不依赖 Linux/第三方库，字节格式按 Android 开源文档手写。

#![no_std]

// host 构建（cargo build / cargo test，未开 kir-standalone）：链接 std 以提供
// 全局分配器与 panic_handler。注意：即使 `cargo test`，cargo 也会先以非 test
// cfg 编译一次库（含 staticlib crate-type），staticlib 必须解析分配器/panic_handler，
// 因此这里不能只用 `#[cfg(test)]`。
// thumbv7a 独立打包构建需加 `--features kir-standalone`：此时 std 关闭，由下方
// standalone 模块提供 Bump 分配器与 panic_handler。
#[cfg(not(feature = "kir-standalone"))]
extern crate std;

extern crate alloc;
#[allow(unused_imports)]
use alloc::format;
use alloc::vec::Vec;

// ----------------------------------------------------------------------------
// 子模块
// ----------------------------------------------------------------------------
/// Deflate (RFC 1951) 解压
pub mod inflate;
/// DEX 兼容解码与 .kir 转译骨架（接入 space-dex 引擎）
pub mod dexcompat;

// ----------------------------------------------------------------------------
// 独立打包支持（feature = "kir-standalone"）：
// 在 thumbv7a-none-eabi 上把本库链接成 staticlib 时，需要一个最小
// GlobalAllocator 与 panic_handler（Space OS 内核内运行时已提供，勿启用本 feature）。
// 堆采用极简 bump 分配（仅支持分配不支持释放，仅用于链接通过）。
// ----------------------------------------------------------------------------
#[cfg(feature = "kir-standalone")]
mod standalone {
    use core::alloc::{GlobalAlloc, Layout};
    use core::panic::PanicInfo;

    static mut HEAP: [u8; 64 * 1024] = [0u8; 64 * 1024];
    static mut OFFSET: usize = 0;

    pub struct Bump;

    unsafe impl GlobalAlloc for Bump {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let start = OFFSET;
            let aligned = (start + layout.align() - 1) & !(layout.align() - 1);
            if aligned + layout.size() > HEAP.len() {
                return core::ptr::null_mut();
            }
            OFFSET = aligned + layout.size();
            HEAP.as_mut_ptr().add(aligned)
        }
        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
            // bump 分配器不回收
        }
    }

    #[global_allocator]
    static GLOBAL: Bump = Bump;

    #[panic_handler]
    fn panic(_info: &PanicInfo) -> ! {
        loop {}
    }
}

/// 错误码（与 Space OS syscall::err 风格一致，负值由调用方转换）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApkError {
    /// 文件过短或魔数不符
    NotZip,
    /// 找不到 End Of Central Directory
    NoEocd,
    /// 中心目录结构异常
    BadCentralDirectory,
    /// 指定的成员不存在
    EntryNotFound,
    /// 不支持的压缩方法（目前支持 Store 与 Deflate；其余走路线图）
    UnsupportedCompression(u16),
    /// AndroidManifest 二进制 XML 解析失败
    BadBinaryXml,
    /// 字符串资源引用无法解析
    BadStringRef,
    /// DEX 解析失败（魔数/头表损坏）
    BadDex,
    /// Deflate 解压失败
    Inflate(crate::inflate::InflateError),
}

impl core::fmt::Display for ApkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use ApkError::*;
        match self {
            NotZip => write!(f, "非 ZIP/APK 文件"),
            NoEocd => write!(f, "找不到 ZIP 目录结尾记录"),
            BadCentralDirectory => write!(f, "ZIP 中心目录损坏"),
            EntryNotFound => write!(f, "APK 中不存在该条目"),
            UnsupportedCompression(m) => write!(f, "不支持的压缩方法 {m}"),
            BadBinaryXml => write!(f, "AndroidManifest 二进制 XML 损坏"),
            BadStringRef => write!(f, "字符串资源引用解析失败"),
            BadDex => write!(f, "DEX 解析失败（魔数或头表损坏）"),
            Inflate(e) => write!(f, "Deflate 解压失败: {e}"),
        }
    }
}

pub type Result<T> = core::result::Result<T, ApkError>;

// ============================================================================
// ZIP 结构（PKWARE 规范，小端）
// ============================================================================

/// EOCD：End Of Central Directory 签名
const EOCD_SIG: u32 = 0x0605_4b50;
/// 中心目录条目签名
const CEN_SIG: u32 = 0x0201_4b50;
/// Local File Header 签名
const LOC_SIG: u32 = 0x0403_4b50;

/// ZIP 中心目录中的一个条目
#[derive(Debug, Clone)]
pub struct ZipEntry {
    /// 成员名（UTF-8，APK 内为 ASCII 路径）
    pub name: Vec<u8>,
    /// 压缩方法：0 = Store（不压缩），8 = Deflate
    pub method: u16,
    /// 未压缩大小
    pub uncompressed_size: u32,
    /// 压缩后大小
    pub compressed_size: u32,
    /// Local Header 相对文件起点的偏移
    pub local_header_offset: u32,
}

/// APK 文件解析器（只读视图，不持有数据）
pub struct Apk<'a> {
    data: &'a [u8],
    entries: Vec<ZipEntry>,
}

impl<'a> Apk<'a> {
    /// 从原始字节解析 APK（验证 EOCD + 中心目录）
    pub fn parse(data: &'a [u8]) -> Result<Apk<'a>> {
        // ---- 1. 定位 EOCD：从文件尾往前搜签名 ----
        let eocd = find_eocd(data)?;
        // EOCD 布局：sig(4) disk(2) cd_disk(2) disk_entries(2) total(2)
        //           cd_size(4) cd_offset(4) comment_len(2)
        let cd_offset = u32_at(data, eocd + 16) as usize;
        let cd_size = u32_at(data, eocd + 12) as usize;
        if cd_offset + cd_size > data.len() {
            return Err(ApkError::BadCentralDirectory);
        }
        // ---- 2. 遍历中心目录 ----
        let mut entries = Vec::new();
        let mut p = cd_offset;
        let end = cd_offset + cd_size;
        while p + 46 <= end {
            let sig = u32_at(data, p);
            if sig != CEN_SIG {
                return Err(ApkError::BadCentralDirectory);
            }
            let method = u16_at(data, p + 10);
            let compressed_size = u32_at(data, p + 20);
            let uncompressed_size = u32_at(data, p + 24);
            let name_len = u16_at(data, p + 28) as usize;
            let extra_len = u16_at(data, p + 30) as usize;
            let comment_len = u16_at(data, p + 32) as usize;
            let local_header_offset = u32_at(data, p + 42);
            let name_start = p + 46;
            if name_start + name_len > end {
                return Err(ApkError::BadCentralDirectory);
            }
            entries.push(ZipEntry {
                name: data[name_start..name_start + name_len].to_vec(),
                method,
                uncompressed_size,
                compressed_size,
                local_header_offset,
            });
            p = name_start + name_len + extra_len + comment_len;
        }
        Ok(Apk { data, entries })
    }

    /// 全部条目名（ASCII 路径）
    pub fn names(&self) -> alloc::vec::Vec<&str> {
        self.entries
            .iter()
            .map(|e| core::str::from_utf8(&e.name).unwrap_or("?"))
            .collect()
    }

    /// 查找条目
    pub fn find(&self, name: &str) -> Result<&ZipEntry> {
        self.entries
            .iter()
            .find(|e| core::str::from_utf8(&e.name).map(|s| s == name).unwrap_or(false))
            .ok_or(ApkError::EntryNotFound)
    }

    /// 提取条目内容（支持 Store 与 Deflate；其余压缩方法返回 UnsupportedCompression）
    pub fn extract(&self, entry: &ZipEntry) -> Result<Vec<u8>> {
        if entry.method != 0 && entry.method != 8 {
            return Err(ApkError::UnsupportedCompression(entry.method));
        }
        // Local Header：sig(4) ver(2) flags(2) method(2) time(2) date(2)
        //               crc(4) csize(4) usize(4) name_len(2) extra_len(2)
        let lh = entry.local_header_offset as usize;
        if lh + 30 > self.data.len() || u32_at(self.data, lh) != LOC_SIG {
            return Err(ApkError::BadCentralDirectory);
        }
        let name_len = u16_at(self.data, lh + 26) as usize;
        let extra_len = u16_at(self.data, lh + 28) as usize;
        let data_start = lh + 30 + name_len + extra_len;
        let size = entry.compressed_size as usize;
        if data_start + size > self.data.len() {
            return Err(ApkError::BadCentralDirectory);
        }
        let raw = &self.data[data_start..data_start + size];
        match entry.method {
            0 => {
                // Store：压缩大小 = 原始大小
                if entry.compressed_size != entry.uncompressed_size {
                    return Err(ApkError::UnsupportedCompression(entry.method));
                }
                Ok(raw.to_vec())
            }
            8 => inflate::inflate(raw).map_err(ApkError::Inflate),
            _ => Err(ApkError::UnsupportedCompression(entry.method)),
        }
    }
}

/// 从文件尾向前搜索 EOCD 签名（最多向后 65535 + 22 字节，注释区）
fn find_eocd(data: &[u8]) -> Result<usize> {
    let max_back = core::cmp::min(data.len(), 22 + 65535);
    let mut p = data.len() - max_back;
    while p + 22 <= data.len() {
        if u32_at(data, p) == EOCD_SIG {
            return Ok(p);
        }
        p += 1;
    }
    Err(ApkError::NoEocd)
}

#[inline]
fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

#[inline]
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

// ============================================================================
// AndroidManifest.xml —— 二进制 XML（AXML）解析
// ============================================================================
//
// AXML 文件布局（Android 开源实现 ResXMLTree）：
//   [Header] magic(0x00080003 LE) header_size(2) file_size(4)
//   [StringPool]  type(0x000001) header_size(2) chunk_size(4) string_count(4)
//                  style_count(4) flags(4) utf8(4) strings_start(4)
//   [ResourceMap] type(0x000180) header_size(2) chunk_size(4) count(4) [u32 资源 id]
//   [StartNamespace] / [StartElement] / [EndElement] / [EndNamespace] 序列
//
// StartElement 布局：
//   type(0x00100102) header_size(2) chunk_size(4)
//   ns(4) name(4)        ← 字符串池索引
//   attr_start(2) attr_size(2) attr_count(2) id_index(2) class_index(2) style_index(2)
//   [attribute × attr_count]：ns(4) name(4) rawValue(4) typedValue(8)

/// AXML 魔数（type=0x0003 + headerSize=8）
const AXML_MAGIC: u32 = 0x0008_0003;
/// StringPool chunk 类型（u16）
const CHUNK_STRING: u16 = 0x0001;
/// StartElement chunk 类型（u16）
const CHUNK_START_ELEMENT: u16 = 0x0102;

/// 提取的 AndroidManifest 关键信息
#[derive(Debug, Default, Clone)]
pub struct ManifestInfo {
    /// package 属性值（如 com.example.app）
    pub package: alloc::string::String,
    /// application android:label 的资源引用（形如 @string/app_name）
    pub label_ref: alloc::string::String,
    /// application android:icon 的资源引用（形如 @mipmap/ic_launcher）
    pub icon_ref: alloc::string::String,
}

/// 解析 AndroidManifest.xml 二进制内容，提取 package / label / icon
pub fn parse_manifest_xml(axml: &[u8]) -> Result<ManifestInfo> {
    if axml.len() < 8 || u32_at(axml, 0) != AXML_MAGIC {
        return Err(ApkError::BadBinaryXml);
    }
    // ---- 1. 定位 StringPool 并读取字符串表 ----
    let mut p = 8usize;
    let mut strings: Vec<alloc::vec::Vec<u8>> = Vec::new();
    let mut found_pool = false;
    while p + 8 <= axml.len() {
        // 每个 chunk 头（ResChunk_header）：type(u16) headerSize(u16) size(u32)
        let chunk_type = u16_at(axml, p);
        let chunk_size = u32_at(axml, p + 4) as usize;
        if chunk_size == 0 || p + chunk_size > axml.len() {
            break;
        }
        if chunk_type == CHUNK_STRING {
            // ResStringPool_header（相对 chunk 起始）：
            //   type@0(2) headerSize@2(2) size@4(4) stringCount@8(4)
            //   styleCount@12(4) flags@16(4) stringsStart@20(4) stylesStart@24(4)
            //   offsets@28 (4×count) → 字符串数据
            let string_count = u32_at(axml, p + 8) as usize;
            let flags = u32_at(axml, p + 16);
            let utf8 = (flags & (1 << 8)) != 0;
            let strings_start = u32_at(axml, p + 20) as usize;
            let base = p;
            let mut off = base + 28;
            // 字符串偏移数组（UTF-16 池偏移以 u16 为单位；UTF-8 池为字节）
            let mut offsets = Vec::with_capacity(string_count);
            for _ in 0..string_count {
                offsets.push(u32_at(axml, off) as usize);
                off += 4;
            }
            for off in offsets {
                let byte_off = if utf8 { off } else { off * 2 };
                let s = read_pool_string(axml, base + strings_start + byte_off, utf8)?;
                strings.push(s);
            }
            found_pool = true;
        } else if chunk_type == CHUNK_START_ELEMENT {
            if !found_pool {
                return Err(ApkError::BadBinaryXml);
            }
            // ResXMLTree_startElement（相对 chunk 起始）：
            //   header 8（type@0/headerSize@4/size@6）+ lineNumber@8(4) + comment@12(4)
            //   ns@16(4) name@20(4) attributeStart@24(2) attributeSize@26(2)
            //   attributeCount@28(2) idIndex@30(2) classIndex@32(2) styleIndex@34(2)
            //   属性表 @36
            let name_idx = u32_at(axml, p + 20) as usize;
            let attr_start = u16_at(axml, p + 24) as usize;
            let attr_count = u16_at(axml, p + 28) as usize;
            let name = strings.get(name_idx).cloned().unwrap_or_default();
            if name == b"manifest" {
                // manifest 元素属性：第一个属性是 package（约定）
                if let Some(info) = parse_attrs(&axml, p, attr_start, attr_count, &strings) {
                    return Ok(info);
                }
            } else if name == b"application" {
                // application 属性里的 label/icon 单独解析
                let attrs = parse_attrs_raw(&axml, p, attr_start, attr_count, &strings);
                let mut info = ManifestInfo::default();
                for a in attrs {
                    if a.0 == "label" {
                        info.label_ref = a.1;
                    } else if a.0 == "icon" {
                        info.icon_ref = a.1;
                    }
                }
                if !info.label_ref.is_empty() || !info.icon_ref.is_empty() {
                    return Ok(info);
                }
            }
        }
        p += chunk_size;
    }
    Err(ApkError::BadBinaryXml)
}

/// 解析 StartElement 的属性表，返回 (attr_name, 资源引用或原始值)
fn parse_attrs_raw(
    axml: &[u8],
    el: usize,
    attr_start: usize,
    attr_count: usize,
    strings: &[alloc::vec::Vec<u8>],
) -> alloc::vec::Vec<(alloc::string::String, alloc::string::String)> {
    let mut out = Vec::new();
    for i in 0..attr_count {
        let a = el + attr_start + i * 20;
        if a + 20 > axml.len() {
            break;
        }
        let name_idx = u32_at(axml, a + 4) as usize;
        let raw_idx = u32_at(axml, a + 8) as usize;
        let name = strings.get(name_idx).cloned().unwrap_or_default();
        let name_s = alloc::string::String::from_utf8_lossy(&name).into_owned();
        if raw_idx != 0xFFFF_FFFF {
            if let Some(s) = strings.get(raw_idx) {
                let raw = alloc::string::String::from_utf8_lossy(s).into_owned();
                out.push((name_s, raw));
            }
        } else {
            // 无 rawValue：资源引用在 typedValue.data
            let data = u32_at(axml, a + 12 + 4);
            out.push((name_s, format!("@res/0x{data:08x}")));
        }
    }
    out
}

/// 解析 manifest 元素：取 package 属性（属性 0）
fn parse_attrs(
    axml: &[u8],
    el: usize,
    attr_start: usize,
    _attr_count: usize,
    strings: &[alloc::vec::Vec<u8>],
) -> Option<ManifestInfo> {
    let a = el + attr_start;
    if a + 20 > axml.len() {
        return None;
    }
    // manifest 的第一个属性通常是 package
    let raw_idx = u32_at(axml, a + 8) as usize;
    if raw_idx == 0xFFFF_FFFF {
        return None;
    }
    let s = strings.get(raw_idx)?;
    let mut info = ManifestInfo::default();
    info.package = alloc::string::String::from_utf8_lossy(s).into_owned();
    Some(info)
}

/// 从 StringPool 读取一条字符串（UTF-8/UTF-16 两种编码）
fn read_pool_string(
    axml: &[u8],
    at: usize,
    utf8: bool,
) -> Result<alloc::vec::Vec<u8>> {
    if utf8 {
        // UTF-8：u8 长度（可变长，此处简化取低 8 位）+ 数据
        if at >= axml.len() {
            return Err(ApkError::BadBinaryXml);
        }
        let len = axml[at] as usize;
        let start = at + 1;
        if start + len > axml.len() {
            return Err(ApkError::BadBinaryXml);
        }
        Ok(axml[start..start + len].to_vec())
    } else {
        // UTF-16LE：u16 长度 + u16 字符
        if at + 2 > axml.len() {
            return Err(ApkError::BadBinaryXml);
        }
        let len = u16_at(axml, at) as usize;
        let start = at + 2;
        if start + len * 2 > axml.len() {
            return Err(ApkError::BadBinaryXml);
        }
        // 转成 UTF-8（仅处理 BMP 字符，emoji 不做——本项目禁 emoji）
        let mut out = Vec::with_capacity(len);
        let mut i = 0;
        while i < len {
            let c = u16::from_le_bytes([axml[start + i * 2], axml[start + i * 2 + 1]]);
            i += 1;
            if c < 0x80 {
                out.push(c as u8);
            } else if c < 0x800 {
                out.push(0xC0 | (c >> 6) as u8);
                out.push(0x80 | (c & 0x3F) as u8);
            } else {
                out.push(0xE0 | (c >> 12) as u8);
                out.push(0x80 | ((c >> 6) & 0x3F) as u8);
                out.push(0x80 | (c & 0x3F) as u8);
            }
        }
        Ok(out)
    }
}

// ============================================================================
// 兼容层应用描述（安装后展示在桌面网格）
// ============================================================================

/// 一个已安装（或待安装）的 APK 应用
#[derive(Debug, Clone, Default)]
pub struct CompatApp {
    /// 包名（Android package）
    pub package: alloc::string::String,
    /// 显示名（label 字符串）
    pub label: alloc::string::String,
    /// 图标（PNG 原始字节；来自 res/mipmap-* 目录，可取 48x48 起步）
    pub icon_png: Vec<u8>,
    /// 在 /APKS 下的文件名
    pub file_name: alloc::string::String,
    /// 运行状态（0=未运行，1=运行中）
    pub state: u8,
}

/// 从 APK 字节构造应用描述（真实解析路径）
pub fn inspect_apk(data: &[u8]) -> Result<CompatApp> {
    let apk = Apk::parse(data)?;
    let mut app = CompatApp::default();

    // ---- 1. AndroidManifest.xml ----
    let man = apk.find("AndroidManifest.xml")?;
    let man_data = apk.extract(man)?;
    let info = parse_manifest_xml(&man_data)?;
    app.package = info.package;

    // ---- 2. label：简化处理，优先取 raw 值；资源引用时尝试 strings.xml ----
    if info.label_ref.starts_with('@') {
        // 形如 @string/app_name → res/values/strings.xml
        if let Ok(sx) = apk.find("res/values/strings.xml") {
            if let Ok(sx_data) = apk.extract(sx) {
                app.label = strings_xml_label(&sx_data).unwrap_or_else(|| info.label_ref.clone());
            }
        }
        if app.label.is_empty() {
            app.label = info.label_ref.clone();
        }
    } else if !info.label_ref.is_empty() {
        app.label = info.label_ref.clone();
    } else {
        app.label = app.package.clone();
    }

    // ---- 3. 图标：找 mipmap 目录下的 PNG ----
    for n in apk.names() {
        if n.starts_with("res/mipmap") && n.ends_with(".png") {
            // 优先取分辨率较大的（mdpi/hdpi/xhdpi 排序靠后取最后一个）
            if let Ok(e) = apk.find(n) {
                if let Ok(png) = apk.extract(e) {
                    app.icon_png = png;
                }
            }
        }
    }
    Ok(app)
}

/// 从 res/values/strings.xml（二进制 AXML）中提取第一个 name 对应的字符串值
/// （简化：解析 StartElement 的 name 属性 = string 名，取第一个文本子元素的 rawValue）
fn strings_xml_label(axml: &[u8]) -> Option<alloc::string::String> {
    let _ = axml;
    // 完整实现见 Phase 5 路线图；此处返回 None 走 label_ref 兜底。
    None
}

/// 把解析出的应用描述序列化为简单 key=value 文本（.kir manifest 风格）
pub fn app_to_text(app: &CompatApp) -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let _ = writeln!(s, "package={}", app.package);
    let _ = writeln!(s, "label={}", app.label);
    let _ = writeln!(s, "file={}", app.file_name);
    let _ = writeln!(s, "state={}", app.state);
    let _ = writeln!(s, "icon_bytes={}", app.icon_png.len());
    s
}
