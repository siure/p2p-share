# p2p-share Electron App

Electron desktop UI for `p2p-share`, now built with TypeScript + `electron-vite`.

## Development

```bash
npm ci
npm run dev
```

## Typecheck and Build

```bash
npm run typecheck
npm run build
```

Build outputs are generated in `out/`:

- `out/main`
- `out/preload`
- `out/renderer`

## Package Desktop App

```bash
npm run dist
```

Platform-specific packages:

```bash
npm run dist:linux
npm run dist:mac
npm run dist:win
```

## Verify Bundled CLI in Packaged Output

```bash
npm run test:packaged-binary
```

This checks that packaged artifacts include `resources/bin/<os>/<arch>/p2p-share(\\.exe)`.

## Event Schema Contract

The desktop app consumes CLI JSON events. Schema details and compatibility rules are documented in:

- `docs/event-schema.md`

At runtime, the Electron renderer queries `app:get-build-info` and warns if CLI schema compatibility is unknown or mismatched.

## Notes

- Build the Rust CLI first for best performance:

```bash
npm run build:cli
```

- If running from source and CLI auto-detection fails, set:

```bash
P2P_SHARE_CLI_PATH=/absolute/path/to/p2p-share
```

## Releases

Desktop artifacts are produced by `electron-builder`:

- Linux: AppImage (`npm run dist:linux`)
- Windows: NSIS installer (`npm run dist:win`)
- macOS: DMG (`npm run dist:mac`)

Repository release automation lives in the root workflow:

- `../.github/workflows/release.yml`
