import { useMemo, useState } from "react";
import type { LayerNode } from "../engine";

interface Props {
  layers: LayerNode[];
  activeLayer: number | null;
  /** Layers holding selected elements (marked with a dot). */
  selectedLayers: Set<number>;
  onActivate: (id: number) => void;
  onAdd: (kind: "normal" | "folder") => void;
  onDelete: (id: number) => void;
  onPatch: (id: number, patch: { name?: string; visible?: boolean; locked?: boolean; kind?: "normal" | "guide" }) => void;
  /** `parent` null = top level; `index` in bottom-to-top storage order, counted after removal. */
  onMove: (id: number, parent: number | null, index: number) => void;
}

interface Row {
  node: LayerNode;
  depth: number;
  parent: number | null;
  /** Index in the parent's bottom-to-top storage list. */
  storageIndex: number;
}

type DropPos = "above" | "below" | "into";

function flatten(nodes: LayerNode[], collapsed: Set<number>, depth = 0, parent: number | null = null, out: Row[] = []): Row[] {
  nodes.forEach((node, displayIndex) => {
    out.push({ node, depth, parent, storageIndex: nodes.length - 1 - displayIndex });
    if (node.kind === "folder" && !collapsed.has(node.id)) flatten(node.children, collapsed, depth + 1, node.id, out);
  });
  return out;
}

/** Flash-style layer list: top layer first, folders, show/lock toggles, drag to reorder. */
export function LayersPanel(props: Props) {
  const { layers, activeLayer, selectedLayers } = props;
  const [collapsed, setCollapsed] = useState<Set<number>>(new Set());
  const [renaming, setRenaming] = useState<number | null>(null);
  const [dragging, setDragging] = useState<number | null>(null);
  const [drop, setDrop] = useState<{ id: number; pos: DropPos } | null>(null);

  const rows = useMemo(() => flatten(layers, collapsed), [layers, collapsed]);
  const allRows = useMemo(() => flatten(layers, new Set()), [layers]);
  const active = allRows.find((r) => r.node.id === activeLayer)?.node;

  const toggleCollapsed = (id: number) =>
    setCollapsed((c) => {
      const n = new Set(c);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const dropPosition = (e: React.DragEvent, node: LayerNode): DropPos => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const f = (e.clientY - r.top) / r.height;
    if (node.kind === "folder" && f > 0.25 && f < 0.75) return "into";
    return f < 0.5 ? "above" : "below";
  };

  const onDrop = (target: Row, pos: DropPos) => {
    const src = allRows.find((r) => r.node.id === dragging);
    if (!src || src.node.id === target.node.id) return;
    let parent: number | null;
    let index: number;
    if (pos === "into") {
      parent = target.node.id;
      index = target.node.children.length;
    } else {
      parent = target.parent;
      index = pos === "above" ? target.storageIndex + 1 : target.storageIndex;
    }
    if (src.parent === parent && src.storageIndex < index) index -= 1;
    props.onMove(src.node.id, parent, index);
  };

  return (
    <div className="layers">
      <div className="panel-title">
        Layers
        <span className="panel-actions">
          <button title="New layer" onClick={() => props.onAdd("normal")}>＋</button>
          <button title="New folder" onClick={() => props.onAdd("folder")}>▤＋</button>
          <button
            title="Toggle guide layer (guides never export)"
            disabled={!active || active.kind === "folder"}
            className={active?.kind === "guide" ? "on" : ""}
            onClick={() => active && props.onPatch(active.id, { kind: active.kind === "guide" ? "normal" : "guide" })}
          >
            ⌗
          </button>
          <button title="Delete layer" disabled={!active} onClick={() => active && props.onDelete(active.id)}>
            🗑
          </button>
        </span>
      </div>
      <ul onDragEnd={() => (setDragging(null), setDrop(null))}>
        {rows.map((row) => {
          const { node } = row;
          const isDrop = drop?.id === node.id ? `drop-${drop.pos}` : "";
          return (
            <li
              key={node.id}
              className={["layer-row", node.kind, node.id === activeLayer ? "active" : "", isDrop].join(" ")}
              style={{ paddingLeft: 6 + row.depth * 16 }}
              draggable={renaming !== node.id}
              onDragStart={(e) => {
                setDragging(node.id);
                e.dataTransfer.effectAllowed = "move";
                e.dataTransfer.setData("text/plain", String(node.id));
              }}
              onDragOver={(e) => {
                if (dragging === null) return;
                e.preventDefault();
                setDrop({ id: node.id, pos: dropPosition(e, node) });
              }}
              onDragLeave={() => setDrop((d) => (d?.id === node.id ? null : d))}
              onDrop={(e) => {
                e.preventDefault();
                onDrop(row, dropPosition(e, node));
                setDragging(null);
                setDrop(null);
              }}
              onClick={() => props.onActivate(node.id)}
            >
              <span
                className="twisty"
                onClick={(e) => {
                  if (node.kind !== "folder") return;
                  e.stopPropagation();
                  toggleCollapsed(node.id);
                }}
              >
                {node.kind === "folder" ? (collapsed.has(node.id) ? "▸" : "▾") : ""}
              </span>
              <span className="kind" title={node.kind}>
                {node.kind === "folder" ? "▤" : node.kind === "guide" ? "⌗" : "▭"}
              </span>
              {renaming === node.id ? (
                <input
                  className="rename"
                  autoFocus
                  defaultValue={node.name}
                  onClick={(e) => e.stopPropagation()}
                  onBlur={(e) => {
                    setRenaming(null);
                    if (e.target.value.trim() && e.target.value !== node.name) props.onPatch(node.id, { name: e.target.value });
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                    if (e.key === "Escape") setRenaming(null);
                  }}
                />
              ) : (
                <span className="name" onDoubleClick={() => setRenaming(node.id)} title="Double-click to rename">
                  {node.name}
                </span>
              )}
              {selectedLayers.has(node.id) && <span className="sel-dot" title="Has selected objects" />}
              <button
                className={`toggle ${node.visible ? "" : "off"}`}
                title={node.visible ? "Hide" : "Show"}
                onClick={(e) => {
                  e.stopPropagation();
                  props.onPatch(node.id, { visible: !node.visible });
                }}
              >
                {node.visible ? "👁" : "–"}
              </button>
              <button
                className={`toggle ${node.locked ? "on" : ""}`}
                title={node.locked ? "Unlock" : "Lock"}
                onClick={(e) => {
                  e.stopPropagation();
                  props.onPatch(node.id, { locked: !node.locked });
                }}
              >
                {node.locked ? "🔒" : "·"}
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
