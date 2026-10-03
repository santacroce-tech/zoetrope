// Scripting runtime behaviour (docs/SCRIPTING.md), end to end through the
// sandbox and the core: the same code the preview and exports run.
import { test } from "node:test";
import assert from "node:assert/strict";
import { layerId, setup, start, withScript } from "./harness";
import { Engine } from "../src/wasm/pkg/zoetrope_web.js";

test("frame scripts see the timeline: vars are timeline properties, let/const stay local", async () => {
  const s = await withScript(`
    var score = 41;
    let hidden = 1;
    score += 1;
    trace(this.score, typeof hidden, root === this, this.kind);
    this.onEnterFrame = function () { trace("tick", score); this.onEnterFrame = null; };
  `);
  s.runtime.advance(1);
  assert.deepEqual(s.traces(), ["42 number true root", "tick 42"]);
  assert.deepEqual(s.errors(), []);
  s.runtime.stop();
});

test("named children are objects; properties write through to the core", async () => {
  const s = await withScript(`
    trace(typeof bee1, bee1.kind, bee1.symbolName, bee1 === root.bee1, title.kind);
    bee1.x = 400; bee1.rotation = 30;
    title.text = "Hello";
    trace(bee1.x, bee1.rotation, title.text, playButton.kind);
    trace(String(nobody));
  `);
  assert.deepEqual(s.traces(), ["object movieClip Bee true text", "400 30 Hello button", "undefined"]);
  s.runtime.stop();
});

test("the sandbox has no page, network or host access", async () => {
  const s = await withScript(`
    trace([typeof window, typeof document, typeof fetch, typeof XMLHttpRequest, typeof require, typeof process, typeof setTimeout].join(","));
  `);
  assert.deepEqual(s.traces(), ["undefined,undefined,undefined,undefined,undefined,undefined,undefined"]);
  s.runtime.stop();
});

test("errors are reported with their location and line, and the movie keeps running", async () => {
  const s = await withScript(`trace("before");\n\nnotAFunction();\ntrace("never");`);
  const [e] = s.errors();
  assert.equal(e.where, "Scene 1 › Actions › frame 1");
  assert.match(e.text, /not a function|not defined/);
  assert.match(e.text, /\(line 3\)/);
  s.runtime.advance(5);
  assert.equal(s.engine.playFrame(), 5);
  s.runtime.stop();
});

test("an endless loop is stopped by the CPU budget", async () => {
  const s = await withScript(`stage.addEventListener("keyDown", function () { while (true) {} });`);
  const t0 = performance.now();
  s.runtime.key("keyDown", "x");
  assert.ok(performance.now() - t0 < 2000, "interrupted promptly");
  assert.match(s.errors()[0].text, /longer than 250 ms/);
  s.runtime.advance(1);
  assert.equal(s.engine.playFrame(), 1, "still playing");
  s.runtime.stop();
});

test("a runaway allocation is stopped (memory or time limit), not the page", async () => {
  const s = await withScript(`var a = []; while (true) a.push(new Array(100000).fill(1));`);
  assert.equal(s.errors().length, 1);
  s.runtime.advance(1);
  assert.equal(s.engine.playFrame(), 1);
  s.runtime.stop();
});

test("stop, play and goto by label drive the root; frame scripts run after a jump", async () => {
  await setup();
  const engine = new Engine();
  engine.newDemo("animation");
  const actions = JSON.stringify([layerId(engine, "Actions")]);
  // Split the Actions layer: a keyframe at 10 labelled "middle" with its own script.
  engine.insertKeyframe(actions, 10, true);
  engine.setFrameLabel(actions, 10, "middle");
  engine.setFrameScript(actions, 10, `trace("middle", currentFrame); stop();`);
  engine.setFrameScript(actions, 0, `trace("start"); gotoAndPlay("middle"); trace("after goto", currentFrame);`);
  const s = await start(engine);
  assert.deepEqual(s.traces(), ["start", "after goto 11", "middle 11"]);
  s.runtime.advance(3);
  assert.equal(s.engine.playFrame(), 10, "stopped on the labelled frame");
  s.runtime.stop();
});

test("order within a frame: entered frame scripts, then onEnterFrame", async () => {
  const s = await withScript(`
    if (!this.started) { this.started = true; this.onEnterFrame = function () { trace("enterFrame", currentFrame); }; }
    trace("frame script", currentFrame);
  `);
  s.runtime.advance(1);
  assert.deepEqual(s.traces(), ["frame script 1", "enterFrame 2"]);
  s.runtime.stop();
});

test("keyboard state and stage key events", async () => {
  const s = await withScript(`
    stage.addEventListener("keyDown", function (e) { trace("down", e.key, Key.isDown("ArrowLeft"), Key.isDown(e.code)); });
    stage.addEventListener("keyUp", function (e) { trace("up", e.key, Key.isDown("ArrowLeft")); });
  `);
  s.runtime.key("keyDown", "ArrowLeft", "ArrowLeft");
  s.runtime.key("keyUp", "ArrowLeft", "ArrowLeft");
  assert.deepEqual(s.traces(), ["down ArrowLeft true true", "up ArrowLeft false"]);
  s.runtime.stop();
});

test("button handlers: onPress, onRelease, onClick", async () => {
  const s = await withScript(`
    playButton.onPress = function () { trace("press", this.name); };
    playButton.onRelease = function () { trace("release"); };
    playButton.onClick = function () { trace("click"); };
  `);
  s.runtime.pointer({ x: 100, y: 500 }, true, true);
  s.runtime.pointer({ x: 100, y: 500 }, true, false);
  s.runtime.pointer({ x: 100, y: 500 }, true, true);
  s.runtime.pointer({ x: 500, y: 100 }, true, false); // released elsewhere: no click
  assert.deepEqual(s.traces(), ["press playButton", "release", "click", "press playButton", "release"]);
  s.runtime.stop();
});

test("Math.random is seeded per session: replayable, and different seeds differ", async () => {
  const draw = async (seed: number) => {
    const s = await withScript(`trace(Math.random(), Math.random());`, { seed });
    s.runtime.stop();
    return s.traces()[0];
  };
  assert.equal(await draw(7), await draw(7));
  assert.notEqual(await draw(7), await draw(8));
});

test("the game demo replays identically with the same inputs", async () => {
  const play = async () => {
    await setup();
    const engine = new Engine();
    engine.newDemo("game");
    const s = await start(engine, 3);
    const digests: string[] = [];
    for (let i = 0; i < 200; i++) {
      if (i === 10) s.runtime.key("keyDown", "ArrowRight");
      if (i === 80) s.runtime.key("keyDown", "ArrowDown");
      if (i === 120) s.runtime.key("keyUp", "ArrowRight");
      digests.push(s.runtime.digest());
      s.runtime.advance(1);
    }
    assert.deepEqual(s.errors(), []);
    s.runtime.stop();
    return digests;
  };
  const a = await play();
  assert.deepEqual(await play(), a);
  assert.ok(new Set(a).size > 150, "the picture changes as the game plays");
});

test("objects of instances that left the stage are dropped with their handlers", async () => {
  await setup();
  const engine = new Engine();
  engine.newDemo("game");
  const s = await start(engine);
  // Frame 2 ("over") has no bee: its object goes away, then comes back fresh.
  s.runtime.key("keyDown", "x");
  const before = s.output.length;
  s.engine.scriptCall(JSON.stringify({ op: "goto", path: [], frame: "over", play: false }));
  s.runtime.advance(2);
  assert.deepEqual(s.errors(), [], "no handler ran against a missing bee");
  assert.ok(s.output.length >= before);
  s.runtime.stop();
});
