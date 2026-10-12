# 10 — Svelte + Vite configuration UI

Dependencies: 02.

Task 09 is optional for live API verification; fixtures allow independent implementation.

## Complexity and model guidance

- **Estimated complexity:** Medium.
- **Suggested model:** Standard; **reasoning effort:** Medium.
- A single conventional page with frozen API fixtures. Most complexity is async state: polling cleanup, preserving edits, token refresh, and save-before-start behavior, rather than visual layout.

## Goal

Build the single-page, client-rendered configuration experience against the frozen HTTP contract.

## Context and files

Read `specs/contracts.md`, `specs/fixtures/`, and the user-flow section of the implementation plan.
Own `ui/`, including package/lockfile, Vite config, Svelte components, typed API client, and UI build instructions.

## Work

- Use Svelte + TypeScript + Vite. No SvelteKit, SSR, router, component library, or global state library needed.
- Implement Send/Receive, role-specific device selection, peer IP, advanced port/buffer settings, Save, Start/Stop, activity, stream state, and errors.
- Explain `CABLE Input` versus `CABLE Output`, link to official installation instructions when no cable is detected, and still allow any compatible capture endpoint.
- Fetch bootstrap token once per backend launch; include it on mutations. Handle backend restart/stale tokens with controlled reinitialization, not infinite retries.
- Poll state modestly (about once per second), cancel on unmount, prevent overlapping requests, and show backend-unavailable status rather than stale success.
- Keep edits distinct from saved settings: Save must succeed before Start; prevent role/device changes during active sessions. Preserve unsaved edits during polling.
- Handle loading, empty device lists, missing selected devices, request failures, and pending Start/Stop without duplicate mutations.
- Provide labels, keyboard navigation, visible focus, and readable status not conveyed by color alone.
- Proxy `/api` to Rust for development; production uses relative same-origin paths. Keep fixtures/mocks development-only.

## Acceptance

- `npm ci`, `npm run check`, and `npm run build` pass; output is static `ui/dist/` assets.
- A focused client check verifies save-before-start/error behavior using contract fixtures; manually check keyboard and empty/error states.
- UI never labels `sending` as confirmed remote connection.
- If API task is available, demonstrate bootstrap, save, start, poll, stop through the proxy. Otherwise document fixture checks and leave live integration to 11.

## Handoff

Provide build/proxy commands and mock removal points. Node.js remains build-time only.
