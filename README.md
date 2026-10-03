# Zoetrope

A Flash-like animation IDE (native desktop via Tauri) and web runtime, built on
one Rust/WASM core shared by the editor preview and the exported player.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/FORMAT.md](docs/FORMAT.md).

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

# Tests & profiling (headless, no UI)
cargo test -p zoetrope-core
cargo run -p zoetrope-core --release --example profile

# Release bundle
cd editor && cargo tauri build
```

## Layout

```
crates/zoetrope-core   scene graph, edits/undo, geometry, render pipeline, file format
crates/zoetrope-web    WASM bindings + Canvas2D renderer
editor/                React/TS editor UI
editor/src-tauri       native shell (dialogs + file I/O only)
scripts/build-wasm.sh  cargo → wasm32 → wasm-bindgen
```
