import { cssPaint, type GradientStop } from "../engine";
import { ColorField, NumberField } from "./fields";

interface Props {
  kind: "linear" | "radial";
  stops: GradientStop[];
  /** Called with stops sorted by offset (the core requires ascending order). */
  onChange: (stops: GradientStop[]) => void;
}

const sorted = (stops: GradientStop[]) => [...stops].sort((a, b) => a.offset - b.offset);

/** Gradient stop list: preview bar, offset %, color, add/remove. */
export function GradientEditor({ kind, stops, onChange }: Props) {
  const commit = (next: GradientStop[]) => onChange(sorted(next));
  const add = () => {
    const s = sorted(stops);
    if (s.length < 2) {
      const only = s[0];
      return commit([...s, { offset: only.offset < 0.5 ? 1 : 0, color: only.color }]);
    }
    // Insert midway into the widest gap between neighboring stops.
    let best = 0;
    for (let i = 1; i + 1 < s.length; i++) {
      if (s[i + 1].offset - s[i].offset > s[best + 1].offset - s[best].offset) best = i;
    }
    commit([...s, { offset: (s[best].offset + s[best + 1].offset) / 2, color: s[best].color }]);
  };
  return (
    <div className="gradient-editor">
      <div className="gradient-bar" style={{ background: cssPaint({ type: "linear", stops: sorted(stops) }) }} title={`${kind} gradient`} />
      {stops.map((s, i) => (
        <div className="row stop-row" key={i}>
          <ColorField value={s.color} onCommit={(color) => commit(stops.map((t, j) => (j === i ? { ...t, color } : t)))} />
          <NumberField
            label=""
            suffix="%"
            precision={1}
            min={0}
            max={100}
            value={s.offset * 100}
            onCommit={(v) => commit(stops.map((t, j) => (j === i ? { ...t, offset: v / 100 } : t)))}
          />
          <button className="mini" title="Remove stop" disabled={stops.length <= 1} onClick={() => commit(stops.filter((_, j) => j !== i))}>
            ✕
          </button>
        </div>
      ))}
      <button className="mini add-stop" onClick={add}>
        ＋ stop
      </button>
    </div>
  );
}
