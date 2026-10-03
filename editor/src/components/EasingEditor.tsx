import { useMemo, useRef, useState } from "react";
import { EASE_PRESETS, type EasePreset, type Easing, type Engine } from "../engine";

interface Props {
  engine: Engine;
  easing: Easing;
  /** Called once per finished change (select, drag end, field commit). */
  onChange: (e: Easing) => void;
}

const W = 200;
const H = 150;
const PAD = 14;
/** Plotted y range (room for back/elastic overshoot). */
const Y_MIN = -0.5;
const Y_MAX = 1.5;

const px = (x: number) => PAD + x * (W - 2 * PAD);
const py = (y: number) => H - PAD - ((y - Y_MIN) / (Y_MAX - Y_MIN)) * (H - 2 * PAD);
const ux = (sx: number) => Math.min(1, Math.max(0, (sx - PAD) / (W - 2 * PAD)));
const uy = (sy: number) => Math.min(Y_MAX, Math.max(Y_MIN, Y_MIN + ((H - PAD - sy) / (H - 2 * PAD)) * (Y_MAX - Y_MIN)));

const label = (name: string) => name.replace(/^ease/, "").replace(/([A-Z])/g, " $1").trim();

/**
 * Easing picker: presets plus a custom cubic-bezier with draggable control
 * points. The curve itself is sampled by the core, so what's drawn is exactly
 * what plays.
 */
export function EasingEditor({ engine, easing, onChange }: Props) {
  // Local copy while dragging; committed on pointer-up.
  const [draft, setDraft] = useState<Easing | null>(null);
  const shown = draft ?? easing;
  const svg = useRef<SVGSVGElement>(null);
  const dragging = useRef<1 | 2 | null>(null);

  const curve = useMemo(() => {
    const ys: number[] = JSON.parse(engine.easingCurveJson(JSON.stringify(shown), 64));
    return ys.map((y, i) => `${px(i / 64).toFixed(1)},${py(y).toFixed(1)}`).join(" ");
  }, [engine, shown]);

  const select = shown.type === "linear" ? "linear" : shown.type === "preset" ? shown.name : "bezier";

  const onSelect = (v: string) => {
    if (v === "linear") onChange({ type: "linear" });
    else if (v === "bezier") onChange({ type: "bezier", x1: 0.42, y1: 0, x2: 0.58, y2: 1 });
    else onChange({ type: "preset", name: v as EasePreset });
  };

  const point = (e: React.PointerEvent) => {
    const r = svg.current!.getBoundingClientRect();
    return { x: ux(((e.clientX - r.left) / r.width) * W), y: uy(((e.clientY - r.top) / r.height) * H) };
  };

  const onMove = (e: React.PointerEvent) => {
    if (!dragging.current || shown.type !== "bezier") return;
    const p = point(e);
    const r = (v: number) => Math.round(v * 100) / 100;
    setDraft(dragging.current === 1 ? { ...shown, x1: r(p.x), y1: r(p.y) } : { ...shown, x2: r(p.x), y2: r(p.y) });
  };

  const onUp = () => {
    dragging.current = null;
    if (draft) onChange(draft);
    setDraft(null);
  };

  const bez = shown.type === "bezier" ? shown : null;

  return (
    <div className="easing-editor">
      <select value={select} onChange={(e) => onSelect(e.target.value)}>
        <option value="linear">Linear</option>
        {EASE_PRESETS.map((p) => (
          <option key={p} value={p}>
            {label(p)}
          </option>
        ))}
        <option value="bezier">Custom (bezier)</option>
      </select>
      <svg
        ref={svg}
        viewBox={`0 0 ${W} ${H}`}
        className="easing-plot"
        onPointerMove={onMove}
        onPointerUp={onUp}
        onPointerLeave={() => dragging.current && onUp()}
      >
        <rect x={px(0)} y={py(1)} width={px(1) - px(0)} height={py(0) - py(1)} className="unit" />
        <line x1={px(0)} y1={py(0)} x2={px(1)} y2={py(1)} className="diag" />
        {bez && (
          <>
            <line x1={px(0)} y1={py(0)} x2={px(bez.x1)} y2={py(bez.y1)} className="arm" />
            <line x1={px(1)} y1={py(1)} x2={px(bez.x2)} y2={py(bez.y2)} className="arm" />
          </>
        )}
        <polyline points={curve} className="curve" />
        {bez && (
          <>
            <circle
              cx={px(bez.x1)}
              cy={py(bez.y1)}
              r={5}
              className="cp"
              onPointerDown={(e) => {
                (e.target as Element).setPointerCapture(e.pointerId);
                dragging.current = 1;
              }}
            />
            <circle
              cx={px(bez.x2)}
              cy={py(bez.y2)}
              r={5}
              className="cp"
              onPointerDown={(e) => {
                (e.target as Element).setPointerCapture(e.pointerId);
                dragging.current = 2;
              }}
            />
          </>
        )}
      </svg>
      {bez && (
        <div className="bezier-values">
          cubic-bezier({bez.x1}, {bez.y1}, {bez.x2}, {bez.y2})
        </div>
      )}
    </div>
  );
}
