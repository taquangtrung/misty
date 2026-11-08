//! [`Popup`]: a box placed next to an anchor (a button, a menu item, the
//! pointer) and kept inside the window, drawn in the frame's overlay so it
//! covers whatever is under it, in the theme's [`Shape`]: square and flat,
//! or rounded and lifted by a shadow. Menus and tooltips are built on it;
//! an app can put its own content inside [`Popup::content`].

use super::super::app::frame::Frame;
use super::super::base::geometry::Rect;
use super::super::base::theme::{Shape, Theme};

// ========================================================================
// Data Structures
// ========================================================================

/// A box placed beside an anchor, inside the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Popup {
    /// The whole box, border included.
    rect: Rect,
}

/// Which side of its anchor a popup opens on.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PopupSide {
    /// Above the anchor, left edges aligned, as a card over the word it
    /// tells about. It flips below the anchor when only there it fits.
    Above,
    /// Below the anchor, left edges aligned, as a dropdown under a button
    /// or a context menu at the pointer. It flips above the anchor when
    /// only there it fits.
    Below,
    /// Right of the anchor, top edges aligned, as a submenu beside its
    /// item. It flips left of the anchor when only there it fits.
    Right,
}

// ========================================================================
// Popup Implementations
// ========================================================================

impl Popup {
    /// A popup `width` by `height` pixels on `side` of `anchor`, moved as
    /// little as needed to stay inside `window`, and cut down to the window
    /// when it is larger.
    pub fn new(anchor: Rect, width: f32, height: f32, side: PopupSide, window: Rect) -> Self {
        let width = width.min(window.width).max(0.0);
        let height = height.min(window.height).max(0.0);
        let right = window.x + window.width;
        let bottom = window.y + window.height;
        let (x, y) = match side {
            PopupSide::Above => {
                let below = anchor.y + anchor.height;
                let y = if anchor.y - height >= window.y {
                    anchor.y - height
                } else if below + height <= bottom {
                    below
                } else {
                    window.y
                };
                (anchor.x.min(right - width), y)
            }
            PopupSide::Below => {
                let below = anchor.y + anchor.height;
                let y = if below + height <= bottom {
                    below
                } else if anchor.y - height >= window.y {
                    anchor.y - height
                } else {
                    bottom - height
                };
                (anchor.x.min(right - width), y)
            }
            PopupSide::Right => {
                let beside = anchor.x + anchor.width;
                let x = if beside + width <= right {
                    beside
                } else if anchor.x - width >= window.x {
                    anchor.x - width
                } else {
                    right - width
                };
                (x, anchor.y.min(bottom - height))
            }
        };
        Self {
            rect: Rect::new(x.max(window.x), y.max(window.y), width, height),
        }
    }

    /// A popup filling `rect`, placed by its owner.
    pub(crate) fn with_rect(rect: Rect) -> Self {
        Self { rect }
    }

    /// How far inside the box its content stays, in a box of `shape` on a
    /// display scaled by `scale_factor`: past the border and clear of the
    /// rounded corners. See [`Shape::content_inset`].
    pub fn inset(shape: &Shape, scale_factor: f64) -> f32 {
        shape.content_inset(scale_factor)
    }

    /// The whole box, border included.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// The inside of the box, [`Popup::inset`] from its edge, in a box of
    /// `shape` on a display scaled by `scale_factor`.
    pub fn content(&self, shape: &Shape, scale_factor: f64) -> Rect {
        let inset = Self::inset(shape, scale_factor);
        let Rect {
            height,
            width,
            x,
            y,
        } = self.rect;
        Rect::new(
            x + inset,
            y + inset,
            (width - 2.0 * inset).max(0.0),
            (height - 2.0 * inset).max(0.0),
        )
    }

    /// Draws the box in the overlay: the surface color lined with the
    /// border color, as a floating box of the theme's shape.
    pub fn draw(&self, frame: &mut Frame, theme: &Theme) {
        let style = theme
            .shape
            .floating_box(theme.surface, theme.border, frame.scale_factor());
        frame.overlay(|frame| frame.draw_box(self.rect, &style));
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Dims `area` in the overlay behind a modal (a dialog, the settings page,
/// a centered quick pick): the theme's background at its
/// [`Theme::backdrop_alpha`], so the app shows through faintly.
pub(crate) fn draw_backdrop(frame: &mut Frame, area: Rect, theme: &Theme) {
    let backdrop = theme.background.with_alpha(theme.backdrop_alpha);
    frame.overlay(|frame| frame.fill_rect(area, backdrop));
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgba;

    const WINDOW: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);

    fn below(anchor: Rect, width: f32, height: f32) -> Rect {
        Popup::new(anchor, width, height, PopupSide::Below, WINDOW).rect()
    }

    fn above(anchor: Rect, width: f32, height: f32) -> Rect {
        Popup::new(anchor, width, height, PopupSide::Above, WINDOW).rect()
    }

    fn right(anchor: Rect, width: f32, height: f32) -> Rect {
        Popup::new(anchor, width, height, PopupSide::Right, WINDOW).rect()
    }

    #[test]
    fn test_a_dropdown_opens_under_its_anchor_when_it_fits() {
        let button = Rect::new(10.0, 0.0, 40.0, 30.0);
        assert_eq!(
            below(button, 200.0, 100.0),
            Rect::new(10.0, 30.0, 200.0, 100.0)
        );
    }

    #[test]
    fn test_a_dropdown_near_the_bottom_flips_above_its_anchor() {
        let anchor = Rect::new(100.0, 550.0, 0.0, 0.0);
        assert_eq!(
            below(anchor, 200.0, 100.0),
            Rect::new(100.0, 450.0, 200.0, 100.0)
        );
    }

    #[test]
    fn test_a_card_opens_over_its_anchor_and_flips_under_it_at_the_top() {
        let word = Rect::new(100.0, 300.0, 8.0, 16.0);
        assert_eq!(
            above(word, 200.0, 100.0),
            Rect::new(100.0, 200.0, 200.0, 100.0)
        );
        let word = Rect::new(100.0, 40.0, 8.0, 16.0);
        assert_eq!(
            above(word, 200.0, 100.0),
            Rect::new(100.0, 56.0, 200.0, 100.0)
        );
        let word = Rect::new(100.0, 290.0, 8.0, 16.0);
        assert_eq!(
            above(word, 200.0, 500.0).y,
            0.0,
            "fitting neither way, it is pushed to the top"
        );
    }

    #[test]
    fn test_a_popup_fitting_neither_way_is_pushed_inside_the_window() {
        let anchor = Rect::new(700.0, 300.0, 0.0, 20.0);
        let rect = below(anchor, 200.0, 400.0);
        assert_eq!(rect, Rect::new(600.0, 200.0, 200.0, 400.0));
    }

    #[test]
    fn test_a_popup_larger_than_the_window_is_cut_to_it() {
        let rect = below(Rect::new(50.0, 50.0, 0.0, 0.0), 1000.0, 900.0);
        assert_eq!(rect, WINDOW);
    }

    #[test]
    fn test_a_submenu_opens_right_and_flips_left_at_the_edge() {
        let item = Rect::new(100.0, 100.0, 150.0, 20.0);
        assert_eq!(
            right(item, 200.0, 80.0),
            Rect::new(250.0, 100.0, 200.0, 80.0)
        );
        let item = Rect::new(500.0, 560.0, 150.0, 20.0);
        assert_eq!(
            right(item, 200.0, 80.0),
            Rect::new(300.0, 520.0, 200.0, 80.0)
        );
    }

    #[test]
    fn test_the_content_sits_inside_the_border() {
        let popup = Popup::new(Rect::default(), 100.0, 50.0, PopupSide::Below, WINDOW);
        assert_eq!(
            popup.content(&Shape::GRID, 2.0),
            Rect::new(2.0, 2.0, 96.0, 46.0)
        );
    }

    #[test]
    fn test_a_modern_theme_rounds_the_popup_and_lifts_it_on_a_shadow() {
        let theme = Theme {
            shape: Shape::MODERN,
            ..Theme::dark()
        };
        let popup = Popup::with_rect(Rect::new(20.0, 10.0, 60.0, 30.0));
        let Some(snapshot) = offscreen::render(100, 70, |frame| {
            frame.clear(Rgba::from(theme.background));
            popup.draw(frame, &theme);
        }) else {
            return;
        };
        let background = theme.background;
        snapshot.assert_pixel(50.0, 25.0, theme.surface, "the inside");
        snapshot.assert_pixel(50.0, 10.0, theme.border, "the top edge's border");
        assert_ne!(
            snapshot.pixel(20.0, 10.0),
            theme.border,
            "the corner is rounded off, not a square border"
        );
        let below = snapshot.pixel(50.0, 42.0);
        assert!(
            below.r < background.r && below.g < background.g && below.b < background.b,
            "the shadow darkens below the popup: {below:?} against {background:?}"
        );
        let content = popup.content(&theme.shape, 1.0);
        assert_eq!(
            content,
            Rect::new(23.0, 13.0, 54.0, 24.0),
            "clear of the corners"
        );
        snapshot.assert_pixel(content.x, content.y, theme.surface, "the content's corner");
    }
}
