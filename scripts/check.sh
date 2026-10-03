#!/usr/bin/env sh
# Every automated check, in the order CI should run them. Fails on the first
# problem. Run from anywhere: scripts/check.sh
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "== format";            cargo fmt --all --check
echo "== clippy (native)";   cargo clippy --workspace --all-targets -q -- -D warnings
echo "== clippy (wasm32)";   cargo clippy -p zoetrope-web --target wasm32-unknown-unknown -q -- -D warnings
echo "== core tests";        cargo test --workspace -q
echo "== wasm + player";     (cd editor && npm run wasm --silent)
echo "== typecheck";         (cd editor && npx tsc -b)
echo "== runtime tests";     (cd editor && npm test --silent)
echo "== editor build";      (cd editor && npx vite build --logLevel warn)
echo "All checks passed."
