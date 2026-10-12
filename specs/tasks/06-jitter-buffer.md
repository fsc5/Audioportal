# 06 — Bounded packet ordering and playout

Dependencies: 05.

## Complexity and model guidance

- **Estimated complexity:** High.
- **Suggested model:** Strong; **reasoning effort:** High.
- Deadlines, ordering, wraparound, malicious timestamp gaps, and bounded resynchronization create interacting edge cases. Deterministic simulations are more useful than extra framework code.

## Goal

Turn validated packets into a continuous, bounded audio timeline despite jitter, reordering, and loss.

## Context and files

Read `specs/contracts.md`, `src/protocol.rs`, and shared audio block types.
Own `src/jitter.rs` and deterministic synthetic-timeline checks.

## Work

- Implement insertion and frame/block playout operations with an injected clock/frame position, not wall-clock sleeps inside the buffer.
- Prefill to the target, reorder within the window, drop late/duplicate packets, and emit silence when a deadline passes without audio.
- Cap memory independently of peer-supplied frame indices; huge timestamp gaps must not allocate huge buffers or loop over the gap.
- Define bounded backlog resynchronization per the frozen contract; distinguish network loss from local overflow.
- Expose occupancy and counters for state reporting and later clock control. Session reset clears timeline and buffered data.
- Do not implement adaptive resampling, sockets, or WASAPI here.

## Acceptance

- A single focused synthetic scenario covers reorder, duplicate, loss, late arrival, prefill, reset, and wraparound.
- A malicious far-future timestamp cannot grow storage or force an unbounded operation.
- Output advances at requested playback deadlines and missing data becomes silence.
- Queue/storage caps and overflow behavior are asserted by runnable checks.

## Handoff

Explain playout clock ownership and occupancy semantics to transport/integration owners.
