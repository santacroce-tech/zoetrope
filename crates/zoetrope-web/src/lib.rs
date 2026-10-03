//! JS-facing API over `zoetrope-core`.
//!
//! Boundary contract (see docs/ARCHITECTURE.md): structured data crosses as
//! JSON strings, ids as plain numbers (u32), coordinates as stage-space
//! numbers, errors as thrown `Error`s that leave the document unchanged. The
//! frontend holds no model state of its own; after every mutation it
//! re-reads what it displays and re-renders.

mod canvas2d;

pub use canvas2d::{Canvas2dRenderer, CanvasPool};

use serde::de::DeserializeOwned;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, ImageBitmap};
use zoetrope_core::asset::AssetKind;
use zoetrope_core::geom::Rect;
use zoetrope_core::interact::{
    gradient_controls, path_hit, path_info, shape_from_drag, snap_point, DragMode, EditSession, EditTarget, Modifiers, PaintPart, PenSession,
    ShapeOptions, ShapeTool, SnapConfig, SnapTargets, TransformSession,
};
use zoetrope_core::ops::{self, Align, Arrange, Distribute, LayerPatch, ShapeStyle};
use zoetrope_core::query::{self, Scope};
use zoetrope_core::player::Player;
use zoetrope_core::render::{render_editing, render_frame, EditView, Onion, RenderOptions};
use zoetrope_core::{
    demo, format, outline, AssetId, Document, Easing, ElementId, Error, LayerId, LayerKind, Matrix, NodeRef, PaintStyle, Point, Project, Stage,
    SymbolId, SymbolKind, Tween,
};

/// Most frames `playTick` advances in one call.
const MAX_CATCH_UP_TICKS: u32 = 240;

/// One level of symbol editing.
struct EditLevel {
    /// The instance entered (edit in place), or `None` when opened from the library.
    instance: Option<ElementId>,
    symbol: SymbolId,
    /// The outer timeline's playhead when this level was entered.
    parent_frame: u32,
}

/// Library thumbnails: no stage background, just the symbol.
struct PreviewRenderer<'a>(Canvas2dRenderer<'a>);

impl zoetrope_core::render::Renderer for PreviewRenderer<'_> {
    fn begin_frame(&mut self, frame: &zoetrope_core::render::FrameInfo) {
        let transparent = zoetrope_core::render::FrameInfo { background: zoetrope_core::Color::rgba(0, 0, 0, 0), ..frame.clone() };
        self.0.begin_frame(&transparent);
    }
    fn fill_path(&mut self, path: &zoetrope_core::geom::Path, transform: &Matrix, paint: &zoetrope_core::Paint, rule: zoetrope_core::FillRule) {
        self.0.fill_path(path, transform, paint, rule)
    }
    fn stroke_path(&mut self, path: &zoetrope_core::geom::Path, transform: &Matrix, stroke: &zoetrope_core::Stroke) {
        self.0.stroke_path(path, transform, stroke)
    }
    fn draw_image(&mut self, asset: AssetId, width: f64, height: f64, transform: &Matrix, color: &zoetrope_core::ColorTransform) {
        self.0.draw_image(asset, width, height, transform, color)
    }
    fn begin_group(&mut self, blend: zoetrope_core::BlendMode, alpha: f64) {
        self.0.begin_group(blend, alpha)
    }
    fn end_group(&mut self) {
        self.0.end_group()
    }
    fn end_frame(&mut self) {
        self.0.end_frame()
    }
}

#[derive(Default)]
struct ImageCache {
    bitmaps: HashMap<AssetId, ImageBitmap>,
    failed: HashSet<AssetId>,
}

#[wasm_bindgen]
pub struct Engine {
    doc: Document,
    session: Option<TransformSession>,
    edit: Option<EditSession>,
    pen: Option<PenSession>,
    /// Current frame of the edited timeline (the playhead).
    frame: u32,
    /// Symbol-editing levels, outermost first (empty = main timeline).
    levels: Vec<EditLevel>,
    /// Runtime used during preview playback of the main timeline.
    player: Option<Player>,
    images: Rc<RefCell<ImageCache>>,
    /// Bumped when the document is replaced, so in-flight decodes for the
    /// old document are discarded.
    generation: Rc<Cell<u32>>,
    pool: RefCell<CanvasPool>,
}

fn js_err(e: Error) -> JsError {
    JsError::new(&e.to_string())
}

fn parse<T: DeserializeOwned>(what: &str, json: &str) -> Result<T, JsError> {
    serde_json::from_str(json).map_err(|e| JsError::new(&format!("bad {what}: {e}")))
}

fn ids(json: &str) -> Result<Vec<ElementId>, JsError> {
    Ok(parse::<Vec<u32>>("id list", json)?.into_iter().map(ElementId).collect())
}

fn layer_ids(json: &str) -> Result<Vec<LayerId>, JsError> {
    Ok(parse::<Vec<u32>>("layer id list", json)?.into_iter().map(LayerId).collect())
}

fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("serializable")
}

#[wasm_bindgen]
impl Engine {
    /// Creates an engine holding the built-in demo project.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Engine {
        Engine {
            doc: Document::new(demo::demo_project()),
            session: None,
            edit: None,
            pen: None,
            frame: 0,
            levels: Vec::new(),
            player: None,
            images: Rc::default(),
            generation: Rc::default(),
            pool: RefCell::default(),
        }
    }

    fn replace_document(&mut self, project: Project) {
        self.doc = Document::new(project);
        self.session = None;
        self.edit = None;
        self.pen = None;
        self.frame = 0;
        self.levels.clear();
        self.player = None;
        *self.images.borrow_mut() = ImageCache::default();
        self.generation.set(self.generation.get() + 1);
    }

    /// The edited symbol, its symbol→stage matrix and frame. Instances on
    /// the edit path are placed as displayed at their parent's frame;
    /// symbols opened from the library sit at the stage center.
    fn scope(&self) -> Scope {
        let p = &self.doc.project;
        let mut scope = Scope::root(p);
        for level in &self.levels {
            match level.instance {
                Some(id) => {
                    let parent = scope.at(level.parent_frame);
                    if let Some(se) = query::displayed(p, &parent, id) {
                        scope.matrix = scope.matrix * se.element.transform.matrix();
                    }
                }
                None => scope.matrix = Matrix::translate(p.stage.width / 2.0, p.stage.height / 2.0),
            }
            scope.symbol = level.symbol;
        }
        scope.at(self.frame)
    }

    /// Stage point → edited symbol's space.
    fn local(&self, x: f64, y: f64) -> Point {
        let inv = self.scope().matrix.invert().unwrap_or(Matrix::IDENTITY);
        inv.apply(Point::new(x, y))
    }

    fn cancel_all(&mut self) {
        self.cancel_transform();
        self.cancel_edit();
        self.pen = None;
    }

    /// Draws `frame` into `ctx`. `scale`/`offset_*` map stage units to canvas
    /// pixels (device-pixel-ratio already applied by the caller).
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        ctx: &CanvasRenderingContext2d,
        frame: u32,
        scale: f64,
        offset_x: f64,
        offset_y: f64,
        clip: bool,
        show_guides: bool,
        onion_json: &str,
    ) -> Result<(), JsError> {
        let onion: Option<Onion> = parse("onion", onion_json)?;
        let view = Matrix::translate(offset_x, offset_y) * Matrix::scale(scale, scale);
        let images = self.images.borrow();
        let mut pool = self.pool.borrow_mut();
        let mut r = Canvas2dRenderer::new(ctx, &mut pool, &images.bitmaps);
        let opts = RenderOptions { view, clip_to_stage: clip, show_guides, onion };
        if self.levels.is_empty() {
            render_frame(&self.doc.project, frame, opts, &mut r);
        } else {
            let scope = self.scope();
            // Context only when every level was entered in place.
            let path: Vec<ElementId> = self.levels.iter().map_while(|l| l.instance).collect();
            let path = if path.len() == self.levels.len() { path } else { Vec::new() };
            let view = EditView {
                path: &path,
                root_frame: self.levels[0].parent_frame,
                symbol: scope.symbol,
                frame,
                matrix: scope.matrix,
                context_alpha: 0.3,
            };
            render_editing(&self.doc.project, &view, opts, &mut r);
        }
        Ok(())
    }

    // ----- symbol editing -----

    /// Edits the symbol of a displayed instance in place (double-click).
    #[wasm_bindgen(js_name = enterInstance)]
    pub fn enter_instance(&mut self, id: u32) -> Result<(), JsError> {
        let scope = self.scope();
        let se = query::displayed(&self.doc.project, &scope, ElementId(id)).ok_or_else(|| JsError::new("that object isn't on this frame"))?;
        let zoetrope_core::ElementKind::Instance { symbol, .. } = se.element.kind else {
            return Err(JsError::new("only symbol instances can be edited in place"));
        };
        self.cancel_all();
        self.levels.push(EditLevel { instance: Some(ElementId(id)), symbol, parent_frame: self.frame });
        self.frame = 0;
        Ok(())
    }

    /// Edits a symbol on its own (from the library).
    #[wasm_bindgen(js_name = enterSymbol)]
    pub fn enter_symbol(&mut self, symbol: u32) -> Result<(), JsError> {
        let id = SymbolId(symbol);
        self.doc.project.require_symbol(id).map_err(js_err)?;
        if id == self.doc.project.root {
            self.exit_to(0);
            return Ok(());
        }
        self.cancel_all();
        self.levels.push(EditLevel { instance: None, symbol: id, parent_frame: self.frame });
        self.frame = 0;
        Ok(())
    }

    /// Leaves symbol editing down to `depth` levels (0 = main timeline),
    /// restoring the playhead the outer timeline had.
    #[wasm_bindgen(js_name = exitTo)]
    pub fn exit_to(&mut self, depth: usize) {
        if depth >= self.levels.len() {
            return;
        }
        self.cancel_all();
        self.frame = self.levels[depth].parent_frame;
        self.levels.truncate(depth);
    }

    #[wasm_bindgen(js_name = editDepth)]
    pub fn edit_depth(&self) -> usize {
        self.levels.len()
    }

    /// Drops edit levels whose instance or symbol no longer exists (after
    /// undo/delete). Returns true if anything changed.
    #[wasm_bindgen(js_name = repairEditStack)]
    pub fn repair_edit_stack(&mut self) -> bool {
        let p = &self.doc.project;
        let bad = self.levels.iter().position(|l| p.symbol(l.symbol).is_none() || l.instance.is_some_and(|id| p.element(id).is_none()));
        match bad {
            Some(depth) => {
                self.exit_to(depth);
                true
            }
            None => false,
        }
    }

    /// `[{ label, kind }]` from the main timeline down to the edited symbol.
    #[wasm_bindgen(js_name = breadcrumbJson)]
    pub fn breadcrumb_json(&self) -> String {
        let p = &self.doc.project;
        let mut crumbs = vec![serde_json::json!({ "label": p.symbol(p.root).map_or("Scene", |s| s.name.as_str()), "kind": "movieClip" })];
        for l in &self.levels {
            let sym = p.symbol(l.symbol);
            let name = sym.map_or("?", |s| s.name.as_str());
            let label = match l.instance.and_then(|id| p.element(id)).filter(|e| !e.name.is_empty()) {
                Some(e) => format!("{} ({name})", e.name),
                None => name.to_string(),
            };
            crumbs.push(serde_json::json!({ "label": label, "kind": sym.map(|s| s.kind) }));
        }
        to_json(&crumbs)
    }

    // ----- library -----

    /// `[{ id, name, kind, uses, length }]` for every symbol except the main timeline.
    #[wasm_bindgen(js_name = libraryJson)]
    pub fn library_json(&self) -> String {
        let p = &self.doc.project;
        let items: Vec<_> = p
            .symbols
            .iter()
            .filter(|s| s.id != p.root)
            .map(|s| {
                let uses = p.symbols.iter().flat_map(|o| o.instanced_symbols()).filter(|id| *id == s.id).count();
                serde_json::json!({ "id": s.id.0, "name": s.name, "kind": s.kind, "uses": uses, "length": s.length() })
            })
            .collect();
        to_json(&items)
    }

    /// Draws a symbol's first frame fitted into the canvas (library thumbnails).
    #[wasm_bindgen(js_name = renderSymbolPreview)]
    pub fn render_symbol_preview(&self, ctx: &CanvasRenderingContext2d, symbol: u32, width: f64, height: f64) {
        let p = &self.doc.project;
        let id = SymbolId(symbol);
        let kind = zoetrope_core::ElementKind::instance(id);
        let Some(b) = query::content_bounds(p, &kind, &Matrix::IDENTITY, 0, 0) else { return };
        let k = ((width - 6.0) / b.width().max(1e-6)).min((height - 6.0) / b.height().max(1e-6)).min(4.0);
        let c = b.center();
        let m = Matrix::translate(width / 2.0, height / 2.0) * Matrix::scale(k, k) * Matrix::translate(-c.x, -c.y);
        let view = EditView { path: &[], root_frame: 0, symbol: id, frame: 0, matrix: m, context_alpha: 0.0 };
        let images = self.images.borrow();
        let mut pool = self.pool.borrow_mut();
        let mut r = PreviewRenderer(Canvas2dRenderer::new(ctx, &mut pool, &images.bitmaps));
        render_editing(p, &view, RenderOptions { view: Matrix::IDENTITY, clip_to_stage: false, show_guides: false, onion: None }, &mut r);
    }

    /// F8: turns elements into a symbol instance. `kind`: graphic | movieClip | button.
    #[wasm_bindgen(js_name = convertToSymbol)]
    pub fn convert_to_symbol(&mut self, ids_json: &str, name: &str, kind: &str) -> Result<u32, JsError> {
        let kind: SymbolKind = parse("symbol kind", &format!("{kind:?}"))?;
        let (instance, _) = ops::convert_to_symbol(&mut self.doc, &ids(ids_json)?, name, kind, self.frame).map_err(js_err)?;
        Ok(instance.0)
    }

    /// `patch_json`: `{ name?, kind? }`.
    #[wasm_bindgen(js_name = setSymbolProps)]
    pub fn set_symbol_props(&mut self, symbol: u32, patch_json: &str) -> Result<(), JsError> {
        #[derive(serde::Deserialize)]
        struct Patch {
            name: Option<String>,
            kind: Option<SymbolKind>,
        }
        let patch: Patch = parse("symbol patch", patch_json)?;
        ops::set_symbol_props(&mut self.doc, SymbolId(symbol), patch.name.as_deref(), patch.kind).map_err(js_err)
    }

    #[wasm_bindgen(js_name = duplicateSymbol)]
    pub fn duplicate_symbol(&mut self, symbol: u32) -> Result<u32, JsError> {
        ops::duplicate_symbol(&mut self.doc, SymbolId(symbol)).map(|s| s.0).map_err(js_err)
    }

    #[wasm_bindgen(js_name = deleteSymbol)]
    pub fn delete_symbol(&mut self, symbol: u32) -> Result<(), JsError> {
        ops::delete_symbol(&mut self.doc, SymbolId(symbol)).map_err(js_err)
    }

    #[wasm_bindgen(js_name = swapSymbol)]
    pub fn swap_symbol(&mut self, ids_json: &str, symbol: u32) -> Result<(), JsError> {
        ops::swap_symbol(&mut self.doc, &ids(ids_json)?, SymbolId(symbol)).map_err(js_err)
    }

    /// Drops a library symbol onto the stage at a stage point.
    #[wasm_bindgen(js_name = placeInstance)]
    pub fn place_instance(&mut self, layer: u32, symbol: u32, x: f64, y: f64) -> Result<u32, JsError> {
        let at = self.local(x, y);
        ops::place_instance(&mut self.doc, LayerId(layer), SymbolId(symbol), at, self.frame).map(|e| e.0).map_err(js_err)
    }

    // ----- preview playback (runtime clocks; main timeline only) -----

    /// Starts the runtime at the current frame of the main timeline.
    #[wasm_bindgen(js_name = playStart)]
    pub fn play_start(&mut self) -> Result<(), JsError> {
        if !self.levels.is_empty() {
            return Err(JsError::new("preview plays the main timeline: leave symbol editing first"));
        }
        self.cancel_all();
        self.player = Some(Player::new(&self.doc.project, self.frame));
        Ok(())
    }

    /// Advances `ticks` frames; returns the main timeline's frame.
    #[wasm_bindgen(js_name = playTick)]
    pub fn play_tick(&mut self, ticks: u32) -> u32 {
        let Some(pl) = &mut self.player else { return self.frame };
        // A stalled tab can owe many frames; never spin for longer than ~10 s of animation.
        for _ in 0..ticks.min(MAX_CATCH_UP_TICKS) {
            pl.tick(&self.doc.project);
        }
        pl.frame
    }

    /// Feeds the pointer (stage coords; `inside` false when it left the stage).
    /// Returns `{ events, overButton }`.
    #[wasm_bindgen(js_name = playPointer)]
    pub fn play_pointer(&mut self, x: f64, y: f64, inside: bool, down: bool) -> String {
        let Some(pl) = &mut self.player else { return "null".into() };
        let events = pl.pointer(&self.doc.project, inside.then(|| Point::new(x, y)), down);
        serde_json::json!({ "events": events, "overButton": pl.over_button() }).to_string()
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = playRender)]
    pub fn play_render(&self, ctx: &CanvasRenderingContext2d, scale: f64, offset_x: f64, offset_y: f64, clip: bool, show_guides: bool) {
        let Some(pl) = &self.player else { return };
        let view = Matrix::translate(offset_x, offset_y) * Matrix::scale(scale, scale);
        let images = self.images.borrow();
        let mut pool = self.pool.borrow_mut();
        let mut r = Canvas2dRenderer::new(ctx, &mut pool, &images.bitmaps);
        pl.render(&self.doc.project, RenderOptions { view, clip_to_stage: clip, show_guides, onion: None }, &mut r);
    }

    /// Stops the runtime; the playhead stays where playback reached.
    #[wasm_bindgen(js_name = playStop)]
    pub fn play_stop(&mut self) -> u32 {
        if let Some(pl) = self.player.take() {
            self.frame = pl.frame;
        }
        self.frame
    }

    #[wasm_bindgen(js_name = isPlaying)]
    pub fn is_playing(&self) -> bool {
        self.player.is_some()
    }

    // ----- timeline -----

    /// Moves the editing playhead. Cancels any drag in progress.
    #[wasm_bindgen(js_name = setFrame)]
    pub fn set_frame(&mut self, frame: u32) {
        if frame != self.frame {
            self.cancel_transform();
            self.cancel_edit();
            self.frame = frame;
        }
    }

    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// Frames in the edited timeline.
    #[wasm_bindgen(js_name = timelineLength)]
    pub fn timeline_length(&self) -> u32 {
        self.doc.project.symbol(self.scope().symbol).map_or(1, |s| s.length())
    }

    /// F5: lengthen the span at `frame` on each layer (JSON id list).
    #[wasm_bindgen(js_name = insertFrames)]
    pub fn insert_frames(&mut self, layers_json: &str, frame: u32, count: u32) -> Result<(), JsError> {
        ops::insert_frames(&mut self.doc, &layer_ids(layers_json)?, frame, count).map_err(js_err)
    }

    /// ⇧F5: shorten the span at `frame`.
    #[wasm_bindgen(js_name = removeFrames)]
    pub fn remove_frames(&mut self, layers_json: &str, frame: u32, count: u32) -> Result<(), JsError> {
        ops::remove_frames(&mut self.doc, &layer_ids(layers_json)?, frame, count).map_err(js_err)
    }

    /// F6 (copy what's shown) / F7 (`blank`).
    #[wasm_bindgen(js_name = insertKeyframe)]
    pub fn insert_keyframe(&mut self, layers_json: &str, frame: u32, blank: bool) -> Result<(), JsError> {
        ops::insert_keyframe(&mut self.doc, &layer_ids(layers_json)?, frame, blank).map_err(js_err)
    }

    /// ⇧F6: remove the keyframe starting at `frame`.
    #[wasm_bindgen(js_name = clearKeyframe)]
    pub fn clear_keyframe(&mut self, layers_json: &str, frame: u32) -> Result<(), JsError> {
        ops::clear_keyframe(&mut self.doc, &layer_ids(layers_json)?, frame).map_err(js_err)
    }

    /// Sets the tween of the span at `frame`: `{ kind, easing, rotate }` or `null`.
    #[wasm_bindgen(js_name = setTween)]
    pub fn set_tween(&mut self, layers_json: &str, frame: u32, tween_json: &str) -> Result<(), JsError> {
        let tween: Option<Tween> = parse("tween", tween_json)?;
        ops::set_tween(&mut self.doc, &layer_ids(layers_json)?, frame, tween).map_err(js_err)
    }

    /// `samples + 1` eased values for t = 0, 1/samples, …, 1 (for the curve editor).
    #[wasm_bindgen(js_name = easingCurveJson)]
    pub fn easing_curve_json(&self, easing_json: &str, samples: u32) -> Result<String, JsError> {
        let easing: Easing = parse("easing", easing_json)?;
        let n = samples.clamp(1, 1000);
        let ys: Vec<f64> = (0..=n).map(|i| easing.apply(i as f64 / n as f64)).collect();
        Ok(to_json(&ys))
    }

    /// Decodes embedded images that are not decoded yet. Resolves to the
    /// number decoded; re-render afterwards. Undecodable images are skipped
    /// (they render as placeholders) and reported on the console.
    #[wasm_bindgen(js_name = decodeImages)]
    pub fn decode_images(&self) -> js_sys::Promise {
        let cache = self.images.borrow();
        let pending: Vec<_> = self
            .doc
            .project
            .assets
            .iter()
            .filter(|a| !cache.bitmaps.contains_key(&a.id) && !cache.failed.contains(&a.id))
            .map(|a| {
                let AssetKind::Image { mime, data, .. } = &a.kind;
                (a.id, a.name.clone(), mime.clone(), data.clone())
            })
            .collect();
        drop(cache);
        let images = self.images.clone();
        let generation = self.generation.clone();
        let started = generation.get();
        wasm_bindgen_futures::future_to_promise(async move {
            let mut decoded = 0;
            for (id, name, mime, data) in pending {
                match decode(&mime, &data.0).await {
                    Ok(bmp) if generation.get() == started => {
                        images.borrow_mut().bitmaps.insert(id, bmp);
                        decoded += 1;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        web_sys::console::warn_2(&format!("could not decode image {name:?}:").into(), &e);
                        if generation.get() == started {
                            images.borrow_mut().failed.insert(id);
                        }
                    }
                }
            }
            Ok(JsValue::from(decoded))
        })
    }

    // ----- documents -----

    #[wasm_bindgen(js_name = newDemo)]
    pub fn new_demo(&mut self) {
        self.replace_document(demo::demo_project());
    }

    /// Serializes the project in the versioned file format.
    #[wasm_bindgen(js_name = saveJson)]
    pub fn save_json(&self) -> String {
        format::save_to_string(&self.doc.project)
    }

    /// Replaces the document (and clears history). Leaves the current
    /// document untouched if the file is invalid.
    #[wasm_bindgen(js_name = loadJson)]
    pub fn load_json(&mut self, json: &str) -> Result<(), JsError> {
        let project = format::load_from_str(json).map_err(js_err)?;
        self.replace_document(project);
        Ok(())
    }

    #[wasm_bindgen(js_name = markSaved)]
    pub fn mark_saved(&mut self) {
        self.doc.mark_saved();
    }

    // ----- queries (JSON) -----

    #[wasm_bindgen(js_name = rootSymbol)]
    pub fn root_symbol(&self) -> u32 {
        self.doc.project.root.0
    }

    #[wasm_bindgen(js_name = stageJson)]
    pub fn stage_json(&self) -> String {
        to_json(&self.doc.project.stage)
    }

    /// Layer tree of the edited symbol, top layer first.
    #[wasm_bindgen(js_name = layersJson)]
    pub fn layers_json(&self) -> String {
        to_json(&outline::layer_tree(&self.doc.project, self.scope().symbol))
    }

    /// `{ canUndo, canRedo, undoLabel, redoLabel, dirty }`
    #[wasm_bindgen(js_name = historyJson)]
    pub fn history_json(&self) -> String {
        serde_json::json!({
            "canUndo": self.doc.undo_label().is_some(),
            "canRedo": self.doc.redo_label().is_some(),
            "undoLabel": self.doc.undo_label(),
            "redoLabel": self.doc.redo_label(),
            "dirty": self.doc.is_dirty(),
        })
        .to_string()
    }

    /// Element + derived info for the properties panel, or `null`.
    #[wasm_bindgen(js_name = elementJson)]
    pub fn element_json(&self, id: u32) -> String {
        to_json(&outline::element_info(&self.doc.project, ElementId(id), self.frame))
    }

    /// Handle geometry for a selection (JSON id list), or `null`.
    #[wasm_bindgen(js_name = selectionJson)]
    pub fn selection_json(&self, ids_json: &str) -> Result<String, JsError> {
        let ids = ids(ids_json)?;
        Ok(to_json(&query::selection_geometry(&self.doc.project, &self.scope(), &ids)))
    }

    /// Topmost selectable element at a stage point (`tolerance` in stage units).
    #[wasm_bindgen(js_name = hitTest)]
    pub fn hit_test(&self, x: f64, y: f64, tolerance: f64) -> Option<u32> {
        query::hit_test(&self.doc.project, &self.scope(), Point::new(x, y), tolerance).map(|e| e.0)
    }

    /// Ids (JSON) of selectable elements touching the stage rectangle.
    pub fn marquee(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> String {
        let ids: Vec<u32> =
            query::marquee(&self.doc.project, &self.scope(), Rect::new(x0, y0, x1, y1)).into_iter().map(|e| e.0).collect();
        to_json(&ids)
    }

    /// Ids (JSON) of every selectable element in the edited symbol.
    #[wasm_bindgen(js_name = selectAll)]
    pub fn select_all(&self) -> String {
        let ids: Vec<u32> = query::selectable(&self.doc.project, &self.scope()).iter().map(|e| e.id.0).collect();
        to_json(&ids)
    }

    /// Drops ids that no longer exist or are no longer selectable (after
    /// undo, layer locks, etc.). Returns the filtered JSON id list.
    #[wasm_bindgen(js_name = validSelection)]
    pub fn valid_selection(&self, ids_json: &str) -> Result<String, JsError> {
        let wanted = ids(ids_json)?;
        let selectable: HashSet<u32> = query::selectable(&self.doc.project, &self.scope()).iter().map(|e| e.id.0).collect();
        let kept: Vec<u32> = wanted.into_iter().map(|e| e.0).filter(|id| selectable.contains(id)).collect();
        Ok(to_json(&kept))
    }

    // ----- direct manipulation -----

    /// Starts a drag. `mode_json`: `{"mode":"move"}`, `{"mode":"scale","handle":"se"}`,
    /// `{"mode":"rotate"}`, `{"mode":"skew","handle":"n"}`, `{"mode":"pivot"}`.
    #[wasm_bindgen(js_name = beginTransform)]
    pub fn begin_transform(&mut self, ids_json: &str, mode_json: &str, x: f64, y: f64) -> Result<(), JsError> {
        let ids = ids(ids_json)?;
        let mode: DragMode = parse("drag mode", mode_json)?;
        self.cancel_transform();
        let s = TransformSession::begin(&self.doc.project, self.scope(), &ids, mode, Point::new(x, y)).map_err(js_err)?;
        self.session = Some(s);
        Ok(())
    }

    /// Previews the drag at a stage point. Returns snap guides as JSON.
    #[wasm_bindgen(js_name = updateTransform)]
    pub fn update_transform(&mut self, x: f64, y: f64, mods_json: &str, snap_json: &str) -> Result<String, JsError> {
        let mods: Modifiers = parse("modifiers", mods_json)?;
        let snap: SnapConfig = parse("snap config", snap_json)?;
        let s = self.session.as_ref().ok_or_else(|| JsError::new("no drag in progress"))?;
        Ok(to_json(&s.update(&mut self.doc.project, Point::new(x, y), mods, &snap)))
    }

    /// Commits the drag as one undo step.
    #[wasm_bindgen(js_name = endTransform)]
    pub fn end_transform(&mut self) -> Result<(), JsError> {
        match self.session.take() {
            Some(s) => s.commit(&mut self.doc).map_err(js_err),
            None => Ok(()),
        }
    }

    #[wasm_bindgen(js_name = cancelTransform)]
    pub fn cancel_transform(&mut self) {
        if let Some(s) = self.session.take() {
            s.cancel(&mut self.doc.project);
        }
    }

    /// Snaps a stage point for drawing tools. Returns `{ x, y, guides }`.
    #[wasm_bindgen(js_name = snapPoint)]
    pub fn snap_point(&self, x: f64, y: f64, snap_json: &str) -> Result<String, JsError> {
        let snap: SnapConfig = parse("snap config", snap_json)?;
        let targets = SnapTargets::collect(&self.doc.project, &self.scope(), &[]);
        let (p, guides) = snap_point(Point::new(x, y), &targets, &snap);
        Ok(serde_json::json!({ "x": p.x, "y": p.y, "guides": guides }).to_string())
    }

    /// Outline (stage-space polylines) of the shape a drawing-tool drag
    /// would create, for the live preview; `null` if too small.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = shapePreview)]
    pub fn shape_preview(&self, tool: &str, x0: f64, y0: f64, x1: f64, y1: f64, mods_json: &str, opts_json: &str) -> Result<String, JsError> {
        let tool: ShapeTool = parse("tool", &format!("{tool:?}"))?;
        let mods: Modifiers = parse("modifiers", mods_json)?;
        let opts: ShapeOptions = parse("shape options", opts_json)?;
        // Preview in stage space; the created shape lives in the edited symbol's space.
        let to_stage = self.scope().matrix;
        let (p0, p1) = (self.local(x0, y0), self.local(x1, y1));
        Ok(to_json(&shape_from_drag(tool, p0, p1, mods, &opts).map(|d| {
            let mut v = d.geometry.to_vector_path();
            v.transform(&(to_stage * Matrix::translate(d.center.x, d.center.y)));
            v.outline(&Matrix::IDENTITY)
        })))
    }

    // ----- pen tool (stage coordinates) -----

    /// Pen click. Returns true if it closed the path (finish on pointer-up).
    #[wasm_bindgen(js_name = penDown)]
    pub fn pen_down(&mut self, x: f64, y: f64, mods_json: &str, close_tolerance: f64) -> Result<bool, JsError> {
        let mods: Modifiers = parse("modifiers", mods_json)?;
        Ok(self.pen.get_or_insert_with(PenSession::default).pointer_down(Point::new(x, y), mods, close_tolerance))
    }

    #[wasm_bindgen(js_name = penDrag)]
    pub fn pen_drag(&mut self, x: f64, y: f64, mods_json: &str) -> Result<(), JsError> {
        let mods: Modifiers = parse("modifiers", mods_json)?;
        if let Some(p) = &mut self.pen {
            p.pointer_drag(Point::new(x, y), mods);
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = penUp)]
    pub fn pen_up(&mut self) {
        if let Some(p) = &mut self.pen {
            p.pointer_up();
        }
    }

    #[wasm_bindgen(js_name = penHover)]
    pub fn pen_hover(&mut self, x: f64, y: f64, mods_json: &str) -> Result<(), JsError> {
        let mods: Modifiers = parse("modifiers", mods_json)?;
        if let Some(p) = &mut self.pen {
            p.hover(Point::new(x, y), mods);
        }
        Ok(())
    }

    /// `{ outline, anchors, handles, canClose }` or `null` when no pen path is in progress.
    #[wasm_bindgen(js_name = penPreviewJson)]
    pub fn pen_preview_json(&self, close_tolerance: f64) -> String {
        to_json(&self.pen.as_ref().map(|p| p.preview(close_tolerance)))
    }

    /// Ends the pen path and creates it on `layer`. Returns the new id, or
    /// `undefined` if there were fewer than two anchors.
    #[wasm_bindgen(js_name = penFinish)]
    pub fn pen_finish(&mut self, layer: u32, style_json: &str) -> Result<Option<u32>, JsError> {
        let style: ShapeStyle = parse("style", style_json)?;
        let Some(mut path) = self.pen.take().and_then(PenSession::finish) else { return Ok(None) };
        path.transform(&self.scope().matrix.invert().unwrap_or(Matrix::IDENTITY));
        ops::create_path(&mut self.doc, LayerId(layer), path, &style, "Pen", self.frame).map(|e| Some(e.0)).map_err(js_err)
    }

    #[wasm_bindgen(js_name = penCancel)]
    pub fn pen_cancel(&mut self) {
        self.pen = None;
    }

    // ----- pencil -----

    /// Creates a path from freehand samples (JSON `[{x, y}, …]`, stage coords).
    #[wasm_bindgen(js_name = createFreehand)]
    pub fn create_freehand(&mut self, layer: u32, points_json: &str, smooth: bool, tolerance: f64, style_json: &str) -> Result<Option<u32>, JsError> {
        let inv = self.scope().matrix.invert().unwrap_or(Matrix::IDENTITY);
        let points: Vec<Point> = parse::<Vec<Point>>("points", points_json)?.into_iter().map(|p| inv.apply(p)).collect();
        let style: ShapeStyle = parse("style", style_json)?;
        ops::create_freehand(&mut self.doc, LayerId(layer), &points, smooth, tolerance, &style, self.frame).map(|e| e.map(|e| e.0)).map_err(js_err)
    }

    // ----- subselection & gradient editing -----

    /// Anchors/handles/outline of a shape in stage coordinates, or `null`.
    #[wasm_bindgen(js_name = pathInfoJson)]
    pub fn path_info_json(&self, id: u32) -> String {
        to_json(&path_info(&self.doc.project, &self.scope(), ElementId(id)))
    }

    /// Nearest outline position `{ subpath, segment, t, distance }` within `tolerance`, or `null`.
    #[wasm_bindgen(js_name = pathHitJson)]
    pub fn path_hit_json(&self, id: u32, x: f64, y: f64, tolerance: f64) -> String {
        to_json(&path_hit(&self.doc.project, &self.scope(), ElementId(id), Point::new(x, y), tolerance))
    }

    /// Gradient controls of a shape's `part` ("fill" | "stroke") in stage coordinates, or `null`.
    #[wasm_bindgen(js_name = gradientJson)]
    pub fn gradient_json(&self, id: u32, part: &str) -> Result<String, JsError> {
        let part: PaintPart = parse("paint part", &format!("{part:?}"))?;
        Ok(to_json(&gradient_controls(&self.doc.project, &self.scope(), ElementId(id), part)))
    }

    /// Starts an anchor/handle/gradient drag. `target_json`:
    /// `{"target":"anchors","nodes":[…]}`, `{"target":"handle","node":{…},"side":"in"}`,
    /// `{"target":"gradient","part":"fill","handle":"start"}`.
    #[wasm_bindgen(js_name = beginEdit)]
    pub fn begin_edit(&mut self, id: u32, target_json: &str, x: f64, y: f64) -> Result<(), JsError> {
        let target: EditTarget = parse("edit target", target_json)?;
        self.cancel_edit();
        self.edit = Some(EditSession::begin(&self.doc.project, self.scope(), ElementId(id), target, Point::new(x, y)).map_err(js_err)?);
        Ok(())
    }

    #[wasm_bindgen(js_name = updateEdit)]
    pub fn update_edit(&mut self, x: f64, y: f64, mods_json: &str) -> Result<(), JsError> {
        let mods: Modifiers = parse("modifiers", mods_json)?;
        let s = self.edit.as_ref().ok_or_else(|| JsError::new("no edit in progress"))?;
        s.update(&mut self.doc.project, Point::new(x, y), mods).map_err(js_err)
    }

    #[wasm_bindgen(js_name = endEdit)]
    pub fn end_edit(&mut self) -> Result<(), JsError> {
        match self.edit.take() {
            Some(s) => s.commit(&mut self.doc).map_err(js_err),
            None => Ok(()),
        }
    }

    #[wasm_bindgen(js_name = cancelEdit)]
    pub fn cancel_edit(&mut self) {
        if let Some(s) = self.edit.take() {
            s.cancel(&mut self.doc.project);
        }
    }

    /// Splits a segment, returning the new anchor `{ subpath, node }` (JSON).
    #[wasm_bindgen(js_name = insertAnchor)]
    pub fn insert_anchor(&mut self, id: u32, subpath: usize, segment: usize, t: f64) -> Result<String, JsError> {
        ops::insert_anchor(&mut self.doc, ElementId(id), subpath, segment, t).map(|r| to_json(&r)).map_err(js_err)
    }

    #[wasm_bindgen(js_name = deleteAnchors)]
    pub fn delete_anchors(&mut self, id: u32, nodes_json: &str) -> Result<(), JsError> {
        let nodes: Vec<NodeRef> = parse("anchors", nodes_json)?;
        ops::delete_anchors(&mut self.doc, ElementId(id), &nodes).map_err(js_err)
    }

    /// Toggles an anchor between corner and smooth.
    #[wasm_bindgen(js_name = convertAnchor)]
    pub fn convert_anchor(&mut self, id: u32, subpath: usize, node: usize) -> Result<(), JsError> {
        ops::convert_anchor(&mut self.doc, ElementId(id), NodeRef { subpath, node }).map_err(js_err)
    }

    #[wasm_bindgen(js_name = convertToPath)]
    pub fn convert_to_path(&mut self, ids_json: &str) -> Result<(), JsError> {
        ops::convert_to_path(&mut self.doc, &ids(ids_json)?).map_err(js_err)
    }

    // ----- paint -----

    /// Eyedropper: `{ part, fill, stroke }` styles of the shape under the point, or `null`.
    #[wasm_bindgen(js_name = pickStyle)]
    pub fn pick_style(&self, x: f64, y: f64, tolerance: f64) -> String {
        to_json(&ops::pick_style(&self.doc.project, &self.scope(), Point::new(x, y), tolerance))
    }

    /// Paint bucket / panel: sets `part` ("fill" | "stroke") of shapes from a
    /// geometry-free style (`null` removes it); gradients fit each shape.
    #[wasm_bindgen(js_name = setPaintStyle)]
    pub fn set_paint_style(&mut self, ids_json: &str, part: &str, style_json: &str) -> Result<(), JsError> {
        let part: PaintPart = parse("paint part", &format!("{part:?}"))?;
        let style: Option<PaintStyle> = parse("paint style", style_json)?;
        ops::set_paint_style(&mut self.doc, &ids(ids_json)?, part, style.as_ref()).map_err(js_err)
    }

    // ----- commands -----

    pub fn undo(&mut self) -> Result<bool, JsError> {
        self.cancel_transform();
        self.cancel_edit();
        self.doc.undo().map_err(js_err)
    }

    pub fn redo(&mut self) -> Result<bool, JsError> {
        self.cancel_transform();
        self.cancel_edit();
        self.doc.redo().map_err(js_err)
    }

    /// Creates a shape from a drawing-tool drag on top of `layer`. Returns the
    /// new id, or `undefined` if the drag was too small.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = createShape)]
    pub fn create_shape(
        &mut self,
        layer: u32,
        tool: &str,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        mods_json: &str,
        opts_json: &str,
        style_json: &str,
    ) -> Result<Option<u32>, JsError> {
        let tool: ShapeTool = parse("tool", &format!("{tool:?}"))?;
        let mods: Modifiers = parse("modifiers", mods_json)?;
        let opts: ShapeOptions = parse("shape options", opts_json)?;
        let style: ShapeStyle = parse("style", style_json)?;
        let (p0, p1) = (self.local(x0, y0), self.local(x1, y1));
        let id = ops::create_shape(&mut self.doc, LayerId(layer), tool, p0, p1, mods, &opts, &style, self.frame)
            .map_err(js_err)?;
        Ok(id.map(|e| e.0))
    }

    /// Embeds image bytes (PNG/JPEG/GIF) and places them centered at (x, y)
    /// on `layer`. Call `decodeImages()` afterwards.
    #[wasm_bindgen(js_name = importImage)]
    pub fn import_image(&mut self, layer: u32, name: &str, bytes: &[u8], x: f64, y: f64) -> Result<u32, JsError> {
        let at = self.local(x, y);
        ops::import_image(&mut self.doc, LayerId(layer), name, bytes, at, self.frame).map(|e| e.0).map_err(js_err)
    }

    /// Merges a JSON patch into each element (see `ops::patch_element`).
    #[wasm_bindgen(js_name = patchElements)]
    pub fn patch_elements(&mut self, ids_json: &str, patch_json: &str) -> Result<(), JsError> {
        let ids = ids(ids_json)?;
        let patch: serde_json::Value = parse("patch", patch_json)?;
        ops::patch_elements(&mut self.doc, &ids, &patch).map_err(js_err)
    }

    #[wasm_bindgen(js_name = setElementSize)]
    pub fn set_element_size(&mut self, id: u32, width: f64, height: f64) -> Result<(), JsError> {
        ops::set_element_size(&mut self.doc, ElementId(id), width, height, self.frame).map_err(js_err)
    }

    #[wasm_bindgen(js_name = translateElements)]
    pub fn translate_elements(&mut self, ids_json: &str, dx: f64, dy: f64) -> Result<(), JsError> {
        ops::translate_elements(&mut self.doc, &ids(ids_json)?, dx, dy).map_err(js_err)
    }

    #[wasm_bindgen(js_name = deleteElements)]
    pub fn delete_elements(&mut self, ids_json: &str) -> Result<(), JsError> {
        ops::delete_elements(&mut self.doc, &ids(ids_json)?).map_err(js_err)
    }

    /// Returns the new ids (JSON).
    #[wasm_bindgen(js_name = duplicateElements)]
    pub fn duplicate_elements(&mut self, ids_json: &str, dx: f64, dy: f64) -> Result<String, JsError> {
        let new = ops::duplicate_elements(&mut self.doc, &ids(ids_json)?, dx, dy).map_err(js_err)?;
        Ok(to_json(&new.iter().map(|e| e.0).collect::<Vec<_>>()))
    }

    /// `mode`: left | centerX | right | top | centerY | bottom.
    pub fn align(&mut self, ids_json: &str, mode: &str, to_stage: bool) -> Result<(), JsError> {
        let mode: Align = parse("align mode", &format!("{mode:?}"))?;
        ops::align(&mut self.doc, &ids(ids_json)?, mode, to_stage, self.frame).map_err(js_err)
    }

    /// `mode`: centersX | centersY | spaceX | spaceY.
    pub fn distribute(&mut self, ids_json: &str, mode: &str, to_stage: bool) -> Result<(), JsError> {
        let mode: Distribute = parse("distribute mode", &format!("{mode:?}"))?;
        ops::distribute(&mut self.doc, &ids(ids_json)?, mode, to_stage, self.frame).map_err(js_err)
    }

    /// `op`: front | forward | backward | back.
    pub fn arrange(&mut self, ids_json: &str, op: &str) -> Result<(), JsError> {
        let op: Arrange = parse("arrange op", &format!("{op:?}"))?;
        ops::arrange(&mut self.doc, &ids(ids_json)?, op).map_err(js_err)
    }

    #[wasm_bindgen(js_name = moveToLayer)]
    pub fn move_to_layer(&mut self, ids_json: &str, layer: u32) -> Result<(), JsError> {
        ops::move_to_layer(&mut self.doc, &ids(ids_json)?, LayerId(layer), self.frame).map_err(js_err)
    }

    /// Replaces the stage settings: `{ width, height, background, fps }`.
    #[wasm_bindgen(js_name = setStage)]
    pub fn set_stage(&mut self, stage_json: &str) -> Result<(), JsError> {
        let stage: Stage = parse("stage", stage_json)?;
        ops::set_stage(&mut self.doc, stage).map_err(js_err)
    }

    // ----- layers -----

    /// Adds a layer above `above` (or at the top). `kind`: normal | guide | folder.
    #[wasm_bindgen(js_name = addLayer)]
    pub fn add_layer(&mut self, above: Option<u32>, kind: &str) -> Result<u32, JsError> {
        let kind: LayerKind = parse("layer kind", &format!("{kind:?}"))?;
        let symbol = self.scope().symbol;
        ops::add_layer(&mut self.doc, symbol, above.map(LayerId), kind).map(|l| l.0).map_err(js_err)
    }

    #[wasm_bindgen(js_name = deleteLayer)]
    pub fn delete_layer(&mut self, id: u32) -> Result<(), JsError> {
        ops::delete_layer(&mut self.doc, LayerId(id)).map_err(js_err)
    }

    /// `patch_json`: any of `{ name, visible, locked, kind }`.
    #[wasm_bindgen(js_name = setLayerProps)]
    pub fn set_layer_props(&mut self, id: u32, patch_json: &str) -> Result<(), JsError> {
        let patch: LayerPatch = parse("layer patch", patch_json)?;
        ops::set_layer_props(&mut self.doc, LayerId(id), &patch).map_err(js_err)
    }

    /// Moves a layer into folder `parent` (or the top level) at `index`
    /// (bottom-to-top storage order, counted after removal).
    #[wasm_bindgen(js_name = moveLayer)]
    pub fn move_layer(&mut self, id: u32, parent: Option<u32>, index: usize) -> Result<(), JsError> {
        ops::move_layer(&mut self.doc, LayerId(id), parent.map(LayerId), index).map_err(js_err)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

async fn decode(mime: &str, data: &[u8]) -> Result<ImageBitmap, JsValue> {
    let bytes = js_sys::Uint8Array::from(data);
    let parts = js_sys::Array::of1(&bytes);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)?;
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let bitmap = wasm_bindgen_futures::JsFuture::from(window.create_image_bitmap_with_blob(&blob)?).await?;
    bitmap.dyn_into()
}
