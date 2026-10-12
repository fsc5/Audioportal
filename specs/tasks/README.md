# Agent task index

These specifications divide the MVP into bounded work units, usually one subsystem and one focused acceptance check per assignment. They describe future implementation; no application code exists yet. Task numbers are identifiers, not a strictly sequential schedule.

## Assigning a task

Give an agent its task file and `specs/contracts.md`. Read the specific existing source files named by the task before editing; consult `specs/implementation-plan.md` for product context if needed. Do not load all task specs into one context.

An assignment should end at that task's acceptance criteria. Do not implement downstream tasks or create speculative abstractions. If a required contract is missing, resolve it with the integration owner before guessing. Dependencies must be implemented and checked, not merely assigned.

Every handoff must report changed files, public interfaces, commands run/results, hardware checks actually performed, and blockers. Record durable hardware findings in the named report. Never label Windows verification passed based only on Linux tests. Keep a blocked task incomplete.

## Tasks and dependency graph

| ID | Spec | Dependencies | Main ownership |
| --- | --- | --- | --- |
| 00 | [Rust bootstrap](00-rust-bootstrap.md) | None | Root Rust package and build |
| 01 | [Windows audio spike](01-windows-audio-spike.md) | 00 | Audio probe and findings |
| 02 | [Freeze contracts](02-freeze-contracts.md) | 01 | Shared types and interface decisions |
| 03 | [Capture backend](03-capture-backend.md) | 02 | Windows capture |
| 04 | [Playback backend](04-playback-backend.md) | 02 | Windows playback |
| 05 | [Packet codec](05-packet-codec.md) | 02 | Wire encoding/validation |
| 06 | [Jitter buffer](06-jitter-buffer.md) | 05 | Ordering and bounded playout |
| 07 | [UDP workers](07-udp-workers.md) | 06 | Sender/receiver transport |
| 08 | [Settings persistence](08-settings-persistence.md) | 02 | Validation and settings file |
| 09 | [Local HTTP API](09-local-http-api.md) | 08 | HTTP and browser boundary |
| 10 | [Svelte UI](10-svelte-ui.md) | 02 | Vite project, UI, typed client |
| 11 | [Session integration](11-session-integration.md) | 03, 04, 07, 09, 10 | Lifecycle and first complete stream |
| 12 | [Clock-drift correction](12-clock-drift.md) | 11 | Playback timing and stability |
| 13 | [Embedded release and setup](13-release-and-setup.md) | 12 | Asset embedding and Windows release |
| 14 | [Real-device validation](14-real-device-validation.md) | 13 | Hardware matrix and release report |

## Parallel lanes

```text
00 → 01 → 02 ┬→ 03 capture ─────────────────┐
             ├→ 04 playback ────────────────┤
             ├→ 05 → 06 → 07 transport ─────┤
             ├→ 08 → 09 local API ──────────┼→ 11 → 12 → 13 → 14
             └→ 10 UI ─────────────────────┘
```

Task 09 uses the bounded development session stub defined in 02; task 10 uses API fixtures matching the same contract. Task 11 removes/replaces those development stubs with real session operations. No production mock audio path.

For two workers, combine capture/playback/network in one lane and settings/API/UI in the other. For more workers, use the separate lanes above. One owner handles integration and contract changes.

Parallel agents must not overwrite shared `Cargo.toml`, lockfiles, `src/main.rs`, or `src/audio/mod.rs`. Coordinate minimal dependency/module-registration changes through the integration owner, or merge those edits serially. Capture and playback live in separate files under the Windows module; reuse enumeration from the spike.

## Gates

- **Feasibility:** 01 is checked on Windows and 02 records usable interfaces.
- **First usable stream:** 11 passes both directions on the two actual machines.
- **Reliable MVP:** 12 passes deterministic drift checks and a one-hour hardware soak.
- **Release candidate:** 13 packages a standalone executable; 14 records measured latency and the full hardware matrix.

Latency under 100 ms on wired LAN remains a provisional target, not an approved product guarantee. Windows versions and CPU architectures must be recorded from the real test machines before promising support.
