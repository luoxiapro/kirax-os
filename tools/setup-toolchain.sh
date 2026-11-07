#!/bin/bash
# =============================================================================
# KiraX OS 交叉编译/仿真工具链一键恢复脚本
# 用途：工作环境被重置后（qemu/arm-none-eabi/gdb 丢失、apt 源与代理丢失），
#       一条命令恢复全部依赖。Rust 工具链（~/.cargo、~/.rustup）通常仍在，
#       本脚本只做 PATH 导出与 apt 层安装。
#
# 用法：  bash tools/setup-toolchain.sh
# 幂等：  重复执行安全（已装包会跳过）。
# =============================================================================
set -euo pipefail

# ---- 0. Rust PATH（工具链在 ~/.rustup，二进制在 ~/.cargo/bin） ----
export PATH="$HOME/.cargo/bin:$PATH"
rustc --version 2>/dev/null || echo "!! rustc 不可用：请检查 ~/.rustup/toolchains 是否存在"

# ---- 1. apt 代理（写回；若已存在则不动） ----
APT_PROXY=/etc/apt/apt.conf.d/99proxy
if [ ! -f "$APT_PROXY" ]; then
  echo "==> 写回 apt 代理配置"
  echo 'Acquire::http::Proxy "http://proxy-sid-23669567:@vortex-3e0s8vctxbmdc6uqpftnfzswr.vortexip.cn-beijing2.volces.com:8080";' | sudo tee "$APT_PROXY" >/dev/null
  echo 'Acquire::https::Proxy "http://proxy-sid-23669567:@vortex-3e0s8vctxbmdc6uqpftnfzswr.vortexip.cn-beijing2.volces.com:8080";' | sudo tee -a "$APT_PROXY" >/dev/null
else
  echo "==> apt 代理已存在，跳过"
fi

# ---- 2. apt 源（清华 jammy；备份旧源后写入） ----
SOURCES=/etc/apt/sources.list
if ! grep -q 'mirrors.tuna.tsinghua.edu.cn' "$SOURCES" 2>/dev/null; then
  echo "==> 切换 apt 源为清华镜像"
  sudo cp "$SOURCES" "$SOURCES.bak" 2>/dev/null || true
  cat <<'EOF' | sudo tee "$SOURCES" >/dev/null
deb http://mirrors.tuna.tsinghua.edu.cn/ubuntu/ jammy main restricted universe multiverse
deb http://mirrors.tuna.tsinghua.edu.cn/ubuntu/ jammy-updates main restricted universe multiverse
deb http://mirrors.tuna.tsinghua.edu.cn/ubuntu/ jammy-security main restricted universe multiverse
EOF
else
  echo "==> apt 源已是清华镜像，跳过"
fi

# ---- 3. 安装 qemu / gdb / arm-none-eabi 工具链 ----
echo "==> apt update（经代理）"
sudo apt-get update -o Acquire::Retries=2 2>&1 | tail -2
echo "==> 安装 qemu / gdb / arm 工具链"
sudo apt-get install -y --no-install-recommends \
  qemu-system-arm qemu-system-aarch64 gdb-multiarch \
  binutils-arm-none-eabi gcc-arm-none-eabi 2>&1 | tail -3

# ---- 4. 验证 ----
echo "==> 验证"
for t in qemu-system-arm qemu-system-aarch64 gdb-multiarch arm-none-eabi-ld arm-none-eabi-as; do
  printf '  %-18s %s\n' "$t" "$(command -v $t || echo MISSING)"
done
echo "==> 完成。之后每次编译请先: export PATH=\"\$HOME/.cargo/bin:\$PATH\""
