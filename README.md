# Space OS

> 树莓派 2 Model B（BCM2709，4×Cortex-A7，ARMv7-A 32 位，1GB）上从零构建的原创操作系统。
> 无 Linux 内核、无发行版、无第三方图形库。Rust + C + ARMv7 汇编。深空星球主题 + 液态玻璃桌面。
> 已适配 **树莓派 3 Model B**（BCM2837，AArch32 模式见 `docs/08-pi3.md`；AArch64 模式产物 `kernel8.img`，由 `make pi3-aarch64` 生成）。

## 快速开始

```bash
# 1. 安装依赖（Ubuntu/Debian，环境被重置时用脚本一键恢复）
bash tools/setup-toolchain.sh     # qemu + gdb + arm-none-eabi + apt 源/代理

# 2. 安装 Rust target
rustup target add thumbv7a-none-eabi aarch64-unknown-none

# 3. 构建并生成统一镜像（一张 SD 卡覆盖全部机型）
make all               # 统一 32 位内核 build/kernel7.img（启动时读 MIDR 自动识别 Pi2/Pi3）
make pi3-aarch64       # AArch64 内核 build/pi3-aarch64/kernel8.img
make qemu              # QEMU 快速验证（Pi2 模拟：mini UART）
make pi3-aarch64-qemu  # QEMU 验证 AArch64（raspi3b 机型）

# 4. 生成最终 SD 镜像（256MB，含 KERNEL7.IMG + KERNEL8.IMG + CONFIG.TXT）
python3 scripts/make-sdimg.py build/kernel7.img build/space-os.img --model pi3 --kernel64 build/pi3-aarch64/kernel8.img

# 5. 烧录（唯一交付镜像）
sudo dd if=build/space-os.img of=/dev/sdX bs=4M conv=fsync
#    Pi3 → 默认 AArch64 桌面；Pi2 → 旧固件忽略 arm_64bit 自动加载统一 32 位内核
```

## 目录结构

```
space-os/
├── boot/                 # 启动汇编 + 链接脚本
│   ├── start.S           # ARMv7 启动 stub（0x10000 QEMU / 0x8000 真机双态）
│   ├── start64.S         # AArch64 启动副本（等价冗余；实际由 kernel/src/start64.rs 的 global_asm! 内嵌进 .a，勿再链接）
│   ├── linker.ld         # 真机链接脚本（0x8000）
│   ├── linker-qemu.ld    # QEMU 链接脚本（0x10000）
│   └── linker64.ld       # AArch64 链接脚本（0x80000）
├── kernel/               # Rust 内核 crate
│   └── src/
│       ├── main.rs       # 内核入口（SOC 运行时检测，统一镜像）
│       ├── soc.rs         # MIDR 运行时识别 Pi2/Pi3（统一镜像核心）
│       ├── console.rs    # 串口转发层（soc::use_pl011() 运行时选择 mini UART / PL011）
│       ├── uart.rs / pl011.rs   # mini UART / PL011 驱动（Pi3 自配 cprman 时钟）
│       ├── memory.rs / mmu.rs   # 物理内存/伙伴堆 / MMU 页表
│       ├── timer.rs / irq.rs / exception.rs / sync.rs / scheduler.rs / syscall.rs
│       ├── mailbox.rs / framebuffer.rs / font.rs   # GPU mailbox / 帧缓冲 / 8×8 位图字体
│       ├── sd.rs / usb/  # EMMC/SD 卡 / USB DWC2 HID
│       ├── graphics/ / window/ / desktop/   # 绘制原语 / 窗口管理 / 玻璃桌面
│       ├── kir/          # .kir(SKIR) 加载器
│       ├── start64.rs    # AArch64 启动（global_asm! 内嵌，EL2→EL1、VMSAv8-64 页表）
│       └── selftest.rs   # 板载自检（14 项）
├── crates/space-core/    # 与内核解耦的纯算法库（host 可单测）
│   └── src/
│       ├── buddy.rs      # 伙伴分配器
│       ├── blend.rs / blur.rs   # Alpha 混合 / 三次盒式高斯模糊
│       ├── roundrect.rs / rr_check.rs   # 圆角矩形 SDF / 校验
│       ├── fat32.rs      # FAT32 只读驱动
│       ├── fmath.rs      # 无 libm 的浮点工具
│       ├── events.rs / glass.rs   # 输入事件环 / Glass 设计令牌（权威色板）
│       └── kir.rs        # .kir(SKIR) 32B 定长头应用二进制格式
├── apps/                 # 系统自带 .kir 应用（about/calculator/clock/editor/hello/terminal，各含 manifest.json）
│   └── apkcompat/        # APK 兼容器（Rust 工程，ZIP/AXML 解析 + 兼容层，产出 apkcompat.kir）
├── tools/
│   ├── space-cli/        # build/run/package/install 命令行（附 README）
│   ├── kirpm/            # .kir 包管理器
│   ├── vscode-space/     # VS Code 扩展（语法高亮 + language-configuration）
│   ├── fontgen/          # 字体生成工具（空占位目录，暂无代码）
│   └── setup-toolchain.sh  # 工具链一键恢复（qemu/gdb/arm-none-eabi/apt 源）
├── space-studio/         # 图形化 IDE（Tauri → Windows exe/setup / cargo-ndk → Android APK）
├── specs/GLASS_DESIGN.md # Glass 设计规范（色值/透明度/模糊半径，与 crates/space-core/src/glass.rs 对齐）
├── docs/                 # 开发文档（01 裸机 … 07 工具链、08 Pi3 适配、09 UTM 存档）
├── scripts/              # pack-apkcompat.py 打包、make-sdimg.py 生成 SD 镜像
├── tests/run_tests.sh    # 一键测试脚本
├── qemu/                 # gdb_script.gdb（Pi2）+ gdb_script_pi3.gdb（Pi3）
└── build/                # 构建产物（pi3/、pi3-aarch64/ 子目录 + *.img SD 镜像）
```

## 构建产物

| 文件 | 用途 | 基址 |
|---|---|---|
| `build/qemu-kernel.elf` | QEMU `-kernel` 加载（Pi2） | 0x10000 |
| `build/kernel.elf` / `build/kernel7.img` | 真机 ELF / 裸二进制（Pi2） | 0x8000 |
| `build/pi3/qemu-kernel.elf` | QEMU `-kernel` 加载（Pi3，PL011） | 0x10000 |
| `build/pi3/kernel.elf` / `build/pi3/kernel7.img` | 真机 ELF / 裸二进制（Pi3，AArch32） | 0x8000 |
| `build/pi3-aarch64/kernel8.elf` / `build/pi3-aarch64/kernel8.img` | 真机 ELF / 裸二进制（Pi3，AArch64） | 0x80000 |
| `build/space-os.img` | **256MB 统一 raw SD 镜像**（KERNEL7.IMG + KERNEL8.IMG + CONFIG.TXT；Pi2/Pi3 双兼容） | — |
| `apps/apkcompat/apkcompat.kir` | APK 兼容器自带应用（SKIR 32B 头） | — |

## 当前状态

- 阶段一（裸机 UART）：完成，QEMU 点亮 banner。
- 阶段二（内核核心）：buddy 分配器、MMU 1:1、timer、irq 跑通；**板载自检 14/14 PASS**（统一内核 + 强制 PL011 版 + AArch64 三路径均通过），四核唤醒、调度器 idle。
- 算法库（space-core）：**host 单测 57/57 PASS**（buddy/blend/blur/roundrect/fat32/fmath/events/glass/kir + rr_check 圆角描边断言）；其中 `kir.rs` 已从旧 KIR1 重写为生态实际的 **SKIR 32B 定长头**线格式，模块单测 6→9 个。
- **统一镜像**：`kernel/src/soc.rs` 启动时读 MIDR 识别 Pi2（Cortex-A7）/Pi3（Cortex-A53），一份 32 位内核 + 一份 AArch64 内核合入单镜像 `space-os.img`，一张 SD 卡覆盖全部机型。
- **零告警**：三个 target（thumbv7a-none-eabi / +pi3 feature / aarch64-unknown-none）编译均 0 error 0 warning；`make test` 三路径 QEMU 自检 14/14，5 轮稳定性重复测试全 PASS。
- **Pi3 适配**：PL011 驱动 + 自配 cprman 时钟；QEMU 验证 14/14 PASS。坑点与排查见 `docs/08-pi3.md`。
- **Pi3 AArch64**：新增 `make pi3-aarch64` → `build/pi3-aarch64/kernel8.img`（用 rust-lld/llvm-objcopy；AArch64 启动代码由 `kernel/src/start64.rs` 的 `global_asm!` 内嵌进 `libspace_kernel.a`，**不依赖** `boot/start64.S`——后者是等价冗余副本，链接会与库内 `_start`/`mmu_init_64`/`_vectors`/`_page_l1` 重复定义）。
- **APK 兼容器**（`apps/apkcompat/`）：ZIP 容器 + AndroidManifest 二进制 XML 解析真实可运行，**host 单测 6/6 PASS**，已打包 `apkcompat.kir` 并自验证。DEX 解释器 / Binder 抽象 / ART 对象模型列入路线图。
- 阶段三~五：驱动、玻璃桌面、.kir 生态代码骨架与文档就绪，真机验证待做。
- 阶段六（Space Studio）：**三平台真产物已交付**——Windows `SpaceStudio.exe`（AS 风格 IDE：项目树/编辑器 tab/行号/输出窗/状态栏）、NSIS 安装器（含 WebView2Loader.dll）、Android APK（arm64-v8a/armeabi-v7a/x86_64 三 ABI，JNI 入口已修复）。构建命令与产物见 `space-studio/README.md`。

## 真机烧录

```bash
# 统一镜像（推荐）：一条 dd 完事，无需手动分区
sudo dd if=build/space-os.img of=/dev/sdX bs=4M conv=fsync

# 手动方式（等价）：镜像内已含 FAT16 分区 + config.txt
#   build/kernel7.img  → /boot/kernel7.img（统一 32 位：Pi2/Pi3 自动识别）
#   build/pi3-aarch64/kernel8.img → /boot/kernel8.img（Pi3 AArch64）
# config.txt（镜像内已生成）：
#   arm_64bit=1
#   enable_uart=1
#   dtoverlay=pi3-miniuart-bt
#   arm_freq=900
#   gpu_mem=16
#   disable_overscan=1
```
