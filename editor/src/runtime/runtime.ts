// Playback session shared by the editor preview and the exported player:
// fixed-rate ticks at the stage fps, the script sandbox, keyboard/pointer
// input and audio. The core decides what each tick means; this only drives
// it, so preview and export behave identically. Rendering is left to the
// caller (`onFrame`), which knows its canvas and view.
import type { Engine, Pt } from "../engine";
import { AudioEngine } from "./audio";
import { loadQuickJS, ScriptHost, type OutputLine, type Path } from "./scripting";

/** A stalled tab never catches up more than this many frames at once. */
const MAX_CATCH_UP = 8;

export interface RuntimeOptions {
  /** Loop the main timeline at its end (the player always does). */
  loop: boolean;
  /** Called after every state change worth redrawing; `frame` = main timeline frame. */
  onFrame: (frame: number) => void;
  onOutput: (line: OutputLine) => void;
  /** Playback reached the end with `loop` off. */
  onEnd?: () => void;
  /** Where keyboard events come from (default: window). */
  keyTarget?: EventTarget;
  /** Seed for the scripts' Math.random (default: random). */
  seed?: number;
  /**
   * Don't run on the wall clock: frames advance only via `advance(n)`.
   * With a fixed seed this makes a session exactly replayable (parity tests).
   */
  manual?: boolean;
}

const isTyping = (t: EventTarget | null) =>
  t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || t instanceof HTMLSelectElement;

/** Keys a game would want that the page would otherwise use to scroll. */
const CAPTURED = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", " ", "PageUp", "PageDown", "Home", "End"]);

export class Runtime {
  private scripts: ScriptHost | null = null;
  private audio: AudioEngine;
  private raf = 0;
  private t0 = 0;
  private done = 0;
  private running = false;
  private down = false;
  private detach: (() => void) | null = null;

  private constructor(
    private engine: Engine,
    private opts: RuntimeOptions,
  ) {
    this.audio = new AudioEngine(engine);
  }

  /** Starts playback of the main timeline from the engine's current frame. */
  static async start(engine: Engine, opts: RuntimeOptions): Promise<Runtime> {
    const rt = new Runtime(engine, opts);
    const audioIds = JSON.parse(engine.audioJson()).map((a: { id: number }) => a.id);
    // Decode sounds and load the sandbox first so the opening frames aren't silent or script-less.
    const [qjs] = await Promise.all([
      loadQuickJS(),
      rt.audio.preload(audioIds).catch((e) => opts.onOutput({ kind: "error", where: "audio", text: `audio unavailable: ${e}` })),
    ]);
    engine.playStart();
    const seed = opts.seed ?? crypto.getRandomValues(new Uint32Array(1))[0];
    rt.scripts = new ScriptHost(qjs, engine, opts.onOutput, seed);
    rt.scripts.runDue();
    rt.syncAudio();
    rt.running = true;
    rt.attachKeys();
    rt.t0 = performance.now();
    if (!opts.manual) rt.raf = requestAnimationFrame(rt.loop);
    opts.onFrame(engine.playFrame());
    return rt;
  }

  private loop = (now: number) => {
    if (!this.running) return;
    const fps = JSON.parse(this.engine.stageJson()).fps as number;
    // rAF timestamps can precede t0 (they mark the frame's start): clamp.
    const due = Math.max(0, Math.floor(((now - this.t0) * fps) / 1000));
    let steps = due - this.done;
    if (steps > MAX_CATCH_UP) {
      // Fell far behind (background tab, breakpoint): skip ahead instead of fast-forwarding.
      this.done = due - MAX_CATCH_UP;
      steps = MAX_CATCH_UP;
    }
    for (let i = 0; i < steps && this.running; i++) {
      this.done++;
      this.step();
    }
    if (steps > 0 && this.running) this.opts.onFrame(this.engine.playFrame());
    if (this.running) this.raf = requestAnimationFrame(this.loop);
  };

  /** One frame: advance clocks, run entered frames' scripts, then onEnterFrame handlers. */
  private step() {
    if (!this.opts.loop) {
      const t = JSON.parse(this.engine.scriptCall(JSON.stringify({ op: "timeline", path: [] })));
      if (t.playing && t.frame === t.length - 1) {
        this.opts.onEnd?.();
        return;
      }
    }
    this.engine.playTick(1);
    this.scripts!.runDue();
    this.scripts!.dispatch({ type: "enterFrame" });
    this.syncAudio();
  }

  /** Manual mode: advance `n` frames now. */
  advance(n: number) {
    for (let i = 0; i < n && this.running; i++) this.step();
    this.opts.onFrame(this.engine.playFrame());
  }

  /** Keyboard input from code (tests, on-screen controls). */
  key(type: "keyDown" | "keyUp", key: string, code = key) {
    this.scripts?.dispatch({ type, key, code });
    this.opts.onFrame(this.engine.playFrame());
  }

  /** Fingerprint of the current picture and sound (see `Player::digest`). */
  digest(): string {
    return this.engine.playDigest();
  }

  private syncAudio() {
    this.audio.sync(JSON.parse(this.engine.playAudioJson()));
  }

  /** Pointer input in stage coordinates (`inside` false when it left the stage). Returns whether it's over a button. */
  pointer(at: Pt, inside: boolean, down: boolean): boolean {
    if (!this.running || !this.scripts) return false;
    if (down) void this.audio.resume();
    const r = JSON.parse(this.engine.playPointer(at.x, at.y, inside, down));
    const type = down && !this.down ? "mouseDown" : !down && this.down ? "mouseUp" : "mouseMove";
    this.down = down;
    this.scripts.dispatch({ type, x: at.x, y: at.y });
    for (const ev of r?.events ?? []) {
      this.scripts.dispatch({ type: "button", event: ev.type as "press" | "release" | "click", path: ev.path as Path, name: ev.name });
    }
    this.opts.onFrame(this.engine.playFrame());
    return !!r?.overButton;
  }

  private attachKeys() {
    if (!this.opts.keyTarget && typeof window === "undefined") return; // no keyboard (tests)
    const target = this.opts.keyTarget ?? window;
    const key = (type: "keyDown" | "keyUp") => (e: Event) => {
      const k = e as KeyboardEvent;
      if (isTyping(k.target) || k.metaKey || k.ctrlKey) return;
      if (CAPTURED.has(k.key)) k.preventDefault();
      if (type === "keyDown") void this.audio.resume();
      if (type === "keyDown" && k.repeat) return;
      this.scripts?.dispatch({ type, key: k.key, code: k.code });
      this.opts.onFrame(this.engine.playFrame());
    };
    const down = key("keyDown");
    const up = key("keyUp");
    const blur = () => this.scripts?.dispatch({ type: "blur" });
    target.addEventListener("keydown", down);
    target.addEventListener("keyup", up);
    const win = typeof window === "undefined" ? null : window;
    win?.addEventListener("blur", blur);
    this.detach = () => {
      target.removeEventListener("keydown", down);
      target.removeEventListener("keyup", up);
      win?.removeEventListener("blur", blur);
    };
  }

  /** Ends the session (sandbox state is discarded). Returns the main timeline frame reached. */
  stop(): number {
    this.running = false;
    if (this.raf) cancelAnimationFrame(this.raf);
    this.detach?.();
    this.audio.stopAll();
    this.scripts?.dispose();
    this.scripts = null;
    return this.engine.playStop();
  }
}
