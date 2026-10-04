import { useState } from "react";
import type { LayerNode } from "../engine";

/** A visible row of the layer tree (folders may be collapsed). */
export interface LayerRow {
  node: LayerNode;
  depth: number;
  parent: number | null;
  /** Index in the parent's bottom-to-top storage list. */
  storageIndex: number;
}

/** Folders and masks hold layers (a mask's children are the layers it masks). */
const holdsLayers = (n: LayerNode) => n.kind === "folder" || n.kind === "mask";

/** Flattens the (top-first) layer tree, skipping collapsed folders' children. */
export function flattenLayers(nodes: LayerNode[], collapsed: Set<number>, depth = 0, parent: number | null = null, out: LayerRow[] = []): LayerRow[] {
  nodes.forEach((node, displayIndex) => {
    out.push({ node, depth, parent, storageIndex: nodes.length - 1 - displayIndex });
    if (holdsLayers(node) && !collapsed.has(node.id)) flattenLayers(node.children, collapsed, depth + 1, node.id, out);
  });
  return out;
}

export interface LayerPatch {
  name?: string;
  visible?: boolean;
  locked?: boolean;
  kind?: "normal" | "guide" | "mask";
}

interface Props {
  rows: LayerRow[];
  /** All rows (ignoring collapse), for resolving drag sources. */
  allRows: LayerRow[];
  collapsed: Set<number>;
  onToggleCollapsed: (id: number) => void;
  activeLayer: number | null;
  /** Layers holding selected elements (marked with a dot). */
  selectedLayers: Set<number>;
  onActivate: (id: number) => void;
  onPatch: (id: number, patch: LayerPatch) => void;
  /** `parent` null = top level; `index` in bottom-to-top storage order, counted after removal. */
  onMove: (id: number, parent: number | null, index: number) => void;
}

type DropPos = "above" | "below" | "into";

/** Layer name column of the timeline: show/lock toggles, rename, drag to reorder or into folders. */
export function LayersPanel(props: Props) {
  const { rows, allRows, collapsed, activeLayer, selectedLayers } = props;
  const [renaming, setRenaming] = useState<number | null>(null);
  const [dragging, setDragging] = useState<number | null>(null);
  const [drop, setDrop] = useState<{ id: number; pos: DropPos } | null>(null);

  const dropPosition = (e: React.DragEvent, node: LayerNode): DropPos => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const f = (e.clientY - r.top) / r.height;
    if (holdsLayers(node) && f > 0.25 && f < 0.75) return "into";
    return f < 0.5 ? "above" : "below";
  };

  const onDrop = (target: LayerRow, pos: DropPos) => {
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
    <ul className="layer-list" onDragEnd={() => (setDragging(null), setDrop(null))}>
      {rows.map((row) => {
        const { node } = row;
        const isDrop = drop?.id === node.id ? `drop-${drop.pos}` : "";
        return (
          <li
            key={node.id}
            className={["layer-row", node.kind, node.id === activeLayer ? "active" : "", isDrop].join(" ")}
            style={{ paddingLeft: 6 + row.depth * 14 }}
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
                if (!holdsLayers(node)) return;
                e.stopPropagation();
                props.onToggleCollapsed(node.id);
              }}
            >
              {holdsLayers(node) ? (collapsed.has(node.id) ? "▸" : "▾") : ""}
            </span>
            <span className="kind" title={node.kind === "mask" ? "mask: the layers inside show only where its shapes are" : node.kind}>
              {node.kind === "folder" ? "▤" : node.kind === "guide" ? "⌗" : node.kind === "mask" ? "◪" : "▭"}
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
  );
}
