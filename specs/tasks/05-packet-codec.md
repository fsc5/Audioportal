# 05 — UDP audio packet codec

Dependencies: 02.

## Complexity and model guidance

- **Estimated complexity:** Medium.
- **Suggested model:** Standard; **reasoning effort:** Medium.
- The format is fixed and pure tests provide strong feedback. Care is still needed for byte order, malformed lengths, signed samples, and wrapping sequence comparisons.

## Goal

Implement the frozen packet format independently of sockets and Windows audio.

## Context and files

Read `specs/contracts.md` and shared audio block/timeline types.
Own `src/protocol.rs` and a focused packet test module.

## Work

- Encode/decode the exact fixed-size datagram, explicit header byte order, and little-endian PCM.
- Validate magic, version, reserved bits, header length, payload length, and frozen timeline consistency rules.
- Support sequence wrapping using the documented comparison rule; never compare wrapping sequence values as ordinary ordered integers.
- Parse untrusted bytes without panics, unchecked indexing, or allocations driven by packet contents.
- Keep socket peer/session policy outside the codec. No networking or jitter buffering in this task.

## Acceptance

- `cargo test` runs a known-byte-vector round-trip check, not only encode/decode symmetry.
- Truncated, oversized, wrong-version, invalid-header, and inconsistent-timeline datagrams are rejected.
- Tests cover signed sample extremes and sequence wraparound.
- Pure checks run on Linux as well as Windows.

## Handoff

Document public codec operations and validated packet values consumed by task 06.
