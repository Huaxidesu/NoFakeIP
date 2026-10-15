#!/usr/bin/env bash
# CI 专用：tauri android init 之后重放本地的 Android 环境适配补丁。
#
# 背景（见 doc/11-构建发布与Android.md）：gen/ 不入库，CI 上 `pnpm tauri android init --ci`
# 生成的工程是 Tauri 默认模板，缺三类东西，产出的 APK 就会「装不上 / 图标是默认的」：
#   1. E8/E15/E16 原生模板补丁（MainActivity / themes / Manifest / 网络安全配置）
#   2. E18 启动图标 —— init 会把 mipmap 重置回 Tauri 默认图标
#   3. E11 签名配置 —— 默认模板只产出 *-unsigned.apk，
#      安装时报 INSTALL_PARSE_FAILED_NO_CERTIFICATES
#
# 签名密钥来源（按优先级）：
#   a. 环境变量 ANDROID_KEYSTORE_BASE64（GitHub Secret 里放 jks 的 base64，
#      保证各次构建签名一致，可覆盖安装升级）
#   b. 都没有时用 keytool 现生成一份自签名开发密钥（每次构建签名都不同，
#      只能全新安装，覆盖升级会因签名不一致被拒）
#
# 用法：在 `pnpm tauri android init --ci` 之后、`tauri android build` 之前执行
#   bash tools/ci-android-setup.sh

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_DIR="$ROOT/desktop/src-tauri/gen/android"
APP_SRC="$ANDROID_DIR/app/src/main"

: "${JAVA_HOME:?需要 JAVA_HOME（keytool 生成密钥用）}"

# —————————————————— 1/3 原生模板补丁（E8/E15/E16） ——————————————————

echo "==> 1/3 应用 Android 原生模板补丁（E8/E15/E16）"
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

# —————————————————— 2/3 启动图标（E18） ——————————————————

echo "==> 2/3 重新生成 Android 启动图标"
"${PYTHON:-python3}" "$ROOT/tools/make_icons.py" --android-only

# —————————————————— 3/3 签名配置（E11） ——————————————————

echo "==> 3/3 配置 APK 签名（E11）"
cd "$ANDROID_DIR"

KEYSTORE="$ANDROID_DIR/nofakeip-release.jks"
if [ -n "${ANDROID_KEYSTORE_BASE64:-}" ]; then
  echo "    使用 Secret ANDROID_KEYSTORE_BASE64 提供的 keystore（签名跨构建稳定）"
  echo "$ANDROID_KEYSTORE_BASE64" | base64 -d > "$KEYSTORE"
else
  echo "    未设置 ANDROID_KEYSTORE_BASE64，现生成自签名开发密钥"
  echo "    ⚠️ 每次构建签名都不同：只能全新安装，覆盖升级会被签名校验拒绝。"
  echo "    ⚠️ 需要稳定的可升级签名时，把 jks 的 base64 配到 Secret 里。"
  keytool -genkeypair -v \
    -keystore "$KEYSTORE" -storetype PKCS12 -storepass nofakeip \
    -alias nofakeip -keypass nofakeip -keyalg RSA -keysize 2048 -validity 36500 \
    -dname "CN=NoFakeIP, OU=Dev, O=NoFakeIP, C=CN"
fi

cat > keystore.properties <<'EOF'
storeFile=nofakeip-release.jks
storePassword=nofakeip
keyAlias=nofakeip
keyPassword=nofakeip
EOF
echo "    已写入 keystore.properties"

# 给默认模板的 build.gradle.kts 注入签名配置（幂等：已打补丁则跳过）。
GRADLE="$ANDROID_DIR/app/build.gradle.kts"
if grep -q "补丁 E11" "$GRADLE"; then
  echo "    build.gradle.kts 已含签名补丁，跳过注入"
else
  "${PYTHON:-python3}" - "$GRADLE" <<'PYEOF'
import sys

path = sys.argv[1]
src = open(path, encoding="utf-8").read()

# 1) keystore 属性加载：插在 android { 之前
anchor = "android {"
prop_block = '''// ⚠️ 环境适配补丁 E11（见 doc/11-构建发布与Android.md 第 2.7 节）：
// 不配置签名时只产出 *-unsigned.apk，Android 拒绝安装
// （INSTALL_PARSE_FAILED_NO_CERTIFICATES）。
// 从 gen/android/keystore.properties 读取密钥；文件不存在时自动跳过。
val keystorePropertiesFile = rootProject.file("keystore.properties")
val keystoreProperties = Properties().apply {
    if (keystorePropertiesFile.exists()) {
        keystorePropertiesFile.inputStream().use { load(it) }
    }
}

'''
if anchor not in src:
    sys.exit("build.gradle.kts 里找不到 android { 锚点")
src = src.replace(anchor, prop_block + anchor, 1)

# 2) signingConfigs 块：插在 android { 之后
sign_cfg = '''
    // 补丁 E11：自签名配置（见文件头说明）
    signingConfigs {
        create("release") {
            if (keystorePropertiesFile.exists()) {
                storeFile = rootProject.file(keystoreProperties["storeFile"] as String)
                storePassword = keystoreProperties["storePassword"] as String
                keyAlias = keystoreProperties["keyAlias"] as String
                keyPassword = keystoreProperties["keyPassword"] as String
            }
        }
    }
'''
src = src.replace(anchor, anchor + sign_cfg, 1)

# 3) release 启用签名
rel_anchor = 'getByName("release") {'
if rel_anchor not in src:
    sys.exit("build.gradle.kts 里找不到 release buildType 锚点")
rel_patch = '''
            // 补丁 E11：有 keystore.properties 时才启用签名
            if (keystorePropertiesFile.exists()) {
                signingConfig = signingConfigs.getByName("release")
            }
'''
src = src.replace(rel_anchor, rel_anchor + rel_patch, 1)

open(path, "w", encoding="utf-8", newline="\n").write(src)
print("    已注入签名配置到 build.gradle.kts")
PYEOF
fi

echo "==> CI Android 环境准备完成"
