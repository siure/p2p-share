# Performance audit and fixes

Measured on 26 September 2026 against the installed Linux AppImage and bundled
CLI, both version 0.3.4. The new build is 0.4.0. Update both devices together;
0.3.x tickets are incompatible with the upgraded network transport.

## Measured results

Transfer measurements use synthetic files, two processes on this Linux host,
direct networking, and verified received contents. They measure preparation,
ticket generation, connection, transfer, and receiver completion. They exclude
human ticket entry. These are local measurements, not predictions for Wi-Fi,
mobile data, or a remote receiver.

| Operation | Installed 0.3.4 | New 0.4.0 |
| --- | ---: | ---: |
| First small-file ticket, fresh CLI | 10,012 ms | 10 ms |
| 1 KiB transfer, fresh CLI | 10,023 ms | 23 ms |
| Reverse 1 KiB transfer, fresh CLI | 10,013 ms | 29 ms |
| 32 MiB transfer, fresh CLI | 10,097 ms | 126 ms |
| Bundle of 100 one-KiB files, fresh CLI | 10,019 ms | 29 ms |
| 32 MiB progress events, per peer | 547 | 2 |

The desktop now keeps its worker alive. With two ready workers, three 1 KiB
transfers completed in 2.9–6.9 ms; tickets appeared in 0.25–0.52 ms. The 32 MiB
transfer completed in 93 ms and the 100-file bundle in 6.8 ms. Worker startup
took 11–12 ms. Both workers became available for another transfer within 0.3 ms
of receiver completion in these runs. Standalone CLI exit still includes roughly
one second of graceful endpoint shutdown; the desktop no longer pays that delay
between transfers.

The large improvement comes primarily from removing the fixed relay wait. It
does not establish improved sustained throughput: the old 32 MiB transfer took
about 69 ms after its ticket appeared, compared with 81 ms for the new worker.

Startup measurements use process launch to Chromium first contentful paint,
Xvfb, identical launch flags, and temporary profiles. Each distribution was
launched three times, first with a new profile and then twice with that profile.
This is not a cold-disk benchmark.

| Distribution | First profile | Repeat 1 | Repeat 2 |
| --- | ---: | ---: | ---: |
| Installed 0.3.4 AppImage | 2.35 s | 1.34 s | 1.55 s |
| New 0.4.0 AppImage | 2.07 s | 1.37 s | 2.18 s |
| New 0.4.0 unpacked app | 0.56 s | 1.63 s | 0.55 s |

AppImage startup remains variable; these samples do not demonstrate a reliable
AppImage speedup. The unpacked app had the lowest median, 0.56 seconds. See the
[unpacked Linux instructions](../electron/README.md). The new renderer made no
remote font requests.

## Changes made

| Bottleneck | Change |
| --- | --- |
| Ten-second relay timeout blocked even local tickets | Publish direct tickets immediately and update them as relay discovery completes. |
| Old transport failed public-relay discovery here | Upgrade to iroh 1.2; public relay became available in about 200 ms in a separate listener check. |
| Endpoint and child process recreated for every desktop transfer | Reuse one worker and endpoint; close them on application exit. Android also reuses its engine. |
| Presence of a UI event sink changed routing policy | Remove the sink-dependent relay policy; event reporting no longer controls network configuration. |
| Synchronous version probe and eager QR dependency | Cache an asynchronous probe and load QR rendering when needed. Main-process bundle fell from about 138 KB to 24 KB. |
| External fonts on startup | Use local system fonts. |
| Per-chunk allocation and UI traffic | Reuse crypto buffers, throttle progress to 10 Hz with final updates, batch renderer paints, and bound logs and Android queues. |
| Bundle reread just to compute its hash | Hash the archive while writing it. |
| Cancellation could retain work or partial output | Add cooperative preparation cancellation, owned staging cleanup, transfer IDs, and per-transfer ticket sessions. |
| Android metadata queries and cache accumulation | Load document labels outside composition, clean owned temporary files, bound polling work, and release JNI controllers off the UI thread. |
| Stale native binaries could enter releases | Rebuild JNI as a Gradle prerequisite; rebuild and verify the desktop CLI during packaging. Enable Android release shrinking. |

Repeated packaged tests also exposed two completion races. The sender now reads
the receiver's final stream end, and the receiver distinguishes confirmed
success from cancellation or failure before emitting completion.

## Validation

- All 30 default Rust tests passed; workspace Clippy with warnings denied,
  formatting, and diff checks passed.
- Five desktop worker lifecycle tests, TypeScript checks, and production
  packaging passed.
- Twenty consecutive packaged transfers, ten normal and ten reverse, verified
  bytes and successful completion on both peers using the same workers.
- Real desktop UI checks passed for drag-and-drop, normal sending, reverse
  bundle receiving, progress completion, and cancellation back to setup.
- Normal and reverse transfers through the public relay passed with IP
  transports disabled. This prevents a direct-path upgrade from disguising a
  relay failure. Run the opt-in check with
  `cargo test -p p2p-share-core public_relay_transfers_without_ip_transports -- --ignored --nocapture`.
- Android release assembly, three native architectures, Kotlin compilation,
  shrinking, and vital lint passed. Packaged native machine code matches the
  generated libraries, and JNI entrypoints survived shrinking.

The Android emulator crashed before boot, so Android runtime behavior remains
unverified. Windows installers and transfers between different physical
networks were not exercised. The Android artifact is unsigned.

## Remaining opportunities

1. **Desktop launch:** distribute or install the unpacked Linux build for the
   lowest measured startup cost. Profile native Wayland/X11 launch separately
   before changing Electron or AppImage compression; the headless measurements
   contain substantial variation.
2. **Large files:** single files are still hashed before sending, and bundles
   still use a temporary archive. A streamed format with a final checksum could
   reduce disk passes and time to first byte, but requires protocol changes.
3. **Android storage:** SAF documents are still copied into app storage before
   sending, and received files are exported afterward. File-descriptor based
   streaming could remove these copies; it needs device/provider testing and
   careful ownership handling.
4. **Network coverage:** measure Wi-Fi, cellular, restrictive NAT, and distant
   relay paths on real devices. The forced-relay check confirms functionality,
   not latency under those conditions.
