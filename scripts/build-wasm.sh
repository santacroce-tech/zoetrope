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

wasm-bindgen --target web $( [ "$PROFILE" = "release" ] && echo --remove-name-section ) --out-dir "$OUT" \
  "$ROOT/target/wasm32-unknown-unknown/$DIR/zoetrope_web.wasm"
# Optional extra size pass when Binaryen is installed (brew install binaryen).
if [ "$PROFILE" = "release" ] && command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    "$OUT/zoetrope_web_bg.wasm" -o "$OUT/zoetrope_web_bg.wasm"
fi
ls -l "$OUT"/zoetrope_web_bg.wasm
