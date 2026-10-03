import { cssPaint, type PaintStyle, type ShapeStyle } from "../engine";
import { ColorField, NumberField } from "./fields";
import type { Tool, ToolOptions } from "./StageView";

interface Props {
  tool: Tool;
  onTool: (t: Tool) => void;
  style: ShapeStyle;
  onStyle: (s: ShapeStyle) => void;
}

const TOOLS: { id: Tool; key: string; label: string; glyph: string }[] = [
  { id: "select", key: "V", label: "Selection", glyph: "➤" },
  { id: "subselect", key: "A", label: "Subselection (anchors & handles; ⌥-click a segment to add an anchor, double-click to convert)", glyph: "⇱" },
  { id: "pen", key: "P", label: "Pen (click: corner, drag: curve, click first anchor: close, Enter/Esc: finish)", glyph: "✒" },
  { id: "pencil", key: "Y", label: "Pencil", glyph: "✎" },
  { id: "rect", key: "R", label: "Rectangle", glyph: "▭" },
  { id: "ellipse", key: "O", label: "Ellipse", glyph: "◯" },
  { id: "polygon", key: "U", label: "Polygon / star (drag from center)", glyph: "⬠" },
  { id: "line", key: "N", label: "Line", glyph: "╱" },
  { id: "gradient", key: "F", label: "Gradient transform", glyph: "◐" },
  { id: "eyedropper", key: "I", label: "Eyedropper (picks fill or stroke)", glyph: "💧" },
  { id: "bucket", key: "K", label: "Paint bucket (⇧: apply stroke color)", glyph: "🪣" },
  { id: "hand", key: "H", label: "Hand (or hold Space)", glyph: "✋" },
];

export const TOOL_KEYS: Record<string, Tool> = Object.fromEntries(TOOLS.map((t) => [t.key.toLowerCase(), t.id]));

/** First color of a paint style (solid color or first gradient stop). */
export function primaryColor(p: PaintStyle | null | undefined, fallback: string): string {
  if (!p) return fallback;
  return p.type === "solid" ? p.color : (p.stops[0]?.color ?? fallback);
}

/** Cycles solid → linear → radial → solid, keeping the colors. */
function cyclePaint(p: PaintStyle): PaintStyle {
  const stops = p.type === "solid" ? [{ offset: 0, color: p.color }, { offset: 1, color: "#ffffff" }] : p.stops;
  if (p.type === "solid") return { type: "linear", stops };
  if (p.type === "linear") return { type: "radial", stops };
  return { type: "solid", color: stops[0].color };
}

function withPrimary(p: PaintStyle | null, color: string): PaintStyle {
  if (!p || p.type === "solid") return { type: "solid", color };
  return { ...p, stops: p.stops.map((s, i) => (i === 0 ? { ...s, color } : s)) };
}

/** Left tool strip plus the style used for newly drawn shapes. */
export function ToolPalette({ tool, onTool, style, onStyle }: Props) {
  const fill = style.fill;
  const stroke = style.stroke;
  return (
    <nav className="tools">
      {TOOLS.map((t) => (
        <button key={t.id} className={tool === t.id ? "active" : ""} title={`${t.label} (${t.key})`} onClick={() => onTool(t.id)}>
          {t.glyph}
        </button>
      ))}
      <div className="tool-sep" />
      <div className="style-swatch" title="Fill for new shapes">
        <span className="style-label">Fill</span>
        <div className="paint-preview" style={{ background: cssPaint(fill) }} />
        <div className="swatch-row">
          <ColorField value={primaryColor(fill, "#e86a92")} onCommit={(c) => onStyle({ ...style, fill: withPrimary(fill, c) })} />
        </div>
        <div className="swatch-row">
          <button
            className="mini"
            title="Solid / linear / radial"
            disabled={!fill}
            onClick={() => fill && onStyle({ ...style, fill: cyclePaint(fill) })}
          >
            {fill?.type === "linear" ? "▤" : fill?.type === "radial" ? "◎" : "■"}
          </button>
          <button
            className={`mini ${fill ? "" : "on"}`}
            title="No fill"
            onClick={() => onStyle({ ...style, fill: fill ? null : { type: "solid", color: "#e86a92" } })}
          >
            ⌀
          </button>
        </div>
      </div>
      <div className="style-swatch" title="Stroke for new shapes">
        <span className="style-label">Stroke</span>
        <div className="swatch-row">
          <ColorField
            value={stroke?.color ?? "#222222"}
            disabled={!stroke}
            onCommit={(color) => stroke && onStyle({ ...style, stroke: { ...stroke, color } })}
          />
        </div>
        <div className="swatch-row">
          <button
            className={`mini ${stroke ? "" : "on"}`}
            title="No stroke"
            onClick={() =>
              onStyle({ ...style, stroke: stroke ? null : { color: "#222222", width: 2, cap: "round", join: "round", dash: [] } })
            }
          >
            ⌀
          </button>
        </div>
        <div className="stroke-width" title="Stroke width for new shapes">
          <NumberField
            label=""
            value={stroke?.width ?? 0}
            disabled={!stroke}
            min={0}
            max={200}
            precision={1}
            onCommit={(width) => stroke && onStyle({ ...style, stroke: { ...stroke, width } })}
          />
        </div>
      </div>
    </nav>
  );
}

/** Contextual options for the current tool (shown in the top toolbar). */
export function ToolOptionsBar({ tool, options, onOptions }: { tool: Tool; options: ToolOptions; onOptions: (o: ToolOptions) => void }) {
  if (tool === "polygon") {
    return (
      <div className="group tool-options">
        <NumberField label="Sides" precision={0} min={3} max={64} value={options.sides} onCommit={(sides) => onOptions({ ...options, sides })} />
        <label className="check">
          <input type="checkbox" checked={options.star !== null} onChange={(e) => onOptions({ ...options, star: e.target.checked ? 0.5 : null })} />
          Star
        </label>
        {options.star !== null && (
          <NumberField
            label="Inner"
            suffix="%"
            precision={0}
            min={5}
            max={95}
            value={options.star * 100}
            onCommit={(v) => onOptions({ ...options, star: v / 100 })}
          />
        )}
      </div>
    );
  }
  if (tool === "pencil") {
    return (
      <div className="group tool-options">
        <span className="field-label">Pencil</span>
        <select value={options.pencilSmooth ? "smooth" : "ink"} onChange={(e) => onOptions({ ...options, pencilSmooth: e.target.value === "smooth" })}>
          <option value="smooth">Smooth</option>
          <option value="ink">Ink (straight segments)</option>
        </select>
      </div>
    );
  }
  return null;
}
