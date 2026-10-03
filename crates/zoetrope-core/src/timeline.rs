//! Timelines: keyframes, tweens, easing, and frame evaluation.
//!
//! A content layer is a sequence of keyframes. Each keyframe holds the
//! layer's elements for a span of `duration` frames; spans are contiguous
//! from frame 0, so a keyframe's start is the sum of the earlier durations
//! (stored data never contains starts, so it can't contradict itself).
//! Frames past the last span show nothing on that layer.
//!
//! A keyframe with a `tween` interpolates its elements toward the matching
//! elements of the *next* keyframe over its span. Elements are matched by
//! `track` (copies made by "insert keyframe" share their original's track);
//! unmatched elements are shown as-is.

use crate::color::Color;
use crate::model::*;
use crate::paint::{GradientStop, Paint, Stroke};
use crate::vector::{Node, NodeKind, SubPath, VectorPath};
use crate::math::Point;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Keyframe {
    /// Frames this keyframe spans (≥ 1).
    pub duration: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<Element>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tween: Option<Tween>,
}

impl Keyframe {
    pub fn blank(duration: u32) -> Keyframe {
        Keyframe { duration, elements: Vec::new(), tween: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TweenKind {
    /// Interpolates transform, pivot, opacity and tint ("classic tween").
    Motion,
    /// Also interpolates shape geometry, paints and strokes.
    Shape,
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tween {
    pub kind: TweenKind,
    #[serde(default)]
    pub easing: Easing,
    /// Extra full turns added to the rotation (positive = clockwise).
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub rotate: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EasePreset {
    EaseInQuad,
    EaseOutQuad,
    EaseInOutQuad,
    EaseInCubic,
    EaseOutCubic,
    EaseInOutCubic,
    EaseInSine,
    EaseOutSine,
    EaseInOutSine,
    EaseInBack,
    EaseOutBack,
    EaseInOutBack,
    EaseInBounce,
    EaseOutBounce,
    EaseInOutBounce,
    EaseInElastic,
    EaseOutElastic,
    EaseInOutElastic,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Easing {
    #[default]
    Linear,
    Preset {
        name: EasePreset,
    },
    /// CSS-style `cubic-bezier(x1, y1, x2, y2)`; x1/x2 in 0..=1.
    Bezier {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
}

impl Easing {
    /// Maps linear progress `t` (0..=1) to eased progress. May overshoot
    /// 0..=1 for back/elastic/bezier curves.
    pub fn apply(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match *self {
            Easing::Linear => t,
            Easing::Preset { name } => preset(name, t),
            Easing::Bezier { x1, y1, x2, y2 } => cubic_bezier(x1.clamp(0.0, 1.0), y1, x2.clamp(0.0, 1.0), y2, t),
        }
    }

    pub fn validate(&self) -> crate::Result<()> {
        match *self {
            Easing::Bezier { x1, y1, x2, y2 } if ![x1, y1, x2, y2].iter().all(|v| v.is_finite()) || !(0.0..=1.0).contains(&x1) || !(0.0..=1.0).contains(&x2) => {
                Err(crate::Error::Invalid("bezier easing needs finite values with x1, x2 in 0..1".into()))
            }
            _ => Ok(()),
        }
    }
}

fn preset(p: EasePreset, t: f64) -> f64 {
    use std::f64::consts::PI;
    use EasePreset::*;
    let bounce_out = |t: f64| {
        let (n1, d1) = (7.5625, 2.75);
        if t < 1.0 / d1 {
            n1 * t * t
        } else if t < 2.0 / d1 {
            let t = t - 1.5 / d1;
            n1 * t * t + 0.75
        } else if t < 2.5 / d1 {
            let t = t - 2.25 / d1;
            n1 * t * t + 0.9375
        } else {
            let t = t - 2.625 / d1;
            n1 * t * t + 0.984375
        }
    };
    let (c1, c2, c3) = (1.70158, 1.70158 * 1.525, 2.70158);
    let (c4, c5) = (2.0 * PI / 3.0, 2.0 * PI / 4.5);
    match p {
        EaseInQuad => t * t,
        EaseOutQuad => 1.0 - (1.0 - t) * (1.0 - t),
        EaseInOutQuad => if t < 0.5 { 2.0 * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(2) / 2.0 },
        EaseInCubic => t * t * t,
        EaseOutCubic => 1.0 - (1.0 - t).powi(3),
        EaseInOutCubic => if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 },
        EaseInSine => 1.0 - (t * PI / 2.0).cos(),
        EaseOutSine => (t * PI / 2.0).sin(),
        EaseInOutSine => -((PI * t).cos() - 1.0) / 2.0,
        EaseInBack => c3 * t * t * t - c1 * t * t,
        EaseOutBack => 1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2),
        EaseInOutBack => {
            if t < 0.5 {
                (2.0 * t).powi(2) * ((c2 + 1.0) * 2.0 * t - c2) / 2.0
            } else {
                ((2.0 * t - 2.0).powi(2) * ((c2 + 1.0) * (t * 2.0 - 2.0) + c2) + 2.0) / 2.0
            }
        }
        EaseInBounce => 1.0 - bounce_out(1.0 - t),
        EaseOutBounce => bounce_out(t),
        EaseInOutBounce => if t < 0.5 { (1.0 - bounce_out(1.0 - 2.0 * t)) / 2.0 } else { (1.0 + bounce_out(2.0 * t - 1.0)) / 2.0 },
        EaseInElastic => match t {
            0.0 => 0.0,
            1.0 => 1.0,
            _ => -(2f64.powf(10.0 * t - 10.0)) * ((10.0 * t - 10.75) * c4).sin(),
        },
        EaseOutElastic => match t {
            0.0 => 0.0,
            1.0 => 1.0,
            _ => 2f64.powf(-10.0 * t) * ((10.0 * t - 0.75) * c4).sin() + 1.0,
        },
        EaseInOutElastic => match t {
            0.0 => 0.0,
            1.0 => 1.0,
            _ if t < 0.5 => -(2f64.powf(20.0 * t - 10.0) * ((20.0 * t - 11.125) * c5).sin()) / 2.0,
            _ => 2f64.powf(-20.0 * t + 10.0) * ((20.0 * t - 11.125) * c5).sin() / 2.0 + 1.0,
        },
    }
}

/// CSS cubic-bezier timing: find s with x(s) = t (Newton, then bisection), return y(s).
fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let curve = |a: f64, b: f64, s: f64| 3.0 * (1.0 - s) * (1.0 - s) * s * a + 3.0 * (1.0 - s) * s * s * b + s * s * s;
    let slope = |a: f64, b: f64, s: f64| 3.0 * (1.0 - s) * (1.0 - s) * a + 6.0 * (1.0 - s) * s * (b - a) + 3.0 * s * s * (1.0 - b);
    let mut s = t;
    for _ in 0..8 {
        let err = curve(x1, x2, s) - t;
        if err.abs() < 1e-9 {
            return curve(y1, y2, s);
        }
        let d = slope(x1, x2, s);
        if d.abs() < 1e-9 {
            break;
        }
        s -= err / d;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    s = t;
    for _ in 0..60 {
        let x = curve(x1, x2, s);
        if (x - t).abs() < 1e-10 {
            break;
        }
        if x < t {
            lo = s
        } else {
            hi = s
        }
        s = (lo + hi) / 2.0;
    }
    curve(y1, y2, s)
}

// ------------------------------------------------------------------ layer timing

impl Layer {
    /// Total frames covered by this layer's keyframes.
    pub fn length(&self) -> u32 {
        self.keyframes.iter().map(|k| k.duration).sum()
    }

    /// Start frame of keyframe `i`.
    pub fn keyframe_start(&self, i: usize) -> u32 {
        self.keyframes[..i].iter().map(|k| k.duration).sum()
    }

    /// The keyframe spanning `frame`: `(index, start)`.
    pub fn keyframe_at(&self, frame: u32) -> Option<(usize, u32)> {
        let mut start = 0;
        for (i, k) in self.keyframes.iter().enumerate() {
            if frame < start + k.duration {
                return Some((i, start));
            }
            start += k.duration;
        }
        None
    }

    /// Every element in every keyframe.
    pub fn all_elements(&self) -> impl Iterator<Item = &Element> {
        self.keyframes.iter().flat_map(|k| k.elements.iter())
    }
}

impl Symbol {
    /// Frames in this symbol's timeline (at least 1).
    pub fn length(&self) -> u32 {
        let mut n = 1;
        walk_layers(&self.layers, &mut |l| n = n.max(l.length()));
        n
    }
}

/// True if `frame` on this layer is inside a tween but not on a keyframe,
/// i.e. what's displayed is interpolated and can't be edited directly.
pub fn is_tweened_frame(layer: &Layer, frame: u32) -> bool {
    match layer.keyframe_at(frame) {
        Some((i, start)) => frame > start && layer.keyframes[i].tween.is_some() && i + 1 < layer.keyframes.len(),
        None => false,
    }
}

/// The elements a layer shows at `frame` (interpolated when tweened). An
/// interpolated element keeps the id of its source keyframe's element.
pub fn evaluate_layer(layer: &Layer, frame: u32) -> Vec<Cow<'_, Element>> {
    let Some((i, start)) = layer.keyframe_at(frame) else { return Vec::new() };
    let kf = &layer.keyframes[i];
    let (Some(tween), Some(next)) = (&kf.tween, layer.keyframes.get(i + 1)) else {
        return kf.elements.iter().map(Cow::Borrowed).collect();
    };
    if frame == start {
        return kf.elements.iter().map(Cow::Borrowed).collect();
    }
    let t = tween.easing.apply((frame - start) as f64 / kf.duration as f64);
    kf.elements
        .iter()
        .map(|a| match next.elements.iter().find(|b| b.track() == a.track()) {
            Some(b) => Cow::Owned(interpolate(a, b, t, tween)),
            None => Cow::Borrowed(a),
        })
        .collect()
}

// ------------------------------------------------------------------ interpolation

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn lerp_pt(a: Point, b: Point, t: f64) -> Point {
    Point::new(lerp(a.x, b.x, t), lerp(a.y, b.y, t))
}

fn lerp_color(a: Color, b: Color, t: f64) -> Color {
    let c = |x: u8, y: u8| lerp(x as f64, y as f64, t).round().clamp(0.0, 255.0) as u8;
    Color::rgba(c(a.r, b.r), c(a.g, b.g), c(a.b, b.b), c(a.a, b.a))
}

/// Interpolates element `a` toward `b` at eased progress `t`.
pub fn interpolate(a: &Element, b: &Element, t: f64, tween: &Tween) -> Element {
    let (ta, tb) = (&a.transform, &b.transform);
    let transform = Transform {
        x: lerp(ta.x, tb.x, t),
        y: lerp(ta.y, tb.y, t),
        scale_x: lerp(ta.scale_x, tb.scale_x, t),
        scale_y: lerp(ta.scale_y, tb.scale_y, t),
        rotation: ta.rotation + (tb.rotation - ta.rotation + 360.0 * tween.rotate as f64) * t,
        skew_x: lerp(ta.skew_x, tb.skew_x, t),
        skew_y: lerp(ta.skew_y, tb.skew_y, t),
        pivot_x: lerp(ta.pivot_x, tb.pivot_x, t),
        pivot_y: lerp(ta.pivot_y, tb.pivot_y, t),
    };
    let tint = match (a.tint, b.tint) {
        (None, None) => None,
        (ka, kb) => {
            let none = |other: Tint| Tint { color: other.color, amount: 0.0 };
            let (ka, kb) = (ka.unwrap_or_else(|| none(kb.unwrap())), kb.unwrap_or_else(|| none(ka.unwrap())));
            Some(Tint { color: lerp_color(ka.color, kb.color, t), amount: lerp(ka.amount, kb.amount, t) })
        }
    };
    let kind = match (&a.kind, &b.kind, tween.kind) {
        (ElementKind::Shape(sa), ElementKind::Shape(sb), TweenKind::Shape) => ElementKind::Shape(lerp_shape(sa, sb, t)),
        _ => a.kind.clone(),
    };
    Element {
        id: a.id,
        track: a.track,
        name: a.name.clone(),
        transform,
        opacity: lerp(a.opacity, b.opacity, t).clamp(0.0, 1.0),
        blend: a.blend,
        tint,
        kind,
    }
}

fn lerp_shape(a: &Shape, b: &Shape, t: f64) -> Shape {
    let geometry = match (&a.geometry, &b.geometry) {
        (Geometry::Rect { width: w0, height: h0 }, Geometry::Rect { width: w1, height: h1 }) => {
            Geometry::Rect { width: lerp(*w0, *w1, t), height: lerp(*h0, *h1, t) }
        }
        (Geometry::Ellipse { width: w0, height: h0 }, Geometry::Ellipse { width: w1, height: h1 }) => {
            Geometry::Ellipse { width: lerp(*w0, *w1, t), height: lerp(*h0, *h1, t) }
        }
        (Geometry::Line { dx: x0, dy: y0 }, Geometry::Line { dx: x1, dy: y1 }) => Geometry::Line { dx: lerp(*x0, *x1, t), dy: lerp(*y0, *y1, t) },
        (ga, gb) => match morph(&ga.to_vector_path(), &gb.to_vector_path(), t) {
            Some(p) => Geometry::Path(p),
            None => if t < 0.5 { ga.clone() } else { gb.clone() },
        },
    };
    let fill = match (&a.fill, &b.fill) {
        (Some(pa), Some(pb)) => Some(lerp_paint(pa, pb, t)),
        (Some(pa), None) => Some(fade(pa, 1.0 - t)),
        (None, Some(pb)) => Some(fade(pb, t)),
        (None, None) => None,
    };
    let stroke = match (&a.stroke, &b.stroke) {
        (Some(sa), Some(sb)) => Some(Stroke {
            width: lerp(sa.width, sb.width, t),
            paint: lerp_paint(&sa.paint, &sb.paint, t),
            miter_limit: lerp(sa.miter_limit, sb.miter_limit, t),
            dash: if sa.dash.len() == sb.dash.len() {
                sa.dash.iter().zip(&sb.dash).map(|(x, y)| lerp(*x, *y, t)).collect()
            } else if t < 0.5 {
                sa.dash.clone()
            } else {
                sb.dash.clone()
            },
            dash_offset: lerp(sa.dash_offset, sb.dash_offset, t),
            ..if t < 0.5 { sa.clone() } else { sb.clone() }
        }),
        (Some(s), None) => Some(Stroke { width: s.width * (1.0 - t), ..s.clone() }),
        (None, Some(s)) => Some(Stroke { width: s.width * t, ..s.clone() }),
        (None, None) => None,
    };
    Shape { geometry, fill, stroke, fill_rule: if t < 0.5 { a.fill_rule } else { b.fill_rule } }
}

/// A paint with its alpha scaled by `k` (for fills appearing/disappearing).
fn fade(p: &Paint, k: f64) -> Paint {
    let f = |c: Color| Color { a: (c.a as f64 * k).round() as u8, ..c };
    match p {
        Paint::Solid { color } => Paint::Solid { color: f(*color) },
        Paint::Linear { start, end, stops } => Paint::Linear { start: *start, end: *end, stops: stops.iter().map(|s| GradientStop { color: f(s.color), ..*s }).collect() },
        Paint::Radial { center, radius, focal, stops } => {
            Paint::Radial { center: *center, radius: *radius, focal: *focal, stops: stops.iter().map(|s| GradientStop { color: f(s.color), ..*s }).collect() }
        }
    }
}

fn lerp_stops(a: &[GradientStop], b: &[GradientStop], t: f64) -> Option<Vec<GradientStop>> {
    (a.len() == b.len()).then(|| a.iter().zip(b).map(|(x, y)| GradientStop { offset: lerp(x.offset, y.offset, t), color: lerp_color(x.color, y.color, t) }).collect())
}

fn lerp_paint(a: &Paint, b: &Paint, t: f64) -> Paint {
    match (a, b) {
        (Paint::Solid { color: ca }, Paint::Solid { color: cb }) => Paint::Solid { color: lerp_color(*ca, *cb, t) },
        (Paint::Linear { start: s0, end: e0, stops: st0 }, Paint::Linear { start: s1, end: e1, stops: st1 }) => match lerp_stops(st0, st1, t) {
            Some(stops) => Paint::Linear { start: lerp_pt(*s0, *s1, t), end: lerp_pt(*e0, *e1, t), stops },
            None => if t < 0.5 { a.clone() } else { b.clone() },
        },
        (Paint::Radial { center: c0, radius: r0, focal: f0, stops: st0 }, Paint::Radial { center: c1, radius: r1, focal: f1, stops: st1 }) => {
            match lerp_stops(st0, st1, t) {
                Some(stops) => Paint::Radial {
                    center: lerp_pt(*c0, *c1, t),
                    radius: lerp(*r0, *r1, t),
                    focal: match (f0, f1) {
                        (None, None) => None,
                        _ => Some(lerp_pt(f0.unwrap_or(*c0), f1.unwrap_or(*c1), t)),
                    },
                    stops,
                },
                None => if t < 0.5 { a.clone() } else { b.clone() },
            }
        }
        // A solid color tweens against a gradient as if it were a gradient of that color.
        (Paint::Solid { color }, g) | (g, Paint::Solid { color }) if !g.stops().is_empty() => {
            let solid_first = matches!(a, Paint::Solid { .. });
            let as_gradient = with_stops(g, g.stops().iter().map(|s| GradientStop { offset: s.offset, color: *color }).collect());
            if solid_first { lerp_paint(&as_gradient, b, t) } else { lerp_paint(a, &as_gradient, t) }
        }
        _ => if t < 0.5 { a.clone() } else { b.clone() },
    }
}

fn with_stops(p: &Paint, stops: Vec<GradientStop>) -> Paint {
    match p {
        Paint::Linear { start, end, .. } => Paint::Linear { start: *start, end: *end, stops },
        Paint::Radial { center, radius, focal, .. } => Paint::Radial { center: *center, radius: *radius, focal: *focal, stops },
        Paint::Solid { .. } => p.clone(),
    }
}

// ------------------------------------------------------------------ path morphing

/// All-cubic copy of a subpath (every handle present), so lines and curves
/// can be interpolated uniformly.
fn as_cubic(sp: &SubPath) -> SubPath {
    let nodes = sp
        .nodes
        .iter()
        .map(|n| Node { handle_in: Some(n.handle_in.unwrap_or(n.point())), handle_out: Some(n.handle_out.unwrap_or(n.point())), kind: NodeKind::Corner, ..*n })
        .collect();
    SubPath { nodes, closed: sp.closed }
}

/// Splits the longest segments of `sp` until it has `n` nodes.
fn grow_to(sp: &mut SubPath, n: usize) {
    while sp.nodes.len() < n && sp.segment_count() > 0 {
        let longest = (0..sp.segment_count())
            .max_by(|&i, &j| {
                let len = |k: usize| {
                    let (p0, _, _, p3, _) = sp.segment(k);
                    (p3.x - p0.x).hypot(p3.y - p0.y)
                };
                len(i).total_cmp(&len(j)).then(j.cmp(&i))
            })
            .unwrap();
        let mut v = VectorPath { subpaths: vec![sp.clone()] };
        if v.split(0, longest, 0.5).is_err() {
            break;
        }
        *sp = as_cubic(&v.subpaths[0]);
    }
}

/// Rotates a closed subpath's node order to best match `reference`.
fn align_start(sp: &mut SubPath, reference: &SubPath) {
    if !sp.closed || sp.nodes.len() != reference.nodes.len() {
        return;
    }
    let n = sp.nodes.len();
    let cost = |shift: usize| -> f64 {
        (0..n)
            .map(|i| {
                let (a, b) = (sp.nodes[(i + shift) % n].point(), reference.nodes[i].point());
                (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
            })
            .sum()
    };
    let best = (0..n).min_by(|&i, &j| cost(i).total_cmp(&cost(j)).then(i.cmp(&j))).unwrap_or(0);
    sp.nodes.rotate_left(best);
}

/// Best-effort morph between two paths. `None` if their subpath structure
/// can't be paired (different subpath counts or open/closed mismatch).
pub fn morph(a: &VectorPath, b: &VectorPath, t: f64) -> Option<VectorPath> {
    if a.subpaths.len() != b.subpaths.len() {
        return None;
    }
    let mut out = VectorPath::default();
    for (sa, sb) in a.subpaths.iter().zip(&b.subpaths) {
        if sa.closed != sb.closed {
            return None;
        }
        let (mut ca, mut cb) = (as_cubic(sa), as_cubic(sb));
        let n = ca.nodes.len().max(cb.nodes.len());
        grow_to(&mut ca, n);
        grow_to(&mut cb, n);
        if ca.nodes.len() != cb.nodes.len() {
            return None;
        }
        align_start(&mut cb, &ca);
        let nodes = ca
            .nodes
            .iter()
            .zip(&cb.nodes)
            .map(|(x, y)| {
                let p = lerp_pt(x.point(), y.point(), t);
                Node {
                    x: p.x,
                    y: p.y,
                    handle_in: Some(lerp_pt(x.handle_in.unwrap(), y.handle_in.unwrap(), t)),
                    handle_out: Some(lerp_pt(x.handle_out.unwrap(), y.handle_out.unwrap(), t)),
                    kind: NodeKind::Corner,
                }
            })
            .collect();
        out.subpaths.push(SubPath { nodes, closed: sa.closed });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_endpoints_and_shapes() {
        let all = [
            EasePreset::EaseInQuad, EasePreset::EaseOutQuad, EasePreset::EaseInOutQuad, EasePreset::EaseInCubic, EasePreset::EaseOutCubic,
            EasePreset::EaseInOutCubic, EasePreset::EaseInSine, EasePreset::EaseOutSine, EasePreset::EaseInOutSine, EasePreset::EaseInBack,
            EasePreset::EaseOutBack, EasePreset::EaseInOutBack, EasePreset::EaseInBounce, EasePreset::EaseOutBounce, EasePreset::EaseInOutBounce,
            EasePreset::EaseInElastic, EasePreset::EaseOutElastic, EasePreset::EaseInOutElastic,
        ];
        for name in all {
            let e = Easing::Preset { name };
            assert!(e.apply(0.0).abs() < 1e-9, "{name:?}(0)");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-9, "{name:?}(1)");
        }
        assert!(Easing::Preset { name: EasePreset::EaseInQuad }.apply(0.5) < 0.5);
        assert!(Easing::Preset { name: EasePreset::EaseOutQuad }.apply(0.5) > 0.5);
        assert!(Easing::Preset { name: EasePreset::EaseInBack }.apply(0.2) < 0.0, "back undershoots");
        assert!(Easing::Preset { name: EasePreset::EaseOutElastic }.apply(0.15) > 1.0, "elastic overshoots");
        assert_eq!(Easing::Linear.apply(0.3), 0.3);
    }

    #[test]
    fn bezier_easing_matches_css() {
        // CSS "ease" = cubic-bezier(0.25, 0.1, 0.25, 1); at x=0.5, y ≈ 0.8024.
        let ease = Easing::Bezier { x1: 0.25, y1: 0.1, x2: 0.25, y2: 1.0 };
        assert!((ease.apply(0.5) - 0.8024).abs() < 1e-3, "{}", ease.apply(0.5));
        // Linear control points reproduce linear timing.
        let lin = Easing::Bezier { x1: 1.0 / 3.0, y1: 1.0 / 3.0, x2: 2.0 / 3.0, y2: 2.0 / 3.0 };
        for t in [0.1, 0.37, 0.9] {
            assert!((lin.apply(t) - t).abs() < 1e-6);
        }
        assert!(Easing::Bezier { x1: 1.5, y1: 0.0, x2: 0.5, y2: 1.0 }.validate().is_err());
    }

    #[test]
    fn morph_rect_to_ellipse_is_continuous() {
        let a = VectorPath::rect(100.0, 100.0);
        let b = VectorPath::ellipse(60.0, 60.0);
        let m0 = morph(&a, &b, 0.0).unwrap();
        let m1 = morph(&a, &b, 1.0).unwrap();
        let bounds = |p: &VectorPath| p.to_path().bounds(&crate::Matrix::IDENTITY).unwrap();
        assert!((bounds(&m0).width() - 100.0).abs() < 1e-6);
        assert!((bounds(&m1).width() - 60.0).abs() < 0.1);
        let mid = bounds(&morph(&a, &b, 0.5).unwrap()).width();
        assert!(mid > 60.0 && mid < 100.0);
        // Different subpath counts can't be paired.
        let mut two = a.clone();
        two.subpaths.push(a.subpaths[0].clone());
        assert!(morph(&two, &b, 0.5).is_none());
    }

    #[test]
    fn grow_to_adds_nodes_without_changing_outline() {
        let mut sp = as_cubic(&VectorPath::rect(40.0, 10.0).subpaths[0]);
        grow_to(&mut sp, 7);
        assert_eq!(sp.nodes.len(), 7);
        let b = VectorPath { subpaths: vec![sp] }.to_path().bounds(&crate::Matrix::IDENTITY).unwrap();
        assert!((b.width() - 40.0).abs() < 1e-9 && (b.height() - 10.0).abs() < 1e-9);
    }
}
