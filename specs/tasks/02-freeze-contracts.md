# 02 — Freeze cross-task contracts

Dependencies: 01, including the Windows feasibility check.

## Goal

Leave concrete interfaces that let audio, network, API, and UI agents work without inventing incompatible behavior.

## Context and files

Read `specs/contracts.md`, `specs/audio-spike-results.md`, and the reusable probe code.
Own updates to `specs/contracts.md`, small Rust shared types/module exports, and API example fixtures under `specs/fixtures/`.

## Work

- Finalize exact settings, device, state, and error types, field nullability, HTTP status mapping, state transitions, and counter reset behavior.
- Finalize packet layout, session/timeline rules, sequence wrap handling, fixed queue caps, overflow policies, and playout interfaces. Keep the baseline unless the spike provides a reason to change it.
- Define concrete capture/playback worker handles, bounded sample-block channels, cancellation, joining, and error reporting. Separate worker startup failure from asynchronous failure.
- Define playback's adjustable consumption-ratio hook, with task 04 implementing it and task 12 controlling it.
- Define the small session operations used by HTTP: snapshot/start/stop. Document how task 09 can use a development-only stub until task 11 wires real workers; avoid a plugin/trait hierarchy just for the mock.
- Define required device validation at Start versus incomplete settings allowed during setup.
- Supply representative JSON fixtures for devices, settings, states, and errors that the UI can consume.

## Acceptance

- Shared Rust types compile; representative JSON round-trips with the documented field names.
- Contracts contain no unresolved type, queue, state-transition, or wire-layout decisions needed by tasks 03–10.
- Any product decisions still requiring the user are explicitly listed, rather than silently finalized.
- The fixtures and document agree, and the contract status becomes frozen for parallel implementation.

## Handoff

List public signatures and fixture paths. Coordinate later contract changes with all affected owners.
