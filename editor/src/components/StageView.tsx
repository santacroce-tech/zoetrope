import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { SYMBOL_DRAG_TYPE } from "../engine";
import type {
  DragMode,
  Engine,
  GradientControls,
  GradientHandle,
  Guides,
  Handle,
  Modifiers,
  NodeRef,
  Onion,
  PathHit,
  PathInfo,
  PenPreview,
  PickedStyle,
  Polyline,
  Pt,
  SelectionGeometry,
  ShapeOptions,
  ShapeStyle,
  ShapeTool,
  SnapConfig,
  StageInfo,
  TextStyle,
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

export type Tool = "select" | "subselect" | "pen" | "pencil" | ShapeTool | "text" | "gradient" | "eyedropper" | "bucket" | "hand";

export interface ToolOptions extends ShapeOptions {
  /** Pencil: smooth curves (true) or straight segments ("ink", false). */
  pencilSmooth: boolean;
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
  /** Current frame of the timeline (already set on the engine). */
  frame: number;
  /** Onion skin settings, or null when off. */
  onion: Onion | null;
  /** Preview playback through the runtime player: render its state and send it the pointer. */
  runtime: boolean;
  /** Double-click on a symbol instance (select tool): edit it in place. */
  onEnterInstance: (id: number) => void;
  /** A library symbol dropped onto the stage. */
  onDropSymbol: (symbol: number, at: Pt) => void;
  stage: StageInfo;
  selection: number[];
  onSelect: (ids: number[]) => void;
  /** Selected anchors of the (single) selected shape, for the subselection tool. */
  anchors: NodeRef[];
  onAnchors: (a: NodeRef[]) => void;
  tool: Tool;
  activeLayer: number | null;
  shapeStyle: ShapeStyle;
  /** Style for new text (text tool). */
  textStyle: TextStyle;
  toolOptions: ToolOptions;
  onPicked: (p: PickedStyle) => void;
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
const NO_GUIDES: Guides = { x: [], y: [] };

type Interaction =
  | { kind: "idle" }
  | { kind: "pending"; screen: Pt; stage: Pt; mode: DragMode; ids: number[] }
  | { kind: "transform"; guides: Guides }
  | { kind: "pendingEdit"; screen: Pt; stage: Pt; id: number; target: object }
  | { kind: "edit" }
  | { kind: "marquee"; start: Pt; current: Pt; base: number[] }
  | { kind: "shape"; tool: ShapeTool; p0: Pt; p1: Pt; guides: Guides; preview: Polyline[] | null }
  | { kind: "pen"; closing: boolean }
  | { kind: "pencil"; points: Pt[] }
  | { kind: "textBox"; p0: Pt; p1: Pt }
  | { kind: "pan"; screen: Pt; view: View };

const HANDLES: Handle[] = ["nw", "ne", "se", "sw", "n", "e", "s", "w"];

function handlePoints(c: Pt[]): Record<Handle, Pt> {
  return { nw: c[0], ne: c[1], se: c[2], sw: c[3], n: mid(c[0], c[1]), e: mid(c[1], c[2]), s: mid(c[2], c[3]), w: mid(c[3], c[0]) };
}

const svgCursor = (svg: string, x: number, y: number, fallback: string) =>
  `url("data:image/svg+xml,${encodeURIComponent(svg)}") ${x} ${y}, ${fallback}`;

const ROTATE_CURSOR = svgCursor(
  '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 22 22"><path d="M17 11a6 6 0 1 1-2-4.5" fill="none" stroke="white" stroke-width="4"/><path d="M17 11a6 6 0 1 1-2-4.5" fill="none" stroke="black" stroke-width="1.6"/><path d="M13 3l4 3.5-4.8 1.5z" fill="black" stroke="white" stroke-width="0.8"/></svg>',
  11,
  11,
  "crosshair",
);
const PEN_CURSOR = svgCursor(
  '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20"><path d="M2 2l5 13 3-3 4 4 2-2-4-4 3-3z" fill="black" stroke="white" stroke-width="1"/></svg>',
  2,
  2,
  "crosshair",
);

function resizeCursor(from: Pt, to: Pt): string {
  const a = ((Math.atan2(to.y - from.y, to.x - from.x) * 180) / Math.PI + 180) % 180;
  if (a < 22.5 || a >= 157.5) return "ew-resize";
  if (a < 67.5) return "nwse-resize";
  if (a < 112.5) return "ns-resize";
  return "nesw-resize";
}

const sameNode = (a: NodeRef, b: NodeRef) => a.subpath === b.subpath && a.node === b.node;

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
  /** Text being typed into (the core previews it live; committed on blur/Esc). */
  const [textEdit, setTextEdit] = useState<{ id: number; value: string } | null>(null);

  const view: View = props.view ?? fitView(size.w || 1, size.h || 1, stage.width, stage.height);
  // Event handlers read the latest props/view through refs.
  const live = useRef({ props, view });
  live.current = { props, view };

  const onEffectiveView = props.onEffectiveView;
  useEffect(() => onEffectiveView(view, size.w, size.h), [onEffectiveView, view.zoom, view.panX, view.panY, size.w, size.h]); // eslint-disable-line react-hooks/exhaustive-deps

  // ------------------------------------------------------------ core queries

  const single = (): number | null => (live.current.props.selection.length === 1 ? live.current.props.selection[0] : null);

  const selectionGeometry = useCallback((): SelectionGeometry | null => {
    const sel = live.current.props.selection;
    return sel.length ? JSON.parse(engine.selectionJson(JSON.stringify(sel))) : null;
  }, [engine]);

  const pathInfo = useCallback((): PathInfo | null => {
    const sel = live.current.props.selection;
    return sel.length === 1 ? JSON.parse(engine.pathInfoJson(sel[0])) : null;
  }, [engine]);

  const gradientControls = useCallback((): GradientControls | null => {
    const sel = live.current.props.selection;
    return sel.length === 1 ? JSON.parse(engine.gradientJson(sel[0], "fill")) : null;
  }, [engine]);

  const snapConfig = (): SnapConfig => {
    const { settings: s } = live.current.props;
    return { grid: s.snapToGrid ? s.gridSize : null, objects: s.snapToObjects, tolerance: 6 / live.current.view.zoom };
  };

  const closeTolerance = () => 8 / live.current.view.zoom;

  /** Stage-space corners of the text box being typed into. */
  const textBoxCorners = (): Pt[] | null => {
    const id = engine.textEditId();
    if (id === undefined) return null;
    const g: SelectionGeometry | null = JSON.parse(engine.selectionJson(JSON.stringify([id])));
    return g?.corners ?? null;
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
    if (p.runtime) p.engine.playRender(ctx, v.zoom * dpr, v.panX * dpr, v.panY * dpr, false, p.settings.showGuides);
    else p.engine.render(ctx, p.frame, v.zoom * dpr, v.panX * dpr, v.panY * dpr, false, p.settings.showGuides, JSON.stringify(p.onion));
    p.onRenderTime(performance.now() - t0);

    fitCanvas(overlay, w, h);
    const o = overlay.getContext("2d")!;
    o.setTransform(dpr, 0, 0, dpr, 0, 0);
    o.clearRect(0, 0, w, h);
    if (p.runtime) {
      drawOverlay(o, p, v, { selection: null, path: null, gradient: null, pen: null, anchors: [], textBox: null }, { kind: "idle" });
      return;
    }
    const chrome: Chrome = {
      selection: p.tool === "select" ? selectionGeometry() : null,
      path: p.tool === "subselect" ? pathInfo() : null,
      gradient: p.tool === "gradient" ? gradientControls() : null,
      pen: p.tool === "pen" ? JSON.parse(p.engine.penPreviewJson(8 / v.zoom)) : null,
      anchors: p.anchors,
      textBox: textBoxCorners(),
    };
    drawOverlay(o, p, v, chrome, interaction.current);

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
  }, [selectionGeometry, pathInfo, gradientControls]); // eslint-disable-line react-hooks/exhaustive-deps

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
  }, [requestDraw, version, selection, props.anchors, size, props.view, stage, settings, props.tool, props.frame, props.onion, props.runtime]);

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

  // ------------------------------------------------------------ text editing

  const commitText = useCallback(() => {
    if (engine.textEditId() === undefined) return setTextEdit(null);
    try {
      const id = engine.endTextEdit();
      live.current.props.onSelect(id === undefined ? [] : [id]);
    } catch (err) {
      fail(err);
    }
    setTextEdit(null);
    live.current.props.onChanged();
  }, [engine]); // eslint-disable-line react-hooks/exhaustive-deps

  const startTextEdit = (id: number) => {
    try {
      const value = engine.beginTextEdit(id);
      setTextEdit({ id, value });
      live.current.props.onSelect([]);
      requestDraw();
    } catch (err) {
      fail(err);
    }
  };

  // Typing ends when the engine ends it (undo, load, playback…) or the tool changes.
  useEffect(() => {
    if (textEdit && engine.textEditId() !== textEdit.id) setTextEdit(null);
  }, [engine, version, textEdit]);
  useEffect(() => {
    if (props.tool !== "text" && props.tool !== "select") commitText();
  }, [props.tool, commitText]);
  useEffect(() => {
    if (props.runtime) commitText();
  }, [props.runtime, commitText]);

  const isText = (id: number | undefined) => id !== undefined && JSON.parse(engine.elementJson(id))?.element.type === "text";

  // ------------------------------------------------------------ pen lifecycle

  const fail = useCallback((e: unknown) => live.current.props.onError(e instanceof Error ? e.message : String(e)), []);

  const finishPen = useCallback(() => {
    const { props: p } = live.current;
    if (JSON.parse(engine.penPreviewJson(0)) === null) return;
    try {
      const id = p.activeLayer === null ? undefined : engine.penFinish(p.activeLayer, JSON.stringify(p.shapeStyle));
      if (id === undefined) engine.penCancel();
      else {
        p.onSelect([id]);
        p.onChanged();
      }
    } catch (err) {
      engine.penCancel();
      fail(err);
    }
    interaction.current = { kind: "idle" };
    requestDraw();
  }, [engine, requestDraw, fail]);

  // Leaving the pen tool finishes the path in progress.
  useEffect(() => {
    if (props.tool !== "pen") finishPen();
  }, [props.tool, finishPen]);

  // ------------------------------------------------------------ chrome hit-testing

  const transformChromeAt = (s: Pt): { mode: DragMode; cursor: string } | null => {
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
    if (!pointInPolygon(s, c)) {
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

  type PathChrome = { anchor: NodeRef } | { handle: NodeRef; side: "in" | "out" } | { segment: PathHit };

  const pathChromeAt = (s: Pt): PathChrome | null => {
    const info = pathInfo();
    const id = single();
    if (!info || id === null) return null;
    const v = live.current.view;
    // Handles of selected anchors first (they sit on top).
    for (const r of live.current.props.anchors) {
      const n = info.subpaths[r.subpath]?.[r.node];
      if (!n) continue;
      for (const side of ["in", "out"] as const) {
        const hpt = n[side];
        if (hpt && dist(s, toScreen(v, hpt)) <= 6) return { handle: r, side };
      }
    }
    for (let sp = 0; sp < info.subpaths.length; sp++) {
      for (let i = 0; i < info.subpaths[sp].length; i++) {
        if (dist(s, toScreen(v, info.subpaths[sp][i])) <= 6) return { anchor: { subpath: sp, node: i } };
      }
    }
    const pt = toStage(v, s);
    const hit: PathHit | null = JSON.parse(engine.pathHitJson(id, pt.x, pt.y, 5 / v.zoom));
    return hit ? { segment: hit } : null;
  };

  const gradientChromeAt = (s: Pt): GradientHandle | null => {
    const g = gradientControls();
    if (!g) return null;
    const v = live.current.view;
    const near = (p: Pt) => dist(s, toScreen(v, p)) <= 7;
    if (g.kind === "linear") return near(g.end) ? "end" : near(g.start) ? "start" : null;
    if (near(g.focal) && dist(toScreen(v, g.focal), toScreen(v, g.center)) > 4) return "focal";
    return near(g.radius) ? "radius" : near(g.center) ? "center" : null;
  };

  // ------------------------------------------------------------ pointer handling

  const screenPoint = (e: { clientX: number; clientY: number }): Pt => {
    const r = viewportRef.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };

  const mods = (e: { shiftKey: boolean; altKey: boolean }): Modifiers => ({ shift: e.shiftKey, alt: e.altKey });

  const snap = (pt: Pt): { p: Pt; guides: Guides } => {
    const r = JSON.parse(engine.snapPoint(pt.x, pt.y, JSON.stringify(snapConfig())));
    return { p: { x: r.x, y: r.y }, guides: r.guides };
  };

  /** Click-selects the element under `pt` (or clears). Returns the hit. */
  const selectAt = (pt: Pt): number | undefined => {
    const { props: p, view: v } = live.current;
    const hit = engine.hitTest(pt.x, pt.y, 3 / v.zoom);
    if (hit === undefined) p.onSelect([]);
    else if (p.selection.length !== 1 || p.selection[0] !== hit) p.onSelect([hit]);
    return hit;
  };

  /** While previewing, the pointer drives buttons instead of editing. */
  const runtimePointer = (e: { clientX: number; clientY: number }, down: boolean, inside = true) => {
    const pt = toStage(live.current.view, screenPoint(e));
    const r = JSON.parse(engine.playPointer(pt.x, pt.y, inside, down));
    setCursor(r?.overButton ? "pointer" : "default");
    requestDraw();
  };

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0 && e.button !== 1) return;
    (e.target as Element).setPointerCapture(e.pointerId);
    const { props: p, view: v } = live.current;
    const s = screenPoint(e);
    const pt = toStage(v, s);
    const m = mods(e);
    if (p.runtime) return runtimePointer(e, true);
    if (engine.textEditId() !== undefined) commitText();

    if (p.tool === "hand" || spaceDown.current || e.button === 1) {
      interaction.current = { kind: "pan", screen: s, view: v };
      setCursor("grabbing");
      return;
    }

    switch (p.tool) {
      case "rect":
      case "ellipse":
      case "line":
      case "polygon": {
        if (p.activeLayer === null) return fail("Select a layer to draw on");
        const { p: p0, guides } = p.tool === "polygon" ? { p: pt, guides: NO_GUIDES } : snap(pt);
        interaction.current = { kind: "shape", tool: p.tool, p0, p1: p0, guides, preview: null };
        return;
      }
      case "pen": {
        if (p.activeLayer === null) return fail("Select a layer to draw on");
        const closing = engine.penDown(pt.x, pt.y, JSON.stringify(m), closeTolerance());
        interaction.current = { kind: "pen", closing };
        requestDraw();
        return;
      }
      case "pencil":
        if (p.activeLayer === null) return fail("Select a layer to draw on");
        interaction.current = { kind: "pencil", points: [pt] };
        return;
      case "text": {
        const hit = engine.hitTest(pt.x, pt.y, 3 / v.zoom);
        if (isText(hit)) return startTextEdit(hit!);
        if (p.activeLayer === null) return fail("Select a layer to type on");
        interaction.current = { kind: "textBox", p0: pt, p1: pt };
        return;
      }
      case "eyedropper": {
        const picked: PickedStyle | null = JSON.parse(engine.pickStyle(pt.x, pt.y, 3 / v.zoom));
        if (picked) p.onPicked(picked);
        return;
      }
      case "bucket": {
        // Click: apply the fill style. Shift-click: apply the stroke color.
        const hit = engine.hitTest(pt.x, pt.y, 3 / v.zoom);
        if (hit === undefined) return;
        const part = e.shiftKey ? "stroke" : "fill";
        const style = part === "fill" ? p.shapeStyle.fill : p.shapeStyle.stroke && { type: "solid", color: p.shapeStyle.stroke.color };
        try {
          engine.setPaintStyle(JSON.stringify([hit]), part, JSON.stringify(style));
          p.onChanged();
        } catch (err) {
          fail(err);
        }
        return;
      }
      case "gradient": {
        const id = single();
        const handle = id !== null ? gradientChromeAt(s) : null;
        if (id !== null && handle) {
          interaction.current = { kind: "pendingEdit", screen: s, stage: pt, id, target: { target: "gradient", part: "fill", handle } };
          return;
        }
        selectAt(pt);
        return;
      }
      case "subselect": {
        const id = single();
        const chrome = id !== null ? pathChromeAt(s) : null;
        if (id !== null && chrome) {
          if ("handle" in chrome) {
            interaction.current = { kind: "pendingEdit", screen: s, stage: pt, id, target: { target: "handle", node: chrome.handle, side: chrome.side } };
          } else if ("anchor" in chrome) {
            let anchors = p.anchors;
            const has = anchors.some((a) => sameNode(a, chrome.anchor));
            if (e.shiftKey) anchors = has ? anchors.filter((a) => !sameNode(a, chrome.anchor)) : [...anchors, chrome.anchor];
            else if (!has) anchors = [chrome.anchor];
            p.onAnchors(anchors);
            if (anchors.some((a) => sameNode(a, chrome.anchor))) {
              interaction.current = { kind: "pendingEdit", screen: s, stage: pt, id, target: { target: "anchors", nodes: anchors } };
            }
          } else if (e.altKey) {
            // Alt-click on a segment adds an anchor there.
            try {
              const r: NodeRef = JSON.parse(engine.insertAnchor(id, chrome.segment.subpath, chrome.segment.segment, chrome.segment.t));
              p.onAnchors([r]);
              p.onChanged();
            } catch (err) {
              fail(err);
            }
          } else {
            p.onAnchors([]);
          }
          return;
        }
        const hit = selectAt(pt);
        if (hit !== id) p.onAnchors([]);
        return;
      }
      case "select": {
        const chrome = transformChromeAt(s);
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
        return;
      }
    }
  };

  const updateTransform = (pt: Pt, e: React.PointerEvent) => {
    try {
      const guides: Guides = JSON.parse(engine.updateTransform(pt.x, pt.y, JSON.stringify(mods(e)), JSON.stringify(snapConfig())));
      interaction.current = { kind: "transform", guides };
    } catch (err) {
      interaction.current = { kind: "transform", guides: NO_GUIDES };
      fail(err);
    }
    requestDraw();
  };

  const updateEdit = (pt: Pt, e: React.PointerEvent) => {
    try {
      engine.updateEdit(pt.x, pt.y, JSON.stringify(mods(e)));
    } catch (err) {
      fail(err);
    }
    requestDraw();
  };

  const hoverCursor = (s: Pt, pt: Pt): string => {
    const { props: p, view: v } = live.current;
    if (p.tool === "hand" || spaceDown.current) return "grab";
    switch (p.tool) {
      case "select": {
        const chrome = transformChromeAt(s);
        if (chrome) return chrome.cursor;
        return engine.hitTest(pt.x, pt.y, 3 / v.zoom) !== undefined ? "move" : "default";
      }
      case "subselect": {
        const chrome = single() !== null ? pathChromeAt(s) : null;
        if (chrome && "segment" in chrome) return "copy";
        return chrome ? "pointer" : "default";
      }
      case "gradient":
        return single() !== null && gradientChromeAt(s) ? "pointer" : "default";
      case "pen":
        return PEN_CURSOR;
      case "text":
        return "text";
      case "eyedropper":
      case "bucket":
        return engine.hitTest(pt.x, pt.y, 3 / v.zoom) !== undefined ? "pointer" : "not-allowed";
      default:
        return "crosshair";
    }
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const { props: p, view: v } = live.current;
    const s = screenPoint(e);
    const pt = toStage(v, s);
    cursorStage.current = pt;
    p.onCursor(pt);
    if (p.runtime) return runtimePointer(e, (e.buttons & 1) === 1);
    const it = interaction.current;

    switch (it.kind) {
      case "idle":
        if (p.tool === "pen") engine.penHover(pt.x, pt.y, JSON.stringify(mods(e)));
        setCursor(hoverCursor(s, pt));
        requestDraw(); // ruler cursor, pen rubber band
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
      case "pendingEdit":
        if (dist(s, it.screen) < DRAG_THRESHOLD) return;
        try {
          engine.beginEdit(it.id, JSON.stringify(it.target), it.stage.x, it.stage.y);
          interaction.current = { kind: "edit" };
        } catch (err) {
          interaction.current = { kind: "idle" };
          return fail(err);
        }
        updateEdit(pt, e);
        return;
      case "edit":
        updateEdit(pt, e);
        return;
      case "marquee":
        interaction.current = { ...it, current: pt };
        requestDraw();
        return;
      case "shape": {
        const snapped = it.tool === "polygon" || e.shiftKey ? { p: pt, guides: NO_GUIDES } : snap(pt);
        const preview: Polyline[] | null = JSON.parse(
          engine.shapePreview(it.tool, it.p0.x, it.p0.y, snapped.p.x, snapped.p.y, JSON.stringify(mods(e)), JSON.stringify(p.toolOptions)),
        );
        interaction.current = { ...it, p1: snapped.p, preview, guides: snapped.guides };
        requestDraw();
        return;
      }
      case "pen":
        engine.penDrag(pt.x, pt.y, JSON.stringify(mods(e)));
        requestDraw();
        return;
      case "textBox":
        it.p1 = pt;
        requestDraw();
        return;
      case "pencil": {
        const last = it.points[it.points.length - 1];
        if (dist(last, pt) * v.zoom >= 1) it.points.push(pt);
        requestDraw();
        return;
      }
    }
  };

  const onPointerUp = (e: React.PointerEvent) => {
    const { props: p, view: v } = live.current;
    if (p.runtime) return runtimePointer(e, false);
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
      case "edit":
        try {
          engine.endEdit();
        } catch (err) {
          fail(err);
        }
        p.onChanged();
        break;
      case "marquee": {
        const hits: number[] = JSON.parse(engine.marquee(it.start.x, it.start.y, it.current.x, it.current.y));
        if (dist(it.start, it.current) * v.zoom >= DRAG_THRESHOLD) p.onSelect([...new Set([...it.base, ...hits])]);
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
            JSON.stringify(p.toolOptions),
            JSON.stringify(p.shapeStyle),
          );
          if (id !== undefined) p.onSelect([id]);
          p.onChanged();
        } catch (err) {
          fail(err);
        }
        break;
      case "pen":
        engine.penUp();
        if (it.closing) finishPen();
        break;
      case "textBox": {
        if (p.activeLayer === null) break;
        // Click: auto-width text. Drag: a wrapping box as wide as the drag.
        const dragged = Math.abs(it.p1.x - it.p0.x) * v.zoom >= DRAG_THRESHOLD;
        const x = dragged ? Math.min(it.p0.x, it.p1.x) : it.p0.x;
        const y = dragged ? Math.min(it.p0.y, it.p1.y) : it.p0.y;
        try {
          const style = { ...p.textStyle, font: p.textStyle.font ?? undefined };
          const id = engine.createText(p.activeLayer, x, y, JSON.stringify(style), dragged ? Math.abs(it.p1.x - it.p0.x) : undefined);
          setTextEdit({ id, value: "" });
          p.onSelect([]);
          p.onChanged();
        } catch (err) {
          fail(err);
        }
        break;
      }
      case "pencil":
        if (p.activeLayer === null || it.points.length < 2) break;
        try {
          const id = engine.createFreehand(
            p.activeLayer,
            JSON.stringify(it.points),
            p.toolOptions.pencilSmooth,
            1.5 / v.zoom,
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

  const onDoubleClick = (e: React.MouseEvent) => {
    const { props: p, view: v } = live.current;
    if (p.runtime) return;
    if (p.tool === "pen") return finishPen();
    if (p.tool === "select") {
      // Double-click a symbol instance: edit it in place.
      const pt = toStage(v, screenPoint(e));
      const hit = engine.hitTest(pt.x, pt.y, 3 / v.zoom);
      const type = hit === undefined ? null : JSON.parse(engine.elementJson(hit))?.element.type;
      if (type === "instance") p.onEnterInstance(hit!);
      if (type === "text") startTextEdit(hit!);
      return;
    }
    if (p.tool !== "subselect") return;
    // Double-click an anchor: toggle corner ⇄ smooth.
    const id = single();
    const chrome = id !== null ? pathChromeAt(screenPoint(e)) : null;
    if (id !== null && chrome && "anchor" in chrome) {
      try {
        engine.convertAnchor(id, chrome.anchor.subpath, chrome.anchor.node);
        p.onChanged();
      } catch (err) {
        fail(err);
      }
    }
  };

  const cancelInteraction = useCallback(() => {
    const it = interaction.current;
    if (it.kind === "transform") engine.cancelTransform();
    if (it.kind === "edit") engine.cancelEdit();
    interaction.current = { kind: "idle" };
    requestDraw();
  }, [engine, requestDraw]);

  // Space = temporary hand; Escape cancels a drag; Enter/Escape end a pen path.
  useEffect(() => {
    const typing = (e: KeyboardEvent) =>
      e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement || e.target instanceof HTMLTextAreaElement;
    const down = (e: KeyboardEvent) => {
      if (typing(e)) return;
      if (e.code === "Space" && !e.repeat) {
        spaceDown.current = true;
        if (interaction.current.kind === "idle") setCursor("grab");
        e.preventDefault();
      }
      const penActive = live.current.props.tool === "pen" && JSON.parse(engine.penPreviewJson(0)) !== null;
      if ((e.key === "Enter" || e.key === "Escape") && penActive) {
        finishPen();
        e.stopImmediatePropagation();
        e.preventDefault();
        return;
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
  }, [cancelInteraction, engine, finishPen]);

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
    const symbol = e.dataTransfer.getData(SYMBOL_DRAG_TYPE);
    if (symbol) return props.onDropSymbol(Number(symbol), toStage(live.current.view, screenPoint(e)));
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
        onPointerLeave={(e) => {
          if (live.current.props.runtime) runtimePointer(e, false, false);
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
          onDoubleClick={onDoubleClick}
        />
        {textEdit && !props.runtime && (
          <TextEditor
            key={textEdit.id}
            value={textEdit.value}
            corners={textBoxCorners()}
            view={view}
            onInput={(value) => {
              engine.updateText(value);
              setTextEdit({ ...textEdit, value });
              requestDraw();
            }}
            onCommit={commitText}
          />
        )}
      </div>
    </div>
  );
}

// ------------------------------------------------------------ overlay drawing

const SELECT = "#2f9bff";
const GUIDE = "#ff3cac";

interface Chrome {
  selection: SelectionGeometry | null;
  path: PathInfo | null;
  gradient: GradientControls | null;
  pen: PenPreview | null;
  anchors: NodeRef[];
  /** Box of the text being typed into. */
  textBox: Pt[] | null;
}

function strokePolylines(o: CanvasRenderingContext2D, lines: Polyline[], sc: (p: Pt) => Pt) {
  o.beginPath();
  for (const pl of lines) {
    pl.points.forEach((p, i) => {
      const q = sc(p);
      if (i) o.lineTo(q.x, q.y);
      else o.moveTo(q.x, q.y);
    });
    if (pl.closed) o.closePath();
  }
  o.stroke();
}

function square(o: CanvasRenderingContext2D, p: Pt, size: number, filled: boolean) {
  o.fillStyle = filled ? SELECT : "#fff";
  o.fillRect(p.x - size / 2, p.y - size / 2, size, size);
  o.strokeRect(p.x - size / 2, p.y - size / 2, size, size);
}

function dot(o: CanvasRenderingContext2D, p: Pt, r: number, fill: string) {
  o.beginPath();
  o.arc(p.x, p.y, r, 0, Math.PI * 2);
  o.fillStyle = fill;
  o.fill();
  o.stroke();
}

function drawOverlay(o: CanvasRenderingContext2D, p: Props, v: View, chrome: Chrome, it: Interaction) {
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

  const geom = chrome.selection;
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
      for (const h of HANDLES) square(o, hp[h], HANDLE, false);
    }
    o.strokeStyle = "#222";
    dot(o, sc(geom.pivot), 4, geom.hasOwnPivot ? "#fff" : "rgba(255,255,255,0.5)");
  }

  // Subselection: outline, anchors, and handles of selected anchors.
  if (chrome.path) {
    const info = chrome.path;
    const selected = (sp: number, i: number) => chrome.anchors.some((a) => a.subpath === sp && a.node === i);
    o.strokeStyle = SELECT;
    o.lineWidth = 1;
    strokePolylines(o, info.outline, sc);
    info.subpaths.forEach((nodes, sp) =>
      nodes.forEach((n, i) => {
        if (!selected(sp, i)) return;
        const a = sc(n);
        for (const h of [n.in, n.out]) {
          if (!h) continue;
          const q = sc(h);
          o.strokeStyle = SELECT;
          o.beginPath();
          o.moveTo(a.x, a.y);
          o.lineTo(q.x, q.y);
          o.stroke();
          dot(o, q, 3.5, "#fff");
        }
      }),
    );
    o.strokeStyle = SELECT;
    info.subpaths.forEach((nodes, sp) => nodes.forEach((n, i) => square(o, sc(n), 6, selected(sp, i))));
  }

  // Gradient controls.
  if (chrome.gradient) {
    const g = chrome.gradient;
    o.strokeStyle = "#111";
    o.lineWidth = 1;
    if (g.kind === "linear") {
      const a = sc(g.start);
      const b = sc(g.end);
      o.setLineDash([4, 3]);
      o.beginPath();
      o.moveTo(a.x, a.y);
      o.lineTo(b.x, b.y);
      o.stroke();
      o.setLineDash([]);
      dot(o, a, 5, "#fff");
      square(o, b, 9, false);
    } else {
      o.setLineDash([4, 3]);
      strokePolylines(o, [{ points: g.ring, closed: true }], sc);
      o.setLineDash([]);
      dot(o, sc(g.center), 5, "#fff");
      square(o, sc(g.radius), 9, false);
      const f = sc(g.focal);
      o.beginPath();
      o.moveTo(f.x, f.y - 6);
      o.lineTo(f.x + 6, f.y);
      o.lineTo(f.x, f.y + 6);
      o.lineTo(f.x - 6, f.y);
      o.closePath();
      o.fillStyle = "#ffd84d";
      o.fill();
      o.stroke();
    }
  }

  // Pen preview.
  if (chrome.pen) {
    const pen = chrome.pen;
    o.strokeStyle = SELECT;
    o.lineWidth = 1.5;
    strokePolylines(o, pen.outline, sc);
    o.lineWidth = 1;
    for (const [a, h] of pen.handles) {
      const qa = sc(a);
      const qh = sc(h);
      o.beginPath();
      o.moveTo(qa.x, qa.y);
      o.lineTo(qh.x, qh.y);
      o.stroke();
      dot(o, qh, 3.5, "#fff");
    }
    pen.anchors.forEach((a, i) => square(o, sc(a), i === 0 && pen.canClose ? 10 : 6, i === pen.anchors.length - 1));
  }

  if (chrome.textBox) {
    o.setLineDash([3, 3]);
    o.strokeStyle = SELECT;
    strokePolylines(o, [{ points: chrome.textBox, closed: true }], sc);
    o.setLineDash([]);
  }

  if (it.kind === "textBox" && Math.abs(it.p1.x - it.p0.x) * v.zoom >= DRAG_THRESHOLD) {
    const a = sc(it.p0);
    const b = sc(it.p1);
    o.setLineDash([3, 3]);
    o.strokeStyle = SELECT;
    o.strokeRect(a.x + 0.5, a.y + 0.5, b.x - a.x, b.y - a.y);
    o.setLineDash([]);
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
    o.setLineDash([5, 4]);
    o.strokeStyle = SELECT;
    strokePolylines(o, it.preview, sc);
    o.setLineDash([]);
  }

  if (it.kind === "pencil") {
    o.strokeStyle = p.shapeStyle.stroke?.color ?? SELECT;
    o.lineWidth = Math.max(1, (p.shapeStyle.stroke?.width ?? 1) * v.zoom);
    o.lineCap = "round";
    o.lineJoin = "round";
    strokePolylines(o, [{ points: it.points, closed: false }], sc);
    o.lineWidth = 1;
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

/**
 * The text field for typing into a text element. The canvas shows the real,
 * shaped result live; this field sits under the box for caret and IME input.
 */
function TextEditor(props: { value: string; corners: Pt[] | null; view: View; onInput: (v: string) => void; onCommit: () => void }) {
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const t = ref.current!;
    t.focus();
    t.setSelectionRange(t.value.length, t.value.length);
  }, []);
  if (!props.corners) return null;
  const pts = props.corners.map((c) => toScreen(props.view, c));
  const left = Math.min(...pts.map((q) => q.x));
  const top = Math.max(...pts.map((q) => q.y)) + 6;
  return (
    <textarea
      ref={ref}
      className="text-editor"
      style={{ left, top }}
      value={props.value}
      placeholder="Type…  (Esc or ⌘↵ to finish)"
      spellCheck={false}
      rows={Math.max(1, props.value.split("\n").length)}
      onChange={(e) => props.onInput(e.target.value)}
      onBlur={props.onCommit}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape" || (e.key === "Enter" && (e.metaKey || e.ctrlKey))) {
          e.preventDefault();
          props.onCommit();
        }
      }}
    />
  );
}
