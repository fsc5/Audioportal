# Task 04 playback handoff

## Implementation and integration

- `audio::windows::start_playback(id)` opens only the selected active render endpoint, starts it on its COM-owning worker, and returns the existing worker/queue/peak/ratio handles plus `metrics`. Startup errors join before returning. Later errors go to the bounded notification channel and the thread's return value. Cancel, then join even after a notification; waits are at most 200 ms.
- Each Start creates a four-block queue, fresh converter, and ratio 1.0. Stop drops converter/device buffers, resets peak and pending-buffer metrics, and drains up to four queued blocks. Integration must cancel producers too and discard the old queue; retained external clones are not reusable sessions.
- Rubato 5.0.1 converts 48 kHz stereo to the endpoint's mix rate (supported range 8–192 kHz). WASAPI shared-mode automatic conversion handles device sample representation and channel layout. Conversion runs in chunks of at most 5 ms; a callback can consume part of a converted chunk. Missing input is zero-filled before filtering; the finite filter tail is allowed to drain rather than abruptly truncating valid audio.
- Ratio units/range follow `contracts.md`. Rubato uses the reciprocal ratio and linearly ramps input consumption within each chunk. A chunk changes the applied ratio by at most `0.0001 * chunk_duration / 5 ms`; requesting a change does not retroactively alter already converted/device-buffered audio.
- Memory is allocated at startup. The worker retains one partial wire block, one converted chunk, preallocated resampler buffers, and one WASAPI-sized byte buffer (rejecting buffers over one second). Queue overflow evicts old queued blocks; the current partial block can contribute up to another 5 ms.
- Metrics are independent relaxed atomic observations, not hardware-clock timestamps. Queue depth, partial input, converted output, filter delay, and WASAPI padding are distinct latency contributions. `submitted_frames` includes silent prefill. `padding_frames` is an estimate at refill, becoming stale between callbacks; neither it nor submitted duration measures endpoint presentation latency. `missing_input_frames` includes startup starvation and is independent of jitter/network loss. Counters persist after Stop; pending buffers and peak become zero.

## Dependency

Rubato 5.0.1 is the current published version of the maintained Rust audio resampler, licensed MIT OR Apache-2.0. It supplies anti-aliased conversion and smooth adjustable consumption without custom DSP. FFT/default features are disabled. `audioadapter-buffers` provides borrowed interleaved slice adapters and is already a Rubato dependency. Both are Windows runtime/test-only dependencies here; Linux production and frontend dependencies are unchanged. The lockfile adds seven packages; no browser bundle impact. Windows executable-size impact has not been measured.

## Observed checks

- Linux: `cargo fmt --check`, `cargo check`, `cargo test` (8 tests), and `cargo clippy --all-targets` passed without warnings. Windows GNU cross-target `cargo check --target x86_64-pc-windows-gnu --all-targets` and corresponding Clippy passed without warnings.
- Windows host: Windows NT 10.0.19045.0, Rust/Cargo 1.99.0, native MSVC `cargo test --target-dir target/windows-native` passed all 12 tests. From WSL, Cargo was invoked through `$env:USERPROFILE\.cargo\bin\cargo.exe`; this host needed `CARGO_INCREMENTAL=0` for the UNC checkout and `LIB` pointing to MSVC 14.51.36231 `lib/x64` plus Windows SDK 10.0.28000.0 `ucrt/x64` and `um/x64` under `C:\Program Files\msvc`. Initial missing-SDK-library and UNC incremental-lock errors were resolved with those process-local environment variables.
- The built `audio_probe.exe list` enumerated Realtek at 48 kHz stereo f32 mix / 16-bit device and MOMENTUM 4 Stereo at 44.1 kHz stereo f32 mix / 16-bit device. IDs match `audio-spike-results.md`.
- Built `playback_probe.exe run missing` returned `DeviceUnavailable` with a relist hint and exit 1.
- Built `playback_probe.exe` runtime results (successful exits; `queue_after_join=0` and peak 0.125 for all rows):

| Endpoint / mode | Submitted frames | Missing 48 kHz input frames | Final ratio | Join |
| --- | ---: | ---: | ---: | ---: |
| Realtek / ratio | 481632 | 0 | 0.995000 | 22.05 ms |
| MOMENTUM stereo / ratio | 442411 | 241 | 0.995000 | 8.76 ms |
| Realtek / underflow | 481536 | 95281 | 1.000000 | 22.68 ms |
| Realtek / restart first | 481536 | 241 | 1.000000 | 21.26 ms |
| Realtek / restart second | 481536 | 96481 | 1.000000 | 18.98 ms |

These initial runs establish runtime startup/refill/ratio/cancellation on the endpoints. User-assisted listening and disconnect results follow.

## User-assisted hardware verification

The user selected MOMENTUM 4 Stereo, endpoint `{0.0.0.00000000}.{fbb55114-4858-4f51-b13b-9029c1146fb8}`, at the enumerated 44.1 kHz stereo mix rate. The built Windows `playback_probe.exe` was run on the same host for each check below.

| Mode | Submitted frames | Missing 48 kHz input frames | Final ratio | Join |
| --- | ---: | ---: | ---: | ---: |
| run, with local music | 441970 | 0 | 1.000000 | 4.55 ms |
| underflow | 442411 | 95695 | 1.000000 | 8.46 ms |
| ratio | 442411 | 241 | 0.995000 | 8.13 ms |
| restart first | 441705 | 0 | 1.000000 | 5.54 ms |
| restart second | 441970 | 96023 | 1.000000 | 5.67 ms |
| run, physical disconnect | 238315 | 241 | 1.000000 | 19 µs |

- All runs ended with `queue_after_join=0`. Peaks were 0.125, except underflow at 0.126. The first five rows exited successfully. Join durations measure the caller's join wait, not disconnect detection latency.
- **Stereo, quality, shared mixing:** user confirmed the lower tone was left, higher tone right, tones sounded clean, and local music continued normally throughout.
- **Underflow:** with music paused, user confirmed silence during the input gap around seconds 3–5 and normal playback afterward.
- **Ratio control:** user confirmed clean sound throughout both changes (1.005 at second 3, 0.995 at second 6), without clicks, crackling, or dropouts.
- **Stop/restart:** user confirmed the second session's initial two-second interval was silent, with no old tone replay, followed by normal playback.
- **Physical disconnect:** user disconnected the headphones during playback. The worker reported `WorkerFailed: ... is Unplugged; reconnect it`, joined, and the probe exited 1. User confirmed tones stopped without switching to the speakers.

Task 04's listening and disconnect acceptance checks are complete on this endpoint. Realtek has runtime checks above but no task 04 listening confirmation; other endpoint formats and long-duration validation are not established by these runs.

## Reproducing the Windows checks

From Developer PowerShell at the repository root, substitute the full playback endpoint ID (including braces) printed by `list`:

```powershell
cargo test
cargo run --example audio_probe -- list
cargo run --example playback_probe -- run '<playback-id>'
cargo run --example playback_probe -- underflow '<playback-id>'
cargo run --example playback_probe -- ratio '<playback-id>'
cargo run --example playback_probe -- restart '<playback-id>'
cargo run --example playback_probe -- run missing
```

- `run`: ten seconds of low-amplitude 440 Hz left / 880 Hz right, alternating every two signal seconds. Confirm channel placement and simultaneous local music/game playback. Timed cancellation happens during streaming; record join duration and counters.
- `underflow`: input stops at second 3, resumes at second 5. After bounded queued audio/filter tail drains, the gap must be silent; `missing_input` increases and sound resumes.
- `ratio`: requests 1.005 at second 3, then 0.995 at second 6. Confirm smooth sound without clicks or sustained glitches; final applied ratio should approach 0.995. Repeat on the 44.1 kHz MOMENTUM 4 Stereo endpoint and record audible quality and printed stream rate.
- `restart`: two separate ten-second sessions; the second begins with two seconds without input. Confirm silence during that interval, no old tone replay, and successful fresh playback. `queue_after_join` should be zero.
- While `run` is sounding on a disconnectable endpoint, unplug/disconnect it. Expect an actionable error, nonzero process exit, and joined worker. A missing ID must fail startup without choosing another output.
- Record Windows version, endpoint IDs/formats, probe outputs, audible observations, and disconnect/stop behavior when repeating these checks on another endpoint.
