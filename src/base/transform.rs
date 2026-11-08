//! A 2D affine [`Transform`]: translation, scale, rotation, and their
//! compositions, applied to points. [`Frame::with_transform`] applies one
//! to the vector paths drawn inside it.
//!
//! [`Frame::with_transform`]: crate::Frame::with_transform

use super::geometry::Point;

// ========================================================================
// Data Structures
// ========================================================================

/// A 2D affine transform, mapping a point `(x, y)` to
/// `(a * x + c * y + e, b * x + d * y + f)`. Build one with
/// [`Transform::translate`], [`Transform::scale`], or
/// [`Transform::rotate`], and chain them with [`Transform::then`]:
/// `Transform::rotate(angle).then(Transform::translate(x, y))` rotates
/// about the origin and then moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// How much `x` adds to the new `x`.
    a: f32,
    /// How much `x` adds to the new `y`.
    b: f32,
    /// How much `y` adds to the new `x`.
    c: f32,
    /// How much `y` adds to the new `y`.
    d: f32,
    /// What is added to the new `x`.
    e: f32,
    /// What is added to the new `y`.
    f: f32,
}

// ========================================================================
// Transform Implementations
// ========================================================================

impl Transform {
    /// The transform that leaves every point where it is.
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// Moves points by `dx` to the right and `dy` down, in pixels.
    pub const fn translate(dx: f32, dy: f32) -> Self {
        Self {
            e: dx,
            f: dy,
            ..Self::IDENTITY
        }
    }

    /// Stretches points away from the origin by `sx` horizontally and `sy`
    /// vertically; `scale(2.0, 2.0)` doubles every size.
    pub const fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            d: sy,
            ..Self::IDENTITY
        }
    }

    /// Turns points about the origin by `radians`, clockwise on screen
    /// (where `y` grows downward).
    pub fn rotate(radians: f32) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    /// This transform followed by `next`: a point is moved by `self`
    /// first, then by `next`.
    pub fn then(self, next: Self) -> Self {
        Self {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            e: next.a * self.e + next.c * self.f + next.e,
            f: next.b * self.e + next.d * self.f + next.f,
        }
    }

    /// Where `point` lands.
    pub fn apply(&self, point: Point) -> Point {
        Point::new(
            self.a * point.x + self.c * point.y + self.e,
            self.b * point.x + self.d * point.y + self.f,
        )
    }

    /// The transform that undoes this one, or `None` when it squashes the
    /// plane flat (a scale by 0) and cannot be undone.
    pub fn inverse(&self) -> Option<Self> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let a = self.d / det;
        let b = -self.b / det;
        let c = -self.c / det;
        let d = self.a / det;
        Some(Self {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        })
    }

    /// How much the transform scales lengths on average: the square root
    /// of how much it scales areas. Exact for rotations and uniform scales;
    /// what a stroke's width is scaled by.
    pub(crate) fn length_scale(&self) -> f32 {
        self.determinant().abs().sqrt()
    }

    /// How much the transform scales areas, negative when it mirrors.
    fn determinant(&self) -> f32 {
        self.a * self.d - self.b * self.c
    }
}

impl Default for Transform {
    /// The identity.
    fn default() -> Self {
        Self::IDENTITY
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;

    fn assert_near(actual: Point, expected: Point) {
        let close = (actual.x - expected.x).abs() < 1e-4 && (actual.y - expected.y).abs() < 1e-4;
        assert!(close, "expected {expected:?}, found {actual:?}");
    }

    #[test]
    fn test_rotate_turns_clockwise_on_screen() {
        let turned = Transform::rotate(FRAC_PI_2).apply(Point::new(1.0, 0.0));
        assert_near(turned, Point::new(0.0, 1.0));
    }

    #[test]
    fn test_then_applies_self_first() {
        let point = Point::new(1.0, 0.0);
        let rotate_then_move = Transform::rotate(FRAC_PI_2).then(Transform::translate(10.0, 0.0));
        assert_near(rotate_then_move.apply(point), Point::new(10.0, 1.0));
        let move_then_rotate = Transform::translate(10.0, 0.0).then(Transform::rotate(FRAC_PI_2));
        assert_near(move_then_rotate.apply(point), Point::new(0.0, 11.0));
    }

    #[test]
    fn test_inverse_undoes_a_composition() {
        let transform = Transform::scale(2.0, 3.0)
            .then(Transform::rotate(0.7))
            .then(Transform::translate(5.0, -4.0));
        let Some(inverse) = transform.inverse() else {
            panic!("an invertible transform has an inverse");
        };
        let point = Point::new(3.0, 7.0);
        assert_near(inverse.apply(transform.apply(point)), point);
        assert_eq!(Transform::scale(0.0, 1.0).inverse(), None);
    }

    #[test]
    fn test_length_scale_ignores_rotation() {
        let transform = Transform::rotate(1.1).then(Transform::scale(3.0, 3.0));
        assert!((transform.length_scale() - 3.0).abs() < 1e-4);
    }
}
