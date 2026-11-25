#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
#
# 编译 t9 验收用的最小探针 APK（不依赖 Gradle）：
#   javac -> d8 -> aapt2 link -> zip classes.dex -> zipalign -> apksigner
# 任何一步失败立即退出（不吞编译错误）。
set -euo pipefail

SDK="${ANDROID_SDK_ROOT:-/opt/homebrew/share/android-commandlinetools}"
BT="$SDK/build-tools/34.0.0"
ANDROID_JAR="$SDK/platforms/android-34/android.jar"
HERE="$(cd "$(dirname "$0")" && pwd)"
BUILD="$HERE/build"

rm -rf "$BUILD"
mkdir -p "$BUILD/classes" "$BUILD/dex"

echo "== javac =="
javac -Xlint:-options -source 8 -target 8 -bootclasspath "$ANDROID_JAR" -d "$BUILD/classes" \
    "$HERE/src/io/df/egressprobe/MainActivity.java"

echo "== d8 =="
# shellcheck disable=SC2046
"$BT/d8" --min-api 29 --lib "$ANDROID_JAR" --output "$BUILD/dex" \
    $(find "$BUILD/classes" -name '*.class')

echo "== aapt2 link =="
"$BT/aapt2" link -o "$BUILD/base.apk" -I "$ANDROID_JAR" \
    --manifest "$HERE/AndroidManifest.xml" \
    --min-sdk-version 29 --target-sdk-version 34

echo "== add classes.dex =="
(cd "$BUILD/dex" && zip -q -X "$BUILD/base.apk" classes.dex)

echo "== zipalign =="
"$BT/zipalign" -f 4 "$BUILD/base.apk" "$BUILD/aligned.apk"

echo "== keystore =="
if [ ! -f "$HERE/debug.keystore" ]; then
    keytool -genkeypair -keystore "$HERE/debug.keystore" -storepass android -keypass android \
        -alias androiddebugkey -keyalg RSA -keysize 2048 -validity 3650 \
        -dname "CN=EgressProbe, OU=t9, O=device_faker, L=NA, S=NA, C=NA" >/dev/null 2>&1
fi

echo "== apksigner =="
"$BT/apksigner" sign --ks "$HERE/debug.keystore" --ks-pass pass:android \
    --key-pass pass:android --ks-key-alias androiddebugkey \
    --out "$HERE/egress-probe.apk" "$BUILD/aligned.apk"

"$BT/apksigner" verify --print-certs "$HERE/egress-probe.apk" | head -3
ls -la "$HERE/egress-probe.apk"
