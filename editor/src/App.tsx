import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  loadEngine,
  type Engine,
  type HistoryState,
  type LayerNode,
  type NodeRef,
  type PickedStyle,
  type Pt,
  type ShapeStyle,
  type StageInfo,
  type LibraryItem,
  type Crumb,
  type SymbolKind,
  type FontAsset,
  type TextStyle,
  SYMBOL_KIND_ICON,
  SYMBOL_KIND_LABEL,
} from "./engine";
import { LibraryPanel } from "./components/LibraryPanel";
import { OutputPanel } from "./components/OutputPanel";
import { Runtime } from "./runtime/runtime";
import { exposeTestHook, TEST_SEED } from "./runtime/testhook";
import type { OutputLine } from "./runtime/scripting";
import { describeImport, importFonts } from "./assets";
import {
  clearAutosave,
  closeWindow,
  exitApp,
  fileToBinary,
  onCloseRequested,
  onMenu,
  onOpenFileRequest,
  openUrl,
  takePendingOpen,
  importImages,
  isTauri,
  openProject,
  openRecent,
  readAutosave,
  recentFiles,
  saveProject,
  writeAutosave,
  type AutosaveEntry,
  type PickedBinary,
  type RecentFile,
} from "./platform";
import { loadPrefs, savePrefs, type Prefs } from "./prefs";
import { PrefsDialog } from "./components/PrefsDialog";
import { ShortcutsDialog } from "./components/ShortcutsDialog";
import { ExportDialog } from "./components/ExportDialog";
import { StageView, type StageSettings, type Tool, type ToolOptions } from "./components/StageView";
import { Timeline, type FrameOp, type OnionSettings } from "./components/Timeline";
import { PropertiesPanel } from "./components/PropertiesPanel";
import { TOOL_KEYS, ToolOptionsBar, ToolPalette } from "./components/ToolPalette";
import { clampZoom, zoomAt, type View } from "./view";

export default function App() {
  const [engine, setEngine] = useState<Engine | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    loadEngine().then(setEngine, (e) => setLoadError(String(e)));
  }, []);

  if (loadError) return <div className="boot">Failed to load the WASM core: {loadError}</div>;
  if (!engine) return <div className="boot">Loading core…</div>;
  return <Editor engine={engine} />;
}

type PendingDiscard = { label: string; run: () => void } | null;

function findLayer(nodes: LayerNode[], kind: LayerNode["kind"]): number | null {
  for (const n of nodes) {
    if (n.kind === kind) return n.id;
    const inner = findLayer(n.children, kind);
    if (inner !== null) return inner;
  }
  return null;
}

/** Topmost normal layer (falling back to a guide layer), for drawing into. */
const firstContentLayer = (nodes: LayerNode[]) => findLayer(nodes, "normal") ?? findLayer(nodes, "guide");

function containsLayer(nodes: LayerNode[], id: number): boolean {
  return nodes.some((n) => n.id === id || containsLayer(n.children, id));
}

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

const SITE = "https://santacroce-tech.github.io/zoetrope/";
const REPO = "https://github.com/santacroce-tech/zoetrope";

/**
 * The action (native menu item id) a key press stands for, if any. Keys
 * and menu accelerators are the same; see src-tauri/src/menu.rs.
 */
function shortcutAction(e: KeyboardEvent): string | null {
  const mod = e.metaKey || e.ctrlKey;
  const k = e.key.toLowerCase();
  if (e.key === "F8") return "convert-to-symbol";
  if (!e.shiftKey && e.key === "F5") return "insert-frame";
  if (!e.shiftKey && e.key === "F6") return "insert-keyframe";
  if (e.key === "F7") return "insert-blank-keyframe";
  if (!mod) return null;
  if (e.shiftKey) {
    return { s: "save-as", e: "export", z: "redo", v: "paste-in-place", "}": "bring-to-front", "]": "bring-to-front", "{": "send-to-back", "[": "send-to-back", '"': "toggle-snap-grid", "'": "toggle-snap-grid" }[k] ?? null;
  }
  return (
    {
      n: "new", o: "open", s: "save", i: "import", w: "close", q: "quit", ",": "preferences",
      z: "undo", y: "redo", a: "select-all", d: "duplicate", e: "edit-symbol", b: "convert-to-path",
      "]": "bring-forward", "[": "send-backward", "=": "zoom-in", "+": "zoom-in", "-": "zoom-out",
      "0": "zoom-fit", "1": "zoom-100", "'": "toggle-grid",
    }[k] ?? null
  );
}

/**
 * The last shortcut pressed anywhere in the page, including text fields.
 * A native menu event for the same action right after it means the key
 * reached both the webview and the menu: the webview already handled it.
 */
const lastShortcut = { id: "", t: -Infinity };
window.addEventListener(
  "keydown",
  (e) => {
    const id = shortcutAction(e);
    if (id) Object.assign(lastShortcut, { id, t: performance.now() });
  },
  true,
);

function Editor({ engine }: { engine: Engine }) {
  // The model lives in the core. `version` is bumped after every change so
  // derived views re-query it.
  const [version, setVersion] = useState(0);
  const [selection, setSelection] = useState<number[]>([]);
  const [activeLayer, setActiveLayer] = useState<number | null>(null);
  const [tool, setTool] = useState<Tool>("select");
  const [shapeStyle, setShapeStyle] = useState<ShapeStyle>({
    fill: { type: "solid", color: "#e86a92" },
    stroke: { color: "#222222", width: 2, cap: "round", join: "round", dash: [] },
  });
  const [toolOptions, setToolOptions] = useState<ToolOptions>({ sides: 5, star: null, pencilSmooth: true });
  const [textStyle, setTextStyle] = useState<TextStyle>({
    font: null,
    size: 32,
    color: "#222222",
    align: "left",
    letterSpacing: 0,
    lineHeight: 1.25,
  });
  const [anchors, setAnchors] = useState<NodeRef[]>([]);
  const [frame, setFrameState] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [prefs, setPrefsState] = useState<Prefs>(loadPrefs);
  const [loop, setLoopState] = useState(prefs.loopPreview);
  const [onion, setOnion] = useState<OnionSettings>({ enabled: false, before: 2, after: 2, alpha: 0.35 });
  const [frameFocus, setFrameFocus] = useState<{ layer: number; frame: number } | null>(null);
  const [settings, setSettings] = useState<StageSettings>(() => ({
    showGrid: prefs.showGrid,
    gridSize: prefs.gridSize,
    snapToGrid: prefs.snapToGrid,
    snapToObjects: prefs.snapToObjects,
    showRulers: prefs.showRulers,
    showGuides: prefs.showGuides,
  }));
  // Stage toggles and the loop button are remembered as preferences.
  const setPrefs = useCallback((p: Prefs) => {
    setPrefsState(p);
    savePrefs(p);
  }, []);
  useEffect(() => {
    setPrefsState((p) => {
      const next = { ...p, ...settings, loopPreview: loop };
      savePrefs(next);
      return next;
    });
  }, [settings, loop]);
  const setLoop = setLoopState;
  const [showPrefs, setShowPrefs] = useState(false);
  const [showShortcuts, setShowShortcuts] = useState(false);
  const [recovery, setRecovery] = useState<AutosaveEntry | null>(null);
  const [recent, setRecent] = useState<RecentFile[]>([]);
  const [view, setView] = useState<View | null>(null);
  const effective = useRef<{ view: View; w: number; h: number } | null>(null);
  const [zoom, setZoom] = useState(1);
  const [cursor, setCursor] = useState<Pt | null>(null);
  const [filePath, setFilePath] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const [renderMs, setRenderMs] = useState(0);
  const [pendingDiscard, setPendingDiscard] = useState<PendingDiscard>(null);

  const layers = useMemo<LayerNode[]>(() => JSON.parse(engine.layersJson()), [engine, version]);
  const library = useMemo<LibraryItem[]>(() => JSON.parse(engine.libraryJson()), [engine, version]);
  const crumbs = useMemo<Crumb[]>(() => JSON.parse(engine.breadcrumbJson()), [engine, version]);
  const [convert, setConvert] = useState<{ name: string; kind: SymbolKind } | null>(null);
  const history = useMemo<HistoryState>(() => JSON.parse(engine.historyJson()), [engine, version]);
  const stage = useMemo<StageInfo>(() => JSON.parse(engine.stageJson()), [engine, version]);
  const fonts = useMemo<FontAsset[]>(() => JSON.parse(engine.fontsJson()), [engine, version]);

  const changed = useCallback(() => setVersion((v) => v + 1), []);
  const timelineLength = useMemo(() => engine.timelineLength(), [engine, version]);

  /** Moves the playhead (the engine edits/queries at this frame). */
  const goTo = useCallback(
    (f: number) => {
      const clamped = Math.max(0, Math.floor(f));
      engine.setFrame(clamped);
      setFrameState(clamped);
    },
    [engine],
  );

  // Playback at the stage fps. On the main timeline it runs the core's
  // runtime through a `Runtime` session (scripts, movie-clip clocks,
  // buttons, keys, audio: exactly what the exported player does); inside a
  // symbol it just steps that symbol's timeline.
  const [runtime, setRuntime] = useState<Runtime | null>(null);
  const [runtimeTick, setRuntimeTick] = useState(0);
  const [output, setOutput] = useState<OutputLine[]>([]);
  const [showOutput, setShowOutput] = useState(false);
  const addOutput = useCallback((line: OutputLine) => {
    setOutput((o) => [...o.slice(-499), line]);
    if (line.kind === "error") setShowOutput(true);
  }, []);
  useEffect(() => {
    if (!playing) return;
    const length = engine.timelineLength();
    const start = frame >= length - 1 && !loop ? 0 : frame;
    if (engine.editDepth() === 0) {
      goTo(start);
      let session: Runtime | null = null;
      let cancelled = false;
      // Dev builds: tests set window.zoetropeTestMode to step the preview by hand.
      const testMode = import.meta.env.DEV && window.zoetropeTestMode === true;
      Runtime.start(engine, {
        loop,
        manual: testMode,
        seed: testMode ? TEST_SEED : undefined,
        onFrame: (f) => {
          setFrameState(f);
          setRuntimeTick((t) => t + 1);
        },
        onOutput: addOutput,
        onEnd: () => setPlaying(false),
      }).then(
        (rt) => {
          if (cancelled) rt.stop();
          else {
            session = rt;
            setRuntime(rt);
            if (testMode) exposeTestHook(rt, engine);
          }
        },
        (e) => {
          setMessage({ text: `Preview failed: ${errorText(e)}`, error: true });
          setPlaying(false);
        },
      );
      return () => {
        cancelled = true;
        if (session) goTo(session.stop());
        setRuntime(null);
      };
    }
    const fps = stage.fps;
    const t0 = performance.now();
    let raf = 0;
    const tick = (now: number) => {
      // rAF timestamps can precede t0 (they mark the frame's start): clamp.
      let due = Math.max(0, Math.floor(((now - t0) * fps) / 1000));
      const atEnd = !loop && start + due >= length - 1;
      if (atEnd) due = length - 1 - start;
      goTo((start + due) % length);
      if (atEnd) return setPlaying(false);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing]); // eslint-disable-line react-hooks/exhaustive-deps

  /** After entering/leaving symbol editing: reset per-timeline UI state. */
  const afterLevelChange = useCallback(() => {
    setPlaying(false);
    setSelection([]);
    setFrameFocus(null);
    setActiveLayer(null);
    setFrameState(engine.frame());
    setVersion((v) => v + 1);
  }, [engine]);

  const enterInstance = useCallback(
    (id: number) => {
      try {
        engine.enterInstance(id);
        afterLevelChange();
      } catch (e) {
        setMessage({ text: errorText(e), error: true });
      }
    },
    [engine, afterLevelChange],
  );
  const enterSymbol = useCallback(
    (symbol: number) => {
      try {
        engine.enterSymbol(symbol);
        afterLevelChange();
      } catch (e) {
        setMessage({ text: errorText(e), error: true });
      }
    },
    [engine, afterLevelChange],
  );
  const exitTo = useCallback(
    (depth: number) => {
      engine.exitTo(depth);
      afterLevelChange();
    },
    [engine, afterLevelChange],
  );

  // An undo can delete the instance being edited: fall back out of it.
  useEffect(() => {
    if (engine.repairEditStack()) afterLevelChange();
  }, [engine, version, afterLevelChange]);

  // Keep selection and the active layer valid after any change (undo, locks…).
  useEffect(() => {
    setSelection((sel) => {
      if (!sel.length) return sel;
      const valid: number[] = JSON.parse(engine.validSelection(JSON.stringify(sel)));
      return valid.length === sel.length ? sel : valid;
    });
    setActiveLayer((l) => (l !== null && containsLayer(layers, l) ? l : firstContentLayer(layers)));
  }, [engine, layers, version, frame]);

  const selectedLayers = useMemo(
    () => new Set<number>(selection.map((id) => JSON.parse(engine.elementJson(id))?.layer).filter((l) => l !== undefined)),
    [engine, selection, version],
  );

  /** Runs a core command; errors surface in the status bar instead of throwing. */
  const run = useCallback(
    (fn: () => unknown) => {
      try {
        fn();
        setMessage(null);
      } catch (e) {
        setMessage({ text: errorText(e), error: true });
      }
      changed();
    },
    [changed],
  );

  const guardUnsaved = useCallback(
    (label: string, action: () => void) => {
      if (history.dirty) setPendingDiscard({ label, run: action });
      else action();
    },
    [history.dirty],
  );

  const resetDocState = () => {
    setPlaying(false);
    goTo(0);
    setFrameFocus(null);
    setFilePath(null);
    setSelection([]);
    setActiveLayer(null);
    setView(null);
  };

  /** Saves; resolves to whether the project was written. */
  const save = useCallback(
    async (saveAs: boolean): Promise<boolean> => {
      try {
        const path = await saveProject(engine.saveJson(), saveAs ? null : filePath);
        if (path === null) return false;
        engine.markSaved();
        setFilePath(path);
        setMessage({ text: `Saved ${path}` });
        void clearAutosave().catch(() => {});
        void recentFiles().then(setRecent, () => {});
        changed();
        return true;
      } catch (e) {
        setMessage({ text: `Save failed: ${errorText(e)}`, error: true });
        return false;
      }
    },
    [engine, filePath, changed],
  );

  const loadOpened = useCallback(
    (file: { path: string; contents: string }) => {
      engine.loadJson(file.contents); // throws (leaving the current doc intact) if invalid
      resetDocState();
      setFilePath(file.path);
      setMessage({ text: `Opened ${file.path}` });
      void clearAutosave().catch(() => {});
      void recentFiles().then(setRecent, () => {});
      changed();
    },
    [engine, changed], // eslint-disable-line react-hooks/exhaustive-deps
  );

  const open = useCallback(async () => {
    try {
      const file = await openProject();
      if (file) loadOpened(file);
    } catch (e) {
      setMessage({ text: `Open failed: ${errorText(e)}`, error: true });
    }
  }, [loadOpened]);

  const openRecentFile = useCallback(
    async (path: string) => {
      try {
        loadOpened(await openRecent(path));
      } catch (e) {
        setMessage({ text: `Open failed: ${errorText(e)}`, error: true });
      }
    },
    [loadOpened],
  );

  // ---------------------------------------------------------------- autosave & recovery
  // Unsaved work is written every few seconds (native: app data folder;
  // browser: IndexedDB) and cleared on save/open/new. Whatever is still
  // there at startup came from a session that ended without saving: offer it.
  const live = useRef({ version, filePath, playing });
  live.current = { version, filePath, playing };
  const autosavedVersion = useRef(-1);
  useEffect(() => {
    void readAutosave().then((a) => a && setRecovery(a), () => {});
    void recentFiles().then(setRecent, () => {});
  }, []);
  const autosaveNow = useCallback(async () => {
    const { version: v, filePath: path } = live.current;
    if (v === autosavedVersion.current || !JSON.parse(engine.historyJson()).dirty) return;
    try {
      await writeAutosave({ contents: engine.saveJson(), meta: { path, savedAt: Date.now() } });
      autosavedVersion.current = v;
    } catch (e) {
      console.warn("autosave failed", e);
    }
  }, [engine]);
  useEffect(() => {
    if (!prefs.autosave) return;
    const id = setInterval(() => void autosaveNow(), prefs.autosaveSeconds * 1000);
    const hidden = () => document.visibilityState === "hidden" && void autosaveNow();
    document.addEventListener("visibilitychange", hidden);
    return () => {
      clearInterval(id);
      document.removeEventListener("visibilitychange", hidden);
    };
  }, [prefs.autosave, prefs.autosaveSeconds, autosaveNow]);

  const recover = useCallback(
    (entry: AutosaveEntry) => {
      try {
        engine.loadJson(entry.contents);
        resetDocState();
        setFilePath(entry.meta.path);
        engine.markUnsaved();
        setMessage({ text: "Recovered unsaved work. Save it to keep it." });
        changed();
      } catch (e) {
        setMessage({ text: `Could not recover: ${errorText(e)}`, error: true });
      }
      setRecovery(null);
    },
    [engine, changed], // eslint-disable-line react-hooks/exhaustive-deps
  );

  const [exporting, setExporting] = useState(false);
  const exportTitle = (filePath?.split(/[\\/]/).pop() ?? "Untitled.zoe").replace(/\.[^.]*$/, "");

  const newDemo = useCallback((kind: "blank" | "animation" | "game" | "stress") => {
    engine.newDemo(kind);
    void clearAutosave().catch(() => {});
    resetDocState();
    setMessage(null);
    changed();
  }, [engine, changed]);

  const placeImages = useCallback(
    (files: PickedBinary[], at: Pt | null) => {
      if (!files.length) return;
      if (activeLayer === null) return setMessage({ text: "Select a layer to import into", error: true });
      const ids: number[] = [];
      const errors: string[] = [];
      const origin = at ?? { x: stage.width / 2, y: stage.height / 2 };
      files.forEach((f, i) => {
        try {
          ids.push(engine.importImage(activeLayer, f.name, f.bytes, origin.x + i * 20, origin.y + i * 20));
        } catch (e) {
          errors.push(`${f.name}: ${errorText(e)}`);
        }
      });
      if (ids.length) {
        setSelection(ids);
        setTool("select");
      }
      setMessage(errors.length ? { text: errors.join("; "), error: true } : { text: `Imported ${ids.length} image(s)` });
      changed();
    },
    [engine, activeLayer, stage, changed],
  );

  // ---------------------------------------------------------------- clipboard
  // The browser's copy/cut/paste events (⌘C/⌘X/⌘V, or the Edit menu) work
  // the same in browsers and the desktop webview, with no extra permission.
  // Copies are self-contained JSON snippets (zoetrope_core::clipboard), so
  // they paste into other projects and other windows too.
  const clip = useRef({ text: "", count: 0, inPlaceNext: false });
  /** Pastes clipboard text (a Zoetrope snippet, or plain text as a text object). */
  const pasteText = useRef<(text: string, inPlace: boolean) => void>(() => {});
  const clipState = useRef({ selection, activeLayer, playing, stage, placeImages });
  clipState.current = { selection, activeLayer, playing, stage, placeImages };
  useEffect(() => {
    const typing = (t: EventTarget | null) =>
      t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || (t instanceof HTMLElement && t.isContentEditable);
    const copy = (e: ClipboardEvent, cut: boolean) => {
      const s = clipState.current;
      if (typing(e.target) || s.playing || !s.selection.length) return;
      try {
        const json = engine.copyJson(JSON.stringify(s.selection));
        e.clipboardData?.setData("text/plain", json);
        e.preventDefault();
        clip.current = { text: json, count: 0, inPlaceNext: false };
        const n = s.selection.length;
        if (cut) {
          engine.deleteElements(JSON.stringify(s.selection));
          setSelection([]);
          changed();
        }
        setMessage({ text: `${cut ? "Cut" : "Copied"} ${n} object${n === 1 ? "" : "s"}` });
      } catch (err) {
        setMessage({ text: `${cut ? "Cut" : "Copy"} failed: ${errorText(err)}`, error: true });
      }
    };
    const paste = (e: ClipboardEvent) => {
      const s = clipState.current;
      if (typing(e.target) || s.playing || !e.clipboardData) return;
      e.preventDefault();
      const images = Array.from(e.clipboardData.files).filter((f) => /^image\/(png|jpeg|gif)$/.test(f.type));
      if (images.length) {
        void Promise.all(images.map(fileToBinary)).then((bins) => s.placeImages(bins, null));
        return;
      }
      pasteText.current(e.clipboardData.getData("text/plain"), clip.current.inPlaceNext);
      clip.current.inPlaceNext = false;
    };
    pasteText.current = (text: string, inPlace: boolean) => {
      const s = clipState.current;
      if (!text.trim()) return;
      if (s.activeLayer === null) return setMessage({ text: "Select a layer to paste into", error: true });
      // Repeated pastes step down-right; ⇧⌘V pastes in place.
      const c = clip.current;
      c.count = text === c.text ? c.count + 1 : 1;
      c.text = text;
      const offset = inPlace ? 0 : 10 * c.count;
      try {
        const ids = engine.pasteJson(s.activeLayer, text, offset, offset);
        if (ids !== undefined) {
          setSelection(JSON.parse(ids));
          const n = JSON.parse(ids).length;
          setMessage({ text: `Pasted ${n} object${n === 1 ? "" : "s"}` });
        } else {
          setSelection([engine.pasteText(s.activeLayer, text, s.stage.width / 2, s.stage.height / 2)]);
          setMessage({ text: "Pasted text" });
        }
        setTool("select");
      } catch (err) {
        setMessage({ text: `Paste failed: ${errorText(err)}`, error: true });
      }
      changed();
    };
    const onCopy = (e: ClipboardEvent) => copy(e, false);
    const onCut = (e: ClipboardEvent) => copy(e, true);
    document.addEventListener("copy", onCopy);
    document.addEventListener("cut", onCut);
    document.addEventListener("paste", paste);
    return () => {
      document.removeEventListener("copy", onCopy);
      document.removeEventListener("cut", onCut);
      document.removeEventListener("paste", paste);
    };
  }, [engine, changed]);

  const importDialog = useCallback(async () => {
    try {
      placeImages(await importImages(), null);
    } catch (e) {
      setMessage({ text: `Import failed: ${errorText(e)}`, error: true });
    }
  }, [placeImages]);

  const importFontDialog = useCallback(async () => {
    try {
      const r = await importFonts(engine);
      if (r.ids.length) setTextStyle((s) => ({ ...s, font: r.ids[r.ids.length - 1] }));
      if (r.ids.length || r.errors.length) setMessage(describeImport("font", r));
    } catch (e) {
      setMessage({ text: `Import failed: ${errorText(e)}`, error: true });
    }
    changed();
  }, [engine, changed]);

  const onDropFiles = useCallback(
    async (files: File[], at: Pt) => placeImages(await Promise.all(files.map(fileToBinary)), at),
    [placeImages],
  );

  // Anchor selection belongs to one shape; reset it when the selection changes.
  // Selecting objects also switches the properties panel back from frame mode.
  useEffect(() => {
    setAnchors([]);
    if (selection.length) setFrameFocus(null);
  }, [selection]);

  const frameOp = useCallback(
    (op: FrameOp) => {
      if (activeLayer === null) return setMessage({ text: "Select a layer first", error: true });
      const ls = JSON.stringify([activeLayer]);
      setPlaying(false);
      run(() => {
        switch (op) {
          case "frame":
            return engine.insertFrames(ls, frame, 1);
          case "removeFrame":
            return engine.removeFrames(ls, frame, 1);
          case "key":
            return engine.insertKeyframe(ls, frame, false);
          case "blank":
            return engine.insertKeyframe(ls, frame, true);
          case "clear":
            return engine.clearKeyframe(ls, frame);
          case "noTween":
            return engine.setTween(ls, frame, "null");
          case "motion":
          case "shape":
            return engine.setTween(ls, frame, JSON.stringify({ kind: op, easing: { type: "linear" }, rotate: 0 }));
        }
      });
      setFrameFocus({ layer: activeLayer, frame });
    },
    [engine, activeLayer, frame, run],
  );

  const findLayerNode = (nodes: LayerNode[], id: number): LayerNode | null => {
    for (const n of nodes) {
      if (n.id === id) return n;
      const c = findLayerNode(n.children, id);
      if (c) return c;
    }
    return null;
  };
  const frameTarget = frameFocus && findLayerNode(layers, frameFocus.layer);

  const onPicked = useCallback((picked: PickedStyle) => {
    // Like Flash: picking a fill loads the bucket; picking a stroke loads the stroke style.
    if (picked.part === "fill" && picked.fill) {
      setShapeStyle((s) => ({ ...s, fill: picked.fill }));
      setTool("bucket");
      setMessage({ text: "Picked fill — click shapes to apply it" });
    } else if (picked.stroke) {
      setShapeStyle((s) => ({ ...s, stroke: picked.stroke }));
      setMessage({ text: "Picked stroke" });
    }
  }, []);

  const sel = JSON.stringify(selection);
  const hasSel = selection.length > 0;
  const undo = () => run(() => engine.undo());
  const redo = () => run(() => engine.redo());

  const zoomBy = (factor: number) => {
    const eff = effective.current;
    if (!eff) return;
    setView(zoomAt(eff.view, { x: eff.w / 2, y: eff.h / 2 }, eff.view.zoom * factor));
  };
  const zoomTo = (z: number) => {
    const eff = effective.current;
    if (!eff) return;
    const zz = clampZoom(z);
    setView({ zoom: zz, panX: eff.w / 2 - (stage.width * zz) / 2, panY: eff.h / 2 - (stage.height * zz) / 2 });
  };

  // ---------------------------------------------------------------- actions
  // Every command reachable from the keyboard or the native menu, by menu id
  // (src-tauri/src/menu.rs). Keyboard shortcuts and menu clicks run the same
  // code; see MENU_KEYS for how double delivery is avoided.
  const textFocused = () => {
    const a = document.activeElement;
    return a instanceof HTMLInputElement || a instanceof HTMLTextAreaElement || a instanceof HTMLSelectElement;
  };
  const arrange = (op: "front" | "forward" | "backward" | "back") => hasSel && run(() => engine.arrange(sel, op));
  const actions: Record<string, () => void> = {
    new: () => guardUnsaved("New", () => newDemo("blank")),
    "new-animation": () => guardUnsaved("New", () => newDemo("animation")),
    "new-game": () => guardUnsaved("New", () => newDemo("game")),
    open: () => guardUnsaved("Open", open),
    save: () => void save(false),
    "save-as": () => void save(true),
    import: () => void importDialog(),
    export: () => setExporting(true),
    close: () => requestClose("close"),
    quit: () => requestClose("quit"),
    preferences: () => setShowPrefs(true),
    // In a text field, these act on the text (a menu click there must not
    // undo the drawing or select every object).
    undo: () => (textFocused() ? document.execCommand("undo") : undo()),
    redo: () => (textFocused() ? document.execCommand("redo") : redo()),
    "select-all": () => (textFocused() ? document.execCommand("selectAll") : setSelection(JSON.parse(engine.selectAll()))),
    deselect: () => setSelection([]),
    "paste-in-place": () => {
      if (clip.current.text && !playing) pasteText.current(clip.current.text, true);
    },
    duplicate: () => hasSel && run(() => setSelection(JSON.parse(engine.duplicateElements(sel, 10, 10)))),
    delete: () => {
      if (!hasSel) return;
      run(() => engine.deleteElements(sel));
      setSelection([]);
    },
    "zoom-in": () => zoomBy(1.25),
    "zoom-out": () => zoomBy(0.8),
    "zoom-fit": () => setView(null),
    "zoom-100": () => zoomTo(1),
    "toggle-grid": () => setSettings((s) => ({ ...s, showGrid: !s.showGrid })),
    "toggle-snap-grid": () => setSettings((s) => ({ ...s, snapToGrid: !s.snapToGrid })),
    "toggle-rulers": () => setSettings((s) => ({ ...s, showRulers: !s.showRulers })),
    "toggle-guides": () => setSettings((s) => ({ ...s, showGuides: !s.showGuides })),
    "toggle-output": () => setShowOutput((v) => !v),
    "convert-to-symbol": () => {
      if (hasSel) setConvert({ name: `Symbol ${library.length + 1}`, kind: "movieClip" });
      else setMessage({ text: "Select objects to convert to a symbol", error: true });
    },
    "edit-symbol": () => selection.length === 1 && enterInstance(selection[0]),
    "convert-to-path": () => hasSel && run(() => engine.convertToPath(sel)),
    "bring-to-front": () => arrange("front"),
    "bring-forward": () => arrange("forward"),
    "send-backward": () => arrange("backward"),
    "send-to-back": () => arrange("back"),
    play: () => setPlaying((p) => !p),
    "first-frame": () => (setPlaying(false), goTo(0)),
    "prev-frame": () => (setPlaying(false), goTo(frame - 1)),
    "next-frame": () => (setPlaying(false), goTo(frame + 1)),
    "last-frame": () => (setPlaying(false), goTo(timelineLength - 1)),
    "insert-frame": () => frameOp("frame"),
    "insert-keyframe": () => frameOp("key"),
    "insert-blank-keyframe": () => frameOp("blank"),
    "help-manual": () => void openUrl(`${SITE}manual/`),
    "help-shortcuts": () => setShowShortcuts(true),
    "help-scripting": () => void openUrl(`${SITE}manual/scripting.html`),
    "help-issue": () => void openUrl(`${REPO}/issues/new`),
  };
  const actionsRef = useRef(actions);
  actionsRef.current = actions;

  // Keyboard shortcuts.
  const keys = useRef<(e: KeyboardEvent) => void>(() => {});
  keys.current = (e: KeyboardEvent) => {
    const t = e.target;
    if (t instanceof HTMLInputElement || t instanceof HTMLSelectElement || t instanceof HTMLTextAreaElement) return;
    // While previewing, the keyboard belongs to the movie; Esc stops it.
    if (playing && engine.editDepth() === 0) {
      if (e.key === "Escape") {
        setPlaying(false);
        e.preventDefault();
      }
      return;
    }
    const mod = e.metaKey || e.ctrlKey;
    const k = e.key.toLowerCase();
    const step = e.shiftKey ? 10 : 1;
    const id = shortcutAction(e);
    let handled = true;
    if (id && (id !== "close" && id !== "quit" ? true : isTauri)) actions[id]();
    else if (e.key === "?" && !mod) setShowShortcuts(true);
    else if (!mod && e.key === "Escape" && !hasSel && crumbs.length > 1) exitTo(crumbs.length - 2);
    else if (e.key === "F5" && e.shiftKey) frameOp("removeFrame");
    else if (e.key === "F6" && e.shiftKey) frameOp("clear");
    else if (!mod && e.key === "Enter") actions.play();
    else if (!mod && e.key === ",") actions["prev-frame"]();
    else if (!mod && e.key === ".") actions["next-frame"]();
    else if (!mod && e.key === "Home") actions["first-frame"]();
    else if (!mod && e.key === "End") actions["last-frame"]();
    else if (mod && k === "v") {
      // The paste itself arrives as a "paste" event.
      clip.current.inPlaceNext = false;
      handled = false;
    } else if (mod) handled = false;
    else if (e.key === "Escape") setSelection([]);
    else if ((e.key === "Delete" || e.key === "Backspace") && tool === "subselect" && anchors.length && selection.length === 1) {
      run(() => engine.deleteAnchors(selection[0], JSON.stringify(anchors)));
      setAnchors([]);
    } else if (e.key === "Delete" || e.key === "Backspace") actions.delete();
    else if (e.key === "ArrowLeft" && hasSel) run(() => engine.translateElements(sel, -step, 0));
    else if (e.key === "ArrowRight" && hasSel) run(() => engine.translateElements(sel, step, 0));
    else if (e.key === "ArrowUp" && hasSel) run(() => engine.translateElements(sel, 0, -step));
    else if (e.key === "ArrowDown" && hasSel) run(() => engine.translateElements(sel, 0, step));
    else if (TOOL_KEYS[k] && !e.repeat) setTool(TOOL_KEYS[k]);
    else handled = false;
    if (handled) e.preventDefault();
  };

  // Native menu: run the action, unless the same shortcut was just pressed
  // in the webview (which then already handled it, natively or above).
  useEffect(() => {
    let off = () => {};
    void onMenu((id) => {
      const recent = performance.now() - lastShortcut.t < 400 && lastShortcut.id === id;
      if (!recent) actionsRef.current[id]?.();
    }).then((u) => (off = u));
    return () => off();
  }, []);

  // ---------------------------------------------------------------- closing & opening from the OS
  const [closeAsk, setCloseAsk] = useState<"close" | "quit" | null>(null);
  const finishClose = useCallback((kind: "close" | "quit") => {
    closing.current = true;
    void (kind === "quit" ? exitApp() : closeWindow());
  }, []);
  const closing = useRef(false);
  const dirtyRef = useRef(history.dirty);
  dirtyRef.current = history.dirty;
  function requestClose(kind: "close" | "quit") {
    if (dirtyRef.current) setCloseAsk(kind);
    else finishClose(kind);
  }
  useEffect(() => {
    let off = () => {};
    void onCloseRequested(() => {
      if (closing.current || !dirtyRef.current) return true;
      setCloseAsk("close");
      return false;
    }).then((u) => (off = u));
    return () => off();
  }, []);
  const openPending = useCallback(async () => {
    try {
      const file = await takePendingOpen();
      if (file) loadOpened(file);
    } catch (e) {
      setMessage({ text: `Open failed: ${errorText(e)}`, error: true });
    }
  }, [loadOpened]);
  const openPendingRef = useRef(openPending);
  openPendingRef.current = openPending;
  const guardRef = useRef(guardUnsaved);
  guardRef.current = guardUnsaved;
  useEffect(() => {
    void openPendingRef.current(); // a file double-clicked to launch the app
    let off = () => {};
    void onOpenFileRequest(() => guardRef.current("Open", () => void openPendingRef.current())).then((u) => (off = u));
    return () => off();
  }, []);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => keys.current(e);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const fileName = filePath?.split(/[\\/]/).pop() ?? "Untitled";
  const toggle = (key: keyof StageSettings, label: string, title: string) => (
    <button className={settings[key] ? "on" : ""} title={title} onClick={() => setSettings((s) => ({ ...s, [key]: !s[key] }))}>
      {label}
    </button>
  );

  return (
    <div className="app">
      <header className="toolbar">
        <span className="brand">Zoetrope</span>
        <div className="group">
          <select
            className="menu"
            value=""
            title="New blank project, or start from a demo"
            onChange={(e) => {
              const kind = e.target.value as "blank" | "animation" | "game" | "stress";
              if (kind) guardUnsaved("New", () => newDemo(kind));
            }}
          >
            <option value="" disabled>
              New…
            </option>
            <option value="blank">Blank project</option>
            <option value="animation">Animation demo</option>
            <option value="game">Game demo (scripted)</option>
            <option value="stress">Stress test (1000 flowers)</option>
          </select>
          <button onClick={() => guardUnsaved("Open", open)} title="⌘O">Open…</button>
          <button onClick={() => save(false)} title="⌘S">Save</button>
          <button onClick={() => save(true)} title="⇧⌘S">Save as…</button>
          <button onClick={importDialog} title="⌘I — or drop images on the stage">Import…</button>
          <button onClick={() => setExporting(true)} title="Publish as a web page that plays offline">Export…</button>
          {recent.length > 0 && (
            <select
              className="menu"
              value=""
              title="Open a recent project"
              onChange={(e) => {
                const path = e.target.value;
                if (path) guardUnsaved("Open", () => void openRecentFile(path));
              }}
            >
              <option value="" disabled>
                Recent…
              </option>
              {recent.map((r) => (
                <option key={r.path} value={r.path} title={r.path}>
                  {r.name}
                </option>
              ))}
            </select>
          )}
          <button onClick={() => setShowPrefs(true)} title="Preferences">
            ⚙
          </button>
          <button onClick={() => setShowShortcuts(true)} title="Keyboard shortcuts (?)">
            ?
          </button>
        </div>
        <div className="group">
          <button onClick={undo} disabled={!history.canUndo} title="⌘Z">
            Undo{history.undoLabel ? ` ${history.undoLabel}` : ""}
          </button>
          <button onClick={redo} disabled={!history.canRedo} title="⇧⌘Z">
            Redo{history.redoLabel ? ` ${history.redoLabel}` : ""}
          </button>
        </div>
        <div className="group">
          {toggle("showGrid", "Grid", "Show grid (⌘')")}
          {toggle("snapToGrid", "Snap grid", "Snap to grid (⇧⌘')")}
          {toggle("snapToObjects", "Snap objects", "Snap to objects and the stage")}
          {toggle("showRulers", "Rulers", "Show rulers")}
          {toggle("showGuides", "Guides", "Show guide layers (never exported)")}
          <label className="grid-size" title="Grid size">
            <input
              type="number"
              min={2}
              max={500}
              value={settings.gridSize}
              onChange={(e) => {
                const v = Number(e.target.value);
                if (v >= 2) setSettings((s) => ({ ...s, gridSize: v }));
              }}
            />
            px
          </label>
        </div>
        <ToolOptionsBar
          tool={tool}
          options={toolOptions}
          onOptions={setToolOptions}
          text={{ style: textStyle, onStyle: setTextStyle, fonts, onImportFont: importFontDialog }}
        />
        <div className="group">
          <button onClick={() => zoomBy(0.8)} title="⌘−">−</button>
          <button className="zoom" onClick={() => zoomTo(1)} title="⌘1: 100%">
            {Math.round(zoom * 100)}%
          </button>
          <button onClick={() => zoomBy(1.25)} title="⌘=">＋</button>
          <button onClick={() => setView(null)} title="⌘0">Fit</button>
        </div>
      </header>

      {showPrefs && (
        <PrefsDialog
          prefs={prefs}
          onClose={() => setShowPrefs(false)}
          onSave={(p) => {
            setPrefs(p);
            setSettings((s) => ({ ...s, showGrid: p.showGrid, gridSize: p.gridSize, snapToGrid: p.snapToGrid, snapToObjects: p.snapToObjects, showRulers: p.showRulers, showGuides: p.showGuides }));
            setLoop(p.loopPreview);
          }}
        />
      )}
      {showShortcuts && <ShortcutsDialog onClose={() => setShowShortcuts(false)} />}
      {closeAsk && (
        <div className="modal-backdrop">
          <div className="modal">
            <h3>Save changes to “{exportTitle}” before {closeAsk === "quit" ? "quitting" : "closing"}?</h3>
            <p className="hint flush">If you don't save, your changes since the last save will be lost.</p>
            <div className="btn-row">
              <button
                onClick={() => {
                  void clearAutosave().catch(() => {});
                  const kind = closeAsk;
                  setCloseAsk(null);
                  finishClose(kind);
                }}
              >
                Don't Save
              </button>
              <button onClick={() => setCloseAsk(null)}>Cancel</button>
              <button
                autoFocus
                onClick={async () => {
                  const kind = closeAsk;
                  if (await save(false)) {
                    setCloseAsk(null);
                    finishClose(kind);
                  }
                }}
              >
                Save
              </button>
            </div>
          </div>
        </div>
      )}
      {recovery && (
        <div className="modal-backdrop">
          <div className="modal recovery">
            <h3>Recover unsaved work?</h3>
            <p className="hint flush">The last session ended with changes that were never saved.</p>
            <dl>
              <dt>Project</dt>
              <dd>{recovery.meta.path ?? "Untitled (never saved)"}</dd>
              <dt>Autosaved</dt>
              <dd>{new Date(recovery.meta.savedAt).toLocaleString()}</dd>
            </dl>
            <div className="btn-row">
              <button
                onClick={() => {
                  void clearAutosave().catch(() => {});
                  setRecovery(null);
                }}
              >
                Discard
              </button>
              <button autoFocus onClick={() => recover(recovery)}>
                Recover
              </button>
            </div>
          </div>
        </div>
      )}
      {exporting && (
        <ExportDialog
          engine={engine}
          fallbackTitle={exportTitle}
          onClose={() => setExporting(false)}
          onSettings={(p) => run(() => engine.setPublish(JSON.stringify(p)))}
          onMessage={setMessage}
        />
      )}
      {convert && (
        <div className="modal-backdrop" onClick={() => setConvert(null)}>
          <form
            className="modal"
            onClick={(e) => e.stopPropagation()}
            onSubmit={(e) => {
              e.preventDefault();
              const c = convert;
              setConvert(null);
              run(() => setSelection([engine.convertToSymbol(JSON.stringify(selection), c.name, c.kind)]));
            }}
          >
            <h3>Convert to symbol</h3>
            <label className="field wide">
              <span className="field-label">Name</span>
              <input autoFocus value={convert.name} onChange={(e) => setConvert({ ...convert, name: e.target.value })} />
            </label>
            <div className="kind-choice">
              {(["movieClip", "graphic", "button"] as const).map((k) => (
                <label key={k} className="check">
                  <input type="radio" checked={convert.kind === k} onChange={() => setConvert({ ...convert, kind: k })} />
                  {SYMBOL_KIND_ICON[k]} {SYMBOL_KIND_LABEL[k]}
                </label>
              ))}
            </div>
            <p className="hint flush">
              {convert.kind === "movieClip" && "Own timeline: plays independently from when it appears."}
              {convert.kind === "graphic" && "Timeline synced to the parent's frames."}
              {convert.kind === "button" && "Up / Over / Down / Hit frames (all four start with this art)."}
            </p>
            <div className="btn-row">
              <button type="button" onClick={() => setConvert(null)}>
                Cancel
              </button>
              <button type="submit" disabled={!convert.name.trim()}>
                Convert
              </button>
            </div>
          </form>
        </div>
      )}

      {pendingDiscard && (
        <div className="banner">
          Unsaved changes will be lost.
          <button
            onClick={() => {
              const action = pendingDiscard.run;
              setPendingDiscard(null);
              action();
            }}
          >
            Discard &amp; {pendingDiscard.label}
          </button>
          <button onClick={() => setPendingDiscard(null)}>Cancel</button>
        </div>
      )}

      <main className="workspace">
        <ToolPalette tool={tool} onTool={setTool} style={shapeStyle} onStyle={setShapeStyle} />
        <div className="stage-col">
          <nav className="breadcrumb">
            {crumbs.map((c, i) => (
              <span key={i}>
                {i > 0 && <span className="crumb-sep">›</span>}
                <button
                  className={`crumb ${i === crumbs.length - 1 ? "current" : ""}`}
                  disabled={i === crumbs.length - 1}
                  onClick={() => exitTo(i)}
                  title={i === 0 ? "Main timeline" : c.kind ? SYMBOL_KIND_LABEL[c.kind] : ""}
                >
                  {i === 0 ? "🎬" : c.kind ? SYMBOL_KIND_ICON[c.kind] : ""} {c.label}
                </button>
              </span>
            ))}
            {crumbs.length > 1 && <span className="crumb-hint">Editing a symbol: Esc goes back up</span>}
          </nav>
        <StageView
          engine={engine}
          version={version}
          stage={stage}
          selection={selection}
          onSelect={setSelection}
          frame={frame}
          onion={onion.enabled && !playing ? onion : null}
          runtime={runtime}
          runtimeTick={runtimeTick}
          onEnterInstance={enterInstance}
          onDropSymbol={(symbol, at) => {
            if (activeLayer === null) return setMessage({ text: "Select a layer first", error: true });
            run(() => setSelection([engine.placeInstance(activeLayer, symbol, at.x, at.y)]));
          }}
          anchors={anchors}
          onAnchors={setAnchors}
          toolOptions={toolOptions}
          onPicked={onPicked}
          tool={tool}
          activeLayer={activeLayer}
          shapeStyle={shapeStyle}
          textStyle={textStyle}
          settings={settings}
          view={view}
          onView={setView}
          onChanged={changed}
          onError={(text) => setMessage({ text, error: true })}
          onRenderTime={setRenderMs}
          onCursor={setCursor}
          onEffectiveView={useCallback((v: View, w: number, h: number) => {
            effective.current = { view: v, w, h };
            setZoom(v.zoom);
          }, [])}
          onDropFiles={onDropFiles}
        />
        </div>
        <aside className="side">
          <div className="side-top">
            <PropertiesPanel
              engine={engine}
              version={version}
              selection={selection}
              stage={stage}
              layers={layers}
              frameTarget={frameTarget ? { layer: frameTarget, frame: frameFocus!.frame } : null}
              library={library}
              onEditInstance={enterInstance}
              onImportFont={importFontDialog}
              onMessage={setMessage}
              run={run}
            />
          </div>
          <div className="side-bottom">
            <LibraryPanel engine={engine} version={version} items={library} run={run} onEdit={enterSymbol} />
          </div>
        </aside>
      </main>

      {showOutput && <OutputPanel lines={output} onClear={() => setOutput([])} onClose={() => setShowOutput(false)} />}

      <Timeline
        layers={layers}
        length={timelineLength}
        frame={frame}
        onFrame={goTo}
        playing={playing}
        onPlaying={setPlaying}
        loop={loop}
        onLoop={setLoop}
        onion={onion}
        onOnion={setOnion}
        activeLayer={activeLayer}
        onActivate={setActiveLayer}
        selectedLayers={selectedLayers}
        focus={frameFocus}
        onFocus={(layer, f) => {
          setFrameFocus({ layer, frame: f });
          setSelection([]);
        }}
        onFrameOp={frameOp}
        onAddLayer={(kind) =>
          run(() => {
            const id = engine.addLayer(activeLayer ?? undefined, kind);
            if (kind !== "folder") setActiveLayer(id);
          })
        }
        onDeleteLayer={(id) => run(() => engine.deleteLayer(id))}
        onPatchLayer={(id, patch) => run(() => engine.setLayerProps(id, JSON.stringify(patch)))}
        onMoveLayer={(id, parent, index) => run(() => engine.moveLayer(id, parent ?? undefined, index))}
      />

      <footer className="status">
        <span>
          {fileName}
          {history.dirty ? " •" : ""}
        </span>
        <span>
          {stage.width}×{stage.height} @ {stage.fps}fps
        </span>
        <span className="coords">{cursor ? `x ${cursor.x.toFixed(1)}  y ${cursor.y.toFixed(1)}` : ""}</span>
        <span>
          frame {frame + 1}/{timelineLength}
        </span>
        <span>{selection.length ? `${selection.length} selected` : ""}</span>
        <span className={message?.error ? "msg error" : "msg"}>{message?.text}</span>
        <button className={`mini output-toggle ${output.some((l) => l.kind === "error") ? "has-errors" : ""}`} onClick={() => setShowOutput((v) => !v)} title="Script output (trace and errors)">
          Output{output.length ? ` (${output.length})` : ""}
        </button>
        <span className="right">
          render {renderMs.toFixed(2)} ms · {isTauri ? "Tauri" : "browser"} · WASM core
        </span>
      </footer>
    </div>
  );
}
