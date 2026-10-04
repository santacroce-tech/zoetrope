#!/usr/bin/env sh
# Builds the complete website into site/dist (what GitHub Pages serves):
#   /            landing page + manual   (site/src, site/build.mjs)
#   /demos/      live exported demos     (editor/tools/export-demos.ts)
#   /app/        the web editor          (editor, built with a relative base)
# Assumes `npm ci` has run in editor/. Preview with:
#   python3 -m http.server -d site/dist 8080
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/site/dist"
rm -rf "$OUT"
cd "$ROOT/editor"
npm run wasm --silent
npm run export-demos --silent -- "$OUT/demos"
npx vite build --base ./ --outDir "$OUT/app" --emptyOutDir --logLevel warn
node "$ROOT/site/build.mjs" "$OUT"
echo "Site built in $OUT"
