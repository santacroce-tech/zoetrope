import { useState } from "react";
import {
  BLEND_MODES,
  type BlendMode,
  type ElementInfo,
  type Engine,
  type GradientStop,
  type LayerNode,
  type Paint,
  type PaintStyle,
  type StageInfo,
  type StrokeData,
  type Tween,
  type TweenKind,
} from "../engine";
import { ColorField, NumberField } from "./fields";
import { GradientEditor } from "./GradientEditor";
import { EasingEditor } from "./EasingEditor";

interface Props {
  engine: Engine;
  version: number;
  selection: number[];
  stage: StageInfo;
  layers: LayerNode[];
  /** Timeline cell last clicked; shows frame/tween properties when set. */
  frameTarget: { layer: LayerNode; frame: number } | null;
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
  return { rect: "Rectangle", ellipse: "Ellipse", line: "Line", path: "Path" }[e.geometry!.kind];
}

export function PropertiesPanel({ engine, selection, stage, layers, frameTarget, run }: Props) {
  const [toStage, setToStage] = useState(false);
  const infos: ElementInfo[] = selection.map((id) => JSON.parse(engine.elementJson(id))).filter(Boolean);
  const ids = JSON.stringify(selection);
  const patch = (p: object) => run(() => engine.patchElements(ids, JSON.stringify(p)));

  if (frameTarget) return <FramePanel engine={engine} target={frameTarget} run={run} />;
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
      {infos.some((i) => i.tweened) && (
        <p className="hint warn">This frame is tweened: values below edit the tween's start keyframe. Insert a keyframe here (F6) to pose it.</p>
      )}

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
        <FillStrokeSection engine={engine} infos={infos} closedShapes={closedShapes} run={run} />
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

type PaintKind = "none" | "solid" | "linear" | "radial";

function paintColor(p: Paint | undefined, fallback: string): string {
  if (!p) return fallback;
  return p.type === "solid" ? p.color : (p.stops[0]?.color ?? fallback);
}

function styleFor(kind: PaintKind, current: Paint | undefined, fallback: string): PaintStyle | null {
  if (kind === "none") return null;
  const color = paintColor(current, fallback);
  if (kind === "solid") return { type: "solid", color };
  const stops: GradientStop[] =
    current && current.type !== "solid" ? current.stops : [{ offset: 0, color }, { offset: 1, color: "#ffffff" }];
  return { type: kind, stops };
}

const DASH_PRESETS: Record<string, number[]> = { solid: [], dashed: [8, 4], dotted: [0.01, 4], "dash-dot": [10, 4, 0.01, 4] };

function FillStrokeSection(props: { engine: Engine; infos: ElementInfo[]; closedShapes: boolean; run: Props["run"] }) {
  const { engine, infos, closedShapes, run } = props;
  const ids = JSON.stringify(infos.map((i) => i.element.id));
  const single = infos.length === 1;
  const e = infos[0].element;
  const fill = e.fill;
  const stroke = e.stroke as StrokeData | undefined;
  const patch = (p: object) => run(() => engine.patchElements(ids, JSON.stringify(p)));
  const setStyle = (part: "fill" | "stroke", style: PaintStyle | null) =>
    run(() => engine.setPaintStyle(ids, part, JSON.stringify(style)));
  const anyPrimitive = infos.some((i) => i.element.geometry?.kind !== "path");

  /** Gradient stops: a single shape keeps its gradient geometry; several are re-fitted. */
  const setStops = (part: "fill" | "stroke", paint: Paint, stops: GradientStop[]) => {
    if (paint.type === "solid") return;
    if (!single) return setStyle(part, { type: paint.type, stops });
    if (part === "fill") patch({ fill: { ...paint, stops } });
    else patch({ stroke: { paint: { ...paint, stops } } });
  };

  const dashName = Object.entries(DASH_PRESETS).find(([, d]) => JSON.stringify(d) === JSON.stringify(stroke?.dash ?? []))?.[0] ?? "custom";

  return (
    <section>
      <h4>
        Fill &amp; stroke
        {anyPrimitive && (
          <button className="mini" title="Convert to editable path (⌘B)" onClick={() => run(() => engine.convertToPath(ids))}>
            to path
          </button>
        )}
      </h4>
      {closedShapes && (
        <>
          <div className="row">
            <span className="field-label">Fill</span>
            <select value={fill?.type ?? "none"} onChange={(ev) => setStyle("fill", styleFor(ev.target.value as PaintKind, fill, "#888888"))}>
              <option value="none">None</option>
              <option value="solid">Solid</option>
              <option value="linear">Linear gradient</option>
              <option value="radial">Radial gradient</option>
            </select>
            {fill?.type === "solid" && (
              <ColorField value={fill.color} onCommit={(color) => (single ? patch({ fill: { type: "solid", color } }) : setStyle("fill", { type: "solid", color }))} />
            )}
          </div>
          {fill && fill.type !== "solid" && (
            <GradientEditor kind={fill.type} stops={fill.stops} onChange={(stops) => setStops("fill", fill, stops)} />
          )}
          <div className="row">
            <span className="field-label">Rule</span>
            <select value={e.fillRule ?? "nonZero"} onChange={(ev) => patch({ fillRule: ev.target.value })} title="How overlapping/self-intersecting areas are filled">
              <option value="nonZero">Non-zero</option>
              <option value="evenOdd">Even-odd</option>
            </select>
          </div>
        </>
      )}
      <div className="row">
        <span className="field-label">Stroke</span>
        <select value={stroke?.paint.type ?? "none"} onChange={(ev) => setStyle("stroke", styleFor(ev.target.value as PaintKind, stroke?.paint, "#000000"))}>
          <option value="none">None</option>
          <option value="solid">Solid</option>
          <option value="linear">Linear gradient</option>
          <option value="radial">Radial gradient</option>
        </select>
        {stroke?.paint.type === "solid" && (
          <ColorField value={stroke.paint.color} onCommit={(color) => patch({ stroke: { paint: { type: "solid", color } } })} />
        )}
      </div>
      {stroke && stroke.paint.type !== "solid" && (
        <GradientEditor kind={stroke.paint.type} stops={stroke.paint.stops} onChange={(stops) => setStops("stroke", stroke.paint, stops)} />
      )}
      {stroke && (
        <div className="grid2 stroke-grid">
          <NumberField label="Width" suffix="px" min={0} value={stroke.width} onCommit={(width) => patch({ stroke: { width } })} />
          <label className="field">
            <span className="field-label">Dash</span>
            <select
              value={dashName}
              onChange={(ev) => ev.target.value !== "custom" && patch({ stroke: { dash: DASH_PRESETS[ev.target.value] } })}
            >
              {Object.keys(DASH_PRESETS).map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
              {dashName === "custom" && <option value="custom">custom</option>}
            </select>
          </label>
          <label className="field">
            <span className="field-label">Cap</span>
            <select value={stroke.cap} onChange={(ev) => patch({ stroke: { cap: ev.target.value } })}>
              <option value="butt">Butt</option>
              <option value="round">Round</option>
              <option value="square">Square</option>
            </select>
          </label>
          <label className="field">
            <span className="field-label">Join</span>
            <select value={stroke.join} onChange={(ev) => patch({ stroke: { join: ev.target.value } })}>
              <option value="miter">Miter</option>
              <option value="round">Round</option>
              <option value="bevel">Bevel</option>
            </select>
          </label>
          <label className="field wide" title="Dash and gap lengths, e.g. 8, 4">
            <span className="field-label">Pattern</span>
            <input
              key={JSON.stringify(stroke.dash ?? [])}
              type="text"
              defaultValue={(stroke.dash ?? []).join(", ")}
              placeholder="solid"
              onBlur={(ev) => {
                const dash = ev.target.value
                  .split(/[\s,]+/)
                  .filter(Boolean)
                  .map(Number);
                if (dash.every((d) => Number.isFinite(d) && d >= 0) && JSON.stringify(dash) !== JSON.stringify(stroke.dash ?? [])) {
                  patch({ stroke: { dash } });
                }
              }}
              onKeyDown={(ev) => ev.key === "Enter" && (ev.target as HTMLInputElement).blur()}
            />
          </label>
          {stroke.join === "miter" && (
            <NumberField label="Miter" min={1} max={100} value={stroke.miterLimit} onCommit={(miterLimit) => patch({ stroke: { miterLimit } })} />
          )}
          {(stroke.dash?.length ?? 0) > 0 && (
            <NumberField label="Offset" value={stroke.dashOffset ?? 0} onCommit={(dashOffset) => patch({ stroke: { dashOffset } })} />
          )}
        </div>
      )}
    </section>
  );
}

function FramePanel({ engine, target, run }: { engine: Engine; target: { layer: LayerNode; frame: number }; run: Props["run"] }) {
  const { layer, frame } = target;
  if (layer.kind === "folder") {
    return (
      <div className="props">
        <div className="panel-title">Folder “{layer.name}”</div>
        <p className="hint">Folders have no frames. Select a layer’s frame to edit tweens.</p>
      </div>
    );
  }
  const index = layer.keyframes.findIndex((k) => frame >= k.start && frame < k.start + k.duration);
  const kf = index >= 0 ? layer.keyframes[index] : null;
  const hasNext = index >= 0 && index + 1 < layer.keyframes.length;
  const ids = JSON.stringify([layer.id]);
  const setTween = (t: Tween | null) => kf && run(() => engine.setTween(ids, kf.start, JSON.stringify(t)));
  const tween = kf?.tween ?? null;

  return (
    <div className="props">
      <div className="panel-title">
        Frame {frame + 1} — {layer.name}
      </div>
      <section>
        {kf ? (
          <p className="hint flush">
            {kf.start === frame ? "Keyframe" : "Frame"} in span {kf.start + 1}–{kf.start + kf.duration}
            {kf.empty ? " (blank)" : ""}
          </p>
        ) : (
          <p className="hint flush">Past the end of this layer. Insert a frame (F5) or keyframe (F6) to extend it.</p>
        )}
        {kf && (
          <>
            <div className="row">
              <span className="field-label">Tween</span>
              <select
                value={tween?.kind ?? "none"}
                disabled={!hasNext && !tween}
                title={hasNext ? "" : "Add a later keyframe (F6) to tween toward"}
                onChange={(e) =>
                  setTween(e.target.value === "none" ? null : { kind: e.target.value as TweenKind, easing: tween?.easing ?? { type: "linear" }, rotate: tween?.rotate ?? 0 })
                }
              >
                <option value="none">None</option>
                <option value="motion">Motion (transform, color)</option>
                <option value="shape">Shape (also geometry, paint)</option>
              </select>
            </div>
            {!hasNext && <p className="hint flush">Add a later keyframe (F6) to tween toward.</p>}
            {tween && (
              <>
                <div className="row">
                  <NumberField
                    label="Spins"
                    precision={0}
                    min={-20}
                    max={20}
                    title="Extra full turns (positive = clockwise)"
                    value={tween.rotate ?? 0}
                    onCommit={(rotate) => setTween({ ...tween, rotate })}
                  />
                </div>
                <h4>Easing</h4>
                <EasingEditor engine={engine} easing={tween.easing} onChange={(easing) => setTween({ ...tween, easing })} />
              </>
            )}
          </>
        )}
      </section>
    </div>
  );
}
