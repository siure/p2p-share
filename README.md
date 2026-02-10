# p2p-share

Encrypted peer-to-peer file transfer with a shared Rust core and two frontends.

## Projects

- `crates/core`: transfer engine and protocol logic
- `crates/cli`: command-line app
- `crates/android-bindings`: Rust bridge for Android
- `android/`: Android app (Jetpack Compose)

## Quick start

```bash
cargo run -p p2p-share -- send ./file.txt
cargo run -p p2p-share -- receive p2psh:... --output .
```

Build Android app:

```bash
cd android
./gradlew assembleDebug
```
