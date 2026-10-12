# Audioportal agent guide

## Project

Audioportal sends audio between two computers on a trusted local network. Windows is the first target; keep platform-specific audio code isolated for future OS support.

- Backend: one Rust executable, WASAPI audio, UDP transport, and an Axum/Tokio localhost API.
- Frontend: Svelte + TypeScript + Vite, client-rendered only. Build static assets and embed them in the executable; Node.js is not a runtime dependency.
- Virtual device: an existing driver such as VB-CABLE. Capture compatible endpoints without hard-coding a vendor dependency.
- V1 supports one direction per session, manual peer IP configuration, and one configuration page.

## Finding and executing tasks

- Product pitch: `specs/pitch.md`.
- Architecture and scope: `specs/implementation-plan.md`.
- Task index, dependencies, and parallel lanes: `specs/tasks/README.md`.
- Individual assignments: `specs/tasks/00-*.md` through `14-*.md`.
- Shared audio, protocol, settings, and HTTP contracts: `specs/contracts.md`.

Read your assigned task and the shared contracts first, then inspect the relevant source. Complete prerequisite tasks before relying on their interfaces. Implement only the assigned scope; do not build future features or abstractions. Shared contracts remain provisional until task 02 freezes them.

Keep changes small and consistent with existing code. Coordinate shared manifest, lockfile, entry-point, and contract changes when working in parallel. Report changed files, checks run, interface changes, and blockers at handoff. Windows hardware validation cannot be replaced by Linux tests or cross-compilation.

## Rust style

- Use stable Rust, standard `rustfmt`, idiomatic `snake_case` functions/modules and `PascalCase` types. Prefer small functions and concrete types over unnecessary traits, wrappers, or generic frameworks.
- Return `Result` for recoverable failures and propagate errors with useful context. Avoid `unwrap`/`expect` on user input, device operations, network data, and filesystem operations; they are acceptable in tests and for clearly documented invariants.
- Validate inputs at trust boundaries. Never silently fall back to another audio device or discard a persistence error.
- Prefer safe Rust. Isolate unavoidable unsafe Windows/FFI code and document each unsafe block's safety requirements.
- Keep Windows dependencies and APIs target-gated. Do not expose platform handles through shared interfaces.
- Keep audio processing bounded: no blocking network/file/HTTP work on audio threads, no unbounded queues, and no avoidable per-block allocation. Use Tokio for async I/O, not as a substitute for dedicated audio workers.
- Run `cargo fmt --check`, `cargo check`, and focused `cargo test` checks appropriate to the change. Use `cargo clippy --all-targets` when the project/toolchain supports it and address warnings introduced by your changes.

## Svelte and TypeScript style

- Use TypeScript with strict checking and the idioms of the installed Svelte version. For Svelte 5, prefer runes for new reactive state rather than mixing legacy and new styles.
- Use `PascalCase.svelte` component names, `camelCase` variables/functions, and explicit types at API boundaries. Avoid `any`; narrow unknown external data where necessary.
- Keep components focused and state local. Extract components or shared helpers when they are actually reused or materially simplify the code; do not add a global state library for this single page.
- Prefer semantic HTML, native controls, scoped CSS, and Svelte's built-in behavior. Provide labels, keyboard operation, visible focus, and status text independent of color.
- Use a small typed API client and relative `/api` URLs. Clean up polling/listeners, avoid overlapping requests, and handle loading, failure, and pending mutation states explicitly.
- No SvelteKit, SSR, or routing framework is needed. Keep development fixtures out of production behavior.
- Follow the existing formatter/linter configuration; avoid formatting unrelated code. Use Prettier with Svelte support if formatting tooling is introduced.
- With a lockfile present, use `npm ci`; run `npm run check` and `npm run build` for frontend changes. Add focused runnable checks for non-trivial client logic rather than a large test setup by default.

## Libraries and dependencies

Rust crates and Svelte/JavaScript libraries are allowed when they solve meaningful, non-trivial work. Prefer, in order: existing project code, standard library/native platform features, already-installed dependencies, then a small new dependency.

- Implement trivial glue directly. Do not add packages for a small formatting helper, a simple fetch wrapper, or behavior already covered by HTML/CSS/Svelte.
- Use maintained libraries for substantial work such as resampling, Windows API integration, protocol security, or other specialist functionality. Do not reinvent complex or security-sensitive code merely to avoid a dependency.
- Before adding a library, check maintenance, license compatibility, platform/framework support, direct size, and transitive dependencies. Prefer narrowly scoped packages and disable unnecessary Rust default features where practical.
- **For Node packages, check especially carefully:** runtime versus dev-only use, dependency tree, install footprint, browser bundle cost, ESM/tree-shaking support, and unnecessary bundled assets or frameworks. Inspect package metadata/dependencies and compare production build output when adding a runtime package.
- There is no arbitrary byte limit: weigh size against the functionality used. A package pulling in a large framework for one small feature is a poor fit; meaningful functionality can justify a larger dependency.
- Record the reason for a new dependency and any material size impact in the handoff. Commit lockfile changes alongside manifest changes when a commit is requested.

## Verification and Git

Leave a small runnable regression check for non-trivial parsing, buffering, lifecycle, persistence, timing, or security changes. Use existing test tooling; do not create tests that merely mirror trivial implementation details.

Do not claim a check passed unless it ran. Mark missing hardware or unavailable tools as blockers. Preserve unfamiliar user work. Do not commit, amend, push, or create pull requests unless explicitly requested.
