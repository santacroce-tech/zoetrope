//! Vector path model. Every shape is reduced to a `Path` here, so renderers
//! only ever replay path commands and never compute geometry themselves.
//! Also hosts the flattening, containment and distance queries used for
//! hit-testing and tight bounds.

use crate::math::{Matrix, Point};

#[derive(Debug, Clone, PartialEq)]
pub enum PathCmd {
    MoveTo(Point),
    LineTo(Point),
    QuadTo(Point, Point),
    CubicTo(Point, Point, Point),
    Close,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    pub cmds: Vec<PathCmd>,
}

/// Cubic bezier control-point ratio that best approximates a quarter circle.
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// Line segments per curve when flattening. Fixed (not adaptive) so results
/// are deterministic and independent of zoom.
const CURVE_STEPS: usize = 24;

/// A flattened subpath.
#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub points: Vec<Point>,
    pub closed: bool,
}

impl Path {
    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.cmds.push(PathCmd::MoveTo(Point::new(x, y)));
        self
    }

    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.cmds.push(PathCmd::LineTo(Point::new(x, y)));
        self
    }

    pub fn cubic_to(&mut self, c1: Point, c2: Point, p: Point) -> &mut Self {
        self.cmds.push(PathCmd::CubicTo(c1, c2, p));
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.cmds.push(PathCmd::Close);
        self
    }

    pub fn rect(x: f64, y: f64, w: f64, h: f64) -> Path {
        let mut p = Path::default();
        p.move_to(x, y).line_to(x + w, y).line_to(x + w, y + h).line_to(x, y + h).close();
        p
    }

    pub fn line(x1: f64, y1: f64, x2: f64, y2: f64) -> Path {
        let mut p = Path::default();
        p.move_to(x1, y1).line_to(x2, y2);
        p
    }

    /// Ellipse centered at `(cx, cy)` built from four cubic segments.
    pub fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> Path {
        let (kx, ky) = (rx * KAPPA, ry * KAPPA);
        let pt = Point::new;
        let mut p = Path::default();
        p.move_to(cx + rx, cy)
            .cubic_to(pt(cx + rx, cy + ky), pt(cx + kx, cy + ry), pt(cx, cy + ry))
            .cubic_to(pt(cx - kx, cy + ry), pt(cx - rx, cy + ky), pt(cx - rx, cy))
            .cubic_to(pt(cx - rx, cy - ky), pt(cx - kx, cy - ry), pt(cx, cy - ry))
            .cubic_to(pt(cx + kx, cy - ry), pt(cx + rx, cy - ky), pt(cx + rx, cy))
            .close();
        p
    }

    /// Converts curves to line segments.
    pub fn flatten(&self) -> Vec<Polyline> {
        let mut out: Vec<Polyline> = Vec::new();
        let mut cur: Option<Polyline> = None;
        let mut last = Point::default();
        let mut start = Point::default();
        let flush = |cur: &mut Option<Polyline>, out: &mut Vec<Polyline>| {
            if let Some(pl) = cur.take() {
                if pl.points.len() > 1 || pl.closed {
                    out.push(pl);
                }
            }
        };
        for cmd in &self.cmds {
            match *cmd {
                PathCmd::MoveTo(p) => {
                    flush(&mut cur, &mut out);
                    cur = Some(Polyline { points: vec![p], closed: false });
                    last = p;
                    start = p;
                }
                PathCmd::LineTo(p) => {
                    cur.get_or_insert_with(|| Polyline { points: vec![last], closed: false }).points.push(p);
                    last = p;
                }
                PathCmd::QuadTo(c, p) => {
                    let pl = cur.get_or_insert_with(|| Polyline { points: vec![last], closed: false });
                    for i in 1..=CURVE_STEPS {
                        let t = i as f64 / CURVE_STEPS as f64;
                        let u = 1.0 - t;
                        pl.points.push(Point::new(
                            u * u * last.x + 2.0 * u * t * c.x + t * t * p.x,
                            u * u * last.y + 2.0 * u * t * c.y + t * t * p.y,
                        ));
                    }
                    last = p;
                }
                PathCmd::CubicTo(c1, c2, p) => {
                    let pl = cur.get_or_insert_with(|| Polyline { points: vec![last], closed: false });
                    for i in 1..=CURVE_STEPS {
                        let t = i as f64 / CURVE_STEPS as f64;
                        let u = 1.0 - t;
                        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                        pl.points.push(Point::new(
                            a * last.x + b * c1.x + c * c2.x + d * p.x,
                            a * last.y + b * c1.y + c * c2.y + d * p.y,
                        ));
                    }
                    last = p;
                }
                PathCmd::Close => {
                    if let Some(pl) = cur.as_mut() {
                        pl.closed = true;
                    }
                    flush(&mut cur, &mut out);
                    last = start;
                }
            }
        }
        flush(&mut cur, &mut out);
        out
    }

    /// Tight bounds of the path (curves flattened) after transforming by `m`.
    pub fn bounds(&self, m: &Matrix) -> Option<Rect> {
        Rect::from_points(self.flatten().iter().flat_map(|pl| pl.points.iter()).map(|p| m.apply(*p)))
    }

    /// Nonzero-winding containment, treating every subpath as closed
    /// (matching how Canvas2D/SVG fill open subpaths).
    pub fn contains(&self, p: Point) -> bool {
        let mut winding = 0i32;
        for pl in self.flatten() {
            let pts = &pl.points;
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                if a.y <= p.y {
                    if b.y > p.y && cross(a, b, p) > 0.0 {
                        winding += 1;
                    }
                } else if b.y <= p.y && cross(a, b, p) < 0.0 {
                    winding -= 1;
                }
            }
        }
        winding != 0
    }

    /// Shortest distance from `p` to the path outline.
    pub fn distance_to_outline(&self, p: Point) -> f64 {
        let mut best = f64::INFINITY;
        for pl in self.flatten() {
            let pts = &pl.points;
            let n = if pl.closed { pts.len() } else { pts.len() - 1 };
            for i in 0..n {
                best = best.min(segment_distance(p, pts[i], pts[(i + 1) % pts.len()]));
            }
            if pts.len() == 1 {
                best = best.min(dist(p, pts[0]));
            }
        }
        best
    }
}

fn cross(a: Point, b: Point, p: Point) -> f64 {
    (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y)
}

fn dist(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

pub fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return dist(p, a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    dist(p, Point::new(a.x + t * dx, a.y + t * dy))
}

/// Axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Rect {
    pub min: Point,
    pub max: Point,
}

impl Rect {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect { min: Point::new(x0.min(x1), y0.min(y1)), max: Point::new(x0.max(x1), y0.max(y1)) }
    }

    pub fn from_points(mut pts: impl Iterator<Item = Point>) -> Option<Rect> {
        let first = pts.next()?;
        Some(pts.fold(Rect { min: first, max: first }, Rect::include))
    }

    pub fn include(self, p: Point) -> Rect {
        Rect {
            min: Point::new(self.min.x.min(p.x), self.min.y.min(p.y)),
            max: Point::new(self.max.x.max(p.x), self.max.y.max(p.y)),
        }
    }

    pub fn union(self, o: Rect) -> Rect {
        self.include(o.min).include(o.max)
    }

    pub fn union_all(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
        rects.reduce(Rect::union)
    }

    pub fn inflate(self, d: f64) -> Rect {
        Rect { min: Point::new(self.min.x - d, self.min.y - d), max: Point::new(self.max.x + d, self.max.y + d) }
    }

    pub fn translate(self, dx: f64, dy: f64) -> Rect {
        Rect { min: Point::new(self.min.x + dx, self.min.y + dy), max: Point::new(self.max.x + dx, self.max.y + dy) }
    }

    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }

    pub fn center(&self) -> Point {
        Point::new((self.min.x + self.max.x) / 2.0, (self.min.y + self.max.y) / 2.0)
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.min.x <= o.max.x && o.min.x <= self.max.x && self.min.y <= o.max.y && o.min.y <= self.max.y
    }

    pub fn contains_rect(&self, o: &Rect) -> bool {
        self.min.x <= o.min.x && self.min.y <= o.min.y && o.max.x <= self.max.x && o.max.y <= self.max.y
    }

    /// The rect's corners mapped by `m` (TL, TR, BR, BL).
    pub fn corners(&self, m: &Matrix) -> [Point; 4] {
        [
            m.apply(self.min),
            m.apply(Point::new(self.max.x, self.min.y)),
            m.apply(self.max),
            m.apply(Point::new(self.min.x, self.max.y)),
        ]
    }

    /// Bounds of this rect after transforming by `m`.
    pub fn transformed(&self, m: &Matrix) -> Rect {
        Rect::from_points(self.corners(m).into_iter()).expect("four corners")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipse_passes_through_axis_points_and_bounds() {
        let e = Path::ellipse(10.0, 20.0, 5.0, 3.0);
        assert_eq!(e.cmds.len(), 6);
        assert_eq!(e.cmds[0], PathCmd::MoveTo(Point::new(15.0, 20.0)));
        let b = e.bounds(&Matrix::IDENTITY).unwrap();
        assert!((b.min.x - 5.0).abs() < 1e-9 && (b.max.x - 15.0).abs() < 1e-9);
        assert!((b.min.y - 17.0).abs() < 1e-3 && (b.max.y - 23.0).abs() < 1e-3);
    }

    #[test]
    fn rotated_ellipse_bounds_are_tight() {
        // A circle's bounds are rotation-invariant; the control hull's are not.
        let c = Path::ellipse(0.0, 0.0, 10.0, 10.0);
        let b = c.bounds(&Matrix::rotate(45.0)).unwrap();
        assert!((b.width() - 20.0).abs() < 0.05, "width {}", b.width());
    }

    #[test]
    fn rect_bounds_under_transform() {
        let r = Path::rect(0.0, 0.0, 10.0, 4.0);
        let b = r.bounds(&(Matrix::translate(1.0, 1.0) * Matrix::scale(2.0, 2.0))).unwrap();
        assert_eq!((b.min, b.max), (Point::new(1.0, 1.0), Point::new(21.0, 9.0)));
        assert!(Path::default().bounds(&Matrix::IDENTITY).is_none());
    }

    #[test]
    fn containment_and_outline_distance() {
        let e = Path::ellipse(0.0, 0.0, 10.0, 5.0);
        assert!(e.contains(Point::new(0.0, 0.0)));
        assert!(e.contains(Point::new(9.0, 0.0)));
        assert!(!e.contains(Point::new(9.0, 4.0)));
        let l = Path::line(0.0, 0.0, 10.0, 0.0);
        assert!((l.distance_to_outline(Point::new(5.0, 3.0)) - 3.0).abs() < 1e-12);
        assert!((l.distance_to_outline(Point::new(13.0, 4.0)) - 5.0).abs() < 1e-12);
        let r = Path::rect(0.0, 0.0, 10.0, 10.0);
        assert!((r.distance_to_outline(Point::new(5.0, 4.0)) - 4.0).abs() < 1e-12);
    }
}
