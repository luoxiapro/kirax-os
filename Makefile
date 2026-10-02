# ============================================================================
# Space OS 顶层构建系统（GNU Make）
# 目标：
#   all         构建 kernel7.img
#   qemu        在 QEMU raspi2b 中运行（串口输出到终端）
#   qemu-gdb    启动 QEMU 并等待 GDB 连接（-S -s）
#   gdb         连接 GDB（配合 qemu-gdb）
#   test        主机单元测试 + QEMU 集成测试（自动判定）
#   install     把 kernel7.img 烧录到 SD 卡（需指定 DEV=/dev/sdX）
#   clean       清理构建产物
# ============================================================================

# ---- 工具链 ----
TOOLCHAIN ?= arm-none-eabi
CARGO     ?= cargo
QEMU      ?= qemu-system-arm
QEMU_A64  ?= qemu-system-aarch64
GDB       ?= gdb-multiarch

# ---- 路径 ----
ROOT      := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
KERNEL    := $(ROOT)/kernel
BUILD     := $(ROOT)/build
KERNEL_ELF   := $(BUILD)/kernel.elf
KERNEL_IMG   := $(BUILD)/kernel7.img
# QEMU 专用内核：raspi2b 模拟固件把镜像加载到 0x10000（真机固件是 0x8000），
# 因此用独立链接脚本以 0x10000 为基址重新链接，保证绝对地址正确。
QEMU_ELF     := $(BUILD)/qemu-kernel.elf
START_O      := $(BUILD)/start.o
RUST_LIB     := $(KERNEL)/target/thumbv7a-none-eabi/release/libspace_kernel.a

# ---- 默认串口：mini UART（QEMU 用第二个串口） ----
QEMU_SERIAL ?= -serial null -serial stdio

.PHONY: all qemu qemu-gdb gdb test install clean kernel build-dir

all: $(KERNEL_IMG)

build-dir:
	@mkdir -p $(BUILD)

# ---- 1. 汇编启动代码 ----
$(START_O): $(ROOT)/boot/start.S | build-dir
	$(TOOLCHAIN)-as -mcpu=cortex-a7 -march=armv7-a -o $@ $<

# ---- 2. Rust 内核静态库（thumbv7a-none-eabi） ----
# 注意：Rust 库规则不带源码依赖（cargo 自身做增量检测），但必须用 FORCE
# 强制每次运行，否则 .a 一旦存在、源码后续修改将永不触发重建。
.PHONY: FORCE
$(RUST_LIB): FORCE
	cd $(KERNEL) && $(CARGO) build --release

# ---- 3. 链接（GNU ld + 链接脚本） ----
$(KERNEL_ELF): $(START_O) $(RUST_LIB) $(ROOT)/boot/linker.ld
	$(TOOLCHAIN)-ld -T $(ROOT)/boot/linker.ld -o $@ $(START_O) $(RUST_LIB)
	$(TOOLCHAIN)-objdump -d $(KERNEL_ELF) > $(BUILD)/kernel.dis

# ---- 4. 生成 kernel7.img（原始二进制，真机固件加载到 0x8000） ----
$(KERNEL_IMG): $(KERNEL_ELF)
	$(TOOLCHAIN)-objcopy -O binary $(KERNEL_ELF) $@
	@echo "==> kernel7.img 生成完毕: $$(stat -c%s $@) bytes"
	@$(TOOLCHAIN)-size $(KERNEL_ELF)

# ---- 4b. QEMU 专用内核（基址 0x10000，模拟固件的加载地址） ----
$(QEMU_ELF): $(START_O) $(RUST_LIB) $(ROOT)/boot/linker-qemu.ld
	$(TOOLCHAIN)-ld -T $(ROOT)/boot/linker-qemu.ld -o $@ $(START_O) $(RUST_LIB)

kernel: $(KERNEL_ELF)

# ---- QEMU ----
qemu: $(QEMU_ELF)
	$(QEMU) -M raspi2b -kernel $(QEMU_ELF) -display none $(QEMU_SERIAL)

# QEMU 等待 GDB（端口 1234）
qemu-gdb: $(QEMU_ELF)
	$(QEMU) -M raspi2b -kernel $(QEMU_ELF) -display none -S -s $(QEMU_SERIAL)

# 连接 GDB（脚本在 qemu/ 下）
# 注意：qemu-gdb 实际运行的是 build/qemu-kernel.elf（0x10000 重定位版），
# qemu/gdb_script.gdb 的 symbol-file 也指向它。因此本目标依赖并传入 $(QEMU_ELF)，
# 使【依赖对象 / 命令行传参 / 脚本 symbol-file】三者一致（原误用 $(KERNEL_ELF) 0x8000）。
gdb: $(QEMU_ELF)
	$(GDB) -x $(ROOT)/qemu/gdb_script.gdb $(QEMU_ELF)

# ---- 测试 ----
test: all
	$(ROOT)/tests/run_tests.sh

# ---- SD 卡烧录（注意：DEV 必须显式指定，防止误写） ----
install: $(KERNEL_IMG)
	@test -n "$(DEV)" || (echo "请指定 DEV=/dev/sdX（如 sudo make install DEV=/dev/sdb）"; exit 1)
	@echo "目标设备: $(DEV)，5 秒后开始写入，Ctrl-C 取消"
	@sleep 5
	sudo dd if=$(KERNEL_IMG) of=$(DEV) bs=512 seek=1 conv=fsync
	sync
	@echo "==> 已写入。把 SD 卡插入树莓派 2 并上电（注意别写整卡！这是写到偏移 512 字节的 FAT 分区）"

clean:
	cd $(KERNEL) && $(CARGO) clean
	rm -rf $(BUILD)

# ============================================================================
# 树莓派 3B 构建（BCM2837，AArch32 模式）
# 差异点：CPU = Cortex-A53（ARMv8-A 兼容 ARMv7 指令）；主串口 = PL011 (UART0)
# 外设基址与 Pi2 相同（0x3F000000），中断/定时器/EMMC/USB/framebuffer 驱动全复用。
# 产物名同为 kernel7.img（Pi3 的 32 位固件加载同名文件，真机地址 0x8000）。
# QEMU 验证：Debian/Ubuntu 的 qemu-system-arm 未编译 raspi3b 机型，故用 raspi2b
#   跑 Pi3 构建产物（外设模型一致，PL011 是 raspi2b 模拟的 uart0），-serial stdio
#   即把 PL011 输出到终端，可跑完全部 14 项自检。
# ============================================================================
PI3_BUILD   := $(BUILD)/pi3
PI3_START_O := $(PI3_BUILD)/start.o
PI3_ELF     := $(PI3_BUILD)/kernel.elf
PI3_IMG     := $(PI3_BUILD)/kernel7.img
# Pi3 使用独立 target 目录（target-pi3），与 Pi2 产物隔离。
# 否则 --features pi3 会把同路径 libspace_kernel.a 覆盖成 Pi3 版，
# 导致后续 make qemu（Pi2）链接到 PL011 驱动、mini UART 串口静默。
PI3_RUST    := $(KERNEL)/target-pi3/thumbv7a-none-eabi/release/libspace_kernel.a

.PHONY: pi3 pi3-qemu pi3-gdb pi3-install

$(PI3_BUILD):
	@mkdir -p $(PI3_BUILD)

$(PI3_START_O): $(ROOT)/boot/start.S | $(PI3_BUILD)
	$(TOOLCHAIN)-as -mcpu=cortex-a53 -march=armv7-a -o $@ $<

# Pi3 内核静态库：同一 Rust 源码，--features pi3 切换串口/SOC 横幅
$(PI3_RUST): FORCE
	cd $(KERNEL) && $(CARGO) build --release --features pi3 --target-dir $(KERNEL)/target-pi3

$(PI3_ELF): $(PI3_START_O) $(PI3_RUST) $(ROOT)/boot/linker.ld | $(PI3_BUILD)
	$(TOOLCHAIN)-ld -T $(ROOT)/boot/linker.ld -o $@ $(PI3_START_O) $(PI3_RUST)
	$(TOOLCHAIN)-objdump -d $@ > $(PI3_BUILD)/kernel.dis

$(PI3_IMG): $(PI3_ELF)
	$(TOOLCHAIN)-objcopy -O binary $(PI3_ELF) $@
	@echo "==> Pi3 kernel7.img 生成完毕: $$(stat -c%s $@) bytes"
	@$(TOOLCHAIN)-size $(PI3_ELF)

pi3: $(PI3_IMG)

# Pi3 构建产物在 QEMU（raspi2b 机型）验证：PL011 接第一个串口，直接 -serial stdio。
# 注意：raspi2b 模拟固件把镜像加载到 0x10000（真机是 0x8000），必须用 qemu 链接脚本，
# 因此额外生成 pi3 qemu 版 ELF（代码与真机镜像完全相同，仅重定位到 0x10000）。
PI3_QEMU_ELF := $(PI3_BUILD)/qemu-kernel.elf

$(PI3_QEMU_ELF): $(PI3_START_O) $(PI3_RUST) $(ROOT)/boot/linker-qemu.ld | $(PI3_BUILD)
	$(TOOLCHAIN)-ld -T $(ROOT)/boot/linker-qemu.ld -o $@ $(PI3_START_O) $(PI3_RUST)

pi3-qemu: $(PI3_QEMU_ELF)
	$(QEMU) -M raspi2b -kernel $(PI3_QEMU_ELF) -display none -serial stdio -serial null

# Pi3 GDB 配套：与 pi3-qemu 成对（终端1 make pi3-qemu，终端2 make pi3-gdb）。
# 依赖 $(PI3_QEMU_ELF)（build/pi3/qemu-kernel.elf，0x10000），脚本 symbol-file 亦指向它。
pi3-gdb: $(PI3_QEMU_ELF)
	$(GDB) -x $(ROOT)/qemu/gdb_script_pi3.gdb $(PI3_QEMU_ELF)

# Pi3 SD 卡烧录（同样写到 FAT 分区偏移 512 字节；config.txt 需 kernel=kernel7.img）
pi3-install: $(PI3_IMG)
	@test -n "$(DEV)" || (echo "请指定 DEV=/dev/sdX（如 sudo make pi3-install DEV=/dev/sdb）"; exit 1)
	@echo "目标设备: $(DEV)，5 秒后开始写入，Ctrl-C 取消"
	@sleep 5
	sudo dd if=$(PI3_IMG) of=$(DEV) bs=512 seek=1 conv=fsync
	sync
	@echo "==> 已写入。把 SD 卡插入树莓派 3B，确认 config.txt 含 kernel=kernel7.img 与 enable_uart=1"

# ============================================================================
# 树莓派 3 AArch64 构建（BCM2837，Cortex-A53，64-bit 模式）
# ----------------------------------------------------------------------------
# 与 32 位流程的关键差异（重要，勿照搬 pi3 目标）：
#   1. 启动代码（EL2→EL1 降级、VMSAv8-64 页表、异常向量表）由
#      kernel/src/start64.rs 的 core::arch::global_asm! 内嵌进 libspace_kernel.a，
#      由 rustc/LLVM 在 cargo build 阶段直接汇编。因此本目标【不需要】外部
#      start.o——boot/start64.S 是与之等价的独立汇编副本，若再链接它会与库内
#      global_asm! 产生 _start / mmu_init_64 / _vectors / _page_l1 重复定义。
#   2. 链接脚本 boot/linker64.ld：入口基址 0x80000（AArch64 固件加载地址）。
#   3. 产物：build/pi3-aarch64/kernel8.elf → kernel8.img（AArch64 固件加载
#      kernel8.img，配合 config.txt 的 arm_64bit=1）。
#   4. 工具链降级：本机若无 arm/aarch64 交叉 ld/objcopy，直接使用 Rust 自带的
#      rust-lld（-flavor gnu，行为等同 GNU ld）与 llvm-objcopy / llvm-size
#      （位于 rustlib bin 下；运行时需 LD_LIBRARY_PATH 指向 toolchain 的 lib/，
#      以加载 libLLVM）。
# ============================================================================
PI3A_BUILD := $(BUILD)/pi3-aarch64
PI3A_ELF   := $(PI3A_BUILD)/kernel8.elf
PI3A_IMG   := $(PI3A_BUILD)/kernel8.img
PI3A_RUST  := $(KERNEL)/target/aarch64-unknown-none/release/libspace_kernel.a

# Rust 自带 LLVM 工具链（降级路径；可被环境变量覆盖）
RUSTUP_TC  := $(HOME)/.rustup/toolchains/stable-x86_64-unknown-linux-gnu
LLVM_BINDIR := $(RUSTUP_TC)/lib/rustlib/x86_64-unknown-linux-gnu/bin
LLD        ?= $(LLVM_BINDIR)/rust-lld
OBJCOPY    ?= $(LLVM_BINDIR)/llvm-objcopy
SIZE       ?= $(LLVM_BINDIR)/llvm-size
LLVM_LDPATH := $(RUSTUP_TC)/lib

.PHONY: pi3-aarch64

$(PI3A_BUILD):
	@mkdir -p $(PI3A_BUILD)

# AArch64 内核静态库（global_asm! 启动代码已内含其中）
$(PI3A_RUST): FORCE
	cd $(KERNEL) && $(CARGO) build --release --target aarch64-unknown-none

$(PI3A_ELF): $(PI3A_RUST) $(ROOT)/boot/linker64.ld | $(PI3A_BUILD)
	LD_LIBRARY_PATH=$(LLVM_LDPATH):$(LD_LIBRARY_PATH) $(LLD) -flavor gnu -T $(ROOT)/boot/linker64.ld -o $@ $(PI3A_RUST)
	@echo "   [size]"; LD_LIBRARY_PATH=$(LLVM_LDPATH):$(LD_LIBRARY_PATH) $(SIZE) $@

$(PI3A_IMG): $(PI3A_ELF)
	LD_LIBRARY_PATH=$(LLVM_LDPATH):$(LD_LIBRARY_PATH) $(OBJCOPY) -O binary $(PI3A_ELF) $@
	@echo "==> Pi3 AArch64 kernel8.img 生成完毕: $$(stat -c%s $@) bytes"

pi3-aarch64: $(PI3A_IMG)

# AArch64 QEMU 验证（raspi3b 机型；kernel8.elf 由 rust-lld 链出，0x80000）
.PHONY: pi3-aarch64-qemu
pi3-aarch64-qemu: $(PI3A_ELF)
	$(QEMU_A64) -M raspi3b -kernel $(PI3A_ELF) -display none -serial stdio -serial null
