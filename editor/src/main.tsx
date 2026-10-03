import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import quickjsWasm from "@jitl/quickjs-wasmfile-release-sync/wasm?url";
import { configureQuickJS } from "./runtime/scripting";
import "./styles.css";

// The script sandbox's engine, served as a regular asset.
configureQuickJS(async () => ({ wasmLocation: quickjsWasm }));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
