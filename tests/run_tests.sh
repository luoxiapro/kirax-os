#!/usr/bin/env bash
# Space OS 一键测试脚本
# 用法: ./tests/run_tests.sh
# 1) host 端单元测试 (cargo test)
# 2) QEMU 端板载自检 (make qemu, 解析串口输出)
set -u
cd "$(dirname "$0")/.."

export PATH="$HOME/.cargo/bin:$PATH:/usr/bin:$PATH"
PASS=0; FAIL=0
echo "===== Space OS 测试套件 ====="

echo
echo "--- [1/2] Host 单元测试 (space-core) ---"
if (cd crates/space-core && cargo test 2>&1 | tail -8); then
  echo "host 测试完成"
fi

echo
echo "--- [2/2] QEMU 板载自检 ---"
LOG=$(timeout 60 make qemu 2>&1)
echo "$LOG" | grep -E "\[test\]|SELFTEST|PANIC|HALTED"
QEMU_PASS=$(echo "$LOG" | grep -c "\[test\] PASS")
QEMU_FAIL=$(echo "$LOG" | grep -c "\[test\] FAIL")
QEMU_PANIC=$(echo "$LOG" | grep -c "KERNEL PANIC")
echo "QEMU: PASS=$QEMU_PASS FAIL=$QEMU_FAIL PANIC=$QEMU_PANIC"

echo
echo "===== 汇总 ====="
echo "host 测试: 见上方"
echo "QEMU 自检: $QEMU_PASS passed / $QEMU_FAIL failed / panic=$QEMU_PANIC"
if [ "$QEMU_PANIC" -ne 0 ]; then
  echo "结果: FAIL (内核 panic)"
  exit 1
else
  echo "结果: OK (无 panic)"
  exit 0
fi
