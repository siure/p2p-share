# p2p-share Electron App

The desktop UI uses a persistent Rust transfer worker. It starts after the window loads and keeps its network endpoint available between transfers. Older CLI overrides without worker support use the original one-process-per-transfer path.

## Development

```bash
npm ci
npm run build:cli
npm run dev
```

Rebuild the CLI after changing Rust code. To select a specific engine, set `P2P_SHARE_CLI_PATH=/absolute/path/to/p2p-share`. The UI displays the selected CLI version and source.

## Checks

```bash
npm run typecheck
npm run test:worker
npm run build
```

The worker tests cover reuse, cancellation, stale events, crashes, and restart. Build output is written to `out/main`, `out/preload`, and `out/renderer`.

## Linux: unpacked application

For faster launch, use the unpacked application or the release `.tar.gz`. They avoid the AppImage's compressed filesystem at runtime.

Build and launch on Linux:

```bash
npm run dist:linux:dir
npm run start:linux:unpacked
```

To keep a local installation, copy the **whole** `dist/linux-unpacked` directory, including `resources`, to a directory of your choice and launch its `p2p-share-electron` executable. For a downloaded `.tar.gz`, extract it once and launch the executable inside the extracted directory. Create a desktop shortcut pointing directly to that executable if desired. No installer or AppImage integration wrapper is required.

## Distribution

Build packages on their target operating system:

```bash
npm run dist:linux  # AppImage and tar.gz
npm run dist:win    # NSIS installer and zip
npm run dist        # unpacked application for the current OS
```

All distribution commands build the release CLI first, build the UI, then verify the bundled CLI's version, event schema, and worker capability. They require Rust and its native build dependencies. `npm run test:packaged-binary` reruns verification independently on the target OS and architecture.

AppImage remains available for portability. The current builder uses gzip for its SquashFS filesystem; `compression: maximum` switches it to xz. Setting general archive compression to `store` does not remove AppImage filesystem compression. We retain the default and offer unpacked/tar.gz output for faster startup rather than increasing AppImage size without a measured benefit.

## Events and lifecycle

The desktop probes `version --json` asynchronously once per app session, then starts `--json worker` when the CLI advertises the `worker` capability. Commands and events are newline-delimited JSON. Each transfer command has an `id`; its events contain `transfer_id`. `process_end` acknowledges cleanup before another transfer is allowed. Cancellation preserves a responsive worker; a stuck worker is killed after five seconds. A crashed worker restarts for the next transfer. Closing the app sends `shutdown` and waits up to five seconds for process exit.

The UI uses system fonts and does not request remote assets. Progress rendering is batched per animation frame. The optional event log keeps the last 200 non-progress events and only renders while open.

See [the event contract](docs/event-schema.md) for transfer fields. Release automation lives in `../.github/workflows/release.yml`.

Installed NSIS apps and AppImages check GitHub Releases on startup using `electron-updater`.
Users choose when to download and when to restart and install; active transfers block installation.
The NSIS installer writes a marker so ZIP distributions keep manual updates. Development builds
and Linux tarballs also skip the updater. `npm run test:updates` covers the updater lifecycle.
See [app updates](../README.md#app-updates) for release metadata and Android behavior.
