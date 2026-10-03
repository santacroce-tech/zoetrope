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
         │ invoke(save_project | open_project | pick_images | read_picked_file)
         ▼
  editor/src-tauri  (Rust native shell: dialogs + filesystem only)
```

The exported player (Phase 8) will load the **same** `zoetrope_web_bg.wasm`
with a minimal JS bootstrap instead of the React editor.

## crates/zoetrope-core

| Module     | Responsibility |
|------------|----------------|
| `model`    | `Project → Symbol → Layer tree → Element`. Elements are `Shape`, `Bitmap` or `Instance` (of another symbol), so the evaluated scene is a **tree of instances**. Also covers `Transform` (with pivot) and its matrix decomposition, appearance (opacity/blend/tint), and validation. |
| `edit`     | `Edit`: primitive mutations (transforms, element insert/remove/replace/reorder, layer insert/remove/props, assets, stage). `Edit::apply` validates and returns the exact inverse. |
| `history`  | `Document` = project + undo/redo. `execute(label, edits)` is atomic (it rolls back on failure). Dirty tracking uses state ids. |
| `ops`      | High-level commands that build `Edit`s: move, duplicate, delete, patch properties, resize, create shapes, import images, align/distribute, arrange, layer add/delete/move/props, stage. The UI only calls these. |
| `query`    | Tight bounds, hit-testing (fill containment, stroke distance, recursion into instances; hidden and locked layers are skipped), marquee, selection handle geometry. |
| `interact` | Direct-manipulation math: `TransformSession` (move/scale/rotate/skew/pivot drags with modifiers), `EditSession` (anchor/handle/gradient drags), `PenSession` (the pen tool's state machine), snapping (grid, objects, stage), shape-tool drags (incl. polygon/star). |
| `vector`   | Editable `VectorPath` (subpaths of anchors with bezier handles): primitive→path conversion, split/insert, delete, convert corner⇄smooth, handle constraints, nearest-point, freehand fitting (RDP simplification + Catmull-Rom smoothing), polystar. |
| `paint`    | `Paint` (solid, linear, radial with focal point), `PaintStyle` (geometry-free tool form, fitted to shapes), `Stroke` (caps, joins, miter, dashes), `FillRule`. |
| `geom`     | `Path` (move/line/quad/cubic/close), deterministic flattening, containment (non-zero / even-odd), outline distance, `Rect`. |
| `render`   | `Renderer` trait, `render_frame` tree walk, `RecordingRenderer` (tests/determinism), `NullRenderer` (profiling). |
| `asset`    | Embedded assets: base64 (de)serialization and PNG/JPEG/GIF header sniffing, so the core is the authority on image size. |
| `format`   | Versioned JSON envelope, migration chain, validation. See FORMAT.md. |
| `outline`  | Panel views: the layer tree and per-element info. |
| `demo`     | The built-in demo scene. |

### Scopes

Selection, hit-testing and transform sessions work within a **scope**: the
symbol whose elements are editable, plus its symbol→stage matrix. Today the
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

Every mutation is an `Edit`, and every transaction is undoable, including
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
| `pick_images` | — | `[{ path, name }]` from a native multi-select dialog (`[]` if cancelled) |
| `read_picked_file` | `path: string` | raw bytes (`ArrayBuffer`), **only** for a path just returned by `pick_images`, readable once |

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
(~230 KB gzipped) in Phase 2 and ~1 MB after Phase 3. A size pass
(`wasm-opt`, `opt-level = "s"`) is planned for the export phase.
