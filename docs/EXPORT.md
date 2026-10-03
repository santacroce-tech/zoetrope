# Export and the player

**Export…** (toolbar) publishes the project as a web page. The page plays it
with the **same WASM core and the same runtime module** (`editor/src/runtime/`)
as the editor's ▶ preview. Nothing is re-implemented for export, so a
project plays identically in both, frame for frame. This is verified as
described below.

## Settings

The settings are saved with the project as `Project.publish` (FORMAT.md), and
changing them is one undo step.

| Setting | Values |
|---------|--------|
| Title | Page title and file name; empty means the project's file name. |
| Format | **Single HTML file** (default) or **Folder** |
| Scaling | **Letterbox** (default): the whole stage as large as fits, with bars in the page color. **Fill**: covers the window, keeps proportions, crops the overflow. **Fixed**: stage pixels 1:1, centered, with the top-left corner kept visible in small windows. |
| Page color | Color of the bars and margins around the stage |
| Start on click | Shows the first frame with a ▶ button and plays on click. Browsers only allow sound after a user gesture, so this is how a movie can start with its sound. Without it, the movie plays on load and sound starts at the first click or key press. |

The dialog also shows an **embed snippet**: an `<iframe>` sized to the stage
that keeps its aspect ratio when narrowed.

## Formats

**Single HTML file**: `Title.html`, one file that plays offline. It works
even when opened straight from disk (`file://`) and makes no network
requests. It contains:

- the player script, inline;
- the project as a gzipped [pack](FORMAT.md#pack), base64-encoded in
  `<script type="application/octet-stream" id="zoetrope-pack">`.

The demos export to about 1.4–1.6 MB, of which about 1.4 MB is the player.

**Folder**: for big projects, or several movies on one site.

- `Title.html` is a small page.
- `zoetrope-player.js` is the player, identical for every export, so browsers
  cache it.
- `Title.zoepack` is the gzipped pack. It holds assets as raw bytes, about 25%
  smaller than base64.

The page fetches the pack, so the folder must be served over http(s).
Browsers block fetches from pages opened from disk; the page says so
explicitly instead of failing silently.

## The player

`editor/src/player/main.ts`, built by `npm run player`
(`vite.player.config.ts`, part of `npm run wasm`) into one IIFE script.

- **Startup:**
  - The core and QuickJS WebAssembly modules are embedded **gzipped**
    (base64), and the pack is gzipped too. All of them are inflated with the
    browser's `DecompressionStream`.
  - Inlining the modules raw made the Phase 7 export 4.2 MB.
- **What it does:**
  - Lays out the canvas for the scale mode, at device-pixel resolution.
  - Draws `playRender` output on demand.
  - Sends the pointer, in stage coordinates, and the keyboard to the
    `Runtime`.
  - Shows a readable error if the page can't start.
- **Browser support:** current Chrome, Edge, Firefox and Safari (16.4+), which
  have `DecompressionStream` and WebAssembly.

**Page contract** (written by `editor/src/export.ts`): `<canvas
id="zoetrope-stage">`, and `<script type="application/json"
id="zoetrope-config">` holding `{ scale, startOnClick, pack? }`. The project
comes either from the inline `#zoetrope-pack` element or from `config.pack`,
a URL relative to the page.

## Parity: preview and export play identically

Both run `Runtime` from `editor/src/runtime/` on the same core. What could
differ, and how each is checked:

1. **Project transport** (save to JSON or to a pack, load in another engine):
   the core test `exported_projects_play_identically` plays a 60-tick session
   with input and scripted property changes. It compares
   `Player::digest` at every tick for the original, a JSON round trip and a
   pack round trip. The digest fingerprints the exact draw calls plus the
   stream sounds.
2. **Page assembly, the player and the sandbox**: both sides expose a test
   hook. The exported page does when its URL has `?zoetrope-test`; the
   editor's dev build does when `window.zoetropeTestMode = true` is set
   before pressing ▶. The hook steps the session by hand with a fixed
   `Math.random` seed (`window.zoetropeTest.advance / key / pointer /
   digest`). The same input script was run in both, comparing digests per
   frame:
   - The animation demo: 120 frames with ▶ pause and resume through its
     script, keys and clicks. All 120 match.
   - The game demo: 520 frames with arrow keys, seeded flower positions, the
     game-over frame and a restart click. All 520 match.
   - The folder export served over http gives the same digests.

Without `?zoetrope-test` the hook doesn't exist. It only lets the page's
own console step its own movie.

## Not yet

- Fullscreen button, pause on hidden tab, preloader progress for very large
  packs.
- A video or GIF export (Phase 9 candidates).
