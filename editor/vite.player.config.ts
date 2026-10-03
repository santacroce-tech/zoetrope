import { readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";

// The exported player as one self-contained script, embedded by the editor
// into exported pages. Built by `npm run player` (part of `npm run wasm`);
// output is gitignored.
//
// Both WebAssembly modules (the core and QuickJS) are embedded gzipped and
// base64-encoded, then inflated at startup with the browser's
// DecompressionStream: about a third of the size of inlining them raw.

const WASM: Record<string, string> = {
  "virtual:gz-wasm/core": fileURLToPath(new URL("src/wasm/pkg/zoetrope_web_bg.wasm", import.meta.url)),
  "virtual:gz-wasm/quickjs": fileURLToPath(new URL("node_modules/@jitl/quickjs-wasmfile-release-sync/dist/emscripten-module.wasm", import.meta.url)),
};

function gzWasm(): Plugin {
  return {
    name: "zoetrope-gz-wasm",
    enforce: "pre",
    resolveId: (id) => (id in WASM ? "\0" + id : null),
    load(id) {
      const file = WASM[id.slice(1)];
      if (!file) return null;
      const b64 = gzipSync(readFileSync(file), { level: 9 }).toString("base64");
      return `export default ${JSON.stringify(b64)};`;
    },
    // The loaders would otherwise inline a second, uncompressed copy of each
    // module for their default "fetch it next to me" path, which we never use.
    transform(code, id) {
      if (id.includes("zoetrope_web.js") || id.includes("emscripten-module")) {
        return code
          .replace(/new URL\(\s*['"]zoetrope_web_bg\.wasm['"]\s*,\s*import\.meta\.url\s*\)/g, "undefined")
          .replace(/new URL\(\s*['"]emscripten-module\.wasm['"]\s*,\s*import\.meta\.url\s*\)/g, '"emscripten-module.wasm"');
      }
      return null;
    },
  };
}

export default defineConfig({
  publicDir: false,
  plugins: [gzWasm()],
  build: {
    target: "es2022",
    outDir: "player-dist",
    emptyOutDir: true,
    lib: {
      entry: "src/player/main.ts",
      formats: ["iife"],
      name: "ZoetropePlayer",
      fileName: () => "player.js",
    },
  },
});
