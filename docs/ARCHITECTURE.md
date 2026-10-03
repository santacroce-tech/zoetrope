# Architecture

Zoetrope has three parts with strict boundaries. The guiding rule: **anything
that affects what a frame looks like, or what an edit does to the model, lives
in the core**, so the editor preview and the exported player can never
diverge, and the UI stays a thin shell.

```
┌────────────────────── Tauri window (webview) ──────────────────────┐
│  editor/src  (TypeScript + React)                                  │
│    UI state only: selection, tool, active layer, view (zoom/pan),  │
│    panels; draws editor chrome (handles, grid, rulers, marquee)    │
│        │  JSON strings / u32 ids / stage coords / thrown Errors    │
│        ▼                                                           │
│  crates/zoetrope-web  (WASM: Engine API + Canvas2dRenderer)        │
│        │  plain Rust calls                                         │
│        ▼                                                           │
│  crates/zoetrope-core (model, edits/undo, geometry, queries,       │
│     interaction math, render walk, file format) — no platform deps │
└────────┬───────────────────────────────────────────────────────────┘
         │ invoke(save_project | open_project | export_begin/write | pick_files | read_picked_file)
         ▼
  editor/src-tauri  (Rust native shell: dialogs + filesystem only)
```

**Runtime and export.** Playback (editor preview ▶ and exported HTML) is one
module, `editor/src/runtime/`. It drives the core's runtime player, the
script sandbox, input and audio. The exported player
(`editor/src/player/main.ts`) is that module plus the **same**
`zoetrope_web_bg.wasm`, bundled into one script with no editor code:

```
 editor preview (React)      exported page (player.js, no React)
          └──────────┬──────────────┘
          editor/src/runtime/  Runtime: ticks at the stage fps, input, audio
             │  scriptCall / playScriptsJson        │ playAudioJson
             ▼                                      ▼
  ScriptHost: QuickJS sandbox (prelude.ts)    AudioEngine (WebAudio)
             │  __host(json): the only exit
             ▼
  zoetrope_core::script → player::Player (clocks, goto, overrides, hit tests)
```

## crates/zoetrope-core

| Module     | Responsibility |
|------------|----------------|
| `model`    | `Project → Symbol → Layer tree → Element`. Elements are `Shape`, `Bitmap`, `Text` or `Instance` (of another symbol), so the evaluated scene is a **tree of instances**. Also covers `Transform` (with pivot) and its matrix decomposition, appearance (opacity/blend/tint), and validation. |
| `edit`     | `Edit`: primitive mutations (transforms, element insert/remove/replace/reorder, layer insert/remove/props, assets, stage). `Edit::apply` validates and returns the exact inverse. |
| `history`  | `Document` = project + undo/redo. `execute(label, edits)` is atomic (it rolls back on failure). Dirty tracking uses state ids. |
| `ops`      | High-level commands that build `Edit`s: move, duplicate, delete, patch properties, resize, create shapes, import images, align/distribute, arrange, layer add/delete/move/props, stage. The UI only calls these. |
| `query`    | Tight bounds, hit-testing (fill containment, stroke distance, recursion into instances; hidden and locked layers are skipped), marquee, selection handle geometry. |
| `interact` | Direct-manipulation math: `TransformSession` (move/scale/rotate/skew/pivot drags with modifiers), `EditSession` (anchor/handle/gradient drags), `PenSession` (the pen tool's state machine), snapping (grid, objects, stage), shape-tool drags (incl. polygon/star). |
| `vector`   | Editable `VectorPath` (subpaths of anchors with bezier handles): primitive→path conversion, split/insert, delete, convert corner⇄smooth, handle constraints, nearest-point, freehand fitting (RDP simplification + Catmull-Rom smoothing), polystar. |
| `timeline` | Keyframes, tweens and easing (presets + cubic-bezier). `evaluate_layer(layer, frame)` produces the elements shown at a frame (interpolated when tweened); also covers path morphing for shape tweens. |
| `player`   | The runtime: a stateful tree of per-instance clocks (movie clips, each playing or stopped), button pointer state and events (`Press`, `Release`, `Click`), and sound cues (stream positions, triggered events). It also handles scripting: `goto` by frame or label, property overrides, frames entered (queuing their scripts), instances that appeared or left the stage, name lookup and hit tests. |
| `script`   | The script bridge: `Call` (one JSON-shaped operation: timeline, play/stop, goto, get/set properties, children, hit tests, bounds) and `call(project, player, call)`. Everything a script can do goes through here, so its meaning lives in the core. See SCRIPTING.md. |
| `text`     | Static text: shaping with rustybuzz (kerning, ligatures), wrapping, alignment, glyph outlines → `Path`. Bundles the default font (Zoetrope Sans). The only text dependency; renderers only ever fill paths. |
| `paint`    | `Paint` (solid, linear, radial with focal point), `PaintStyle` (geometry-free tool form, fitted to shapes), `Stroke` (caps, joins, miter, dashes), `FillRule`. |
| `geom`     | `Path` (move/line/quad/cubic/close), deterministic flattening, containment (non-zero / even-odd), outline distance, `Rect`. |
| `render`   | `Renderer` trait, `render_frame` tree walk, `RecordingRenderer` (tests/determinism), `NullRenderer` (profiling). |
| `asset`    | Embedded assets (images, fonts, audio): base64 (de)serialization, PNG/JPEG/GIF header sniffing (so the core is the authority on image size), and audio container sniffing. |
| `format`   | Versioned JSON envelope, migration chain, validation. See FORMAT.md. |
| `outline`  | Panel views: the layer tree and per-element info. |
| `demo`     | The built-in demos: the animation scene and the scripted game ("Bee Catcher", the Phase 7 gate). |

### Clocks

`render_with_clock(project, frame, opts, &dyn Clock, …)` walks the tree
carrying each instance's path, and asks the `Clock` which frame a nested
symbol shows. `Stateless` applies the timeline rules directly (editing,
scrubbing); `Player` answers from its runtime state (preview, export). See
FORMAT.md, "Symbol timing".

### Scopes

Selection, hit-testing and transform sessions work within a **scope**: the
symbol whose elements are editable, its symbol→stage matrix, and the
**frame** of its timeline being edited. Editing a symbol in place pushes an
edit level (`Engine.levels`): the scope becomes that symbol, with the matrix
composed down the instance path as displayed at the parent's frame. A symbol
opened from the library sits at the stage center. `render_editing` draws the
rest of the scene dimmed, without the edited instance, then the symbol on top.
Drawing tools convert stage points into the scope's space. Every query evaluates the scene at
that frame, so you can select an object mid-tween. Direct manipulation of
interpolated ("tweened") frames is refused: the user inserts a keyframe (F6)
to pose there. Property edits on a tweened frame apply to the tween's start
keyframe, and the panel says so. Today the
scope is always the root timeline with the identity matrix. Phase 5's
edit-in-place supplies other scopes without changing the query or interaction
code.

### Rendering contract

`render_frame(project, frame, opts, &mut dyn Renderer)` walks the root symbol
→ layer tree (bottom→top; hidden layers and folders skipped; guide layers only
when `opts.show_guides`) → elements (back→front). It composes
`parent · translate · rotate · skew · scale · translate(-pivot)` down the tree.

The `Renderer` trait has seven methods: `begin_frame`, `fill_path`,
`stroke_path`, `draw_image`, `begin_group`, `end_group`, `end_frame`.

* Paints arrive **final**: the core applies the composed color transform
  (tint, then opacity, child first) to solid colors and to every gradient
  stop before calling the backend. Gradient geometry, stroke widths and
  dashes are in path coordinates, so they follow the `transform` the backend
  is given. `fill_path` also receives the fill rule.
* Images receive the `ColorTransform` because only the backend can recolor
  pixels. In Phase 2 its RGB multipliers are always uniform (tint + alpha
  form). The Canvas2D backend implements tint with a `source-atop` fill and
  alpha with `globalAlpha`.
* A non-`normal` blend mode wraps the element in `begin_group(blend, opacity)`
  / `end_group`. Canvas2D renders groups to pooled offscreen canvases and
  composites them with `globalCompositeOperation`.

Output is deterministic: the same project, frame and options always produce
the same call sequence (asserted in tests).

### Undo contract

Every mutation is an `Edit`. Timeline structure changes (frames,
keyframes, tweens) use one edit, `SetKeyframes`, which replaces a layer's
keyframe list and so inverts trivially. Every transaction is undoable, including
layer visibility/lock, renames and reordering. By design:
`Project::next_id` is monotonic and not rolled back (ids are never reused),
and loading a file replaces the document and clears history.

**Drags**: a `TransformSession` previews by writing transforms directly into
the project (no history entries). `commit` restores the initial transforms and
replays the final ones as one transaction, so a whole drag is one undo step,
and `cancel` (Esc) restores. Undo/redo cancel any open session first.

## Boundary: core ↔ frontend (zoetrope-web `Engine`)

* `new Engine()` holds one `Document`, an optional `TransformSession` and the
  decoded-image cache.
* **Queries** return JSON strings: `stageJson`, `layersJson`, `historyJson`,
  `elementJson(id)`, `selectionJson(ids)`, `marquee(...)`, `selectAll()`,
  `validSelection(ids)`, `hitTest(x, y, tol)`, `snapPoint`, `shapePreview`,
  `saveJson`.
* **Commands** take `u32` ids, JSON id lists and stage-space numbers. On
  failure they throw a JS `Error` with a readable message and leave the
  document unchanged.
* **Drags**: `beginTransform(ids, mode, x, y)` → `updateTransform(x, y, mods,
  snap)` (returns snap guides) → `endTransform()` or `cancelTransform()`.
  Path and gradient drags follow the same pattern with
  `beginEdit(id, target)` / `updateEdit` / `endEdit` / `cancelEdit`.
* **Timeline**: `setFrame` moves the editing playhead (all queries and
  commands use it). `timelineLength`, `insertFrames`, `removeFrames`,
  `insertKeyframe` (copy or blank), `clearKeyframe`, `setTween`, and
  `easingCurveJson` (samples a curve for the easing editor, so the UI never
  evaluates easing itself). `render(…, onionJson)` takes onion settings.
  Playback is driven by the UI's clock: frame = start + ⌊elapsed·fps⌋,
  looping or stopping at the end, calling `setFrame` + `render`.
* **Symbols**: `convertToSymbol`, `libraryJson`, `renderSymbolPreview`,
  `setSymbolProps`, `duplicateSymbol`, `deleteSymbol`, `swapSymbol`,
  `placeInstance`; editing levels with `enterInstance` / `enterSymbol` /
  `exitTo` / `breadcrumbJson` / `repairEditStack` (pops levels an undo made
  invalid).
* **Preview runtime** (main timeline only): `playStart`, `playTick(n)`
  (returns the main frame; capped per call), `playPointer` (button state and
  events), `playRender`, `playStop`.
* **Text**: `createText(layer, x, y, style, width?)` creates an empty text and
  starts typing into it. `beginTextEdit(id)` starts typing into an existing
  one. `updateText(s)` previews live with no history entry. `endTextEdit()`
  commits one undo step; a new text that was left empty is discarded without
  a trace in history. `textEditId()` lets the UI notice when undo, load or
  playback ended the typing. The UI's text field only captures keystrokes
  (and IME input), and the canvas shows the real shaped result.
* **Fonts & audio**: `fontsJson`, `importFont(name, bytes)`, `audioJson`,
  `importAudio(name, bytes, duration)`, `assetBytes(id)` (for the platform's
  decoder), `setSound(layers, frame, sound)`, and `playAudioJson()` (during
  preview: `{ streams, events }`, where events are drained by the call).
  `editor/src/runtime/audio.ts` (`AudioEngine`) applies that state with
  WebAudio, scheduling nothing while the browser keeps audio suspended. It
  decodes buffers once and preloads them before playback starts. It keeps
  each stream within 0.12 s of its cue position, starts events with their
  loops, and stops everything when playback stops. It holds no timing logic
  of its own, so the exported player can reuse it as is.
* **Export**: `savePack()` / `loadPack(bytes)` (FORMAT.md, "Pack"), and
  `publishJson()` / `setPublish(json)` for the export settings, saved with
  the project and undoable. `playDigest()` fingerprints what the runtime shows
  and plays, for the parity checks.
* **Scripting**: `playScriptsJson()` drains what is due:
  `{ removed, instances, frames }`, where `instances` are symbol scripts and
  `frames` are frame scripts, each with its path, source and a readable
  location. `scriptCall(json)` performs one bridge call, throwing a readable
  error on misuse. `playFrame()` gives the runtime's root frame. The editing
  commands are `setFrameScript`, `setFrameLabel`, `setSymbolScript` and
  `symbolScript`. `newDemo("game")` loads the scripted demo.
* **Script sandbox** (`editor/src/runtime/scripting.ts`): QuickJS compiled to
  WASM, via `quickjs-emscripten-core` and the single-file release variant.
  This is the one dependency Phase 7 adds. It's chosen because the engine is
  separate from the page: scripts have no DOM, network or storage. It's also
  synchronous, so `stop()` takes effect immediately, and the same build runs
  in the editor and in exports. One VM per playback session, with a 250 ms CPU
  budget per entry (interrupt handler), 64 MB memory and a 1 MB stack. The
  prelude (`prelude.ts`) is the Flash-like object model. It is sugar over
  `__host(json)`: properties, timelines, names and collisions are all
  answered by the core.
* **Pen**: `penDown` / `penDrag` / `penUp` / `penHover` feed the core's pen
  state machine. `penPreviewJson` returns what to draw, and `penFinish`
  creates the path. **Pencil**: the UI collects raw pointer samples and
  `createFreehand` fits them.
* **Vector queries**: `pathInfoJson` (anchors/handles/outline in stage
  coordinates), `pathHitJson`, `gradientJson`, `pickStyle` (eyedropper).
  Previews such as `shapePreview` return stage-space polylines, so the UI never
  evaluates curves itself.
* **Rendering**: `render(ctx, frame, scale, offsetX, offsetY, clip,
  showGuides)` draws via Canvas2D. `decodeImages()` resolves once newly
  embedded images are decoded (the caller then redraws). Undecodable images
  render as placeholders.
* The frontend keeps **no model copy**. After any command it bumps a version
  counter and re-queries. It never computes model geometry, transforms,
  snapping or hit-testing. It only maps screen ↔ stage coordinates (zoom/pan)
  and draws chrome from geometry the core returns.

## Boundary: frontend ↔ native shell (Tauri)

| Command | Args | Returns |
|---------|------|---------|
| `save_project` | `contents: string, path: string \| null` | written path, or `null` if the dialog was cancelled |
| `open_project` | — | `{ path, contents }`, or `null` if cancelled |
| `export_begin` | `single: bool, name: string` | asks for the destination (a `.html` file, or a folder for folder exports) and remembers it; the path, or `null` if cancelled |
| `export_write` | raw bytes, header `x-file-name` (percent-encoded) | writes one export file. Single-file exports go to the chosen path. Otherwise the file goes into the chosen folder, and only plain names are accepted (no separators, no leading dot). |
| `pick_files` | `kind: "image" \| "font" \| "audio"` | `[{ path, name }]` from a native multi-select dialog filtered by kind (`[]` if cancelled) |
| `read_picked_file` | `path: string` | raw bytes (`ArrayBuffer`), **only** for a path just returned by `pick_files`, readable once |

The shell treats project contents as opaque strings and never parses them.
Dialogs open from Rust, so JS is granted no dialog/fs permissions
(`capabilities/default.json` = `core:default`). The picked-path allowlist means
the webview can never read an arbitrary file. Saves are atomic (write a temp
file, then rename). The window sets `dragDropEnabled: false` so HTML5
drag-and-drop works in the webview (layer reordering, dropping images onto the
stage). `editor/src/platform.ts` provides browser fallbacks
(download/upload), so the editor also runs in a plain browser.

## WASM build step

```
cargo build -p zoetrope-web --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir editor/src/wasm/pkg target/wasm32-unknown-unknown/release/zoetrope_web.wasm
```

This is wrapped in `scripts/build-wasm.sh` (`npm run wasm`), which `tauri dev`
and `tauri build` run automatically. The `wasm-bindgen` crate is pinned
(`=0.2.126`) and must match the CLI version. The module was ~700 KB unoptimized
(~230 KB gzipped) in Phase 2, ~1 MB after Phase 3, and 2.37 MB after Phase 6
(rustybuzz plus the ~97 KB bundled font). A size pass
(`wasm-opt`, `opt-level = "s"`) is planned for the export phase.

`npm run wasm` also builds the **player bundle** (`npm run player`, using
`vite.player.config.ts`) into `editor/player-dist/player.js`, which is
gitignored. Both WebAssembly modules are embedded gzipped, about 1.4 MB in
total. The editor imports it lazily (`?raw`) only when exporting. See
EXPORT.md for formats, the page contract and the parity checks.

**Size pass (Phase 8).** The release profile uses `lto = true` and
`codegen-units = 1`. The release build drops the 0.5 MB `name` section
(`--remove-name-section`), and `wasm-opt -O3` runs if Binaryen is installed.
Measured on the demo:

| Build | Size | gzip | Render cost |
|-------|------|------|-------------|
| Phase 7 | 2.51 MB | 848 KB | 0.021 ms/frame |
| lto + codegen-units=1, names stripped (shipped) | 1.98 MB | 713 KB | 0.021 ms/frame |

`opt-level = "s"` would have saved another 2.5% at 25% slower rendering, and
`"z"` 4.5% at three times slower, so `opt-level` stays at 3. The traversal
benchmark is `Engine.benchTraversal(frames)`. The engine grew to 2.05 MB
with the Phase 8 APIs. Exports carry it gzipped.
