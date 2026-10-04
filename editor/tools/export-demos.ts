// Exports the built-in demos as standalone pages (the same code path as
// the editor's Export dialog), for the website's live examples.
//   npm run export-demos -- <output dir>
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import init, { Engine } from "../src/wasm/pkg/zoetrope_web.js";
import { buildExport, type PublishSettings } from "../src/export";

const out = process.argv[2] ?? "dist-demos";
const DEMOS: { kind: string; file: string; title: string }[] = [
  { kind: "animation", file: "spring", title: "Spring in Zoetrope" },
  { kind: "game", file: "bee-catcher", title: "Bee Catcher" },
];

await init({ module_or_path: readFileSync("src/wasm/pkg/zoetrope_web_bg.wasm") });
const player = readFileSync("player-dist/player.js", "utf8");
mkdirSync(out, { recursive: true });
for (const d of DEMOS) {
  const engine = new Engine();
  engine.newDemo(d.kind);
  const settings: PublishSettings = { title: d.title, scale: "letterbox", mode: "singleFile", pageColor: "#0f1020", startOnClick: true };
  const [page] = await buildExport(engine, settings, d.title, player);
  writeFileSync(join(out, `${d.file}.html`), page.data);
  console.log(`${d.file}.html  ${(String(page.data).length / 1024).toFixed(0)} KB`);
}
