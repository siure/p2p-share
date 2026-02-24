# p2p-share

Encrypted peer-to-peer file transfer with a shared Rust core and two frontends.

## Projects

- `crates/core`: transfer engine and protocol logic
- `crates/cli`: command-line app
- `crates/android-bindings`: Rust bridge for Android
- `android/`: Android app (Jetpack Compose)
- `electron/`: Electron desktop GUI (Windows/Linux)

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

Run desktop GUI:

```bash
cd electron
npm install
npm run dev
```

Desktop GUI notes:

- The GUI runs the existing Rust CLI under the hood using structured JSON events (`--json`).
- It supports all transfer modes:
  - send and wait (`send <FILE>`)
  - send to receiver ticket (`send <FILE> --to <TICKET>`)
  - receive from target (`receive <TARGET> --output <DIR>`)
  - receive listen/QR mode (`receive --qr --output <DIR>`)
- For best performance, build the CLI first:

```bash
cargo build --release -p p2p-share
```

- Packaging scripts are available in `electron/package.json` (`dist:win`, `dist:linux`). Build each target on its native OS for the most reliable results.
- Packaged desktop builds include the CLI binary inside the app (`resources/bin/<os>/<arch>`), so they run standalone.
- If you run the GUI from source and auto-detection fails, set `P2P_SHARE_CLI_PATH` to a built binary path before launching Electron.

Run tests:

```bash
cargo test --workspace --all-targets
```

## CI/CD

This repository includes GitHub Actions workflows:

- `.github/workflows/ci.yml`: builds CLI on Linux and Windows, and builds Android debug APK on pushes/PRs.
- `.github/workflows/release.yml`: builds and publishes release artifacts on tags like `v0.3.0`.

### Release artifacts

When you push a tag `vX.Y.Z`, the release workflow publishes:

- `p2p-share-cli-vX.Y.Z-linux-x86_64.tar.gz`
- `p2p-share-cli-vX.Y.Z-windows-x86_64.zip`
- `p2p-share-gui-vX.Y.Z-linux-x86_64.AppImage`
- `p2p-share-gui-vX.Y.Z-linux-x86_64.tar.gz`
- `p2p-share-gui-vX.Y.Z-windows-x86_64.exe`
- `p2p-share-gui-vX.Y.Z-windows-x86_64.zip`
- `p2p-share-android-vX.Y.Z.apk` (when Android signing secrets are configured)
- `p2p-share-checksums-vX.Y.Z.txt`

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
git tag v0.3.0
git push origin v0.3.0
```
