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
} from "./engine";
import { fileToBinary, importImages, isTauri, openProject, saveProject, type PickedBinary } from "./platform";
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
  const [anchors, setAnchors] = useState<NodeRef[]>([]);
  const [frame, setFrameState] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [loop, setLoop] = useState(true);
  const [onion, setOnion] = useState<OnionSettings>({ enabled: false, before: 2, after: 2, alpha: 0.35 });
  const [frameFocus, setFrameFocus] = useState<{ layer: number; frame: number } | null>(null);
  const [settings, setSettings] = useState<StageSettings>({
    showGrid: false,
    gridSize: 20,
    snapToGrid: false,
    snapToObjects: true,
    showRulers: true,
    showGuides: true,
  });
  const [view, setView] = useState<View | null>(null);
  const effective = useRef<{ view: View; w: number; h: number } | null>(null);
  const [zoom, setZoom] = useState(1);
  const [cursor, setCursor] = useState<Pt | null>(null);
  const [filePath, setFilePath] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const [renderMs, setRenderMs] = useState(0);
  const [pendingDiscard, setPendingDiscard] = useState<PendingDiscard>(null);

  const layers = useMemo<LayerNode[]>(() => JSON.parse(engine.layersJson()), [engine, version]);
  const history = useMemo<HistoryState>(() => JSON.parse(engine.historyJson()), [engine, version]);
  const stage = useMemo<StageInfo>(() => JSON.parse(engine.stageJson()), [engine, version]);

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

  // Playback: advance by wall-clock time at the stage fps.
  useEffect(() => {
    if (!playing) return;
    const length = engine.timelineLength();
    const fps = stage.fps;
    const start = frame >= length - 1 && !loop ? 0 : frame;
    const t0 = performance.now();
    let raf = 0;
    const tick = (now: number) => {
      let f = start + Math.floor(((now - t0) * fps) / 1000);
      if (f >= length) {
        if (!loop) {
          goTo(length - 1);
          setPlaying(false);
          return;
        }
        f %= length;
      }
      goTo(f);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing]); // eslint-disable-line react-hooks/exhaustive-deps

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

  const save = useCallback(
    async (saveAs: boolean) => {
      try {
        const path = await saveProject(engine.saveJson(), saveAs ? null : filePath);
        if (path === null) return;
        engine.markSaved();
        setFilePath(path);
        setMessage({ text: `Saved ${path}` });
        changed();
      } catch (e) {
        setMessage({ text: `Save failed: ${errorText(e)}`, error: true });
      }
    },
    [engine, filePath, changed],
  );

  const open = useCallback(async () => {
    try {
      const file = await openProject();
      if (!file) return;
      engine.loadJson(file.contents); // throws (leaving the current doc intact) if invalid
      resetDocState();
      setFilePath(file.path);
      setMessage({ text: `Opened ${file.path}` });
      changed();
    } catch (e) {
      setMessage({ text: `Open failed: ${errorText(e)}`, error: true });
    }
  }, [engine, changed]);

  const newDemo = useCallback(() => {
    engine.newDemo();
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

  const importDialog = useCallback(async () => {
    try {
      placeImages(await importImages(), null);
    } catch (e) {
      setMessage({ text: `Import failed: ${errorText(e)}`, error: true });
    }
  }, [placeImages]);

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

  // Keyboard shortcuts.
  const keys = useRef<(e: KeyboardEvent) => void>(() => {});
  keys.current = (e: KeyboardEvent) => {
    const t = e.target;
    if (t instanceof HTMLInputElement || t instanceof HTMLSelectElement || t instanceof HTMLTextAreaElement) return;
    const mod = e.metaKey || e.ctrlKey;
    const k = e.key.toLowerCase();
    const step = e.shiftKey ? 10 : 1;
    let handled = true;
    if (e.key === "F5") frameOp(e.shiftKey ? "removeFrame" : "frame");
    else if (e.key === "F6") frameOp(e.shiftKey ? "clear" : "key");
    else if (e.key === "F7") frameOp("blank");
    else if (!mod && e.key === "Enter") setPlaying((p) => !p);
    else if (!mod && e.key === ",") (setPlaying(false), goTo(frame - 1));
    else if (!mod && e.key === ".") (setPlaying(false), goTo(frame + 1));
    else if (!mod && e.key === "Home") (setPlaying(false), goTo(0));
    else if (!mod && e.key === "End") (setPlaying(false), goTo(timelineLength - 1));
    else if (mod && k === "z" && !e.shiftKey) undo();
    else if (mod && ((k === "z" && e.shiftKey) || k === "y")) redo();
    else if (mod && k === "s") save(e.shiftKey);
    else if (mod && k === "o") guardUnsaved("Open", open);
    else if (mod && k === "i") importDialog();
    else if (mod && k === "a") setSelection(JSON.parse(engine.selectAll()));
    else if (mod && k === "b" && hasSel) run(() => engine.convertToPath(sel));
    else if (mod && k === "d" && hasSel) run(() => setSelection(JSON.parse(engine.duplicateElements(sel, 10, 10))));
    else if (mod && (e.key === "]" || e.key === "}") && hasSel) run(() => engine.arrange(sel, e.shiftKey ? "front" : "forward"));
    else if (mod && (e.key === "[" || e.key === "{") && hasSel) run(() => engine.arrange(sel, e.shiftKey ? "back" : "backward"));
    else if (mod && (e.key === "=" || e.key === "+")) zoomBy(1.25);
    else if (mod && e.key === "-") zoomBy(0.8);
    else if (mod && e.key === "0") setView(null);
    else if (mod && e.key === "1") zoomTo(1);
    else if (mod && e.key === "'") setSettings((s) => ({ ...s, [e.shiftKey ? "snapToGrid" : "showGrid"]: !s[e.shiftKey ? "snapToGrid" : "showGrid"] }));
    else if (mod) handled = false;
    else if (e.key === "Escape") setSelection([]);
    else if ((e.key === "Delete" || e.key === "Backspace") && tool === "subselect" && anchors.length && selection.length === 1) {
      run(() => engine.deleteAnchors(selection[0], JSON.stringify(anchors)));
      setAnchors([]);
    } else if ((e.key === "Delete" || e.key === "Backspace") && hasSel) {
      run(() => engine.deleteElements(sel));
      setSelection([]);
    } else if (e.key === "ArrowLeft" && hasSel) run(() => engine.translateElements(sel, -step, 0));
    else if (e.key === "ArrowRight" && hasSel) run(() => engine.translateElements(sel, step, 0));
    else if (e.key === "ArrowUp" && hasSel) run(() => engine.translateElements(sel, 0, -step));
    else if (e.key === "ArrowDown" && hasSel) run(() => engine.translateElements(sel, 0, step));
    else if (TOOL_KEYS[k] && !e.repeat) setTool(TOOL_KEYS[k]);
    else handled = false;
    if (handled) e.preventDefault();
  };
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
          <button onClick={() => guardUnsaved("New", newDemo)}>New demo</button>
          <button onClick={() => guardUnsaved("Open", open)} title="⌘O">Open…</button>
          <button onClick={() => save(false)} title="⌘S">Save</button>
          <button onClick={() => save(true)} title="⇧⌘S">Save as…</button>
          <button onClick={importDialog} title="⌘I — or drop images on the stage">Import…</button>
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
        <ToolOptionsBar tool={tool} options={toolOptions} onOptions={setToolOptions} />
        <div className="group">
          <button onClick={() => zoomBy(0.8)} title="⌘−">−</button>
          <button className="zoom" onClick={() => zoomTo(1)} title="⌘1: 100%">
            {Math.round(zoom * 100)}%
          </button>
          <button onClick={() => zoomBy(1.25)} title="⌘=">＋</button>
          <button onClick={() => setView(null)} title="⌘0">Fit</button>
        </div>
      </header>

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
        <StageView
          engine={engine}
          version={version}
          stage={stage}
          selection={selection}
          onSelect={setSelection}
          frame={frame}
          onion={onion.enabled && !playing ? onion : null}
          anchors={anchors}
          onAnchors={setAnchors}
          toolOptions={toolOptions}
          onPicked={onPicked}
          tool={tool}
          activeLayer={activeLayer}
          shapeStyle={shapeStyle}
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
        <aside className="side">
          <PropertiesPanel
            engine={engine}
            version={version}
            selection={selection}
            stage={stage}
            layers={layers}
            frameTarget={frameTarget ? { layer: frameTarget, frame: frameFocus!.frame } : null}
            run={run}
          />
        </aside>
      </main>

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
        <span className="right">
          render {renderMs.toFixed(2)} ms · {isTauri ? "Tauri" : "browser"} · WASM core
        </span>
      </footer>
    </div>
  );
}
