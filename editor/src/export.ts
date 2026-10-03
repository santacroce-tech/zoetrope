// "Export HTML": one self-contained page that plays the project offline:
// the player script (same WASM core and runtime as the preview, built by
// `npm run player`) plus the project file, embedded inline.
import type { Engine } from "./engine";

const escapeHtml = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

/** Inline <script> content must never contain "</script" (or "<!--"). */
const scriptSafe = (s: string) => s.replace(/<\/(script)/gi, "<\\/$1").replace(/<!--/g, "<\\!--");

export async function buildHtml(engine: Engine, title: string): Promise<string> {
  const { default: player } = await import("../player-dist/player.js?raw");
  const stage = JSON.parse(engine.stageJson());
  // In JSON, "<" only occurs inside strings, where \u003c means the same.
  const project = engine.saveJson().replace(/</g, "\\u003c");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="generator" content="Zoetrope">
<title>${escapeHtml(title)}</title>
<style>
  html, body { margin: 0; height: 100%; overflow: hidden; background: #111; }
  #zoetrope-stage { display: block; width: 100vw; height: 100vh; outline: none; touch-action: none; }
</style>
</head>
<body>
<canvas id="zoetrope-stage" tabindex="0" aria-label="${escapeHtml(title)} (${stage.width}×${stage.height})"></canvas>
<script type="application/json" id="zoetrope-project">${project}</script>
<script>${scriptSafe(player)}</script>
</body>
</html>
`;
}
