#!/usr/bin/env bash
# Android APK 构建脚本（Windows / Git Bash）
#
# 为什么需要脚本而不是直接 `pnpm tauri android build`：
#   1. Tauri 的 Android 模板会用「符号链接」把编好的 .so 放进 jniLibs，
#      在本机（Windows，无开发者模式）这一步会留下一个 0 字节的占位文件，
#      导致产出的 APK 里 .so 是空的（装上就崩）。
#   2. 因此这里改为「自己 cargo 编译 → 自己拷贝 .so → 让 Gradle 只做打包」，
#      跳过 Gradle 里会覆盖 .so 的 rustBuild* 任务。
#   3. 顺带启用 keystore.properties 里的自签名，产出可直接安装的 APK。
#
# 前置条件（一次性）：
#   - gen/android 已通过 `pnpm tauri android init` 生成
#   - 已应用 doc/11 第 2.2 / 2.4 / 2.5 / 2.6 / 2.7 节的全部补丁
#   - gen/android/keystore.properties 与 nofakeip-release.jks 已生成
#
# 用法：
#   bash tools/build-android.sh            # 默认 arm64-v8a
#   ABI=arm64 bash tools/build-android.sh
#
# 产物：desktop/src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ABI="${ABI:-arm64}"

# ABI -> (rust target, jniLibs 目录名)
case "$ABI" in
  arm64)  RUST_TARGET="aarch64-linux-android";   JNI_DIR="arm64-v8a" ;;
  armv7)  RUST_TARGET="armv7-linux-androideabi"; JNI_DIR="armeabi-v7a" ;;
  x86)    RUST_TARGET="i686-linux-android";      JNI_DIR="x86" ;;
  x86_64) RUST_TARGET="x86_64-linux-android";    JNI_DIR="x86_64" ;;
  *) echo "不支持的 ABI: $ABI（可选 arm64 / armv7 / x86 / x86_64）" >&2; exit 1 ;;
esac

: "${ANDROID_HOME:?请先设置 ANDROID_HOME}"
: "${JAVA_HOME:?请先设置 JAVA_HOME（需要 JDK 21）}"

SRC_TAURI="$ROOT/desktop/src-tauri"
ANDROID_DIR="$SRC_TAURI/gen/android"
SO_NAME="libnofakeip_desktop_lib.so"

echo "==> 1/3 编译 Rust 动态库（$RUST_TARGET）"
cd "$SRC_TAURI"
cargo build --release --target "$RUST_TARGET"

echo "==> 2/3 把 .so 放进 jniLibs（绕过 Tauri 的符号链接步骤）"
mkdir -p "$ANDROID_DIR/app/src/main/jniLibs/$JNI_DIR"
cp -f "$SRC_TAURI/target/$RUST_TARGET/release/$SO_NAME" \
      "$ANDROID_DIR/app/src/main/jniLibs/$JNI_DIR/$SO_NAME"
ls -l "$ANDROID_DIR/app/src/main/jniLibs/$JNI_DIR/$SO_NAME"

echo "==> 3/3 Gradle 打包（跳过 rustBuild* 任务，避免 .so 被覆盖成 0 字节）"
cd "$ANDROID_DIR"
./gradlew assembleUniversalRelease \
  -x rustBuildUniversalRelease \
  -x rustBuildArm64Release \
  -x rustBuildArmRelease \
  -x rustBuildX86Release \
  -x rustBuildX86_64Release

APK="$ANDROID_DIR/app/build/outputs/apk/universal/release/app-universal-release.apk"
echo
echo "==> 完成：$APK"
ls -l "$APK"

echo "==> 校验签名"
"$ANDROID_HOME/build-tools/36.0.0/apksigner.bat" verify --print-certs "$APK" | head -4
