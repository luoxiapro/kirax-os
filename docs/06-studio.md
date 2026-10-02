# 阶段六：开发者工具链 Space Studio

## 三层形态

1. **space-cli**（Rust，跨平台命令行）：`build` / `run` / `package` / `install`。
2. **VS Code 扩展**：.kir 语法高亮 + 一键打包（过渡工具）。
3. **Space Studio**（图形 IDE，跑在 Space OS 桌面上）：项目管理器、代码编辑器、可视化 UI 设计器、一键打包部署。

## Space Studio 跨平台交付（用户硬要求）

### Windows 原生程序
- 技术栈：Tauri 2.0（Rust 后端 + Web 前端）→ MSVC `x86_64-pc-windows-msvc` target。
- 产物：`SpaceStudio.exe`（NSIS 安装器）。
- 构建：`cargo tauri build --target x86_64-pc-windows-msvc`。

### Android APK
- 技术栈：Tauri Mobile / cargo-ndk。
- 构建：
  ```bash
  cargo install tauri-cli
  rustup target add aarch64-linux-android armv7-linux-androideabi
  cargo install cargo-ndk
  cd space-studio
  cargo tauri android init
  cargo tauri android build --apk
  ```
- 产物：`space-studio/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`。

## space-cli 命令

```bash
space-cli new <app>       # 新建 .kir 项目骨架
space-cli build           # 编译当前项目 → build/*.kir
space-cli run             # 用 QEMU 跑带该应用的镜像
space-cli package         # 打包 .kir
space-cli install <name>  # 安装到 Space OS /apps/
```

## VS Code 扩展

- `tools/vscode-space/syntaxes/space.tmLanguage.json`：语法高亮。
- 打包命令注册：`space-cli package`。

## 常见坑

| 症状 | 原因 |
|---|---|
| Tauri Windows 构建失败 | 没装 MSVC Build Tools / WebView2 |
| Android APK 构建失败 | ANDROID_NDK_ROOT 未设，或 target 没加 |
