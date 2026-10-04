# Scripting

Zoetrope projects are scripted in **JavaScript** (ES2023) with a small
Flash-like API. ActionScript source compatibility is a non-goal. Scripts run
the same way in the editor's preview (▶) and in exported HTML, because both use
the same runtime (`editor/src/runtime/`) on the same WASM core.

## Where scripts live

| Kind | Attached to | Runs | `this` |
|------|-------------|------|--------|
| **Frame script** | a keyframe (Properties → frame → *Label & script*) | each time that timeline's playhead **enters** the keyframe's first frame | the timeline: `root`, or the movie clip |
| **Symbol script** | a symbol (Library → `{ }`) | once for **every instance** when it appears on stage | the instance |

The script editor (`ScriptEditor.tsx`, language support in `scriptlang.ts`)
highlights JavaScript, completes the API, the timeline's instance names
(`Engine.scriptNamesJson`), the script's own variables, frame labels inside
`gotoAndPlay("…")`, stage event names and key names, and shows syntax errors
as you type. Bracket mistakes are found by its tokenizer; everything else by
compiling the script in QuickJS exactly as playback wraps it
(`checkSyntax` in `runtime/scripting.ts`), without running it.

The timeline marks keyframes that have a script with an **a**, and labelled
keyframes with **⚑name**. Frame labels are names for `gotoAndPlay("name")`.

### Scope

Inside frame and symbol scripts, **free names resolve against `this`**, as on
a Flash timeline:

- `stop()`, `gotoAndPlay(5)` and similar calls act on this timeline.
- `bee` is this timeline's child with the instance name `bee`.
- `var score = 0` becomes a property of the timeline (`this.score`), which its
  other frame scripts and handlers can read.
- `let` and `const` stay local to the script; `function` declarations do too.
  Closures keep working.
- Globals (`stage`, `root`, `Key`, `Mouse`, `trace`, `Math`, …) always win over
  child names.

Reading an unknown name gives `undefined` rather than throwing, as in Flash.

## Display objects

Every named element on a timeline, whether a symbol instance, text, shape or
bitmap, is reachable as an object. `root` is the main timeline.

| Member | Notes |
|--------|-------|
| `x`, `y`, `rotation`, `scaleX`, `scaleY`, `alpha`, `visible` | read/write. Writing takes the property over from the timeline: from then on the animation no longer moves it. |
| `text` | read/write, text elements only |
| `name`, `kind`, `symbolName` | `kind`: `"root"`, `"movieClip"`, `"graphic"`, `"button"`, `"text"`, `"shape"` or `"bitmap"` |
| `currentFrame`, `totalFrames`, `isPlaying` | root and movie clips (frames count from **1**) |
| `play()`, `stop()` | root and movie clips |
| `gotoAndPlay(f)`, `gotoAndStop(f)` | `f` = frame number (from 1) or label |
| `nextFrame()`, `prevFrame()` | step and stop |
| `parent`, `root` | |
| `childName` / `getChildByName(n)` / `getChildren()` | named children, as currently displayed |
| `hitTestPoint(x, y, shapeFlag)` | stage point inside the object: drawn shapes when `shapeFlag` is true, otherwise its bounds |
| `hitTestObject(other)` | whether the stage bounds overlap |
| `getBounds()` | `{ x, y, width, height }` on stage |
| `onStage` | whether the object is still on stage |

Any other property you assign (`this.speed = 3`) is a plain script property.
Objects are stable: `root.bee === bee` is `true`. When an instance leaves the
stage, its object, handlers and scripted properties go with it. If it comes
back, it is a fresh instance, and its symbol script runs again.

## Events

Assign a function to one of these properties on an object:

| Handler | When |
|---------|------|
| `onEnterFrame` | every frame, on every object that has one, in creation order |
| `onPress` | pointer pressed on a button |
| `onRelease` | pointer released after pressing that button, wherever it is released |
| `onClick` | pressed and released on the same button |

Stage-wide events go through
`stage.addEventListener(type, fn)` and `stage.removeEventListener(type, fn)`:

- `"enterFrame"` fires every frame.
- `"keyDown"` and `"keyUp"` pass `{ key, code }`.
- `"mouseDown"`, `"mouseUp"` and `"mouseMove"` pass `{ x, y }` in stage
  coordinates.
- `"click"`, `"buttonPress"` and `"buttonRelease"` pass the button.

## Globals

| Global | Notes |
|--------|-------|
| `stage` | `width`, `height`, `fps`, plus `addEventListener` / `removeEventListener` |
| `root` | the main timeline |
| `Key.isDown(k)` | `k` is a `KeyboardEvent.key` (`"ArrowLeft"`, `"a"`, `" "`) or `.code` (`"KeyA"`, `"Space"`) |
| `Mouse.x`, `Mouse.y`, `Mouse.isDown` | pointer in stage coordinates |
| `trace(...values)` | writes to the editor's **Output** panel (the browser console in exports) |

Standard JavaScript built-ins (`Math`, `JSON`, `Map`, `Date`, …) are
available. There is no `setTimeout`: use `onEnterFrame` and count frames.

## Order within a frame

1. Every playing timeline advances one frame (the root, then each movie clip).
2. Instances that left the stage are dropped.
3. Symbol scripts run for instances that appeared, parents first.
4. Frame scripts run for timelines that entered a frame, parents first, then
   layers bottom to top. A guide layer's scripts don't run.
5. `onEnterFrame` handlers run, then `"enterFrame"` listeners.
6. The frame is drawn and sound is updated.

A `gotoAnd…` to a different frame takes effect immediately: properties and
children reflect the new frame at once. That frame's scripts run right after
the current script finishes. Jumping to the frame you are already on does
nothing.

Pointer and keyboard events are delivered between frames as they happen, and
any frame scripts they cause by jumping run straight away.

## Sandbox and limits

Scripts run in **QuickJS**, a separate JavaScript engine compiled to
WebAssembly. They cannot reach anything outside it:

- **No page access:** no `window`, no `document`, no DOM.
- **No network and no files:** no `fetch` or `XMLHttpRequest`, no `require`,
  no imports.
- **The only way out** is the runtime API above. It goes through
  `zoetrope_core::script`, the same core the editor and the player use.

**Limits:**
- **Time:** each entry (one frame script, or one round of handlers) may run for
  250 ms. Longer than that, it is stopped and reported as an endless loop. The
  editor and the player stay responsive.
- **Memory:** 64 MB.
- **Stack:** 1 MB.

**Errors** are reported with their location and line number, for example
`Bee Catcher › Actions › frame 1: TypeError: not a function (line 5)` or
`onClick of restartButton: …`. In the editor they appear in the **Output**
panel, which opens on the first error; in exports they go to the browser
console. The movie keeps running.

**State:** each preview or page load starts a fresh sandbox; nothing survives
between sessions. Network and storage capabilities may later be granted per
project, but none exist now.

## Example (the "Game demo")

```js
// Frame 1, label "play"
stop();
const speed = 7;
var score = 0;

function placeFlower() {
  flower.x = 80 + Math.random() * (stage.width - 160);
  flower.y = 140 + Math.random() * (stage.height - 240);
}

this.onEnterFrame = function () {
  if (Key.isDown("ArrowLeft")) bee.x -= speed;
  if (Key.isDown("ArrowRight")) bee.x += speed;
  if (bee.hitTestObject(flower)) {
    score += 1;
    placeFlower();
  }
  scoreText.text = "Score: " + score;
};
restartButton.onClick = function () {
  score = 0;        // already on "play": jumping there again would do nothing
  placeFlower();
};
```

Frame 2 (label `"over"`) can bring the player back with
`restartButton.onClick = function () { gotoAndStop("play"); };`, which
enters frame 1 again and so re-runs its script.
