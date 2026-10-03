import { ColorField, NumberField } from "./fields";
import type { ShapeStyle, Tool } from "./StageView";

interface Props {
  tool: Tool;
  onTool: (t: Tool) => void;
  style: ShapeStyle;
  onStyle: (s: ShapeStyle) => void;
}

const TOOLS: { id: Tool; key: string; label: string; glyph: string }[] = [
  { id: "select", key: "V", label: "Selection", glyph: "➤" },
  { id: "rect", key: "R", label: "Rectangle", glyph: "▭" },
  { id: "ellipse", key: "O", label: "Ellipse", glyph: "◯" },
  { id: "line", key: "N", label: "Line", glyph: "╱" },
  { id: "hand", key: "H", label: "Hand (or hold Space)", glyph: "✋" },
];

export const TOOL_KEYS: Record<string, Tool> = Object.fromEntries(TOOLS.map((t) => [t.key.toLowerCase(), t.id]));

/** Left tool strip plus the style used for newly drawn shapes. */
export function ToolPalette({ tool, onTool, style, onStyle }: Props) {
  return (
    <nav className="tools">
      {TOOLS.map((t) => (
        <button key={t.id} className={tool === t.id ? "active" : ""} title={`${t.label} (${t.key})`} onClick={() => onTool(t.id)}>
          {t.glyph}
        </button>
      ))}
      <div className="tool-sep" />
      <StyleSwatch
        label="Fill"
        color={style.fill}
        fallback="#e86a92"
        onChange={(fill) => onStyle({ ...style, fill })}
      />
      <StyleSwatch
        label="Stroke"
        color={style.stroke}
        fallback="#222222"
        onChange={(stroke) => onStyle({ ...style, stroke })}
      />
      <div className="stroke-width" title="Stroke width for new shapes">
        <NumberField label="" value={style.strokeWidth} min={0} max={200} precision={1} onCommit={(strokeWidth) => onStyle({ ...style, strokeWidth })} />
      </div>
    </nav>
  );
}

function StyleSwatch(props: { label: string; color: string | null; fallback: string; onChange: (c: string | null) => void }) {
  const { label, color, fallback, onChange } = props;
  return (
    <div className="style-swatch" title={`${label} for new shapes`}>
      <span className="style-label">{label}</span>
      <ColorField value={color ?? fallback} onCommit={onChange} disabled={color === null} />
      <button className={`none ${color === null ? "on" : ""}`} title={`No ${label.toLowerCase()}`} onClick={() => onChange(color === null ? fallback : null)}>
        ⌀
      </button>
    </div>
  );
}
