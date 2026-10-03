// Runs the real runtime (WASM core + QuickJS sandbox + Runtime) under Node,
// without a browser: manual clock, no audio, no DOM keyboard.
import { readFileSync } from "node:fs";
import init, { Engine } from "../src/wasm/pkg/zoetrope_web.js";
import { configureQuickJS } from "../src/runtime/scripting";
import { Runtime } from "../src/runtime/runtime";
import type { OutputLine } from "../src/runtime/scripting";

let ready: Promise<void> | null = null;

export function setup(): Promise<void> {
  ready ??= (async () => {
    // Paths are relative to editor/ (where `npm test` runs).
    await init({ module_or_path: readFileSync("src/wasm/pkg/zoetrope_web_bg.wasm") });
    const qjs = readFileSync("node_modules/@jitl/quickjs-wasmfile-release-sync/dist/emscripten-module.wasm");
    configureQuickJS(async () => ({ wasmBinary: qjs.buffer.slice(qjs.byteOffset, qjs.byteOffset + qjs.byteLength) as ArrayBuffer }));
  })();
  return ready;
}

interface LayerNode {
  id: number;
  name: string;
  children: LayerNode[];
}

export function layerId(engine: Engine, name: string): number {
  const find = (nodes: LayerNode[]): number | undefined => {
    for (const n of nodes) {
      if (n.name === name) return n.id;
      const inner = find(n.children);
      if (inner !== undefined) return inner;
    }
  };
  const id = find(JSON.parse(engine.layersJson()));
  if (id === undefined) throw new Error(`no layer ${name}`);
  return id;
}

export interface Session {
  engine: Engine;
  runtime: Runtime;
  output: OutputLine[];
  traces: () => string[];
  errors: () => OutputLine[];
}

/** The animation demo with `script` as its frame-1 script, playing (manual clock). */
export async function withScript(script: string, opts: { seed?: number; demo?: string } = {}): Promise<Session> {
  await setup();
  const engine = new Engine();
  engine.newDemo(opts.demo ?? "animation");
  if (!opts.demo || opts.demo === "animation") engine.setFrameScript(JSON.stringify([layerId(engine, "Actions")]), 0, script);
  return start(engine, opts.seed ?? 1);
}

export async function start(engine: Engine, seed = 1): Promise<Session> {
  const output: OutputLine[] = [];
  const runtime = await Runtime.start(engine, { loop: true, manual: true, seed, onFrame: () => {}, onOutput: (l) => output.push(l) });
  return {
    engine,
    runtime,
    output,
    traces: () => output.filter((l) => l.kind === "trace").map((l) => l.text),
    errors: () => output.filter((l) => l.kind === "error"),
  };
}
