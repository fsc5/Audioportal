# 09 — Local HTTP control API

Dependencies: 08.

## Complexity and model guidance

- **Estimated complexity:** High.
- **Suggested model:** Strong; **reasoning effort:** High.
- Endpoint plumbing is routine, but Host/Origin/token enforcement, development proxy behavior, and lifecycle conflicts need careful boundary reasoning. Request-level tests provide the key feedback.

## Goal

Expose the frozen configuration/session API safely on loopback, ready for UI and later real audio integration.

## Context and files

Read `specs/contracts.md`, frozen session signatures/fixtures, settings code, and device enumeration API.
Own `src/web.rs`, HTTP startup wiring, and focused request-boundary checks. Coordinate `main.rs` edits.

## Work

- Serve all specified `/api` routes using Axum/Tokio, with bounded request bodies and the agreed JSON/error mapping.
- Bind to loopback only. Handle the fixed port already being occupied with a clear error rather than silently exposing another listener.
- Enforce Host validation, Origin policy, mutation token checks, and non-cacheable bootstrap responses. Generate a fresh unpredictable token per launch; never put it in URLs or logs.
- Specify exact development proxy origins/hosts separately from release rules. Preserve Origin through Vite and do not enable wildcard CORS.
- Route session methods through the contract's small controller surface. Until task 11, use a development-only stub; it must not ship as a successful production audio session.
- Reject active-session settings changes and map unavailable audio on non-Windows hosts explicitly.
- Do not implement asset embedding, socket streaming, or UI components.

## Acceptance

- Focused HTTP checks cover settings persistence, invalid input, lifecycle conflict, and structured errors.
- Bad Host, foreign/missing mutation Origin, and missing/wrong token are rejected; valid same-origin mutations succeed.
- A same-origin UI can bootstrap then call the API, including through the documented Vite proxy configuration.
- The listener is loopback-only; audio/session work cannot block async HTTP execution.

## Handoff

Document controller wiring and proxy requirements for tasks 10/11; list any development-only stub code to remove.
