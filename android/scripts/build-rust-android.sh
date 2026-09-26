#!/usr/bin/env bash
set -euo pipefail

# Requires cargo-ndk:
#   cargo install cargo-ndk
# And Android NDK configured in ANDROID_NDK_HOME.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CRATE_DIR="$ROOT_DIR/crates/android-bindings"
JNI_DIR="${1:-$ROOT_DIR/android/app/build/generated/rustJniLibs}"

mkdir -p "$JNI_DIR"

pushd "$CRATE_DIR" >/dev/null
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o "$JNI_DIR" build --release --locked
popd >/dev/null

echo "Native libraries copied to $JNI_DIR"
