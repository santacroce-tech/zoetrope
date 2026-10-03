import { useEffect, useMemo, useRef, useState } from "react";
import type { LayerNode, Onion } from "../engine";
import { fitCanvas } from "../view";
import { flattenLayers, LayersPanel, type LayerPatch, type LayerRow } from "./LayersPanel";

export type FrameOp = "frame" | "removeFrame" | "key" | "blank" | "clear" | "motion" | "shape" | "noTween";

export interface OnionSettings extends Onion {
  enabled: boolean;
}

interface Props {
  layers: LayerNode[];
  length: number;
  frame: number;
  onFrame: (f: number) => void;
  playing: boolean;
  onPlaying: (p: boolean) => void;
  loop: boolean;
  onLoop: (l: boolean) => void;
  onion: OnionSettings;
  onOnion: (o: OnionSettings) => void;
  activeLayer: number | null;
  onActivate: (id: number) => void;
  selectedLayers: Set<number>;
  /** The frame cell last clicked (drives the frame properties panel). */
  focus: { layer: number; frame: number } | null;
  onFocus: (layer: number, frame: number) => void;
  onFrameOp: (op: FrameOp) => void;
  onAddLayer: (kind: "normal" | "folder") => void;
  onDeleteLayer: (id: number) => void;
  onPatchLayer: (id: number, patch: LayerPatch) => void;
  onMoveLayer: (id: number, parent: number | null, index: number) => void;
}

const CELL = 10;
const ROW = 22;
const HEADER = 20;

const SPAN = { static: "rgba(160,160,180,0.22)", motion: "rgba(150,135,235,0.55)", shape: "rgba(100,195,110,0.55)" };

/** Flash-style timeline: layer column + frame grid with keyframes, spans and tweens. */
export function Timeline(props: Props) {
  const { layers, length, frame, onion } = props;
  const [collapsed, setCollapsed] = useState<Set<number>>(new Set());
  const rows = useMemo(() => flattenLayers(layers, collapsed), [layers, collapsed]);
  const allRows = useMemo(() => flattenLayers(layers, new Set()), [layers]);
  const scroller = useRef<HTMLDivElement>(null);
  const header = useRef<HTMLCanvasElement>(null);
  const cells = useRef<HTMLCanvasElement>(null);
  const [viewW, setViewW] = useState(800);
  const frames = Math.max(length + 30, Math.ceil(viewW / CELL) + 1);
  const active = allRows.find((r) => r.node.id === props.activeLayer)?.node;

  useEffect(() => {
    const el = scroller.current!;
    const ro = new ResizeObserver(() => setViewW(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Keep the playhead visible while playing / stepping.
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const x = frame * CELL;
    const left = el.scrollLeft;
    const visible = el.clientWidth - 230;
    if (x < left || x > left + visible - CELL) el.scrollLeft = Math.max(0, x - visible / 2);
  }, [frame]);

  useEffect(() => {
    drawHeader(header.current!, frames, frame, length);
    drawCells(cells.current!, rows, frames, frame, props.focus, props.activeLayer);
  }, [rows, frames, frame, length, props.focus, props.activeLayer]);

  const frameAt = (e: React.PointerEvent) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    return Math.max(0, Math.min(frames - 1, Math.floor((e.clientX - r.left) / CELL)));
  };

  const scrub = (e: React.PointerEvent) => {
    props.onPlaying(false);
    props.onFrame(frameAt(e));
  };

  const toggle = (id: number) =>
    setCollapsed((c) => {
      const n = new Set(c);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  return (
    <section className="timeline">
      <div className="tl-toolbar">
        <div className="group first">
          <button title="First frame (Home)" onClick={() => (props.onPlaying(false), props.onFrame(0))}>⏮</button>
          <button title="Previous frame (,)" onClick={() => (props.onPlaying(false), props.onFrame(Math.max(0, frame - 1)))}>◀</button>
          <button className={props.playing ? "on" : ""} title="Play / pause (Enter)" onClick={() => props.onPlaying(!props.playing)}>
            {props.playing ? "⏸" : "▶"}
          </button>
          <button title="Next frame (.)" onClick={() => (props.onPlaying(false), props.onFrame(frame + 1))}>▶|</button>
          <button title="Last frame (End)" onClick={() => (props.onPlaying(false), props.onFrame(Math.max(0, length - 1)))}>⏭</button>
          <button className={props.loop ? "on" : ""} title="Loop playback" onClick={() => props.onLoop(!props.loop)}>⟲</button>
          <span className="frame-counter" title="Current frame / length">
            {frame + 1} / {length}
          </span>
        </div>
        <div className="group">
          <button className={onion.enabled ? "on" : ""} title="Onion skin" onClick={() => props.onOnion({ ...onion, enabled: !onion.enabled })}>
            Onion
          </button>
          {onion.enabled && (
            <span className="onion-range" title="Frames shown before / after">
              −<input type="number" min={0} max={20} value={onion.before} onChange={(e) => props.onOnion({ ...onion, before: Math.max(0, Number(e.target.value)) })} />
              +<input type="number" min={0} max={20} value={onion.after} onChange={(e) => props.onOnion({ ...onion, after: Math.max(0, Number(e.target.value)) })} />
            </span>
          )}
        </div>
        <div className="group">
          <button title="Insert frame (F5)" onClick={() => props.onFrameOp("frame")}>+Frame</button>
          <button title="Remove frame (⇧F5)" onClick={() => props.onFrameOp("removeFrame")}>−Frame</button>
          <button title="Insert keyframe (F6)" onClick={() => props.onFrameOp("key")}>Keyframe</button>
          <button title="Insert blank keyframe (F7)" onClick={() => props.onFrameOp("blank")}>Blank</button>
          <button title="Clear keyframe (⇧F6)" onClick={() => props.onFrameOp("clear")}>Clear</button>
        </div>
        <div className="group">
          <button title="Create motion tween" onClick={() => props.onFrameOp("motion")}>Motion tween</button>
          <button title="Create shape tween" onClick={() => props.onFrameOp("shape")}>Shape tween</button>
          <button title="Remove tween" onClick={() => props.onFrameOp("noTween")}>No tween</button>
        </div>
        <div className="group layer-actions">
          <button title="New layer" onClick={() => props.onAddLayer("normal")}>＋ Layer</button>
          <button title="New folder" onClick={() => props.onAddLayer("folder")}>＋ Folder</button>
          <button
            title="Toggle guide layer (guides never export)"
            disabled={!active || active.kind === "folder"}
            className={active?.kind === "guide" ? "on" : ""}
            onClick={() => active && props.onPatchLayer(active.id, { kind: active.kind === "guide" ? "normal" : "guide" })}
          >
            Guide
          </button>
          <button title="Delete layer" disabled={!active} onClick={() => active && props.onDeleteLayer(active.id)}>
            🗑
          </button>
        </div>
      </div>
      <div className="tl-scroll" ref={scroller}>
        <div className="tl-grid" style={{ width: 230 + frames * CELL }}>
          <div className="tl-corner">Layers</div>
          <canvas
            ref={header}
            className="tl-header"
            onPointerDown={(e) => {
              (e.target as Element).setPointerCapture(e.pointerId);
              scrub(e);
            }}
            onPointerMove={(e) => e.buttons & 1 && scrub(e)}
          />
          <div className="tl-names">
            <LayersPanel
              rows={rows}
              allRows={allRows}
              collapsed={collapsed}
              onToggleCollapsed={toggle}
              activeLayer={props.activeLayer}
              selectedLayers={props.selectedLayers}
              onActivate={props.onActivate}
              onPatch={props.onPatchLayer}
              onMove={props.onMoveLayer}
            />
          </div>
          <canvas
            ref={cells}
            className="tl-cells"
            onPointerDown={(e) => {
              const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
              const row = rows[Math.floor((e.clientY - r.top) / ROW)];
              if (!row) return;
              const f = frameAt(e);
              props.onPlaying(false);
              props.onActivate(row.node.id);
              props.onFrame(f);
              props.onFocus(row.node.id, f);
            }}
          />
        </div>
      </div>
    </section>
  );
}

function drawHeader(c: HTMLCanvasElement, frames: number, frame: number, length: number) {
  const w = frames * CELL;
  c.style.width = `${w}px`;
  c.style.height = `${HEADER}px`;
  const dpr = fitCanvas(c, w, HEADER);
  const g = c.getContext("2d")!;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.fillStyle = "#24242d";
  g.fillRect(0, 0, w, HEADER);
  g.fillStyle = "rgba(255,255,255,0.04)";
  g.fillRect(0, 0, length * CELL, HEADER);
  g.strokeStyle = "#4a4a58";
  g.fillStyle = "#9a9aab";
  g.font = "9px -apple-system, system-ui, sans-serif";
  g.beginPath();
  for (let i = 0; i < frames; i++) {
    const x = i * CELL + 0.5;
    const major = (i + 1) % 5 === 0 || i === 0;
    g.moveTo(x, HEADER);
    g.lineTo(x, major ? HEADER - 7 : HEADER - 3);
    if (major) g.fillText(String(i + 1), x + 2, 10);
  }
  g.stroke();
  // Playhead.
  g.fillStyle = "rgba(232,106,146,0.85)";
  g.fillRect(frame * CELL, 0, CELL, HEADER);
  g.fillStyle = "#fff";
  g.fillText(String(frame + 1), frame * CELL + 1, 10);
}

function drawCells(
  c: HTMLCanvasElement,
  rows: LayerRow[],
  frames: number,
  frame: number,
  focus: { layer: number; frame: number } | null,
  activeLayer: number | null,
) {
  const w = frames * CELL;
  const h = Math.max(rows.length * ROW, 1);
  c.style.width = `${w}px`;
  c.style.height = `${h}px`;
  const dpr = fitCanvas(c, w, h);
  const g = c.getContext("2d")!;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.fillStyle = "#1e1e26";
  g.fillRect(0, 0, w, h);
  // Every 5th frame column tinted; frame separators.
  g.fillStyle = "rgba(255,255,255,0.025)";
  for (let i = 4; i < frames; i += 5) g.fillRect(i * CELL, 0, CELL, h);
  g.strokeStyle = "#2a2a35";
  g.beginPath();
  for (let i = 0; i <= frames; i++) {
    g.moveTo(i * CELL + 0.5, 0);
    g.lineTo(i * CELL + 0.5, h);
  }
  g.stroke();

  rows.forEach((row, ri) => {
    const y = ri * ROW;
    if (row.node.id === activeLayer) {
      g.fillStyle = "rgba(232,106,146,0.08)";
      g.fillRect(0, y, w, ROW);
    }
    if (row.node.kind === "folder") {
      g.fillStyle = "#2a2a33";
      g.fillRect(0, y, w, ROW);
    }
    for (const k of row.node.keyframes) {
      const x0 = k.start * CELL;
      const x1 = (k.start + k.duration) * CELL;
      g.fillStyle = k.tween ? SPAN[k.tween.kind] : k.empty ? "rgba(0,0,0,0)" : SPAN.static;
      g.fillRect(x0 + 1, y + 2, x1 - x0 - 1, ROW - 4);
      if (k.empty && !k.tween) {
        g.strokeStyle = "#3a3a48";
        g.strokeRect(x0 + 1.5, y + 2.5, x1 - x0 - 2, ROW - 5);
      }
      if (k.tween && k.duration > 1) {
        // Tween arrow.
        const cy = y + ROW / 2;
        g.strokeStyle = "rgba(20,20,30,0.8)";
        g.beginPath();
        g.moveTo(x0 + CELL, cy);
        g.lineTo(x1 - 3, cy);
        g.moveTo(x1 - 7, cy - 3);
        g.lineTo(x1 - 3, cy);
        g.lineTo(x1 - 7, cy + 3);
        g.stroke();
      } else if (k.duration > 1) {
        // End-of-span marker.
        g.strokeStyle = "#8a8a9a";
        g.strokeRect(x1 - CELL + 2.5, y + 7.5, CELL - 5, ROW - 15);
      }
      // Flash-style markers: "a" for a frame script, a flag + name for a label.
      let markX = x0 + CELL + 1;
      if (k.script) {
        g.fillStyle = "#ffd84d";
        g.font = "bold 10px system-ui, sans-serif";
        g.fillText("a", x0 + 2.5, y + 9);
      }
      if (k.label && k.duration > 1) {
        g.fillStyle = "#ff9fc0";
        g.font = "10px system-ui, sans-serif";
        const text = `⚑${k.label}`;
        g.save();
        g.beginPath();
        g.rect(markX, y, x1 - markX - 2, ROW);
        g.clip();
        g.fillText(text, markX, y + ROW / 2 + 4);
        g.restore();
        markX += g.measureText(text).width + 4;
      }
      if (k.sound) {
        // Sound: a line along the span for streams (they play only while it lasts), a note for both.
        g.fillStyle = "#7fd1ff";
        if (k.sound.sync === "stream") g.fillRect(x0 + 2, y + ROW - 5, x1 - x0 - 3, 2);
        if (k.duration > 1) {
          g.font = "10px system-ui, sans-serif";
          if (markX < x1 - 8) g.fillText("♪", markX, y + ROW / 2 + 4);
        }
      }
      // Keyframe dot: filled = has content, hollow = blank.
      g.beginPath();
      g.arc(x0 + CELL / 2 + 0.5, y + ROW / 2, 3.2, 0, Math.PI * 2);
      if (k.empty) {
        g.strokeStyle = "#ccc";
        g.stroke();
      } else {
        g.fillStyle = "#e8e8ee";
        g.fill();
      }
    }
    g.strokeStyle = "#2f2f3a";
    g.beginPath();
    g.moveTo(0, y + ROW - 0.5);
    g.lineTo(w, y + ROW - 0.5);
    g.stroke();
    if (focus && focus.layer === row.node.id) {
      g.fillStyle = "rgba(47,155,255,0.45)";
      g.fillRect(focus.frame * CELL + 1, y + 1, CELL - 1, ROW - 2);
    }
  });
  // Playhead line.
  g.fillStyle = "rgba(232,106,146,0.9)";
  g.fillRect(frame * CELL + CELL / 2, 0, 1, h);
}
