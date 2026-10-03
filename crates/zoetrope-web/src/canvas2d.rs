//! Canvas2D implementation of the core `Renderer` trait.
//!
//! Groups (non-normal blend modes) render into pooled offscreen canvases the
//! size of the target, then composite with `globalCompositeOperation`.
//! Image color transforms are tint + alpha (see `render.rs`): tint is applied
//! via a `source-atop` fill on a scratch canvas, alpha via `globalAlpha`.
//!
//! Performance (Phase 9): paths are built once as `Path2D` objects and
//! reused across draws and frames (keyed by their exact geometry), and
//! fill/stroke state is only sent when it changes. Each avoided call is a
//! WASM→JS crossing; together they cut the renderer's own overhead by about
//! half on heavy scenes (see docs/ARCHITECTURE.md, "Performance").

use std::collections::HashMap;
use wasm_bindgen::JsCast;
use web_sys::{CanvasGradient, CanvasRenderingContext2d, CanvasWindingRule, HtmlCanvasElement, ImageBitmap, Path2d};
use zoetrope_core::color::Color;
use zoetrope_core::geom::{Path, PathCmd};
use zoetrope_core::render::{FrameInfo, Renderer};
use zoetrope_core::{AssetId, BlendMode, ColorTransform, FillRule, LineCap, LineJoin, Matrix, Paint, Stroke};

/// A resolved Canvas2D paint.
enum Style {
    Color(String),
    Gradient(CanvasGradient),
}

/// Builds the Canvas2D style for a paint. Gradient geometry is interpreted
/// in the current transform's space, so call after `set_transform`.
fn canvas_style(ctx: &CanvasRenderingContext2d, paint: &Paint) -> Style {
    let gradient = match paint {
        Paint::Solid { color } => return Style::Color(color.to_css()),
        Paint::Linear { start, end, .. } => {
            if (start.x - end.x).abs() < 1e-9 && (start.y - end.y).abs() < 1e-9 {
                // Degenerate: Canvas would paint nothing; use the last stop.
                return Style::Color(paint.stops().last().map_or_else(String::new, |s| s.color.to_css()));
            }
            ctx.create_linear_gradient(start.x, start.y, end.x, end.y)
        }
        Paint::Radial { center, radius, focal, .. } => {
            let f = focal.unwrap_or(*center);
            match ctx.create_radial_gradient(f.x, f.y, 0.0, center.x, center.y, *radius) {
                Ok(g) => g,
                Err(_) => return Style::Color(paint.primary_color().to_css()),
            }
        }
    };
    for stop in paint.stops() {
        gradient.add_color_stop(stop.offset as f32, &stop.color.to_css()).ok();
    }
    Style::Gradient(gradient)
}

/// Renderer resources kept across frames: reusable offscreen canvases and
/// built `Path2D`s.
#[derive(Default)]
pub struct CanvasPool {
    free: Vec<HtmlCanvasElement>,
    paths: HashMap<u64, Path2d>,
}

/// Most distinct paths kept; the cache is simply emptied when full.
const PATH_CACHE_LIMIT: usize = 16384;

/// Exact fingerprint of a path's commands (FNV-1a over the coordinate bits).
fn path_key(path: &Path) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    for cmd in &path.cmds {
        match cmd {
            PathCmd::MoveTo(p) => [1, p.x.to_bits(), p.y.to_bits()].into_iter().for_each(&mut mix),
            PathCmd::LineTo(p) => [2, p.x.to_bits(), p.y.to_bits()].into_iter().for_each(&mut mix),
            PathCmd::QuadTo(c, p) => {
                [3, c.x.to_bits(), c.y.to_bits(), p.x.to_bits(), p.y.to_bits()].into_iter().for_each(&mut mix)
            }
            PathCmd::CubicTo(a, b, p) => {
                [4, a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits(), p.x.to_bits(), p.y.to_bits()]
                    .into_iter()
                    .for_each(&mut mix)
            }
            PathCmd::Close => mix(5),
        }
    }
    h
}

impl CanvasPool {
    /// The `Path2D` for `path`, built on first use.
    fn path(&mut self, path: &Path) -> Option<Path2d> {
        let key = path_key(path);
        if let Some(p) = self.paths.get(&key) {
            return Some(p.clone());
        }
        let p = Path2d::new().ok()?;
        for cmd in &path.cmds {
            match cmd {
                PathCmd::MoveTo(q) => p.move_to(q.x, q.y),
                PathCmd::LineTo(q) => p.line_to(q.x, q.y),
                PathCmd::QuadTo(c, q) => p.quadratic_curve_to(c.x, c.y, q.x, q.y),
                PathCmd::CubicTo(c1, c2, q) => p.bezier_curve_to(c1.x, c1.y, c2.x, c2.y, q.x, q.y),
                PathCmd::Close => p.close_path(),
            }
        }
        if self.paths.len() >= PATH_CACHE_LIMIT {
            self.paths.clear();
        }
        self.paths.insert(key, p.clone());
        Some(p)
    }
}

/// Context state already sent to Canvas2D, so unchanged values aren't resent.
#[derive(Default, Clone, PartialEq)]
struct SentState {
    fill: Option<Color>,
    stroke: Option<Color>,
    width: Option<u64>,
    cap: Option<LineCap>,
    join: Option<LineJoin>,
    miter: Option<u64>,
    /// Whether a non-empty dash pattern is set.
    dashed: bool,
}

impl CanvasPool {
    fn take(&mut self, w: u32, h: u32) -> Option<(HtmlCanvasElement, CanvasRenderingContext2d)> {
        let canvas = match self.free.pop() {
            Some(c) => c,
            None => web_sys::window()?.document()?.create_element("canvas").ok()?.dyn_into().ok()?,
        };
        if canvas.width() != w || canvas.height() != h {
            canvas.set_width(w);
            canvas.set_height(h);
        }
        let ctx: CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
        ctx.reset_transform().ok();
        ctx.set_global_alpha(1.0);
        ctx.set_global_composite_operation("source-over").ok();
        ctx.clear_rect(0.0, 0.0, w as f64, h as f64);
        Some((canvas, ctx))
    }

    fn give(&mut self, c: HtmlCanvasElement) {
        self.free.push(c);
    }
}

struct Group {
    canvas: HtmlCanvasElement,
    blend: BlendMode,
    alpha: f64,
}

pub struct Canvas2dRenderer<'a> {
    /// `stack[0]` is the target; each open group pushes its offscreen context.
    stack: Vec<CanvasRenderingContext2d>,
    /// What each context in `stack` already has set.
    sent: Vec<SentState>,
    /// One entry per open group (`None` if it could not get an offscreen
    /// canvas and draws straight into its parent).
    groups: Vec<Option<Group>>,
    pool: &'a mut CanvasPool,
    images: &'a HashMap<AssetId, ImageBitmap>,
    clipped: bool,
}

fn composite_op(b: BlendMode) -> &'static str {
    match b {
        BlendMode::Normal | BlendMode::Layer => "source-over",
        BlendMode::Multiply => "multiply",
        BlendMode::Screen => "screen",
        BlendMode::Overlay => "overlay",
        BlendMode::Darken => "darken",
        BlendMode::Lighten => "lighten",
        BlendMode::HardLight => "hard-light",
        BlendMode::Difference => "difference",
        BlendMode::Add => "lighter",
    }
}

impl<'a> Canvas2dRenderer<'a> {
    pub fn new(ctx: &CanvasRenderingContext2d, pool: &'a mut CanvasPool, images: &'a HashMap<AssetId, ImageBitmap>) -> Self {
        Canvas2dRenderer {
            stack: vec![ctx.clone()],
            sent: vec![SentState::default()],
            groups: Vec::new(),
            pool,
            images,
            clipped: false,
        }
    }

    fn ctx(&self) -> &CanvasRenderingContext2d {
        self.stack.last().expect("target context")
    }

    fn target_size(&self) -> (u32, u32) {
        self.stack[0].canvas().map_or((1, 1), |c| (c.width().max(1), c.height().max(1)))
    }

    fn set_transform(&self, m: &Matrix) {
        // Only fails for non-finite values, which the model never produces.
        let _ = self.ctx().set_transform(m.a, m.b, m.c, m.d, m.e, m.f);
    }

    fn sent(&mut self) -> &mut SentState {
        self.sent.last_mut().expect("target state")
    }

    fn placeholder(&self, w: f64, h: f64) {
        let ctx = self.ctx();
        ctx.set_fill_style_str("rgba(160,160,170,0.35)");
        ctx.fill_rect(-w / 2.0, -h / 2.0, w, h);
        ctx.set_stroke_style_str("rgba(120,120,130,0.8)");
        ctx.set_line_width(1.0);
        ctx.begin_path();
        ctx.rect(-w / 2.0, -h / 2.0, w, h);
        ctx.move_to(-w / 2.0, -h / 2.0);
        ctx.line_to(w / 2.0, h / 2.0);
        ctx.move_to(w / 2.0, -h / 2.0);
        ctx.line_to(-w / 2.0, h / 2.0);
        ctx.stroke();
    }
}

impl Renderer for Canvas2dRenderer<'_> {
    fn begin_frame(&mut self, frame: &FrameInfo) {
        let ctx = self.ctx().clone();
        ctx.reset_transform().ok();
        ctx.set_global_alpha(1.0);
        ctx.set_global_composite_operation("source-over").ok();
        let (w, h) = self.target_size();
        ctx.clear_rect(0.0, 0.0, w as f64, h as f64);
        self.set_transform(&frame.view);
        ctx.set_fill_style_str(&frame.background.to_css());
        ctx.fill_rect(0.0, 0.0, frame.stage_width, frame.stage_height);
        // Styles may have been changed by other code since the last frame.
        *self.sent() = SentState { fill: Some(frame.background), ..Default::default() };
        if frame.clip_to_stage {
            ctx.save();
            ctx.begin_path();
            ctx.rect(0.0, 0.0, frame.stage_width, frame.stage_height);
            ctx.clip();
            self.clipped = true;
        }
    }

    fn fill_path(&mut self, path: &Path, transform: &Matrix, paint: &Paint, rule: FillRule) {
        let Some(p2d) = self.pool.path(path) else { return };
        self.set_transform(transform);
        match paint {
            Paint::Solid { color } => {
                if self.sent().fill != Some(*color) {
                    self.ctx().set_fill_style_str(&color.to_css());
                    self.sent().fill = Some(*color);
                }
            }
            _ => {
                match canvas_style(self.ctx(), paint) {
                    Style::Gradient(g) => self.ctx().set_fill_style_canvas_gradient(&g),
                    Style::Color(c) => self.ctx().set_fill_style_str(&c),
                }
                self.sent().fill = None;
            }
        }
        let ctx = self.ctx();
        match rule {
            FillRule::NonZero => ctx.fill_with_path_2d(&p2d),
            FillRule::EvenOdd => ctx.fill_with_path_2d_and_winding(&p2d, CanvasWindingRule::Evenodd),
        }
    }

    fn stroke_path(&mut self, path: &Path, transform: &Matrix, stroke: &Stroke) {
        let Some(p2d) = self.pool.path(path) else { return };
        self.set_transform(transform);
        match &stroke.paint {
            Paint::Solid { color } => {
                if self.sent().stroke != Some(*color) {
                    self.ctx().set_stroke_style_str(&color.to_css());
                    self.sent().stroke = Some(*color);
                }
            }
            paint => {
                match canvas_style(self.ctx(), paint) {
                    Style::Gradient(g) => self.ctx().set_stroke_style_canvas_gradient(&g),
                    Style::Color(c) => self.ctx().set_stroke_style_str(&c),
                }
                self.sent().stroke = None;
            }
        }
        let ctx = self.ctx().clone();
        let sent = self.sent();
        if sent.width != Some(stroke.width.to_bits()) {
            ctx.set_line_width(stroke.width);
            sent.width = Some(stroke.width.to_bits());
        }
        if sent.cap != Some(stroke.cap) {
            ctx.set_line_cap(match stroke.cap {
                LineCap::Butt => "butt",
                LineCap::Round => "round",
                LineCap::Square => "square",
            });
            sent.cap = Some(stroke.cap);
        }
        if sent.join != Some(stroke.join) {
            ctx.set_line_join(match stroke.join {
                LineJoin::Miter => "miter",
                LineJoin::Round => "round",
                LineJoin::Bevel => "bevel",
            });
            sent.join = Some(stroke.join);
        }
        if sent.miter != Some(stroke.miter_limit.to_bits()) {
            ctx.set_miter_limit(stroke.miter_limit);
            sent.miter = Some(stroke.miter_limit.to_bits());
        }
        if !stroke.dash.is_empty() {
            let dash: js_sys::Array = stroke.dash.iter().map(|d| wasm_bindgen::JsValue::from_f64(*d)).collect();
            ctx.set_line_dash(&dash).ok();
            ctx.set_line_dash_offset(stroke.dash_offset);
        } else if sent.dashed {
            ctx.set_line_dash(&js_sys::Array::new()).ok();
        }
        sent.dashed = !stroke.dash.is_empty();
        ctx.stroke_with_path(&p2d);
    }

    fn draw_image(&mut self, asset: AssetId, width: f64, height: f64, transform: &Matrix, color: &ColorTransform) {
        self.set_transform(transform);
        let Some(bitmap) = self.images.get(&asset) else {
            self.placeholder(width, height);
            *self.sent() = SentState::default();
            return;
        };
        let alpha = color.mul[3].clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }
        let amount = (1.0 - color.mul[0]).clamp(0.0, 1.0);
        let ctx = self.ctx().clone();
        ctx.set_global_alpha(alpha);
        if amount > 1e-3 {
            let (w, h) = (bitmap.width(), bitmap.height());
            if let Some((scratch, sctx)) = self.pool.take(w, h) {
                let ch = |i: usize| (color.add[i] / amount).round().clamp(0.0, 255.0);
                sctx.draw_image_with_image_bitmap(bitmap, 0.0, 0.0).ok();
                sctx.set_global_composite_operation("source-atop").ok();
                sctx.set_global_alpha(amount);
                sctx.set_fill_style_str(&format!("rgb({},{},{})", ch(0), ch(1), ch(2)));
                sctx.fill_rect(0.0, 0.0, w as f64, h as f64);
                ctx.draw_image_with_html_canvas_element_and_dw_and_dh(&scratch, -width / 2.0, -height / 2.0, width, height).ok();
                self.pool.give(scratch);
            }
        } else {
            ctx.draw_image_with_image_bitmap_and_dw_and_dh(bitmap, -width / 2.0, -height / 2.0, width, height).ok();
        }
        ctx.set_global_alpha(1.0);
    }

    fn begin_group(&mut self, blend: BlendMode, alpha: f64) {
        let (w, h) = self.target_size();
        match self.pool.take(w, h) {
            Some((canvas, ctx)) => {
                self.stack.push(ctx);
                self.sent.push(SentState::default());
                self.groups.push(Some(Group { canvas, blend, alpha }));
            }
            // No DOM (shouldn't happen in a browser): draw ungrouped.
            None => {
                self.stack.push(self.ctx().clone());
                self.sent.push(self.sent.last().cloned().unwrap_or_default());
                self.groups.push(None);
            }
        }
    }

    fn end_group(&mut self) {
        let Some(group) = self.groups.pop() else { return };
        self.stack.pop();
        let inner = self.sent.pop().unwrap_or_default();
        if group.is_none() {
            // Drew into the parent directly: its state is the inner one.
            *self.sent() = inner;
        }
        if let Some(g) = group {
            let parent = self.ctx().clone();
            parent.save();
            parent.reset_transform().ok();
            parent.set_global_alpha(g.alpha.clamp(0.0, 1.0));
            parent.set_global_composite_operation(composite_op(g.blend)).ok();
            parent.draw_image_with_html_canvas_element(&g.canvas, 0.0, 0.0).ok();
            parent.restore();
            self.pool.give(g.canvas);
        }
    }

    fn end_frame(&mut self) {
        while !self.groups.is_empty() {
            self.end_group();
        }
        let ctx = self.ctx().clone();
        if self.clipped {
            ctx.restore();
            self.clipped = false;
        }
        ctx.reset_transform().ok();
    }
}
