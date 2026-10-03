import { useEffect, useRef } from "react";
import type { OutputLine } from "../runtime/scripting";

/** Script output during preview: trace() lines and errors with their location. */
export function OutputPanel({ lines, onClear, onClose }: { lines: OutputLine[]; onClear: () => void; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines]);
  return (
    <div className="output-panel">
      <div className="panel-title">
        Output
        <span className="panel-actions">
          <button title="Clear" onClick={onClear}>
            ⌫
          </button>
          <button title="Close" onClick={onClose}>
            ✕
          </button>
        </span>
      </div>
      <div className="output-lines" ref={ref}>
        {lines.length === 0 && <p className="hint">trace() output and script errors appear here during preview.</p>}
        {lines.map((l, i) => (
          <div key={i} className={`output-line ${l.kind}`}>
            {l.where && <span className="where">{l.where}: </span>}
            {l.text}
          </div>
        ))}
      </div>
    </div>
  );
}
