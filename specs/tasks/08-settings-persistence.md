# 08 — Validated settings and safe persistence

Dependencies: 02.

## Goal

Persist user configuration without accepting invalid settings or destroying existing files on failure.

## Context and files

Read `specs/contracts.md` and frozen Settings/error types.
Own `src/settings.rs` and focused persistence/validation checks.

## Work

- Load/save versioned JSON in the user's application-data directory using an existing platform-path dependency if available.
- Distinguish valid incomplete setup from start-ready configuration. Validate mode, unicast peer IPv4, port, buffer bounds/increments, field types, and input size.
- Leave device existence checks to session Start, using enumeration. Stored endpoint IDs are opaque, not filesystem paths.
- Use an atomic same-directory replacement supported on Windows; failure must preserve the previous file. Do not assume Unix rename-over-existing behavior works identically on Windows.
- Missing file gives defaults; corrupt or unknown-version files produce a recoverable error without overwriting the original. Saving a replacement requires an explicit user action.
- Keep storage out of audio threads and avoid logging credentials/tokens.

## Acceptance

- A focused check covers defaults, valid save/load, invalid IP/range/schema, corrupt input preservation, and failed-write preservation.
- Verify replacement of an existing settings file on Windows as well as on the development host.
- Settings and fixtures serialize identically to the frozen API contract.

## Handoff

Report settings path, recovery/error semantics, and separation between save validation and Start validation for tasks 09 and 11.
