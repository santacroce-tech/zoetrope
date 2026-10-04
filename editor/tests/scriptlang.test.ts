// The script editor's language support (src/scriptlang.ts) and its syntax
// check, which compiles exactly as playback does.
import { test } from "node:test";
import assert from "node:assert/strict";
import { layerId, setup } from "./harness";
import { checkSyntax } from "../src/runtime/scripting";
import { checkBrackets, complete, highlight, newlineAt, shiftLines, toggleComment, tokenize } from "../src/scriptlang";
import { Engine } from "../src/wasm/pkg/zoetrope_web.js";

const NAMES = { names: ["bee1", "playButton", "title"], labels: ["intro", "gameOver"] };
const at = (src: string) => ({ src: src.replace("|", ""), caret: src.indexOf("|") });
const labels = (src: string, explicit = false) => {
  const { src: s, caret } = at(src);
  return complete(s, caret, NAMES, explicit)?.items.map((c) => c.label) ?? null;
};

test("tokens: strings, comments, regex vs division, unterminated strings", () => {
  const kinds = (src: string) => tokenize(src).filter((t) => t.kind !== "space").map((t) => `${t.kind}:${src.slice(t.start, t.end)}`);
  assert.deepEqual(kinds(`x = a / b / 2; // done`), ["ident:x", "punct:=", "ident:a", "punct:/", "ident:b", "punct:/", "number:2", "punct:;", "comment:// done"]);
  assert.deepEqual(kinds(`if (/ab+c/i.test(s)) stop()`).slice(2, 3), ["regex:/ab+c/i"]);
  assert.deepEqual(kinds(`gotoAndPlay("it's")`), ["api:gotoAndPlay", "punct:(", `string:"it's"`, "punct:)"]);
  assert.equal(tokenize(`"abc\nx`)[0].open, true);
  assert.equal(tokenize("`a\nb` + 1")[0].open, undefined);
  assert.equal(tokenize(`"a\\\\"`)[0].open, undefined);
});

test("bracket problems point at the right place", () => {
  assert.equal(checkBrackets("if (a) {\n  stop();\n}"), null);
  assert.deepEqual(checkBrackets("stop();\n}\n"), { line: 2, column: 1, message: "'}' has no matching opening bracket" });
  assert.deepEqual(checkBrackets("this.onEnterFrame = function () {\n  x += 1;\n"), {
    line: 1,
    column: 33,
    message: "this '{' is never closed",
  });
  assert.equal(checkBrackets("f(a];")?.message, "expected ')' to close the '(' on line 1, found ']'");
  assert.equal(checkBrackets('trace("hi);\nstop();')?.line, 1);
  assert.equal(checkBrackets("// a { in a comment\ntrace('}')"), null);
});

test("completions: API, instance names, members, labels, events and keys", () => {
  assert.deepEqual(labels("goto|")?.slice(0, 2), ["gotoAndPlay", "gotoAndStop"]);
  assert.ok(labels("be|")!.includes("bee1"));
  // After a dot: display-object members (and children), without typing.
  const members = labels("bee1.|")!;
  assert.ok(members.includes("x") && members.includes("hitTestObject") && members.includes("onEnterFrame"));
  assert.deepEqual(labels("Key.|"), ["isDown"]);
  assert.deepEqual(labels("stage.add|"), ["addEventListener"]);
  assert.deepEqual(labels("Math.flo|"), ["floor"]);
  assert.deepEqual(labels('gotoAndPlay("|'), ["intro", "gameOver"]);
  assert.deepEqual(labels('root.gotoAndStop("ga|'), ["gameOver"]);
  assert.ok(labels('stage.addEventListener("key|')!.includes("keyDown"));
  assert.ok(labels('Key.isDown("Arr|')!.includes("ArrowLeft"));
  // Variables already in the script.
  assert.ok(labels("var speed = 4;\nsp|")!.includes("speed"));
  // Nothing in comments, numbers, ordinary strings, or with no prefix unless asked.
  assert.equal(labels("// sto|"), null);
  assert.equal(labels('trace("sto|'), null);
  assert.equal(labels("x = 12|"), null);
  assert.equal(labels("x = |"), null);
  assert.ok(labels("x = |", true)!.length > 20);
  // Functions insert their parentheses, events a handler.
  const { src, caret } = at("sto|");
  assert.deepEqual(complete(src, caret, NAMES)!.items[0], {
    label: "stop",
    kind: "function",
    detail: "stop(): halt this timeline",
    insert: "stop()",
  });
  assert.equal(complete("gotoAndS", 8, NAMES)!.items[0].insert, "gotoAndStop($0)");
});

test("editing helpers: indent on Enter, shift and comment lines", () => {
  assert.deepEqual(newlineAt("  if (a) {}", 10), { text: "\n    \n  ", caret: 5 });
  assert.deepEqual(newlineAt("  stop();", 9), { text: "\n  ", caret: 3 });
  assert.deepEqual(newlineAt("f(", 2), { text: "\n  ", caret: 3 });
  assert.equal(shiftLines("a\n\n  b", false), "  a\n\n    b");
  assert.equal(shiftLines("  a\n b\nc", true), "a\nb\nc");
  assert.equal(toggleComment("  a\n\n    b"), "  // a\n\n  //   b");
  assert.equal(toggleComment("  // a\n\n  //   b"), "  a\n\n    b");
});

test("highlighting escapes HTML and underlines the error to the end of its line", () => {
  const html = highlight("if (a < b) trace('<b>');\nstop()", { line: 1, column: 12, message: "x" });
  assert.ok(html.includes("&lt;") && !html.includes("<b>"));
  assert.match(html, /<span class="t-keyword">if<\/span>/);
  assert.match(html, /<span class="t-error"><span class="t-api">trace<\/span><\/span>/);
  assert.ok(!/t-error[^\n]*stop/.test(html), "the next line isn't underlined");
});

test("syntax check compiles as playback does and reports script lines", async () => {
  await setup();
  assert.equal(await checkSyntax("stop();\nthis.onEnterFrame = function () { x += 1; };"), null);
  // `return` is fine: scripts run as function bodies.
  assert.equal(await checkSyntax("if (done) return;"), null);
  assert.deepEqual(await checkSyntax("stop();\nvar x = ;"), { line: 2, column: 9, message: "unexpected token in expression: ';'" });
  // Line 1 columns don't include the wrapper.
  assert.equal((await checkSyntax("var = 3;"))?.column, 5);
  // Errors at the very end land on the script's last line.
  assert.equal((await checkSyntax("x = (1 +\n"))?.line, 2);
});

test("the engine lists a timeline's instance names and labels", async () => {
  await setup();
  const e = new Engine();
  e.newDemo("animation");
  e.setFrameLabel(JSON.stringify([layerId(e, "Actions")]), 0, "intro");
  const n = JSON.parse(e.scriptNamesJson(undefined));
  for (const name of ["bee1", "playButton", "title"]) assert.ok(n.names.includes(name), name);
  assert.deepEqual([...n.names].sort(), n.names);
  assert.deepEqual(n.labels, ["intro"]);
});
