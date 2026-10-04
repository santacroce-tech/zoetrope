// Builds the website (GitHub Pages) from site/src into site/dist, or the
// directory given as the first argument. No dependencies: each page is an
// HTML fragment wrapped in src/_layout.html.
//
//   node site/build.mjs [outDir]
//
// Page metadata sits in a leading comment:
//   <!-- title: Getting started
//        description: Install Zoetrope and take a tour. -->
// The live demos (dist/demos) and the web editor (dist/app) are added by the
// Pages workflow (.github/workflows/pages.yml); see docs/WEBSITE.md.

import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = join(HERE, "src");
const OUT = process.argv[2] ?? join(HERE, "dist");
export const REPO = "https://github.com/santacroce-tech/zoetrope";

/** The manual, in reading order: [file, title in the sidebar]. */
const MANUAL = [
  ["index.html", "Contents"],
  ["getting-started.html", "Getting started"],
  ["first-animation.html", "Tutorial: your first animation"],
  ["first-game.html", "Tutorial: a small game"],
  ["drawing.html", "Drawing"],
  ["timeline.html", "Timeline and tweens"],
  ["symbols.html", "Symbols"],
  ["text-and-sound.html", "Text and sound"],
  ["scripting.html", "Scripting"],
  ["exporting.html", "Exporting"],
  ["shortcuts.html", "Keyboard shortcuts"],
  ["troubleshooting.html", "Troubleshooting"],
];

const layout = readFileSync(join(SRC, "_layout.html"), "utf8");

function meta(html) {
  const m = html.match(/^\s*<!--([\s\S]*?)-->/);
  const out = { title: "", description: "" };
  if (m) {
    for (const line of m[1].split("\n")) {
      const kv = line.match(/^\s*(\w+):\s*(.*)$/);
      if (kv) out[kv[1]] = kv[2].trim();
    }
  }
  return { ...out, body: m ? html.slice(m[0].length) : html };
}

function manualNav(file, root) {
  const i = MANUAL.findIndex(([f]) => f === file);
  const items = MANUAL.map(([f, t]) => `<li><a href="${root}manual/${f}"${f === file ? ' aria-current="page"' : ""}>${t}</a></li>`).join("\n");
  const prev = i > 0 ? `<a class="prev" href="${root}manual/${MANUAL[i - 1][0]}">← ${MANUAL[i - 1][1]}</a>` : "<span></span>";
  const next = i >= 0 && i < MANUAL.length - 1 ? `<a class="next" href="${root}manual/${MANUAL[i + 1][0]}">${MANUAL[i + 1][1]} →</a>` : "<span></span>";
  return { sidebar: `<nav class="manual-nav" aria-label="Manual"><ol>${items}</ol></nav>`, pager: `<nav class="pager">${prev}${next}</nav>` };
}

function render(rel) {
  const { title, description, body } = meta(readFileSync(join(SRC, rel), "utf8"));
  const depth = rel.split(sep).length - 1;
  const root = depth ? "../".repeat(depth) : "./";
  let content = body;
  let kind = "page";
  if (rel === "index.html") kind = "home";
  if (rel.startsWith(`manual${sep}`)) {
    kind = "manual";
    const { sidebar, pager } = manualNav(rel.slice("manual/".length), root);
    content = `<div class="manual">${sidebar}<article class="doc">${body}${pager}</article></div>`;
  }
  const fullTitle = title && rel !== "index.html" ? `${title} · Zoetrope` : "Zoetrope: animate, script and publish to the web";
  return layout
    .replaceAll("{{title}}", fullTitle)
    .replaceAll("{{description}}", description)
    .replaceAll("{{kind}}", kind)
    .replaceAll("{{root}}", root)
    .replaceAll("{{repo}}", REPO)
    .replaceAll("{{year}}", String(new Date().getFullYear()))
    .replace("{{content}}", content.replaceAll("{{root}}", root).replaceAll("{{repo}}", REPO));
}

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(relative(SRC, p));
  }
  return out;
}

// Keep anything the workflow put there (demos/, app/); rebuild the rest.
mkdirSync(OUT, { recursive: true });
for (const keep of ["assets", "manual", "index.html"]) {
  if (existsSync(join(OUT, keep))) rmSync(join(OUT, keep), { recursive: true });
}
let pages = 0;
for (const rel of walk(SRC)) {
  const base = rel.split(sep).pop();
  const dest = join(OUT, rel);
  mkdirSync(dirname(dest), { recursive: true });
  if (base.startsWith("_")) continue;
  if (rel.endsWith(".html")) {
    writeFileSync(dest, render(rel));
    pages++;
  } else cpSync(join(SRC, rel), dest);
}
writeFileSync(join(OUT, ".nojekyll"), "");
console.log(`site: ${pages} pages → ${OUT}`);
