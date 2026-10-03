import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type {
  DragMode,
  Engine,
  Guides,
  Handle,
  Modifiers,
  Pt,
  SelectionGeometry,
  ShapeDrag,
  ShapeTool,
  SnapConfig,
  StageInfo,
} from "../engine";
import {
  dist,
  drawRuler,
  fitCanvas,
  fitView,
  mid,
  pointInPolygon,
  segmentDistance,
  toScreen,
  toStage,
  zoomAt,
  type View,
} from "../view";

export type Tool = "select" | ShapeTool | "hand";

export interface ShapeStyle {
  fill: string | null;
  stroke: string | null;
  strokeWidth: number;
}

export interface StageSettings {
  showGrid: boolean;
  gridSize: number;
  snapToGrid: boolean;
  snapToObjects: boolean;
  showRulers: boolean;
  showGuides: boolean;
}

interface Props {
  engine: Engine;
  /** Bumped by the parent after every committed model change. */
  version: number;
  stage: StageInfo;
  selection: number[];
  onSelect: (ids: number[]) => void;
  tool: Tool;
  activeLayer: number | null;
  shapeStyle: ShapeStyle;
  settings: StageSettings;
  /** null = fit to window. */
  view: View | null;
  onView: (v: View | null) => void;
  onChanged: () => void;
  onError: (msg: string) => void;
  onRenderTime: (ms: number) => void;
  onCursor: (p: Pt | null) => void;
  /** Reports the view actually in use (fit or explicit) and the viewport size. */
  onEffectiveView: (v: View, w: number, h: number) => void;
  onDropFiles: (files: File[], at: Pt) => void;
}

const RULER = 20;
const HANDLE = 7;
const DRAG_THRESHOLD = 3;

type Interaction =
  | { kind: "idle" }
  | { kind: "pending"; screen: Pt; stage: Pt; mode: DragMode; ids: number[] }
  | { kind: "transform"; guides: Guides }
  | { kind: "marquee"; start: Pt; current: Pt; base: number[] }
  | { kind: "shape"; tool: ShapeTool; p0: Pt; p1: Pt; guides: Guides; preview: ShapeDrag | null }
  | { kind: "pan"; screen: Pt; view: View };

const HANDLES: Handle[] = ["nw", "ne", "se", "sw", "n", "e", "s", "w"];

function handlePoints(c: Pt[]): Record<Handle, Pt> {
  return { nw: c[0], ne: c[1], se: c[2], sw: c[3], n: mid(c[0], c[1]), e: mid(c[1], c[2]), s: mid(c[2], c[3]), w: mid(c[3], c[0]) };
}

const ROTATE_CURSOR = `url("data:image/svg+xml,${encodeURIComponent(
  '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 22 22"><path d="M17 11a6 6 0 1 1-2-4.5" fill="none" stroke="white" stroke-width="4"/><path d="M17 11a6 6 0 1 1-2-4.5" fill="none" stroke="black" stroke-width="1.6"/><path d="M13 3l4 3.5-4.8 1.5z" fill="black" stroke="white" stroke-width="0.8"/></svg>',
)}") 11 11, crosshair`;

function resizeCursor(from: Pt, to: Pt): string {
  const a = ((Math.atan2(to.y - from.y, to.x - from.x) * 180) / Math.PI + 180) % 180;
  if (a < 22.5 || a >= 157.5) return "ew-resize";
  if (a < 67.5) return "nwse-resize";
  if (a < 112.5) return "ns-resize";
  return "nesw-resize";
}

/** The canvas stage: renders via the core, draws editing chrome on an overlay. */
export function StageView(props: Props) {
  const { engine, stage, settings, version, selection } = props;
  const viewportRef = useRef<HTMLDivElement>(null);
  const sceneRef = useRef<HTMLCanvasElement>(null);
  const overlayRef = useRef<HTMLCanvasElement>(null);
  const rulerXRef = useRef<HTMLCanvasElement>(null);
  const rulerYRef = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const interaction = useRef<Interaction>({ kind: "idle" });
  const cursorStage = useRef<Pt | null>(null);
  const spaceDown = useRef(false);
  const frame = useRef(0);
  const [cursor, setCursor] = useState("default");

  const view: View = props.view ?? fitView(size.w || 1, size.h || 1, stage.width, stage.height);
  // Event handlers read the latest props/view through refs.
  const live = useRef({ props, view });
  live.current = { props, view };

  const onEffectiveView = props.onEffectiveView;
  useEffect(() => onEffectiveView(view, size.w, size.h), [onEffectiveView, view.zoom, view.panX, view.panY, size.w, size.h]); // eslint-disable-line react-hooks/exhaustive-deps

  const selectionGeometry = useCallback((): SelectionGeometry | null => {
    const sel = live.current.props.selection;
    return sel.length ? JSON.parse(engine.selectionJson(JSON.stringify(sel))) : null;
  }, [engine]);

  const snapConfig = (): SnapConfig => {
    const { settings: s } = live.current.props;
    return { grid: s.snapToGrid ? s.gridSize : null, objects: s.snapToObjects, tolerance: 6 / live.current.view.zoom };
  };

  // ------------------------------------------------------------ drawing

  const draw = useCallback(() => {
    frame.current = 0;
    const { props: p, view: v } = live.current;
    const scene = sceneRef.current;
    const overlay = overlayRef.current;
    const vp = viewportRef.current;
    if (!scene || !overlay || !vp) return;
    const w = vp.clientWidth;
    const h = vp.clientHeight;

    const dpr = fitCanvas(scene, w, h);
    const ctx = scene.getContext("2d")!;
    const t0 = performance.now();
    p.engine.render(ctx, 0, v.zoom * dpr, v.panX * dpr, v.panY * dpr, false, p.settings.showGuides);
    p.onRenderTime(performance.now() - t0);

    fitCanvas(overlay, w, h);
    const o = overlay.getContext("2d")!;
    o.setTransform(dpr, 0, 0, dpr, 0, 0);
    o.clearRect(0, 0, w, h);
    drawOverlay(o, p, v, selectionGeometry(), interaction.current);

    for (const [ref, horizontal] of [
      [rulerXRef, true],
      [rulerYRef, false],
    ] as const) {
      const c = ref.current;
      if (!c) continue;
      const len = horizontal ? w : h;
      const rdpr = fitCanvas(c, horizontal ? len : RULER, horizontal ? RULER : len);
      const rc = c.getContext("2d")!;
      rc.setTransform(rdpr, 0, 0, rdpr, 0, 0);
      drawRuler(rc, horizontal, len, RULER, v, cursorStage.current, {
        bg: "#24242d",
        tick: "#5a5a6a",
        text: "#9a9aab",
        cursor: "#e86a92",
      });
    }
  }, [selectionGeometry]);

  const requestDraw = useCallback(() => {
    if (!frame.current) frame.current = requestAnimationFrame(draw);
  }, [draw]);

  useLayoutEffect(() => {
    const vp = viewportRef.current!;
    const ro = new ResizeObserver(() => setSize({ w: vp.clientWidth, h: vp.clientHeight }));
    ro.observe(vp);
    return () => ro.disconnect();
  }, [settings.showRulers]);

  useEffect(() => {
    requestDraw();
  }, [requestDraw, version, selection, size, props.view, stage, settings, props.tool]);

  useEffect(
    () => () => {
      cancelAnimationFrame(frame.current);
      frame.current = 0;
    },
    [],
  );

  // Decode embedded images whenever the document may have gained some.
  useEffect(() => {
    let alive = true;
    engine.decodeImages().then((n: number) => alive && n > 0 && requestDraw());
    return () => {
      alive = false;
    };
  }, [engine, version, requestDraw]);

  // ------------------------------------------------------------ hit-testing chrome

  const chromeAt = (s: Pt): { mode: DragMode; cursor: string } | null => {
    const geom = selectionGeometry();
    if (!geom) return null;
    const v = live.current.view;
    const c = geom.corners.map((p) => toScreen(v, p));
    const hp = handlePoints(c);
    const center = mid(c[0], c[2]);
    if (geom.hasOwnPivot && dist(s, toScreen(v, geom.pivot)) <= 6) return { mode: { mode: "pivot" }, cursor: "cell" };
    for (const h of HANDLES) {
      if (dist(s, hp[h]) <= HANDLE) return { mode: { mode: "scale", handle: h }, cursor: resizeCursor(center, hp[h]) };
    }
    const inside = pointInPolygon(s, c);
    if (!inside) {
      for (const h of ["nw", "ne", "se", "sw"] as const) {
        const d = dist(s, hp[h]);
        if (d > HANDLE && d <= 22) return { mode: { mode: "rotate" }, cursor: ROTATE_CURSOR };
      }
    }
    const edges: [Handle, Pt, Pt][] = [
      ["n", c[0], c[1]],
      ["e", c[1], c[2]],
      ["s", c[2], c[3]],
      ["w", c[3], c[0]],
    ];
    for (const [h, a, b] of edges) {
      if (segmentDistance(s, a, b) <= 3 && dist(a, b) > 3 * HANDLE) {
        return { mode: { mode: "skew", handle: h }, cursor: h === "n" || h === "s" ? "col-resize" : "row-resize" };
      }
    }
    return null;
  };

  // ------------------------------------------------------------ pointer handling

  const screenPoint = (e: { clientX: number; clientY: number }): Pt => {
    const r = viewportRef.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };

  const mods = (e: { shiftKey: boolean; altKey: boolean }): Modifiers => ({ shift: e.shiftKey, alt: e.altKey });

  const fail = (e: unknown) => live.current.props.onError(e instanceof Error ? e.message : String(e));

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0 && e.button !== 1) return;
    (e.target as Element).setPointerCapture(e.pointerId);
    const { props: p, view: v } = live.current;
    const s = screenPoint(e);
    const pt = toStage(v, s);

    if (p.tool === "hand" || spaceDown.current || e.button === 1) {
      interaction.current = { kind: "pan", screen: s, view: v };
      setCursor("grabbing");
      return;
    }

    if (p.tool !== "select") {
      if (p.activeLayer === null) return fail("Select a layer to draw on");
      const snapped = JSON.parse(engine.snapPoint(pt.x, pt.y, JSON.stringify(snapConfig())));
      const p0 = { x: snapped.x, y: snapped.y };
      interaction.current = { kind: "shape", tool: p.tool, p0, p1: p0, guides: snapped.guides, preview: null };
      return;
    }

    const chrome = chromeAt(s);
    if (chrome) {
      interaction.current = { kind: "pending", screen: s, stage: pt, mode: chrome.mode, ids: p.selection };
      return;
    }

    const hit = engine.hitTest(pt.x, pt.y, 3 / v.zoom);
    if (hit !== undefined) {
      let ids = p.selection;
      if (e.shiftKey) {
        ids = ids.includes(hit) ? ids.filter((i) => i !== hit) : [...ids, hit];
        p.onSelect(ids);
        if (!ids.includes(hit)) return;
      } else if (!ids.includes(hit)) {
        ids = [hit];
        p.onSelect(ids);
      }
      interaction.current = { kind: "pending", screen: s, stage: pt, mode: { mode: "move" }, ids };
      return;
    }

    interaction.current = { kind: "marquee", start: pt, current: pt, base: e.shiftKey ? p.selection : [] };
    if (!e.shiftKey) p.onSelect([]);
  };

  const updateTransform = (pt: Pt, e: React.PointerEvent) => {
    try {
      const guides: Guides = JSON.parse(engine.updateTransform(pt.x, pt.y, JSON.stringify(mods(e)), JSON.stringify(snapConfig())));
      interaction.current = { kind: "transform", guides };
    } catch (err) {
      interaction.current = { kind: "transform", guides: { x: [], y: [] } };
      fail(err);
    }
    requestDraw();
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const { props: p, view: v } = live.current;
    const s = screenPoint(e);
    const pt = toStage(v, s);
    cursorStage.current = pt;
    p.onCursor(pt);
    const it = interaction.current;

    switch (it.kind) {
      case "idle":
        if (p.tool === "hand" || spaceDown.current) setCursor("grab");
        else if (p.tool !== "select") setCursor("crosshair");
        else {
          const chrome = chromeAt(s);
          if (chrome) setCursor(chrome.cursor);
          else setCursor(engine.hitTest(pt.x, pt.y, 3 / v.zoom) !== undefined ? "move" : "default");
        }
        requestDraw(); // ruler cursor
        return;
      case "pan":
        p.onView({ zoom: it.view.zoom, panX: it.view.panX + s.x - it.screen.x, panY: it.view.panY + s.y - it.screen.y });
        return;
      case "pending":
        if (dist(s, it.screen) < DRAG_THRESHOLD) return;
        try {
          engine.beginTransform(JSON.stringify(it.ids), JSON.stringify(it.mode), it.stage.x, it.stage.y);
        } catch (err) {
          interaction.current = { kind: "idle" };
          return fail(err);
        }
        updateTransform(pt, e);
        return;
      case "transform":
        updateTransform(pt, e);
        return;
      case "marquee":
        interaction.current = { ...it, current: pt };
        requestDraw();
        return;
      case "shape": {
        const snapped = JSON.parse(engine.snapPoint(pt.x, pt.y, JSON.stringify(snapConfig())));
        const p1 = e.shiftKey ? pt : { x: snapped.x, y: snapped.y };
        const preview: ShapeDrag | null = JSON.parse(
          engine.shapePreview(it.tool, it.p0.x, it.p0.y, p1.x, p1.y, JSON.stringify(mods(e))),
        );
        interaction.current = { ...it, p1, preview, guides: e.shiftKey ? { x: [], y: [] } : snapped.guides };
        requestDraw();
        return;
      }
    }
  };

  const onPointerUp = (e: React.PointerEvent) => {
    const { props: p } = live.current;
    const it = interaction.current;
    interaction.current = { kind: "idle" };
    switch (it.kind) {
      case "pan":
        setCursor(p.tool === "hand" || spaceDown.current ? "grab" : "default");
        break;
      case "transform":
        try {
          engine.endTransform();
        } catch (err) {
          fail(err);
        }
        p.onChanged();
        break;
      case "marquee": {
        const hits: number[] = JSON.parse(engine.marquee(it.start.x, it.start.y, it.current.x, it.current.y));
        const moved = dist(it.start, it.current) * live.current.view.zoom >= DRAG_THRESHOLD;
        if (moved) p.onSelect([...new Set([...it.base, ...hits])]);
        break;
      }
      case "shape":
        if (p.activeLayer === null) break;
        try {
          const id = engine.createShape(
            p.activeLayer,
            it.tool,
            it.p0.x,
            it.p0.y,
            it.p1.x,
            it.p1.y,
            JSON.stringify(mods(e)),
            JSON.stringify(p.shapeStyle),
          );
          if (id !== undefined) p.onSelect([id]);
          p.onChanged();
        } catch (err) {
          fail(err);
        }
        break;
    }
    requestDraw();
  };

  const cancelInteraction = useCallback(() => {
    const it = interaction.current;
    if (it.kind === "transform") engine.cancelTransform();
    interaction.current = { kind: "idle" };
    requestDraw();
  }, [engine, requestDraw]);

  // Space = temporary hand; Escape cancels a drag in progress.
  useEffect(() => {
    const typing = (e: KeyboardEvent) => e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement;
    const down = (e: KeyboardEvent) => {
      if (typing(e)) return;
      if (e.code === "Space" && !e.repeat) {
        spaceDown.current = true;
        if (interaction.current.kind === "idle") setCursor("grab");
        e.preventDefault();
      }
      if (e.key === "Escape" && interaction.current.kind !== "idle") {
        cancelInteraction();
        e.stopImmediatePropagation();
      }
    };
    const up = (e: KeyboardEvent) => {
      if (e.code === "Space") {
        spaceDown.current = false;
        if (interaction.current.kind === "idle") setCursor("default");
      }
    };
    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up);
    };
  }, [cancelInteraction]);

  // Wheel: pan; with ⌘/Ctrl (or pinch): zoom at the cursor.
  useEffect(() => {
    const vp = viewportRef.current!;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const { props: p, view: v } = live.current;
      if (e.ctrlKey || e.metaKey) {
        p.onView(zoomAt(v, screenPoint(e), v.zoom * Math.exp(-e.deltaY * 0.01)));
      } else {
        p.onView({ ...v, panX: v.panX - e.deltaX, panY: v.panY - e.deltaY });
      }
    };
    vp.addEventListener("wheel", onWheel, { passive: false });
    return () => vp.removeEventListener("wheel", onWheel);
  }, []);

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    const files = Array.from(e.dataTransfer.files).filter((f) => /^image\/(png|jpeg|gif)$/.test(f.type));
    if (files.length) props.onDropFiles(files, toStage(live.current.view, screenPoint(e)));
  };

  return (
    <div className={`stage-area ${settings.showRulers ? "with-rulers" : ""}`}>
      {settings.showRulers && (
        <>
          <div className="ruler-corner" onDoubleClick={() => props.onView(null)} title="Double-click: fit" />
          <canvas className="ruler ruler-x" ref={rulerXRef} />
          <canvas className="ruler ruler-y" ref={rulerYRef} />
        </>
      )}
      <div
        className="viewport"
        ref={viewportRef}
        onDragOver={(e) => e.preventDefault()}
        onDrop={onDrop}
        onPointerLeave={() => {
          cursorStage.current = null;
          props.onCursor(null);
          requestDraw();
        }}
      >
        <canvas ref={sceneRef} />
        <canvas
          ref={overlayRef}
          style={{ cursor }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={cancelInteraction}
        />
      </div>
    </div>
  );
}

// ------------------------------------------------------------ overlay drawing

const SELECT = "#2f9bff";
const GUIDE = "#ff3cac";

function drawOverlay(o: CanvasRenderingContext2D, p: Props, v: View, geom: SelectionGeometry | null, it: Interaction) {
  const sc = (pt: Pt) => toScreen(v, pt);
  const { stage, settings } = p;

  // Stage border.
  const s0 = sc({ x: 0, y: 0 });
  const s1 = sc({ x: stage.width, y: stage.height });
  o.strokeStyle = "rgba(0,0,0,0.5)";
  o.lineWidth = 1;
  o.strokeRect(Math.round(s0.x) - 0.5, Math.round(s0.y) - 0.5, Math.round(s1.x - s0.x) + 1, Math.round(s1.y - s0.y) + 1);

  if (settings.showGrid && settings.gridSize * v.zoom >= 4) {
    const step = settings.gridSize;
    o.save();
    o.beginPath();
    o.rect(s0.x, s0.y, s1.x - s0.x, s1.y - s0.y);
    o.clip();
    o.strokeStyle = "rgba(60,60,90,0.18)";
    o.beginPath();
    for (let x = 0; x <= stage.width; x += step) {
      const sx = Math.round(x * v.zoom + v.panX) + 0.5;
      o.moveTo(sx, s0.y);
      o.lineTo(sx, s1.y);
    }
    for (let y = 0; y <= stage.height; y += step) {
      const sy = Math.round(y * v.zoom + v.panY) + 0.5;
      o.moveTo(s0.x, sy);
      o.lineTo(s1.x, sy);
    }
    o.stroke();
    o.restore();
  }

  if (geom && it.kind !== "marquee") {
    o.strokeStyle = "rgba(47,155,255,0.6)";
    if (geom.items.length > 1) {
      for (const r of geom.items) {
        const a = sc(r.min);
        const b = sc(r.max);
        o.strokeRect(a.x + 0.5, a.y + 0.5, b.x - a.x, b.y - a.y);
      }
    }
    const c = geom.corners.map(sc);
    o.strokeStyle = SELECT;
    o.beginPath();
    c.forEach((pt, i) => (i ? o.lineTo(pt.x, pt.y) : o.moveTo(pt.x, pt.y)));
    o.closePath();
    o.stroke();
    if (it.kind !== "transform") {
      const hp = handlePoints(c);
      o.fillStyle = "#fff";
      for (const h of HANDLES) {
        const q = hp[h];
        o.fillRect(q.x - HANDLE / 2, q.y - HANDLE / 2, HANDLE, HANDLE);
        o.strokeRect(q.x - HANDLE / 2, q.y - HANDLE / 2, HANDLE, HANDLE);
      }
    }
    const pv = sc(geom.pivot);
    o.beginPath();
    o.arc(pv.x, pv.y, 4, 0, Math.PI * 2);
    o.fillStyle = geom.hasOwnPivot ? "#fff" : "rgba(255,255,255,0.5)";
    o.fill();
    o.strokeStyle = "#222";
    o.stroke();
  }

  if (it.kind === "marquee") {
    const a = sc(it.start);
    const b = sc(it.current);
    o.fillStyle = "rgba(47,155,255,0.08)";
    o.fillRect(a.x, a.y, b.x - a.x, b.y - a.y);
    o.setLineDash([4, 3]);
    o.strokeStyle = SELECT;
    o.strokeRect(a.x + 0.5, a.y + 0.5, b.x - a.x, b.y - a.y);
    o.setLineDash([]);
  }

  if (it.kind === "shape" && it.preview) {
    const { geometry: g, center } = it.preview;
    const c = sc(center);
    o.setLineDash([5, 4]);
    o.strokeStyle = SELECT;
    o.beginPath();
    if (g.kind === "rect") o.rect(c.x - (g.width! * v.zoom) / 2, c.y - (g.height! * v.zoom) / 2, g.width! * v.zoom, g.height! * v.zoom);
    else if (g.kind === "ellipse") o.ellipse(c.x, c.y, (g.width! * v.zoom) / 2, (g.height! * v.zoom) / 2, 0, 0, Math.PI * 2);
    else {
      o.moveTo(c.x - (g.dx! * v.zoom) / 2, c.y - (g.dy! * v.zoom) / 2);
      o.lineTo(c.x + (g.dx! * v.zoom) / 2, c.y + (g.dy! * v.zoom) / 2);
    }
    o.stroke();
    o.setLineDash([]);
  }

  const guides = it.kind === "transform" || it.kind === "shape" ? it.guides : null;
  if (guides) {
    o.strokeStyle = GUIDE;
    o.beginPath();
    for (const x of guides.x) {
      const sx = Math.round(x * v.zoom + v.panX) + 0.5;
      o.moveTo(sx, 0);
      o.lineTo(sx, o.canvas.height);
    }
    for (const y of guides.y) {
      const sy = Math.round(y * v.zoom + v.panY) + 0.5;
      o.moveTo(0, sy);
      o.lineTo(o.canvas.width, sy);
    }
    o.stroke();
  }
}

