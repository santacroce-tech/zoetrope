// The exported player: boots the same WASM core and the same `Runtime` as
// the editor preview, with the project embedded in the page. No editor
// code, no network access: the page works offline from a file:// URL.
// Phase 7 ships it with a letterbox fit; Phase 8 adds scaling options,
// an assets-folder mode and embed snippets.
import init, { Engine } from "../wasm/pkg/zoetrope_web.js";
import { Runtime } from "../runtime/runtime";

async function boot() {
  const canvas = document.getElementById("zoetrope-stage") as HTMLCanvasElement;
  const data = document.getElementById("zoetrope-project")?.textContent;
  if (!canvas || !data) throw new Error("player page is missing its stage or project");
  await init();
  const engine = new Engine();
  engine.loadJson(data);
  const stage = JSON.parse(engine.stageJson()) as { width: number; height: number };
  const ctx = canvas.getContext("2d")!;
  let view = { scale: 1, x: 0, y: 0 };

  // Letterbox: the whole stage, as large as fits, centered.
  const layout = () => {
    const w = window.innerWidth;
    const h = window.innerHeight;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    const scale = Math.min(w / stage.width, h / stage.height);
    view = { scale, x: (w - stage.width * scale) / 2, y: (h - stage.height * scale) / 2 };
    draw();
  };
  let pending = 0;
  const draw = () => {
    pending = 0;
    const dpr = window.devicePixelRatio || 1;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    engine.playRender(ctx, view.scale * dpr, view.x * dpr, view.y * dpr, true, false);
  };
  const requestDraw = () => {
    if (!pending) pending = requestAnimationFrame(draw);
  };

  // Embedded images decode asynchronously; redraw once they're ready.
  void engine.decodeImages().then((n: number) => n > 0 && requestDraw());
  const runtime = await Runtime.start(engine, {
    loop: true,
    onFrame: requestDraw,
    onOutput: (l) => (l.kind === "error" ? console.error(`${l.where ?? "script"}: ${l.text}`) : console.log(l.text)),
  });
  window.addEventListener("resize", layout);
  layout();

  const toStage = (e: PointerEvent) => {
    const r = canvas.getBoundingClientRect();
    return { x: (e.clientX - r.left - view.x) / view.scale, y: (e.clientY - r.top - view.y) / view.scale };
  };
  const pointer = (e: PointerEvent, inside = true) => {
    const over = runtime.pointer(toStage(e), inside, (e.buttons & 1) === 1);
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
  document.body.insertAdjacentHTML("beforeend", `<p style="color:#f88;font:14px system-ui;position:fixed;top:8px;left:8px">Could not start: ${String(e?.message ?? e)}</p>`);
});
