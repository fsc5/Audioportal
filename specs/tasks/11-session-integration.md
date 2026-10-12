# 11 — Session lifecycle and first end-to-end stream

Dependencies: 03, 04, 07, 09, 10.

## Goal

Connect the existing modules into one owned session controllable from the browser.

## Context and files

Read `specs/contracts.md` and dependency handoff summaries first. Inspect public APIs of capture, playback, transport, settings, and web; read internals only when wiring exposes a mismatch.
Own `src/session.rs` and startup integration. Make minimal consumer edits; coordinate any contract changes.

## Work

- Start validates saved settings and device availability, then builds the role's bounded pipeline: capture → UDP sender or UDP receiver/jitter → playback.
- Implement frozen state transitions, aggregate peaks/counters, and propagate asynchronous worker failures.
- A single session owns all workers, buffers, and resources. Duplicate/concurrent Start is a conflict; Stop is idempotent and waits for cancellation/join.
- Partial startup failure tears down already-started workers. No mutex needed for state may be held while joining a worker that might publish state.
- Peer timeout returns the receiver to listening; device loss becomes failed; restarting clears stale audio/session identifiers.
- Bind the real session operations into HTTP and remove development success stubs. Unsupported platforms fail explicitly, while pure tests still build.
- Verify the actual UI against the API and resolve fixture/serialization drift at the contract boundary.
- Do not tune drift controllers or build the release package in this task.

## Acceptance

- One focused lifecycle check covers duplicate Start, partial startup failure cleanup, repeated Stop, worker failure, and restart using narrowly scoped test doubles.
- On two Windows PCs, use the UI to send real app audio in either direction while local playback continues.
- Repeated start/stop/role changes do not duplicate workers or replay old audio.
- Report peer restart, wrong/missing device, and port conflict results. Hardware checks are required for completion.

## Handoff

Record reproducible first-stream steps and current latency/buffer observations for task 12. Flag any callbacks or queues whose timing still needs correction.
