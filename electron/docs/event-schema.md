# Transfer Event Schema

`p2p-share` desktop integrations consume newline-delimited JSON events from the CLI (`--json` mode).

## Versioning

- Current schema version: `1.0.0`
- Compatibility rule: same major version is compatible.
- During the transition period, producers may omit `schema_version`; consumers should treat that as `unknown` compatibility.

## Base Shape

All events are JSON objects with:

- `kind` (string, required)
- `message` (string, optional)
- `value` (string, optional)
- `schema_version` (string, optional for now, recommended)

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

8. `error`
- `message`: human-readable error.
- `value`: error code.

9. `process_end`
- `message`: process code/signal summary.
- `value`: `completed` | `canceled` | implementation-defined.
