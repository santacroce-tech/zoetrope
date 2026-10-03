// Loads the shared WASM core. All rendering, geometry, hit-testing and model
// logic lives there; this module only declares the JSON shapes it returns.
import init, { Engine } from "./wasm/pkg/zoetrope_web.js";

export type { Engine };

let enginePromise: Promise<Engine> | null = null;

export function loadEngine(): Promise<Engine> {
  enginePromise ??= init().then(() => new Engine());
  return enginePromise;
}

export interface Pt {
  x: number;
  y: number;
}

export interface Rect {
  min: Pt;
  max: Pt;
}

export type LayerKind = "normal" | "guide" | "folder";

export interface LayerNode {
  id: number;
  name: string;
  kind: LayerKind;
  visible: boolean;
  locked: boolean;
  /** Keyframe spans (content layers). */
  keyframes: KeyframeView[];
  /** Top (front-most) first. */
  children: LayerNode[];
}

export interface HistoryState {
  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
  dirty: boolean;
}

export interface StageInfo {
  width: number;
  height: number;
  background: string;
  fps: number;
}

export interface Transform {
  x: number;
  y: number;
  scaleX: number;
  scaleY: number;
  rotation: number;
  skewX: number;
  skewY: number;
  pivotX: number;
  pivotY: number;
}

export type BlendMode =
  | "normal"
  | "layer"
  | "multiply"
  | "screen"
  | "overlay"
  | "darken"
  | "lighten"
  | "hardLight"
  | "difference"
  | "add";

export const BLEND_MODES: BlendMode[] = [
  "normal",
  "layer",
  "multiply",
  "screen",
  "overlay",
  "darken",
  "lighten",
  "hardLight",
  "difference",
  "add",
];

export interface ElementData {
  id: number;
  name: string;
  transform: Transform;
  /** Omitted when 1. */
  opacity?: number;
  /** Omitted when normal. */
  blend?: BlendMode;
  tint?: { color: string; amount: number };
  type: "shape" | "instance" | "bitmap" | "text";
  geometry?: { kind: "rect" | "ellipse" | "line" | "path"; width?: number; height?: number; dx?: number; dy?: number };
  fill?: Paint;
  stroke?: StrokeData;
  fillRule?: FillRule;
  symbol?: number;
  asset?: number;
  /** Graphic instances: frame shown when the parent keyframe starts. */
  firstFrame?: number;
  loopMode?: LoopMode;
  // Text elements:
  text?: string;
  font?: number;
  size?: number;
  align?: TextAlign;
  letterSpacing?: number;
  lineHeight?: number;
  /** Wrap width; absent = auto width. */
  width?: number;
}

export interface ElementInfo {
  element: ElementData;
  layer: number;
  contentWidth: number;
  contentHeight: number;
  bounds: Rect | null;
  sourceName: string | null;
  /** Shown interpolated at the current frame (inside a tween): on-stage edits are disabled. */
  tweened: boolean;
}

export interface SelectionGeometry {
  /** Box corners in stage space: TL, TR, BR, BL (box-space order). */
  corners: [Pt, Pt, Pt, Pt];
  pivot: Pt;
  bounds: Rect;
  items: Rect[];
  hasOwnPivot: boolean;
}

export interface Guides {
  x: number[];
  y: number[];
}

export type ShapeTool = "rect" | "ellipse" | "line" | "polygon";

export type Handle = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

export type DragMode =
  | { mode: "move" }
  | { mode: "rotate" }
  | { mode: "pivot" }
  | { mode: "scale"; handle: Handle }
  | { mode: "skew"; handle: Handle };

export interface SnapConfig {
  grid: number | null;
  objects: boolean;
  tolerance: number;
}

export interface Modifiers {
  shift: boolean;
  alt: boolean;
}

// ---------- Phase 3: paints, paths, pen ----------

export interface GradientStop {
  offset: number;
  color: string;
}

export type Paint =
  | { type: "solid"; color: string }
  | { type: "linear"; start: Pt; end: Pt; stops: GradientStop[] }
  | { type: "radial"; center: Pt; radius: number; focal?: Pt; stops: GradientStop[] };

/** Geometry-free paint, as held by tools; the core fits gradients to shapes. */
export type PaintStyle =
  | { type: "solid"; color: string }
  | { type: "linear"; stops: GradientStop[] }
  | { type: "radial"; stops: GradientStop[] };

export type LineCap = "butt" | "round" | "square";
export type LineJoin = "miter" | "round" | "bevel";
export type FillRule = "nonZero" | "evenOdd";

export interface StrokeData {
  width: number;
  paint: Paint;
  cap: LineCap;
  join: LineJoin;
  miterLimit: number;
  dash?: number[];
  dashOffset?: number;
}

export interface StrokeStyle {
  color: string;
  width: number;
  cap: LineCap;
  join: LineJoin;
  dash: number[];
}

export interface ShapeStyle {
  fill: PaintStyle | null;
  stroke: StrokeStyle | null;
}

export interface ShapeOptions {
  sides: number;
  star: number | null;
}

export interface Polyline {
  points: Pt[];
  closed: boolean;
}

export interface NodeRef {
  subpath: number;
  node: number;
}

export interface NodeInfo {
  x: number;
  y: number;
  in: Pt | null;
  out: Pt | null;
  kind: "corner" | "smooth" | "symmetric";
}

export interface PathInfo {
  subpaths: NodeInfo[][];
  closed: boolean[];
  outline: Polyline[];
  primitive: boolean;
}

export interface PathHit {
  subpath: number;
  segment: number;
  t: number;
  distance: number;
}

export interface PenPreview {
  outline: Polyline[];
  anchors: Pt[];
  handles: [Pt, Pt][];
  canClose: boolean;
}

export type GradientControls =
  | { kind: "linear"; start: Pt; end: Pt }
  | { kind: "radial"; center: Pt; radius: Pt; focal: Pt; ring: Pt[] };

export type GradientHandle = "start" | "end" | "center" | "radius" | "focal";

export interface PickedStyle {
  part: "fill" | "stroke";
  fill: PaintStyle | null;
  stroke: StrokeStyle | null;
}

/** CSS preview of a paint style (for swatches and gradient bars). */
export function cssPaint(p: PaintStyle | Paint | null | undefined): string {
  if (!p) return "transparent";
  if (p.type === "solid") return p.color;
  const stops = p.stops.map((s) => `${s.color} ${(s.offset * 100).toFixed(1)}%`).join(", ");
  return p.type === "linear" ? `linear-gradient(90deg, ${stops})` : `radial-gradient(circle, ${stops})`;
}

// ---------- Phase 4: timeline ----------

export type EasePreset =
  | "easeInQuad" | "easeOutQuad" | "easeInOutQuad"
  | "easeInCubic" | "easeOutCubic" | "easeInOutCubic"
  | "easeInSine" | "easeOutSine" | "easeInOutSine"
  | "easeInBack" | "easeOutBack" | "easeInOutBack"
  | "easeInBounce" | "easeOutBounce" | "easeInOutBounce"
  | "easeInElastic" | "easeOutElastic" | "easeInOutElastic";

export const EASE_PRESETS: EasePreset[] = [
  "easeInQuad", "easeOutQuad", "easeInOutQuad",
  "easeInCubic", "easeOutCubic", "easeInOutCubic",
  "easeInSine", "easeOutSine", "easeInOutSine",
  "easeInBack", "easeOutBack", "easeInOutBack",
  "easeInBounce", "easeOutBounce", "easeInOutBounce",
  "easeInElastic", "easeOutElastic", "easeInOutElastic",
];

export type Easing =
  | { type: "linear" }
  | { type: "preset"; name: EasePreset }
  | { type: "bezier"; x1: number; y1: number; x2: number; y2: number };

export type TweenKind = "motion" | "shape";

export interface Tween {
  kind: TweenKind;
  easing: Easing;
  /** Extra full turns (positive = clockwise). */
  rotate?: number;
}

export interface KeyframeView {
  start: number;
  duration: number;
  empty: boolean;
  tween?: Tween;
  sound?: SoundRef;
  /** Frame label (scripts can jump to it). */
  label?: string;
  /** Frame script (JavaScript). */
  script?: string;
}

export interface Onion {
  before: number;
  after: number;
  alpha: number;
}

// ---------- Phase 5: symbols ----------

export type SymbolKind = "graphic" | "movieClip" | "button";
export type LoopMode = "loop" | "playOnce" | "singleFrame";

export interface LibraryItem {
  id: number;
  name: string;
  kind: SymbolKind;
  uses: number;
  length: number;
  /** Has a symbol script. */
  hasScript: boolean;
}

export interface Crumb {
  label: string;
  kind: SymbolKind | null;
}

export const SYMBOL_KIND_LABEL: Record<SymbolKind, string> = { graphic: "Graphic", movieClip: "Movie clip", button: "Button" };
export const SYMBOL_KIND_ICON: Record<SymbolKind, string> = { graphic: "◇", movieClip: "🎞", button: "⏺" };

/** Drag-and-drop type for library symbols dropped onto the stage. */
export const SYMBOL_DRAG_TYPE = "application/x-zoetrope-symbol";

// ---------- Phase 6: text & audio ----------

export type TextAlign = "left" | "center" | "right";

export interface FontAsset {
  id: number;
  name: string;
  family: string;
}

export interface AudioAsset {
  id: number;
  name: string;
  duration: number;
  mime: string;
}

export type SoundSync = "event" | "stream";

export interface SoundRef {
  asset: number;
  sync: SoundSync;
  volume?: number;
  loops?: number;
}

/** A sound the platform should be playing (stream) or start now (event). */
export interface SoundCue {
  key: string;
  asset: number;
  sync: SoundSync;
  position: number;
  volume: number;
  loops: number;
}

export interface TextStyle {
  /** null = the bundled default font. */
  font: number | null;
  size: number;
  color: string;
  align: TextAlign;
  letterSpacing: number;
  lineHeight: number;
}
