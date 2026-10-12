# 13 — Embedded UI release and Windows setup

Dependencies: 12, which includes completion of the UI/API/session tasks.

## Goal

Produce a standalone Windows executable serving the built Svelte UI, with reproducible setup instructions.

## Context and files

Read `specs/contracts.md`, UI build instructions, HTTP security rules, and actual Windows test platform records.
Own embedded-asset serving, a minimal release build script, root setup documentation, and release build configuration.

## Work

- Build UI assets before the Rust release; embed `ui/dist/` using an existing crate such as `rust-embed` or a comparably small existing solution.
- Serve index/assets with correct MIME types, safe path handling, and sensible cache rules. Do not add SPA routing fallback where there is no router; API failures must stay JSON, not index HTML.
- Ensure the package needs neither Node.js nor a separate asset directory at runtime. Missing build output should fail the release build clearly.
- Define one repeatable build command with locked Rust/npm dependencies. Do not build an elaborate installer/update system.
- Start the localhost UI with a documented URL, useful diagnostics, and graceful application shutdown.
- Explain manual VB-CABLE installation/admin/reboot, cable naming, app/default routing, firewall UDP allowance on private networks, both roles, reversing direction, and settings recovery.
- Link to the official driver installer; do not redistribute it without a separate licensing decision. The app itself should not require administrator access for normal operation.
- Record supported/tested Windows versions and architectures based on evidence, and document the trusted-LAN limitation.

## Acceptance

- From a clean build environment, documented commands produce the Windows executable.
- On a Windows machine without Node.js and outside the source tree, the executable serves working static UI and API.
- Asset requests, missing assets, and `/api` failures have correct content types/statuses; browser security checks remain effective.
- Setup instructions are sufficient to reproduce a two-machine session without Voicemeeter.

## Handoff

Provide build command, artifact path, runtime prerequisites, and exact setup steps for task 14. Do not claim clean-machine validation unless performed.
