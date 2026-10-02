#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
apkcompat.kir 打包脚本（Space OS 自带软件 · APK 兼容器）

产物结构（SKIR，与 tools/space-cli、tools/kirpm、space-studio kirpack.rs 一致）：
  魔数 "SKIR" | 版本 u32 | 头 32B（code/icon/manifest 三段的偏移+大小） | code | icon | manifest
注意：crates/space-core/src/kir.rs 当前实现的是另一套 KIR1/128B 头格式，尚未对齐本 SKIR 格式。

用法：
  python3 scripts/pack-apkcompat.py [输出路径]

步骤：
  1. 编译 thumbv7a staticlib（--features kir-standalone）
  2. objcopy 提取 .text + .rodata 作为 code 段
  3. 生成 Glass 风格几何图标（行星+轨道，PNG）
  4. 按 SKIR 规范打包并自验证
"""
import struct, sys, os, zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIB = os.path.join(ROOT, "apps/apkcompat/target/thumbv7a-none-eabi/release/libapkcompat.a")
MANIFEST = os.path.join(ROOT, "apps/apkcompat/manifest.json")
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "apps/apkcompat/apkcompat.kir")

# ---------------------------------------------------------------------------
# 1. 提取代码段（.text + .rodata，去掉 BSS 与符号表）
# ---------------------------------------------------------------------------
def which(candidates):
    """返回 PATH 中第一个可用的工具名，都没有则 None。"""
    import shutil
    for c in candidates:
        if shutil.which(c):
            return c
    return None


def extract_code():
    if not os.path.exists(LIB):
        print("[pack] 未找到 %s，请先：cargo build --target thumbv7a-none-eabi --release --features kir-standalone" % LIB)
        sys.exit(1)
    # objcopy / ar 都按候选探测（llvm-* 在 rustup llvm-tools 里，arm-none-eabi-* 在
    # 交叉工具链里，GNU objcopy/ar 作为兜底），不再硬编码某一个前缀。
    oc = which(("llvm-objcopy", "arm-none-eabi-objcopy", "objcopy"))
    if oc is None:
        print("[pack] 找不到 objcopy 工具（尝试 llvm-objcopy / arm-none-eabi-objcopy / objcopy）")
        sys.exit(1)
    ar = which(("llvm-ar", "arm-none-eabi-ar", "ar"))
    if ar is None:
        print("[pack] 找不到 ar 工具（尝试 llvm-ar / arm-none-eabi-ar / ar）")
        sys.exit(1)
    print("[pack] 使用 %s / %s" % (oc, ar))
    # 先解开归档取对象
    ar_tmp = os.path.join(ROOT, "apps/apkcompat/target/thumbv7a-none-eabi/release/_obj")
    if os.path.isdir(ar_tmp):
        for stale in os.listdir(ar_tmp):
            os.remove(os.path.join(ar_tmp, stale))
    else:
        os.makedirs(ar_tmp, exist_ok=True)
    if os.system("cd %s && %s x %s >/dev/null 2>&1" % (ar_tmp, ar, LIB)) != 0:
        print("[pack] 解归档失败：%s x %s" % (ar, LIB))
        sys.exit(1)
    tmp = os.path.join(ROOT, "apps/apkcompat/target/thumbv7a-none-eabi/release/code.bin")
    # 逐个对象提取（GNU/LLVM objcopy 一次只接受一个输入文件）
    with open(tmp, "wb") as f:
        for o in sorted(os.listdir(ar_tmp)):
            if o.endswith(".o"):
                os.system("%s -O binary -j .text -j .rodata %s/%s %s" % (oc, ar_tmp, o, tmp + ".part"))
                if os.path.exists(tmp + ".part"):
                    f.write(open(tmp + ".part", "rb").read())
                    os.remove(tmp + ".part")
    if not os.path.exists(tmp) or os.path.getsize(tmp) == 0:
        print("[pack] 代码段提取失败")
        sys.exit(1)
    print("[pack] code 段: %d 字节" % os.path.getsize(tmp))
    return tmp

# ---------------------------------------------------------------------------
# 2. 生成 Glass 风格几何图标（行星 + 轨道，48x48 PNG，手写 PNG 编码）
# ---------------------------------------------------------------------------
def make_icon():
    W = H = 48
    # 背景：深空近黑蓝紫渐变
    px = bytearray()
    for y in range(H):
        for x in range(W):
            t = (x + y) / (W + H)
            r = int(10 + 14 * t)      # 近黑
            g = int(12 + 22 * t)      # 蓝紫倾向
            b = int(28 + 44 * t)
            # 行星：中心圆（(24,26) r=10），带高光
            dx, dy = x - 24, y - 26
            d = (dx * dx + dy * dy) ** 0.5
            if d <= 10:
                # 渐变球体：左侧亮（冷光），右侧暗
                sh = max(0.0, min(1.0, (dx + 8) / 16.0))
                r = int(90 + 80 * sh)
                g = int(130 + 70 * sh)
                b = int(220 + 30 * sh)
                # 高光点
                if (x - 21) ** 2 + (y - 22) ** 2 <= 9:
                    r = min(255, r + 60); g = min(255, g + 60); b = min(255, b + 60)
            # 轨道环（椭圆，a=17,b=8，粗 1px，上下端削平营造液态玻璃描边感）
            el = ((x - 24) / 17.0) ** 2 + ((y - 26) / 8.0) ** 2
            if 0.90 <= el <= 1.02 and y > 20:
                r, g, b = 150, 210, 255
            # 远轨道（更大更淡）
            el2 = ((x - 24) / 21.0) ** 2 + ((y - 26) / 11.0) ** 2
            if 0.96 <= el2 <= 1.02 and y < 32:
                r = max(0, r - 40); g = max(0, g - 40); b = max(0, b - 40)
            px += bytes((r, g, b))
    # PNG 编码（IHDR + IDAT + IEND）
    def chunk(tag, data):
        c = tag + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)
    raw = bytearray()
    for y in range(H):
        raw.append(0)  # filter none
        raw += px[y * W * 3:(y + 1) * W * 3]
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    return png

# ---------------------------------------------------------------------------
# 3. SKIR 打包
# ---------------------------------------------------------------------------
def pack(code_path, icon, manifest_path):
    code = open(code_path, "rb").read()
    man = open(manifest_path, "rb").read()
    HDR = 32
    out = bytearray()
    out += b"SKIR"
    out += struct.pack("<I", 1)  # 版本
    code_off = HDR
    icon_off = HDR + len(code)
    man_off = HDR + len(code) + len(icon)
    out += struct.pack("<IIIIII",
                       code_off, len(code),
                       icon_off, len(icon),
                       man_off, len(man))
    out += code
    out += icon
    out += man
    open(OUT, "wb").write(bytes(out))
    print("[pack] 已生成 %s（%d 字节）" % (OUT, len(out)))
    return bytes(out)

# ---------------------------------------------------------------------------
# 4. 自验证
# ---------------------------------------------------------------------------
def verify(kir):
    if kir[:4] != b"SKIR":
        print("[verify] 魔数错误"); sys.exit(1)
    code_off, code_len, icon_off, icon_len, man_off, man_len = struct.unpack(
        "<IIIIII", kir[8:32])
    if code_off + code_len + icon_len + man_len != len(kir):
        print("[verify] 段边界错误: %d+%d+%d+%d != %d" % (
            code_off, code_len, icon_len, man_len, len(kir))); sys.exit(1)
    icon = kir[icon_off:icon_off + icon_len]
    if icon[:8] != b"\x89PNG\r\n\x1a\n":
        print("[verify] 图标非 PNG"); sys.exit(1)
    man = kir[man_off:man_off + man_len]
    if b'"apkcompat"' not in man:
        print("[verify] manifest 内容异常"); sys.exit(1)
    print("[verify] PASS：魔数 OK / 段边界 OK / 图标 PNG OK / manifest OK")
    print("[verify] code=%d  icon=%d  manifest=%d" % (code_len, icon_len, man_len))

if __name__ == "__main__":
    code = extract_code()
    icon = make_icon()
    kir = pack(code, icon, MANIFEST)
    verify(kir)
