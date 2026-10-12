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

The executable currently starts and exits. Audio backend and device behavior will be investigated separately on Windows with real audio hardware; Linux builds and checks are not evidence of hardware validation.
