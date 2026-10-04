import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { checkSyntax } from "../runtime/scripting";
import {
  checkBrackets,
  complete,
  highlight,
  lineRange,
  newlineAt,
  offsetOf,
  shiftLines,
  toggleComment,
  type Completion,
  type CompletionResult,
  type Problem,
  type ScriptNames,
} from "../scriptlang";

interface Props {
  /** Saved script ("" = none). */
  value: string;
  onApply: (script: string) => void;
  placeholder?: string;
  /** Visible text rows. */
  rows?: number;
  autoFocus?: boolean;
  /** Instance names and frame labels the script can use, for completion. */
  names?: ScriptNames;
}

const NO_NAMES: ScriptNames = { names: [], labels: [] };
const LINE_HEIGHT = 12 * 1.45;
const PAD_X = 8;
const PAD_Y = 6;
/** Room the completion list needs (list plus detail line). */
const MENU_HEIGHT = 220;

const KIND_ICON: Record<Completion["kind"], string> = {
  function: "ƒ",
  property: "◦",
  event: "⚡",
  object: "◆",
  keyword: "k",
  name: "▣",
  label: "⚑",
  value: "\"",
  word: "·",
};

/**
 * The script field: highlighted JavaScript over a plain textarea (so
 * selection, IME, spellcheck-off and native undo all behave), with
 * auto-indent, Tab/⇧Tab indenting, ⌘/ comments, completions for the
 * scripting API and the timeline's names, and syntax errors checked by the
 * same compiler playback uses. ⌘↵ / ⌘S applies (one undo step in the
 * document), Esc reverts. Edits stay local until applied, so typing never
 * floods the history.
 */
export function ScriptEditor({ value, onApply, placeholder, rows = 10, autoFocus, names = NO_NAMES }: Props) {
  const [draft, setDraft] = useState(value);
  const [problem, setProblem] = useState<Problem | null>(null);
  const [menu, setMenu] = useState<(CompletionResult & { index: number }) | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  const hl = useRef<HTMLPreElement>(null);
  const gutter = useRef<HTMLPreElement>(null);
  const charWidth = useRef(7.2);
  /**
   * A closing parenthesis a completion inserted, as a distance from the end
   * of the text (typing inside the parentheses doesn't move it): typing ")"
   * right before it steps over it.
   */
  const autoClose = useRef(-1);
  // A new saved value (undo, another keyframe) replaces the draft.
  useEffect(() => setDraft(value), [value]);
  const dirty = draft !== value;
  const lineCount = draft.split("\n").length;
  const lines = Math.max(rows, lineCount + 1);

  // Brackets are checked at once (cheap, precise); the compiler after a pause.
  useEffect(() => {
    const quick = checkBrackets(draft);
    if (quick || !draft.trim()) {
      setProblem(quick);
      return;
    }
    let live = true;
    const t = setTimeout(() => {
      checkSyntax(draft)
        .then((p) => live && setProblem(p))
        .catch(() => live && setProblem(null));
    }, 350);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [draft]);

  const html = useMemo(() => highlight(draft, problem), [draft, problem]);

  // The list is placed in window coordinates: close it when anything else scrolls.
  const menuOpen = !!menu;
  useEffect(() => {
    if (!menuOpen) return;
    const close = (e: Event) => {
      if (e.target === ref.current || (e.target instanceof Element && e.target.closest(".script-menu"))) return;
      setMenu(null);
    };
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [menuOpen]);

  useLayoutEffect(() => {
    const probe = document.createElement("span");
    probe.className = "script-probe";
    probe.textContent = "M".repeat(40);
    ref.current?.parentElement?.appendChild(probe);
    charWidth.current = probe.getBoundingClientRect().width / 40 || 7.2;
    probe.remove();
  }, []);

  const syncScroll = () => {
    const t = ref.current;
    if (!t) return;
    if (hl.current) {
      hl.current.scrollTop = t.scrollTop;
      hl.current.scrollLeft = t.scrollLeft;
    }
    if (gutter.current) gutter.current.scrollTop = t.scrollTop;
  };
  useLayoutEffect(syncScroll, [html]);

  /** Replaces `[a, b)` through the browser's editing commands, so ⌘Z in the field undoes it. */
  const replace = (a: number, b: number, text: string, caret = text.length, selectAll = false) => {
    const t = ref.current!;
    t.focus();
    t.setSelectionRange(a, b);
    if (!document.execCommand("insertText", false, text)) {
      t.setRangeText(text, a, b, "end");
      setDraft(t.value);
    }
    if (selectAll) t.setSelectionRange(a, a + text.length);
    else t.setSelectionRange(a + caret, a + caret);
  };

  const refreshMenu = (explicit = false) => {
    const t = ref.current!;
    if (t.selectionStart !== t.selectionEnd) return setMenu(null);
    const r = complete(t.value, t.selectionStart, names, explicit);
    setMenu(r ? { ...r, index: 0 } : null);
  };

  const accept = (c: Completion) => {
    const t = ref.current!;
    const from = menu!.from;
    // Don't double the parentheses when they are already there.
    const after = t.value.slice(t.selectionStart);
    const template = c.insert && !(c.insert.includes("(") && after.startsWith("(")) ? c.insert : c.label;
    // Multi-line inserts follow the current indent.
    const indent = /^[ \t]*/.exec(t.value.slice(t.value.lastIndexOf("\n", from - 1) + 1))![0];
    const full = template.replace(/\n/g, `\n${indent}`);
    const mark = full.indexOf("$0");
    const text = full.replace("$0", "");
    replace(from, t.selectionStart, text, mark < 0 ? text.length : mark);
    if (mark >= 0 && text[mark] === ")") autoClose.current = t.value.length - (from + mark);
    setMenu(null);
  };

  const goTo = (p: Problem) => {
    const t = ref.current!;
    const off = offsetOf(t.value, p.line, p.column);
    t.focus();
    t.setSelectionRange(off, off);
    t.scrollTop = Math.max(0, (p.line - 3) * LINE_HEIGHT);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    e.stopPropagation();
    const t = e.currentTarget;
    const mod = e.metaKey || e.ctrlKey;
    const { selectionStart: a, selectionEnd: b, value: v } = t;

    if (menu) {
      const n = menu.items.length;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setMenu({ ...menu, index: (menu.index + (e.key === "ArrowDown" ? 1 : n - 1)) % n });
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        accept(menu.items[menu.index]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setMenu(null);
        return;
      }
    }

    if (e.key === ")" && a === b && v.length - a === autoClose.current && v[a] === ")") {
      e.preventDefault();
      t.setSelectionRange(a + 1, a + 1);
      autoClose.current = -1;
    } else if (e.key === " " && e.ctrlKey) {
      e.preventDefault();
      refreshMenu(true);
    } else if (e.key === "Tab") {
      e.preventDefault();
      if (a === b && !e.shiftKey) return replace(a, b, "  ");
      const r = lineRange(v, a, b);
      const shifted = shiftLines(v.slice(r.start, r.end), e.shiftKey);
      replace(r.start, r.end, shifted, 0, true);
    } else if (e.key === "Enter" && !mod && !e.shiftKey && !e.altKey) {
      e.preventDefault();
      const n = newlineAt(v, a);
      replace(a, b, n.text, n.caret);
    } else if ((e.key === "}" || e.key === "]" || e.key === ")") && a === b) {
      // A closer typed on a blank line lines up with its opener's indent.
      const lineStart = v.lastIndexOf("\n", a - 1) + 1;
      const lead = v.slice(lineStart, a);
      if (lead.length >= 2 && /^[ \t]+$/.test(lead)) {
        e.preventDefault();
        replace(lineStart, a, lead.slice(2) + e.key);
      }
    } else if (mod && e.key === "/") {
      e.preventDefault();
      const r = lineRange(v, a, b);
      replace(r.start, r.end, toggleComment(v.slice(r.start, r.end)), 0, true);
    } else if (mod && (e.key === "Enter" || e.key === "s")) {
      e.preventDefault();
      if (dirty) onApply(draft);
    } else if (e.key === "Escape") {
      setDraft(value);
    }
  };

  // Where the completion list goes: under the caret, or above it near the
  // bottom of the window. Fixed, so scrolling panels don't clip it.
  const menuPos = (() => {
    const t = ref.current;
    if (!menu || !t) return null;
    const r = t.getBoundingClientRect();
    const before = t.value.slice(0, menu.from);
    const line = before.split("\n").length - 1;
    const col = menu.from - (before.lastIndexOf("\n") + 1);
    const left = Math.min(r.left + PAD_X + col * charWidth.current - t.scrollLeft - 22, window.innerWidth - 240);
    const top = r.top + PAD_Y + line * LINE_HEIGHT - t.scrollTop;
    const below = top + LINE_HEIGHT + 2;
    return below + MENU_HEIGHT < window.innerHeight
      ? { left: Math.max(4, left), top: below }
      : { left: Math.max(4, left), bottom: window.innerHeight - top + 2 };
  })();
  const active = menu?.items[menu.index];

  return (
    <div className="script-editor">
      <div className="script-body">
        <pre className="script-gutter" aria-hidden ref={gutter}>
          {Array.from({ length: lines }, (_, i) =>
            problem && problem.line === i + 1 ? (
              <span key={i} className="script-gutter-error" title={problem.message}>
                {i + 1}
                {"\n"}
              </span>
            ) : (
              `${i + 1}\n`
            ),
          )}
        </pre>
        <div className="script-code">
          <pre className="script-hl" aria-hidden ref={hl} dangerouslySetInnerHTML={{ __html: html }} />
          <textarea
            ref={ref}
            autoFocus={autoFocus}
            value={draft}
            rows={rows}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            autoComplete="off"
            placeholder={placeholder}
            aria-invalid={!!problem}
            onChange={(e) => {
              setDraft(e.target.value);
              const ne = e.nativeEvent as InputEvent;
              // Typing (not deleting or pasting) opens or narrows the list.
              if (ne.inputType === "insertText" && ne.data && /[\w$."'.]/.test(ne.data)) refreshMenu();
              else if (menu && ne.inputType?.startsWith("delete")) refreshMenu();
              else setMenu(null);
            }}
            onScroll={syncScroll}
            onBlur={() => setMenu(null)}
            onClick={() => setMenu(null)}
            onKeyDown={onKeyDown}
          />
          {menu && menuPos && (
            <div className="script-menu" style={menuPos} onMouseDown={(e) => e.preventDefault()}>
              <ul role="listbox">
                {menu.items.slice(0, 50).map((c, i) => (
                  <li
                    key={c.label}
                    role="option"
                    aria-selected={i === menu.index}
                    className={i === menu.index ? "on" : ""}
                    ref={i === menu.index ? (el) => el?.scrollIntoView({ block: "nearest" }) : undefined}
                    onClick={() => accept(c)}
                  >
                    <span className={`script-menu-kind k-${c.kind}`}>{KIND_ICON[c.kind]}</span>
                    {c.label === " " ? "␠" : c.label}
                  </li>
                ))}
              </ul>
              {active?.detail && <div className="script-menu-detail">{active.detail}</div>}
            </div>
          )}
        </div>
      </div>
      {problem && (
        <button className="script-problem" onClick={() => goTo(problem)} title="Go to the error">
          ⚠ Line {problem.line}: {problem.message}
        </button>
      )}
      <div className="btn-row script-actions">
        <span className="muted small">
          {dirty ? "Unapplied changes · ⌘↵ to apply" : value ? "Applied" : "Ctrl+Space for suggestions"}
        </span>
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
