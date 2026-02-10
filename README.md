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

## CI/CD

This repository includes GitHub Actions workflows:

- `.github/workflows/ci.yml`: builds CLI on Linux and Windows, and builds Android debug APK on pushes/PRs.
- `.github/workflows/release.yml`: builds and publishes release artifacts on tags like `v0.2.0`.

### Release artifacts

When you push a tag `vX.Y.Z`, the release workflow publishes:

- `p2p-share-X.Y.Z-linux-x86_64.tar.gz`
- `p2p-share-X.Y.Z-windows-x86_64.zip`
- `p2p-share-X.Y.Z-android-release.apk`

### Required GitHub secrets for Android signing

Set these in `Settings -> Secrets and variables -> Actions`:

- `ANDROID_KEYSTORE_BASE64`
- `ANDROID_KEYSTORE_PASSWORD`
- `ANDROID_KEY_ALIAS`
- `ANDROID_KEY_PASSWORD`

### Create a release

1. Ensure `crates/cli/Cargo.toml` version matches the intended tag version.
2. Create and push a tag:

```bash
git tag v0.2.0
git push origin v0.2.0
```
