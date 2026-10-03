# Project file format (`.zoe`) — schema version 5

UTF-8 JSON. Field names are camelCase. Unknown fields are ignored on read.
Fields marked *optional* may be omitted and take the listed default; the
writer omits several of them when they hold their default.

History:

* **v1** (Phases 1–2). Phase 2's additions (pivot, opacity, blend, tint,
  layer kinds/folders, bitmaps, assets) were all optional with defaults, so
  they did not need a version bump.
* **v2** (Phase 3). Strokes changed shape: `{ width, color }` became
  `{ width, paint, cap, join, miterLimit, dash, dashOffset }`. The `v1_to_v2`
  migration rewrites every v1 stroke to a solid paint with `cap: "butt"`,
  `join: "miter"` and `miterLimit: 10`, which are exactly the Canvas2D
  defaults v1 rendered with, so old files look identical (covered by the
  `phase1_files_still_load` test).
* **v3** (Phase 4). Content layers hold `keyframes` instead of `elements`.
  The `v2_to_v3` migration wraps each layer's elements into a single
  one-frame keyframe, so v2 scenes are unchanged (their only frame). Elements
  gain an optional `track`.
* **v4** (Phase 6). Adds text elements, font and audio assets, and keyframe
  sounds. These are all additions, so `v3_to_v4` changes nothing but the
  version. The bump exists so that a v3 reader rejects v4 files with a clear
  "newer version" error rather than an "unknown variant" error.
* **v5** (Phase 7). Adds frame labels, frame scripts and symbol scripts. These
  are optional fields, so `v4_to_v5` changes only the version.

## Envelope

```json
{
  "format": "zoetrope-project",
  "schemaVersion": 1,
  "project": { ... }
}
```

* `format` must equal `"zoetrope-project"`.
* `schemaVersion` is a positive integer. Files with a version newer than the
  reader supports are **rejected** (never guessed at).

## Versioning & migration

Loading parses the file to untyped JSON, then runs `MIGRATIONS[v-1]` for each
version `v` from the file's version up to the current one
(`crates/zoetrope-core/src/format.rs`). Each step is `fn(Value) -> Result<Value>`
upgrading one version; the runner rewrites `schemaVersion` between steps.
Only then is the result deserialized and validated.

To make a breaking change: bump `SCHEMA_VERSION`, append a migration, and add a
test that loads a fixture of the old version. Purely additive optional fields
(with defaults) do not need a version bump.

## Project

| Field | Type | Notes |
|-------|------|-------|
| `nextId` | u32 | Next unallocated id. All ids (symbols, layers, elements, assets) share one space and are unique. Every id must be `< nextId`. |
| `stage` | Stage | |
| `root` | id | Symbol used as the main timeline. |
| `symbols` | Symbol[] | Symbol definitions. |
| `assets` | Asset[] | *optional*, default `[]`. Embedded binary assets. |

**Stage**: `width`, `height` (stage px, in (0, 16384]), `background` (Color), `fps` (in (0, 240]).

**Symbol**: `id`, `name`, `kind` (*optional*, default `"graphic"`: one of
`"graphic"`, `"movieClip"`, `"button"`; see *Symbol timing*), `layers: Layer[]`,
`script` (*optional*: JavaScript run for every instance as it appears; see
docs/SCRIPTING.md).
The root symbol is the main timeline; its kind is irrelevant (the demo marks
it `movieClip`).

### Layer

Layer lists (`Symbol.layers`, `Layer.children`) are **bottom-to-top**
(index 0 is drawn first).

| Field | Type | Notes |
|-------|------|-------|
| `id`, `name` | | |
| `kind` | `"normal"` \| `"guide"` \| `"folder"` | *optional*, default `normal`. Guide layers render in the editor only, never in the player/export. |
| `visible` | bool | *optional*, default `true`. For folders it cascades to children. |
| `locked` | bool | *optional*, default `false`. Cascades. Locked content can't be selected or drawn into. |
| `keyframes` | Keyframe[] | normal/guide layers only, at least one. See below. |
| `children` | Layer[] | folders only. *Optional*, default `[]`. |

### Keyframe & tween

A content layer's keyframes are **contiguous spans starting at frame 0**:
keyframe *i* starts at the sum of the earlier durations (starts are never
stored, so they can't disagree). Frames past the last span show nothing on
that layer. A symbol's length is its longest layer (≥ 1).

| Keyframe field | Notes |
|----------------|-------|
| `duration` | frames spanned, ≥ 1 |
| `elements` | Element[], **back-to-front**. *Optional*, default `[]` (a blank keyframe). |
| `tween` | *optional* Tween: interpolate toward the **next** keyframe over this span |
| `sound` | *optional* SoundRef: a sound attached to this keyframe (see "Sound timing") |
| `label` | *optional* frame label (non-blank); scripts jump to it with `gotoAndPlay("label")`. With duplicates, the earliest frame wins. |
| `script` | *optional* frame script (JavaScript), run when the playhead enters this keyframe (docs/SCRIPTING.md) |

| SoundRef field | Notes |
|----------------|-------|
| `asset` | id of an **audio** asset |
| `sync` | *optional*: `"event"` (default) or `"stream"` |
| `volume` | *optional* 0..1, default 1 |
| `loops` | *optional* extra repeats, default 0 |

| Tween field | Notes |
|-------------|-------|
| `kind` | `"motion"`: transform, pivot, opacity and tint. `"shape"`: those, plus shape geometry, fill/stroke paints and stroke width/dashes. |
| `easing` | *optional* (default linear): `{"type":"linear"}` \| `{"type":"preset","name":<preset>}` \| `{"type":"bezier","x1","y1","x2","y2"}` (CSS `cubic-bezier`; x1/x2 in 0..1) |
| `rotate` | *optional* extra full turns added to the rotation (positive = clockwise), default 0 |

Presets: `easeIn|Out|InOut` × `Quad`, `Cubic`, `Sine`, `Back`, `Bounce`,
`Elastic` (Penner's equations), e.g. `easeInOutSine`.

**Evaluation at frame *f*** (span starting at *s*, duration *d*, eased
progress *e* = easing((*f* − *s*) / *d*)): each element of the tweened
keyframe is paired with the element of the next keyframe that has the same
**track**, and interpolated at *e*. At *f* = *s* + *d* the next keyframe
itself is shown. Unpaired elements are shown unchanged. Rotation goes
`a + (b − a + 360·rotate)·e`. Shape tweens morph geometry:
- **Same primitive kind:** the parameters are interpolated.
- **Otherwise:** both shapes are converted to paths and node counts are
  equalized by splitting the longest segments. Closed subpaths are rotated
  to the best-matching start node. If the subpath structure doesn't match,
  the shape switches at *e* = 0.5.
- **Paints:** interpolated per stop when the stop counts match; a solid
  tweens against a gradient as a uniform gradient.

### Sound timing

The core decides what should be heard and when. The platform (WebAudio in
the editor and player) only plays it.

* **Stream**: the sound is locked to the timeline of the keyframe's span. At
  frame *f* of a span starting at *s*, the clip should be at
  (*f* − *s*) / fps seconds. It plays only while the span is showing. Seeking,
  starting mid-span and looping all re-position it. The player restarts a
  stream whose audio drifts more than 0.12 s from that position.
* **Event**: the sound starts from the beginning when its keyframe is
  entered, including when playback starts on that keyframe. It then plays to
  the end (repeated `loops` extra times) whatever the timeline does next.
* Sounds inside movie clips follow that clip's own clock. Sounds on hidden
  and guide layers are silent.

### Symbol timing (nested timelines)

The display tree is a **tree of clocks**. An instance is identified by its
*instance path*: the `(layer id, element track)` at each level from the main
timeline down. What frame an instance of a symbol of length *L* shows:

| Kind | Rule |
|------|------|
| Graphic | Synced to its parent: `n = firstFrame + (parentFrame − start of the parent keyframe holding it)`; `loop` → `n mod L`, `playOnce` → `min(n, L−1)`, `singleFrame` → `min(firstFrame, L−1)`. |
| Movie clip | Its own clock: frame 0 on the tick it **appears**, then +1 per tick, looping (`mod L`). It "appears" when its track starts an unbroken run of keyframes (same layer, same track, same symbol) containing it; it keeps its clock across those keyframes and restarts if it disappears and comes back. |
| Button | Frame 0 Up, 1 Over (pointer over the hit area), 2 Down (pressed on it), 3 Hit (the clickable area; never drawn; if missing, the Up art is the hit area). |

The **runtime** (`player::Player`, used by preview playback, the exported
player and scripts) holds the clocks. Each tick advances the main timeline and
every on-stage movie clip by one frame, looping, *while they are playing*
(scripts can `stop()`, `play()` and `goto`). A movie clip still on stage when
the main timeline loops keeps running. A timeline **enters** a frame when it
advances to it, when a script jumps to a different frame, or when its
instance appears. Entering queues that frame's scripts and fires its event
sounds; a stopped timeline never re-enters its frame.

Scripts can take over an instance's `x`, `y`, `rotation`, `scaleX`, `scaleY`,
`alpha`, `visible` (and a text's `text`). These **overrides** belong to the
instance path and replace the animated values until the instance leaves the
stage. They exist only at runtime and are never saved.

The **editor** (scrubbing, stage editing, single-frame rendering) uses the
stateless form of the same rules: a movie clip shows `(parentFrame −
appearedFrame) mod L`, i.e. "as if played straight through from frame 0".
The two agree frame-for-frame until the main timeline first loops (asserted
by a test).

### Element

| Field | Type | Notes |
|-------|------|-------|
| `id` | u32 | |
| `track` | u32 | *optional*: tween pairing identity, default = `id`. Copies made by "insert keyframe" keep their source's track. |
| `name` | string | *optional*, default `""` |
| `transform` | Transform | *optional*, default identity |
| `opacity` | number 0..1 | *optional*, default `1` (multiplies alpha) |
| `blend` | BlendMode | *optional*, default `normal` |
| `tint` | `{ "color": Color, "amount": 0..1 }` | *optional*, default none |
| `type` | `"shape"` \| `"instance"` \| `"bitmap"` \| `"text"` | discriminator, see below |

* `"type": "shape"`: `geometry`, optional `fill` (Paint), optional `stroke`
  (Stroke), optional `fillRule` (`"nonZero"` default \| `"evenOdd"`).
* `"type": "instance"`: `symbol` (id), plus for graphic symbols *optional*
  `firstFrame` (default 0) and `loopMode` (`"loop"` default \| `"playOnce"` \|
  `"singleFrame"`). Instance references must be acyclic. The element `name`
  is the instance name (button events report it; scripts will address it).
* `"type": "bitmap"`: `asset` (id of an image asset), drawn at the image's pixel size, centered on the content origin.
* `"type": "text"`: static text, laid out by the core from an embedded font
  (see below).

| Text field | Notes |
|------------|-------|
| `text` | string; `\n` starts a new line |
| `font` | id of a **font** asset |
| `size` | em size in content units, > 0 |
| `fill` | Paint. Gradients are in content coordinates; the box is `(0,0)–(width,height)`. |
| `align` | *optional*: `"left"` (default), `"center"` or `"right"` |
| `letterSpacing` | *optional*: extra advance after each glyph, default 0 |
| `lineHeight` | *optional*: baseline-to-baseline distance as a multiple of `size`, default 1.25 |
| `width` | *optional*: wrap width. When absent, lines break only at `\n` and the box is as wide as the longest line. |

**Text layout** (`crates/zoetrope-core/src/text.rs`):
* Each paragraph is shaped with rustybuzz (a HarfBuzz port), which gives
  kerning, ligatures and complex scripts.
* Lines wrap greedily at whitespace. A word wider than the box overflows it.
* The first baseline sits at the font's ascender. The box height is ascender
  − descender + (lines − 1) × lineHeight × size, and an empty text still
  has a box one line high.
* Glyph outlines become paths that are filled with the non-zero rule.
  Renderers never see fonts, so text looks identical everywhere and needs
  no installed fonts.
* The whole box is clickable.

**BlendMode**: `normal`, `layer`, `multiply`, `screen`, `overlay`, `darken`,
`lighten`, `hardLight`, `difference`, `add`. Any non-`normal` mode renders the
element as an isolated group composited with its opacity; `layer` composites
normally, so opacity applies to the group as a whole instead of to each
primitive.

**Color transforms** (Flash semantics): an element's tint is applied, then its
opacity, and the result composes with every ancestor instance's transform
(child first). They apply per drawn primitive, unless a blend group isolates
them.

### Transform

All fields *optional*: `x`, `y` (0), `scaleX`, `scaleY` (1), `rotation`,
`skewX`, `skewY` (degrees, 0), `pivotX`, `pivotY` (0).

```
matrix = translate(x, y) · rotate(rotation) · skew(skewX, skewY) · scale(scaleX, scaleY) · translate(-pivotX, -pivotY)
```

The pivot (anchor point) is given in content coordinates. It is placed at
`(x, y)`, and rotation/scale/skew act around it. Y points down, so positive
rotation is clockwise. `skew(kx, ky)` maps `(x, y) → (x + tan(kx)·y, tan(ky)·x + y)`.

### Geometry

Primitives are centered on the content origin:
`{"kind":"rect","width","height"}` | `{"kind":"ellipse","width","height"}` |
`{"kind":"line","dx","dy"}` (from `(-dx/2,-dy/2)` to `(dx/2,dy/2)`; never filled).

**Paths** are `{"kind":"path","subpaths":[SubPath…]}` in content coordinates:

| SubPath field | Notes |
|---------------|-------|
| `nodes` | Node[] (at least one) |
| `closed` | bool, *optional*, default `false` |

| Node field | Notes |
|------------|-------|
| `x`, `y` | anchor point |
| `in`, `out` | *optional* `{x, y}`: absolute positions of the incoming/outgoing bezier handles |
| `kind` | `"corner"` (default) \| `"smooth"` (handles collinear) \| `"symmetric"` (collinear and equal length). An editing constraint only; it doesn't affect rendering. |

Segment `a → b` is a cubic bezier with control points `a.out ?? a` and
`b.in ?? b`, or a straight line when both are absent. A closed subpath adds
the segment last → first. Quadratic curves are stored as their exact cubic
equivalent.

### Paint

| `type` | Fields |
|--------|--------|
| `"solid"` | `color` |
| `"linear"` | `start`, `end` (`{x, y}`, content coords), `stops` |
| `"radial"` | `center`, `radius` (> 0), *optional* `focal` (default `center`, must lie inside the circle), `stops` |

**Stops**: `[{ "offset": 0..1, "color": Color }, …]`, non-empty, in ascending
offset order. Gradients pad beyond their ends. Their geometry is in the
shape's content coordinates, so it moves, rotates and scales with the shape.

### Stroke

| Field | Notes |
|-------|-------|
| `width` | ≥ 0, content units (scales with the element) |
| `paint` | Paint |
| `cap` | `"butt"` \| `"round"` (default) \| `"square"` |
| `join` | `"miter"` \| `"round"` (default) \| `"bevel"` |
| `miterLimit` | ≥ 1, default 4 |
| `dash` | *optional* dash/gap lengths (content units); omitted/empty = solid |
| `dashOffset` | *optional*, default 0 |

**Color**: string `"#rrggbb"` (opaque) or `"#rrggbbaa"`; `"#rgb"` is accepted on read.

### Asset

| Field | Type | Notes |
|-------|------|-------|
| `id`, `name` | | |
| `type` | `"image"` \| `"font"` \| `"audio"` | |
| `data` | string | Standard base64 (RFC 4648, padded) of the original file bytes |

Each type has its own fields:

* **image**: `mime` (`image/png`, `image/jpeg` or `image/gif`) and `width`,
  `height` (u32 pixel size, read from the file header by the core on import).
* **font**: `family` (read from the font's name table on import). The data is
  a TrueType/OpenType file (`.ttf`/`.otf`, first face of a collection) and
  must parse on load. Fonts are embedded on import; new text with no font
  chosen embeds the bundled **Zoetrope Sans** once (a Latin subset of Lato
  2.015, SIL OFL 1.1, renamed per the Reserved Font Name clause, see
  `crates/zoetrope-core/assets/fonts/`).
* **audio**: `mime` (sniffed by the core: `audio/wav`, `audio/mpeg`,
  `audio/ogg`, `audio/mp4`, `audio/flac`) and `duration` (seconds, > 0). The
  duration comes from the platform's decoder at import time, which also
  proves the file is playable. The core does not decode audio.

An asset cannot be removed while a bitmap, text or keyframe sound uses it.

## Validation on load

The root symbol exists; stage values are in range; ids are unique and
`< nextId`; folders hold no elements and only folders hold child layers;
element values are in range (opacity, tint amount, finite transforms,
non-negative sizes and stroke widths, valid paints/dashes, non-empty finite
paths, text sizes/spacing); every instance, bitmap, text-font and sound
reference resolves to an asset of the right type, and fonts parse; no symbol contains
itself, directly or transitively.

Floats round-trip exactly (`serde_json` with `float_roundtrip`), so save →
load → save is byte-stable.

## Planned changes

Export options (Phase 8) may add a player settings block (scaling mode,
autoplay), as optional fields.
