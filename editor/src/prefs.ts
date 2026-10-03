// User preferences: per machine, not per project. Stored in localStorage
// (persistent in the Tauri webview and in browsers); every read falls back
// to the defaults, so a missing or damaged entry never breaks startup.

export interface Prefs {
  autosave: boolean;
  /** Seconds between autosaves of unsaved work. */
  autosaveSeconds: number;
  showGrid: boolean;
  gridSize: number;
  snapToGrid: boolean;
  snapToObjects: boolean;
  showRulers: boolean;
  showGuides: boolean;
  /** Loop the preview at the end of the timeline. */
  loopPreview: boolean;
}

export const DEFAULT_PREFS: Prefs = {
  autosave: true,
  autosaveSeconds: 30,
  showGrid: false,
  gridSize: 20,
  snapToGrid: false,
  snapToObjects: true,
  showRulers: true,
  showGuides: true,
  loopPreview: true,
};

const KEY = "zoetrope.prefs.v1";

export function loadPrefs(): Prefs {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    const out = { ...DEFAULT_PREFS };
    for (const k of Object.keys(DEFAULT_PREFS) as (keyof Prefs)[]) {
      if (typeof saved[k] === typeof DEFAULT_PREFS[k]) (out as Record<string, unknown>)[k] = saved[k];
    }
    out.autosaveSeconds = Math.min(600, Math.max(5, out.autosaveSeconds));
    out.gridSize = Math.min(500, Math.max(2, out.gridSize));
    return out;
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

export function savePrefs(p: Prefs) {
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    // Storage full or disabled: preferences just won't persist.
  }
}
