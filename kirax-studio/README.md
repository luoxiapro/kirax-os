# KiraX Studio — KiraX OS 图形化 IDE

KiraX Studio 是运行在 KiraX OS 桌面上的开发工具，同时发布 **Windows 原生程序** 与 **Android APK**，
界面严格遵循 `../specs/GLASS_DESIGN.md`（深空星球主题 + 液态玻璃，三档性能）。

## 功能

| 模块 | 说明 |
|---|---|
| 项目管理器 | 扫描/新建 KiraX OS 工程（manifest.json + src/main.rs） |
| 代码编辑器 | 行号、Ctrl+S 保存、.kir 应用代码编辑 |
| 可视化 UI 设计器 | 拖拽玻璃面板/按钮/文本/轨道图标到窗口画布，实时调 Glass 参数，导出 ui-layout.json |
| 一键打包 | 封装 .kir（SKIR 魔数 + 代码段 + 图标 + manifest），与内核加载器格式一致 |
| 一键运行 | 拉起 QEMU（-M raspi2b）跑内核 |
| Glass 性能档 | 极致 / 平衡 / 性能 三档，实时切换（对应规范参数表） |

## 目录结构

```
kirax-studio/
├── src/                  # 前端（纯 HTML/CSS/JS，无构建依赖）
│   ├── index.html
│   ├── styles.css        # Glass 设计语言实现
│   └── app.js            # 逻辑（Tauri invoke / 浏览器 mock 双模式）
├── src-tauri/            # Rust 后端（Tauri 2）
│   ├── tauri.conf.json   # 窗口 + NSIS/MSI + Android 配置
│   ├── capabilities/     # 权限
│   ├── icons/            # 已生成（PNG/ICO/ICNS）
│   └── src/
│       ├── main.rs
│       ├── lib.rs        # 命令：工程/文件/打包/运行
│       └── kirpack.rs    # .kir 打包器（纯 Rust，3 个单测）
└── scripts/
    ├── build-windows.ps1 # Windows MSVC → .exe + NSIS 安装器
    └── build-android.sh  # cargo-ndk → APK
```

## 构建 Windows 程序（.exe + NSIS 安装器）

前置：Windows + VS Build Tools（C++ workload）+ Rust MSVC + WebView2 Runtime。

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build-windows.ps1
# 产物：
#   src-tauri\target\x86_64-pc-windows-msvc\release\KiraXStudio.exe
#   src-tauri\target\...\bundle\nsis\KiraXStudio_0.1.0_x64-setup.exe
```

手动步骤等价：
```powershell
rustup target add x86_64-pc-windows-msvc
cargo install tauri-cli --version "^2"
cargo tauri build --target x86_64-pc-windows-msvc
```

## 构建 Android APK

前置：Android SDK + NDK + JDK 17+。

```bash
export ANDROID_HOME=/path/to/android-sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/26.1.10909125   # 或让 cargo-ndk 自动探测
./scripts/build-android.sh
# 产物：
#   src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

## Linux 开发预览

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf
cd src-tauri && cargo build --release
cargo tauri dev   # 需要 tauri-cli
```

## UI 设计器

KiraX Studio 内置可视化 UI 设计器：拖拽玻璃面板 → 导出 .kir 布局 JSON。
所有 UI 遵循 `../specs/GLASS_DESIGN.md`：三档性能、色值、模糊半径参数表内置于设计器。

## 验证状态

- [x] `cargo check`（Linux）通过
- [x] `cargo build --release`（Linux）通过
- [x] `kirpack` 单测 3/3（打包/解包往返、坏魔数拒绝、空代码段）
- [x] 前端 JS 语法检查通过
- [x] 图标（32/128 PNG、ICO、ICNS）已生成
- [x] **Windows .exe**（PE32+ x86-64，MinGW 交叉编译）已产出 → `src-tauri/dist/KiraXStudio.exe`
- [x] **Windows 安装器**（NSIS）已产出 → `src-tauri/dist/KiraXStudio_0.1.0_setup.exe`
- [x] **Android APK**（arm64-v8a/armeabi-v7a/x86_64，已签名）已产出 → `src-tauri/dist/KiraXStudio_0.1.0.apk`
- [x] **Android 启动崩溃已修复**：补上 `#[cfg_attr(mobile, tauri::mobile_entry_point)]`（Tauri 2 移动端必需，缺失导致 .so 无 JNI 符号、UnsatisfiedLinkError 崩溃）；已用 llvm-readelf 验证三 ABI 各导出 22 个 `Java_com_kiraxos_studio_*` 符号

## 实际构建记录（2026-09-26，Linux 主机交叉构建）

Windows：
```bash
apt-get install mingw-w64
rustup target add x86_64-pc-windows-gnu
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
  cargo build --target x86_64-pc-windows-gnu --release
makensis -DSRCROOT=<kirax-os 绝对路径> installer.nsi   # NSIS 安装器
```

Android：
```bash
# SDK + NDK（cmdline-tools → sdkmanager platform-tools / platforms;android-34 / build-tools;35.0.0 / ndk;26.1.10909125）
cargo tauri android init
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o gen/android/app/src/main/jniLibs build --release
# gradle 打包（跳过 tauri WS 守护的 rust 任务，jniLibs 已手动就位）
./gradlew :app:assembleUniversalRelease -x :app:rustBuildArm64Release -x :app:rustBuildArmRelease \
  -x :app:rustBuildX86_64Release -x :app:rustBuildX86Release
# 签名
keytool -genkeypair -keystore kiraxos.keystore -alias kiraxos -keyalg RSA -keysize 2048 -validity 10000
zipalign -f 4 app-universal-release-unsigned.apk aligned.apk
apksigner sign --ks kiraxos.keystore --out KiraXStudio.apk aligned.apk
```
