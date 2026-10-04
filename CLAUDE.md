# Zoetrope: notes for Claude

A Flash-style animation IDE. A Rust core is compiled to WASM and shared by the
React editor, the Tauri desktop shell and the exported HTML player.
README.md has the overview; `docs/` has the specs. Read
`docs/ARCHITECTURE.md` before structural changes.

## The one architecture rule

**Anything that affects what a frame looks like, what an edit does, or what a
script means lives in `crates/zoetrope-core`.** The editor (`editor/src`)
stays a thin shell. It holds UI state only, no copy of the model, and
re-queries the engine after each change. Don't compute geometry, timing,
hit tests or script semantics in TypeScript.

- **The core never panics on user input.** Commands return `Result`. In the
  web crate they become JS `Error`s, which the UI shows in the status bar.
- **Every model change goes through an `ops::` function that builds `Edit`s
  and runs them with `Document::execute`.**
  - One user command is one undo step, and a failing command must change
    nothing.
  - `random_edit_sequences_undo_and_redo_exactly` enforces this. Add new ops
    to its op mix.
- **The editor preview and the exported player share `editor/src/runtime/`**
  (`Runtime`, the QuickJS `ScriptHost`, `AudioEngine`). Don't fork playback
  logic for one of them: preview/export parity is a feature, verified by
  `Player::digest`.
- **The script bridge is `core::script::Call`, a JSON protocol.**
  `runtime/prelude.ts` is only the Flash-like object model on top of it, and
  sandboxed scripts reach the outside world only through it.
- **The Tauri shell does dialogs and file I/O only.** It never parses project
  contents, and it writes only to paths the user chose (`KnownProjects`, the
  recent list, `ExportTarget`). Keep it that way.

## Commands

```sh
scripts/check.sh                  # everything CI runs; must pass before a PR
cd editor && npm run wasm         # rebuild core → WASM (+ player bundle) after ANY Rust change
cd editor && npx vite --port 5199 # browser editor for manual checks
cd editor && npm run tauri dev    # native app (cargo tauri dev also works)
cargo test --workspace            # core tests
cd editor && npm test             # runtime/scripting tests under Node (needs `npm run wasm` first)
scripts/build-site.sh             # website into site/dist
```

## Gotchas

- **WASM freshness:** the editor imports the generated `editor/src/wasm/pkg`,
  which is gitignored. After a Rust change, run `npm run wasm` again. If the
  browser then shows a `LinkError`, it cached the old glue: clear the cache.
- **wasm-bindgen pin:** `wasm-bindgen = "=0.2.126"` must match the CLI
  version, in CI as well.
- **Native shell needs the frontend first:** compiling it embeds
  `editor/dist`, so on a fresh checkout `vite build` must run before clippy or
  tests of `editor/src-tauri`. `check.sh` already orders it that way.
- **Formatting:** `rustfmt.toml` (max_width 130, compact) matches the code
  style. Run `cargo fmt --all` and don't reformat unrelated code.
- **File format changes:**
  - Bump `SCHEMA_VERSION` and append a migration in `format.rs`, even for
    purely additive optional fields.
  - Add a "vN files load as vN+1" test.
  - Document it in `docs/FORMAT.md`.
- **New `Engine` APIs** are camelCase via `js_name`, take and return JSON
  strings or ids, and get a line in ARCHITECTURE.md's core ↔ frontend
  section.
- **Scripts:** frame numbers are 0-based in the core and 1-based in the JS
  API. Script scope uses `with` with a proxy, so `var` becomes a timeline
  property (docs/SCRIPTING.md).
- **Preview keyboard:** during ▶ preview the keyboard belongs to the movie.
  `App.tsx` handles only Esc then.
- **Dependencies are deliberately minimal** and pinned (QuickJS, esbuild,
  the Tauri CLI). Justify any new one in its PR.

## Workflow

- Branch and PR for every change. CI (`.github/workflows/ci.yml`) must be
  green. Don't merge without the owner's OK.
- **Docs:**
  - Keep `docs/` current with behavior.
  - User-facing changes also go in the manual (`site/src/manual/*.html`,
    listed in `site/build.mjs`).
  - Screenshots are WebP in `site/src/assets/img`, captured at 1440×900.
- **Releases:** bump the version in `editor/src-tauri/tauri.conf.json`,
  `editor/package.json` and `Cargo.toml`, merge, then `git tag vX.Y.Z && git
  push origin vX.Y.Z`. `release.yml` builds macOS, Windows and Linux and
  publishes; the website picks it up (docs/WEBSITE.md).
- **Verify UI changes in a real browser** (Playwright against the Vite dev
  server), not just with tsc. Prefer the deterministic test hook
  (`window.zoetropeTestMode`, `?zoetrope-test`) for playback checks.
