#!/usr/bin/env bash
# Android APK 构建脚本（Windows / Git Bash）
#
# 为什么需要脚本而不是直接 `pnpm tauri android build`：
#   1. Tauri 的 Android 构建会用「符号链接」把编好的 .so 放进 jniLibs，
#      在本机（Windows，无开发者模式）这一步会留下一个 0 字节的占位文件，
#      导致产出的 APK 里 .so 是空的（装上就崩，且构建不报错）。
#      因此这里改为「让 Tauri 编好 .so → 手动拷贝 → 让 Gradle 只做打包」。
#   2. 顺带启用 keystore.properties 里的自签名，产出可直接安装的 APK。
#   3. 打包后校验 APK 内 .so 的实际大小，避免再次静默产出坏包。
#
# ⚠️ 为什么不让脚本自己调 cargo：
#   `cargo build --target aarch64-linux-android` 会因为缺少 Tauri 设置的
#   CC_<target> / RUSTFLAGS 等环境变量而编译 ring 失败
#   （报 `fatal error: 'assert.h' file not found`）。
#   所以编译这一步必须交给 `tauri android build` 来做。
#
# 前置条件（一次性）：
#   - gen/android 已通过 `pnpm tauri android init` 生成
#   - 已应用 doc/11 第 2.2 / 2.4 / 2.5 / 2.6 / 2.7 节的全部补丁
#   - gen/android/keystore.properties 与 nofakeip-release.jks 已生成
#   - desktop/dist 已构建（脚本会先跑 node tools/verify-dist.mjs 校验）
#
# 用法：
#   bash tools/build-android.sh                 # 默认 arm64
#   ABI=armv7 bash tools/build-android.sh
#
# 产物：desktop/src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ABI="${ABI:-arm64}"

# ABI 简称 -> (tauri --target 取值, rust target triple, jniLibs 目录名)
# 注意：tauri 的 --target 用的是 aarch64 / armv7 / i686 / x86_64，不是 arm64。
case "$ABI" in
  arm64)  TAURI_TARGET="aarch64"; RUST_TARGET="aarch64-linux-android";   JNI_DIR="arm64-v8a" ;;
  armv7)  TAURI_TARGET="armv7";   RUST_TARGET="armv7-linux-androideabi"; JNI_DIR="armeabi-v7a" ;;
  x86)    TAURI_TARGET="i686";    RUST_TARGET="i686-linux-android";      JNI_DIR="x86" ;;
  x86_64) TAURI_TARGET="x86_64";  RUST_TARGET="x86_64-linux-android";    JNI_DIR="x86_64" ;;
  *) echo "不支持的 ABI: $ABI（可选 arm64 / armv7 / x86 / x86_64）" >&2; exit 1 ;;
esac

: "${ANDROID_HOME:?请先设置 ANDROID_HOME}"
: "${JAVA_HOME:?请先设置 JAVA_HOME（需要 JDK 21）}"

SRC_TAURI="$ROOT/desktop/src-tauri"
ANDROID_DIR="$SRC_TAURI/gen/android"
SO_NAME="libnofakeip_desktop_lib.so"
JNI_SO="$ANDROID_DIR/app/src/main/jniLibs/$JNI_DIR/$SO_NAME"
APK="$ANDROID_DIR/app/build/outputs/apk/universal/release/app-universal-release.apk"

echo "==> 0/4 校验前端产物完整性"
node "$ROOT/tools/verify-dist.mjs"

echo "==> 1/4 让 Tauri 编译 Rust 动态库（$RUST_TARGET）"
echo "    （这一步产出的 APK 里 .so 会是 0 字节，下一步修复）"
cd "$ROOT/desktop"
./node_modules/.bin/tauri android build --apk --target "$TAURI_TARGET" --ci \
  --config '{"build":{"beforeBuildCommand":""}}'

echo "==> 2/4 用真实 .so 覆盖 jniLibs 里的 0 字节文件"
REAL_SO="$SRC_TAURI/target/$RUST_TARGET/release/$SO_NAME"
[ -f "$REAL_SO" ] || { echo "找不到 $REAL_SO" >&2; exit 1; }
mkdir -p "$(dirname "$JNI_SO")"
cp -f "$REAL_SO" "$JNI_SO"
ls -l "$JNI_SO"

echo "==> 3/4 Gradle 重新打包（跳过 rustBuild* 任务，避免 .so 又被写成 0 字节）"
cd "$ANDROID_DIR"
./gradlew assembleUniversalRelease \
  -x rustBuildUniversalRelease \
  -x rustBuildArm64Release \
  -x rustBuildArmRelease \
  -x rustBuildX86Release \
  -x rustBuildX86_64Release

echo "==> 4/4 校验产物"
[ -f "$APK" ] || { echo "找不到 APK：$APK" >&2; exit 1; }
ls -l "$APK"

SO_SIZE="$(unzip -l "$APK" | awk '/lib\// {print $1; exit}')"
SO_SIZE="${SO_SIZE:-0}"
if [ "$SO_SIZE" -lt 1000000 ]; then
  echo "❌ APK 里的 .so 只有 $SO_SIZE 字节（应为 10 MB 量级），打包失败" >&2
  exit 1
fi
echo "   .so 大小：$SO_SIZE 字节  ✅"

"$ANDROID_HOME/build-tools/36.0.0/apksigner.bat" verify --print-certs "$APK" | head -4

echo
echo "==> 完成：$APK"
