// Editor viewport math (zoom/pan) and chrome drawing helpers. This is UI
// chrome only — scene geometry always comes from the core.
import type { Pt } from "./engine";

/** Screen (CSS px, relative to the viewport) = stage * zoom + pan. */
export interface View {
  zoom: number;
  panX: number;
  panY: number;
}

export const MIN_ZOOM = 0.05;
export const MAX_ZOOM = 32;
const FIT_PADDING = 40;

export function fitView(vw: number, vh: number, sw: number, sh: number): View {
  const zoom = clampZoom(Math.min((vw - 2 * FIT_PADDING) / sw, (vh - 2 * FIT_PADDING) / sh));
  return { zoom, panX: (vw - sw * zoom) / 2, panY: (vh - sh * zoom) / 2 };
}

export function clampZoom(z: number): number {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z));
}

/** Zooms so the stage point under screen point `s` stays put. */
export function zoomAt(v: View, s: Pt, zoom: number): View {
  const z = clampZoom(zoom);
  return { zoom: z, panX: s.x - ((s.x - v.panX) * z) / v.zoom, panY: s.y - ((s.y - v.panY) * z) / v.zoom };
}

export const toStage = (v: View, s: Pt): Pt => ({ x: (s.x - v.panX) / v.zoom, y: (s.y - v.panY) / v.zoom });
export const toScreen = (v: View, p: Pt): Pt => ({ x: p.x * v.zoom + v.panX, y: p.y * v.zoom + v.panY });

export const dist = (a: Pt, b: Pt) => Math.hypot(a.x - b.x, a.y - b.y);
export const mid = (a: Pt, b: Pt): Pt => ({ x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });

export function segmentDistance(p: Pt, a: Pt, b: Pt): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len2 = dx * dx + dy * dy;
  if (len2 === 0) return dist(p, a);
  const t = Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2));
  return dist(p, { x: a.x + t * dx, y: a.y + t * dy });
}

export function pointInPolygon(p: Pt, poly: Pt[]): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const a = poly[i];
    const b = poly[j];
    if (a.y > p.y !== b.y > p.y && p.x < ((b.x - a.x) * (p.y - a.y)) / (b.y - a.y) + a.x) inside = !inside;
  }
  return inside;
}

/** A "nice" ruler/grid step (1, 2, 5 × 10ⁿ) giving at least `minPx` on screen. */
export function niceStep(zoom: number, minPx: number): number {
  const raw = minPx / zoom;
  const pow = Math.pow(10, Math.floor(Math.log10(raw)));
  for (const m of [1, 2, 5, 10]) if (m * pow >= raw) return m * pow;
  return 10 * pow;
}

export function drawRuler(
  ctx: CanvasRenderingContext2D,
  horizontal: boolean,
  length: number,
  thickness: number,
  view: View,
  cursor: Pt | null,
  colors: { bg: string; tick: string; text: string; cursor: string },
) {
  const pan = horizontal ? view.panX : view.panY;
  ctx.fillStyle = colors.bg;
  ctx.fillRect(0, 0, horizontal ? length : thickness, horizontal ? thickness : length);
  const major = niceStep(view.zoom, 60);
  const minor = major / (major * view.zoom >= 100 ? 10 : 5);
  const first = Math.floor(-pan / view.zoom / minor) * minor;
  const last = (length - pan) / view.zoom;
  ctx.strokeStyle = colors.tick;
  ctx.fillStyle = colors.text;
  ctx.font = "9px -apple-system, system-ui, sans-serif";
  ctx.lineWidth = 1;
  ctx.beginPath();
  for (let v = first; v <= last; v += minor) {
    const s = Math.round(v * view.zoom + pan) + 0.5;
    const isMajor = Math.abs(v / major - Math.round(v / major)) < 1e-6;
    const len = isMajor ? thickness : thickness * 0.3;
    if (horizontal) {
      ctx.moveTo(s, thickness);
      ctx.lineTo(s, thickness - len);
    } else {
      ctx.moveTo(thickness, s);
      ctx.lineTo(thickness - len, s);
    }
    if (isMajor) {
      const label = String(Math.round(v));
      if (horizontal) ctx.fillText(label, s + 2, 9);
      else {
        ctx.save();
        ctx.translate(9, s + 2);
        ctx.rotate(-Math.PI / 2);
        ctx.textAlign = "right";
        ctx.fillText(label, 0, 0);
        ctx.restore();
      }
    }
  }
  ctx.stroke();
  if (cursor) {
    const s = Math.round((horizontal ? cursor.x : cursor.y) * view.zoom + pan) + 0.5;
    ctx.strokeStyle = colors.cursor;
    ctx.beginPath();
    if (horizontal) {
      ctx.moveTo(s, 0);
      ctx.lineTo(s, thickness);
    } else {
      ctx.moveTo(0, s);
      ctx.lineTo(thickness, s);
    }
    ctx.stroke();
  }
}

/** Sizes a canvas's backing store to its CSS box at the device pixel ratio. */
export function fitCanvas(canvas: HTMLCanvasElement, w: number, h: number): number {
  const dpr = window.devicePixelRatio || 1;
  const bw = Math.max(1, Math.round(w * dpr));
  const bh = Math.max(1, Math.round(h * dpr));
  if (canvas.width !== bw || canvas.height !== bh) {
    canvas.width = bw;
    canvas.height = bh;
  }
  return dpr;
}
