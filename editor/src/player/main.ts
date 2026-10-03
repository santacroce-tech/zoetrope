// The exported player: boots the same WASM core and the same `Runtime` as
// the editor preview. No editor code, no network access needed for a
// single-file export (the page works offline from a file:// URL).
//
// Page contract (written by editor/src/export.ts, see docs/EXPORT.md):
// - <canvas id="zoetrope-stage">
// - <script type="application/json" id="zoetrope-config"> PlayerConfig
// - either <script type="application/octet-stream" id="zoetrope-pack"> with
//   the gzipped pack in base64 (single file), or config.pack = URL of the
//   gzipped .zoepack next to the page (folder export; needs http(s)).
import init, { Engine } from "../wasm/pkg/zoetrope_web.js";
import coreWasm from "virtual:gz-wasm/core";
import quickjsWasm from "virtual:gz-wasm/quickjs";
import { Runtime } from "../runtime/runtime";
import { configureQuickJS } from "../runtime/scripting";
import { exposeTestHook, TEST_SEED } from "../runtime/testhook";

type ScaleMode = "letterbox" | "fill" | "fixed";

interface PlayerConfig {
  scale: ScaleMode;
  startOnClick: boolean;
  /** Folder export: the pack's URL (relative to the page). */
  pack?: string;
}

const base64Bytes = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));

async function gunzip(bytes: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([bytes as BlobPart]).stream().pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

async function loadPack(config: PlayerConfig): Promise<Uint8Array> {
  const inline = document.getElementById("zoetrope-pack")?.textContent?.trim();
  if (inline) return gunzip(base64Bytes(inline));
  if (!config.pack) throw new Error("this page has no project");
  let res: Response;
  try {
    res = await fetch(config.pack);
  } catch {
    throw new Error(
      location.protocol === "file:"
        ? "This export loads its assets from a separate file, which browsers block for pages opened from disk. Serve the folder over http(s), or export as a single file."
        : `could not load ${config.pack}`,
    );
  }
  if (!res.ok) throw new Error(`could not load ${config.pack} (${res.status})`);
  return gunzip(new Uint8Array(await res.arrayBuffer()));
}

/** Stage → window placement for a scale mode (CSS pixels). */
function place(mode: ScaleMode, w: number, h: number, sw: number, sh: number) {
  const scale = mode === "fixed" ? 1 : mode === "fill" ? Math.max(w / sw, h / sh) : Math.min(w / sw, h / sh);
  const x = (w - sw * scale) / 2;
  const y = (h - sh * scale) / 2;
  // Fixed size: keep the top-left corner visible in a small window.
  return mode === "fixed" ? { scale, x: Math.max(0, x), y: Math.max(0, y) } : { scale, x, y };
}

async function boot() {
  const canvas = document.getElementById("zoetrope-stage") as HTMLCanvasElement | null;
  if (!canvas) throw new Error("player page has no stage");
  const config: PlayerConfig = { scale: "letterbox", startOnClick: false, ...JSON.parse(document.getElementById("zoetrope-config")?.textContent || "{}") };
  const testing = /[?&#]zoetrope-test\b/.test(location.search + location.hash);

  configureQuickJS(async () => ({ wasmBinary: (await gunzip(base64Bytes(quickjsWasm))).buffer as ArrayBuffer }));
  const [wasm, pack] = await Promise.all([gunzip(base64Bytes(coreWasm)), loadPack(config)]);
  await init({ module_or_path: wasm });
  const engine = new Engine();
  engine.loadPack(pack);
  const stage = JSON.parse(engine.stageJson()) as { width: number; height: number };
  const ctx = canvas.getContext("2d")!;
  let view = { scale: 1, x: 0, y: 0 };
  let runtime: Runtime | null = null;

  let pending = 0;
  const draw = () => {
    pending = 0;
    const dpr = window.devicePixelRatio || 1;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    const [s, x, y] = [view.scale * dpr, view.x * dpr, view.y * dpr];
    // Before the first click (start-on-click), the first frame as a poster.
    if (runtime) engine.playRender(ctx, s, x, y, true, false);
    else engine.render(ctx, 0, s, x, y, true, false, "null");
  };
  const requestDraw = () => {
    if (!pending) pending = requestAnimationFrame(draw);
  };
  const layout = () => {
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    view = place(config.scale, w, h, stage.width, stage.height);
    draw();
  };
  window.addEventListener("resize", layout);
  layout();
  // Embedded images decode asynchronously; redraw once they're ready.
  void engine.decodeImages().then((n: number) => n > 0 && requestDraw());

  const start = async () => {
    runtime = await Runtime.start(engine, {
      loop: true,
      manual: testing,
      seed: testing ? TEST_SEED : undefined,
      keyTarget: window,
      onFrame: requestDraw,
      onOutput: (l) => (l.kind === "error" ? console.error(`${l.where ?? "script"}: ${l.text}`) : console.log(l.text)),
    });
    requestDraw();
    if (testing) exposeTestHook(runtime, engine);
  };

  if (config.startOnClick && !testing) {
    const button = Object.assign(document.createElement("button"), { className: "zoetrope-start", title: "Play", textContent: "▶" });
    document.body.append(button);
    await new Promise<void>((resolve) =>
      button.addEventListener("click", () => {
        button.remove();
        resolve();
      }),
    );
  }
  await start();

  const toStage = (e: PointerEvent) => {
    const r = canvas.getBoundingClientRect();
    return { x: (e.clientX - r.left - view.x) / view.scale, y: (e.clientY - r.top - view.y) / view.scale };
  };
  const pointer = (e: PointerEvent, inside = true) => {
    const over = runtime!.pointer(toStage(e), inside, (e.buttons & 1) === 1);
    canvas.style.cursor = over ? "pointer" : "default";
  };
  canvas.addEventListener("pointerdown", (e) => {
    canvas.setPointerCapture(e.pointerId);
    canvas.focus();
    pointer(e);
  });
  canvas.addEventListener("pointermove", (e) => pointer(e));
  canvas.addEventListener("pointerup", (e) => pointer(e));
  canvas.addEventListener("pointerleave", (e) => pointer(e, false));
}

boot().catch((e) => {
  console.error(e);
  const p = document.createElement("p");
  p.className = "zoetrope-error";
  p.textContent = `This movie could not start: ${e instanceof Error ? e.message : String(e)}`;
  document.body.append(p);
});
