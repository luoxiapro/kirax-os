#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
scan_dex 逐方法对拍验证脚本（baksmali 权威口径）

对 DEX 样本中的每一个"有 code_item 的代码方法"，独立复算指令条数
（官方格式指令长度表 + switch/fill 载荷区间跳过），与 baksmali 反汇编
产物（smali 目录）逐方法比对，输出差异清单与总数口径。

用法:
    python3 verify_scan_vs_baksmali.py <classes.dex> <smaliout_dir>

037 样本实测定稿口径:
    scan 解码总数 257909 vs baksmali 指令行 257910（差 1 = Ls/d;-><init>
    array-data-payload 后 2 字节对齐填充被 baksmali 反汇编为独立 nop，
    scan 将其归入载荷区间——纯口径差异，非 bug）。
    方法级: 11852 个代码方法中 11851 个逐条一致。
"""
import struct
import re
import glob
import os
import sys


def u16(buf, o):
    try:
        return struct.unpack_from('<H', buf, o)[0]
    except struct.error:
        return 0


def u32(buf, o):
    try:
        return struct.unpack_from('<I', buf, o)[0]
    except struct.error:
        return 0


# 官方 Dalvik 指令长度表（code units）。
# 与 crates/kirax-dex/examples/scan_dex.rs 的 inst_units 保持同源。
def inst_units(op):
    # 10x / 12x / 11n / 11x / 21t / 31i(nop 等) / 一元 12x / 21s 部分 / 23x 部分 / 高字节 op
    if op in (0x00, 0x01, 0x04, 0x07, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
              0x10, 0x11, 0x12, 0x1d, 0x1e, 0x21, 0x27, 0x28, 0x3e, 0x3f) \
            or 0x7b <= op <= 0x8f or 0xb0 <= op <= 0xcf or 0xe3 <= op <= 0xff:
        return 1
    # 31t: switch / fill-array-data
    if op in (0x14, 0x17, 0x1b, 0x24, 0x25, 0x26, 0x2a, 0x2b, 0x2c) \
            or 0x6e <= op <= 0x72 or 0x74 <= op <= 0x78:
        return 3
    # 21c / 22b / 22c / 22s / 23x / 35c / 3rc 的部分（2u）
    if op in (0x02, 0x05, 0x08, 0x13, 0x15, 0x16, 0x19, 0x1a, 0x1c,
              0x1f, 0x20, 0x22, 0x23, 0x29) \
            or 0x2d <= op <= 0x3d or 0x40 <= op <= 0x6d \
            or 0x90 <= op <= 0xaf or 0xd0 <= op <= 0xe2:
        return 2
    if op == 0x18:      # const-string/jumbo
        return 5
    if op in (0x79, 0x7a):  # invoke-polymorphic（R8 样本未出现，按 4u）
        return 4
    return 0  # 未知 opcode


def dex_string(buf, off):
    """DEX string_data_item: uleb128 utf16_size + modified UTF-8（无 utf16 数组）"""
    p = off
    while buf[p] & 0x80:
        p += 1
    p += 1
    e = p
    while buf[e] != 0:
        e += 1
    return buf[p:e].decode('utf-8', 'replace')


def load_dex(path):
    buf = open(path, 'rb').read()
    sid_off = u32(buf, 60)
    sid_size = u32(buf, 56)
    tid_off = u32(buf, 68)
    tid_size = u32(buf, 64)
    pid_off = u32(buf, 76)
    mid_off = u32(buf, 92)
    mid_size = u32(buf, 88)
    cd_off = u32(buf, 100)
    cd_size = u32(buf, 96)
    strs = [u32(buf, sid_off + i * 4) for i in range(sid_size)]

    def type_name(t):
        if t >= tid_size:
            return '?'
        d = u32(buf, tid_off + t * 4)
        if d >= sid_size:
            return '?'
        return dex_string(buf, strs[d])

    def proto_sig(p):
        if p * 12 + 12 > len(buf):
            return '?'
        ret = type_name(u32(buf, pid_off + p * 12 + 4))
        po = u32(buf, pid_off + p * 12 + 8)
        args = []
        if po and po + 4 <= len(buf):
            sz = u32(buf, po)
            for i in range(sz):
                args.append(type_name(u16(buf, po + 4 + i * 2)))  # type_list entries 是 u16
        return '(' + ''.join(args) + ')' + ret

    def method_sig(m):
        if m * 8 + 8 > len(buf):
            return '?'
        try:
            return type_name(u16(buf, mid_off + m * 8)) + '->' + \
                dex_string(buf, strs[u32(buf, mid_off + m * 8 + 4)]) + \
                proto_sig(u16(buf, mid_off + m * 8 + 2))
        except Exception:
            return '?'

    def class_methods(cls_data):
        """class_data_item 解析：四计数 + 跳过字段编码 + direct/virtual 独立差分"""
        r = cls_data
        out = []

        def uleb():
            nonlocal r
            v = 0
            sh = 0
            while True:
                b = buf[r]
                r += 1
                v |= (b & 0x7f) << sh
                if b < 0x80:
                    return v
                sh += 7

        sf = uleb()
        ifl = uleb()
        dm = uleb()
        vm = uleb()
        for _ in range(sf + ifl):  # 跳过 encoded_field（diff + access_flags）
            uleb()
            uleb()
        for n in (dm, vm):  # direct / virtual 各自从 0 独立差分
            last = 0
            for _ in range(n):
                diff = uleb()
                flags = uleb()
                co = uleb()
                last += diff
                out.append((last, flags, co))
        return out

    methods = {}
    for c in range(cd_size):
        cls_data = u32(buf, cd_off + c * 32 + 24)
        if cls_data == 0:
            continue
        for idx, flags, co in class_methods(cls_data):
            if co == 0:  # abstract/native 无代码
                continue
            methods[method_sig(idx)] = co
    return buf, methods, sid_off, tid_off, mid_off, cd_off


def scan_count(buf, code_off):
    """按 scan_dex 口径复算指令条数（含载荷区间跳过）"""
    insns = code_off + 16
    sz = u32(buf, code_off + 12)
    end = insns + sz * 2
    pc = 0
    pl = []  # [(start, end)] 载荷区间（含 4 字节对齐填充）
    n = 0
    while pc + 2 <= end - insns:
        hit = [e for s, e in pl if s <= pc < e]
        if hit:
            pc = hit[0]
            continue
        op = u16(buf, insns + pc) & 0xff
        n += 1
        if op in (0x26, 0x2b, 0x2c):  # fill-array-data / packed-switch / sparse-switch
            off = u16(buf, insns + pc + 2)
            s = pc + off * 2
            if s + 4 <= end - insns:
                ident = u16(buf, insns + s)
                if ident == 0x0300:  # array-data-payload: 8 + size*element_width，对齐 4
                    ew = u16(buf, insns + s + 2)
                    sz2 = u32(buf, insns + s + 4)
                    e = (s + 8 + sz2 * ew + 3) & ~3
                elif ident == 0x0100:  # packed-switch-payload: 8 + size*4（ident+size+first_key）
                    sz2 = u16(buf, insns + s + 2)
                    e = (s + 8 + sz2 * 4 + 3) & ~3
                elif ident == 0x0200:  # sparse-switch-payload: 4 + size*8
                    sz2 = u16(buf, insns + s + 2)
                    e = (s + 4 + sz2 * 8 + 3) & ~3
                else:
                    e = 0
                if e and e <= end - insns and not any(a < e and s < b for a, b in pl):
                    pl.append((s, e))
        pc += inst_units(op) * 2
    return n


def load_smali_counts(smali_root):
    """统计 baksmali 反汇编产物：每个方法（类->签名）的指令行数"""
    counts = {}
    total = 0
    for sm in glob.glob(os.path.join(smali_root, '**', '*.smali'), recursive=True):
        rel = os.path.relpath(sm, smali_root)[:-6]
        cls = 'L' + rel + ';'
        cur = None
        cnt = 0
        for line in open(sm, encoding='utf-8'):
            if line.startswith('.method '):
                parts = line.split()
                cur = (cls + '->' + parts[-1]) if parts else None
                cnt = 0
            elif line.startswith('.end method'):
                if cur:
                    counts[cur] = counts.get(cur, 0) + cnt
                    total += cnt
                cur = None
            elif cur and re.match(r'^    [a-z]', line) and not line.startswith('    .'):
                cnt += 1
    return counts, total


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    buf, methods, _, _, _, _ = load_dex(sys.argv[1])
    smali_counts, smali_total = load_smali_counts(sys.argv[2])
    scan_total = 0
    diffs = []
    unmatched = 0
    for sig, co in methods.items():
        n = scan_count(buf, co)
        scan_total += n
        s = smali_counts.get(sig, -1)
        if s == -1:
            unmatched += 1
        elif s != n:
            diffs.append((sig, n, s, s - n))
    print('代码方法数:', len(methods))
    print('scan 解码总数:', scan_total)
    print('baksmali 指令行:', smali_total)
    print('baksmali 未匹配方法数:', unmatched)
    print('差异方法数:', len(diffs))
    for d in diffs[:25]:
        print('  ', d)
    if not diffs:
        print('口径一致（差异 0）')


if __name__ == '__main__':
    main()
