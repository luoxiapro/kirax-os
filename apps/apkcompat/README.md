# APK 兼容器（KiraX OS 自带软件 · 参考鸿蒙卓易通）

在 KiraX OS 桌面上解析、安装、卸载与管理 Android APK 文件的系统应用，
沿用 Glass 深空液态玻璃设计语言（几何行星图标、圆角面板、性能降级开关）。

## 当前已实现（真实可运行）
- **ZIP/APK 容器解析**：EOCD 定位 → 中心目录 → Local Header → 文件提取
  （Store 与 **Deflate** 两种压缩方法；Deflate 为 no_std 手写 RFC 1951 inflate）
- **AndroidManifest.xml 二进制 XML 解析**：字符串池（UTF-8/UTF-16）→ StartElement
  属性表 → 提取 package、label、icon 引用
- **图标提取**：从 res/mipmap-* 取出 PNG 原始数据
- **DEX 兼容解码**（接入 crates/kirax-dex 引擎，经 037 真实 DEX baksmali 审计）：
  类/方法/指令统计、opcode 分布与解释器未支持清单、方法级 **.kir 转译骨架**
  （AOT 方向；每行 = 地址 | 助记符 | 原始 code units，与 baksmali dump 同构）
- **兼容层应用模型**：CompatApp（包名/显示名/图标/运行状态）与安装管理入口

## 路线图
1. ~~DEX 字节码解码与解释器~~ → 解码/转译骨架已落地（第 1 步）；解释器
   （interp.rs 已在 kirax-dex 就绪）接入做完整运行
2. Binder 服务抽象（Intent / PackageManager / ActivityManager 微内核）
3. ART-like 对象模型（堆内句柄 + 弱引用 + 标清 GC）
4. resources.arsc 完整资源表解析（当前 label 资源引用走 strings.xml 兜底）

## 目录
- `src/lib.rs`：ZIP/Manifest/兼容层（no_std，host 可测）
- `src/inflate.rs`：Deflate 解压（RFC 1951，stored/fixed/dynamic Huffman）
- `src/dexcompat.rs`：DEX 解码报告 + .kir 转译骨架（复用 kirax-dex）
- `examples/apk_dex.rs`：host 演示入口（`cargo run --example apk_dex -- <apk|dex> [--kir]`）
- `testdata/sample.apk`：真实样本 fixture（035 classes.dex deflate 压缩 + 真实 manifest）
- `tests/apkcompat.rs`：6 个单测；`tests/dex_e2e.rs`：6 个端到端（真实样本）
- `manifest.json`：.kir 应用清单（Glass 风格 UI 说明）
- `apkcompat.kir`：打包产物（SKIR：code=1840B + icon=775B PNG + manifest=1013B）

## 构建与验证
```bash
cd apps/apkcompat && cargo test          # host 测试 15/15（inflate 3 + lib 6 + e2e 6）
# 真实样本端到端（037 定稿口径：257909 条指令、仅 0x0d×790 未支持）
cargo run --example apk_dex -- <真实 classes.dex 路径>
cargo run --example apk_dex -- <真实 .apk 路径>        # ZIP→Deflate→DEX 全链路
cargo run --example apk_dex -- testdata/sample.apk --kir   # .kir 转译骨架
cargo build --target thumbv7a-none-eabi --release --features kir-standalone
cd ../.. && python3 scripts/pack-apkcompat.py   # 生成并自验证 apkcompat.kir
```
