//! How a box is drawn: its fill, rounded corners, [`Border`], and soft
//! [`Shadow`], together a [`BoxStyle`]. Sizes are in physical pixels, like
//! every other size a frame draws.

use super::color::Rgba;
use super::geometry::Point;

// ========================================================================
// Data Structures
// ========================================================================

/// The look of a box: a fill, rounded corners, and an optional border and
/// shadow. A style with no radius, border, or shadow draws a plain
/// rectangle, pixel for pixel as a fill does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxStyle {
    /// A line drawn just inside the edge, over the fill; `None` for none.
    pub border: Option<Border>,
    /// The radius of each corner's rounding, in pixels; 0 for square
    /// corners. A radius over half the shorter side is cut to it, which
    /// rounds that side into a half circle.
    pub corner_radius: f32,
    /// The color inside the edge, blended by its alpha; a transparent fill
    /// leaves only the border.
    pub fill: Rgba,
    /// A blurred copy of the box's shape drawn under it; `None` for none.
    pub shadow: Option<Shadow>,
}

/// A line along the inside of a box's edge, following its corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    /// The line's color, blended by its alpha.
    pub color: Rgba,
    /// How thick the line is, in pixels, measured inward from the edge.
    pub width: f32,
}

/// The soft shadow a box casts: its shape, moved by `offset` and blurred.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// How far the shadow fades out, in pixels: it reaches `color` that far
    /// inside the shape's edge and nothing that far outside. 0 for a sharp
    /// copy of the shape.
    pub blur: f32,
    /// The shadow's color at its darkest, blended by its alpha.
    pub color: Rgba,
    /// How far the shadow is moved from the box, in pixels right and down.
    pub offset: Point,
}
