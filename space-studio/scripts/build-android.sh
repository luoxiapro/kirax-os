#!/usr/bin/env bash
# Space Studio — Android APK 构建（实测流程，Linux 主机）
# 前置：
#   - JDK 17：    export JAVA_HOME=/usr/lib/jvm/java-17-openjdk-amd64
#   - Android SDK：export ANDROID_HOME=/path/to/android-sdk
#   - Android NDK：export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/<版本>
set -euo pipefail
cd "$(dirname "$0")/.."

[ -n "${ANDROID_HOME:-}" ] || { echo "缺少 ANDROID_HOME"; exit 1; }
[ -n "${ANDROID_NDK_HOME:-}" ] || { echo "缺少 ANDROID_NDK_HOME"; exit 1; }

echo "==== Space Studio Android APK 构建 ===="
BT="$ANDROID_HOME/build-tools/35.0.0"

# 进入 Rust 工程目录（Cargo.toml / tauri.conf.json 所在处）
cd src-tauri

# 1. Rust Android targets
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android

# 2. cargo-ndk
command -v cargo-ndk >/dev/null || cargo install cargo-ndk --locked

# 3. 初始化 Android 工程（首次；需 tauri-cli，cargo install tauri-cli --version "^2"）
if [ ! -d "gen/android" ]; then
  cargo tauri android init
fi

# 4. 编译三 ABI 原生库到 jniLibs（绕开 tauri WS 守护）
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 \
  -o gen/android/app/src/main/jniLibs build --release

# 5. gradle 打包（禁用 rust 任务，jniLibs 已就位）
cd gen/android
./gradlew :app:assembleUniversalRelease \
  -x :app:rustBuildArm64Release -x :app:rustBuildArmRelease \
  -x :app:rustBuildX86_64Release -x :app:rustBuildX86Release

# 6. 签名（生成 keystore：keytool -genkeypair -keystore spaceos.keystore -alias spaceos \
#      -keyalg RSA -keysize 2048 -validity 10000 -storepass spaceos123 -keypass spaceos123 \
#      -dname "CN=Space OS,OU=Space Studio,O=Space OS,L=Zhengzhou,ST=Henan,C=CN"）
KS="${SPACE_OS_KEYSTORE:-/tmp/spaceos.keystore}"
KS_PASS="${SPACE_OS_KEYSTORE_PASS:-spaceos123}"
UNSIGNED="app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk"
"$BT/zipalign" -f 4 "$UNSIGNED" /tmp/space-studio-aligned.apk
"$BT/apksigner" sign --ks "$KS" --ks-pass "pass:$KS_PASS" --key-pass "pass:$KS_PASS" \
  --out ../../dist/SpaceStudio_0.1.0.apk /tmp/space-studio-aligned.apk
"$BT/apksigner" verify ../../dist/SpaceStudio_0.1.0.apk

echo "构建成功: src-tauri/dist/SpaceStudio_0.1.0.apk"
