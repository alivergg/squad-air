#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [[ -d "$HOME/.cargo/bin" ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
if [[ -d "/opt/homebrew/bin" ]]; then
    export PATH="/opt/homebrew/bin:$PATH"
fi

if [[ -z "${JAVA_HOME:-}" ]]; then
    for jdk_candidate in \
        "/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home" \
        "/usr/local/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home"; do
        if [[ -d "$jdk_candidate" ]]; then
            export JAVA_HOME="$jdk_candidate"
            export PATH="$JAVA_HOME/bin:$PATH"
            break
        fi
    done
fi

if [[ -z "${ANDROID_HOME:-}" ]]; then
    for sdk_candidate in "$HOME/Library/Android/sdk" "$HOME/Android/Sdk"; do
        if [[ -d "$sdk_candidate" ]]; then
            export ANDROID_HOME="$sdk_candidate"
            break
        fi
    done
fi
if [[ -n "${ANDROID_HOME:-}" && -z "${ANDROID_SDK_ROOT:-}" ]]; then
    export ANDROID_SDK_ROOT="$ANDROID_HOME"
fi

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

mkdir -p "$ROOT_DIR/target"
TOOLCHAIN_WRAPPER="$ROOT_DIR/target/android-arm64-toolchain.cmake"
cat > "$TOOLCHAIN_WRAPPER" <<EOF
set(ANDROID_ABI "arm64-v8a" CACHE STRING "")
set(ANDROID_PLATFORM "android-26" CACHE STRING "")
set(ANDROID_STL "c++_shared" CACHE STRING "")
include("$ANDROID_NDK_HOME/build/cmake/android.toolchain.cmake")
EOF

export CMAKE_TOOLCHAIN_FILE="$TOOLCHAIN_WRAPPER"
export ANDROID_ABI="arm64-v8a"
export ANDROID_PLATFORM="android-26"
export OPUS_STATIC="1"
if command -v ninja >/dev/null 2>&1; then
    export CMAKE_GENERATOR="Ninja"
fi

# Purge stale CMake caches (wrong ABI or mismatched generator).
for cache in \
    "$ROOT_DIR"/target/aarch64-linux-android/*/build/audiopus_sys/*/out/build/CMakeCache.txt \
    "$ROOT_DIR"/target/aarch64-linux-android/*/build/audiopus_sys-*/out/build/CMakeCache.txt
do
    if [ -f "$cache" ]; then
        if grep -q "armeabi-v7a" "$cache" || { [ -n "${CMAKE_GENERATOR:-}" ] && ! grep -q "CMAKE_GENERATOR:INTERNAL=$CMAKE_GENERATOR" "$cache"; }; then
            rm -rf "$(dirname "$cache")"
        fi
    fi
done

JNI_DIR="$ROOT_DIR/android/app/src/main/jniLibs"
ARM64_DIR="$JNI_DIR/arm64-v8a"
mkdir -p "$ARM64_DIR"

echo "==> Building libsquad_core.so for arm64-v8a (API 26)..."
export CARGO_PROFILE_RELEASE_STRIP="debuginfo"
cargo ndk -t arm64-v8a --platform 26 -o "$JNI_DIR" \
    build --release --locked -p squad-core --no-default-features --features android-ffi

HOST_TAG=""
LIBCXX_SRC=""
for host_tag in linux-x86_64 darwin-arm64 darwin-x86_64; do
    candidate="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host_tag/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
    if [[ -f "$candidate" ]]; then
        HOST_TAG="$host_tag"
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
env -u CMAKE_TOOLCHAIN_FILE -u ANDROID_ABI -u ANDROID_PLATFORM -u OPUS_STATIC -u CMAKE_GENERATOR \
    cargo run --locked -p squad-core --no-default-features --features android-ffi --bin uniffi-bindgen -- \
    generate --library "$SO_PATH" --language kotlin --out-dir "$ROOT_DIR/android/app/src/main/java"

LLVM_STRIP="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$HOST_TAG/bin/llvm-strip"
if [[ -x "$LLVM_STRIP" ]]; then
    "$LLVM_STRIP" --strip-unneeded "$ARM64_DIR/libsquad_core.so"
fi

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
