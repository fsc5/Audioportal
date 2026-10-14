# Audioportal UI (Task 10)

Requires Node.js 22.12+ and npm for development/build only. From `ui/`:

```sh
npm ci
npm run check
npm test
npm run build
```

Static output is `ui/dist/`. Embedding it in the executable belongs to Task 13.
Svelte is compiled into the browser bundle; all packages are build-time tools,
with no Node server, component library, router, or testing framework at runtime.

## Development proxy

From the repository root:

```sh
cargo run -- --dev-proxy
```

In another terminal, from `ui/`, run `npm run dev` and open
`http://127.0.0.1:5173`. The strict-port proxy preserves Host and Origin.
Windows supplies real devices and starts the selected audio/UDP pipeline.
Linux returns `503 backend_unavailable` for device enumeration and otherwise
valid Start requests. Neither the UI nor its production build simulates audio success.

Client fixture checks are in `tests/client.test.ts`, using `specs/fixtures/`.
There are no application mock removal points. Task 11 integration and hardware
verification steps are recorded in `specs/session-results.md`.

## Browser verification

Run the retained advanced-field regression check from `ui/` (port 5173 must be free):

```sh
npx playwright install chromium
npm run test:browser
```

This uses Playwright as a development-only browser driver. It checks both submit
buttons and both numeric inputs for empty, range, and step errors while Advanced
settings is collapsed: the section must open, the invalid input must receive
focus and native validation feedback, and submission must work after correction.
The test starts/stops Vite itself and serves API fixtures only in the test browser.

- Tab through controls, use arrow keys for selects and Enter/Space for buttons
  and Advanced settings. Check visible focus and readable errors.
- With empty devices, expect an explicit empty message and disabled Start;
  with a stored unavailable device ID, keep that ID visible without substitution.
- Edit a field and wait for state polling; refresh devices and confirm edits
  remain. Save & Start must save first, and save failure must prevent Start.
- Disconnect the API: the next failed poll must show unknown stream status and
  unavailable counters, not the previous successful stream state.
- Restart the API and attempt a mutation: the failed request refreshes bootstrap
  once, reports the error, and requires an explicit retry (no automatic replay).
- Through the proxy verify bootstrap, Save, Start, state polling, API errors,
  and idempotent Stop. Real streaming requires Windows audio devices.
