# 03 — Windows capture worker

Dependencies: 02.

## Complexity and model guidance

- **Estimated complexity:** High.
- **Suggested model:** Strong; **reasoning effort:** High.
- Native threading, sample conversion, discontinuities, bounded queues, and cancellation interact. The validated spike reduces uncertainty, but completion still requires Windows hardware checks.

## Goal

Capture a selected Windows recording endpoint into bounded wire-rate audio blocks.

## Context and files

Read `specs/contracts.md`, the audio spike results, shared audio types, and endpoint enumeration code.
Own `src/audio/windows/capture.rs`; coordinate registration/dependencies with the integration owner.

## Work

- Reuse enumeration and WASAPI setup from the spike; select by stable endpoint ID, not display name.
- Initialize COM on the correct thread and capture in shared/event-driven mode as supported by the validated crate.
- Convert device audio to 48 kHz stereo samples using verified system conversion or an existing resampler. Do not assume every endpoint is already in the wire format.
- Assemble 240-frame blocks despite arbitrary native callback sizes; preserve timeline advancement when an overflowing queue drops old data.
- Handle WASAPI silent buffers, discontinuities, and device loss explicitly; publish input peaks and actionable worker errors.
- Implement cancellation and deterministic join. No network, file I/O, or HTTP work on the audio path.

## Acceptance

- Focused pure checks cover block assembly/conversion edge cases where custom logic exists.
- Windows probe captures real cable audio with correct channel order, amplitude, and sample count.
- Queue capacity stays bounded when downstream consumption stops.
- Start/stop/restart and a missing/disconnected device do not leak workers or hang.

## Handoff

Document input/output block types, timeline semantics, peak/error reporting, and Windows checks for task 11.
