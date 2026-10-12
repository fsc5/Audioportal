# 04 — Windows playback worker

Dependencies: 02.

## Goal

Play bounded incoming audio through a selected Windows output alongside local apps.

## Context and files

Read `specs/contracts.md`, the audio spike results, shared audio types, and enumeration code.
Own `src/audio/windows/playback.rs` and its device-rate conversion code; coordinate shared registrations.

## Work

- Open the selected endpoint in WASAPI shared mode, with event-driven operation verified by the spike.
- Consume wire-rate stereo blocks and convert to the negotiated device format/rate.
- Fill missing audio with silence without blocking; retain any partial block across device callback sizes.
- Provide the frozen adjustable consumption-ratio hook using an existing resampler if required. Apply ratio changes smoothly; task 12 supplies the controller.
- Keep queues and allocations bounded, expose output peaks/underruns and playback timing to integration, and report device loss.
- Implement cancellation/join, flush stale audio between sessions, and never silently change the selected output.

## Acceptance

- A small runnable check covers partial-block consumption and zero-fill behavior.
- Windows playback of a known stereo signal has correct channel mapping and coexists with local game/player audio.
- Underflow produces silence; stop/restart does not replay stale samples.
- Native-rate conversion and small ratio changes work on the tested endpoint; device loss exits cleanly.

## Handoff

Document playback timing, queue occupancy, ratio-control units/range, and worker lifecycle for tasks 11 and 12.
