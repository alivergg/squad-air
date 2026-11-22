#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if ! command -v cargo-ndk >/dev/null 2>&1; then
    echo "Error: cargo-ndk is not installed. Install with: cargo install cargo-ndk" >&2
    exit 1
fi

if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
    for candidate in \
        "${ANDROID_HOME:-}/ndk/"* \
        "${ANDROID_SDK_ROOT:-}/ndk/"* \
        "$HOME/Library/Android/sdk/ndk/"* \
        "$HOME/Android/Sdk/ndk/"*; do
        if [[ -d "$candidate" && -f "$candidate/build/cmake/android.toolchain.cmake" ]]; then
            ANDROID_NDK_HOME="$candidate"
        fi
    done
fi

if [[ -z "${ANDROID_NDK_HOME:-}" || ! -d "$ANDROID_NDK_HOME" ]]; then
    echo "Error: ANDROID_NDK_HOME is not set and no NDK installation was found." >&2
    exit 1
fi
export ANDROID_NDK_HOME

export CMAKE_TOOLCHAIN_FILE="$ANDROID_NDK_HOME/build/cmake/android.toolchain.cmake"
export ANDROID_ABI="arm64-v8a"
export ANDROID_PLATFORM="android-26"
export OPUS_STATIC="1"

JNI_DIR="$ROOT_DIR/android/app/src/main/jniLibs"
ARM64_DIR="$JNI_DIR/arm64-v8a"
mkdir -p "$ARM64_DIR"

echo "==> Building libsquad_core.so for arm64-v8a (API 26)..."
cargo ndk -t arm64-v8a -p 26 -o "$JNI_DIR" \
    build --release --locked -p squad-core --no-default-features --features android-ffi

LIBCXX_SRC=""
for host_tag in linux-x86_64 darwin-arm64 darwin-x86_64; do
    candidate="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host_tag/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
    if [[ -f "$candidate" ]]; then
        LIBCXX_SRC="$candidate"
        break
    fi
done

if [[ -z "$LIBCXX_SRC" ]]; then
    echo "Error: Could not locate libc++_shared.so in $ANDROID_NDK_HOME" >&2
    exit 1
fi

cp -f "$LIBCXX_SRC" "$ARM64_DIR/libc++_shared.so"

SO_PATH="$ROOT_DIR/target/aarch64-linux-android/release/libsquad_core.so"
echo "==> Generating Kotlin UniFFI bindings..."
cargo run --locked -p squad-core --no-default-features --features android-ffi --bin uniffi-bindgen -- \
    generate --library "$SO_PATH" --language kotlin --out-dir "$ROOT_DIR/android/app/src/main/java"

echo "==> Assembling release APK..."
pushd "$ROOT_DIR/android" >/dev/null
if [[ -x "./gradlew" ]]; then
    ./gradlew assembleRelease --no-daemon
elif command -v gradle >/dev/null 2>&1; then
    gradle assembleRelease --no-daemon
else
    echo "Error: Neither android/gradlew nor gradle found on PATH." >&2
    exit 1
fi
popd >/dev/null

APK_SRC="$ROOT_DIR/android/app/build/outputs/apk/release/app-release.apk"
if [[ ! -f "$APK_SRC" ]]; then
    echo "Error: Expected release APK not found at $APK_SRC" >&2
    exit 1
fi

mkdir -p "$ROOT_DIR/dist"
cp -f "$APK_SRC" "$ROOT_DIR/dist/SquadAir-android-arm64.apk"
echo "==> Built signed APK: $ROOT_DIR/dist/SquadAir-android-arm64.apk"
