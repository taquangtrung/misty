//! The resize border of a frameless window: which edge or corner a point
//! grabs, [`ResizeEdge`], and the pointer shape that says so.

use winit::window::ResizeDirection;

use super::super::base::cursor::CursorIcon;
use super::super::base::geometry::{Point, Rect};

// ========================================================================
// Constants
// ========================================================================

/// How deep the resize border reaches into the window, in logical pixels.
const BORDER: f32 = 6.0;

/// How far along each edge a corner's grab zone reaches, in logical pixels:
/// longer than the border is deep, so a corner is easy to hit.
const CORNER: f32 = 16.0;

// ========================================================================
// Data Structures
// ========================================================================

/// An edge or corner of a window, grabbed to resize it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ResizeEdge {
    /// The right edge.
    East,
    /// The top edge.
    North,
    /// The top right corner.
    NorthEast,
    /// The top left corner.
    NorthWest,
    /// The bottom edge.
    South,
    /// The bottom right corner.
    SouthEast,
    /// The bottom left corner.
    SouthWest,
    /// The left edge.
    West,
}

// ========================================================================
// ResizeEdge Implementations
// ========================================================================

impl ResizeEdge {
    /// The edge or corner of a window covering `window` that `point` grabs,
    /// on a display scaled by `scale_factor`, or `None` when it is inside,
    /// away from the border, or outside the window. Within a corner's zone,
    /// which reaches further along the edges than the border is deep, the
    /// corner wins over either edge.
    pub fn at(point: Point, window: Rect, scale_factor: f64) -> Option<Self> {
        let scale = scale_factor as f32;
        let (border, corner) = (BORDER * scale, CORNER * scale);
        let (left, top) = (point.x - window.x, point.y - window.y);
        let (right, bottom) = (window.width - left, window.height - top);
        if left < 0.0 || top < 0.0 || right < 0.0 || bottom < 0.0 {
            return None;
        }
        let on_border = left.min(right).min(top).min(bottom) <= border;
        if !on_border {
            return None;
        }
        let west = left <= corner;
        let east = !west && right <= corner;
        let north = top <= corner;
        let south = !north && bottom <= corner;
        match (north, south, west, east) {
            (true, _, true, _) => Some(Self::NorthWest),
            (true, _, _, true) => Some(Self::NorthEast),
            (_, true, true, _) => Some(Self::SouthWest),
            (_, true, _, true) => Some(Self::SouthEast),
            (_, _, true, _) => Some(Self::West),
            (_, _, _, true) => Some(Self::East),
            (true, _, _, _) => Some(Self::North),
            (_, true, _, _) => Some(Self::South),
            (false, false, false, false) => None,
        }
    }

    /// The pointer shape shown over this edge.
    pub fn cursor(self) -> CursorIcon {
        match self {
            Self::East => CursorIcon::EResize,
            Self::North => CursorIcon::NResize,
            Self::NorthEast => CursorIcon::NeResize,
            Self::NorthWest => CursorIcon::NwResize,
            Self::South => CursorIcon::SResize,
            Self::SouthEast => CursorIcon::SeResize,
            Self::SouthWest => CursorIcon::SwResize,
            Self::West => CursorIcon::WResize,
        }
    }

    /// This edge as winit names it.
    pub(crate) fn direction(self) -> ResizeDirection {
        match self {
            Self::East => ResizeDirection::East,
            Self::North => ResizeDirection::North,
            Self::NorthEast => ResizeDirection::NorthEast,
            Self::NorthWest => ResizeDirection::NorthWest,
            Self::South => ResizeDirection::South,
            Self::SouthEast => ResizeDirection::SouthEast,
            Self::SouthWest => ResizeDirection::SouthWest,
            Self::West => ResizeDirection::West,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Rect = Rect::new(0.0, 0.0, 400.0, 300.0);

    fn at(x: f32, y: f32) -> Option<ResizeEdge> {
        ResizeEdge::at(Point::new(x, y), WINDOW, 1.0)
    }

    #[test]
    fn test_edges_are_grabbed_only_within_the_border() {
        assert_eq!(at(0.0, 150.0), Some(ResizeEdge::West));
        assert_eq!(at(6.0, 150.0), Some(ResizeEdge::West));
        assert_eq!(at(7.0, 150.0), None);
        assert_eq!(at(397.0, 150.0), Some(ResizeEdge::East));
        assert_eq!(at(200.0, 2.0), Some(ResizeEdge::North));
        assert_eq!(at(200.0, 299.0), Some(ResizeEdge::South));
        assert_eq!(at(200.0, 150.0), None);
    }

    #[test]
    fn test_corners_win_and_reach_further_along_the_edges() {
        assert_eq!(at(0.0, 0.0), Some(ResizeEdge::NorthWest));
        // On the left border, 16 pixels down: still the corner.
        assert_eq!(at(2.0, 16.0), Some(ResizeEdge::NorthWest));
        assert_eq!(at(2.0, 17.0), Some(ResizeEdge::West));
        // On the top border, 14 pixels from the right.
        assert_eq!(at(386.0, 3.0), Some(ResizeEdge::NorthEast));
        assert_eq!(at(395.0, 296.0), Some(ResizeEdge::SouthEast));
        assert_eq!(at(10.0, 298.0), Some(ResizeEdge::SouthWest));
    }

    #[test]
    fn test_inside_a_corner_zone_but_off_the_border_is_not_an_edge() {
        assert_eq!(at(10.0, 10.0), None);
    }

    #[test]
    fn test_the_border_scales_with_the_display() {
        let scaled = |x: f32| ResizeEdge::at(Point::new(x, 150.0), WINDOW, 2.0);
        assert_eq!(scaled(12.0), Some(ResizeEdge::West));
        assert_eq!(scaled(13.0), None);
    }

    #[test]
    fn test_points_outside_the_window_grab_nothing() {
        assert_eq!(at(-1.0, 150.0), None);
        assert_eq!(at(401.0, 150.0), None);
        let offset = Rect::new(100.0, 100.0, 50.0, 50.0);
        assert_eq!(
            ResizeEdge::at(Point::new(101.0, 125.0), offset, 1.0),
            Some(ResizeEdge::West)
        );
    }
}
