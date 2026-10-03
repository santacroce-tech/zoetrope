use serde::{Deserialize, Serialize};
use std::ops::Mul;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

/// 2D affine matrix in Canvas2D order: maps `(x, y)` to
/// `(a*x + c*y + e, b*x + d*y + f)`.
///
/// `p * q` composes so that `q` is applied first, then `p`
/// (i.e. `(p * q).apply(pt) == p.apply(q.apply(pt))`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Matrix {
    fn default() -> Self {
        Matrix::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn translate(x: f64, y: f64) -> Matrix {
        Matrix { e: x, f: y, ..Matrix::IDENTITY }
    }

    pub fn scale(sx: f64, sy: f64) -> Matrix {
        Matrix { a: sx, d: sy, ..Matrix::IDENTITY }
    }

    /// Rotation in degrees, clockwise on screen (y axis points down).
    pub fn rotate(degrees: f64) -> Matrix {
        let (s, c) = degrees.to_radians().sin_cos();
        Matrix { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }

    /// Skew in degrees. `skew_x` shears x by y, `skew_y` shears y by x.
    pub fn skew(skew_x: f64, skew_y: f64) -> Matrix {
        Matrix { a: 1.0, b: skew_y.to_radians().tan(), c: skew_x.to_radians().tan(), d: 1.0, e: 0.0, f: 0.0 }
    }

    /// The matrix without its translation.
    pub fn linear(&self) -> Matrix {
        Matrix { e: 0.0, f: 0.0, ..*self }
    }

    pub fn apply(&self, p: Point) -> Point {
        Point::new(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f)
    }

    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    pub fn invert(&self) -> Option<Matrix> {
        let det = self.determinant();
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        Some(Matrix {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            e: (self.c * self.f - self.d * self.e) * inv,
            f: (self.b * self.e - self.a * self.f) * inv,
        })
    }

    pub fn approx_eq(&self, o: &Matrix, eps: f64) -> bool {
        [self.a - o.a, self.b - o.b, self.c - o.c, self.d - o.d, self.e - o.e, self.f - o.f].iter().all(|d| d.abs() <= eps)
    }
}

impl Mul for Matrix {
    type Output = Matrix;

    fn mul(self, r: Matrix) -> Matrix {
        Matrix {
            a: self.a * r.a + self.c * r.b,
            b: self.b * r.a + self.d * r.b,
            c: self.a * r.c + self.c * r.d,
            d: self.b * r.c + self.d * r.d,
            e: self.a * r.e + self.c * r.f + self.e,
            f: self.b * r.e + self.d * r.f + self.f,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(p: Point, x: f64, y: f64) -> bool {
        (p.x - x).abs() < 1e-9 && (p.y - y).abs() < 1e-9
    }

    #[test]
    fn composition_applies_right_operand_first() {
        let m = Matrix::translate(10.0, 0.0) * Matrix::scale(2.0, 2.0);
        assert!(close(m.apply(Point::new(1.0, 1.0)), 12.0, 2.0));
    }

    #[test]
    fn rotation_is_clockwise_with_y_down() {
        assert!(close(Matrix::rotate(90.0).apply(Point::new(1.0, 0.0)), 0.0, 1.0));
    }

    #[test]
    fn skew_shears_axes() {
        let m = Matrix::skew(45.0, 0.0);
        assert!(close(m.apply(Point::new(0.0, 1.0)), 1.0, 1.0));
        let m = Matrix::skew(0.0, 45.0);
        assert!(close(m.apply(Point::new(1.0, 0.0)), 1.0, 1.0));
    }

    #[test]
    fn invert_round_trips() {
        let m = Matrix::translate(5.0, -3.0) * Matrix::rotate(33.0) * Matrix::skew(10.0, 4.0) * Matrix::scale(2.0, 0.5);
        let i = m.invert().unwrap();
        assert!((m * i).approx_eq(&Matrix::IDENTITY, 1e-12));
        assert!(Matrix::scale(0.0, 1.0).invert().is_none());
    }
}
