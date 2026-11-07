#!/bin/sh
# KiraX OS —— QEMU ARM virt（PC 虚拟机通用平台）启动骨架。
#
# 与 raspi 的差异：virt 是 ARM 虚拟机标准平台，不依赖树莓派固件与
# BCM 外设，PC 上用 qemu-system-arm 即可启动，是"不再绑定树莓派"的
# 目标平台。
#
# 当前状态（1.0.0）：
#   - 地址表已切（--features board_virt，见 kernel/src/board/virt.rs）
#   - PL011 串口地址已切到 0x09000000
#   - 待适配：GIC 中断接管（现 BCM 中断控制器）、virtio-mmio 块设备
#     （现 SDHCI）、无 GPIO/CPRMAN 初始化路径。
#
# 编译板级：cargo build --features board_virt --release
# 启动本脚本。
set -e
cd "$(dirname "$0")/.."
KERNEL="${1:-build/qemu-kernel.elf}"
exec qemu-system-arm \
  -M virt \
  -cpu cortex-a15 \
  -m 1024 \
  -kernel "$KERNEL" \
  -display none \
  -serial stdio \
  "$@"
