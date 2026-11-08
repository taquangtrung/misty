//! Small chrome [`Icon`]s drawn from rectangles, so they need no font and
//! stay crisp at any scale, and the logical to physical pixel rounding
//! chrome sizes go through.

use super::super::base::geometry::Rect;

// ========================================================================
// Constants
// ========================================================================

/// The share of the icon a restore icon's front square takes.
const RESTORE_FRONT_SHARE: f32 = 0.75;

// ========================================================================
// Data Structures
// ========================================================================

/// An icon on a chrome button.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Icon {
    /// A diagonal cross, for closing.
    Close,
    /// A square outline.
    Maximize,
    /// Three level lines, for a menu.
    Menu,
    /// A horizontal line.
    Minimize,
    /// An upright cross, for adding.
    Plus,
    /// Two overlapping square outlines.
    Restore,
}

// ========================================================================
// Icon Implementations
// ========================================================================

impl Icon {
    /// The rectangles that draw this icon `size` logical pixels square,
    /// centered in `area`, with strokes `stroke` logical pixels thick, on a
    /// display scaled by `scale_factor`.
    pub(crate) fn strokes(
        self,
        area: Rect,
        size: f32,
        stroke: f32,
        scale_factor: f64,
    ) -> Vec<Rect> {
        let size = logical(size, scale_factor);
        let stroke = logical(stroke, scale_factor);
        let left = (area.x + (area.width - size) / 2.0).round();
        let top = (area.y + (area.height - size) / 2.0).round();
        match self {
            Self::Close => {
                // Two diagonals, as stair steps a stroke wide.
                let steps = (size / stroke).round() as usize;
                (0..steps)
                    .flat_map(|step| {
                        let offset = step as f32 * stroke;
                        [
                            Rect::new(left + offset, top + offset, stroke, stroke),
                            Rect::new(left + size - offset - stroke, top + offset, stroke, stroke),
                        ]
                    })
                    .collect()
            }
            Self::Maximize => outline(Rect::new(left, top, size, size), stroke).to_vec(),
            Self::Menu => {
                let middle = ((size - stroke) / 2.0).round();
                [0.0, middle, size - stroke]
                    .map(|offset| Rect::new(left, top + offset, size, stroke))
                    .to_vec()
            }
            Self::Minimize => vec![Rect::new(left, top + (size / 2.0).round(), size, stroke)],
            Self::Plus => {
                let middle = ((size - stroke) / 2.0).round();
                vec![
                    Rect::new(left, top + middle, size, stroke),
                    Rect::new(left + middle, top, stroke, size),
                ]
            }
            Self::Restore => {
                let front = (size * RESTORE_FRONT_SHARE).round();
                let back_offset = size - front;
                let mut strokes =
                    outline(Rect::new(left, top + back_offset, front, front), stroke).to_vec();
                // Only the back square's top and right edges show.
                strokes.push(Rect::new(left + back_offset, top, front, stroke));
                strokes.push(Rect::new(left + size - stroke, top, stroke, front));
                strokes
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `size` logical pixels in whole physical pixels, at least one.
pub(crate) fn logical(size: f32, scale_factor: f64) -> f32 {
    (size * scale_factor as f32).round().max(1.0)
}

/// The four edges of `square`, each `stroke` thick.
pub(crate) fn outline(square: Rect, stroke: f32) -> [Rect; 4] {
    let Rect {
        height,
        width,
        x,
        y,
    } = square;
    [
        Rect::new(x, y, width, stroke),
        Rect::new(x, y + height - stroke, width, stroke),
        Rect::new(x, y, stroke, height),
        Rect::new(x + width - stroke, y, stroke, height),
    ]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::base::geometry::Point;

    #[test]
    fn test_icons_stay_inside_their_area() {
        let area = Rect::new(752.0, 0.0, 48.0, 32.0);
        let icons = [
            Icon::Close,
            Icon::Maximize,
            Icon::Menu,
            Icon::Minimize,
            Icon::Plus,
            Icon::Restore,
        ];
        for icon in icons {
            for scale_factor in [1.0, 1.25, 2.0] {
                for stroke in icon.strokes(area, 10.0, 1.0, scale_factor) {
                    assert!(
                        area.contains(Point::new(stroke.x, stroke.y))
                            && stroke.x + stroke.width <= area.x + area.width
                            && stroke.y + stroke.height <= area.y + area.height,
                        "{icon:?} at {scale_factor}: {stroke:?}"
                    );
                }
            }
        }
    }
}
