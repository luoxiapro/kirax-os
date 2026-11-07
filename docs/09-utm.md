# 在 UTM / UTM SE 中运行 KiraX OS（已放弃 / 存档）

> **状态：UTM 运行路线已被放弃，本页仅作存档。**
> 三张 `.img` 镜像作为交付物仍保留在 `build/`（由 `scripts/make-sdimg.py` 生成），
> 但后续不再投入 UTM 适配与答疑；常规验证请走 QEMU（`make qemu` / `make pi3-qemu`）
> 或真机烧录。下文为历史步骤，保留备查。

## 你需要的文件

| 文件 | 用途 |
|---|---|
| `kirax-os-pi2.img` (64MB) | 树莓派 2 Model B 镜像（mini UART，QEMU raspi2b 机型） |
| `kirax-os-pi3.img` (64MB) | 树莓派 3 Model B 镜像（PL011 UART0，AArch32 模式） |
| `kirax-os-pi3-aarch64.img` (64MB) | 树莓派 3 Model B 镜像（AArch64 模式，kernel8.img + arm_64bit=1） |

三个镜像都是 raw 格式 SD 卡：MBR + FAT16 分区，内含内核与 `config.txt`，
QEMU/UTM 的 VideoCore 模拟 bootloader 会自动加载内核。
镜像内文件名与配置已经过修正：
- **AArch64 镜像内核名必须是 `kernel8.img`**（32 位才是 `kernel7.img`）；
- `config.txt` 已按标准 8.3 命名（`CONFIG.TXT`），含 `arm_64bit=1`（64 位）、
  `enable_uart=1` 与 `dtoverlay=pi3-miniuart-bt`（Pi3 串口）。
  旧版镜像 config.txt 名字长度错误导致固件读不到配置，表现为「黑屏无输出」。

## 一、iOS / iPadOS（UTM SE）

1. App Store 安装 **UTM SE**。
2. 把 `.img` 文件传到「文件」App 的「我的 iPhone」本地目录（**不要放 iCloud 云盘**，
   UTM 读取会超时）。
3. 打开 UTM SE → 左上角 **+** → **新建虚拟机**：
   - 类型：**仿真 (Emulate)**（不要选虚拟化，ARMv7 是跨架构仿真）
   - 系统：**其他 (Other)**
   - 架构：**ARM (32-bit)**
4. 硬件：
   - 内存：**512 MB**（Pi2/Pi3 只有 1GB，给 512 足够；太大 iOS 会杀进程）
   - CPU 核心：**1**
5. 驱动器：**导入镜像** → 选你传进来的 `kirax-os-pi2.img`。
6. 完成并启动。
7. **串口输出**：KiraX OS 目前是命令行/文本输出，UTM 启动后会直接显示
   终端窗口；banner 形如：
   ```
   ========================================
    KiraX OS v0.1.0  (0.1.0)
    ARMv7-A (AArch32) | BCM2709/BCM2836 (Raspberry Pi 2 Model B)
   ========================================
   [boot] core0 up, mini UART 115200 ready
   ...
   [test] PASS  ...（14 项自检）
   ```

## 二、macOS（UTM 完整版）

1. 打开 UTM → **+** → **仿真** → **其他** → **ARM (32-bit)**。
2. 硬件：内存 512 MB、CPU 1 核。
3. 驱动器：导入 `kirax-os-pi2.img`。
4. **QEMU 参数（关键）**：UTM → 虚拟机 → 编辑 → **QEMU** →
   在「QEMU 配置」里把机型改成：
   - Pi2：`raspi2b`
   - Pi3：`raspi3b`
   （默认 "其他" 是 `virt` 机型，跑不了树莓派裸机，必须改）
5. 启动。

## 三、树莓派 3（Pi3 镜像）

- UTM 里机型选 **raspi3b**（macOS UTM 支持；iOS UTM SE 若没有 raspi3b，
  直接用 `kirax-os-pi2.img` 即可，Pi3 镜像在 raspi2b 上也能跑——外设模型一致）。
- Pi3 输出走 PL011 UART0，banner 显示
  `BCM2837 (Raspberry Pi 3 Model B, AArch32 mode)`。

## 三·五、树莓派 3 AArch64（64 位镜像）

导入 `kirax-os-pi3-aarch64.img`，创建虚拟机时：

1. 类型：**仿真 (Emulate)**；系统：**其他 (Other)**；
   架构：**ARM64 (AArch64)**（不是 ARM 32-bit！）。
2. 内存 512 MB、CPU 1 核。
3. macOS UTM 完整版：虚拟机 → 编辑 → QEMU → 机型改成 **raspi3b**。
4. 启动后 banner 显示 `BCM2837 (Raspberry Pi 3 Model B, AArch64 mode)`，
   自检 14 项 PASS。

> 注意：UTM SE（iOS）机型列表若没有 raspi3b，64 位镜像会无法启动
> （QEMU 的 raspi2b 只认 32 位 kernel7.img）。iOS 上请用 32 位
> `kirax-os-pi2.img` / `kirax-os-pi3.img`。

## 四、常见问题

| 现象 | 原因 / 解决 |
|---|---|
| 黑屏，无任何输出 | 机型选成了 `virt`；改回 `raspi2b`/`raspi3b`。或内存 >1GB 被 iOS 杀掉。 |
| AArch64 镜像黑屏 | 架构必须选 **ARM64 (AArch64)** 且机型 **raspi3b**；UTM SE 无 raspi3b 时改用 32 位镜像。 |
| 启动后卡在一行不动 | TCG 仿真慢，14 项自检在 QEMU 上约 5~10 秒，耐心等。 |
| 看不到串口 | UTM 默认终端窗口即串口；检查 USB 外设未误接。 |
| iPad 上闪退 | 关后台其他 App；内存改 256MB。 |
| 想按键交互 | 目前 KiraX OS 仅输出日志，键盘尚未接入；下一阶段接 USB HID。 |

## 五、命令行直跑（对照）

```bash
# Pi2
qemu-system-arm -M raspi2b \
  -drive file=kirax-os-pi2.img,format=raw,if=sd \
  -display none -serial null -serial stdio

# Pi3
qemu-system-arm -M raspi3b \
  -drive file=kirax-os-pi3.img,format=raw,if=sd \
  -display none -serial stdio -serial null
```
