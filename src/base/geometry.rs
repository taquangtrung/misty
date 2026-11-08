//! Positions and areas inside a window, in physical pixels from its top
//! left corner: [`Point`] and [`Rect`].

// ========================================================================
// Data Structures
// ========================================================================

/// A position in physical pixels, `x` to the right and `y` down from the
/// window's top left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Pixels right of the left edge.
    pub x: f32,
    /// Pixels below the top edge.
    pub y: f32,
}

/// An axis-aligned rectangle in physical pixels: its top left corner and its
/// size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// The height in pixels.
    pub height: f32,
    /// The width in pixels.
    pub width: f32,
    /// The left edge.
    pub x: f32,
    /// The top edge.
    pub y: f32,
}

// ========================================================================
// Point Implementations
// ========================================================================

impl Point {
    /// The point `x` pixels right and `y` pixels down.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

// ========================================================================
// Rect Implementations
// ========================================================================

impl Rect {
    /// The rectangle with its top left corner at `x`, `y`, `width` pixels
    /// wide and `height` pixels high.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            height,
            width,
            x,
            y,
        }
    }

    /// Whether `point` is inside: on the left or top edge counts, on the
    /// right or bottom edge does not, so rectangles side by side never both
    /// hold a point.
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && point.x < self.x + self.width
            && point.y < self.y + self.height
    }

    /// The area both this and `other` cover, or `None` when they do not
    /// overlap (touching edges do not count).
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        (right > left && bottom > top).then(|| Rect::new(left, top, right - left, bottom - top))
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_intersection_is_the_shared_area_and_none_when_apart() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(
            a.intersection(&Rect::new(5.0, -5.0, 10.0, 10.0)),
            Some(Rect::new(5.0, 0.0, 5.0, 5.0))
        );
        assert_eq!(
            a.intersection(&Rect::new(10.0, 0.0, 5.0, 5.0)),
            None,
            "touching"
        );
        assert_eq!(
            a.intersection(&Rect::new(2.0, 2.0, 1.0, 1.0)),
            Some(Rect::new(2.0, 2.0, 1.0, 1.0))
        );
    }
}
