# p2p-share

Encrypted peer-to-peer file transfer with a shared Rust core and two frontends.

Version 0.4 uses iroh 1.x. Update both devices before transferring: tickets from
0.3.x are incompatible. A ticket belongs to one transfer and expires when that
transfer finishes or is canceled.

See the [performance audit](docs/performance-audit.md) for measured results,
implemented fixes, and remaining bottlenecks.

## Projects

- `crates/core`: transfer engine and protocol logic
- `crates/cli`: command-line app
- `crates/android-bindings`: Rust bridge for Android
- `android/`: Android app (Jetpack Compose)
- `electron/`: Electron desktop GUI (Windows/Linux)

## Quick start

```bash
cargo run -p p2p-share -- send ./file.txt
cargo run -p p2p-share -- send ./file.txt ./photo.jpg ./notes.pdf
cargo run -p p2p-share -- receive p2psh:... --output .
```

Build Android app:

```bash
cd android
./gradlew assembleDebug
```

Android builds regenerate the Rust libraries into the build directory. Install
`cargo-ndk`, the Android Rust targets, and an Android NDK first; see
[`android/scripts/build-rust-android.sh`](android/scripts/build-rust-android.sh).

Run desktop GUI:

```bash
cd electron
npm install
npm run dev
```

Desktop GUI notes:

- The GUI runs the existing Rust CLI under the hood using structured JSON events (`--json`).
- The GUI keeps one Rust worker and network endpoint alive between transfers.
  Direct connection tickets appear as soon as a LAN address is available; relay
  discovery continues in the background and refreshes the ticket.
- It supports all transfer modes:
  - send and wait (`send <FILE>...`)
  - send to receiver ticket (`send <FILE>... --to <TICKET>`)
  - receive from target (`receive <TARGET> --output <DIR>`)
  - receive listen/QR mode (`receive --qr --output <DIR>`)
- For best performance, build the CLI first:

```bash
cargo build --release -p p2p-share
```

- Packaging scripts are available in `electron/package.json` (`dist:win`, `dist:linux`). Build each target on its native OS for the most reliable results.
- Packaged desktop builds include the CLI binary inside the app (`resources/bin/<os>/<arch>`), so they run standalone.
- Linux users can run the unpacked distribution to avoid AppImage mount and
  decompression overhead. See the [desktop instructions](electron/README.md).
- If you run the GUI from source and auto-detection fails, set `P2P_SHARE_CLI_PATH` to a built binary path before launching Electron.

Run tests:

```bash
cargo test --workspace --all-targets
```

Desktop checks:

```bash
cd electron
npm run typecheck
npm run test:worker
npm run build
```

The Rust integration tests exercise worker reuse, cancellation, normal and
reverse transfers, and verified multi-file bundles. Desktop tests cover worker
reuse, stale events, crashes, and cancellation timeouts.

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
- `latest.yml`, `latest-linux.yml`, and installer blockmaps for desktop updates
- `android-update.json` when the signed Android APK is built

### App updates

Installed Windows apps, Linux AppImages, and Android release builds check GitHub at startup.
An available update appears in a row with an **Update** action. Downloads run in the background;
desktop users then choose **Restart and install**, and Android users choose **Install**.
Installation stays disabled while files are being prepared or transferred. Failed checks and
downloads can be retried without closing the app.

Android asks users to allow installs from P2P Share, then uses the system installer for
confirmation. APK downloads are checked against the release's size and SHA-256 checksum;
Android verifies that the signing key matches the installed app. Keep the same release keystore.
Debug builds and desktop `.zip`/`.tar.gz` distributions keep manual updates.
Users must manually install the first release containing the updater.

Desktop artifact names are set by electron-builder so the generated update metadata stays
consistent. Upload the metadata and blockmaps alongside the installers without renaming them.
The Android manifest is generated from Gradle's APK metadata. Both apps use the public
`siure/p2p-share` releases; forks must change the desktop publish configuration and Android's
`RELEASES_URL` before distributing builds.

### Required GitHub secrets for Android signing

Set these in `Settings -> Secrets and variables -> Actions`:

- `ANDROID_KEYSTORE_BASE64`
- `ANDROID_KEYSTORE_PASSWORD`
- `ANDROID_KEY_ALIAS`
- `ANDROID_KEY_PASSWORD`

### Create a release

1. Set the same version in `Cargo.toml` (`workspace.package`), `electron/package.json`
   (and its lockfile), and `android/app/build.gradle.kts` (`versionName`). Android derives
   `versionCode` as `major * 1,000,000 + minor * 1,000 + patch`; each component must be 0–999.
2. Create and push a tag:

```bash
git tag v0.3.0
git push origin v0.3.0
```
