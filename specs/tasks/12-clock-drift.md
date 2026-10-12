# 12 — Clock-drift correction and long-running stability

Dependencies: 11.

## Goal

Prevent independent device clocks from gradually emptying/filling buffers and increasing delay.

## Context and files

Read `specs/contracts.md`, task 11's findings, jitter occupancy APIs, playback resampler/ratio hook, and session timing wiring.
Own a small controller in `src/audio/drift.rs` and its integration into playback. Reuse the existing resampler; do not write a new DSP library.

## Work

- Measure occupancy in frames/audio duration, including relevant queued/in-flight audio so hidden downstream buffering cannot defeat the target.
- Adjust consumption ratio smoothly around the target buffer. Bound correction and keep tunable target/gain/ratio limits with documented units.
- Do not chase individual packet jitter: use a modest filter and reset controller history on session restart or timeline reset.
- Handle prolonged stalls/backlog with bounded resynchronization rather than extreme speed changes or increasing storage.
- Separate nominal device-rate conversion from small network clock correction. Record direction/sign of ratio changes explicitly.
- Add a deterministic simulated sender/playback timeline with faster/slower clocks and injected jitter/loss, advancing simulated time instead of sleeping.
- Perform a real one-hour two-PC soak and record occupancy, underruns, loss, and latency trend.

## Acceptance

- Simulated ±200 ppm clock mismatches over one hour settle within configured occupancy bounds without progressive queue growth; document the tested jitter/loss profile.
- Reversing drift direction corrects in the appropriate direction; restart resets controller state.
- Real audio remains usable for an hour without progressive delay or recurring buffer failures; measured issues are recorded, not hidden by counters.
- `cargo test` runs the deterministic check without Windows hardware or hour-long wall-clock waits.

## Handoff

Record tuning defaults, hardware measurements, and remaining latency limitations for release validation.
