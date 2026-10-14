# Audioportal

Audioportal is a Windows-first application for sending audio between computers on a trusted local network.

## Development

Install the stable Rust toolchain with [rustup](https://rustup.rs/). Linux is suitable for developing and checking platform-independent code:

```sh
cargo fmt --check
cargo check
cargo run
```

These checks do not validate Windows audio devices or replace the Windows hardware tests.

## Windows build and run

Install stable Rust using rustup with the `x86_64-pc-windows-msvc` target, and install the Visual Studio C++ Build Tools with the MSVC toolchain and Windows SDK. From a Developer PowerShell in the repository root, run:

```powershell
cargo build
cargo run
```

The executable serves the control API on `127.0.0.1:46001`. For the browser UI,
run `cargo run -- --dev-proxy`, then in a second terminal run `npm ci` and
`npm run dev` from `ui/`. Open `http://127.0.0.1:5173`. Stop sessions in the UI;
Ctrl+C also cancels and joins workers before the process exits. Static asset
embedding belongs to Task 13. See `specs/session-results.md` for two-PC setup
and current verification status.

To list active Windows audio endpoints and exercise a ten-second capture/playback path, use a Developer PowerShell from the repository root:

```powershell
cargo run --example audio_probe -- list
cargo run --example audio_probe -- run '<CABLE Output capture ID>' '<headphones playback ID>'
```

Use the full IDs printed by `list`, including braces. Route a browser/player to **CABLE Input** in Windows audio settings; select its paired **CABLE Output** as the capture ID and headphones/speakers as playback. Start local audio on the selected headphones independently to check shared-mode mixing. Never select CABLE Input as the probe's playback device: that would feed the cable back into itself. The `peak` line reports captured sample activity; `0.000` means no audible signal was measured. The probe exits after ten seconds. See `specs/audio-spike-results.md` for observed results and remaining manual checks.
