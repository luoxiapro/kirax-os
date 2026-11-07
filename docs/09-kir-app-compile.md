# .kir 应用编译规范（SKIR 线格式 · KIR2）

> 适用范围：KiraX OS（树莓派 2/3，ARMv7/AArch64）上 `.kir` 应用的编译与打包。
> 本文是权威规范，与真实产物一致：`crates/kirax-core/src/kir.rs`（解析端）、
> `scripts/pack-apkcompat.py`（打包端）、`kirax-studio/src-tauri/src/kirpack.rs`（打包器）、
> `kernel/src/kir/mod.rs`（加载器）均按本格式校验。历史 KIR1（魔数 `0x4B495231`、128B 头、
> key=value manifest）已废弃，不要再用。

## 1. 目标平台与工具链

| 项 | 约定 |
|---|---|
| 目标架构 | ARMv7 Thumb（树莓派 2/3 的 32 位模式；当前生态真实产物即此） |
| Rust 目标 | `thumbv7a-none-eabi`（`rustup target add thumbv7a-none-eabi`） |
| C 等价工具链 | `arm-none-eabi-gcc -mthumb -mcpu=cortex-a7`（或 a53） |
| 运行环境 | `no_std`；堆由运行时/内核提供，应用不依赖 Linux 或第三方库 |
| 最大体积 | 单文件 ≤ 256KB（加载器 `sd_read_file` 上限） |

## 2. 二进制格式（SKIR）

32 字节定长头，全部 **小端 u32**；之后三段顺序拼接，**无对齐填充**：

```
偏移   大小   字段                   说明
0x00   4      魔数 "SKIR"            0x52494B53（小端存储 "SKIR"）
0x04   4      头部版本               当前 = 1
0x08   4      code_offset            代码段偏移（相对文件起始）
0x0C   4      code_size              代码段大小
0x10   4      icon_offset            图标偏移
0x14   4      icon_size              图标大小
0x18   4      manifest_offset        manifest 偏移
0x1C   4      manifest_size          manifest 大小
0x20   ...    代码段（ARMv7 Thumb 机器码）
...           图标段（PNG 原始字节，或 BGRA 位图）
...           manifest 段（JSON 文本）
```

校验规则（加载器强制）：

- 文件长度 ≥ 32 字节；
- 魔数必须是 `SKIR`，否则报"魔数错误：不是 .kir(SKIR) 文件"；
- `version == 1`，否则报"版本不兼容"；
- 三段 `offset + size` 均不得越出文件尾（报"段越界：文件被截断"）；
- 三段不必相邻，但当前打包器一律顺序拼接。

## 3. 代码段要求

代码段是**位置相关的平坦二进制**（position-dependent flat binary）：

- 无 ELF 头、无符号表、无重定位表、无动态链接；
- 代码、只读数据（`.text` + `.rodata`）直接以机器码形式进入 code 段；
- 不要向内核加载器传入 ELF 文件本身（`tools/kirax-cli package` 目前直接打包 ELF，
  属占位实现，**不是合规路径**——加载器按机器码执行会跑飞，正式打包必须先用 objcopy 提取纯二进制）；
- 加载器预期（Phase 5 完整版）：代码段映射到用户地址空间 `0x2000_0000` 起始、BSS 清零、
  SPSR 置 User 模式后入调度队列，入口为 code 段起始字节。
  当前实现只完成头校验（返回生态可用信号），映射与跳转待用户态支持落地——涉及绝对
  地址引用（PC 字面量池等）的应用在完整装载落地前需按 `0x2000_0000` 基址链接并做真机验证。

## 4. 系统调用 ABI

调用约定（用户态 mini libc 封装）：

```text
svc #0
r7 = 系统调用号
r0-r3 = 参数 0..3
返回值在 r0；负数 = 错误码（u32 表示）
```

| 号 | 名称 | 说明 |
|---|---|---|
| 0 | exit | 退出当前进程 |
| 1 | write | fd=1 写串口 |
| 2 | read | fd=0 读串口 |
| 3 | time | 单调 tick 数 |
| 4 | fb_info | 获取 framebuffer 尺寸/地址 |
| 5 | fb_flush | 刷新脏矩形到显存 |
| 6 | sbrk | 扩展堆（简化，内核堆分配，仅调试） |
| 7 | events_poll | 读取输入事件（返回事件数） |
| 8 | window_create | 创建窗口 |
| 9 | window_update | 更新窗口内容 |
| 10 | kir_load | 加载并启动 .kir 应用 |

错误码：`OK=0`、`EINVAL=-22`、`ENOENT=-2`、`ENOMEM=-12`、`ENOSYS=-38`（以 u32 负值表示）。

## 5. manifest 规范

manifest 段是 JSON 文本。内核解析的最小字段：

| 字段 | 必需 | 说明 |
|---|---|---|
| `name` | 是 | 应用名（安装/启动器用） |
| `author` | 是 | 作者 |
| `version` | 是 | 版本号（字符串，如 "0.1.0"） |

可选扩展字段（`apps/*/manifest.json` 模板中约定，内核当前不解析）：

`title`（显示标题）、`icon`（图标主题名）、`width` / `height`（默认窗口尺寸）、`content`（应用内容/文案）。

## 6. Rust 工程要求

参考工程：`apps/apkcompat/`（唯一完整 Rust 工程，产物 `apkcompat.kir` 已入库自验证）。

`Cargo.toml` 关键项：

```toml
[lib]
name = "<app>"
crate-type = ["rlib", "staticlib"]   # staticlib 供链接器提取 .text 作为 code 段

[dependencies]                       # 无第三方依赖

[features]
kir-standalone = []                  # 独立打包：提供最小 allocator + panic handler

[profile.release]
opt-level = 3
```

源码要求：

- `#![no_std]` + `extern crate alloc`；
- host 单测路径（未开 `kir-standalone`）：链接 `std` 提供分配器与 panic_handler（注意
  `cargo test` 也会先以非 test cfg 编一次 staticlib，必须能解析分配器/panic_handler）；
- 独立打包路径（`--features kir-standalone`）：关闭 `std`，由 feature 内模块提供
  64KB bump 分配器与 `panic_handler`（仅用于链接通过；内核内运行时已提供，勿开此 feature）；
- 入口：`#[no_mangle] pub extern "C" fn ...` 无参函数，置于最靠前编译单元，保证
  `.text` 拼接后入口位于 code 段起始（当前打包按归档对象顺序拼接，入口稳定性依赖
  单对象/链接顺序，完整装载落地后按加载器约定校准）。

## 7. 编译与打包流程

```bash
# 1) 编译（thumbv7a 独立打包）
cd apps/<app>
cargo build --target thumbv7a-none-eabi --release --features kir-standalone

# 2) 打包（参考 scripts/pack-apkcompat.py，步骤固定）：
#    a. ar 解包 target/.../release/lib<app>.a
#    b. 逐个对象 objcopy -O binary -j .text -j .rodata，按序拼接 → code 段
#       （工具探测顺序：llvm-objcopy / arm-none-eabi-objcopy / objcopy，ar 同理）
#    c. 生成图标：PNG 原始字节（推荐 48×48 手写 PNG 编码，见脚本 make_icon）
#    d. 写 manifest JSON（见第 5 节）
#    e. 按 SKIR 布局拼装：头(32B) + code + icon + manifest
#    f. 自验证：魔数 / 段边界 / 图标 PNG 头 / manifest 内容

# 3) 验证
cd ../.. && python3 scripts/pack-apkcompat.py    # 生成并自验证 apkcompat.kir
```

其它可用打包入口：

- `crates/kirax-core/src/kir.rs::pack(code, icon, _w, _h, name, author, version, ...)`
  → Rust API 打包（host 可测，单测含真实产物 roundtrip）；
- `kirax-studio/src-tauri/src/kirpack.rs::pack(code, icon, manifest)`
  → 纯 Rust 打包/解包（无系统依赖，跨平台）；
- `scripts/pack-apkcompat.py` → 完整参考流水线（编译 → 提取 → 图标 → 打包 → 自验证）。

## 8. 常见坑

| 症状 | 原因 |
|---|---|
| 加载报"魔数错误：不是 .kir(SKIR) 文件" | 文件不是 SKIR，或偏移写错；误用 KIR1 头 |
| 报"版本不兼容" | 头部 version ≠ 1 |
| 报"段越界：文件被截断" | 某段 offset+size 超出文件尾 |
| 应用跑飞 / 乱执行 | 把 ELF 当 code 段；或代码含未重定位的绝对地址 |
| 链接报缺分配器/panic_handler | 独立打包未开 `--features kir-standalone` |
| 加载报"文件系统未就绪" / ENOENT | SD/FAT32 未挂载，或路径不在 `/APPS/*.kir` |
| 单测与打包产物行为不一致 | host 路径（std）与 kir-standalone 路径分配器不同，确认 feature 开关 |

## 9. 自验证清单（交付前逐项过）

1. `cargo test`（host）全绿；
2. `cargo build --target thumbv7a-none-eabi --release --features kir-standalone` 0 error 0 warning；
3. 打包脚本自验证 PASS（魔数 / 段边界 / 图标 PNG / manifest）；
4. 用 `kirax-core::kir::KirHeader::parse` 复解析产物通过；
5. 单文件 ≤ 256KB；manifest 含 `name` / `author` / `version`。
