//! How a vector path is painted: the [`Paint`] that fills its area or its
//! outline (one color or a [`LinearGradient`]) and the [`Stroke`] that
//! gives an outline its width, joins, caps, and dashes.

use super::color::{Rgb, Rgba};
use super::geometry::Point;

// ========================================================================
// Constants
// ========================================================================

/// The largest channel value, as a float.
const CHANNEL_MAX: f32 = 255.0;

// ========================================================================
// Data Structures
// ========================================================================

/// What a path is painted with. Misty widgets pass theme colors; an app
/// passes whatever its data calls for. A color converts into a paint, so
/// `frame.fill_path(&path, theme.accent)` works as is.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    /// Colors blended along a line.
    LinearGradient(LinearGradient),
    /// One color, its alpha blending it over what is under it.
    Solid(Rgba),
}

/// Colors blended along the line from `start` to `end`: each point takes
/// the color of its projection on the line, and points beyond either end
/// take that end's color. Its points move with the path under a
/// [`Transform`](crate::Transform).
#[derive(Clone, Debug, PartialEq)]
pub struct LinearGradient {
    /// Where the gradient reaches offset 1, in pixels.
    pub end: Point,
    /// Where the gradient is at offset 0, in pixels.
    pub start: Point,
    /// The colors and where along the line they sit, in increasing
    /// offset order; colors between two stops blend. No stops paints
    /// nothing.
    pub stops: Vec<GradientStop>,
}

/// One color of a [`LinearGradient`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    /// The color at the stop.
    pub color: Rgba,
    /// Where the stop sits along the gradient line, from 0 at its start
    /// to 1 at its end.
    pub offset: f32,
}

/// How a path's outline is drawn: its width, how segments meet, how open
/// ends finish, and an optional dash pattern. Build one with
/// [`Stroke::new`] and the `with_` methods.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    /// How the ends of open subpaths and dashes finish.
    cap: LineCap,
    /// The lengths of dashes and gaps, alternating, or empty for a solid
    /// line.
    dash: Vec<f32>,
    /// How far into the dash pattern the line starts.
    dash_offset: f32,
    /// How segments meet at corners.
    join: LineJoin,
    /// The width, in pixels.
    width: f32,
}

/// How the ends of an open line finish.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineCap {
    /// Cut square at the end point (the default).
    #[default]
    Butt,
    /// A half circle around the end point.
    Round,
    /// Cut square half the width past the end point.
    Square,
}

/// How two segments of a line meet at a corner.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineJoin {
    /// The corner cut off straight.
    Bevel,
    /// The outer edges extended until they meet (the default), cut off as
    /// a bevel at very sharp corners.
    #[default]
    Miter,
    /// The corner rounded.
    Round,
}

// ========================================================================
// Paint Implementations
// ========================================================================

impl From<Rgba> for Paint {
    /// The paint of one color.
    fn from(color: Rgba) -> Self {
        Self::Solid(color)
    }
}

impl From<Rgb> for Paint {
    /// The paint of one opaque color.
    fn from(color: Rgb) -> Self {
        Self::Solid(Rgba::from(color))
    }
}

impl From<LinearGradient> for Paint {
    /// The paint blending `gradient`'s colors.
    fn from(gradient: LinearGradient) -> Self {
        Self::LinearGradient(gradient)
    }
}

// ========================================================================
// LinearGradient Implementations
// ========================================================================

impl LinearGradient {
    /// The color at `offset` along the line: a stop's own color at its
    /// offset, the nearest end stop's beyond the ends, and between two
    /// stops their blend, mixed with alpha premultiplied so a transparent
    /// stop fades a color out without darkening it. Transparent for no
    /// stops.
    pub(crate) fn color_at(&self, offset: f32) -> Rgba {
        let (Some(first), Some(last)) = (self.stops.first(), self.stops.last()) else {
            return Rgba::new(0, 0, 0, 0);
        };
        if offset <= first.offset {
            return first.color;
        }
        let Some(index) = self.stops.iter().position(|stop| stop.offset > offset) else {
            return last.color;
        };
        let (before, after) = (self.stops[index - 1], self.stops[index]);
        let span = after.offset - before.offset;
        let t = (offset - before.offset) / span;
        mix_premultiplied(before.color, after.color, t)
    }
}

// ========================================================================
// Stroke Implementations
// ========================================================================

impl Stroke {
    /// A solid line `width` pixels wide with butt caps and miter joins.
    /// Lines thinner than a pixel are drawn a pixel wide and fainter.
    pub fn new(width: f32) -> Self {
        Self {
            cap: LineCap::default(),
            dash: Vec::new(),
            dash_offset: 0.0,
            join: LineJoin::default(),
            width,
        }
    }

    /// The stroke with its open ends (and its dashes' ends) finished by
    /// `cap`.
    pub fn with_cap(self, cap: LineCap) -> Self {
        Self { cap, ..self }
    }

    /// The stroke with its corners joined by `join`.
    pub fn with_join(self, join: LineJoin) -> Self {
        Self { join, ..self }
    }

    /// The stroke dashed by `pattern`: the lengths, in pixels, of a dash,
    /// a gap, a dash, and so on, repeated along each subpath, starting
    /// `offset` pixels into the pattern. An odd count is repeated to make
    /// it even, as in SVG; a pattern with a negative length or adding up
    /// to nothing draws a solid line.
    pub fn with_dash(self, pattern: &[f32], offset: f32) -> Self {
        let mut dash = pattern.to_vec();
        if dash.len() % 2 == 1 {
            dash.extend_from_slice(pattern);
        }
        let valid = dash.iter().all(|&length| length >= 0.0) && dash.iter().sum::<f32>() > 0.0;
        Self {
            dash: match valid {
                true => dash,
                false => Vec::new(),
            },
            dash_offset: offset,
            ..self
        }
    }

    /// How the ends of open subpaths and dashes finish.
    pub(crate) fn cap(&self) -> LineCap {
        self.cap
    }

    /// The dash and gap lengths, alternating; empty for a solid line.
    pub(crate) fn dash(&self) -> &[f32] {
        &self.dash
    }

    /// How far into the dash pattern the line starts.
    pub(crate) fn dash_offset(&self) -> f32 {
        self.dash_offset
    }

    /// How segments meet at corners.
    pub(crate) fn join(&self) -> LineJoin {
        self.join
    }

    /// The width, in pixels.
    pub(crate) fn width(&self) -> f32 {
        self.width
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `from` blended `t` of the way to `to`, with alpha premultiplied.
fn mix_premultiplied(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let premultiplied = |color: Rgba| {
        let alpha = f32::from(color.a) / CHANNEL_MAX;
        [color.r, color.g, color.b].map(|channel| f32::from(channel) * alpha)
    };
    let (alpha_from, alpha_to) = (f32::from(from.a), f32::from(to.a));
    let alpha = alpha_from + (alpha_to - alpha_from) * t;
    if alpha <= 0.0 {
        return Rgba::new(0, 0, 0, 0);
    }
    let (a, b) = (premultiplied(from), premultiplied(to));
    let unit_alpha = alpha / CHANNEL_MAX;
    let channel = |index: usize| {
        let mixed = a[index] + (b[index] - a[index]) * t;
        // Clamped to the channel range, so the cast cannot wrap.
        (mixed / unit_alpha).round().clamp(0.0, CHANNEL_MAX) as u8
    };
    Rgba::new(
        channel(0),
        channel(1),
        channel(2),
        alpha.round().clamp(0.0, CHANNEL_MAX) as u8,
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(stops: &[(f32, Rgba)]) -> LinearGradient {
        LinearGradient {
            end: Point::new(1.0, 0.0),
            start: Point::new(0.0, 0.0),
            stops: stops
                .iter()
                .map(|&(offset, color)| GradientStop { color, offset })
                .collect(),
        }
    }

    #[test]
    fn test_a_gradient_blends_between_the_stops_around_an_offset() {
        let red = Rgba::rgb(255, 0, 0);
        let green = Rgba::rgb(0, 255, 0);
        let blue = Rgba::rgb(0, 0, 255);
        let three = gradient(&[(0.0, red), (0.5, green), (1.0, blue)]);
        assert_eq!(three.color_at(-1.0), red, "before the first stop");
        assert_eq!(three.color_at(0.5), green, "on a stop");
        assert_eq!(three.color_at(0.75), Rgba::rgb(0, 128, 128));
        assert_eq!(three.color_at(2.0), blue, "past the last stop");
    }

    #[test]
    fn test_fading_to_transparent_keeps_the_color() {
        let fade = gradient(&[(0.0, Rgba::rgb(200, 100, 0)), (1.0, Rgba::new(0, 0, 0, 0))]);
        let middle = fade.color_at(0.5);
        assert_eq!((middle.r, middle.g, middle.b), (200, 100, 0));
        assert_eq!(middle.a, 128);
    }

    #[test]
    fn test_an_odd_dash_pattern_repeats_and_a_bad_one_is_solid() {
        assert_eq!(Stroke::new(1.0).with_dash(&[4.0], 0.0).dash(), &[4.0, 4.0]);
        assert!(
            Stroke::new(1.0)
                .with_dash(&[0.0, 0.0], 0.0)
                .dash()
                .is_empty()
        );
        assert!(
            Stroke::new(1.0)
                .with_dash(&[4.0, -1.0], 0.0)
                .dash()
                .is_empty()
        );
    }
}
