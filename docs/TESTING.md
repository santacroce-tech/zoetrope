# Testing

`scripts/check.sh` runs everything below, in the order CI should. It stops
at the first failure.

| Step | Command | What it covers |
|------|---------|----------------|
| Format | `cargo fmt --all --check` | Rust style (`rustfmt.toml`) |
| Lints | `cargo clippy … -D warnings`, native and `wasm32` | Rust code quality, including the WASM-only renderer |
| Core tests | `cargo test --workspace` | See below |
| Build | `npm run wasm` | Core → WASM (`scripts/build-wasm.sh`), then the player bundle |
| Typecheck | `npx tsc -b` | The editor, runtime and player (tests and tools are type-stripped by esbuild) |
| Runtime tests | `npm test` (in `editor/`) | See below |
| Editor build | `npx vite build` | The production frontend |

## Core tests (`crates/zoetrope-core`)

- **Unit tests** sit next to the code: geometry, math, color, timeline
  interpolation and easing, text layout, asset sniffing, and so on.
- **Integration tests** are in `tests/core.rs`. They are grouped by phase;
  the main areas are:
  - **Serialization:** every demo round-trips through JSON and the pack
    format. Migrations from v1 fixtures up to v6 are covered, damaged or
    newer files are rejected, and validation errors are readable.
  - **Undo:** each operation is undoable and atomic.
    `random_edit_sequences_undo_and_redo_exactly` runs 4 seeds × 300 random
    commands across 26 kinds of edits and checks that:
    - every command adds exactly one undo step;
    - a failing command changes nothing;
    - every state survives JSON and pack round trips;
    - undoing all and redoing all retraces every state exactly.
  - **Timeline and runtime:** keyframes and tweens; movie-clip clocks
    (the runtime matches the stateless evaluator until the first loop);
    buttons; sound cues.
  - **Scripting semantics in the core:** entering frames queues scripts,
    goto by label, overrides drive rendering and collisions, removed
    instances lose their state.
  - **Export parity:** `exported_projects_play_identically` plays a scripted
    session and compares per-frame digests after a JSON round trip and after
    a pack round trip.
- **Profiling** is opt-in:
  - `cargo test --release -p zoetrope-core --test core profile_stress_scene -- --ignored --nocapture`
  - `cargo run -p zoetrope-core --release --example profile`

## Runtime tests (`editor/tests`, `npm test`)

These run the real playback stack under Node: the WASM core, the QuickJS
sandbox and `Runtime`, with a manual clock and no DOM or audio. They are
bundled with esbuild, a dev dependency already used by Vite, and run with
Node's built-in `node --test`, so there is no test framework dependency.

They cover:
- scope rules (`var` on the timeline, `let`/`const` local);
- named children and properties;
- the sandbox having no `window`, `document`, `fetch`, `require` or
  `process`;
- error locations and line numbers;
- the CPU and memory limits;
- stop, play and goto by label, with frame scripts after jumps;
- event order;
- keys and stage events, and button handlers;
- seeded `Math.random`;
- deterministic replay of the game demo;
- cleanup when instances leave the stage.

## Browser checks (manual / Playwright)

Rendering and the export pipeline need a real browser:

- **Parity:** the editor dev build (`window.zoetropeTestMode = true` before
  ▶) and an exported page (`?zoetrope-test`) expose the same stepping hook.
  Run the same input script in both and compare `digest()` per frame. See
  EXPORT.md for the results.
- **Performance:** load **New demo… → Stress test** and read the `render … ms`
  figure in the status bar while stepping frames.
