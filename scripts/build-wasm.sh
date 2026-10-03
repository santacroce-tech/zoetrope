#!/usr/bin/env sh
# Builds the shared core (zoetrope-web) to WASM and generates JS bindings.
# Output: editor/src/wasm/pkg/ (gitignored). Requires the wasm32 target and a
# wasm-bindgen CLI whose version equals the crate pin in crates/zoetrope-web.
#
#   scripts/build-wasm.sh            # release (default)
#   PROFILE=dev scripts/build-wasm.sh
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${PROFILE:-release}"
OUT="$ROOT/editor/src/wasm/pkg"

if [ "$PROFILE" = "release" ]; then
  cargo build --manifest-path "$ROOT/Cargo.toml" -p zoetrope-web --target wasm32-unknown-unknown --release
  DIR=release
else
  cargo build --manifest-path "$ROOT/Cargo.toml" -p zoetrope-web --target wasm32-unknown-unknown
  DIR=debug
fi

wasm-bindgen --target web --out-dir "$OUT" \
  "$ROOT/target/wasm32-unknown-unknown/$DIR/zoetrope_web.wasm"
ls -l "$OUT"/zoetrope_web_bg.wasm
