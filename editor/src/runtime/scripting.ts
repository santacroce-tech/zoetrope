// The script sandbox: one QuickJS VM per playback session. User scripts
// run inside it with no access to the page, the network or the file
// system. The only way out is `__host`, which reaches the core's script
// bridge (`Engine.scriptCall`) plus `trace` and error reporting. CPU time
// per entry and memory are capped so a runaway script can't hang the
// editor or the exported player. See docs/SCRIPTING.md.
import {
  newQuickJSWASMModuleFromVariant,
  newVariant,
  type CustomizeVariantOptions,
  type QuickJSContext,
  type QuickJSHandle,
  type QuickJSRuntime,
  type QuickJSWASMModule,
} from "quickjs-emscripten-core";
import variant from "@jitl/quickjs-wasmfile-release-sync";
import type { Engine } from "../engine";
import { PRELUDE } from "./prelude";

/** Longest a single script entry (one frame script, one event dispatch) may run. */
const ENTRY_BUDGET_MS = 250;
const MEMORY_LIMIT = 64 * 1024 * 1024;
const STACK_LIMIT = 1024 * 1024;

export type Path = [number, number][];

export interface OutputLine {
  kind: "trace" | "error";
  text: string;
  /** Where an error happened ("Scene 1 › Actions › frame 1", "onClick of restartButton"…). */
  where?: string;
}

export interface DueScripts {
  removed: Path[];
  instances: { path: Path; script: string; where: string }[];
  frames: { path: Path; script: string; where: string }[];
}

export type ScriptEvent =
  | { type: "enterFrame" }
  | { type: "button"; event: "press" | "release" | "click"; path: Path; name: string }
  | { type: "keyDown" | "keyUp"; key: string; code: string }
  | { type: "mouseMove" | "mouseDown" | "mouseUp"; x: number; y: number }
  | { type: "blur" };

let modulePromise: Promise<QuickJSWASMModule> | null = null;
let wasmSource: () => Promise<CustomizeVariantOptions> = () => Promise.reject(new Error("QuickJS location not configured"));

/**
 * Where the QuickJS WebAssembly comes from: the editor serves it as an
 * asset URL, the exported player inflates an embedded copy. Call before
 * the first playback.
 */
export function configureQuickJS(source: () => Promise<CustomizeVariantOptions>) {
  wasmSource = source;
  modulePromise = null;
}

/** Loads the QuickJS engine (once). */
export function loadQuickJS(): Promise<QuickJSWASMModule> {
  modulePromise ??= wasmSource().then((opts) => newQuickJSWASMModuleFromVariant(newVariant(variant, opts)));
  return modulePromise;
}

/**
 * How frame and symbol scripts are compiled: a function whose scope is the
 * timeline or instance. It opens on the script's first line, so line
 * numbers match the source.
 */
const WRAP_OPEN = "(function () { with (__scope(this)) { ";
const wrapScript = (code: string) => `${WRAP_OPEN}${code}\n} })`;

/** A syntax error; line and column are 1-based in the script's own text. */
export interface SyntaxProblem {
  line: number;
  column: number;
  message: string;
}

let checker: QuickJSContext | null = null;

/**
 * Compiles a script without running it, exactly as playback would, and
 * returns its syntax error, if any. For the script editor.
 */
export async function checkSyntax(code: string): Promise<SyntaxProblem | null> {
  const qjs = await loadQuickJS();
  checker ??= qjs.newContext();
  const r = checker.evalCode(wrapScript(code), "script", { type: "global", strict: false, compileOnly: true });
  if (!r.error) {
    r.value.dispose();
    return null;
  }
  const e = checker.dump(r.error);
  r.error.dispose();
  const at = /:(\d+):(\d+)/.exec(String(e?.stack ?? ""));
  const lines = code.split("\n");
  let line = at ? Number(at[1]) : (e?.lineNumber ?? 1);
  let column = at ? Number(at[2]) : 1;
  if (line === 1) column -= WRAP_OPEN.length;
  // Errors found only at the wrapper's closing line belong to the end of the script.
  if (line > lines.length) {
    line = lines.length;
    column = lines[line - 1].length + 1;
  }
  return { line, column: Math.max(1, column), message: String(e?.message ?? e) };
}

const keyOf = (path: Path) => path.map(([l, t]) => `${l}.${t}`).join("/");

export class ScriptHost {
  private rt: QuickJSRuntime;
  private vm: QuickJSContext;
  private deadline = Infinity;
  private compiled = new Map<string, QuickJSHandle>();
  private fns: { scope: QuickJSHandle; obj: QuickJSHandle; removed: QuickJSHandle; event: QuickJSHandle };

  constructor(
    qjs: QuickJSWASMModule,
    private engine: Engine,
    private output: (line: OutputLine) => void,
    private seed: number,
  ) {
    this.rt = qjs.newRuntime();
    this.rt.setMemoryLimit(MEMORY_LIMIT);
    this.rt.setMaxStackSize(STACK_LIMIT);
    this.rt.setInterruptHandler(() => performance.now() > this.deadline);
    this.vm = this.rt.newContext();
    const hostFn = this.vm.newFunction("__host", (msg) => this.host(this.vm.getString(msg)));
    this.vm.setProp(this.vm.global, "__host", hostFn);
    hostFn.dispose();
    this.eval(PRELUDE, "prelude.js", true);
    const get = (name: string) => this.vm.getProp(this.vm.global, name);
    this.fns = { scope: get("__scope"), obj: get("__obj"), removed: get("__removed"), event: get("__event") };
  }

  /** The bridge, as seen from inside the sandbox. */
  private host(json: string): QuickJSHandle | { error: QuickJSHandle } {
    const msg = JSON.parse(json);
    switch (msg.op) {
      case "trace":
        this.output({ kind: "trace", text: msg.text });
        return this.vm.newString("");
      case "error":
        this.output({ kind: "error", text: describe(msg.message, msg.stack), where: msg.where });
        return this.vm.newString("");
      case "stageInfo": {
        const s = JSON.parse(this.engine.stageJson());
        return this.vm.newString(JSON.stringify({ width: s.width, height: s.height, fps: s.fps, seed: this.seed }));
      }
      default:
        try {
          return this.vm.newString(this.engine.scriptCall(json));
        } catch (e) {
          return { error: this.vm.newError(e instanceof Error ? e.message : String(e)) };
        }
    }
  }

  /** Runs everything the runtime has queued (and what that queues), in order. */
  runDue() {
    // Scripts can jump timelines, which can queue more scripts; bound the cascade.
    for (let round = 0; round < 32; round++) {
      const due: DueScripts = JSON.parse(this.engine.playScriptsJson());
      if (!due.removed.length && !due.instances.length && !due.frames.length) return;
      if (due.removed.length) this.callEntry(this.fns.removed, "runtime", JSON.stringify(due.removed.map(keyOf)));
      for (const s of due.instances) this.runScript(s.script, s.where, s.path);
      for (const s of due.frames) this.runScript(s.script, s.where, s.path);
    }
    this.output({ kind: "error", text: "scripts kept jumping between frames; stopped after 32 rounds", where: "runtime" });
  }

  dispatch(e: ScriptEvent) {
    this.callEntry(this.fns.event, `${e.type} handler`, JSON.stringify(e));
    this.runDue();
  }

  /** Runs a frame or symbol script with `this` = the timeline/instance at `path`. */
  private runScript(code: string, where: string, path: Path) {
    let fn = this.compiled.get(code);
    if (!fn) {
      const r = this.vm.evalCode(wrapScript(code), where, { type: "global", strict: false });
      if (r.error) {
        this.reportHandle(r.error, where);
        r.error.dispose();
        return;
      }
      fn = r.value;
      this.compiled.set(code, fn);
    }
    const pathHandle = this.vm.unwrapResult(this.vm.evalCode(`(${JSON.stringify(path)})`));
    const self = this.vm.unwrapResult(this.vm.callFunction(this.fns.obj, this.vm.undefined, pathHandle));
    pathHandle.dispose();
    this.deadline = performance.now() + ENTRY_BUDGET_MS;
    const r = this.vm.callFunction(fn, self);
    this.deadline = Infinity;
    self.dispose();
    if (r.error) {
      this.reportHandle(r.error, where);
      r.error.dispose();
    } else r.value.dispose();
  }

  /** Calls a prelude entry point with one JSON argument. */
  private callEntry(fn: QuickJSHandle, where: string, json: string) {
    const arg = this.vm.unwrapResult(this.vm.evalCode(`(${json})`));
    this.deadline = performance.now() + ENTRY_BUDGET_MS;
    const r = this.vm.callFunction(fn, this.vm.undefined, arg);
    this.deadline = Infinity;
    arg.dispose();
    if (r.error) {
      this.reportHandle(r.error, where);
      r.error.dispose();
    } else r.value.dispose();
  }

  private eval(code: string, filename: string, strict: boolean) {
    const r = this.vm.evalCode(code, filename, { type: "global", strict });
    if (r.error) {
      const e = this.vm.dump(r.error);
      r.error.dispose();
      throw new Error(`${filename}: ${e?.message ?? e}`);
    }
    r.value.dispose();
  }

  private reportHandle(h: QuickJSHandle, where: string) {
    const e = this.vm.dump(h);
    const message = typeof e === "object" && e !== null ? `${e.name ?? "Error"}: ${e.message}` : String(e);
    const interrupted = /interrupted/i.test(message);
    this.output({
      kind: "error",
      where,
      text: interrupted ? `script ran longer than ${ENTRY_BUDGET_MS} ms and was stopped (endless loop?)` : describe(message, e?.stack),
    });
  }

  dispose() {
    for (const h of this.compiled.values()) h.dispose();
    this.compiled.clear();
    for (const h of Object.values(this.fns)) h.dispose();
    this.vm.dispose();
    this.rt.dispose();
  }
}

/** Message plus the first stack line that points into user code. */
function describe(message: string, stack?: string): string {
  const line = stack
    ?.split("\n")
    .map((s) => s.trim())
    .find((s) => /:\d+/.test(s) && !s.includes("prelude.js"));
  const at = line?.match(/:(\d+)(?::\d+)?\)?$/);
  return at ? `${message} (line ${at[1]})` : message;
}
