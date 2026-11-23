//! A 2D affine [`Transform`]: translation, scale, rotation, and their
//! compositions, applied to points. [`Frame::with_transform`] applies one
//! to everything drawn inside it.
//!
//! [`Frame::with_transform`]: crate::Frame::with_transform

use super::geometry::{Point, Rect};

// ========================================================================
// Constants
// ========================================================================

/// How small a coefficient may be, next to the transform's largest, and
/// still count as 0 when telling whether a transform keeps rectangles
/// axis-aligned: what float error leaves of a coefficient that cancels.
const ALIGNED_TOLERANCE: f32 = 1e-5;

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

    /// The transform's six numbers, `[a, b, c, d, e, f]` as the type's
    /// docs name them.
    pub(crate) fn coefficients(&self) -> [f32; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }

    /// How far the transform moves points, `(dx, dy)`, when that is all it
    /// does; `None` when it also scales, turns, or mirrors.
    pub(crate) fn translation(&self) -> Option<(f32, f32)> {
        let moves_only = self.a == 1.0 && self.b == 0.0 && self.c == 0.0 && self.d == 1.0;
        moves_only.then_some((self.e, self.f))
    }

    /// Whether the transform maps every axis-aligned rectangle to an
    /// axis-aligned rectangle: it moves, scales, mirrors, or turns by a
    /// quarter turn, but turns by no other angle and skews nothing.
    pub(crate) fn is_axis_aligned(&self) -> bool {
        let largest = self
            .a
            .abs()
            .max(self.b.abs())
            .max(self.c.abs().max(self.d.abs()));
        let tiny = |value: f32| value.abs() <= largest * ALIGNED_TOLERANCE;
        (tiny(self.b) && tiny(self.c)) || (tiny(self.a) && tiny(self.d))
    }

    /// Where the corners of `rect` land: top left, top right, bottom
    /// right, bottom left of the rectangle before it is moved.
    pub(crate) fn corners(&self, rect: Rect) -> [Point; 4] {
        let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);
        [
            Point::new(rect.x, rect.y),
            Point::new(right, rect.y),
            Point::new(right, bottom),
            Point::new(rect.x, bottom),
        ]
        .map(|corner| self.apply(corner))
    }

    /// The smallest axis-aligned rectangle holding `rect` once moved: the
    /// moved rectangle itself when [`Transform::is_axis_aligned`], and
    /// `rect` as it is for the identity.
    pub(crate) fn bounds_of(&self, rect: Rect) -> Rect {
        if *self == Self::IDENTITY {
            return rect;
        }
        let corners = self.corners(rect);
        let (xs, ys) = (
            corners.map(|corner| corner.x),
            corners.map(|corner| corner.y),
        );
        let least = |values: [f32; 4]| values.into_iter().fold(f32::INFINITY, f32::min);
        let most = |values: [f32; 4]| values.into_iter().fold(f32::NEG_INFINITY, f32::max);
        let (left, top, right, bottom) = (least(xs), least(ys), most(xs), most(ys));
        Rect::new(left, top, right - left, bottom - top)
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
    fn test_only_quarter_turns_keep_rectangles_axis_aligned() {
        assert!(Transform::scale(-1.0, 2.0).is_axis_aligned(), "a flip");
        assert!(
            Transform::rotate(FRAC_PI_2).is_axis_aligned(),
            "a quarter turn"
        );
        assert!(!Transform::rotate(0.3).is_axis_aligned());
        let undone = Transform::rotate(0.3).then(Transform::rotate(-0.3));
        assert!(undone.is_axis_aligned(), "float error left by cancelling");
    }

    #[test]
    fn test_bounds_of_a_turned_rect_hold_all_its_corners() {
        let bounds = Transform::rotate(FRAC_PI_2).bounds_of(Rect::new(0.0, 0.0, 10.0, 4.0));
        assert!((bounds.x + 4.0).abs() < 1e-4 && bounds.y.abs() < 1e-4);
        assert!((bounds.width - 4.0).abs() < 1e-4 && (bounds.height - 10.0).abs() < 1e-4);
        assert_eq!(
            Transform::translate(2.0, 3.0).translation(),
            Some((2.0, 3.0))
        );
        assert_eq!(Transform::scale(2.0, 1.0).translation(), None);
    }

    #[test]
    fn test_length_scale_ignores_rotation() {
        let transform = Transform::rotate(1.1).then(Transform::scale(3.0, 3.0));
        assert!((transform.length_scale() - 3.0).abs() < 1e-4);
    }
}
