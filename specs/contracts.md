# Shared implementation contracts

Status: proposed baseline. Task 02 freezes these after the Windows spike. Later changes must update this document and affected consumers together.

Read this document with your assigned task; do not read every other task spec.

## Repository boundaries

One Rust package at the root, with `src/`; one Vite project in `ui/`.

- `src/audio/`: platform-independent audio values plus Windows device I/O.
- `src/protocol.rs`: packet encoding/validation only.
- `src/jitter.rs`: deterministic packet ordering/playout only.
- `src/transport.rs`: UDP workers, peer filtering, receive timeouts.
- `src/session.rs`: session ownership, lifecycle, state snapshots.
- `src/settings.rs`: settings validation and persistence.
- `src/web.rs`: local API and browser boundary checks.
- `ui/src/`: Svelte UI and its typed API client.

These are ownership boundaries, not a requirement for an elaborate framework. Introduce smaller audio files when capture/playback need separate owners. Target-specific dependencies and `cfg` keep non-Windows builds useful for pure logic tests; unsupported audio returns an explicit error.

## Audio boundary

- Wire audio: 48,000 frames/s, two channels, interleaved signed 16-bit samples.
- One packet contains 240 frames, 480 samples, 960 payload bytes.
- Internal device processing may use float samples; conversion is explicit at the wire boundary.
- Public device records contain opaque stable `id`, `name`, and `kind` (`capture` or `playback`); never leak COM types.
- Capture emits bounded blocks; playback consumes bounded blocks without waiting on network or HTTP work. Task 02 fixes exact Rust types, queue limits, cancellation, and worker-error reporting based on the spike.
- Device-native rate conversion and sender/receiver clock correction are distinct responsibilities. Playback must expose an adjustable consumption ratio for task 12.

## Wire protocol v1

Proposed UDP audio datagram: exactly 988 bytes. No negotiation or compression.

| Offset | Bytes | Field |
| --- | --- | --- |
| 0 | 4 | ASCII `APRT` |
| 4 | 1 | Version `1` |
| 5 | 1 | Reserved, must be zero |
| 6 | 2 | Header length `28` |
| 8 | 8 | Random session ID |
| 16 | 4 | Wrapping packet sequence |
| 20 | 8 | Sample-frame index of first payload frame |
| 28 | 960 | Interleaved PCM payload |

Header integers use network byte order; PCM uses little-endian. Frame index starts at zero and advances by 240, including packets discarded locally; sequence advances per packet interval. Validate length, header, and timeline consistency before buffering. Define wrapping-sequence comparison explicitly and test wraparound.

UDP default port: `46000`. Sender uses an ephemeral source port; receiver binds the configured destination port. Both configure the peer IPv4 address for v1. Receiver learns the first valid session from that IP. After one second without accepted audio, it clears buffered/session data and returns to listening; a new session may then establish a fresh timeline. A different session does not replace an active one. Peer IP filtering is not authentication.

Receive buffer: default 30 ms, configurable 10–100 ms in 5 ms increments. Establish the timeline from the first accepted packet and prefill the target before playout. Output silence on missing deadlines, discard late/duplicate data, and cap storage independently of timestamp gaps. Task 02 specifies the hard cap and sender queue overflow policy. Queue overflow must skip old audio rather than accumulating latency.

## Settings and HTTP API

Settings JSON:

```json
{"version":1,"mode":"send","capture_device_id":null,"playback_device_id":null,"peer_ip":"","udp_port":46000,"buffer_ms":30}
```

Incomplete settings may be saved for setup; Start requires a valid unicast IPv4 peer and an available selected device for the role. Reject unknown schema versions, wrong types, invalid ranges, and oversized input. Do not silently migrate unsupported versions or erase a corrupt settings file.

- `GET /api/devices`: `{ "devices": [Device] }`.
- `GET /api/state`: `{ "status": "stopped", "mode": null, "input_peak": 0, "output_peak": 0, "packets_lost": 0, "buffer_ms": 0, "error": null }`.
- `GET /api/settings`: saved Settings.
- `PUT /api/settings`: complete Settings; returns validated saved Settings. Reject changes while a session is active.
- `POST /api/start`: no settings payload; uses saved Settings; returns State.
- `POST /api/stop`: idempotent; returns State after workers stop.
- `GET /api/bootstrap`: `{ "csrf_token": "per-launch-random-value" }`, for same-origin UI initialization.

State statuses: `stopped`, `starting`, `listening`, `receiving`, `sending`, `stopping`, `failed`. Peaks are finite values 0–1; unavailable measurements are zero. `sending` means local emission, not confirmed remote reception. Task 02 fixes counter lifetime, state transitions, timeouts, and structured error codes.

Errors: `{ "error": { "code": "invalid_settings", "message": "human-readable explanation" } }`; use 400 for invalid input, 409 for lifecycle conflicts, 503 for unavailable devices/backends, 500 for unexpected failures. Device errors must not silently select a different endpoint.

HTTP default: `127.0.0.1:46001`; no LAN listener. Validate Host on all requests; validate Origin where supplied, require expected Origin for browser mutations, and require `X-Audioportal-Token` on mutations. Bootstrap responses are not cacheable. No wildcard CORS. Task 09 specifies the development proxy allowlist without loosening release rules.

## Build and verification

- Rust: `cargo fmt --check`, `cargo check`, and focused `cargo test` checks.
- UI: `npm ci`, `npm run check`, `npm run build`, using the committed lockfile.
- Vite dev server proxies `/api` to Rust. Release embeds `ui/dist/` assets; Node is build-time only.
- Windows audio checks require real Windows hardware. Linux pure-logic checks or cross-compilation do not count as hardware validation.
- Leave a small runnable check for packet parsing, buffering, lifecycle, drift, persistence, and web security behavior. Prefer focused tests over duplicated per-function suites.
