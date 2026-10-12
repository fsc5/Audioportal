# 00 — Rust project bootstrap

Dependencies: none.

## Complexity and model guidance

- **Estimated complexity:** Low.
- **Suggested model:** Lightweight; **reasoning effort:** Low.
- Conventional project setup with a narrow scope. The main check is correct Windows dependency gating, not architectural reasoning.

## Goal

Create the smallest buildable foundation for the audio spike and later application.

## Context and files

Read `specs/contracts.md` and the technology section of `specs/implementation-plan.md`.
Own `Cargo.toml`, `Cargo.lock`, `src/main.rs`, `.gitignore`, and a short root `README.md`.

## Work

- Create one Rust package using stable Rust; commit its dependency lockfile when changes are committed by the project owner.
- Add only dependencies required for the next audio spike. Keep Windows-only dependencies target-gated.
- Provide an executable that starts and exits cleanly; do not scaffold empty subsystems.
- Explain Linux development versus Windows hardware validation and basic build commands.
- Ignore generated Rust/UI build output and local settings. Do not initialize Git or create commits unless requested.

## Acceptance

- `cargo fmt --check` and `cargo check` succeed on the development host.
- The README explains how to build/run on Windows and records any required toolchain prerequisites.
- No Svelte project, streaming engine, or application framework is implemented in this task.

## Handoff

Report package/toolchain choices and the commands checked. Task 01 owns the Windows audio experiment; task 10 creates the UI.
