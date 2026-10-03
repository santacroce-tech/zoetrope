import { useState } from "react";
import { BLEND_MODES, type BlendMode, type ElementInfo, type Engine, type LayerNode, type StageInfo } from "../engine";
import { ColorField, NumberField } from "./fields";

interface Props {
  engine: Engine;
  version: number;
  selection: number[];
  stage: StageInfo;
  layers: LayerNode[];
  /** Runs a core command, reporting errors and refreshing views. */
  run: (fn: () => unknown) => void;
}

const BLEND_LABELS: Record<BlendMode, string> = {
  normal: "Normal",
  layer: "Layer",
  multiply: "Multiply",
  screen: "Screen",
  overlay: "Overlay",
  darken: "Darken",
  lighten: "Lighten",
  hardLight: "Hard light",
  difference: "Difference",
  add: "Add",
};

function contentLayers(nodes: LayerNode[], prefix = ""): { id: number; label: string }[] {
  return nodes.flatMap((n) =>
    n.kind === "folder" ? contentLayers(n.children, `${prefix}${n.name} / `) : [{ id: n.id, label: prefix + n.name }],
  );
}

function kindLabel(info: ElementInfo): string {
  const e = info.element;
  if (e.type === "bitmap") return `Bitmap — ${info.sourceName ?? "?"}`;
  if (e.type === "instance") return `Instance of ${info.sourceName ?? "?"}`;
  return { rect: "Rectangle", ellipse: "Ellipse", line: "Line" }[e.geometry!.kind];
}

export function PropertiesPanel({ engine, selection, stage, layers, run }: Props) {
  const [toStage, setToStage] = useState(false);
  const infos: ElementInfo[] = selection.map((id) => JSON.parse(engine.elementJson(id))).filter(Boolean);
  const ids = JSON.stringify(selection);
  const patch = (p: object) => run(() => engine.patchElements(ids, JSON.stringify(p)));

  if (infos.length === 0) return <StagePanel engine={engine} stage={stage} run={run} />;

  const first = infos[0];
  const e = first.element;
  const t = e.transform;
  const single = infos.length === 1;
  const allShapes = infos.every((i) => i.element.type === "shape");
  const closedShapes = allShapes && infos.every((i) => i.element.geometry?.kind !== "line");
  const opacity = e.opacity ?? 1;
  const blend = e.blend ?? "normal";

  return (
    <div className="props">
      <div className="panel-title">{single ? kindLabel(first) : `${infos.length} objects`}</div>

      {single && (
        <section>
          <label className="field wide">
            <span className="field-label">Name</span>
            <input
              key={`${e.id}:${e.name}`}
              defaultValue={e.name}
              placeholder="(unnamed)"
              onBlur={(ev) => ev.target.value !== e.name && patch({ name: ev.target.value })}
              onKeyDown={(ev) => ev.key === "Enter" && (ev.target as HTMLInputElement).blur()}
            />
          </label>
        </section>
      )}

      {single && (
        <section>
          <h4>Position &amp; size</h4>
          <div className="grid2">
            <NumberField label="X" value={t.x} onCommit={(x) => patch({ transform: { x } })} />
            <NumberField label="Y" value={t.y} onCommit={(y) => patch({ transform: { y } })} />
            <NumberField
              label="W"
              value={first.contentWidth * Math.abs(t.scaleX)}
              min={0.01}
              disabled={first.contentWidth === 0}
              onCommit={(w) => run(() => engine.setElementSize(e.id, w, first.contentHeight * Math.abs(t.scaleY)))}
            />
            <NumberField
              label="H"
              value={first.contentHeight * Math.abs(t.scaleY)}
              min={0.01}
              disabled={first.contentHeight === 0}
              onCommit={(h) => run(() => engine.setElementSize(e.id, first.contentWidth * Math.abs(t.scaleX), h))}
            />
          </div>
          <h4>Transform</h4>
          <div className="grid2">
            <NumberField label="Scale X" suffix="%" value={t.scaleX * 100} onCommit={(v) => v !== 0 && patch({ transform: { scaleX: v / 100 } })} />
            <NumberField label="Scale Y" suffix="%" value={t.scaleY * 100} onCommit={(v) => v !== 0 && patch({ transform: { scaleY: v / 100 } })} />
            <NumberField label="Rotate" suffix="°" value={t.rotation} onCommit={(rotation) => patch({ transform: { rotation } })} />
            <span />
            <NumberField label="Skew X" suffix="°" min={-89} max={89} value={t.skewX} onCommit={(skewX) => patch({ transform: { skewX } })} />
            <NumberField label="Skew Y" suffix="°" min={-89} max={89} value={t.skewY} onCommit={(skewY) => patch({ transform: { skewY } })} />
            <NumberField
              label="Pivot X"
              title="Anchor point in the object's own coordinates (0 = center)"
              value={t.pivotX}
              onCommit={(pivotX) => patch({ transform: { pivotX } })}
            />
            <NumberField label="Pivot Y" value={t.pivotY} onCommit={(pivotY) => patch({ transform: { pivotY } })} />
          </div>
        </section>
      )}

      <section>
        <h4>Appearance</h4>
        <div className="grid2">
          <NumberField label="Opacity" suffix="%" precision={0} min={0} max={100} value={opacity * 100} onCommit={(v) => patch({ opacity: v / 100 })} />
          <label className="field">
            <span className="field-label">Blend</span>
            <select value={blend} onChange={(ev) => patch({ blend: ev.target.value })}>
              {BLEND_MODES.map((b) => (
                <option key={b} value={b}>
                  {BLEND_LABELS[b]}
                </option>
              ))}
            </select>
          </label>
        </div>
        <div className="row">
          <label className="check">
            <input
              type="checkbox"
              checked={!!e.tint}
              onChange={(ev) => patch({ tint: ev.target.checked ? { color: "#ff0000", amount: 0.5 } : null })}
            />
            Tint
          </label>
          {e.tint && (
            <>
              <ColorField value={e.tint.color} onCommit={(color) => patch({ tint: { ...e.tint!, color } })} />
              <NumberField
                label=""
                suffix="%"
                precision={0}
                min={0}
                max={100}
                value={e.tint.amount * 100}
                onCommit={(v) => patch({ tint: { ...e.tint!, amount: v / 100 } })}
              />
            </>
          )}
        </div>
      </section>

      {allShapes && (
        <section>
          <h4>Fill &amp; stroke</h4>
          {closedShapes && (
            <div className="row">
              <label className="check">
                <input
                  type="checkbox"
                  checked={!!e.fill}
                  onChange={(ev) => patch({ fill: ev.target.checked ? { type: "solid", color: "#888888" } : null })}
                />
                Fill
              </label>
              {e.fill && <ColorField value={e.fill.color} onCommit={(color) => patch({ fill: { type: "solid", color } })} />}
            </div>
          )}
          <div className="row">
            <label className="check">
              <input
                type="checkbox"
                checked={!!e.stroke}
                onChange={(ev) => patch({ stroke: ev.target.checked ? { width: 1, color: "#000000" } : null })}
              />
              Stroke
            </label>
            {e.stroke && (
              <>
                <ColorField value={e.stroke.color} onCommit={(color) => patch({ stroke: { ...e.stroke!, color } })} />
                <NumberField label="" suffix="px" min={0} value={e.stroke.width} onCommit={(width) => patch({ stroke: { ...e.stroke!, width } })} />
              </>
            )}
          </div>
        </section>
      )}

      <section>
        <h4>Layer &amp; order</h4>
        <label className="field wide">
          <span className="field-label">Layer</span>
          <select
            value={infos.every((i) => i.layer === first.layer) ? first.layer : ""}
            onChange={(ev) => run(() => engine.moveToLayer(ids, Number(ev.target.value)))}
          >
            {!infos.every((i) => i.layer === first.layer) && <option value="">(mixed)</option>}
            {contentLayers(layers).map((l) => (
              <option key={l.id} value={l.id}>
                {l.label}
              </option>
            ))}
          </select>
        </label>
        <div className="btn-row">
          {(
            [
              ["front", "Bring to front", "⤒"],
              ["forward", "Bring forward", "↑"],
              ["backward", "Send backward", "↓"],
              ["back", "Send to back", "⤓"],
            ] as const
          ).map(([op, label, glyph]) => (
            <button key={op} title={label} onClick={() => run(() => engine.arrange(ids, op))}>
              {glyph}
            </button>
          ))}
        </div>
      </section>

      <section>
        <h4>
          Align
          <label className="check small">
            <input type="checkbox" checked={toStage || single} disabled={single} onChange={(ev) => setToStage(ev.target.checked)} />
            to stage
          </label>
        </h4>
        <div className="btn-row">
          {(
            [
              ["left", "Align left", "⇤"],
              ["centerX", "Align horizontal centers", "↔"],
              ["right", "Align right", "⇥"],
              ["top", "Align top", "⤒"],
              ["centerY", "Align vertical centers", "↕"],
              ["bottom", "Align bottom", "⤓"],
            ] as const
          ).map(([mode, label, glyph]) => (
            <button key={mode} title={label} onClick={() => run(() => engine.align(ids, mode, toStage))}>
              {glyph}
            </button>
          ))}
        </div>
        <div className="btn-row">
          {(
            [
              ["centersX", "Distribute horizontal centers", "⋯"],
              ["spaceX", "Space evenly horizontally", "↔="],
              ["centersY", "Distribute vertical centers", "⋮"],
              ["spaceY", "Space evenly vertically", "↕="],
            ] as const
          ).map(([mode, label, glyph]) => (
            <button
              key={mode}
              title={label}
              disabled={infos.length < (toStage ? 2 : 3)}
              onClick={() => run(() => engine.distribute(ids, mode, toStage))}
            >
              {glyph}
            </button>
          ))}
        </div>
      </section>
    </div>
  );
}

function StagePanel({ engine, stage, run }: { engine: Engine; stage: StageInfo; run: Props["run"] }) {
  const set = (p: Partial<StageInfo>) => run(() => engine.setStage(JSON.stringify({ ...stage, ...p })));
  return (
    <div className="props">
      <div className="panel-title">Stage</div>
      <section>
        <div className="grid2">
          <NumberField label="Width" suffix="px" precision={0} min={1} max={16384} value={stage.width} onCommit={(width) => set({ width })} />
          <NumberField label="Height" suffix="px" precision={0} min={1} max={16384} value={stage.height} onCommit={(height) => set({ height })} />
          <NumberField label="FPS" precision={2} min={1} max={240} value={stage.fps} onCommit={(fps) => set({ fps })} />
          <label className="field">
            <span className="field-label">Color</span>
            <ColorField value={stage.background} onCommit={(background) => set({ background })} />
          </label>
        </div>
      </section>
      <p className="hint">
        Select objects with the arrow tool (V) or draw with R / O / N. Drag handles to scale, just outside corners to rotate,
        edges to skew, and the white dot to move the pivot.
      </p>
    </div>
  );
}
