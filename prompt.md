Build a complete, production-quality Flash-like animation IDE (native desktop)
and web exporter/runtime. This is a large project; execute it in PHASES, each
of which must build, run, and be independently demoable. Stop at each PHASE
GATE and wait for my go-ahead.

VISION
A native desktop authoring tool that recreates the core magic of Macromedia/
Adobe Flash for the modern web: draw vector art, build reusable animated
symbols, script interactivity, and export self-contained content that plays in
any browser with no install, no plugin, and no server. The exported runtime and
the editor's live preview share ONE Rust/WASM core so they never diverge.

ARCHITECTURE (three parts, strict boundaries)
1. CORE crate (Rust → WASM): the single source of truth. Owns:
   - the scene graph (a TREE of symbol instances, each potentially with its
     own timeline — not a flat list),
   - timeline evaluation and tween/easing math,
   - the vector geometry model (paths, fills, strokes) and its rasterization,
   - the rendering pipeline (behind a Renderer trait so the backend can be
     Canvas2D now and WebGL/wgpu later WITHOUT touching scene logic),
   - the scripting runtime host (see SCRIPTING).
   Compiled ONCE to WASM and used in TWO places: the editor preview AND the
   exported player. This guarantees byte-identical rendering — true WYSIWYG.
2. TAURI NATIVE SHELL (Rust): native file dialogs, asset import from disk
   (PNG/JPEG/SVG/audio), project save/load, and writing export bundles. The
   only reason this is a desktop app. Does I/O and OS integration only.
3. EDITOR FRONTEND (Tauri webview, TypeScript + React): all authoring UI —
   tools, panels, timeline, stage interactions. Loads the CORE as WASM for all
   rendering and evaluation; calls the Tauri backend for all filesystem ops.
   NEVER re-implements rendering, geometry, or tween math — always asks the core.

CROSS-CUTTING REQUIREMENTS (apply to every phase from the start)
- Undo/redo: a command/transaction system in the core. Every edit is a
  reversible command. Build this in Phase 1, not bolted on later.
- Serialization: a versioned project format (JSON for structure; binary/base64
  for embedded assets). Include a schema version field and a migration hook.
- Determinism: given a project + frame, rendering is reproducible.
- Performance budget: target smooth playback of scenes with many nested
  animated symbols. Keep the Renderer trait clean so a faster backend can drop
  in. Profile at the end of each phase.
- Testing: unit tests for the core (tween math, timeline eval, geometry,
  serialization round-trips). The core must be testable headless, without the
  UI.

=== PHASE 1 — FOUNDATION & TOOLCHAIN ===
- Tauri app scaffold + CORE crate building to WASM, rendering a HARDCODED scene
  in the webview. Prove cargo + wasm build + Tauri + webview end to end.
- Establish the scene graph as a TREE from day one (even if only depth 1 is
  used yet), the versioned serialization format, and the undo/redo command
  system.
- Renderer trait + a Canvas2D implementation.
GATE: hardcoded nested scene renders; undo/redo works on a trivial edit;
project round-trips through save/load JSON.

=== PHASE 2 — STAGE, OBJECTS, LAYERS, PROPERTIES ===
- Stage with configurable size/background.
- Primitive objects (rectangle, ellipse, line) + imported raster images.
- Full transform model: x, y, scaleX, scaleY, rotation, skew, anchor/pivot
  point, opacity, blend mode, color tint.
- Layers: reorder, show/hide, lock, folders, guide layers.
- Selection, multi-select, drag, transform handles (scale/rotate/skew), numeric
  property panel, alignment/distribution, snapping, grid, rulers.
GATE: author a static multi-layer scene with transformed objects; everything
round-trips and undoes.

=== PHASE 3 — VECTOR DRAWING ===
- Path model (bezier/quadratic segments), fills (solid, linear/radial gradient),
  strokes (width, caps, joins, dashes).
- Tools: pen (bezier), pencil/freehand, shape tools, line, eyedropper, paint
  bucket, selection/subselection (edit anchor points and handles).
- Boolean/shape operations if feasible (union/subtract) — optional sub-goal.
GATE: draw and edit vector paths with gradients and strokes; render matches in
preview and (later) export.

=== PHASE 4 — TIMELINE, KEYFRAMES, TWEENING ===
- Multi-layer timeline UI: frames, keyframes, blank keyframes, frame spans.
- Classic/motion tweens: interpolate transform + color + opacity.
- Shape tweens: interpolate vector geometry between keyframes (best-effort).
- Easing: a curve editor with presets (linear, ease in/out, cubic, back,
  bounce, elastic) and custom bezier easing.
- Playback engine: play/pause/stop, loop, scrub, fps control, onion skinning.
GATE: build a tweened animation with custom easing and onion skin; scrubbing is
smooth.

=== PHASE 5 — SYMBOLS & NESTED TIMELINES (the big one) ===
- Symbol library: convert selections into reusable symbols; symbol types
  (graphic, movie clip, button).
- Instances: multiple instances of a symbol, each independently transformable.
- NESTED TIMELINES: movie-clip symbols run their own timeline; the scene graph
  is a true tree of clocks. Define and document the timing/propagation rules.
- Symbol editing mode (edit-in-place), breadcrumb navigation, library panel
  with reuse, rename, duplicate, swap-instance.
- Button symbols with up/over/down/hit states.
GATE: a movie clip containing its own animation, instanced several times, each
playing correctly and independently; a working button symbol.

=== PHASE 6 — TEXT & AUDIO ===
- Text tool: static text with font/size/color/alignment/spacing; embed fonts
  into the export so it renders without the font installed.
- Audio: import clips, place on timeline (event + streaming sync), basic
  volume; audio plays in preview and export.
GATE: text with an embedded font and a synced audio clip both play in preview.

=== PHASE 7 — SCRIPTING / INTERACTIVITY ===
DECISION REQUIRED FROM ME before building: scripting language.
Default proposal: embed a sandboxed JavaScript/TypeScript runtime (a
WASM-friendly JS engine) with a Flash-like API surface (timeline control:
play/stop/gotoAndPlay; instance refs; input/mouse/keyboard events; simple
collision helpers). ActionScript source compatibility is an EXPLICIT NON-GOAL.
- Script attachment: per-frame scripts and per-symbol scripts.
- Event model: enterFrame, mouse/keyboard, button events.
- Sandboxing: scripts cannot touch the host filesystem or network unless
  explicitly allowed.
GATE: a scripted interactive demo (e.g., a tiny playable game: move a symbol
with arrow keys, click a button to restart) runs in preview AND export.

=== PHASE 8 — EXPORT, RUNTIME & PUBLISH ===
- Export a single self-contained offline HTML bundle embedding the SAME WASM
  core as the runtime, the project data, and all assets (images/fonts/audio as
  base64 or packed). Plays on load, offline, no install.
- Also support an "assets folder" export mode for large projects (HTML + a
  loadable packed asset file) for when inlining everything is too big.
- Responsive/scaling options (fixed, fill, letterbox) and HTML embed snippet.
- Verify exported output renders and behaves IDENTICALLY to the editor preview,
  including nested timelines, easing, text, audio, and scripts.
GATE: a complete project (vectors + symbols + tweens + text + audio + script)
exports to a standalone HTML file that matches the preview exactly.

=== PHASE 9 — POLISH & PRODUCTION READINESS ===
- Robust undo/redo across all operations; autosave + crash recovery.
- Keyboard shortcuts, preferences, recent files.
- Performance pass: profile heavy scenes; if Canvas2D is the bottleneck,
  implement the WebGL/wgpu Renderer behind the existing trait.
- Error handling, project-format migration, and a test suite covering
  serialization round-trips and timeline/scripting behavior.
- Documentation: data format spec, scripting API reference, architecture notes
  (core↔frontend and frontend↔Tauri boundaries, the WASM build step).
GATE: production-quality build with docs and tests.

DELIVERY RULES (every phase)
State the plan briefly, implement, give exact build/run commands (including the
WASM build and `tauri dev`), provide the manual test(s) for the gate, run the
automated tests, and STOP for my go-ahead. Minimal, justified dependencies.
Document any boundary or format you introduce. Prefer clarity over cleverness.
Flag any decision that needs my input rather than guessing.
