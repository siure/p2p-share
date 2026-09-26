# Transfer Event Schema

`p2p-share` desktop integrations consume newline-delimited JSON events from the CLI (`--json` mode).

## Versioning

- Current schema version: `1.1.0`
- Compatibility rule: same major version is compatible.
- During the transition period, producers may omit `schema_version`; consumers should treat that as `unknown` compatibility.

## Base Shape

All events are JSON objects with:

- `kind` (string, required)
- `message` (string, optional)
- `value` (string, optional)
- `schema_version` (string, optional for legacy engines)
- `transfer_id` (string, required for worker transfer events)

## Event Kinds

1. `status`
- Used for lifecycle and informational messages.

2. `ticket`
- `value`: generated ticket string.

3. `qr_payload`
- `value`: string to encode into QR.

4. `handshake_code`
- `value`: short code shown on both peers.

5. `progress`
- `done`: number of bytes transferred.
- `total`: number of bytes total.

6. `connection_path`
- `value`: `direct` | `relay` | `mixed` | `none`
- `message`: path details.
- `latency_ms`: number (optional).

7. `completed`
- `file_name`: resulting file name.
- `size_bytes`: resulting file size.
- `saved_path`: saved destination path (preferred).
- `saved_to`: legacy destination path key (compatibility field).
- `content_kind`: `file` | `bundle` (optional).
- `item_count`: number of logical files transferred (optional).

8. `error`
- `message`: human-readable error.
- `value`: error code.

9. `process_end`
- `message`: optional process code/signal summary for the legacy CLI path.
- `value`: `completed` | `canceled` | `failed`.
- In worker mode, this event acknowledges that the transfer task has finished cleanup; the worker stays alive.

## Persistent Worker

`version --json` advertises `capabilities: ["worker"]` when supported. Start the engine with `--json worker`. It emits `worker_ready` after binding its endpoint; commands may be queued on stdin before that event. No transfer ID is attached to `worker_ready`.

Commands are newline-delimited JSON:

```json
{"id":"transfer-1","command":"send","files":["/path/file"]}
{"id":"transfer-2","command":"send","files":["/path/file"],"to":"ticket"}
{"id":"transfer-3","command":"receive","target":"ticket","output":"/downloads"}
{"id":"transfer-4","command":"receive","output":"/downloads"}
{"id":"transfer-4","command":"cancel"}
{"command":"shutdown"}
```

Only one transfer is active at a time. Receive without a target listens for an incoming sender. Transfer events echo the command's `id` in `transfer_id`. Ignore events belonging to old IDs. `cancel` waits for the active task to stop before emitting `process_end`; `shutdown` cancels any active transfer, closes the endpoint, and exits. Closing stdin also shuts down the worker.
