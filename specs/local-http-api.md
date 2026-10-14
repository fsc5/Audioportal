# Task 09: local HTTP API handoff

Historical Task 09 handoff: Task 11 replaces the stub and removes the `dev-api`
feature. Current commands and verification: `session-results.md`. Use
`cargo run -- --dev-proxy` for UI development and `cargo test --locked web::tests`
for HTTP regression checks. The browser boundary below still applies.

Run `cargo run --features dev-api` for the control API at `http://127.0.0.1:46001`.
The `dev-api` feature explicitly includes the development-only controller and
binary. Default builds do not include this stub. Start returns
`503 backend_unavailable`; it does not start workers or validate start-readiness.
Saved settings are still loaded/validated and lifecycle conflicts still apply.

## Browser and Vite proxy policy (Task 10)

Normal mode accepts exactly these Host/Origin pairs:

| Host | Origin |
| --- | --- |
| `127.0.0.1:46001` | `http://127.0.0.1:46001` |
| `localhost:46001` | `http://localhost:46001` |

Only an explicit `--dev-proxy` invocation additionally accepts:

| Host | Origin |
| --- | --- |
| `127.0.0.1:5173` | `http://127.0.0.1:5173` |
| `localhost:5173` | `http://localhost:5173` |

Host is always required. An Origin, if present, must match that Host's exact
Origin; mutations require it. Duplicate security headers are rejected.
Boundary rejections use `400 invalid_request`. There are no CORS grants.
Forwarded/X-Forwarded headers do not alter these checks.

Task 10 should use this Vite server configuration (the UI project does not yet
exist). Start the API with `cargo run --features dev-api -- --dev-proxy`:

```ts
server: {
  host: '127.0.0.1',
  port: 5173,
  strictPort: true,
  proxy: {
    '/api': {
      target: 'http://127.0.0.1:46001',
      changeOrigin: false,
    },
  },
}
```

Use relative `/api` requests. Preserve the browser's Host and Origin through
the proxy; do not rewrite either. Fetch `/api/bootstrap` and retain its token in
memory, then attach `X-Audioportal-Token` on PUT/POST requests. PUT settings uses
`Content-Type: application/json` and a complete settings object. Other routes
accept empty bodies only. All responses have `Cache-Control: no-store`.
Release integration must retain normal-mode rules and omit the development
proxy opt-in and development controller.

## Controller wiring (Task 11)

`SessionController::{snapshot,start,stop}` is the concrete contract surface in
`src/session.rs`. Replace this entire development stub with real worker
ownership and Start validation; remove its `dev-api` gating when integrating.
`require_stopped` is also used to serialize Save and Start with the controller.
HTTP owns one shared controller. Session locking, file I/O, and device
enumeration run in `spawn_blocking`; Save holds the session lock through
persistence, and Start holds it through loading saved settings and startup.
Bootstrap and independent reads need no controller lock.

Windows enumeration initializes and uninitializes COM on its blocking thread
and converts the existing endpoint enumeration into public devices. Other
platforms return `503 backend_unavailable`. Corrupt settings are reported and
left intact until an explicit valid PUT replaces them.

Run `cargo test --locked --features dev-api web::tests` for request-boundary,
storage, lifecycle, executor responsiveness, and fixed-port checks. The port
test needs `127.0.0.1:46001` free. Windows additionally exercises real endpoint
enumeration; it does not validate streaming audio.
