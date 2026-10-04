//! Direct-manipulation math: transform sessions (move / scale / rotate /
//! skew / pivot drags), snapping, and shape-tool drags. The editor forwards
//! raw pointer positions (stage coordinates) and modifier keys; everything
//! that decides the resulting geometry lives here.
//!
//! A session previews by writing transforms straight into the project, then
//! `commit` restores the initial state and replays the final transforms as a
//! single undoable transaction (so a whole drag is one undo step).

use crate::edit::Edit;
use crate::error::{Error, Result};
use crate::geom::{Polyline, Rect};
use crate::history::Document;
use crate::math::{Matrix, Point};
use crate::model::*;
use crate::query::{content_bounds, selectable, selection_frame, Scope};
use crate::vector::{HandleSide, NodeRef};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Handle {
    N,
    S,
    E,
    W,
    Ne,
    Nw,
    Se,
    Sw,
}

impl Handle {
    /// Which box edges the handle moves: (x side, y side), -1 = min, 1 = max.
    fn sides(self) -> (i8, i8) {
        match self {
            Handle::N => (0, -1),
            Handle::S => (0, 1),
            Handle::E => (1, 0),
            Handle::W => (-1, 0),
            Handle::Ne => (1, -1),
            Handle::Nw => (-1, -1),
            Handle::Se => (1, 1),
            Handle::Sw => (-1, 1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum DragMode {
    Move,
    Scale {
        handle: Handle,
    },
    Rotate,
    /// `handle` must be an edge (N/S shear horizontally, E/W vertically).
    Skew {
        handle: Handle,
    },
    /// Moves a single element's pivot without moving the element.
    Pivot,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Modifiers {
    /// Constrain: axis-lock moves, proportional scale, 15° rotation steps,
    /// 45° lines, square/circle shapes.
    pub shift: bool,
    /// From center: scale about the box center; draw shapes from center.
    pub alt: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapConfig {
    /// Grid spacing in stage units, if snapping to grid.
    pub grid: Option<f64>,
    /// Snap to other objects' edges/centers and the stage's.
    pub objects: bool,
    /// Maximum snap distance in stage units.
    pub tolerance: f64,
}

/// Snap guide lines to display (stage coordinates).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Guides {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
}

/// Candidate snap lines gathered once per drag.
#[derive(Debug, Clone, Default)]
pub struct SnapTargets {
    xs: Vec<f64>,
    ys: Vec<f64>,
}

impl SnapTargets {
    /// Stage edges/center plus edges/centers of selectable elements not in `exclude`.
    pub fn collect(p: &Project, scope: &Scope, exclude: &[ElementId]) -> SnapTargets {
        let (w, h) = (p.stage.width, p.stage.height);
        let mut t = SnapTargets { xs: vec![0.0, w / 2.0, w], ys: vec![0.0, h / 2.0, h] };
        for e in selectable(p, scope) {
            if exclude.contains(&e.id) {
                continue;
            }
            if let Some(b) = content_bounds(p, &e.kind, &(scope.matrix * e.transform.matrix()), scope.frame, 0) {
                let c = b.center();
                t.xs.extend([b.min.x, c.x, b.max.x]);
                t.ys.extend([b.min.y, c.y, b.max.y]);
            }
        }
        t
    }
}

/// Best correction moving any of `edges` onto a target line or grid line.
/// Returns (correction, guide line if it came from an object/stage target).
fn snap_axis(edges: &[f64], targets: &[f64], cfg: &SnapConfig) -> Option<(f64, Option<f64>)> {
    let mut best: Option<(f64, Option<f64>)> = None;
    let mut consider = |delta: f64, guide: Option<f64>| {
        if delta.abs() <= cfg.tolerance && best.is_none_or(|(b, _)| delta.abs() < b.abs()) {
            best = Some((delta, guide));
        }
    };
    if cfg.objects {
        for &e in edges {
            for &t in targets {
                consider(t - e, Some(t));
            }
        }
    }
    if let Some(g) = cfg.grid.filter(|g| *g > 0.0) {
        for &e in edges {
            consider((e / g).round() * g - e, None);
        }
    }
    best
}

/// Snaps a rect's edges/center; returns the (dx, dy) correction and guides.
pub fn snap_rect(r: &Rect, targets: &SnapTargets, cfg: &SnapConfig) -> (f64, f64, Guides) {
    let c = r.center();
    let mut guides = Guides::default();
    let (mut dx, mut dy) = (0.0, 0.0);
    if let Some((d, g)) = snap_axis(&[r.min.x, c.x, r.max.x], &targets.xs, cfg) {
        dx = d;
        guides.x.extend(g);
    }
    if let Some((d, g)) = snap_axis(&[r.min.y, c.y, r.max.y], &targets.ys, cfg) {
        dy = d;
        guides.y.extend(g);
    }
    (dx, dy, guides)
}

pub fn snap_point(pt: Point, targets: &SnapTargets, cfg: &SnapConfig) -> (Point, Guides) {
    let (dx, dy, g) = snap_rect(&Rect { min: pt, max: pt }, targets, cfg);
    (Point::new(pt.x + dx, pt.y + dy), g)
}

pub struct TransformSession {
    scope: Scope,
    ids: Vec<ElementId>,
    initial: Vec<Transform>,
    mode: DragMode,
    /// Handle box: box space → stage, and its extent in box space.
    frame: Matrix,
    bbox: Rect,
    /// Rotation center (stage).
    center: Point,
    start: Point,
    /// Stage bounds of the selection at drag start (for move snapping).
    start_bounds: Option<Rect>,
    targets: SnapTargets,
}

impl TransformSession {
    pub fn begin(p: &Project, scope: Scope, ids: &[ElementId], mode: DragMode, start: Point) -> Result<TransformSession> {
        if ids.is_empty() {
            return Err(Error::Invalid("nothing selected".into()));
        }
        for id in ids {
            check_editable(p, &scope, *id)?;
        }
        if mode == DragMode::Pivot && ids.len() != 1 {
            return Err(Error::Invalid("the pivot can only be moved for a single element".into()));
        }
        let initial: Vec<Transform> = ids.iter().map(|id| p.element(*id).unwrap().transform).collect();
        let (frame, bbox) = selection_frame(p, &scope, ids).unwrap_or((Matrix::IDENTITY, Rect { min: start, max: start }));
        let center = match ids {
            [_] => scope.matrix.apply(Point::new(initial[0].x, initial[0].y)),
            _ => frame.apply(bbox.center()),
        };
        let start_bounds = Rect::union_all(ids.iter().filter_map(|id| {
            let e = p.element(*id)?;
            content_bounds(p, &e.kind, &(scope.matrix * e.transform.matrix()), scope.frame, 0)
        }));
        let targets = SnapTargets::collect(p, &scope, ids);
        Ok(TransformSession { scope, ids: ids.to_vec(), initial, mode, frame, bbox, center, start, start_bounds, targets })
    }

    pub fn ids(&self) -> &[ElementId] {
        &self.ids
    }

    /// Computes transforms for pointer position `pt` and previews them.
    pub fn update(&self, p: &mut Project, pt: Point, mods: Modifiers, snap: &SnapConfig) -> Guides {
        let (transforms, guides) = self.compute(pt, mods, snap);
        for (id, t) in self.ids.iter().zip(transforms) {
            if let Some(e) = p.element_mut(*id) {
                e.transform = t;
            }
        }
        guides
    }

    /// Restores the initial transforms and records the final ones as one
    /// undoable step (no-op if nothing changed).
    pub fn commit(self, doc: &mut Document) -> Result<()> {
        let mut edits = Vec::new();
        for (id, t0) in self.ids.iter().zip(&self.initial) {
            let Some(e) = doc.project.element_mut(*id) else {
                continue;
            };
            let t1 = std::mem::replace(&mut e.transform, *t0);
            if t1 != *t0 {
                edits.push(Edit::SetTransform { element: *id, transform: t1 });
            }
        }
        let label = match self.mode {
            DragMode::Move => "Move",
            DragMode::Scale { .. } => "Scale",
            DragMode::Rotate => "Rotate",
            DragMode::Skew { .. } => "Skew",
            DragMode::Pivot => "Move Pivot",
        };
        doc.execute(label, edits)
    }

    pub fn cancel(self, p: &mut Project) {
        for (id, t0) in self.ids.iter().zip(&self.initial) {
            if let Some(e) = p.element_mut(*id) {
                e.transform = *t0;
            }
        }
    }

    fn compute(&self, pt: Point, mods: Modifiers, snap: &SnapConfig) -> (Vec<Transform>, Guides) {
        let mut guides = Guides::default();
        let scope_inv = self.scope.matrix.invert().unwrap_or(Matrix::IDENTITY);
        let out = match self.mode {
            DragMode::Move => {
                let (mut dx, mut dy) = (pt.x - self.start.x, pt.y - self.start.y);
                if mods.shift {
                    if dx.abs() > dy.abs() {
                        dy = 0.0
                    } else {
                        dx = 0.0
                    }
                }
                if let Some(b) = self.start_bounds {
                    let (sx, sy, g) = snap_rect(&b.translate(dx, dy), &self.targets, snap);
                    // Don't let snapping break an axis lock.
                    if !(mods.shift && dx == 0.0) {
                        dx += sx;
                    }
                    if !(mods.shift && dy == 0.0) {
                        dy += sy;
                    }
                    guides = g;
                }
                let d = scope_inv.linear().apply(Point::new(dx, dy));
                self.initial.iter().map(|t| Transform { x: t.x + d.x, y: t.y + d.y, ..*t }).collect()
            }
            DragMode::Pivot => {
                let t = self.initial[0];
                let elem_to_stage = self.scope.matrix * t.matrix();
                // Snap to the box's corners, edge midpoints and center.
                let (b, frame) = (self.bbox, self.frame);
                let mut q = pt;
                let mut best = snap.tolerance;
                for fx in [0.0, 0.5, 1.0] {
                    for fy in [0.0, 0.5, 1.0] {
                        let cand = frame.apply(Point::new(b.min.x + fx * b.width(), b.min.y + fy * b.height()));
                        let d = (cand.x - pt.x).hypot(cand.y - pt.y);
                        if d <= best {
                            best = d;
                            q = cand;
                        }
                    }
                }
                let local = elem_to_stage.invert().map_or(t.pivot(), |inv| inv.apply(q));
                vec![t.with_pivot(local)]
            }
            DragMode::Rotate => {
                let c = self.center;
                let a0 = (self.start.y - c.y).atan2(self.start.x - c.x);
                let a1 = (pt.y - c.y).atan2(pt.x - c.x);
                let mut angle = (a1 - a0).to_degrees();
                if mods.shift {
                    let base = if self.ids.len() == 1 { self.initial[0].rotation } else { 0.0 };
                    angle = ((base + angle) / 15.0).round() * 15.0 - base;
                }
                let g = Matrix::translate(c.x, c.y) * Matrix::rotate(angle) * Matrix::translate(-c.x, -c.y);
                let local_g = scope_inv * g * self.scope.matrix;
                self.initial.iter().map(|t| t.with_matrix(&(local_g * t.matrix()), t.rotation + angle)).collect()
            }
            DragMode::Scale { handle } => self.box_op(self.scale_op(handle, pt, mods), &scope_inv),
            DragMode::Skew { handle } => self.box_op(self.skew_op(handle, pt, mods), &scope_inv),
        };
        (out, guides)
    }

    /// Applies an operation expressed in box space to every element.
    fn box_op(&self, op: Matrix, scope_inv: &Matrix) -> Vec<Transform> {
        if self.ids.len() == 1 {
            // Box space is the element's content space: M' = M · op.
            let t = self.initial[0];
            return vec![t.with_matrix(&(t.matrix() * op), t.rotation)];
        }
        let frame_inv = self.frame.invert().unwrap_or(Matrix::IDENTITY);
        let g = *scope_inv * self.frame * op * frame_inv * self.scope.matrix;
        self.initial.iter().map(|t| t.with_matrix(&(g * t.matrix()), t.rotation)).collect()
    }

    fn box_points(&self, pt: Point) -> (Point, Point) {
        let inv = self.frame.invert().unwrap_or(Matrix::IDENTITY);
        (inv.apply(self.start), inv.apply(pt))
    }

    fn scale_op(&self, handle: Handle, pt: Point, mods: Modifiers) -> Matrix {
        let (p0, p) = self.box_points(pt);
        let b = self.bbox;
        let (sx, sy) = handle.sides();
        let anchor = if mods.alt {
            b.center()
        } else {
            Point::new(if sx > 0 { b.min.x } else { b.max.x }, if sy > 0 { b.min.y } else { b.max.y })
        };
        let factor = |side: i8, from: f64, to: f64, a: f64| -> f64 {
            if side == 0 || (from - a).abs() < 1e-9 {
                return 1.0;
            }
            let f = (to - a) / (from - a);
            if f.abs() < 1e-4 {
                1e-4_f64.copysign(f)
            } else {
                f
            }
        };
        let mut fx = factor(sx, p0.x, p.x, anchor.x);
        let mut fy = factor(sy, p0.y, p.y, anchor.y);
        if mods.shift && sx != 0 && sy != 0 {
            let f = if fx.abs() > fy.abs() { fx } else { fy };
            (fx, fy) = (f, f);
        }
        Matrix::translate(anchor.x, anchor.y) * Matrix::scale(fx, fy) * Matrix::translate(-anchor.x, -anchor.y)
    }

    fn skew_op(&self, handle: Handle, pt: Point, mods: Modifiers) -> Matrix {
        let (p0, p) = self.box_points(pt);
        let b = self.bbox;
        let c = b.center();
        let (sx, sy) = handle.sides();

        if sy != 0 {
            let ay = if mods.alt {
                c.y
            } else if sy > 0 {
                b.min.y
            } else {
                b.max.y
            };
            let span = p0.y - ay;
            let k = if span.abs() < 1e-9 { 0.0 } else { (p.x - p0.x) / span };
            Matrix::translate(0.0, ay) * Matrix { a: 1.0, b: 0.0, c: k, d: 1.0, e: 0.0, f: 0.0 } * Matrix::translate(0.0, -ay)
        } else {
            let ax = if mods.alt {
                c.x
            } else if sx > 0 {
                b.min.x
            } else {
                b.max.x
            };
            let span = p0.x - ax;
            let k = if span.abs() < 1e-9 { 0.0 } else { (p.y - p0.y) / span };
            Matrix::translate(ax, 0.0) * Matrix { a: 1.0, b: k, c: 0.0, d: 1.0, e: 0.0, f: 0.0 } * Matrix::translate(-ax, 0.0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShapeTool {
    Rect,
    Ellipse,
    Line,
    /// Regular polygon or star, drawn from its center.
    Polygon,
}

/// Options for tools with parameters (polygon/star).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShapeOptions {
    pub sides: u32,
    /// Inner/outer radius ratio; `Some` draws a star.
    pub star: Option<f64>,
}

impl Default for ShapeOptions {
    fn default() -> Self {
        ShapeOptions { sides: 5, star: None }
    }
}

/// Geometry + placement (stage coordinates) for a shape-tool drag, or
/// `None` if the drag is too small to make a shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeDrag {
    pub geometry: Geometry,
    pub center: Point,
}

impl ShapeDrag {
    /// Flattened outline in stage coordinates (for live previews).
    pub fn outline(&self) -> Vec<Polyline> {
        self.geometry.to_vector_path().outline(&Matrix::translate(self.center.x, self.center.y))
    }
}

pub fn shape_from_drag(tool: ShapeTool, p0: Point, p1: Point, mods: Modifiers, opts: &ShapeOptions) -> Option<ShapeDrag> {
    let (mut dx, mut dy) = (p1.x - p0.x, p1.y - p0.y);
    match tool {
        ShapeTool::Polygon => {
            let radius = dx.hypot(dy);
            if radius < 1.0 {
                return None;
            }
            let mut rotation = dy.atan2(dx).to_degrees() + 90.0;
            if mods.shift {
                rotation = (rotation / 15.0).round() * 15.0;
            }
            let path = VectorPath::polystar(opts.sides, radius, opts.star, rotation);
            Some(ShapeDrag { geometry: Geometry::Path(path), center: p0 })
        }
        ShapeTool::Line => {
            if mods.shift {
                let len = dx.hypot(dy);
                let ang = (dy.atan2(dx) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
                (dx, dy) = (len * ang.cos(), len * ang.sin());
                (dx, dy) = (clean(dx), clean(dy));
            }
            if dx.hypot(dy) < 1.0 {
                return None;
            }
            let (center, full) =
                if mods.alt { (p0, (2.0 * dx, 2.0 * dy)) } else { (Point::new(p0.x + dx / 2.0, p0.y + dy / 2.0), (dx, dy)) };
            Some(ShapeDrag { geometry: Geometry::Line { dx: full.0, dy: full.1 }, center })
        }
        ShapeTool::Rect | ShapeTool::Ellipse => {
            if mods.shift {
                let s = dx.abs().max(dy.abs());
                (dx, dy) = (s.copysign(dx), s.copysign(dy));
            }
            let (center, w, h) = if mods.alt {
                (p0, 2.0 * dx.abs(), 2.0 * dy.abs())
            } else {
                (Point::new(p0.x + dx / 2.0, p0.y + dy / 2.0), dx.abs(), dy.abs())
            };
            if w < 1.0 || h < 1.0 {
                return None;
            }
            let geometry = match tool {
                ShapeTool::Rect => Geometry::Rect { width: w, height: h },
                _ => Geometry::Ellipse { width: w, height: h },
            };
            Some(ShapeDrag { geometry, center })
        }
    }
}

fn clean(v: f64) -> f64 {
    if v.abs() < 1e-9 {
        0.0
    } else {
        v
    }
}

// ------------------------------------------------------------------ pen

/// The pen tool's in-progress path (stage coordinates). Click adds a corner
/// anchor; click-drag pulls out symmetric handles (Alt: only the outgoing
/// handle, making a cusp); clicking the first anchor closes the path. Shift
/// constrains new anchors to 45° steps from the previous one.
#[derive(Debug, Clone, Default)]
pub struct PenSession {
    nodes: Vec<Node>,
    dragging: bool,
    closing: bool,
    hover: Option<Point>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PenPreview {
    pub outline: Vec<Polyline>,
    pub anchors: Vec<Point>,
    /// (anchor, handle) pairs to draw as handle lines.
    pub handles: Vec<(Point, Point)>,
    /// The pointer is over the first anchor, so a click would close the path.
    pub can_close: bool,
}

impl PenSession {
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn constrain(&self, p: Point, shift: bool) -> Point {
        match (self.nodes.last(), shift) {
            (Some(last), true) if !self.dragging => {
                let (dx, dy) = (p.x - last.x, p.y - last.y);
                let len = dx.hypot(dy);
                let step = std::f64::consts::FRAC_PI_4;
                let a = (dy.atan2(dx) / step).round() * step;
                Point::new(last.x + clean(len * a.cos()), last.y + clean(len * a.sin()))
            }
            _ => p,
        }
    }

    fn near_first(&self, p: Point, tol: f64) -> bool {
        self.nodes.len() >= 2 && (p.x - self.nodes[0].x).hypot(p.y - self.nodes[0].y) <= tol
    }

    /// Returns true if this click closes the path.
    pub fn pointer_down(&mut self, p: Point, mods: Modifiers, close_tolerance: f64) -> bool {
        if self.near_first(p, close_tolerance) {
            self.closing = true;
            self.dragging = true;
            return true;
        }
        let p = self.constrain(p, mods.shift);
        self.nodes.push(Node::corner(p));
        self.dragging = true;
        false
    }

    pub fn pointer_drag(&mut self, p: Point, mods: Modifiers) {
        if !self.dragging {
            return;
        }
        let idx = if self.closing { 0 } else { self.nodes.len() - 1 };
        let Some(n) = self.nodes.get_mut(idx) else {
            return;
        };
        let a = n.point();
        if (p.x - a.x).hypot(p.y - a.y) < 0.5 {
            return;
        }
        let mirrored = Point::new(2.0 * a.x - p.x, 2.0 * a.y - p.y);
        if self.closing {
            // Dragging on the first anchor shapes the closing segment.
            n.handle_in = Some(mirrored);
            if n.handle_out.is_some() {
                n.handle_out = Some(p);
                n.kind = NodeKind::Symmetric;
            }
        } else {
            n.handle_out = Some(p);
            if mods.alt {
                n.kind = NodeKind::Corner;
            } else {
                n.handle_in = Some(mirrored);
                n.kind = NodeKind::Symmetric;
            }
        }
    }

    pub fn pointer_up(&mut self) {
        self.dragging = false;
    }

    pub fn hover(&mut self, p: Point, mods: Modifiers) {
        self.hover = Some(self.constrain(p, mods.shift));
    }

    pub fn is_closing(&self) -> bool {
        self.closing
    }

    pub fn preview(&self, close_tolerance: f64) -> PenPreview {
        let mut nodes = self.nodes.clone();
        let can_close = self.hover.is_some_and(|h| self.near_first(h, close_tolerance));
        if let (Some(h), false, false) = (self.hover, self.closing, self.dragging) {
            if !can_close {
                nodes.push(Node::corner(h));
            }
        }
        let closed = self.closing || can_close;
        let outline = if nodes.len() >= 2 { VectorPath::single(nodes, closed).outline(&Matrix::IDENTITY) } else { Vec::new() };
        let mut handles = Vec::new();
        for n in self.nodes.iter().rev().take(1).chain(self.nodes.first().filter(|_| self.closing)) {
            for h in [n.handle_in, n.handle_out].into_iter().flatten() {
                handles.push((n.point(), h));
            }
        }
        PenPreview { outline, anchors: self.nodes.iter().map(Node::point).collect(), handles, can_close }
    }

    /// The finished path (stage coordinates), or `None` with fewer than two anchors.
    pub fn finish(self) -> Option<VectorPath> {
        (self.nodes.len() >= 2).then(|| VectorPath::single(self.nodes, self.closing))
    }
}

// ------------------------------------------------------------------ path & gradient editing

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaintPart {
    Fill,
    Stroke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GradientHandle {
    /// Linear start point.
    Start,
    /// Linear end point.
    End,
    /// Radial center (moves the focal point along).
    Center,
    /// Radial radius (handle on the circle).
    Radius,
    /// Radial focal point.
    Focal,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "target", rename_all = "camelCase")]
pub enum EditTarget {
    /// Move anchors (with their handles).
    Anchors { nodes: Vec<NodeRef> },
    /// Move one bezier handle; Alt breaks the tangent.
    Handle { node: NodeRef, side: HandleSide },
    /// Move a gradient control.
    Gradient { part: PaintPart, handle: GradientHandle },
}

/// A drag that edits one shape's path or gradient. Like `TransformSession`,
/// it previews in place and commits as one undo step. Editing anchors of a
/// rectangle/ellipse/line converts it to a path.
pub struct EditSession {
    id: ElementId,
    initial: Element,
    path0: VectorPath,
    target: EditTarget,
    /// Stage → content coordinates.
    to_local: Matrix,
    start_local: Point,
}

fn shape_of(e: &Element) -> Result<&Shape> {
    match &e.kind {
        ElementKind::Shape(s) => Ok(s),
        _ => Err(Error::Invalid("only shapes have editable paths and gradients".into())),
    }
}

fn paint_of(s: &Shape, part: PaintPart) -> Option<&Paint> {
    match part {
        PaintPart::Fill => s.fill.as_ref(),
        PaintPart::Stroke => s.stroke.as_ref().map(|st| &st.paint),
    }
}

/// An element can be directly manipulated if it is in the edited symbol, on
/// an unlocked layer, and shown un-interpolated at the scope's frame.
fn check_editable(p: &Project, scope: &Scope, id: ElementId) -> Result<()> {
    let loc = p.locate(id).ok_or_else(|| Error::NotFound(format!("element {}", id.0)))?;
    if loc.symbol != scope.symbol {
        return Err(Error::Invalid("element is not in the edited symbol".into()));
    }
    if p.layer_state(loc.layer).is_some_and(|(_, locked)| locked) {
        return Err(Error::Invalid("element is on a locked layer".into()));
    }
    let layer = p.require_layer(loc.layer)?;
    if layer.keyframe_at(scope.frame).map(|(i, _)| i) != Some(loc.keyframe) {
        return Err(Error::Invalid("element is not on the current frame".into()));
    }
    if crate::timeline::is_tweened_frame(layer, scope.frame) {
        return Err(Error::Invalid("this frame is tweened: insert a keyframe here (F6) to edit it".into()));
    }
    Ok(())
}

impl EditSession {
    pub fn begin(p: &Project, scope: Scope, id: ElementId, target: EditTarget, start: Point) -> Result<EditSession> {
        check_editable(p, &scope, id)?;
        let e = p.require_element(id)?;
        let shape = shape_of(e)?;
        if let EditTarget::Gradient { part, .. } = &target {
            if !matches!(paint_of(shape, *part), Some(Paint::Linear { .. } | Paint::Radial { .. })) {
                return Err(Error::Invalid("that paint is not a gradient".into()));
            }
        }
        let to_local = (scope.matrix * e.transform.matrix())
            .invert()
            .ok_or_else(|| Error::Invalid("element is collapsed to zero size".into()))?;
        Ok(EditSession {
            id,
            initial: e.clone(),
            path0: shape.geometry.to_vector_path(),
            target,
            to_local,
            start_local: to_local.apply(start),
        })
    }

    pub fn update(&self, p: &mut Project, pt: Point, mods: Modifiers) -> Result<()> {
        let local = self.to_local.apply(pt);
        let ElementKind::Shape(mut shape) = self.initial.kind.clone() else { unreachable!() };
        match &self.target {
            EditTarget::Anchors { nodes } => {
                let (mut dx, mut dy) = (local.x - self.start_local.x, local.y - self.start_local.y);
                if mods.shift {
                    if dx.abs() > dy.abs() {
                        dy = 0.0
                    } else {
                        dx = 0.0
                    }
                }
                let mut path = self.path0.clone();
                path.move_nodes(nodes, Point::new(dx, dy))?;
                shape.geometry = Geometry::Path(path);
            }
            EditTarget::Handle { node, side } => {
                let mut path = self.path0.clone();
                path.set_handle(*node, *side, local, mods.alt)?;
                shape.geometry = Geometry::Path(path);
            }
            EditTarget::Gradient { part, handle } => {
                let paint = match part {
                    PaintPart::Fill => shape.fill.as_mut(),
                    PaintPart::Stroke => shape.stroke.as_mut().map(|s| &mut s.paint),
                };
                let d = Point::new(local.x - self.start_local.x, local.y - self.start_local.y);
                match (paint, handle) {
                    (Some(Paint::Linear { start, .. }), GradientHandle::Start) => *start = local,
                    (Some(Paint::Linear { end, .. }), GradientHandle::End) => *end = local,
                    (Some(Paint::Radial { center, focal, .. }), GradientHandle::Center) => {
                        *center = Point::new(center.x + d.x, center.y + d.y);
                        *focal = focal.map(|f| Point::new(f.x + d.x, f.y + d.y));
                    }
                    (Some(Paint::Radial { center, radius, .. }), GradientHandle::Radius) => {
                        *radius = (local.x - center.x).hypot(local.y - center.y).max(0.5);
                    }
                    (Some(Paint::Radial { center, radius, focal, .. }), GradientHandle::Focal) => {
                        // Keep the focal point inside the circle (Canvas2D requires it).
                        let (fx, fy) = (local.x - center.x, local.y - center.y);
                        let l = fx.hypot(fy);
                        let max = *radius * 0.98;
                        let k = if l > max { max / l } else { 1.0 };
                        *focal = Some(Point::new(center.x + fx * k, center.y + fy * k));
                    }
                    _ => return Err(Error::Invalid("that handle does not apply to this gradient".into())),
                }
            }
        }
        if let Some(e) = p.element_mut(self.id) {
            e.kind = ElementKind::Shape(shape);
        }
        Ok(())
    }

    pub fn commit(self, doc: &mut Document) -> Result<()> {
        let Some(e) = doc.project.element_mut(self.id) else {
            return Ok(());
        };
        let edited = std::mem::replace(e, self.initial.clone());
        if edited == self.initial {
            return Ok(());
        }
        let label = match self.target {
            EditTarget::Gradient { .. } => "Edit Gradient",
            _ => "Edit Path",
        };
        doc.execute(label, vec![Edit::ReplaceElement { element: edited }])
    }

    pub fn cancel(self, p: &mut Project) {
        if let Some(e) = p.element_mut(self.id) {
            *e = self.initial;
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub x: f64,
    pub y: f64,
    #[serde(rename = "in")]
    pub handle_in: Option<Point>,
    #[serde(rename = "out")]
    pub handle_out: Option<Point>,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathInfo {
    /// Anchors and handles in stage coordinates, per subpath.
    pub subpaths: Vec<Vec<NodeInfo>>,
    pub closed: Vec<bool>,
    /// Flattened outline in stage coordinates.
    pub outline: Vec<Polyline>,
    /// True for rectangles/ellipses/lines (editing converts them to paths).
    pub primitive: bool,
}

/// What the subselection tool draws for a shape.
pub fn path_info(p: &Project, scope: &Scope, id: ElementId) -> Option<PathInfo> {
    let e = p.element(id)?;
    let ElementKind::Shape(s) = &e.kind else {
        return None;
    };
    let m = scope.matrix * e.transform.matrix();
    let v = s.geometry.to_vector_path();
    let subpaths = v
        .subpaths
        .iter()
        .map(|sp| {
            sp.nodes
                .iter()
                .map(|n| {
                    let q = m.apply(n.point());
                    NodeInfo {
                        x: q.x,
                        y: q.y,
                        handle_in: n.handle_in.map(|h| m.apply(h)),
                        handle_out: n.handle_out.map(|h| m.apply(h)),
                        kind: n.kind,
                    }
                })
                .collect()
        })
        .collect();
    Some(PathInfo {
        subpaths,
        closed: v.subpaths.iter().map(|sp| sp.closed).collect(),
        outline: v.outline(&m),
        primitive: !matches!(s.geometry, Geometry::Path(_)),
    })
}

/// The point on a shape's outline nearest `pt` (stage), within `tolerance`.
pub fn path_hit(p: &Project, scope: &Scope, id: ElementId, pt: Point, tolerance: f64) -> Option<crate::vector::PathHit> {
    let e = p.element(id)?;
    let ElementKind::Shape(s) = &e.kind else {
        return None;
    };
    let m = scope.matrix * e.transform.matrix();
    let local = m.invert()?.apply(pt);
    let hit = s.geometry.to_vector_path().nearest(local)?;
    (hit.distance * crate::query::scale_factor(&m) <= tolerance).then_some(hit)
}

/// Gradient controls in stage coordinates, for the gradient tool.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GradientControls {
    Linear { start: Point, end: Point },
    Radial { center: Point, radius: Point, focal: Point, ring: Vec<Point> },
}

pub fn gradient_controls(p: &Project, scope: &Scope, id: ElementId, part: PaintPart) -> Option<GradientControls> {
    let e = p.element(id)?;
    let ElementKind::Shape(s) = &e.kind else {
        return None;
    };
    let m = scope.matrix * e.transform.matrix();
    match paint_of(s, part)? {
        Paint::Linear { start, end, .. } => Some(GradientControls::Linear { start: m.apply(*start), end: m.apply(*end) }),
        Paint::Radial { center, radius, focal, .. } => {
            let ring = (0..=48)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / 48.0;
                    m.apply(Point::new(center.x + radius * a.cos(), center.y + radius * a.sin()))
                })
                .collect();
            Some(GradientControls::Radial {
                center: m.apply(*center),
                radius: m.apply(Point::new(center.x + radius, center.y)),
                focal: m.apply(focal.unwrap_or(*center)),
                ring,
            })
        }
        Paint::Solid { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::demo_project;
    use crate::render::{render_frame, RecordingRenderer, RenderOptions};

    fn named(p: &Project, name: &str) -> ElementId {
        selectable(p, &Scope::root(p)).iter().find(|e| e.name == name).unwrap().id
    }

    fn no_snap() -> SnapConfig {
        SnapConfig::default()
    }

    fn render(p: &Project) -> Vec<crate::render::DrawOp> {
        let mut r = RecordingRenderer::default();
        render_frame(
            p,
            0,
            RenderOptions { view: Matrix::IDENTITY, clip_to_stage: true, show_guides: true, onion: None, edit_masks: false },
            &mut r,
        );
        r.ops
    }

    #[test]
    fn move_session_previews_and_commits_as_one_undo_step() {
        let mut doc = Document::new(demo_project());
        let before = render(&doc.project);
        let sun = named(&doc.project, "sun");
        let s = TransformSession::begin(&doc.project, Scope::root(&doc.project), &[sun], DragMode::Move, Point::new(820.0, 90.0))
            .unwrap();
        s.update(&mut doc.project, Point::new(830.0, 95.0), Modifiers::default(), &no_snap());
        s.update(&mut doc.project, Point::new(840.0, 100.0), Modifiers::default(), &no_snap());
        assert_eq!(doc.project.element(sun).unwrap().transform.x, 840.0);
        s.commit(&mut doc).unwrap();
        assert_eq!(doc.undo_label(), Some("Move"));
        assert_eq!(doc.project.element(sun).unwrap().transform.x, 840.0);
        doc.undo().unwrap();
        assert_eq!(render(&doc.project), before);
    }

    #[test]
    fn cancel_restores() {
        let mut doc = Document::new(demo_project());
        let before = doc.project.clone();
        let sun = named(&doc.project, "sun");
        let s =
            TransformSession::begin(&doc.project, Scope::root(&doc.project), &[sun], DragMode::Rotate, Point::new(900.0, 90.0))
                .unwrap();
        s.update(&mut doc.project, Point::new(820.0, 200.0), Modifiers::default(), &no_snap());
        s.cancel(&mut doc.project);
        assert_eq!(doc.project, before);
    }

    #[test]
    fn move_snaps_to_stage_edge() {
        let mut p = demo_project();
        let sun = named(&p, "sun"); // 110 wide at x=820 → right edge 875
        let s = TransformSession::begin(&p, Scope::root(&p), &[sun], DragMode::Move, Point::new(820.0, 90.0)).unwrap();
        let cfg = SnapConfig { grid: None, objects: true, tolerance: 6.0 };
        let g = s.update(&mut p, Point::new(902.0, 90.0), Modifiers::default(), &cfg);
        // Right edge would be 957; the stage edge at 960 is within tolerance.
        assert_eq!(p.element(sun).unwrap().transform.x, 905.0);
        assert!(g.x.contains(&960.0));
    }

    #[test]
    fn corner_scale_keeps_opposite_corner_fixed() {
        let mut p = demo_project();
        let sun = named(&p, "sun");
        {
            let e = p.element_mut(sun).unwrap();
            e.transform.rotation = 30.0;
        }
        let scope = Scope::root(&p);
        let (frame0, b) = selection_frame(&p, &scope, &[sun]).unwrap();
        let fixed = frame0.apply(b.min); // NW corner is the anchor for an SE drag
        let start = frame0.apply(b.max);
        let s = TransformSession::begin(&p, scope, &[sun], DragMode::Scale { handle: Handle::Se }, start).unwrap();
        let target = frame0.apply(Point::new(b.max.x + 55.0, b.max.y + 55.0));
        s.update(&mut p, target, Modifiers::default(), &no_snap());
        let t = p.element(sun).unwrap().transform;
        assert!((t.scale_x - 1.5).abs() < 1e-9 && (t.scale_y - 1.5).abs() < 1e-9);
        assert!((t.rotation - 30.0).abs() < 1e-9, "rotation preserved");
        let (frame1, b1) = selection_frame(&p, &scope, &[sun]).unwrap();
        let now = frame1.apply(b1.min);
        assert!((now.x - fixed.x).abs() < 1e-6 && (now.y - fixed.y).abs() < 1e-6);
    }

    #[test]
    fn rotation_about_pivot_with_shift_snaps() {
        let mut p = demo_project();
        let sun = named(&p, "sun");
        let s = TransformSession::begin(&p, Scope::root(&p), &[sun], DragMode::Rotate, Point::new(920.0, 90.0)).unwrap();
        s.update(&mut p, Point::new(920.0, 125.0), Modifiers { shift: true, alt: false }, &no_snap());
        let t = p.element(sun).unwrap().transform;
        assert_eq!(t.rotation % 15.0, 0.0);
        assert_eq!((t.x, t.y), (820.0, 90.0), "pivot stays put");
    }

    #[test]
    fn multi_selection_rotation_is_rigid() {
        let mut p = demo_project();
        let ids = [named(&p, "sun"), named(&p, "flower B")];
        let before: Vec<Matrix> = ids.iter().map(|id| p.element(*id).unwrap().transform.matrix()).collect();
        let scope = Scope::root(&p);
        let s = TransformSession::begin(&p, scope, &ids, DragMode::Rotate, Point::new(1000.0, 200.0)).unwrap();
        let c = s.center;
        s.update(&mut p, Point::new(c.x, c.y + 500.0), Modifiers::default(), &no_snap());
        let g = Matrix::translate(c.x, c.y)
            * Matrix::rotate(s_angle(c, Point::new(1000.0, 200.0), Point::new(c.x, c.y + 500.0)))
            * Matrix::translate(-c.x, -c.y);
        for (id, m0) in ids.iter().zip(before) {
            assert!(p.element(*id).unwrap().transform.matrix().approx_eq(&(g * m0), 1e-9));
        }
    }

    fn s_angle(c: Point, a: Point, b: Point) -> f64 {
        ((b.y - c.y).atan2(b.x - c.x) - (a.y - c.y).atan2(a.x - c.x)).to_degrees()
    }

    #[test]
    fn pivot_drag_does_not_move_element() {
        let mut p = demo_project();
        let sun = named(&p, "sun");
        let m0 = p.element(sun).unwrap().transform.matrix();
        let s = TransformSession::begin(&p, Scope::root(&p), &[sun], DragMode::Pivot, Point::new(820.0, 90.0)).unwrap();
        // Near the NW corner (765, 35) → snaps to it.
        s.update(&mut p, Point::new(767.0, 37.0), Modifiers::default(), &SnapConfig { tolerance: 5.0, ..Default::default() });
        let t = p.element(sun).unwrap().transform;
        assert!(t.matrix().approx_eq(&m0, 1e-9));
        assert!((t.x - 765.0).abs() < 1e-9 && (t.y - 35.0).abs() < 1e-9);
        assert!((t.pivot_x + 55.0).abs() < 1e-9);
    }

    #[test]
    fn skew_edge_drag() {
        let mut p = demo_project();
        let sun = named(&p, "sun");
        let s =
            TransformSession::begin(&p, Scope::root(&p), &[sun], DragMode::Skew { handle: Handle::S }, Point::new(820.0, 145.0))
                .unwrap();
        // Bottom edge sheared 110 px over the 110 px box height → 45°.
        s.update(&mut p, Point::new(930.0, 145.0), Modifiers::default(), &no_snap());
        let t = p.element(sun).unwrap().transform;
        assert!((t.skew_x - 45.0).abs() < 1e-9, "skew_x {}", t.skew_x);
        assert_eq!(t.rotation, 0.0);
    }

    #[test]
    fn shape_drags() {
        let d = shape_from_drag(
            ShapeTool::Rect,
            Point::new(10.0, 10.0),
            Point::new(50.0, 30.0),
            Modifiers::default(),
            &ShapeOptions::default(),
        )
        .unwrap();
        assert_eq!(d.geometry, Geometry::Rect { width: 40.0, height: 20.0 });
        assert_eq!(d.center, Point::new(30.0, 20.0));
        let sq = shape_from_drag(
            ShapeTool::Ellipse,
            Point::new(10.0, 10.0),
            Point::new(-30.0, 20.0),
            Modifiers { shift: true, alt: false },
            &ShapeOptions::default(),
        )
        .unwrap();
        assert_eq!(sq.geometry, Geometry::Ellipse { width: 40.0, height: 40.0 });
        assert_eq!(sq.center, Point::new(-10.0, 30.0));
        let c = shape_from_drag(
            ShapeTool::Rect,
            Point::new(0.0, 0.0),
            Point::new(5.0, 3.0),
            Modifiers { shift: false, alt: true },
            &ShapeOptions::default(),
        )
        .unwrap();
        assert_eq!((c.geometry, c.center), (Geometry::Rect { width: 10.0, height: 6.0 }, Point::new(0.0, 0.0)));
        let l = shape_from_drag(
            ShapeTool::Line,
            Point::new(0.0, 0.0),
            Point::new(10.0, 1.0),
            Modifiers { shift: true, alt: false },
            &ShapeOptions::default(),
        )
        .unwrap();
        assert_eq!(l.geometry, Geometry::Line { dx: 10.04987562112089, dy: 0.0 });
        assert!(shape_from_drag(
            ShapeTool::Rect,
            Point::new(0.0, 0.0),
            Point::new(0.5, 9.0),
            Modifiers::default(),
            &ShapeOptions::default()
        )
        .is_none());
    }

    #[test]
    fn pen_builds_and_closes_paths() {
        let mut pen = PenSession::default();
        let m = Modifiers::default();
        assert!(!pen.pointer_down(Point::new(0.0, 0.0), m, 5.0));
        pen.pointer_up();
        // Click-drag: a smooth anchor with symmetric handles.
        pen.pointer_down(Point::new(100.0, 0.0), m, 5.0);
        pen.pointer_drag(Point::new(130.0, 20.0), m);
        pen.pointer_up();
        // Shift constrains the next click to 45° steps from the previous anchor.
        pen.pointer_down(Point::new(103.0, 97.0), Modifiers { shift: true, alt: false }, 5.0);
        pen.pointer_up();
        pen.hover(Point::new(2.0, 1.0), m);
        assert!(pen.preview(5.0).can_close);
        assert!(pen.pointer_down(Point::new(2.0, 1.0), m, 5.0), "click on first anchor closes");
        pen.pointer_up();
        let v = pen.finish().unwrap();
        let sp = &v.subpaths[0];
        assert!(sp.closed);
        assert_eq!(sp.nodes.len(), 3);
        assert_eq!(sp.nodes[1].kind, NodeKind::Symmetric);
        assert_eq!(sp.nodes[1].handle_in, Some(Point::new(70.0, -20.0)));
        // (103, 97) snapped to straight down from (100, 0), keeping its distance.
        assert_eq!(sp.nodes[2].x, 100.0);
        assert!((sp.nodes[2].y - 97.0f64.hypot(3.0)).abs() < 1e-9);
        assert!(PenSession::default().finish().is_none());
    }

    #[test]
    fn edit_session_moves_anchor_and_converts_primitive() {
        let mut doc = Document::new(demo_project());
        let sun = named(&doc.project, "sun");
        let scope = Scope::root(&doc.project);
        let info = path_info(&doc.project, &scope, sun).unwrap();
        assert!(info.primitive);
        let a0 = &info.subpaths[0][0]; // rightmost point of the sun (875, 90)
        let s = EditSession::begin(
            &doc.project,
            scope,
            sun,
            EditTarget::Anchors { nodes: vec![NodeRef { subpath: 0, node: 0 }] },
            Point::new(a0.x, a0.y),
        )
        .unwrap();
        s.update(&mut doc.project, Point::new(a0.x + 30.0, a0.y), Modifiers::default()).unwrap();
        s.commit(&mut doc).unwrap();
        let after = path_info(&doc.project, &scope, sun).unwrap();
        assert!(!after.primitive);
        assert!((after.subpaths[0][0].x - (a0.x + 30.0)).abs() < 1e-9);
        assert_eq!(doc.undo_label(), Some("Edit Path"));
        doc.undo().unwrap();
        assert!(path_info(&doc.project, &scope, sun).unwrap().primitive);
    }

    #[test]
    fn gradient_handles_edit_in_content_space() {
        let mut doc = Document::new(demo_project());
        let sun = named(&doc.project, "sun");
        let scope = Scope::root(&doc.project);
        let Some(GradientControls::Radial { center, radius, .. }) = gradient_controls(&doc.project, &scope, sun, PaintPart::Fill)
        else {
            panic!()
        };
        assert_eq!((center, radius), (Point::new(820.0, 90.0), Point::new(875.0, 90.0)));
        let s = EditSession::begin(
            &doc.project,
            scope,
            sun,
            EditTarget::Gradient { part: PaintPart::Fill, handle: GradientHandle::Radius },
            radius,
        )
        .unwrap();
        s.update(&mut doc.project, Point::new(820.0, 170.0), Modifiers::default()).unwrap();
        // Focal clamps inside the circle.
        s.commit(&mut doc).unwrap();
        let e = doc.project.element(sun).unwrap();
        let ElementKind::Shape(sh) = &e.kind else { panic!() };
        let Some(Paint::Radial { radius, .. }) = &sh.fill else { panic!() };
        assert!((radius - 80.0).abs() < 1e-9);
        assert!(EditSession::begin(
            &doc.project,
            scope,
            named(&doc.project, "ground"),
            EditTarget::Gradient { part: PaintPart::Fill, handle: GradientHandle::Start },
            Point::default()
        )
        .is_err());
    }

    #[test]
    fn polygon_tool() {
        let d = shape_from_drag(
            ShapeTool::Polygon,
            Point::new(50.0, 50.0),
            Point::new(50.0, 10.0),
            Modifiers::default(),
            &ShapeOptions { sides: 6, star: None },
        )
        .unwrap();
        assert_eq!(d.center, Point::new(50.0, 50.0));
        let Geometry::Path(v) = &d.geometry else { panic!() };
        assert_eq!(v.subpaths[0].nodes.len(), 6);
        assert_eq!(v.subpaths[0].nodes[0].point(), Point::new(0.0, -40.0), "first vertex toward the pointer");
    }
}
