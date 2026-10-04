// Language support for the script editor: a small JavaScript tokenizer for
// highlighting and bracket checks, and context-aware completions for the
// scripting API (docs/SCRIPTING.md). Plain functions, no DOM, so they are
// unit-tested in tests/scriptlang.test.ts.

export type TokenKind = "comment" | "string" | "number" | "keyword" | "api" | "ident" | "punct" | "regex" | "space";

export interface Token {
  kind: TokenKind;
  start: number;
  end: number;
  /** Strings and block comments that run to the end of the line or source. */
  open?: boolean;
}

/** A problem in the source; line and column are 1-based. */
export interface Problem {
  line: number;
  column: number;
  message: string;
}

const KEYWORDS = new Set(
  (
    "break case catch class const continue debugger default delete do else export extends false finally for function if " +
    "import in instanceof let new null of return super switch this throw true try typeof undefined var void while with yield async await"
  ).split(" "),
);

/** Words that start a regex when followed by `/` (otherwise it's division). */
const BEFORE_REGEX = new Set(["return", "typeof", "case", "do", "else", "in", "of", "new", "delete", "void", "throw", "yield", "await"]);

export function tokenize(src: string): Token[] {
  const out: Token[] = [];
  let i = 0;
  // The last significant token, to tell a regex from a division.
  let prev: Token | null = null;
  const push = (kind: TokenKind, start: number, end: number, open = false) => {
    const t: Token = open ? { kind, start, end, open } : { kind, start, end };
    out.push(t);
    if (kind !== "space" && kind !== "comment") prev = t;
  };
  while (i < src.length) {
    const c = src[i];
    const start = i;
    if (/\s/.test(c)) {
      while (i < src.length && /\s/.test(src[i])) i++;
      push("space", start, i);
    } else if (c === "/" && src[i + 1] === "/") {
      while (i < src.length && src[i] !== "\n") i++;
      push("comment", start, i);
    } else if (c === "/" && src[i + 1] === "*") {
      const end = src.indexOf("*/", i + 2);
      i = end < 0 ? src.length : end + 2;
      push("comment", start, i, end < 0);
    } else if (c === '"' || c === "'" || c === "`") {
      i++;
      while (i < src.length && src[i] !== c) {
        if (src[i] === "\\") i++;
        else if (src[i] === "\n" && c !== "`") break;
        i++;
      }
      const open = i >= src.length || src[i] !== c;
      if (!open) i++;
      push("string", start, i, open);
    } else if (/[0-9]/.test(c) || (c === "." && /[0-9]/.test(src[i + 1] ?? ""))) {
      while (i < src.length && /[0-9a-fA-FxXoObBn_.eE]/.test(src[i])) {
        // Exponent sign: 1e-3.
        if ((src[i] === "e" || src[i] === "E") && (src[i + 1] === "-" || src[i + 1] === "+") && !/^0[xX]/.test(src.slice(start, i))) i++;
        i++;
      }
      push("number", start, i);
    } else if (/[A-Za-z_$]/.test(c)) {
      while (i < src.length && /[\w$]/.test(src[i])) i++;
      const word = src.slice(start, i);
      push(KEYWORDS.has(word) ? "keyword" : API_WORDS.has(word) ? "api" : "ident", start, i);
    } else if (c === "/" && regexAllowed(src, prev)) {
      let inClass = false;
      i++;
      while (i < src.length && src[i] !== "\n") {
        if (src[i] === "\\") i++;
        else if (src[i] === "[") inClass = true;
        else if (src[i] === "]") inClass = false;
        else if (src[i] === "/" && !inClass) break;
        i++;
      }
      i = Math.min(i + 1, src.length);
      while (i < src.length && /[a-z]/.test(src[i])) i++;
      push("regex", start, i);
    } else {
      i++;
      push("punct", start, i);
    }
  }
  return out;
}

function regexAllowed(src: string, prev: Token | null): boolean {
  if (!prev) return true;
  const text = src.slice(prev.start, prev.end);
  if (prev.kind === "punct") return !")]}".includes(text);
  return prev.kind === "keyword" && BEFORE_REGEX.has(text);
}

/** 1-based line and column of an offset. */
export function position(src: string, offset: number): { line: number; column: number } {
  const before = src.slice(0, offset);
  const line = before.split("\n").length;
  return { line, column: offset - before.lastIndexOf("\n") };
}

/** Offset of a 1-based line and column (clamped to the source). */
export function offsetOf(src: string, line: number, column: number): number {
  let off = 0;
  for (let l = 1; l < line; l++) {
    const nl = src.indexOf("\n", off);
    if (nl < 0) return src.length;
    off = nl + 1;
  }
  const eol = src.indexOf("\n", off);
  return Math.min(off + column - 1, eol < 0 ? src.length : eol);
}

const PAIRS: Record<string, string> = { "(": ")", "[": "]", "{": "}" };

/**
 * Unbalanced brackets, reported where a reader would look: the stray or
 * mismatched closer, or the opener that is never closed. Also unterminated
 * strings and comments. Precise where the parser's own message, in a
 * wrapped script, would be vague.
 */
export function checkBrackets(src: string, tokens = tokenize(src)): Problem | null {
  const stack: { ch: string; at: number }[] = [];
  for (const t of tokens) {
    const text = src.slice(t.start, t.end);
    if (t.kind === "string" && t.open) {
      return { ...position(src, t.start), message: text[0] === "`" ? "this template string is never closed" : "this string is never closed (strings end on the same line)" };
    }
    if (t.kind === "comment" && t.open) {
      return { ...position(src, t.start), message: "this comment is never closed with */" };
    }
    if (t.kind !== "punct") continue;
    if (PAIRS[text]) stack.push({ ch: text, at: t.start });
    else if (")]}".includes(text)) {
      const open = stack.pop();
      if (!open) return { ...position(src, t.start), message: `'${text}' has no matching opening bracket` };
      if (PAIRS[open.ch] !== text) {
        const where = position(src, open.at);
        return { ...position(src, t.start), message: `expected '${PAIRS[open.ch]}' to close the '${open.ch}' on line ${where.line}, found '${text}'` };
      }
    }
  }
  const open = stack.pop();
  return open ? { ...position(src, open.at), message: `this '${open.ch}' is never closed` } : null;
}

// ---------------------------------------------------------------- completions

export type CompletionKind = "function" | "property" | "event" | "object" | "keyword" | "name" | "label" | "value" | "word";

export interface Completion {
  label: string;
  kind: CompletionKind;
  /** Signature or a one-line note. */
  detail?: string;
  /** Text to insert instead of `label` (e.g. with parentheses); `$0` marks where the caret goes (else the end). */
  insert?: string;
}

const fn = (label: string, detail: string, args = true): Completion => ({
  label,
  kind: "function",
  detail,
  insert: args ? `${label}($0)` : `${label}()`,
});
const prop = (label: string, detail: string): Completion => ({ label, kind: "property", detail });
const ev = (label: string, detail: string): Completion => ({ label, kind: "event", detail, insert: `${label} = function () {\n  $0\n};` });

/** Members of every display object (and, through the script scope, bare names in a script). */
const DISPLAY_MEMBERS: Completion[] = [
  prop("x", "number: position (stage pixels)"),
  prop("y", "number: position (stage pixels)"),
  prop("rotation", "number: degrees"),
  prop("scaleX", "number: 1 = 100%"),
  prop("scaleY", "number: 1 = 100%"),
  prop("alpha", "number: 0 to 1"),
  prop("visible", "boolean"),
  prop("text", "string: text elements only"),
  prop("name", "string: the instance name"),
  prop("kind", '"root", "movieClip", "graphic", "button", "text", "shape" or "bitmap"'),
  prop("symbolName", "string: the symbol's library name"),
  prop("currentFrame", "number: from 1"),
  prop("totalFrames", "number"),
  prop("isPlaying", "boolean"),
  prop("parent", "the containing timeline"),
  prop("root", "the main timeline"),
  prop("onStage", "boolean: still displayed"),
  fn("play", "play(): run this timeline", false),
  fn("stop", "stop(): halt this timeline", false),
  fn("gotoAndPlay", 'gotoAndPlay(frame | "label")'),
  fn("gotoAndStop", 'gotoAndStop(frame | "label")'),
  fn("nextFrame", "nextFrame(): step and stop", false),
  fn("prevFrame", "prevFrame(): step back and stop", false),
  fn("getChildByName", "getChildByName(name)"),
  fn("getChildren", "getChildren(): named children on stage", false),
  fn("getBounds", "getBounds(): { x, y, width, height } on stage", false),
  fn("hitTestPoint", "hitTestPoint(x, y, shapeFlag)"),
  fn("hitTestObject", "hitTestObject(other)"),
  ev("onEnterFrame", "every frame"),
  ev("onPress", "buttons: pointer pressed"),
  ev("onRelease", "buttons: pointer released after a press"),
  ev("onClick", "buttons: pressed and released on it"),
];

const GLOBALS: Completion[] = [
  fn("trace", "trace(...values): print to the Output panel"),
  { label: "stage", kind: "object", detail: "width, height, fps, addEventListener" },
  { label: "Key", kind: "object", detail: "Key.isDown(key)" },
  { label: "Mouse", kind: "object", detail: "Mouse.x, Mouse.y, Mouse.isDown" },
  { label: "Math", kind: "object", detail: "Math.random(), Math.floor()…" },
  { label: "this", kind: "keyword", detail: "the timeline or instance the script belongs to" },
];

const OBJECT_MEMBERS: Record<string, Completion[]> = {
  Key: [fn("isDown", 'isDown("ArrowLeft" | "a" | "Space"…)')],
  Mouse: [prop("x", "number: stage x"), prop("y", "number: stage y"), prop("isDown", "boolean")],
  stage: [
    prop("width", "number"),
    prop("height", "number"),
    prop("fps", "number"),
    fn("addEventListener", 'addEventListener("enterFrame" | "keyDown"…, fn)'),
    fn("removeEventListener", "removeEventListener(type, fn)"),
  ],
  Math: [
    fn("random", "random(): 0 ≤ n < 1, seeded per session", false),
    fn("floor", "floor(n)"),
    fn("round", "round(n)"),
    fn("ceil", "ceil(n)"),
    fn("abs", "abs(n)"),
    fn("min", "min(a, b, …)"),
    fn("max", "max(a, b, …)"),
    fn("sqrt", "sqrt(n)"),
    fn("sin", "sin(radians)"),
    fn("cos", "cos(radians)"),
    fn("atan2", "atan2(y, x)"),
    fn("hypot", "hypot(dx, dy)"),
    prop("PI", "3.14159…"),
  ],
};

const STAGE_EVENTS = ["enterFrame", "keyDown", "keyUp", "mouseDown", "mouseUp", "mouseMove", "click", "buttonPress", "buttonRelease"];
const KEY_NAMES = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", " ", "Enter", "Escape", "Shift", "Space", "KeyA", "KeyD", "KeyS", "KeyW"];

const KEYWORD_COMPLETIONS: Completion[] = [
  "function",
  "var",
  "let",
  "const",
  "if",
  "else",
  "for",
  "while",
  "return",
  "true",
  "false",
  "null",
  "new",
  "typeof",
  "break",
  "continue",
].map((label) => ({ label, kind: "keyword" as const }));

/** Highlighted as API: functions, event handlers and global objects (not plain properties like `x`). */
const API_WORDS = new Set(
  [...DISPLAY_MEMBERS, ...GLOBALS].filter((c) => c.kind === "function" || c.kind === "event" || c.kind === "object").map((c) => c.label),
);

/** What the script can name besides the API: from `Engine.scriptNamesJson`. */
export interface ScriptNames {
  names: string[];
  labels: string[];
}

export interface CompletionResult {
  /** Where the replaced text starts; it ends at the caret. */
  from: number;
  items: Completion[];
}

/**
 * Completions at `caret`, or null where none make sense (inside comments,
 * numbers, ordinary strings). `explicit` (Ctrl+Space) also lists everything
 * for an empty prefix.
 */
export function complete(src: string, caret: number, names: ScriptNames, explicit = false): CompletionResult | null {
  const tokens = tokenize(src.slice(0, caret));
  const last = tokens[tokens.length - 1];
  if (last?.kind === "comment" || last?.kind === "number" || last?.kind === "regex") return null;

  // Inside a string: labels for gotoAndPlay("…"), events for addEventListener("…"), keys for isDown("…").
  if (last?.kind === "string") {
    if (!last.open) return null;
    const text = src.slice(last.start, caret);
    const callee = calleeBefore(src, tokens, tokens.length - 1);
    const values =
      callee === "gotoAndPlay" || callee === "gotoAndStop"
        ? names.labels.map((label): Completion => ({ label, kind: "label" }))
        : callee === "addEventListener" || callee === "removeEventListener"
          ? STAGE_EVENTS.map((label): Completion => ({ label, kind: "value" }))
          : callee === "isDown"
            ? KEY_NAMES.map((label): Completion => ({ label, kind: "value", detail: label === " " ? "space bar (key)" : undefined }))
            : null;
    if (!values) return null;
    return filter(last.start + 1, text.slice(1), values, true);
  }

  const word = last && (last.kind === "ident" || last.kind === "api" || last.kind === "keyword") && last.end === caret ? last : null;
  const prefix = word ? src.slice(word.start, caret) : "";
  const from = word ? word.start : caret;
  if (!prefix && !explicit) {
    // Right after a dot, members pop up without typing.
    if (!(last?.kind === "punct" && src.slice(last.start, last.end) === ".")) return null;
  }

  // Member access: `obj.` or `obj.pre`.
  const dotIndex = significantBefore(tokens, word ? tokens.length - 2 : tokens.length - 1);
  if (dotIndex >= 0 && src.slice(tokens[dotIndex].start, tokens[dotIndex].end) === ".") {
    const objIndex = significantBefore(tokens, dotIndex - 1);
    const obj = objIndex >= 0 ? src.slice(tokens[objIndex].start, tokens[objIndex].end) : "";
    const members = OBJECT_MEMBERS[obj] ?? [...DISPLAY_MEMBERS, ...names.names.map(nameItem)];
    return filter(from, prefix, members, true);
  }

  const items = [...names.names.map(nameItem), ...GLOBALS, ...DISPLAY_MEMBERS, ...KEYWORD_COMPLETIONS, ...wordsIn(src, from, prefix)];
  return filter(from, prefix, items, explicit);
}

const nameItem = (label: string): Completion => ({ label, kind: "name", detail: "instance on this timeline" });

/** Identifiers already in the script (variables, functions), for completion. */
function wordsIn(src: string, skipAt: number, prefix: string): Completion[] {
  const seen = new Set<string>();
  for (const t of tokenize(src)) {
    if (t.kind !== "ident" || t.start === skipAt) continue;
    const w = src.slice(t.start, t.end);
    if (w.length > 2 && w !== prefix) seen.add(w);
  }
  return [...seen].map((label) => ({ label, kind: "word" as const }));
}

function filter(from: number, prefix: string, items: Completion[], allowEmpty: boolean): CompletionResult | null {
  if (!prefix && !allowEmpty) return null;
  const p = prefix.toLowerCase();
  const seen = new Set<string>();
  const scored: { c: Completion; score: number; i: number }[] = [];
  items.forEach((c, i) => {
    if (seen.has(c.label)) return;
    const l = c.label.toLowerCase();
    // Prefix matches first, then camel-case/substring matches.
    const score = l.startsWith(p) ? 0 : p.length > 1 && l.includes(p) ? 1 : -1;
    if (score < 0 || (c.label === prefix && c.kind !== "function")) return;
    seen.add(c.label);
    scored.push({ c, score, i });
  });
  scored.sort((a, b) => a.score - b.score || a.i - b.i);
  return scored.length ? { from, items: scored.map((s) => s.c) } : null;
}

function significantBefore(tokens: Token[], i: number): number {
  while (i >= 0 && (tokens[i].kind === "space" || tokens[i].kind === "comment")) i--;
  return i;
}

/** The function name in `name(` or `obj.name(` just before token `i`. */
function calleeBefore(src: string, tokens: Token[], i: number): string | null {
  const paren = significantBefore(tokens, i - 1);
  if (paren < 0 || src.slice(tokens[paren].start, tokens[paren].end) !== "(") return null;
  const name = significantBefore(tokens, paren - 1);
  return name >= 0 && (tokens[name].kind === "ident" || tokens[name].kind === "api") ? src.slice(tokens[name].start, tokens[name].end) : null;
}

// ---------------------------------------------------------------- editing helpers

/** Leading whitespace of the line containing `offset`. */
export function indentAt(src: string, offset: number): string {
  const lineStart = src.lastIndexOf("\n", offset - 1) + 1;
  return /^[ \t]*/.exec(src.slice(lineStart))![0];
}

/**
 * Text for Enter at `caret`: newline, the line's indent, one more level
 * after an opening bracket, and the closer on its own line when the caret
 * sits between a pair (`{|}`). Returns the text and the caret offset in it.
 */
export function newlineAt(src: string, caret: number, unit = "  "): { text: string; caret: number } {
  const indent = indentAt(src, caret);
  const before = src.slice(0, caret).trimEnd();
  const after = src.slice(caret);
  const opener = before[before.length - 1];
  if (opener && PAIRS[opener]) {
    const inner = `\n${indent}${unit}`;
    if (after.trimStart()[0] === PAIRS[opener] && !after.slice(0, after.indexOf(PAIRS[opener])).includes("\n")) {
      return { text: `${inner}\n${indent}`, caret: inner.length };
    }
    return { text: inner, caret: inner.length };
  }
  return { text: `\n${indent}`, caret: indent.length + 1 };
}

/** The lines `[start, end)` touched by a selection, as offsets of whole lines. */
export function lineRange(src: string, a: number, b: number): { start: number; end: number } {
  const start = src.lastIndexOf("\n", a - 1) + 1;
  // A selection ending at the start of a line doesn't include that line.
  const endFrom = b > a && src[b - 1] === "\n" ? b - 1 : b;
  const nl = src.indexOf("\n", endFrom);
  return { start, end: nl < 0 ? src.length : nl };
}

/** Indents (or outdents) every line of `text`. */
export function shiftLines(text: string, outdent: boolean, unit = "  "): string {
  return text
    .split("\n")
    .map((l) => (outdent ? l.replace(new RegExp(`^( {1,${unit.length}}|\\t)`), "") : l.length ? unit + l : l))
    .join("\n");
}

/** Comments out every line with `// `, or uncomments when all already are. */
export function toggleComment(text: string): string {
  const lines = text.split("\n");
  const content = lines.filter((l) => l.trim());
  const all = content.length > 0 && content.every((l) => /^\s*\/\//.test(l));
  if (all) return lines.map((l) => l.replace(/^(\s*)\/\/ ?/, "$1")).join("\n");
  const indent = Math.min(...content.map((l) => /^\s*/.exec(l)![0].length));
  return lines.map((l) => (l.trim() ? `${l.slice(0, indent)}// ${l.slice(indent)}` : l)).join("\n");
}

// ---------------------------------------------------------------- highlighting

const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/**
 * Highlighted HTML (one `<span class="t-…">` per token). With a problem,
 * the text from its column to the end of its line is wrapped in
 * `<span class="t-error">` so the editor can underline it.
 */
export function highlight(src: string, problem?: Problem | null): string {
  let errFrom = -1;
  let errTo = -1;
  if (problem) {
    errFrom = offsetOf(src, problem.line, problem.column);
    const eol = src.indexOf("\n", errFrom);
    errTo = eol < 0 ? src.length : eol;
    // Nothing to underline at the end of a line: mark the last character instead.
    if (errTo === errFrom && errFrom > 0 && src[errFrom - 1] !== "\n") errFrom--;
  }
  let html = "";
  for (const t of tokenize(src)) {
    // Split tokens at the error boundaries so the underline is exact.
    const cuts = [t.start, ...[errFrom, errTo].filter((c) => c > t.start && c < t.end), t.end];
    for (let k = 0; k + 1 < cuts.length; k++) {
      const [a, b] = [cuts[k], cuts[k + 1]];
      const text = escapeHtml(src.slice(a, b));
      const inErr = a >= errFrom && b <= errTo && errTo > errFrom;
      const cls = t.kind === "space" || t.kind === "punct" || t.kind === "ident" ? "" : `t-${t.kind}`;
      const inner = cls ? `<span class="${cls}">${text}</span>` : text;
      html += inErr ? `<span class="t-error">${inner}</span>` : inner;
    }
  }
  // A trailing newline needs content after it, or the last line collapses.
  return html + "\n";
}
