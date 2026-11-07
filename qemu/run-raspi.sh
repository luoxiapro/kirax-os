#!/bin/sh
# KiraX OS —— QEMU 树莓派（raspi2b）验证启动。
# 用法：./qemu/run-raspi.sh [kernel.elf]
#   默认 build/qemu-kernel.elf（32 位 ARMv7，与真机 Pi2 同指令集）。
set -e
cd "$(dirname "$0")/.."
KERNEL="${1:-build/qemu-kernel.elf}"
exec qemu-system-arm \
  -M raspi2b \
  -kernel "$KERNEL" \
  -m 1024 \
  -display none \
  -serial stdio \
  "$@"
