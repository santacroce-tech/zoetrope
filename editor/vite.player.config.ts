import { defineConfig } from "vite";

// The exported player as one self-contained script (WASM core and QuickJS
// inlined as base64), embedded by the editor into exported HTML files.
// Built by `npm run player` (part of `npm run wasm`); output is gitignored.
export default defineConfig({
  publicDir: false,
  build: {
    target: "es2022",
    outDir: "player-dist",
    emptyOutDir: true,
    assetsInlineLimit: Number.MAX_SAFE_INTEGER,
    lib: {
      entry: "src/player/main.ts",
      formats: ["iife"],
      name: "ZoetropePlayer",
      fileName: () => "player.js",
    },
  },
});
