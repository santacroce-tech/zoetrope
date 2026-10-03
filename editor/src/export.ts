// Export ("publish"): pages that play the project with the same WASM core
// and runtime as the preview (see docs/EXPORT.md).
//
// - Single file: one .html holding the player script and the gzipped pack
//   (base64). Opens offline, even from file://.
// - Folder: page.html + zoetrope-player.js (shared, cacheable) +
//   page.zoepack (gzipped pack, loaded with fetch; needs http(s)).
import type { Engine } from "./engine";

export type ScaleMode = "letterbox" | "fill" | "fixed";
export type ExportMode = "singleFile" | "folder";

/** Saved with the project (`Project.publish`). */
export interface PublishSettings {
  title: string;
  scale: ScaleMode;
  mode: ExportMode;
  pageColor: string;
  startOnClick: boolean;
}

export interface ExportFile {
  name: string;
  data: string | Uint8Array;
}

export const PLAYER_FILE = "zoetrope-player.js";

const escapeHtml = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

/** Inline <script> content must never contain "</script" or "<!--". */
const scriptSafe = (s: string) => s.replace(/<\/(script)/gi, "<\\/$1").replace(/<!--/g, "<\\!--");

/** A file-name-safe version of a title. */
export function baseName(title: string): string {
  return title.trim().replace(/[\\/:*?"<>|]+/g, "-").replace(/\s+/g, " ").slice(0, 80) || "Untitled";
}

async function gzip(bytes: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([bytes as BlobPart]).stream().pipeThrough(new CompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

async function playerScript(): Promise<string> {
  // Built by `npm run player`; loaded only when exporting.
  const { default: player } = await import("../player-dist/player.js?raw");
  return player;
}

function page(o: { title: string; settings: PublishSettings; config: object; body: string; width: number; height: number }) {
  const t = escapeHtml(o.title);
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="generator" content="Zoetrope">
<title>${t}</title>
<style>
  html, body { margin: 0; height: 100%; overflow: hidden; background: ${o.settings.pageColor}; }
  #zoetrope-stage { display: block; width: 100vw; height: 100vh; outline: none; touch-action: none; }
  .zoetrope-start { position: fixed; inset: 0; margin: auto; width: 96px; height: 96px; border-radius: 50%; border: 0;
    background: rgba(0, 0, 0, 0.55); color: #fff; font-size: 40px; cursor: pointer; }
  .zoetrope-start:hover { background: rgba(0, 0, 0, 0.75); }
  .zoetrope-error { position: fixed; top: 8px; left: 8px; right: 8px; margin: 0; color: #fbb; font: 14px/1.4 system-ui, sans-serif; }
</style>
</head>
<body>
<canvas id="zoetrope-stage" tabindex="0" aria-label="${t} (${o.width}×${o.height})"></canvas>
<script type="application/json" id="zoetrope-config">${scriptSafe(JSON.stringify(o.config))}</script>
${o.body}
</body>
</html>
`;
}

/** The files to write for the current project and its publish settings. */
export async function buildExport(engine: Engine, settings: PublishSettings, fallbackTitle: string): Promise<ExportFile[]> {
  const title = settings.title.trim() || fallbackTitle;
  const base = baseName(title);
  const stage = JSON.parse(engine.stageJson());
  const [player, pack] = await Promise.all([playerScript(), gzip(engine.savePack())]);
  const common = { title, settings, width: stage.width, height: stage.height };
  const config = { scale: settings.scale, startOnClick: settings.startOnClick };
  if (settings.mode === "singleFile") {
    const body = `<script type="application/octet-stream" id="zoetrope-pack">${toBase64(pack)}</script>
<script>${scriptSafe(player)}</script>`;
    return [{ name: `${base}.html`, data: page({ ...common, config, body }) }];
  }
  const packName = `${base}.zoepack`;
  const body = `<script src="${PLAYER_FILE}"></script>`;
  return [
    { name: `${base}.html`, data: page({ ...common, config: { ...config, pack: packName }, body }) },
    { name: PLAYER_FILE, data: player },
    { name: packName, data: pack },
  ];
}

/** HTML to embed an exported page in another site. */
export function embedSnippet(engine: Engine, settings: PublishSettings, fallbackTitle: string): string {
  const title = settings.title.trim() || fallbackTitle;
  const stage = JSON.parse(engine.stageJson());
  return `<iframe src="${escapeHtml(encodeURI(baseName(title) + ".html"))}" width="${stage.width}" height="${stage.height}" title="${escapeHtml(title)}" style="border:0; max-width:100%; aspect-ratio:${stage.width} / ${stage.height}; height:auto" allow="autoplay; fullscreen" allowfullscreen></iframe>`;
}

export const totalSize = (files: ExportFile[]) =>
  files.reduce((n, f) => n + (typeof f.data === "string" ? new TextEncoder().encode(f.data).length : f.data.length), 0);
