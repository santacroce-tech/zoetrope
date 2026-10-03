//! Rendering pipeline. The core walks the scene tree and emits backend-neutral
//! draw calls through the `Renderer` trait. Backends (Canvas2D today,
//! WebGL/wgpu later) only rasterize what they are told; they never see the
//! scene graph, so swapping one never touches scene logic.
//!
//! Color handling: vector fills/strokes arrive with the composed color
//! transform already applied. Images receive the `ColorTransform` because
//! only the backend can recolor pixels. Blend modes other than `Normal` are
//! drawn as isolated groups (`begin_group` … `end_group`) composited with
//! the element's opacity.

use crate::color::{Color, ColorTransform};
use crate::geom::Path;
use crate::math::Matrix;
use crate::model::*;
use crate::timeline::{evaluate_layer_shown, instance_frame, is_tweened_frame, Shown};

/// Per-frame setup handed to the backend.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameInfo {
    pub stage_width: f64,
    pub stage_height: f64,
    pub background: Color,
    /// Maps stage coordinates to backend (device pixel) coordinates.
    pub view: Matrix,
    /// Clip drawing to the stage rectangle (the player does; the editor
    /// shows off-stage content on the pasteboard).
    pub clip_to_stage: bool,
}

pub trait Renderer {
    /// Clears the target and paints the stage background.
    fn begin_frame(&mut self, frame: &FrameInfo);
    /// `transform` maps path coordinates (and gradient geometry) to backend
    /// coordinates (view included). Paint colors are final (color transforms
    /// already applied).
    fn fill_path(&mut self, path: &Path, transform: &Matrix, paint: &Paint, rule: FillRule);
    /// Width and dash lengths are in path coordinates (they scale with `transform`).
    fn stroke_path(&mut self, path: &Path, transform: &Matrix, stroke: &Stroke);
    /// Draws image `asset` (`width`×`height` pixels) centered on the origin of
    /// `transform`. `color` has uniform rgb multipliers (tint + alpha form).
    fn draw_image(&mut self, asset: AssetId, width: f64, height: f64, transform: &Matrix, color: &ColorTransform);
    /// Subsequent draws go to an isolated group until the matching `end_group`,
    /// which composites it with `blend` at `alpha`. Groups nest.
    fn begin_group(&mut self, blend: BlendMode, alpha: f64);
    fn end_group(&mut self);
    fn end_frame(&mut self);
}

#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    pub view: Matrix,
    pub clip_to_stage: bool,
    /// Draw guide layers (editor: yes; player/export: no).
    pub show_guides: bool,
    /// Editor onion skinning of the root timeline.
    pub onion: Option<Onion>,
}

impl RenderOptions {
    pub fn player(view: Matrix) -> Self {
        RenderOptions { view, clip_to_stage: true, show_guides: false, onion: None }
    }
}

/// Onion skin (root timeline, editor only): on each unlocked layer, the
/// `before`/`after` neighbouring frames are drawn faded (tinted blue / green)
/// just beneath that layer's current content, so animated objects show their
/// path even over opaque backgrounds on lower layers. Frames that look the
/// same as the current one are skipped. Nearer frames are stronger.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Onion {
    pub before: u32,
    pub after: u32,
    /// Opacity of the nearest onion frame (0..1).
    #[serde(default = "onion_alpha")]
    pub alpha: f64,
}

fn onion_alpha() -> f64 {
    0.35
}

/// Renders frame `frame` of the project. Deterministic: the same project,
/// frame and options always produce the same sequence of renderer calls.
///
pub fn render_frame(project: &Project, frame: u32, opts: RenderOptions, r: &mut dyn Renderer) {
    render_with_clock(project, frame, opts, &Stateless, r);
}

/// Renders the root timeline at `frame`, taking nested symbols' frames from `clock`.
pub fn render_with_clock(project: &Project, frame: u32, opts: RenderOptions, clock: &dyn Clock, r: &mut dyn Renderer) {
    begin(project, &opts, r);
    let ctx = Ctx::new(project, project.root, opts, clock, &[]);
    ctx.symbol(project.root, frame, opts.view, &ColorTransform::IDENTITY, 0, &mut Vec::new(), None, r);
    r.end_frame();
}

fn begin(project: &Project, opts: &RenderOptions, r: &mut dyn Renderer) {
    r.begin_frame(&FrameInfo {
        stage_width: project.stage.width,
        stage_height: project.stage.height,
        background: project.stage.background,
        view: opts.view,
        clip_to_stage: opts.clip_to_stage,
    });
}

/// What the editor shows while editing a symbol.
#[derive(Debug, Clone)]
pub struct EditView<'a> {
    /// Instances from the root down to the edited one (empty when editing a
    /// symbol from the library, i.e. without context).
    pub path: &'a [ElementId],
    /// Root frame the context is shown at.
    pub root_frame: u32,
    pub symbol: SymbolId,
    /// Frame of the edited symbol's timeline.
    pub frame: u32,
    /// Edited symbol's space → stage.
    pub matrix: Matrix,
    /// Opacity of the surrounding (dimmed) scene.
    pub context_alpha: f64,
}

/// Edit-in-place view: the rest of the scene dimmed (without the edited
/// instance), then the edited symbol on top at full strength.
pub fn render_editing(project: &Project, view: &EditView, opts: RenderOptions, r: &mut dyn Renderer) {
    begin(project, &opts, r);
    if !view.path.is_empty() {
        let context_opts = RenderOptions { onion: None, ..opts };
        let ctx = Ctx::new(project, project.root, context_opts, &Stateless, view.path);
        r.begin_group(BlendMode::Layer, view.context_alpha.clamp(0.0, 1.0));
        ctx.symbol(project.root, view.root_frame, opts.view, &ColorTransform::IDENTITY, 0, &mut Vec::new(), Some(0), r);
        r.end_group();
    }
    let ctx = Ctx::new(project, view.symbol, opts, &Stateless, &[]);
    ctx.symbol(view.symbol, view.frame, opts.view * view.matrix, &ColorTransform::IDENTITY, 0, &mut Vec::new(), None, r);
    r.end_frame();
}

/// Identifies an instance in the display tree: `(layer id, element track)`
/// for each nesting level from the root timeline down.
pub type InstancePath = Vec<(u32, u32)>;

/// Where nested symbols get their frames.
pub trait Clock {
    /// The frame the instance at `path` (a `kind` symbol) shows, given the
    /// stateless timing rule's answer. Must be < the symbol's length; callers clamp.
    fn frame(&self, path: &[(u32, u32)], kind: SymbolKind, stateless: u32) -> u32;

    /// Properties a script has set on the element at `path` (whose last
    /// step is the element's own layer and track).
    fn element_override(&self, _path: &[(u32, u32)]) -> Option<&crate::player::Override> {
        None
    }

    /// Cheap check that lets the walk skip override lookups entirely.
    fn has_overrides(&self) -> bool {
        false
    }
}

/// Timing derived purely from the timeline (editing, scrubbing, export of a
/// single frame): see `timeline::instance_frame`.
pub struct Stateless;

impl Clock for Stateless {
    fn frame(&self, _: &[(u32, u32)], _: SymbolKind, stateless: u32) -> u32 {
        stateless
    }
}

struct Ctx<'a> {
    project: &'a Project,
    opts: RenderOptions,
    clock: &'a dyn Clock,
    /// Length of the top timeline (onion frames never wrap past it).
    onion_length: u32,
    /// Instance path to leave out (edit-in-place context).
    exclude: &'a [ElementId],
}

impl<'a> Ctx<'a> {
    fn new(project: &'a Project, top: SymbolId, opts: RenderOptions, clock: &'a dyn Clock, exclude: &'a [ElementId]) -> Self {
        Ctx { project, opts, clock, onion_length: project.symbol(top).map_or(1, |s| s.length()), exclude }
    }

    #[allow(clippy::too_many_arguments)]
    fn symbol(
        &self,
        id: SymbolId,
        frame: u32,
        m: Matrix,
        ct: &ColorTransform,
        depth: usize,
        path: &mut InstancePath,
        excl: Option<usize>,
        r: &mut dyn Renderer,
    ) {
        if depth > MAX_NESTING_DEPTH {
            return;
        }
        let Some(symbol) = self.project.symbol(id) else { return };
        self.layers(&symbol.layers, frame, m, ct, depth, path, excl, r);
    }

    #[allow(clippy::too_many_arguments)]
    fn layers(
        &self,
        layers: &[Layer],
        frame: u32,
        m: Matrix,
        ct: &ColorTransform,
        depth: usize,
        path: &mut InstancePath,
        excl: Option<usize>,
        r: &mut dyn Renderer,
    ) {
        for layer in layers.iter().filter(|l| l.visible) {
            match layer.kind {
                LayerKind::Folder => self.layers(&layer.children, frame, m, ct, depth, path, excl, r),
                LayerKind::Guide if !self.opts.show_guides => {}
                LayerKind::Guide | LayerKind::Normal => {
                    if depth == 0 && !layer.locked {
                        self.onion_ghosts(layer, frame, m, r);
                    }
                    for shown in evaluate_layer_shown(layer, frame) {
                        self.element(&shown, layer.id, frame, m, ct, depth, path, excl, r);
                    }
                }
            }
        }
    }

    /// Faded neighbouring frames of one top-level layer (see `Onion`).
    fn onion_ghosts(&self, layer: &Layer, frame: u32, m: Matrix, r: &mut dyn Renderer) {
        let Some(onion) = self.opts.onion else { return };
        let current = layer.keyframe_at(frame).map(|(i, _)| i);
        let static_at = |f: u32| {
            // Same keyframe and not mid-tween at both frames ⇒ identical picture.
            layer.keyframe_at(f).map(|(i, _)| i) == current && !is_tweened_frame(layer, f) && !is_tweened_frame(layer, frame)
        };
        let mut ghost = |f: u32, k: u32, n: u32, tint: Color| {
            if static_at(f) {
                return;
            }
            let alpha = onion.alpha.clamp(0.0, 1.0) * (n - k + 1) as f64 / n as f64;
            let ct = ColorTransform::tint(tint, 0.45);
            r.begin_group(BlendMode::Layer, alpha);
            for shown in evaluate_layer_shown(layer, f) {
                self.element(&shown, layer.id, f, m, &ct, 0, &mut Vec::new(), None, r);
            }
            r.end_group();
        };
        for k in (1..=onion.before).rev() {
            if let Some(f) = frame.checked_sub(k) {
                ghost(f, k, onion.before, Color::rgb(0x30, 0x60, 0xff));
            }
        }
        for k in (1..=onion.after).rev() {
            if frame + k < self.onion_length {
                ghost(frame + k, k, onion.after, Color::rgb(0x20, 0xb0, 0x40));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn element(
        &self,
        shown: &Shown,
        layer: LayerId,
        frame: u32,
        parent: Matrix,
        parent_ct: &ColorTransform,
        depth: usize,
        path: &mut InstancePath,
        excl: Option<usize>,
        r: &mut dyn Renderer,
    ) {
        let overridden;
        let el: &Element = if self.clock.has_overrides() {
            path.push((layer.0, shown.element.track()));
            let ov = self.clock.element_override(path);
            path.pop();
            match ov {
                Some(o) if o.visible == Some(false) => return,
                Some(o) => {
                    overridden = o.apply(&shown.element);
                    &overridden
                }
                None => &shown.element,
            }
        } else {
            &shown.element
        };
        // Edit-in-place context: skip the edited instance, follow its path.
        let child_excl = match excl {
            Some(k) if self.exclude.get(k) == Some(&el.id) => {
                if k + 1 == self.exclude.len() {
                    return;
                }
                Some(k + 1)
            }
            _ => None,
        };
        let m = parent * el.transform.matrix();
        let grouped = el.blend != BlendMode::Normal;
        // A group applies the element's opacity when compositing, so content
        // inside it only gets the tint.
        let ct = el.color_transform(!grouped).then(parent_ct);
        if grouped {
            r.begin_group(el.blend, el.opacity);
        }
        match &el.kind {
            ElementKind::Shape(shape) => {
                let path = shape.geometry.to_path();
                if let (Some(fill), true) = (&shape.fill, shape.geometry.is_fillable()) {
                    let fill = fill.transformed(&ct);
                    if !fill.is_invisible() {
                        r.fill_path(&path, &m, &fill, shape.fill_rule);
                    }
                }
                if let Some(stroke) = &shape.stroke {
                    let paint = stroke.paint.transformed(&ct);
                    if !paint.is_invisible() && stroke.width > 0.0 {
                        r.stroke_path(&path, &m, &Stroke { paint, ..stroke.clone() });
                    }
                }
            }
            ElementKind::Bitmap { asset } => {
                if let Some(crate::asset::AssetKind::Image { width, height, .. }) = self.project.asset(*asset).map(|a| &a.kind) {
                    r.draw_image(*asset, *width as f64, *height as f64, &m, &ct);
                }
            }
            ElementKind::Text(t) => {
                if let Some(layout) = self.project.layout_text(t) {
                    let fill = t.fill.transformed(&ct);
                    if !fill.is_invisible() && !layout.path.cmds.is_empty() {
                        r.fill_path(&layout.path, &m, &fill, FillRule::NonZero);
                    }
                }
            }
            ElementKind::Instance { symbol, .. } => {
                if let Some(child) = self.project.symbol(*symbol) {
                    path.push((layer.0, el.track()));
                    let stateless = instance_frame(child, &el.kind, shown, frame);
                    let f = self.clock.frame(path, child.kind, stateless).min(child.length() - 1);
                    self.symbol(*symbol, f, m, &ct, depth + 1, path, child_excl, r);
                    path.pop();
                }
            }
        }
        if grouped {
            r.end_group();
        }
    }
}

/// One recorded renderer call.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawOp {
    Begin(FrameInfo),
    Fill { path: Path, transform: Matrix, paint: Paint, rule: FillRule },
    Stroke { path: Path, transform: Matrix, stroke: Stroke },
    Image { asset: AssetId, width: f64, height: f64, transform: Matrix, color: ColorTransform },
    BeginGroup { blend: BlendMode, alpha: f64 },
    EndGroup,
    End,
}

/// Headless renderer that records calls. Used for tests, determinism checks
/// and export verification.
#[derive(Debug, Default)]
pub struct RecordingRenderer {
    pub ops: Vec<DrawOp>,
}

impl Renderer for RecordingRenderer {
    fn begin_frame(&mut self, frame: &FrameInfo) {
        self.ops.push(DrawOp::Begin(frame.clone()));
    }
    fn fill_path(&mut self, path: &Path, transform: &Matrix, paint: &Paint, rule: FillRule) {
        self.ops.push(DrawOp::Fill { path: path.clone(), transform: *transform, paint: paint.clone(), rule });
    }
    fn stroke_path(&mut self, path: &Path, transform: &Matrix, stroke: &Stroke) {
        self.ops.push(DrawOp::Stroke { path: path.clone(), transform: *transform, stroke: stroke.clone() });
    }
    fn draw_image(&mut self, asset: AssetId, width: f64, height: f64, transform: &Matrix, color: &ColorTransform) {
        self.ops.push(DrawOp::Image { asset, width, height, transform: *transform, color: *color });
    }
    fn begin_group(&mut self, blend: BlendMode, alpha: f64) {
        self.ops.push(DrawOp::BeginGroup { blend, alpha });
    }
    fn end_group(&mut self) {
        self.ops.push(DrawOp::EndGroup);
    }
    fn end_frame(&mut self) {
        self.ops.push(DrawOp::End);
    }
}

/// A stable 64-bit fingerprint of a recorded frame (FNV-1a over the calls'
/// exact values), for comparing renderings across engines: the editor
/// preview and an exported player of the same project must agree.
pub fn digest(ops: &[DrawOp]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    // Debug formatting prints f64s in shortest round-trip form: exact and stable.
    for b in format!("{ops:?}").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Renderer that does nothing; isolates traversal cost when profiling.
pub struct NullRenderer;

impl Renderer for NullRenderer {
    fn begin_frame(&mut self, _: &FrameInfo) {}
    fn fill_path(&mut self, _: &Path, _: &Matrix, _: &Paint, _: FillRule) {}
    fn stroke_path(&mut self, _: &Path, _: &Matrix, _: &Stroke) {}
    fn draw_image(&mut self, _: AssetId, _: f64, _: f64, _: &Matrix, _: &ColorTransform) {}
    fn begin_group(&mut self, _: BlendMode, _: f64) {}
    fn end_group(&mut self) {}
    fn end_frame(&mut self) {}
}
