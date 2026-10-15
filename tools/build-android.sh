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
#     （其中 E15「edge-to-edge 安全区」由本脚本从 tools/android/ 自动写入，无需手动改）
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
# 注意：tauri 的 --target 用的是 aarch64 / armv7，不是 arm64。
#
# 只出 ARM：本应用面向手机竖屏（doc/11 第 2.10 节），不做 x86 适配。
# 需要在 x86 模拟器上调试时，直接装 arm64 包即可 ——
# WSA 与主流模拟器（MuMu 等）都带 ARM 翻译层，实测可正常运行。
case "$ABI" in
  arm64)  TAURI_TARGET="aarch64"; RUST_TARGET="aarch64-linux-android";   JNI_DIR="arm64-v8a" ;;
  armv7)  TAURI_TARGET="armv7";   RUST_TARGET="armv7-linux-androideabi"; JNI_DIR="armeabi-v7a" ;;
  *) echo "不支持的 ABI: $ABI（本应用只出 arm64 / armv7）" >&2; exit 1 ;;
esac

: "${ANDROID_HOME:?请先设置 ANDROID_HOME}"
: "${JAVA_HOME:?请先设置 JAVA_HOME（需要 JDK 21）}"

SRC_TAURI="$ROOT/desktop/src-tauri"
ANDROID_DIR="$SRC_TAURI/gen/android"
SO_NAME="libnofakeip_desktop_lib.so"
JNI_SO="$ANDROID_DIR/app/src/main/jniLibs/$JNI_DIR/$SO_NAME"
APK="$ANDROID_DIR/app/build/outputs/apk/universal/release/app-universal-release.apk"

echo "==> 0/5 校验前端产物完整性"
node "$ROOT/tools/verify-dist.mjs"

echo "==> 1/5 应用 Android 原生模板补丁（E8/E15/E16）"
APP_SRC="$ANDROID_DIR/app/src/main"
MAIN_ACTIVITY="$(find "$APP_SRC/java" -name MainActivity.kt 2>/dev/null | head -1)"
if [ -z "$MAIN_ACTIVITY" ]; then
  echo "找不到 MainActivity.kt，请先执行 pnpm tauri android init" >&2
  exit 1
fi

apply_patch() {
  local src="$1" dst="$2"
  [ -f "$src" ] || { echo "缺少模板 $src" >&2; exit 1; }
  if [ ! -f "$dst" ] || ! cmp -s "$src" "$dst"; then
    mkdir -p "$(dirname "$dst")"
    cp -f "$src" "$dst"
    echo "    已写入 ${dst#"$ANDROID_DIR"/}"
  else
    echo "    已是最新 ${dst#"$ANDROID_DIR"/}"
  fi
}

apply_patch "$ROOT/tools/android/MainActivity.kt"              "$MAIN_ACTIVITY"
apply_patch "$ROOT/tools/android/themes.xml"                   "$APP_SRC/res/values/themes.xml"
apply_patch "$ROOT/tools/android/themes-night.xml"             "$APP_SRC/res/values-night/themes.xml"
apply_patch "$ROOT/tools/android/AndroidManifest.xml"          "$APP_SRC/AndroidManifest.xml"
apply_patch "$ROOT/tools/android/network_security_config.xml"  "$APP_SRC/res/xml/network_security_config.xml"

# 启动图标同样落在 gen/ 里，重新 init 后会被重置回 Tauri 默认图标，
# 这里从 tools/icon_source.png 重新生成各密度 mipmap。
echo "    重新生成 Android 启动图标"
"${PYTHON:-python}" "$ROOT/tools/make_icons.py" --android-only

echo "==> 2/5 让 Tauri 编译 Rust 动态库（$RUST_TARGET）"
echo "    （这一步产出的 APK 里 .so 会是 0 字节，下一步修复）"
cd "$ROOT/desktop"
./node_modules/.bin/tauri android build --apk --target "$TAURI_TARGET" --ci \
  --config '{"build":{"beforeBuildCommand":""}}'

echo "==> 3/5 用真实 .so 覆盖 jniLibs 里的 0 字节文件"
REAL_SO="$SRC_TAURI/target/$RUST_TARGET/release/$SO_NAME"
[ -f "$REAL_SO" ] || { echo "找不到 $REAL_SO" >&2; exit 1; }

# ⚠️ 先清掉其它 ABI 的残留目录。
# 换 ABI 构建（例如之前试过 x86_64）会在 jniLibs 下留下 0 字节的同名 .so，
# 而 Android 会优先加载设备本地 ABI 的那一份 —— 结果是装上就崩：
#   java.lang.UnsatisfiedLinkError: dlopen failed
# 这类问题不会在构建期报错，只能在运行期暴露，所以这里主动清理。
JNI_ROOT="$ANDROID_DIR/app/src/main/jniLibs"
if [ -d "$JNI_ROOT" ]; then
  find "$JNI_ROOT" -mindepth 1 -maxdepth 1 -type d ! -name "$JNI_DIR" -print0 \
    | while IFS= read -r -d '' stale; do
        echo "    清理残留 ABI 目录：${stale#"$ANDROID_DIR"/}"
        rm -rf "$stale"
      done
fi

mkdir -p "$(dirname "$JNI_SO")"
cp -f "$REAL_SO" "$JNI_SO"
ls -l "$JNI_SO"

echo "==> 4/5 Gradle 重新打包（跳过 rustBuild* 任务，避免 .so 又被写成 0 字节）"
cd "$ANDROID_DIR"
./gradlew assembleUniversalRelease \
  -x rustBuildUniversalRelease \
  -x rustBuildArm64Release \
  -x rustBuildArmRelease \
  -x rustBuildX86Release \
  -x rustBuildX86_64Release

echo "==> 5/5 校验产物"
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
