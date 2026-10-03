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
use crate::geom::Rect;
use crate::history::Document;
use crate::math::{Matrix, Point};
use crate::model::*;
use crate::query::{content_bounds, selectable, selection_frame, Scope};
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
    Scale { handle: Handle },
    Rotate,
    /// `handle` must be an edge (N/S shear horizontally, E/W vertically).
    Skew { handle: Handle },
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
            if let Some(b) = content_bounds(p, &e.kind, &(scope.matrix * e.transform.matrix()), 0) {
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
            let loc = p.locate(*id).ok_or_else(|| Error::NotFound(format!("element {}", id.0)))?;
            if loc.symbol != scope.symbol {
                return Err(Error::Invalid("element is not in the edited symbol".into()));
            }
            if p.layer_state(loc.layer).is_some_and(|(_, locked)| locked) {
                return Err(Error::Invalid("element is on a locked layer".into()));
            }
        }
        if mode == DragMode::Pivot && ids.len() != 1 {
            return Err(Error::Invalid("the pivot can only be moved for a single element".into()));
        }
        let initial: Vec<Transform> = ids.iter().map(|id| p.element(*id).unwrap().transform).collect();
        let (frame, bbox) = selection_frame(p, &scope, ids)
            .unwrap_or((Matrix::IDENTITY, Rect { min: start, max: start }));
        let center = match ids {
            [_] => scope.matrix.apply(Point::new(initial[0].x, initial[0].y)),
            _ => frame.apply(bbox.center()),
        };
        let start_bounds = Rect::union_all(ids.iter().filter_map(|id| {
            let e = p.element(*id)?;
            content_bounds(p, &e.kind, &(scope.matrix * e.transform.matrix()), 0)
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
            let Some(e) = doc.project.element_mut(*id) else { continue };
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
            Point::new(
                if sx > 0 { b.min.x } else { b.max.x },
                if sy > 0 { b.min.y } else { b.max.y },
            )
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
            let ay = if mods.alt { c.y } else if sy > 0 { b.min.y } else { b.max.y };
            let span = p0.y - ay;
            let k = if span.abs() < 1e-9 { 0.0 } else { (p.x - p0.x) / span };
            Matrix::translate(0.0, ay) * Matrix { a: 1.0, b: 0.0, c: k, d: 1.0, e: 0.0, f: 0.0 } * Matrix::translate(0.0, -ay)
        } else {
            let ax = if mods.alt { c.x } else if sx > 0 { b.min.x } else { b.max.x };
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
}

/// Geometry + placement (stage coordinates) for a shape-tool drag, or
/// `None` if the drag is too small to make a shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeDrag {
    pub geometry: Geometry,
    pub center: Point,
}

pub fn shape_from_drag(tool: ShapeTool, p0: Point, p1: Point, mods: Modifiers) -> Option<ShapeDrag> {
    let (mut dx, mut dy) = (p1.x - p0.x, p1.y - p0.y);
    match tool {
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
            let (center, full) = if mods.alt {
                (p0, (2.0 * dx, 2.0 * dy))
            } else {
                (Point::new(p0.x + dx / 2.0, p0.y + dy / 2.0), (dx, dy))
            };
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
        render_frame(p, 0, RenderOptions { view: Matrix::IDENTITY, clip_to_stage: true, show_guides: true }, &mut r);
        r.ops
    }

    #[test]
    fn move_session_previews_and_commits_as_one_undo_step() {
        let mut doc = Document::new(demo_project());
        let before = render(&doc.project);
        let sun = named(&doc.project, "sun");
        let s = TransformSession::begin(&doc.project, Scope::root(&doc.project), &[sun], DragMode::Move, Point::new(820.0, 90.0)).unwrap();
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
        let s = TransformSession::begin(&doc.project, Scope::root(&doc.project), &[sun], DragMode::Rotate, Point::new(900.0, 90.0)).unwrap();
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
        let g = Matrix::translate(c.x, c.y) * Matrix::rotate(s_angle(c, Point::new(1000.0, 200.0), Point::new(c.x, c.y + 500.0))) * Matrix::translate(-c.x, -c.y);
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
        let s = TransformSession::begin(&p, Scope::root(&p), &[sun], DragMode::Skew { handle: Handle::S }, Point::new(820.0, 145.0)).unwrap();
        // Bottom edge sheared 110 px over the 110 px box height → 45°.
        s.update(&mut p, Point::new(930.0, 145.0), Modifiers::default(), &no_snap());
        let t = p.element(sun).unwrap().transform;
        assert!((t.skew_x - 45.0).abs() < 1e-9, "skew_x {}", t.skew_x);
        assert_eq!(t.rotation, 0.0);
    }

    #[test]
    fn shape_drags() {
        let d = shape_from_drag(ShapeTool::Rect, Point::new(10.0, 10.0), Point::new(50.0, 30.0), Modifiers::default()).unwrap();
        assert_eq!(d.geometry, Geometry::Rect { width: 40.0, height: 20.0 });
        assert_eq!(d.center, Point::new(30.0, 20.0));
        let sq = shape_from_drag(ShapeTool::Ellipse, Point::new(10.0, 10.0), Point::new(-30.0, 20.0), Modifiers { shift: true, alt: false }).unwrap();
        assert_eq!(sq.geometry, Geometry::Ellipse { width: 40.0, height: 40.0 });
        assert_eq!(sq.center, Point::new(-10.0, 30.0));
        let c = shape_from_drag(ShapeTool::Rect, Point::new(0.0, 0.0), Point::new(5.0, 3.0), Modifiers { shift: false, alt: true }).unwrap();
        assert_eq!((c.geometry, c.center), (Geometry::Rect { width: 10.0, height: 6.0 }, Point::new(0.0, 0.0)));
        let l = shape_from_drag(ShapeTool::Line, Point::new(0.0, 0.0), Point::new(10.0, 1.0), Modifiers { shift: true, alt: false }).unwrap();
        assert_eq!(l.geometry, Geometry::Line { dx: 10.04987562112089, dy: 0.0 });
        assert!(shape_from_drag(ShapeTool::Rect, Point::new(0.0, 0.0), Point::new(0.5, 9.0), Modifiers::default()).is_none());
    }
}
