# 14 — Real-device release validation

Dependencies: 13.

## Goal

Establish what the release actually supports and whether it meets the original laptop/video plus gaming-PC use case.

## Context and files

Read setup documentation, `specs/contracts.md`, and audio spike/drift findings.
Own `specs/release-validation.md` and small root-cause fixes directly required by failed checks. Escalate larger fixes to their subsystem task with a reproducible case.

## Work

- Record executable/build version, Windows versions, architectures, audio endpoints, driver versions, network type, and settings for every run.
- Test both directions on wired LAN and the actual home Wi-Fi while gaming/video playback applies realistic load.
- Verify fresh configuration, persisted configuration, missing driver/device, invalid peer, port occupied, peer disappearance/restart, and repeated role changes.
- Exercise sleep/resume, device unplug/replug, and network interruption. Accept a clear recoverable stopped/failed state where seamless recovery is not specified; never accept silent dead workers or growing backlog.
- Measure added end-to-end audio latency using a documented repeatable method with a local reference and remote recording; report the measurement uncertainty. Do not substitute configured buffer duration for measured latency.
- Check browser/player lip-sync and run the one-hour stability scenario on the packaged executable.
- Verify accessibility basics, loopback-only HTTP, firewall instructions, and operation without Node.js or Voicemeeter.
- Record results as pass/fail/blocked with reproduction steps, known limitations, and unresolved user decisions.

## Acceptance

- Every matrix entry has a result and evidence; unavailable hardware is marked blocked.
- Provisional wired target is under 100 ms added delay. If missed, report measured values and the limiting stage; do not silently change the target.
- No unresolved crash, data-loss, unbounded-buffer, or duplicate-worker defect remains in validated scenarios.
- Final report distinguishes tested Windows support from intended future portability and identifies any release blockers.

## Handoff

Summarize release readiness, measured latency/stability, remaining blockers, and whether the actual viewing/gaming use case is satisfactory.
