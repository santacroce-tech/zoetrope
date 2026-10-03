import { useEffect, useRef, useState } from "react";

interface Props {
  /** Saved script ("" = none). */
  value: string;
  onApply: (script: string) => void;
  placeholder?: string;
  /** Visible text rows. */
  rows?: number;
  autoFocus?: boolean;
}

/**
 * A plain code field: monospace, Tab indents, ⌘↵ / ⌘S applies (one undo
 * step), Esc reverts. Edits stay local until applied, so typing never
 * floods the history.
 */
export function ScriptEditor({ value, onApply, placeholder, rows = 10, autoFocus }: Props) {
  const [draft, setDraft] = useState(value);
  const ref = useRef<HTMLTextAreaElement>(null);
  // A new saved value (undo, another keyframe) replaces the draft.
  useEffect(() => setDraft(value), [value]);
  const dirty = draft !== value;
  const lines = Math.max(rows, draft.split("\n").length + 1);

  return (
    <div className="script-editor">
      <div className="script-body">
        <pre className="script-gutter" aria-hidden>
          {Array.from({ length: lines }, (_, i) => i + 1).join("\n")}
        </pre>
        <textarea
          ref={ref}
          autoFocus={autoFocus}
          value={draft}
          rows={rows}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          placeholder={placeholder}
          onChange={(e) => setDraft(e.target.value)}
          onScroll={(e) => {
            const g = e.currentTarget.previousElementSibling as HTMLElement | null;
            if (g) g.scrollTop = e.currentTarget.scrollTop;
          }}
          onKeyDown={(e) => {
            e.stopPropagation();
            const t = e.currentTarget;
            if (e.key === "Tab") {
              e.preventDefault();
              const { selectionStart: a, selectionEnd: b } = t;
              const next = draft.slice(0, a) + "  " + draft.slice(b);
              setDraft(next);
              requestAnimationFrame(() => t.setSelectionRange(a + 2, a + 2));
            } else if ((e.metaKey || e.ctrlKey) && (e.key === "Enter" || e.key === "s")) {
              e.preventDefault();
              if (dirty) onApply(draft);
            } else if (e.key === "Escape") {
              setDraft(value);
            }
          }}
        />
      </div>
      <div className="btn-row script-actions">
        <span className="muted small">{dirty ? "Unapplied changes · ⌘↵ to apply" : value ? "Applied" : ""}</span>
        <button disabled={!dirty} onClick={() => setDraft(value)} title="Discard edits (Esc)">
          Revert
        </button>
        <button disabled={!dirty} onClick={() => onApply(draft)} title="Apply (⌘↵)">
          Apply
        </button>
      </div>
    </div>
  );
}
