# Task 11 session integration handoff

Implementation and automated verification are complete. **Task 11 hardware acceptance remains incomplete:** two-PC UI streaming, listening, shared local playback, physical disconnect, and latency observations need a user/device operator.

## Integration

- `SessionController` validates Start, owns each role's workers, and monitors failures independently of HTTP polling (10 ms monitor interval). Send uses capture → UDP; Receive uses UDP/jitter → playback. Workers retain the existing bounded queues.
- Cancellation reaches every worker before any join. State locks are released before joins. Partial startup, worker error/panic, Stop, and controller drop clean up workers. Application shutdown explicitly stops the session on a blocking thread before waiting up to one second for HTTP draining. A terminal controller flag prevents delayed Start/Save handlers from reopening a session after shutdown.
- Sending is reported after local UDP emission; until then a started sender remains `starting`. Receiver status/loss comes from transport metrics. Unavailable peaks are zero. Stop/failure retain loss and clear peaks; new startup resets measurements.
- The `dev-api` feature gate is removed. `cargo run` starts the real API; `--dev-proxy` opts into the existing Vite Host/Origin allowlist. JSON and HTTP contracts are unchanged. `web::router_with_session` lets the executable retain the controller for explicit shutdown; `web::router` remains available.
- No new dependencies, audio algorithms, drift control, or static-asset packaging were added.

## Checks performed

- Linux after the shutdown fix: `cargo fmt --check`, `cargo check --locked`, `cargo test --locked` (39 tests), `cargo clippy --all-targets --locked`, and `cargo build --locked` passed.
- Native Windows, Rust 1.99.0 `x86_64-pc-windows-msvc`: latest `cargo test --locked` (43 passed, one opt-in hardware test ignored) and `cargo clippy --all-targets --locked` passed. `cargo build --locked` passed during initial integration, before the shutdown follow-up.
- Review follow-up: a new raw-TCP test passes on Linux and Windows with unfinished GET/POST bodies, worker cancellation/join, bounded server return, and rejected delayed Start/Save. The rebuilt Linux executable also exited successfully on SIGINT in 1.02 seconds while an unfinished-body client stayed connected. `.review-findings.md` retains the original issue for independent recheck.
- New session tests cover validation, duplicate Start, partial startup cleanup, asynchronous error/panic without API polling, stop-time join error, peer cleanup, fresh restart, metrics, idle timeout, sender emission, and controller drop. They use small cancellable worker doubles and real localhost UDP workers.
- The initial Windows run exposed a port-test assumption: a specific loopback bind can coexist with a wildcard bind. Both conflict checks now occupy `0.0.0.0`, matching the actual receiver. Production socket behavior is unchanged.
- Native `audio_probe list` enumerated the selected CABLE Output and Realtek endpoints. The opt-in `native_session_api_round_trip` test then passed using real WASAPI and localhost UDP through two temporary-settings HTTP routers: missing/wrong-kind device returns 503 while stopped; occupied receiver port returns 500/failed and cleans up playback; repeated sender restart returns receiver to listening then receiving; role swaps, duplicate Start, active Save rejection, and repeated Stop succeed as expected. This test exercises router requests, not a browser or two computers.
- UI: `npm ci` (zero audit vulnerabilities), `npm run check` (zero errors/warnings), `npm run build`, `npm test` (6 tests), and `npm run test:browser` (1 browser regression) passed. Production output: 51.07 kB JS / 19.25 kB gzip; 1.58 kB CSS / 0.59 kB gzip.
- A temporary Playwright smoke check against the running Linux executable and actual Vite proxy passed bootstrap, Save, polling/refresh, unsupported-device error display, disabled Start without devices, repeated Stop, and clean SIGINT exit. A missing headless-shell executable on the first attempt was resolved by using the installed Chromium path, as the retained browser test does. Successful Windows UI Start/Stop with streaming still needs manual verification.

## Reproduce the native integration check

From Developer PowerShell, enumerate endpoints and supply explicit IDs:

```powershell
cargo run --example audio_probe -- list
$env:AUDIOPORTAL_TEST_CAPTURE_ID = '<capture endpoint ID>'
$env:AUDIOPORTAL_TEST_PLAYBACK_ID = '<headphones or speakers endpoint ID>'
cargo test --locked native_session_api_round_trip -- --ignored --nocapture
```

Observed IDs on this host:

- CABLE Output: `{0.0.1.00000000}.{7c016758-fdb5-46bc-9715-10e2cf1fa78b}`; 48 kHz stereo float mix.
- Realtek speakers: `{0.0.0.00000000}.{9b223774-3716-45cd-b861-eb5adb91dd34}`; 48 kHz stereo float mix.

The test forwards whatever is present on the selected capture endpoint to the selected playback endpoint. It uses temporary settings and does not change the user's saved configuration. It does not generate an application signal or confirm audibility.

From WSL, native checks used PowerShell, process-local PATH including `$env:USERPROFILE\.cargo\bin`, `CARGO_INCREMENTAL=0`, and `CARGO_TARGET_DIR=$env:TEMP\audioportal-session-target`. `LIB` pointed to MSVC 14.51.36231 `lib\x64` and Windows SDK 10.0.26100.0 `um\x64`/`ucrt\x64`. These versions were checked on the host.

## First-stream steps on two Windows PCs (pending)

1. On each PC run `cargo run -- --dev-proxy` from the repository root. In a second terminal run `npm ci` then `npm run dev` from `ui/`. Open `http://127.0.0.1:5173` locally on each PC. Allow the configured UDP port on the trusted private network if Windows Firewall requires it.
2. On the sender, route a browser/player to the virtual cable's playback endpoint (for VB-CABLE, CABLE Input). In the UI select Send and its paired capture endpoint (CABLE Output), enter the receiver's LAN IPv4 address, and use UDP port 46000.
3. On the receiver, select Receive and headphones/speakers, enter the sender's LAN IPv4 address, and use the same UDP port and the default 30 ms receive buffer.
4. Save & Start the receiver, then the sender. Confirm listening → receiving, input/output activity, audible app audio, and continued independent local music/game playback on the receiver. Record packet loss, buffer setting, endpoint formats, Windows versions, and audible/latency observations.
5. Stop and restart the sender. After one second without accepted audio, the receiver should return to listening; restarted audio should resume with a new timeline. Repeat Stop/Start and confirm silence rather than old audio replay before fresh input.
6. Stop both, swap roles and device selections, and repeat. The new sender needs a compatible app-routing capture endpoint.
7. Disconnect the selected device while streaming: expect failed, stopped peers, and an actionable error without a default-device substitution. Stop, reconnect/refresh, and start again. Check an unavailable/wrong-kind saved endpoint and an occupied receiver UDP port; distinguish pre-start rejection (stopped) from worker startup failure (failed).

## Timing handoff to Task 12

The native integration check used the default 30 ms target. No end-to-end latency or lip-sync measurement was made; under 100 ms remains a provisional target, not a measured result. Receive jitter is monotonic-clock paced, with a four-block/20 ms playback queue; capture holds at most twenty blocks/100 ms. Converter partials, filter delay, and WASAPI padding add separate latency contributions (see `playback-results.md`). Ratio remains 1.0; transport resets it on idle timeout. Two independent device clocks and sustained occupancy/loss behavior require Task 12's measurement and correction work. No callback/queue timing changes were made in this task.
