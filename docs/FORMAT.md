# Project file format (`.zoe`) — schema version 2

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

**Symbol**: `id`, `name`, `layers: Layer[]`.

### Layer

Layer lists (`Symbol.layers`, `Layer.children`) are **bottom-to-top**
(index 0 is drawn first).

| Field | Type | Notes |
|-------|------|-------|
| `id`, `name` | | |
| `kind` | `"normal"` \| `"guide"` \| `"folder"` | *optional*, default `normal`. Guide layers render in the editor only, never in the player/export. |
| `visible` | bool | *optional*, default `true`. For folders it cascades to children. |
| `locked` | bool | *optional*, default `false`. Cascades. Locked content can't be selected or drawn into. |
| `elements` | Element[] | normal/guide only. **Back-to-front**. *Optional*, default `[]`. |
| `children` | Layer[] | folders only. *Optional*, default `[]`. |

### Element

| Field | Type | Notes |
|-------|------|-------|
| `id` | u32 | |
| `name` | string | *optional*, default `""` |
| `transform` | Transform | *optional*, default identity |
| `opacity` | number 0..1 | *optional*, default `1` (multiplies alpha) |
| `blend` | BlendMode | *optional*, default `normal` |
| `tint` | `{ "color": Color, "amount": 0..1 }` | *optional*, default none |
| `type` | `"shape"` \| `"instance"` \| `"bitmap"` | discriminator, see below |

* `"type": "shape"`: `geometry`, optional `fill` (Paint), optional `stroke`
  (Stroke), optional `fillRule` (`"nonZero"` default \| `"evenOdd"`).
* `"type": "instance"`: `symbol` (id). Instance references must be acyclic.
* `"type": "bitmap"`: `asset` (id of an image asset), drawn at the image's pixel size, centered on the content origin.

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
| `type` | `"image"` | |
| `mime` | string | `image/png`, `image/jpeg` or `image/gif` |
| `width`, `height` | u32 | Pixel size, read from the file header by the core on import |
| `data` | string | Standard base64 (RFC 4648, padded) of the original file bytes |

## Validation on load

The root symbol exists; stage values are in range; ids are unique and
`< nextId`; folders hold no elements and only folders hold child layers;
element values are in range (opacity, tint amount, finite transforms,
non-negative sizes and stroke widths, valid paints/dashes, non-empty finite
paths); every instance and bitmap reference resolves; no symbol contains
itself, directly or transitively.

Floats round-trip exactly (`serde_json` with `float_roundtrip`), so save →
load → save is byte-stable.

## Planned changes

Timelines and keyframes (Phase 4) change the layer structure. That will be
schema v3, with a migration that wraps each layer's `elements` into a single
keyframe at frame 0.
