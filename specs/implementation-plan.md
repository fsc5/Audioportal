# Audioportal implementation plan

## Confirmed scope

- Windows first; keep Windows APIs isolated so other OS backends can be added later.
- Either machine can send or receive; one direction per session in v1.
- Existing virtual audio driver is acceptable.
- Configure through a localhost web app; enter the peer IP manually.
- First release targets two machines on a trusted home LAN.
- Rust application with a Svelte + TypeScript + Vite UI, built as static assets. No server-side rendering.

## Proposed user flow

1. Install Audioportal on both machines. Install VB-CABLE on whichever machine will send audio.
2. On the sender, route the desired apps to `CABLE Input` using Windows audio settings. Audioportal captures the paired `CABLE Output` recording endpoint.
3. Select **Send**, the cable capture device, and the receiver's IP.
4. On the receiver, select **Receive**, the sender's IP, and the headphones/speakers to use.
5. Start the receiver, then the sender. Windows shared-mode playback mixes the incoming stream with local game audio.
6. Save settings so the next launch only needs Start.

The driver names are counterintuitive: CABLE Input is a playback device; CABLE Output is a recording device. Explain this in setup rather than exposing users to audio terminology throughout the UI.

Reversing direction means stopping the session and changing roles on both machines. The new sender needs a virtual cable too. No local monitoring, simultaneous send/receive, microphone routing, or automatic Windows default-device changes in v1.

## Agreed technology — audio library subject to the spike

**One Rust executable with a small embedded web UI.**

- Audio: WASAPI shared mode, initially through an existing Rust WASAPI crate. Validate the crate on real Windows hardware before committing to it.
- Networking: UDP sockets and a bounded receive buffer.
- Local configuration server: Axum/Tokio; Svelte + TypeScript + Vite static assets embedded in the executable. Vite proxies `/api` to Rust during development; Node.js is not required at runtime.
- Settings: a versioned JSON file in the user's application-data directory; save atomically.
- No database, SvelteKit, SSR, client-side router, background Windows service, or custom virtual driver.

The native audio path and configuration server run in the same Rust process. The browser UI is a client of the local HTTP API.

### Portability boundary

Keep device enumeration, capture, playback, and format negotiation in `audio/windows.rs`. Keep transport, buffering, configuration, and UI independent of WASAPI handles and Windows endpoint types.

Expose only the operations the first implementation needs: list devices, start capture/playback, stop. Preserve stable endpoint IDs separately from display names. Do not build a plugin system or unused Linux/macOS backends. Define a shared backend interface when a second backend actually needs one.

## Audio and transport contract

Proposed defaults, to validate during the spike:

- Wire format: 48 kHz, stereo, signed 16-bit PCM, explicitly little-endian.
- Packets: 5 ms / 240 frames / 960 bytes of audio plus a small header. This fits comfortably inside a normal LAN MTU without IP fragmentation.
- Header: magic, protocol version, session identifier, sequence number, and sample-frame timestamp. Specify byte order and exact lengths before independent sender/receiver work.
- Bandwidth: roughly 1.54 Mbit/s of audio before network overhead. No codec needed for the initial LAN use case.
- One configurable UDP port; a fixed default documented in setup.
- Initial receive-buffer target: around 30 ms; expose one latency/buffer control if hardware testing shows it is needed.
- Receiver checks peer IP, packet length, version, and session before accepting data. IP filtering is not authentication: this first transport assumes a trusted LAN and is not for Internet exposure.
- Missing packets become silence at their playback deadline; late packets are dropped. Do not retransmit stale audio.
- Bound queues by audio duration, reorder within the receive window, and discard backlog rather than allowing latency to grow without limit.
- Handle device sample formats and rates explicitly. Validate WASAPI shared-mode conversion support; use an existing resampler where needed.
- Sender and receiver clocks will drift. Include occupancy-driven rate correction with a tunable target buffer; a fixed jitter buffer alone is not sufficient for a long-running session.

Audio callbacks/workers must not wait on network I/O, HTTP handlers, file writes, or unbounded allocations. Communicate through bounded queues. A session owns its workers and buffers; Stop must cancel and join them before another session starts.

## UI and local API

One page:

- Send / Receive mode.
- Source device for Send; playback device for Receive.
- Peer IP; port under advanced settings.
- Start / Stop.
- Connection/stream state, input/output activity, and useful errors.
- Optional advanced buffer setting; no routing matrix.

Small proposed API: `GET /api/devices`, `GET /api/state`, `PUT /api/settings`, `POST /api/start`, `POST /api/stop`. Poll state at a modest rate initially; no real-time framework is required for these controls.

Bind HTTP only to loopback. Reject unexpected Host/Origin values and require a per-launch token on mutating requests so arbitrary websites cannot control the local app. Validate IPs, port ranges, device IDs, and buffer limits server-side. Never expose the configuration server on the LAN by default.

Display distinct states such as stopped, listening, receiving, sending, and failed. A sender emitting UDP packets must not claim the remote machine is connected without an acknowledgment. Report missing devices and stalled streams rather than silently choosing a different output.

## Work breakdown

See [the agent task index](tasks/README.md) for executable task specifications, dependencies, parallel lanes, and handoff requirements. [Shared contracts](contracts.md) define the small interfaces agents must agree on before working independently.

The sequence remains: project bootstrap → real Windows audio spike → contract freeze → parallel audio/network/UI work → session integration → drift correction → packaging and real-device validation.

## Acceptance targets and remaining decisions

- Initial latency target: under 100 ms added audio delay on wired LAN, measured end-to-end rather than inferred from buffer size. Confirm whether this is acceptable for watching videos; tighter targets may change buffering and device-period choices.
- Video lip-sync depends on added delay; test the actual browser/player before finalizing the latency target.
- Supported Windows versions and CPU architectures need an explicit decision before packaging. Start by testing the two actual machines.
- Windows validation requires real Windows devices even if development happens on Linux.
- VB-CABLE's official installer requires administrator access and documents a reboot. Manual installation is the initial setup path. Review distribution licensing before bundling or automating its installer.
- Auto-discovery, more peers, encryption/authentication for untrusted networks, codecs, and other OS backends are follow-up features with separate requirements.

## References

- [VB-CABLE official documentation and installer](https://vb-audio.com/Cable/)
- [Microsoft WASAPI loopback recording](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording): useful later for a driver-free system-audio capture option, but it does not create a selectable virtual audio device.
