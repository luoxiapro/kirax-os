# APK 兼容器（Space OS 自带软件 · 参考鸿蒙卓易通）

在 Space OS 桌面上解析、安装、卸载与管理 Android APK 文件的系统应用，
沿用 Glass 深空液态玻璃设计语言（几何行星图标、圆角面板、性能降级开关）。

## 当前已实现（真实可运行）
- **ZIP/APK 容器解析**：EOCD 定位 → 中心目录 → Local Header → 文件提取（Store 方法）
- **AndroidManifest.xml 二进制 XML 解析**：字符串池（UTF-8/UTF-16）→ StartElement
  属性表 → 提取 package、label、icon 引用
- **图标提取**：从 res/mipmap-* 取出 PNG 原始数据
- **兼容层应用模型**：CompatApp（包名/显示名/图标/运行状态）与安装管理入口

## 路线图
1. DEX 字节码解码与解释器（或 AOT 转译到 .kir 中间码）
2. Binder 服务抽象（Intent / PackageManager / ActivityManager 微内核）
3. ART-like 对象模型（堆内句柄 + 弱引用 + 标清 GC）
4. resources.arsc 完整资源表解析（当前 label 资源引用走 strings.xml 兜底）

## 目录
- `src/lib.rs`：解析器与兼容层管理器（no_std，host 可测）
- `tests/apkcompat.rs`：6 个单测（ZIP 提取/魔数/缺失条目/XML 包名/整包检查/缺 manifest）
- `manifest.json`：.kir 应用清单（Glass 风格 UI 说明）
- `apkcompat.kir`：打包产物（SKIR：code=1840B + icon=775B PNG + manifest=1013B）

## 构建与验证
```bash
cd apps/apkcompat && cargo test          # host 单测 6/6
cargo build --target thumbv7a-none-eabi --release --features kir-standalone
cd ../.. && python3 scripts/pack-apkcompat.py   # 生成并自验证 apkcompat.kir
```
