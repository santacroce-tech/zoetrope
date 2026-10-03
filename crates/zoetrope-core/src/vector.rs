//! Editable vector paths: subpaths of anchor nodes with optional bezier
//! handles, plus the editing operations behind the pen, pencil and
//! subselection tools.
//!
//! A segment between nodes `a` → `b` is a cubic with control points
//! `a.out ?? a` and `b.in ?? b`, or a straight line when both are absent.
//! (Quadratic curves are representable exactly as cubics; `from_path`
//! degree-elevates them.) Coordinates are content coordinates.

use crate::error::{Error, Result};
use crate::geom::{Path, PathCmd, Polyline};
use crate::math::{Matrix, Point};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    /// Handles move independently.
    #[default]
    Corner,
    /// Handles stay collinear (lengths independent).
    Smooth,
    /// Handles stay collinear and equal length.
    Symmetric,
}

fn is_corner(k: &NodeKind) -> bool {
    *k == NodeKind::Corner
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub x: f64,
    pub y: f64,
    /// Incoming handle (absolute position).
    #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
    pub handle_in: Option<Point>,
    /// Outgoing handle (absolute position).
    #[serde(rename = "out", default, skip_serializing_if = "Option::is_none")]
    pub handle_out: Option<Point>,
    #[serde(default, skip_serializing_if = "is_corner")]
    pub kind: NodeKind,
}

impl Node {
    pub fn corner(p: Point) -> Node {
        Node { x: p.x, y: p.y, handle_in: None, handle_out: None, kind: NodeKind::Corner }
    }

    pub fn point(&self) -> Point {
        Point::new(self.x, self.y)
    }

    fn map(&mut self, f: impl Fn(Point) -> Point) {
        let p = f(self.point());
        (self.x, self.y) = (p.x, p.y);
        self.handle_in = self.handle_in.map(&f);
        self.handle_out = self.handle_out.map(&f);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubPath {
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub closed: bool,
}

impl SubPath {
    pub fn segment_count(&self) -> usize {
        match self.nodes.len() {
            0 | 1 => 0,
            n if self.closed => n,
            n => n - 1,
        }
    }

    /// Segment `i` as (P0, P1, P2, P3); P1/P2 equal the endpoints for lines.
    pub fn segment(&self, i: usize) -> (Point, Point, Point, Point, bool) {
        let a = self.nodes[i];
        let b = self.nodes[(i + 1) % self.nodes.len()];
        let curved = a.handle_out.is_some() || b.handle_in.is_some();
        (a.point(), a.handle_out.unwrap_or(a.point()), b.handle_in.unwrap_or(b.point()), b.point(), curved)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRef {
    pub subpath: usize,
    pub node: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HandleSide {
    In,
    Out,
}

/// Where a point lies on a path: segment `segment` of `subpath`, at
/// parameter `t`, at distance `distance`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PathHit {
    pub subpath: usize,
    pub segment: usize,
    pub t: f64,
    pub distance: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VectorPath {
    pub subpaths: Vec<SubPath>,
}

const KAPPA: f64 = 0.552_284_749_830_793_4;
const SEGMENT_STEPS: usize = 32;

fn lerp(a: Point, b: Point, t: f64) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn cubic_at(p0: Point, p1: Point, p2: Point, p3: Point, t: f64) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(a * p0.x + b * p1.x + c * p2.x + d * p3.x, a * p0.y + b * p1.y + c * p2.y + d * p3.y)
}

fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}

fn add(a: Point, b: Point) -> Point {
    Point::new(a.x + b.x, a.y + b.y)
}

fn scale(a: Point, k: f64) -> Point {
    Point::new(a.x * k, a.y * k)
}

fn len(a: Point) -> f64 {
    a.x.hypot(a.y)
}

impl VectorPath {
    pub fn single(nodes: Vec<Node>, closed: bool) -> VectorPath {
        VectorPath { subpaths: vec![SubPath { nodes, closed }] }
    }

    /// Straight segments through `points`.
    pub fn polyline(points: &[Point], closed: bool) -> VectorPath {
        VectorPath::single(points.iter().map(|p| Node::corner(*p)).collect(), closed)
    }

    /// A smooth curve through `points` (Catmull-Rom → cubic bezier).
    pub fn smooth_through(points: &[Point], closed: bool) -> VectorPath {
        let n = points.len();
        if n < 3 {
            return VectorPath::polyline(points, closed);
        }
        let at = |i: isize| -> Point {
            if closed {
                points[i.rem_euclid(n as isize) as usize]
            } else {
                points[i.clamp(0, n as isize - 1) as usize]
            }
        };
        let nodes = (0..n)
            .map(|i| {
                let p = points[i];
                let tangent = scale(sub(at(i as isize + 1), at(i as isize - 1)), 1.0 / 6.0);
                let endpoint = !closed && (i == 0 || i == n - 1);
                Node {
                    x: p.x,
                    y: p.y,
                    handle_in: (closed || i > 0).then(|| sub(p, tangent)),
                    handle_out: (closed || i < n - 1).then(|| add(p, tangent)),
                    kind: if endpoint { NodeKind::Corner } else { NodeKind::Smooth },
                }
            })
            .collect();
        VectorPath::single(nodes, closed)
    }

    pub fn rect(width: f64, height: f64) -> VectorPath {
        let (w, h) = (width / 2.0, height / 2.0);
        VectorPath::polyline(&[Point::new(-w, -h), Point::new(w, -h), Point::new(w, h), Point::new(-w, h)], true)
    }

    pub fn ellipse(width: f64, height: f64) -> VectorPath {
        let (rx, ry) = (width / 2.0, height / 2.0);
        let (kx, ky) = (rx * KAPPA, ry * KAPPA);
        let node = |x: f64, y: f64, hin: (f64, f64), hout: (f64, f64)| Node {
            x,
            y,
            handle_in: Some(Point::new(x + hin.0, y + hin.1)),
            handle_out: Some(Point::new(x + hout.0, y + hout.1)),
            kind: NodeKind::Symmetric,
        };
        VectorPath::single(
            vec![
                node(rx, 0.0, (0.0, -ky), (0.0, ky)),
                node(0.0, ry, (kx, 0.0), (-kx, 0.0)),
                node(-rx, 0.0, (0.0, ky), (0.0, -ky)),
                node(0.0, -ry, (-kx, 0.0), (kx, 0.0)),
            ],
            true,
        )
    }

    /// Regular polygon (or star when `inner_ratio` is set) centered on the
    /// origin; `rotation` in degrees, 0 = first vertex straight up.
    pub fn polystar(sides: u32, radius: f64, inner_ratio: Option<f64>, rotation: f64) -> VectorPath {
        let sides = sides.clamp(3, 64) as usize;
        let count = if inner_ratio.is_some() { sides * 2 } else { sides };
        let inner = radius * inner_ratio.unwrap_or(1.0).clamp(0.01, 1.0);
        let pts: Vec<Point> = (0..count)
            .map(|i| {
                let a = (rotation - 90.0).to_radians() + std::f64::consts::TAU * i as f64 / count as f64;
                let r = if inner_ratio.is_some() && i % 2 == 1 { inner } else { radius };
                Point::new(clean(r * a.cos()), clean(r * a.sin()))
            })
            .collect();
        VectorPath::polyline(&pts, true)
    }

    /// Converts a flat command path (e.g. from a primitive) into nodes.
    pub fn from_path(path: &Path) -> VectorPath {
        let mut out = VectorPath::default();
        let mut cur: Option<SubPath> = None;
        for cmd in &path.cmds {
            match *cmd {
                PathCmd::MoveTo(p) => {
                    out.subpaths.extend(cur.take());
                    cur = Some(SubPath { nodes: vec![Node::corner(p)], closed: false });
                }
                PathCmd::LineTo(p) => cur.get_or_insert(SubPath { nodes: vec![], closed: false }).nodes.push(Node::corner(p)),
                PathCmd::QuadTo(c, p) => {
                    let sp = cur.get_or_insert(SubPath { nodes: vec![Node::corner(Point::default())], closed: false });
                    let last = sp.nodes.last_mut().unwrap();
                    let p0 = last.point();
                    // Degree elevation: exact.
                    last.handle_out = Some(lerp(p0, c, 2.0 / 3.0));
                    sp.nodes.push(Node { handle_in: Some(lerp(p, c, 2.0 / 3.0)), ..Node::corner(p) });
                }
                PathCmd::CubicTo(c1, c2, p) => {
                    let sp = cur.get_or_insert(SubPath { nodes: vec![Node::corner(Point::default())], closed: false });
                    sp.nodes.last_mut().unwrap().handle_out = Some(c1);
                    sp.nodes.push(Node { handle_in: Some(c2), ..Node::corner(p) });
                }
                PathCmd::Close => {
                    if let Some(mut sp) = cur.take() {
                        // A closing node that repeats the first one merges into it.
                        if sp.nodes.len() > 1 {
                            let (first, last) = (sp.nodes[0], *sp.nodes.last().unwrap());
                            if (first.x - last.x).abs() < 1e-9 && (first.y - last.y).abs() < 1e-9 {
                                sp.nodes[0].handle_in = last.handle_in;
                                sp.nodes.pop();
                            }
                        }
                        sp.closed = true;
                        out.subpaths.push(sp);
                    }
                }
            }
        }
        out.subpaths.extend(cur);
        out
    }

    pub fn to_path(&self) -> Path {
        let mut p = Path::default();
        for sp in &self.subpaths {
            let Some(first) = sp.nodes.first() else { continue };
            p.cmds.push(PathCmd::MoveTo(first.point()));
            for i in 0..sp.segment_count() {
                let (_, c1, c2, end, curved) = sp.segment(i);
                p.cmds.push(if curved { PathCmd::CubicTo(c1, c2, end) } else { PathCmd::LineTo(end) });
            }
            if sp.closed {
                p.cmds.push(PathCmd::Close);
            }
        }
        p
    }

    pub fn node_count(&self) -> usize {
        self.subpaths.iter().map(|s| s.nodes.len()).sum()
    }

    pub fn node(&self, r: NodeRef) -> Option<&Node> {
        self.subpaths.get(r.subpath)?.nodes.get(r.node)
    }

    fn node_mut(&mut self, r: NodeRef) -> Result<&mut Node> {
        self.subpaths
            .get_mut(r.subpath)
            .and_then(|s| s.nodes.get_mut(r.node))
            .ok_or_else(|| Error::NotFound(format!("anchor {}:{}", r.subpath, r.node)))
    }

    pub fn transform(&mut self, m: &Matrix) {
        for sp in &mut self.subpaths {
            for n in &mut sp.nodes {
                n.map(|p| m.apply(p));
            }
        }
    }

    pub fn validate(&self) -> Result<()> {
        let finite = |p: Point| p.x.is_finite() && p.y.is_finite();
        let ok = self.subpaths.iter().all(|sp| {
            !sp.nodes.is_empty()
                && sp.nodes.iter().all(|n| finite(n.point()) && n.handle_in.is_none_or(finite) && n.handle_out.is_none_or(finite))
        });
        if self.subpaths.is_empty() || !ok {
            return Err(Error::Invalid("path needs at least one non-empty subpath with finite points".into()));
        }
        Ok(())
    }

    /// Closest point on the outline to `p`.
    pub fn nearest(&self, p: Point) -> Option<PathHit> {
        let mut best: Option<PathHit> = None;
        for (si, sp) in self.subpaths.iter().enumerate() {
            for seg in 0..sp.segment_count() {
                let (p0, p1, p2, p3, _) = sp.segment(seg);
                let mut prev = p0;
                for k in 1..=SEGMENT_STEPS {
                    let t1 = k as f64 / SEGMENT_STEPS as f64;
                    let cur = cubic_at(p0, p1, p2, p3, t1);
                    // Project onto this chord to refine t within the step.
                    let d = sub(cur, prev);
                    let l2 = d.x * d.x + d.y * d.y;
                    let u = if l2 > 0.0 { (((p.x - prev.x) * d.x + (p.y - prev.y) * d.y) / l2).clamp(0.0, 1.0) } else { 0.0 };
                    let q = add(prev, scale(d, u));
                    let dist = len(sub(p, q));
                    if best.is_none_or(|b| dist < b.distance) {
                        let t = (k as f64 - 1.0 + u) / SEGMENT_STEPS as f64;
                        best = Some(PathHit { subpath: si, segment: seg, t, distance: dist });
                    }
                    prev = cur;
                }
            }
        }
        best
    }

    /// Splits segment `segment` of `subpath` at `t`, inserting an anchor
    /// without changing the outline. Returns the new node.
    pub fn split(&mut self, subpath: usize, segment: usize, t: f64) -> Result<NodeRef> {
        let sp = self.subpaths.get_mut(subpath).ok_or_else(|| Error::NotFound("subpath".into()))?;
        if segment >= sp.segment_count() {
            return Err(Error::NotFound("segment".into()));
        }
        let t = t.clamp(1e-4, 1.0 - 1e-4);
        let n = sp.nodes.len();
        let (ia, ib) = (segment, (segment + 1) % n);
        let (p0, p1, p2, p3, curved) = sp.segment(segment);
        let new = if curved {
            let (p01, p12, p23) = (lerp(p0, p1, t), lerp(p1, p2, t), lerp(p2, p3, t));
            let (p012, p123) = (lerp(p01, p12, t), lerp(p12, p23, t));
            let mid = lerp(p012, p123, t);
            sp.nodes[ia].handle_out = Some(p01);
            sp.nodes[ib].handle_in = Some(p23);
            Node { x: mid.x, y: mid.y, handle_in: Some(p012), handle_out: Some(p123), kind: NodeKind::Smooth }
        } else {
            Node::corner(lerp(p0, p3, t))
        };
        let at = segment + 1;
        sp.nodes.insert(at, new);
        Ok(NodeRef { subpath, node: at })
    }

    /// Removes anchors. Subpaths left with fewer than two anchors are removed.
    pub fn delete_nodes(&mut self, refs: &[NodeRef]) {
        let mut refs = refs.to_vec();
        refs.sort_by_key(|r| std::cmp::Reverse((r.subpath, r.node)));
        refs.dedup();
        for r in refs {
            if let Some(sp) = self.subpaths.get_mut(r.subpath) {
                if r.node < sp.nodes.len() {
                    sp.nodes.remove(r.node);
                }
            }
        }
        self.subpaths.retain(|sp| sp.nodes.len() >= 2);
    }

    /// Moves anchors (with their handles) by `d`.
    pub fn move_nodes(&mut self, refs: &[NodeRef], d: Point) -> Result<()> {
        for r in refs {
            self.node_mut(*r)?.map(|p| add(p, d));
        }
        Ok(())
    }

    /// Sets a handle. Unless `break_tangent`, smooth/symmetric anchors keep
    /// the opposite handle collinear (and equal length for symmetric);
    /// breaking turns the anchor into a corner.
    pub fn set_handle(&mut self, r: NodeRef, side: HandleSide, pos: Point, break_tangent: bool) -> Result<()> {
        let n = self.node_mut(r)?;
        let p = n.point();
        let (this, other) = match side {
            HandleSide::In => (&mut n.handle_in, &mut n.handle_out),
            HandleSide::Out => (&mut n.handle_out, &mut n.handle_in),
        };
        *this = Some(pos);
        if break_tangent {
            n.kind = NodeKind::Corner;
            return Ok(());
        }
        let v = sub(pos, p);
        let l = len(v);
        match n.kind {
            NodeKind::Corner => {}
            NodeKind::Symmetric => *other = Some(sub(p, v)),
            NodeKind::Smooth => {
                if let (Some(o), true) = (*other, l > 1e-9) {
                    let ol = len(sub(o, p));
                    *other = Some(sub(p, scale(v, ol / l)));
                }
            }
        }
        Ok(())
    }

    /// Corner with handles → plain corner; corner without handles → smooth
    /// anchor with handles along its neighbors' direction.
    pub fn convert_node(&mut self, r: NodeRef) -> Result<()> {
        let sp = self.subpaths.get(r.subpath).ok_or_else(|| Error::NotFound("subpath".into()))?;
        let n = *sp.nodes.get(r.node).ok_or_else(|| Error::NotFound("anchor".into()))?;
        if n.handle_in.is_some() || n.handle_out.is_some() {
            let node = self.node_mut(r)?;
            (node.handle_in, node.handle_out, node.kind) = (None, None, NodeKind::Corner);
            return Ok(());
        }
        let count = sp.nodes.len();
        let prev = if r.node > 0 || sp.closed { Some(sp.nodes[(r.node + count - 1) % count].point()) } else { None };
        let next = if r.node + 1 < count || sp.closed { Some(sp.nodes[(r.node + 1) % count].point()) } else { None };
        let p = n.point();
        let dir = sub(next.unwrap_or(p), prev.unwrap_or(p));
        let l = len(dir);
        if l < 1e-9 {
            return Ok(());
        }
        let u = scale(dir, 1.0 / l);
        let node = self.node_mut(r)?;
        node.handle_in = prev.map(|q| sub(p, scale(u, len(sub(p, q)) / 3.0)));
        node.handle_out = next.map(|q| add(p, scale(u, len(sub(q, p)) / 3.0)));
        node.kind = NodeKind::Smooth;
        Ok(())
    }

    /// Flattened outline (for overlays and hit-testing), mapped by `m`.
    pub fn outline(&self, m: &Matrix) -> Vec<Polyline> {
        let mut p = self.to_path();
        for cmd in &mut p.cmds {
            match cmd {
                PathCmd::MoveTo(a) | PathCmd::LineTo(a) => *a = m.apply(*a),
                PathCmd::QuadTo(a, b) => (*a, *b) = (m.apply(*a), m.apply(*b)),
                PathCmd::CubicTo(a, b, c) => (*a, *b, *c) = (m.apply(*a), m.apply(*b), m.apply(*c)),
                PathCmd::Close => {}
            }
        }
        p.flatten()
    }
}

fn clean(v: f64) -> f64 {
    if v.abs() < 1e-9 {
        0.0
    } else {
        (v * 1e9).round() / 1e9
    }
}

/// Ramer–Douglas–Peucker simplification (keeps endpoints).
pub fn simplify(points: &[Point], tolerance: f64) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let mut best = (0.0, 0);
        for i in a + 1..b {
            let d = crate::geom::segment_distance(points[i], points[a], points[b]);
            if d > best.0 {
                best = (d, i);
            }
        }
        if best.0 > tolerance {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    points.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// Turns a freehand stroke (raw pointer samples) into a path: simplified,
/// optionally smoothed, and closed if it ends near where it started.
pub fn freehand(points: &[Point], tolerance: f64, smooth: bool, close_distance: f64) -> Option<VectorPath> {
    let mut pts: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if pts.last().is_none_or(|q: &Point| len(sub(*p, *q)) > 1e-6) {
            pts.push(*p);
        }
    }
    if pts.len() < 2 {
        return None;
    }
    let closed = pts.len() > 3 && len(sub(pts[0], *pts.last().unwrap())) <= close_distance;
    let mut simple = simplify(&pts, tolerance.max(0.01));
    if closed && simple.len() > 3 {
        simple.pop();
    }
    if simple.len() < 2 {
        return None;
    }
    let closed = closed && simple.len() >= 3;
    Some(if smooth { VectorPath::smooth_through(&simple, closed) } else { VectorPath::polyline(&simple, closed) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6
    }

    #[test]
    fn ellipse_path_matches_primitive() {
        let v = VectorPath::ellipse(20.0, 10.0);
        let a = v.to_path().bounds(&Matrix::IDENTITY).unwrap();
        let b = Path::ellipse(0.0, 0.0, 10.0, 5.0).bounds(&Matrix::IDENTITY).unwrap();
        assert!(close(a.min, b.min) && close(a.max, b.max));
        // Round-trips through command form.
        let back = VectorPath::from_path(&Path::ellipse(0.0, 0.0, 10.0, 5.0));
        assert_eq!(back.to_path().flatten(), Path::ellipse(0.0, 0.0, 10.0, 5.0).flatten());
    }

    #[test]
    fn quadratics_degree_elevate_exactly() {
        let mut p = Path::default();
        p.move_to(0.0, 0.0);
        p.cmds.push(PathCmd::QuadTo(pt(10.0, 20.0), pt(20.0, 0.0)));
        let v = VectorPath::from_path(&p);
        let q = |t: f64| {
            let u = 1.0 - t;
            pt(2.0 * u * t * 10.0 + t * t * 20.0, 2.0 * u * t * 20.0)
        };
        let (p0, p1, p2, p3, _) = v.subpaths[0].segment(0);
        for t in [0.1, 0.5, 0.77] {
            assert!(close(cubic_at(p0, p1, p2, p3, t), q(t)));
        }
    }

    #[test]
    fn split_preserves_shape() {
        let mut v = VectorPath::ellipse(40.0, 20.0);
        let before = v.to_path();
        let r = v.split(0, 1, 0.3).unwrap();
        assert_eq!(r, NodeRef { subpath: 0, node: 2 });
        assert_eq!(v.subpaths[0].nodes.len(), 5);
        // Every point on the new outline lies on the old one.
        let old = before.flatten();
        for pl in v.to_path().flatten() {
            for p in pl.points.iter().step_by(7) {
                let d = old[0].points.windows(2).map(|w| crate::geom::segment_distance(*p, w[0], w[1])).fold(f64::INFINITY, f64::min);
                assert!(d < 0.05, "{d}");
            }
        }
        // Lines split into corners; the closing segment inserts at the end.
        let mut sq = VectorPath::rect(10.0, 10.0);
        let r = sq.split(0, 3, 0.5).unwrap();
        assert_eq!(r.node, 4);
        assert!(close(sq.subpaths[0].nodes[4].point(), pt(-5.0, 0.0)));
    }

    #[test]
    fn nearest_finds_segment_and_t() {
        let v = VectorPath::rect(10.0, 10.0);
        let h = v.nearest(pt(0.0, -6.0)).unwrap();
        assert_eq!((h.subpath, h.segment), (0, 0));
        assert!((h.t - 0.5).abs() < 1e-9 && (h.distance - 1.0).abs() < 1e-9);
    }

    #[test]
    fn handle_constraints() {
        let mut v = VectorPath::smooth_through(&[pt(0.0, 0.0), pt(10.0, 10.0), pt(20.0, 0.0)], false);
        let r = NodeRef { subpath: 0, node: 1 };
        let n = *v.node(r).unwrap();
        let in_len = len(sub(n.handle_in.unwrap(), n.point()));
        v.set_handle(r, HandleSide::Out, pt(10.0, 0.0), false).unwrap();
        let n = *v.node(r).unwrap();
        assert!(close(n.handle_in.unwrap(), pt(10.0, 10.0 + in_len)), "smooth keeps opposite length, mirrors direction");
        v.set_handle(r, HandleSide::Out, pt(15.0, 15.0), true).unwrap();
        assert_eq!(v.node(r).unwrap().kind, NodeKind::Corner);
        assert!(close(v.node(r).unwrap().handle_in.unwrap(), pt(10.0, 10.0 + in_len)), "break leaves the other handle");

        let mut e = VectorPath::ellipse(20.0, 20.0);
        let r0 = NodeRef { subpath: 0, node: 0 };
        e.set_handle(r0, HandleSide::Out, pt(10.0, 8.0), false).unwrap();
        assert!(close(e.node(r0).unwrap().handle_in.unwrap(), pt(10.0, -8.0)), "symmetric mirrors");
    }

    #[test]
    fn convert_and_delete_nodes() {
        let mut v = VectorPath::rect(10.0, 10.0);
        let r = NodeRef { subpath: 0, node: 1 };
        v.convert_node(r).unwrap();
        assert_eq!(v.node(r).unwrap().kind, NodeKind::Smooth);
        assert!(v.node(r).unwrap().handle_in.is_some() && v.node(r).unwrap().handle_out.is_some());
        v.convert_node(r).unwrap();
        assert!(v.node(r).unwrap().handle_in.is_none());
        v.delete_nodes(&[NodeRef { subpath: 0, node: 0 }, NodeRef { subpath: 0, node: 2 }]);
        assert_eq!(v.subpaths[0].nodes.len(), 2);
        v.delete_nodes(&[NodeRef { subpath: 0, node: 0 }]);
        assert!(v.subpaths.is_empty());
    }

    #[test]
    fn freehand_simplifies_smooths_and_closes() {
        let circle: Vec<Point> = (0..=200)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / 200.0;
                pt(50.0 * a.cos(), 50.0 * a.sin())
            })
            .collect();
        let v = freehand(&circle, 1.0, true, 5.0).unwrap();
        assert!(v.subpaths[0].closed);
        assert!(v.subpaths[0].nodes.len() < 30, "simplified to {}", v.subpaths[0].nodes.len());
        let b = v.to_path().bounds(&Matrix::IDENTITY).unwrap();
        assert!((b.width() - 100.0).abs() < 3.0);
        let line = freehand(&[pt(0.0, 0.0), pt(5.0, 0.01), pt(10.0, 0.0)], 1.0, false, 2.0).unwrap();
        assert_eq!(line.subpaths[0].nodes.len(), 2);
        assert!(!line.subpaths[0].closed);
        assert!(freehand(&[pt(1.0, 1.0)], 1.0, true, 1.0).is_none());
    }

    #[test]
    fn polystar_vertices() {
        let p = VectorPath::polystar(4, 10.0, None, 0.0);
        assert!(close(p.subpaths[0].nodes[0].point(), pt(0.0, -10.0)));
        let s = VectorPath::polystar(5, 10.0, Some(0.5), 0.0);
        assert_eq!(s.subpaths[0].nodes.len(), 10);
        assert!((len(s.subpaths[0].nodes[1].point()) - 5.0).abs() < 1e-9);
    }
}
