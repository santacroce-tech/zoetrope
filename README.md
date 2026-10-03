# Zoetrope

A Flash-like animation IDE (native desktop via Tauri) and web runtime, built on
one Rust/WASM core shared by the editor preview and the exported player.

- **Draw and animate:**
  - Vector shapes, a pen and pencil, gradients, and bitmaps.
  - Layers and folders.
  - Keyframes with motion and shape tweens and easing.
  - Graphic, movie-clip and button symbols, which can nest.
  - Text in embedded fonts, and synced audio.
- **Script it** in sandboxed JavaScript with a Flash-like API.
- **Export it** as a single offline HTML file, or as a folder for the web.
  Either one plays exactly like the editor's preview.

## Documentation

| Document | What's in it |
|----------|--------------|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Parts and boundaries: core ↔ frontend, frontend ↔ Tauri, the WASM build, performance, reliability |
| [docs/FORMAT.md](docs/FORMAT.md) | The `.zoe` project format (versioned, with migrations) and the `.zoepack` pack |
| [docs/SCRIPTING.md](docs/SCRIPTING.md) | Scripting API reference, event order, sandbox and limits |
| [docs/EXPORT.md](docs/EXPORT.md) | Export formats, the player, and how preview/export parity is verified |
| [docs/TESTING.md](docs/TESTING.md) | The test suites and `scripts/check.sh` |

In the app, press **?** for every keyboard shortcut and **⚙** for preferences.

## Prerequisites

* Rust (stable) with `rustup target add wasm32-unknown-unknown`
* `wasm-bindgen` CLI **0.2.126** (`cargo install wasm-bindgen-cli --version 0.2.126`)
* Node 20+ and the Tauri CLI (`cargo install tauri-cli --version "^2"`)

## Commands

```sh
cd editor && npm install          # once

# Native app (builds WASM, starts Vite, launches Tauri)
cd editor && cargo tauri dev

# Browser-only editor (no native shell; save = download, open = upload)
cd editor && npm run wasm && npm run dev    # http://localhost:5173

# WASM core only
scripts/build-wasm.sh

# Every check (format, clippy, Rust tests, WASM build, typecheck, runtime tests, build)
scripts/check.sh

# Individually
cargo test --workspace                       # core: model, undo, timeline, runtime, formats
cd editor && npm test                        # scripting runtime under Node (real WASM + QuickJS)
cargo run -p zoetrope-core --release --example profile   # traversal profile

# Release bundle
cd editor && cargo tauri build
```

## Layout

```
crates/zoetrope-core   scene graph, edits/undo, geometry, render pipeline, file format
crates/zoetrope-web    WASM bindings + Canvas2D renderer
editor/                React/TS editor UI
editor/src-tauri       native shell (dialogs + file I/O only)
editor/src/runtime     playback shared by preview and export: Runtime, script sandbox, audio
editor/src/player      the exported player (bundled by `npm run player`)
editor/tests           runtime tests (Node + real WASM + QuickJS)
scripts/build-wasm.sh  cargo → wasm32 → wasm-bindgen (optional wasm-opt)
scripts/check.sh       all automated checks
```
