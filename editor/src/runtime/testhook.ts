// Deterministic stepping for parity tests: the exported player (with
// `?zoetrope-test` in its URL) and the editor's dev build (with
// `window.zoetropeTestMode = true` before pressing ▶) expose the same hook,
// so a test can drive both identically and compare `digest()` per frame.
import type { Engine } from "../engine";
import type { Runtime } from "./runtime";

export interface TestHook {
  ready: boolean;
  advance(n: number): void;
  digest(): string;
  frame(): number;
  key(type: "keyDown" | "keyUp", key: string, code?: string): void;
  pointer(x: number, y: number, down: boolean): boolean;
}

declare global {
  interface Window {
    zoetropeTest?: TestHook;
    zoetropeTestMode?: boolean;
  }
}

/** Sessions under test use this seed for the scripts' Math.random. */
export const TEST_SEED = 1;

export function exposeTestHook(runtime: Runtime, engine: Engine) {
  window.zoetropeTest = {
    ready: true,
    advance: (n) => runtime.advance(n),
    digest: () => runtime.digest(),
    frame: () => engine.playFrame(),
    key: (type, key, code) => runtime.key(type, key, code),
    pointer: (x, y, down) => runtime.pointer({ x, y }, true, down),
  };
}
