import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and must see Rust-side errors in the terminal.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  // The exported-player chunk (~1.4 MB, both WASM modules inlined) only
  // loads when exporting, so it doesn't slow startup.
  build: { target: "es2022", chunkSizeWarningLimit: 1600 },
});
