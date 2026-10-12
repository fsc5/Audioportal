# 01 — Prove Windows audio I/O

Dependencies: 00.

## Goal

Prove that the intended device path works before other agents depend on it.

## Context and files

Read `specs/contracts.md`, `Cargo.toml`, and the root README.
Own a small `examples/audio_probe.rs`, reusable device enumeration in `src/audio/`, and `specs/audio-spike-results.md`.

## Work

- Use an existing WASAPI crate; inspect its actual supported APIs rather than designing around assumed behavior.
- List capture/playback endpoints with stable IDs, names, formats, and selected device periods.
- On Windows, capture `CABLE Output` while a browser/player sends to `CABLE Input`.
- Play captured or generated audio to selected headphones in shared mode while another app also plays locally. Use bounded buffering; never enable routing that creates a feedback loop.
- Investigate sample-format/rate conversion, event-driven operation, COM thread initialization, stop behavior, and device-disconnect errors.
- Record Windows version, architecture, devices, crate version, commands, observed limitations, and whether adjustable playback resampling is feasible.
- Reuse successful code later; do not build the HTTP server, UI, or network streaming here.

## Acceptance

- A documented command lists endpoints and another exercises capture/playback on Windows.
- Actual app audio is audible and shared playback coexists with local audio.
- Probe exits without hanging and handles unavailable endpoints with an actionable error.
- Report distinguishes observations from assumptions. Without Windows access, leave the hardware gate blocked rather than selecting an unverified final backend.

## Handoff

Recommend the WASAPI crate and minimal worker/format interfaces to task 02. Include any changes needed to the proposed audio contract.
