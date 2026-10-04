import { useEffect, useRef, useState } from "react";
import { ScriptEditor } from "./ScriptEditor";
import { SYMBOL_DRAG_TYPE, SYMBOL_KIND_ICON, SYMBOL_KIND_LABEL, type Engine, type LibraryItem, type SymbolKind } from "../engine";

interface Props {
  engine: Engine;
  version: number;
  items: LibraryItem[];
  run: (fn: () => unknown) => void;
  onEdit: (symbol: number) => void;
}

/** Symbol library: thumbnails, rename, kind, duplicate, delete, drag onto the stage. */
export function LibraryPanel({ engine, version, items, run, onEdit }: Props) {
  const [selected, setSelected] = useState<number | null>(null);
  const [renaming, setRenaming] = useState<number | null>(null);
  const [scripting, setScripting] = useState<LibraryItem | null>(null);
  const sel = items.find((i) => i.id === selected) ?? null;

  return (
    <div className="library">
      <div className="panel-title">
        Library
        <span className="panel-actions">
          <button title="Edit symbol" disabled={!sel} onClick={() => sel && onEdit(sel.id)}>
            ✎
          </button>
          <button title="Symbol script (runs for each instance as it appears)" disabled={!sel} onClick={() => sel && setScripting(sel)}>
            {"{ }"}
          </button>
          <button title="Duplicate symbol" disabled={!sel} onClick={() => sel && run(() => setSelected(engine.duplicateSymbol(sel.id)))}>
            ⧉
          </button>
          <button
            title={sel && sel.uses > 0 ? "In use — delete its instances first" : "Delete symbol"}
            disabled={!sel || sel.uses > 0}
            onClick={() => sel && run(() => engine.deleteSymbol(sel.id))}
          >
            🗑
          </button>
        </span>
      </div>
      {items.length === 0 && <p className="hint">No symbols yet. Select objects and press F8 to make one.</p>}
      <ul>
        {items.map((item) => (
          <li
            key={item.id}
            className={`lib-item ${item.id === selected ? "active" : ""}`}
            draggable={renaming !== item.id}
            onDragStart={(e) => {
              e.dataTransfer.setData(SYMBOL_DRAG_TYPE, String(item.id));
              e.dataTransfer.effectAllowed = "copy";
            }}
            onClick={() => setSelected(item.id)}
            onDoubleClick={() => onEdit(item.id)}
            title="Drag onto the stage to place an instance; double-click to edit"
          >
            <Thumb engine={engine} symbol={item.id} version={version} />
            <div className="lib-meta">
              {renaming === item.id ? (
                <input
                  autoFocus
                  defaultValue={item.name}
                  onClick={(e) => e.stopPropagation()}
                  onBlur={(e) => {
                    setRenaming(null);
                    const name = e.target.value.trim();
                    if (name && name !== item.name) run(() => engine.setSymbolProps(item.id, JSON.stringify({ name })));
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                    if (e.key === "Escape") setRenaming(null);
                  }}
                />
              ) : (
                <span
                  className="name"
                  onDoubleClick={(e) => {
                    e.stopPropagation();
                    setRenaming(item.id);
                  }}
                  title="Double-click to rename"
                >
                  {item.name}
                </span>
              )}
              <span className="lib-sub">
                <select
                  value={item.kind}
                  onClick={(e) => e.stopPropagation()}
                  onChange={(e) => run(() => engine.setSymbolProps(item.id, JSON.stringify({ kind: e.target.value as SymbolKind })))}
                  title="Symbol type"
                >
                  {(["graphic", "movieClip", "button"] as const).map((k) => (
                    <option key={k} value={k}>
                      {SYMBOL_KIND_ICON[k]} {SYMBOL_KIND_LABEL[k]}
                    </option>
                  ))}
                </select>
                <span className="muted">
                  {item.length}f · {item.uses}×
                </span>
                {item.hasScript && (
                  <span className="script-badge" title="Has a symbol script">
                    {"{ }"}
                  </span>
                )}
              </span>
            </div>
          </li>
        ))}
      </ul>
      {scripting && (
        <div className="modal-backdrop" onClick={() => setScripting(null)}>
          <div className="modal wide" onClick={(e) => e.stopPropagation()}>
            <h3>Symbol script: {scripting.name}</h3>
            <p className="hint flush">
              Runs once for every instance of this symbol when it appears on stage; <code>this</code> is the instance.
            </p>
            <ScriptEditor
              value={engine.symbolScript(scripting.id) ?? ""}
              names={JSON.parse(engine.scriptNamesJson(scripting.id))}
              onApply={(s) => run(() => engine.setSymbolScript(scripting.id, s.trim() ? s : undefined))}
              placeholder={"this.onEnterFrame = function () {\n  this.rotation += 2;\n};"}
              rows={20}
              autoFocus
            />
            <div className="btn-row">
              <button onClick={() => setScripting(null)}>Close</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function Thumb({ engine, symbol, version }: { engine: Engine; symbol: number; version: number }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const c = ref.current!;
    const dpr = window.devicePixelRatio || 1;
    c.width = 44 * dpr;
    c.height = 44 * dpr;
    engine.renderSymbolPreview(c.getContext("2d")!, symbol, 44 * dpr, 44 * dpr);
  }, [engine, symbol, version]);
  return <canvas ref={ref} className="lib-thumb" />;
}
