import { useEffect, useRef, useState } from "react";

const fmt = (v: number, precision: number) => String(Number(v.toFixed(precision)));

interface NumberFieldProps {
  label: string;
  value: number;
  onCommit: (v: number) => void;
  step?: number;
  precision?: number;
  suffix?: string;
  min?: number;
  max?: number;
  disabled?: boolean;
  title?: string;
}

/**
 * Numeric input that commits on Enter/blur (one undo step per commit) and
 * steps with ↑/↓ (⇧ ×10). Escape reverts.
 */
export function NumberField({ label, value, onCommit, step = 1, precision = 2, suffix, min, max, disabled, title }: NumberFieldProps) {
  const [text, setText] = useState(fmt(value, precision));
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setText(fmt(value, precision));
  }, [value, precision]);

  const clamp = (v: number) => Math.min(max ?? Infinity, Math.max(min ?? -Infinity, v));
  const commit = (v: number) => {
    const c = clamp(v);
    setText(fmt(c, precision));
    if (Number.isFinite(c) && Math.abs(c - value) > 1e-9) onCommit(c);
  };

  return (
    <label className="field" title={title}>
      <span className="field-label">{label}</span>
      <input
        type="text"
        inputMode="decimal"
        disabled={disabled}
        value={text}
        onFocus={(e) => {
          focused.current = true;
          e.target.select();
        }}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          focused.current = false;
          const v = parseFloat(text);
          if (Number.isFinite(v)) commit(v);
          else setText(fmt(value, precision));
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          else if (e.key === "Escape") {
            setText(fmt(value, precision));
            focused.current = false;
            (e.target as HTMLInputElement).blur();
          } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            e.preventDefault();
            const base = parseFloat(text);
            const next = (Number.isFinite(base) ? base : value) + (e.key === "ArrowUp" ? 1 : -1) * step * (e.shiftKey ? 10 : 1);
            commit(next);
          }
        }}
      />
      {suffix && <span className="field-suffix">{suffix}</span>}
    </label>
  );
}

interface ColorFieldProps {
  value: string;
  onCommit: (hex: string) => void;
  disabled?: boolean;
  title?: string;
}

/**
 * Color swatch. Commits once when the picker closes (native `change`), not on
 * every intermediate `input`, so a pick is a single undo step. Preserves the
 * existing alpha channel of `#rrggbbaa` values.
 */
export function ColorField({ value, onCommit, disabled, title }: ColorFieldProps) {
  const ref = useRef<HTMLInputElement>(null);
  const latest = useRef({ value, onCommit });
  latest.current = { value, onCommit };
  useEffect(() => {
    const el = ref.current!;
    el.value = value.slice(0, 7);
  }, [value]);
  useEffect(() => {
    const el = ref.current!;
    const onChange = () => {
      const { value: cur, onCommit: commit } = latest.current;
      const alpha = cur.length === 9 ? cur.slice(7) : "";
      if (el.value !== cur.slice(0, 7)) commit(el.value + alpha);
    };
    el.addEventListener("change", onChange);
    return () => el.removeEventListener("change", onChange);
  }, []);
  return <input ref={ref} type="color" className="swatch" disabled={disabled} title={title} defaultValue={value.slice(0, 7)} />;
}
