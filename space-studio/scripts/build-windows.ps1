# Space Studio — Windows 原生构建脚本（PowerShell）
# 前置：安装 Rust MSVC 工具链 + VS Build Tools (C++ workload) + WebView2
# 用法：powershell -ExecutionPolicy Bypass -File scripts/build-windows.ps1
$ErrorActionPreference = "Stop"

Write-Host "==== Space Studio Windows (MSVC) 构建 ====" -ForegroundColor Cyan

# 1. 添加 MSVC target
rustup target add x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw "rustup target add 失败" }

# 2. 安装 tauri-cli（如缺失）
if (-not (Get-Command cargo-tauri -ErrorAction SilentlyContinue)) {
    cargo install tauri-cli --version "^2"
}

# 3. 生成图标与元数据
cargo tauri icon src-tauri/icons/128x128.png

# 4. 构建（NSIS 安装器 + 便携 exe）
cargo tauri build --target x86_64-pc-windows-msvc

$exe = "src-tauri\target\x86_64-pc-windows-msvc\release\SpaceStudio.exe"
if (Test-Path $exe) {
    Write-Host "构建成功: $exe" -ForegroundColor Green
    Get-Item $exe | Select-Object Length, LastWriteTime
} else {
    throw "未找到产物 $exe"
}

# 5. NSIS 安装器
$installer = Get-ChildItem "src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\*.exe" -ErrorAction SilentlyContinue
if ($installer) {
    Write-Host "安装器: $($installer.FullName)" -ForegroundColor Green
}
