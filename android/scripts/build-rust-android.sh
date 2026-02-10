#!/usr/bin/env bash
set -euo pipefail

# Requires cargo-ndk:
#   cargo install cargo-ndk
# And Android NDK configured in ANDROID_NDK_HOME.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CRATE_DIR="$ROOT_DIR/crates/android-bindings"
JNI_DIR="$ROOT_DIR/android/app/src/main/jniLibs"

rm -rf "$JNI_DIR/arm64-v8a" "$JNI_DIR/armeabi-v7a" "$JNI_DIR/x86_64"

pushd "$CRATE_DIR" >/dev/null
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o "$JNI_DIR" build --release
popd >/dev/null

echo "Native libraries copied to $JNI_DIR"
