#!/usr/bin/env bash
# Build dist/Audemo-<version>-arm64.apk (64-bit ARM: the Galaxy Tab S9
# Ultra and every current Android tablet and phone).
#
# Needs: Rust with the aarch64-linux-android target, the Android SDK
# (ANDROID_HOME, with build-tools and a platform) and NDK (ANDROID_NDK_HOME),
# and a JDK for keytool. No Gradle: Audemo is a NativeActivity with no Java.
#
# Signing: AUDEMO_ANDROID_KEYSTORE / AUDEMO_ANDROID_KEYSTORE_PASS (alias
# "audemo"). Without them a debug key is made in ~/.android, and an APK
# signed with a different key can't be installed over this one.
set -euo pipefail
cd "$(dirname "$0")/.."

: "${ANDROID_HOME:=${ANDROID_SDK_ROOT:-}}"
: "${ANDROID_HOME:?Set ANDROID_HOME to the Android SDK}"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}"
[ -d "$NDK" ] || { echo "Set ANDROID_NDK_HOME to the Android NDK" >&2; exit 1; }
export ANDROID_NDK_HOME="$NDK" ANDROID_NDK_ROOT="$NDK"

MIN_SDK=30      # Android 11: All files access
TARGET_SDK=34
BT=$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)
PLATFORM_JAR=$(ls -d "$ANDROID_HOME"/platforms/android-* | sort -V | tail -1)/android.jar
HOST_TAG=$(ls "$NDK/toolchains/llvm/prebuilt" | head -1)
TOOLS="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin"
echo "build-tools: $BT"
echo "platform:    $PLATFORM_JAR"
echo "NDK:         $NDK"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
IFS=. read -r v_major v_minor v_patch <<< "$version"
version_code=$((v_major * 10000 + v_minor * 100 + v_patch))

# ------------------------------------------------------------ native library
TARGET=aarch64-linux-android
export CC_aarch64_linux_android="$TOOLS/${TARGET}${MIN_SDK}-clang"
export CXX_aarch64_linux_android="$TOOLS/${TARGET}${MIN_SDK}-clang++"
export AR_aarch64_linux_android="$TOOLS/llvm-ar"
export RANLIB_aarch64_linux_android="$TOOLS/llvm-ranlib"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
# FDK AAC includes AOSP's <log/log.h> on Android; a stand-in is provided.
export CFLAGS_aarch64_linux_android="-I$PWD/assets/android/include"
export CXXFLAGS_aarch64_linux_android="-I$PWD/assets/android/include"
cargo rustc --target "$TARGET" --lib --release --crate-type cdylib

SO="target/$TARGET/release/libaudemo.so"
STAGE=target/android-apk
rm -rf "$STAGE"
mkdir -p "$STAGE/lib/arm64-v8a" dist
cp "$SO" "$STAGE/lib/arm64-v8a/"
# The C++ runtime, if Oboe (or anything else) links it dynamically.
if "$TOOLS/llvm-readelf" -d "$SO" | grep -q 'libc++_shared.so'; then
    cp "$NDK/toolchains/llvm/prebuilt/$HOST_TAG/sysroot/usr/lib/$TARGET/libc++_shared.so" "$STAGE/lib/arm64-v8a/"
fi
"$TOOLS/llvm-readelf" -d "$STAGE/lib/arm64-v8a/libaudemo.so" | grep NEEDED || true

# ------------------------------------------------------------ APK
"$BT/aapt2" compile --dir assets/android/res -o "$STAGE/res.zip"
"$BT/aapt2" link -o "$STAGE/unaligned.apk" -I "$PLATFORM_JAR" \
    --manifest assets/android/AndroidManifest.xml \
    --version-code "$version_code" --version-name "$version" \
    --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
    "$STAGE/res.zip"
(cd "$STAGE" && zip -q -r unaligned.apk lib)
"$BT/zipalign" -f -p 4 "$STAGE/unaligned.apk" "$STAGE/aligned.apk"

KS="${AUDEMO_ANDROID_KEYSTORE:-$HOME/.android/audemo-debug.keystore}"
PASS="${AUDEMO_ANDROID_KEYSTORE_PASS:-android}"
if [ ! -f "$KS" ]; then
    mkdir -p "$(dirname "$KS")"
    keytool -genkeypair -keystore "$KS" -storepass "$PASS" -keypass "$PASS" -alias audemo \
        -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Audemo,O=M0JSX,C=GB"
fi
OUT="dist/Audemo-$version-arm64.apk"
"$BT/apksigner" sign --ks "$KS" --ks-pass "pass:$PASS" --key-pass "pass:$PASS" --ks-key-alias audemo --out "$OUT" "$STAGE/aligned.apk"
"$BT/apksigner" verify "$OUT"
ls -l "$OUT"
