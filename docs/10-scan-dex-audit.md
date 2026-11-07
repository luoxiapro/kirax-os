# 10 · scan_dex 真实 DEX 兼容性审计报告（037 样本结案）

> 状态：**已结案**。结论可由 `scripts/verify_scan_vs_baksmali.py` 独立复现。

## 1. 审计对象

| 项 | 值 |
|---|---|
| 037 样本 | `kirax-studio/src-tauri/gen/android/app/build/intermediates/dex/universalRelease/minifyUniversalReleaseWithR8/classes.dex`（2013104B，magic `dex\n037`） |
| 头部表 | 11022 strings / 2860 types / 4105 protos / 6661 fields / 16439 methods / 1965 classes |
| 035 回归样本 | pianshu 构建产物 classes.dex（3920B） |
| 权威 oracle | baksmali 3.0.10 反汇编产物：每类 smali + 逐地址 dump（912351 行） |

## 2. 指令长度修复链（crates/kirax-dex，commit 06b497a）

| # | 修复 | 实证依据 |
|---|---|---|
| 1 | 载荷区间化 `payload_range(start,end)`，重叠去重 `!any(s<r[1] && r[0]<e)` | 修复前幻影 33 种 → 8 种 |
| 2 | lit16 `0xd0..=0xd7 => 2u`（A/B 各 4 位） | dump `16f26c: d370 e803 \| div-int/lit16 v0,v7,1000` |
| 3 | packed-switch 载荷头 `4+sz*4` → `8+sz*4`（补 first_key 4B） | dump `057e24..057e2c`：ident/size/first_key/target |
| 4 | units 表：`0x90..=0xaf => 2u`（23x binop int）拆分自 `0x7b..=0x8f => 1u`（12x 一元） | dump `13fd58: 9008 050e \| add-int v8,v5,v14` 后紧邻 4 字节处 invoke |
| 5 | 31t（fill-array-data / packed / sparse-switch）`3u` | 官方字节码表 + dump 锚点 |

配套：interp.rs 同步官方格式（iget/iput 2u、lit16 A/B 4/4、0x90 并入 binop int）；tests.rs mini/fib 构造字节官方化。

## 3. 计数口径定稿

- **方法数**：代码 11852 / native 4587 = **12780 code_item**，与 baksmali 完全一致（对死）。
- **指令总数**：scan 解码 **257909** vs baksmali 权威指令行 **257910**，**差 1**。
- **唯一方法级差异**：`Ls/d;-><init>()V`（code_off=0x15ca9c，insns_size=300 units）scan 138 / smali 139。
  根因：方法末尾 array-data-payload（ew=1、size=2，10 字节）后需 4 字节对齐到下一 payload，
  2 字节对齐填充 `0x0000` 被 baksmali 反汇编为独立 `nop` 计入；scan 将其归入载荷区间跳过。
  **纯计数口径差异，非 bug**。
- **幻影"未支持"**：037 最终输出仅 `0x0d move-exception ×790`（try/catch 处理器入口，
  baksmali 同样反汇编它，属合理缺口）；035 样本 0 未支持，不回归。

早期口径（226832 smali 文件行数、282659 dump grep 行数）含载荷/伪行/头混排，均不作数。

## 4. 复现

```bash
# 单元测试
cargo test -p kirax-dex            # 11/11 全绿

# 重扫 037 / 035（在 crates/kirax-dex 下）
cargo run --quiet --example scan_dex -- <037_classes.dex>   # 期望仅 0x0d x790
cargo run --quiet --example scan_dex -- <035_classes.dex>   # 期望 0 未支持

# 逐方法权威对拍（需 baksmali smaliout 目录）
python3 scripts/verify_scan_vs_baksmali.py <classes.dex> <smaliout_dir>
# 期望输出: 代码方法数 11852 / scan 257909 / baksmali 257910 / 差异方法数 1(Ls/d <init>)
```

## 5. 提交记录

- `06b497a` dex: 官方指令长度修正（上述修复链）+ 037 幻影清零
- `4918cb0` kirax-dex: 逐方法对拍脚本（本报告结论的可复现依据）
