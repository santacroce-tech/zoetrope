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
  elementCount: number;
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
  type: "shape" | "instance" | "bitmap";
  geometry?: { kind: "rect" | "ellipse" | "line"; width?: number; height?: number; dx?: number; dy?: number };
  fill?: { type: "solid"; color: string };
  stroke?: { width: number; color: string };
  symbol?: number;
  asset?: number;
}

export interface ElementInfo {
  element: ElementData;
  layer: number;
  contentWidth: number;
  contentHeight: number;
  bounds: Rect | null;
  sourceName: string | null;
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

export type ShapeTool = "rect" | "ellipse" | "line";

export interface ShapeDrag {
  geometry: NonNullable<ElementData["geometry"]>;
  center: Pt;
}

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
