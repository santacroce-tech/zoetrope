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
}

impl RenderOptions {
    pub fn player(view: Matrix) -> Self {
        RenderOptions { view, clip_to_stage: true, show_guides: false }
    }
}

/// Renders frame `frame` of the project. Deterministic: the same project,
/// frame and options always produce the same sequence of renderer calls.
///
/// Phase 2 note: the model has no timelines yet, so `frame` does not affect
/// output. It is part of the signature so callers already pass a clock.
pub fn render_frame(project: &Project, frame: u32, opts: RenderOptions, r: &mut dyn Renderer) {
    let _ = frame;
    r.begin_frame(&FrameInfo {
        stage_width: project.stage.width,
        stage_height: project.stage.height,
        background: project.stage.background,
        view: opts.view,
        clip_to_stage: opts.clip_to_stage,
    });
    let ctx = Ctx { project, opts };
    ctx.symbol(project.root, opts.view, &ColorTransform::IDENTITY, 0, r);
    r.end_frame();
}

struct Ctx<'a> {
    project: &'a Project,
    opts: RenderOptions,
}

impl Ctx<'_> {
    fn symbol(&self, id: SymbolId, m: Matrix, ct: &ColorTransform, depth: usize, r: &mut dyn Renderer) {
        if depth > MAX_NESTING_DEPTH {
            return;
        }
        let Some(symbol) = self.project.symbol(id) else { return };
        self.layers(&symbol.layers, m, ct, depth, r);
    }

    fn layers(&self, layers: &[Layer], m: Matrix, ct: &ColorTransform, depth: usize, r: &mut dyn Renderer) {
        for layer in layers.iter().filter(|l| l.visible) {
            match layer.kind {
                LayerKind::Folder => self.layers(&layer.children, m, ct, depth, r),
                LayerKind::Guide if !self.opts.show_guides => {}
                LayerKind::Guide | LayerKind::Normal => {
                    for el in &layer.elements {
                        self.element(el, m, ct, depth, r);
                    }
                }
            }
        }
    }

    fn element(&self, el: &Element, parent: Matrix, parent_ct: &ColorTransform, depth: usize, r: &mut dyn Renderer) {
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
                if let Some(a) = self.project.asset(*asset) {
                    let crate::asset::AssetKind::Image { width, height, .. } = a.kind;
                    r.draw_image(*asset, width as f64, height as f64, &m, &ct);
                }
            }
            ElementKind::Instance { symbol } => self.symbol(*symbol, m, &ct, depth + 1, r),
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
