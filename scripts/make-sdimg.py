#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
生成 UTM/QEMU 可直接挂载的 Space OS SD 卡镜像（raw img，纯 Python 手写 FAT16）。

结构：
  扇区 0       : MBR（活动分区 0x0E FAT16 LBA，起始 2048）
  扇区 2048~   : FAT16 分区（kernel*.img + config.txt）
  QEMU raspi2b/raspi3b 模拟的 VideoCore bootloader 自动从该分区加载内核

用法：
  python3 scripts/make-sdimg.py <kernel.img> <输出.img> [--arm64] [--model pi3|pi2]
  --arm64  按 AArch64 固件打包：内核命名为 KERNEL8.IMG，config.txt 加 arm_64bit=1
  --model  默认 pi2（kernel7.img，mini UART）；pi3 追加 enable_uart=1 + pi3-miniuart-bt
"""
import struct, sys, os, argparse

SECTOR = 512
PART_START = 2048          # 1MB 对齐
# 镜像总大小必须为 2 的幂：QEMU raspi3b 的 SD 卡模型要求容量为 2 的幂
#（当前 QEMU 版本最小接受 256MiB），256MiB 兼容 QEMU 全部机型与 UTM，
# 真机 SD 卡亦无影响。
IMG_MB = 256
NFAT = 2
ROOT_ENTRIES = 512
ROOT_DIR_SECTORS = (ROOT_ENTRIES * 32 + SECTOR - 1) // SECTOR
RESERVED = 1
SPF = 512                  # 每 FAT 扇数（FAT16，65536 簇容量）

def make(kernel_path, out_path, label="SPACEOS", arm64=False, model="pi2", kernel64_path=None):
    total_sectors = IMG_MB * 1024 * 1024 // SECTOR
    part_size = total_sectors - PART_START

    # ---- 分区内扇区布局 ----
    # [0] BPB (1) [1..2*SPF] FAT1/FAT2 [..root] 根目录 [..data] 数据区
    data_start_rel = RESERVED + NFAT * SPF + ROOT_DIR_SECTORS
    # 文件数据从簇 2 开始；簇 n 的扇区 = data_start_rel + (n-2)

    part = bytearray(part_size * SECTOR)

    # ---- BPB ----
    bpb = part
    bpb[0:3] = b"\xEB\x3C\x90"                       # jmp
    bpb[3:11] = b"MSWIN4.1"                          # OEM
    struct.pack_into("<H", bpb, 0x0B, SECTOR)         # bytes per sector
    bpb[0x0D] = 1                                    # sectors per cluster
    struct.pack_into("<H", bpb, 0x0E, RESERVED)      # reserved sectors
    bpb[0x10] = NFAT                                 # number of FATs
    struct.pack_into("<H", bpb, 0x11, ROOT_ENTRIES)  # root entries
    struct.pack_into("<H", bpb, 0x13, 0)             # total sectors 16-bit (0=用32位)
    bpb[0x15] = 0xF8                                 # media descriptor
    struct.pack_into("<H", bpb, 0x16, SPF)           # sectors per FAT
    struct.pack_into("<H", bpb, 0x18, 32)            # sectors per track
    struct.pack_into("<H", bpb, 0x1A, 2)             # heads
    struct.pack_into("<I", bpb, 0x1C, PART_START)    # hidden sectors
    struct.pack_into("<I", bpb, 0x20, part_size)     # total sectors 32-bit
    bpb[0x24] = 0x80                                 # drive number
    bpb[0x26] = 0x29                                 # boot signature
    struct.pack_into("<I", bpb, 0x27, 0x12345678)    # volume id
    bpb[0x2B:0x36] = label.ljust(11).encode()[:11]   # volume label
    bpb[0x36:0x3E] = b"FAT16   "                     # fs type
    bpb[510] = 0x55; bpb[511] = 0xAA

    # ---- FAT 表初始化（簇 0/1 保留）----
    fat_off = RESERVED * SECTOR
    fat = bytearray(SPF * SECTOR)
    struct.pack_into("<H", fat, 0, 0xFFF8)   # 簇 0 = media descriptor + 0xFF00 高
    struct.pack_into("<H", fat, 2, 0xFFFF)    # 簇 1 = EOF
    # 写两份 FAT
    part[fat_off:fat_off + len(fat)] = fat
    part[fat_off + SPF * SECTOR:fat_off + 2 * SPF * SECTOR] = fat

    # ---- 写文件 ----
    root_off = (RESERVED + NFAT * SPF) * SECTOR
    cluster = 2
    entries = []

    def add_file(name_83, data, cur_cluster):
        # 写数据到连续簇
        n = len(data)
        nsec = (n + SECTOR - 1) // SECTOR
        base = (data_start_rel + (cur_cluster - 2)) * SECTOR
        part[base:base + n] = data
        # 链：除最后一簇外都指向下一簇
        for i in range(nsec):
            c = cur_cluster + i
            if i < nsec - 1:
                nxt = c + 1
            else:
                nxt = 0xFFFF
            struct.pack_into("<H", fat, c * 2, nxt)
        # 根目录条目
        e = bytearray(32)
        e[0:11] = name_83
        e[0x0B] = 0x20                              # ARCH
        struct.pack_into("<H", e, 0x1A, cur_cluster)
        struct.pack_into("<I", e, 0x1C, n)
        entries.append(e)
        return cur_cluster + nsec

    with open(kernel_path, "rb") as kf:
        kdata = kf.read()
    # 32 位内核：固件加载 kernel7.img（统一内核，Pi2/Pi3 自动识别 SoC）
    kname = b"KERNEL7 IMG"
    cluster = add_file(kname, kdata, cluster)

    # 可选 64 位内核：固件加载 kernel8.img（仅 Pi3；配合 arm_64bit=1）
    kdata64 = None
    if kernel64_path:
        with open(kernel64_path, "rb") as kf:
            kdata64 = kf.read()
        cluster = add_file(b"KERNEL8 IMG", kdata64, cluster)

    # ---- config.txt（统一镜像：默认 AArch64；Pi2 旧固件不认识 arm_64bit
    #      会忽略该行并加载 kernel7.img，天然双兼容）----
    cfg_lines = []
    if kernel64_path:
        cfg_lines.append("arm_64bit=1")              # Pi3 固件进入 AArch64 模式，加载 kernel8.img
        cfg_lines.append("enable_uart=1")            # PL011(UART0) 作串口控制台
        cfg_lines.append("dtoverlay=pi3-miniuart-bt")
    elif arm64:
        cfg_lines.append("arm_64bit=1")
        cfg_lines.append("enable_uart=1")
        cfg_lines.append("dtoverlay=pi3-miniuart-bt")
    cfg_lines += ["arm_freq=900", "gpu_mem=16", "disable_overscan=1"]
    if model == "pi3" and not kernel64_path and not arm64:
        cfg_lines.append("enable_uart=1")            # 32 位 Pi3 专用
        cfg_lines.append("dtoverlay=pi3-miniuart-bt")
    cfg = ("\n".join(cfg_lines) + "\n").encode()
    cluster = add_file(b"CONFIG  TXT", cfg, cluster)

    # 把更新后的 FAT 写回两份
    part[fat_off:fat_off + len(fat)] = fat
    part[fat_off + SPF * SECTOR:fat_off + 2 * SPF * SECTOR] = fat

    # 写根目录条目
    for i, e in enumerate(entries):
        off = root_off + i * 32
        part[off:off + 32] = e

    # ---- 组装完整镜像（MBR + 分区）----
    out = bytearray(total_sectors * SECTOR)
    mbr = bytearray(SECTOR)
    entry = bytearray(16)
    entry[0] = 0x80
    entry[1:4] = b"\x00\x02\x00"
    entry[4] = 0x0E
    entry[5:8] = b"\xFE\xFF\xFF"   # CHS end（3 字节；之前写成 entry[5:7] 导致
                                   # 切片赋值宽度不匹配，bytearray 被撑大 1 字节，
                                   # 镜像不再是 2 的幂，QEMU SD 卡模型拒绝挂载）
    struct.pack_into("<I", entry, 8, PART_START)
    struct.pack_into("<I", entry, 12, part_size)
    mbr[446:462] = entry
    mbr[510] = 0x55; mbr[511] = 0xAA
    out[0:SECTOR] = mbr
    out[PART_START * SECTOR:PART_START * SECTOR + len(part)] = part

    with open(out_path, "wb") as f:
        f.write(out)
    sz = os.path.getsize(out_path)
    print("[sdimg] 已生成 %s（%.1f MB）" % (out_path, sz / 1024 / 1024))
    print("[sdimg]   KERNEL7.IMG: %d 字节（%d 簇）" % (len(kdata), (len(kdata)+SECTOR-1)//SECTOR))
    if kdata64:
        print("[sdimg]   KERNEL8.IMG: %d 字节（%d 簇）" % (len(kdata64), (len(kdata64)+SECTOR-1)//SECTOR))
    print("[sdimg]   config.txt: %d 字节" % len(cfg))
    print("[sdimg]   用法（QEMU/UTM）：-M raspi2b/raspi3b -drive file=%s,format=raw" % out_path)

if __name__ == "__main__":
    ap = argparse.ArgumentParser(description="生成 Space OS SD 卡镜像")
    ap.add_argument("kernel", help="32 位内核二进制（统一 kernel7.img）")
    ap.add_argument("out", help="输出镜像路径")
    ap.add_argument("--kernel64", default=None, help="64 位内核二进制（kernel8.img），同时写入并启用 arm_64bit=1")
    ap.add_argument("--arm64", action="store_true", help="仅打包 AArch64（KERNEL8.IMG + arm_64bit=1）")
    ap.add_argument("--model", choices=["pi2", "pi3"], default="pi2", help="机型（pi3 追加 enable_uart=1）")
    args = ap.parse_args()
    make(args.kernel, args.out, arm64=args.arm64, model=args.model, kernel64_path=args.kernel64)
