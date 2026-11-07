# kirax-cli

KiraX OS 应用构建命令行工具（宿主端，纯 Rust，零第三方依赖）。

## 构建

```bash
cargo build            # 产出 target/debug/kirax-cli
cargo build --release
```

## 命令

```text
kirax-cli <new|build|run|package|install>
```

| 命令 | 行为（已实跑验证） |
|---|---|
| `kirax-cli`（无参数） | 打印用法：`用法: kirax-cli <new|build|run|package|install>` |
| `kirax-cli new <name>` | 在当前目录生成 `apps/<name>/` 脚手架：`Cargo.toml` + `src/main.rs`（`#![no_std] #![no_main]` 模板） |
| `kirax-cli build` | 透传执行 `cargo build --release`（需在一个含 Cargo 工程的目录里运行） |
| `kirax-cli run` | 先 `cargo build --release`，再用 QEMU 启动 `build/qemu-kernel.elf`（需 `qemu-system-arm`） |
| `kirax-cli package` | 读 `target/thumbv7a-none-eabi/release/app`（Thumb ELF），按 SKIR 格式写出 `build/app.kir`；ELF 不存在时以空代码段打包（仅 32B 头） |
| `kirax-cli install <name>` | 打印安装提示（实际写盘需真机挂载 SD，未实现） |

### `package` 产物格式（SKIR，32 字节头）

```text
"SKIR" | version u32=1 | code_off(=32) code_len | icon_off=0 icon_len=0 | man_off=0 man_len=0 | code
```

> 注意：本工具的 `package` 是通用脚手架，写死读取 `target/thumbv7a-none-eabi/release/app`、
> 不接应用目录参数、也不内嵌图标/manifest。带图标的完整应用打包请用
> `scripts/pack-apkcompat.py`（见 apps/apkcompat）或 kirax-studio 的 kirpack。

## 实跑记录（宿主 cargo 1.98.1）

- `cargo build`：OK，产出 `target/debug/kirax-cli`。
- `new myapp`：正确生成 `apps/myapp/Cargo.toml` 与 `apps/myapp/src/main.rs`。
- `package`（无 ELF）：生成 `build/app.kir`（32 字节，魔数 `SKIR`，code_len=0）。
- `install demo`：打印安装提示，退出码 0。
