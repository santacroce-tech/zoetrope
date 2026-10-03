//! Paints (solid / linear / radial gradients), strokes and fill rules.
//!
//! Gradient geometry is in the shape's content coordinates, so gradients move,
//! rotate and scale with their shape. `PaintStyle` is the geometry-free form
//! used by tools (palette, eyedropper, paint bucket); `fit` turns it into a
//! concrete `Paint` spanning a shape's bounds.

use crate::color::{Color, ColorTransform};
use crate::error::{Error, Result};
use crate::geom::Rect;
use crate::math::Point;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    /// 0..=1 along the gradient.
    pub offset: f64,
    pub color: Color,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Paint {
    Solid {
        color: Color,
    },
    /// Color varies along `start` → `end`; padded beyond the ends.
    Linear {
        start: Point,
        end: Point,
        stops: Vec<GradientStop>,
    },
    /// Color varies from `focal` (default: `center`) out to the circle
    /// (`center`, `radius`); padded beyond it.
    Radial {
        center: Point,
        radius: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focal: Option<Point>,
        stops: Vec<GradientStop>,
    },
}

impl Paint {
    pub fn solid(color: Color) -> Paint {
        Paint::Solid { color }
    }

    pub fn stops(&self) -> &[GradientStop] {
        match self {
            Paint::Solid { .. } => &[],
            Paint::Linear { stops, .. } | Paint::Radial { stops, .. } => stops,
        }
    }

    /// Applies a color transform to every color in the paint.
    pub fn transformed(&self, ct: &ColorTransform) -> Paint {
        let map = |stops: &[GradientStop]| -> Vec<GradientStop> {
            stops.iter().map(|s| GradientStop { offset: s.offset, color: ct.apply(s.color) }).collect()
        };
        match self {
            Paint::Solid { color } => Paint::Solid { color: ct.apply(*color) },
            Paint::Linear { start, end, stops } => Paint::Linear { start: *start, end: *end, stops: map(stops) },
            Paint::Radial { center, radius, focal, stops } => {
                Paint::Radial { center: *center, radius: *radius, focal: *focal, stops: map(stops) }
            }
        }
    }

    pub fn is_invisible(&self) -> bool {
        match self {
            Paint::Solid { color } => color.a == 0,
            _ => self.stops().iter().all(|s| s.color.a == 0),
        }
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |m: &str| Err(Error::Invalid(format!("paint: {m}")));
        let finite = |p: &Point| p.x.is_finite() && p.y.is_finite();
        match self {
            Paint::Solid { .. } => return Ok(()),
            Paint::Linear { start, end, .. } => {
                if !finite(start) || !finite(end) {
                    return bad("gradient points must be finite");
                }
            }
            Paint::Radial { center, radius, focal, .. } => {
                if !(finite(center) && focal.as_ref().is_none_or(finite) && radius.is_finite() && *radius > 0.0) {
                    return bad("radial gradient needs a finite center and a positive radius");
                }
            }
        }
        let stops = self.stops();
        if stops.is_empty() {
            return bad("a gradient needs at least one stop");
        }
        if stops.iter().any(|s| !(0.0..=1.0).contains(&s.offset)) {
            return bad("stop offsets must be within 0..1");
        }
        if stops.windows(2).any(|w| w[1].offset < w[0].offset) {
            return bad("stop offsets must be in ascending order");
        }
        Ok(())
    }

    /// The geometry-free style of this paint.
    pub fn style(&self) -> PaintStyle {
        match self {
            Paint::Solid { color } => PaintStyle::Solid { color: *color },
            Paint::Linear { stops, .. } => PaintStyle::Linear { stops: stops.clone() },
            Paint::Radial { stops, .. } => PaintStyle::Radial { stops: stops.clone() },
        }
    }

    /// Representative color (solid color, or the first stop).
    pub fn primary_color(&self) -> Color {
        match self {
            Paint::Solid { color } => *color,
            _ => self.stops().first().map_or(Color::BLACK, |s| s.color),
        }
    }
}

/// A paint without geometry, as held by tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PaintStyle {
    Solid { color: Color },
    Linear { stops: Vec<GradientStop> },
    Radial { stops: Vec<GradientStop> },
}

impl PaintStyle {
    /// Concrete paint spanning `bounds` (content coords): linear left→right
    /// through the middle, radial from the center out to the farther half-side.
    pub fn fit(&self, bounds: Rect) -> Paint {
        let c = bounds.center();
        match self {
            PaintStyle::Solid { color } => Paint::Solid { color: *color },
            PaintStyle::Linear { stops } => {
                Paint::Linear { start: Point::new(bounds.min.x, c.y), end: Point::new(bounds.max.x, c.y), stops: stops.clone() }
            }
            PaintStyle::Radial { stops } => Paint::Radial {
                center: c,
                radius: (bounds.width().max(bounds.height()) / 2.0).max(1e-3),
                focal: None,
                stops: stops.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineCap {
    Butt,
    #[default]
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineJoin {
    Miter,
    #[default]
    Round,
    Bevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

fn default_miter_limit() -> f64 {
    4.0
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    /// In content units (scales with the element, like Flash's "normal" scaling).
    pub width: f64,
    pub paint: Paint,
    #[serde(default)]
    pub cap: LineCap,
    #[serde(default)]
    pub join: LineJoin,
    #[serde(default = "default_miter_limit")]
    pub miter_limit: f64,
    /// Alternating dash/gap lengths in content units; empty = solid line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dash: Vec<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dash_offset: f64,
}

impl Stroke {
    pub fn solid(width: f64, color: Color) -> Stroke {
        Stroke {
            width,
            paint: Paint::solid(color),
            cap: LineCap::default(),
            join: LineJoin::default(),
            miter_limit: default_miter_limit(),
            dash: Vec::new(),
            dash_offset: 0.0,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if !(self.width.is_finite() && self.width >= 0.0) {
            return Err(Error::Invalid("stroke width must be >= 0".into()));
        }
        if !(self.miter_limit.is_finite() && self.miter_limit >= 1.0) {
            return Err(Error::Invalid("miter limit must be >= 1".into()));
        }
        if self.dash.iter().any(|d| !(d.is_finite() && *d >= 0.0)) || !self.dash_offset.is_finite() {
            return Err(Error::Invalid("dash lengths must be finite and >= 0".into()));
        }
        if !self.dash.is_empty() && self.dash.iter().all(|d| *d == 0.0) {
            return Err(Error::Invalid("a dash pattern needs a non-zero length".into()));
        }
        self.paint.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_fits_to_bounds() {
        let stops = vec![GradientStop { offset: 0.0, color: Color::BLACK }, GradientStop { offset: 1.0, color: Color::WHITE }];
        let b = Rect::new(-10.0, -5.0, 30.0, 15.0);
        assert_eq!(
            PaintStyle::Linear { stops: stops.clone() }.fit(b),
            Paint::Linear { start: Point::new(-10.0, 5.0), end: Point::new(30.0, 5.0), stops: stops.clone() }
        );
        let Paint::Radial { center, radius, .. } = PaintStyle::Radial { stops }.fit(b) else { panic!() };
        assert_eq!((center, radius), (Point::new(10.0, 5.0), 20.0));
    }

    #[test]
    fn validation() {
        let s = |offset| GradientStop { offset, color: Color::BLACK };
        let lin = |stops| Paint::Linear { start: Point::default(), end: Point::new(1.0, 0.0), stops };
        assert!(lin(vec![s(0.0), s(1.0)]).validate().is_ok());
        assert!(lin(vec![]).validate().is_err());
        assert!(lin(vec![s(0.5), s(0.2)]).validate().is_err());
        assert!(lin(vec![s(1.5)]).validate().is_err());
        let mut st = Stroke::solid(2.0, Color::BLACK);
        st.dash = vec![0.0, 0.0];
        assert!(st.validate().is_err());
        st.dash = vec![4.0, 2.0];
        assert!(st.validate().is_ok());
    }
}
