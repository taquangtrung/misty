//! [`Popup`]: a box placed next to an anchor (a button, a menu item, the
//! pointer) and kept inside the window, drawn in the frame's overlay so it
//! covers whatever is under it, in the theme's [`Shape`]: square and flat,
//! or rounded and lifted by a shadow. Menus and tooltips are built on it;
//! an app can put its own content inside [`Popup::content`], under a
//! titled strip from [`draw_popup_header`].

use super::super::app::frame::Frame;
use super::super::base::box_style::BoxStyle;
use super::super::base::geometry::{Point, Rect};
use super::super::base::styled::StyledText;
use super::super::base::theme::{LabelFont, Shape, Theme};
use super::super::render::font::CellMetrics;
use super::label::{draw_styled_label, styled_label_width};

// ========================================================================
// Constants
// ========================================================================

/// How much of the border color a popup's header strip mixes into the
/// raised color, setting it a little apart from the body below.
const HEADER_BORDER_SHARE: f32 = 0.4;

/// A popup header's height as a multiple of the cell height.
pub(crate) const HEADER_HEIGHT_RATIO: f32 = 1.5;

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

    /// A popup filling `rect`, placed by its owner, for an app that lays out
    /// a box of its own (a dialog on the card that asked, a stack of cards)
    /// and draws it as misty draws popups.
    pub fn with_rect(rect: Rect) -> Self {
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

    /// How tall a header strip from [`draw_popup_header`] is for cells of
    /// `metrics`, for a popup that sizes its own header.
    pub fn header_height(metrics: CellMetrics) -> f32 {
        (metrics.height * HEADER_HEIGHT_RATIO).round()
    }

    /// Where a header strip `height` tall goes across the top of this box
    /// on a display scaled by `scale_factor`: from one side of the box to
    /// the other just inside its border, for [`draw_popup_header`], which
    /// rounds its top corners as the box's are.
    pub fn header_strip(&self, shape: &Shape, scale_factor: f64, height: f32) -> Rect {
        let border = shape.border(scale_factor);
        Rect::new(
            self.rect.x + border,
            self.rect.y + border,
            (self.rect.width - 2.0 * border).max(0.0),
            height.min((self.rect.height - 2.0 * border).max(0.0)),
        )
    }

    /// Draws the box in the overlay: the raised color lined with the border
    /// color, as a floating box of the theme's shape.
    pub fn draw(&self, frame: &mut Frame, theme: &Theme) {
        let style = theme
            .shape
            .floating_box(theme.raised, theme.border, frame.scale_factor());
        frame.overlay(|frame| frame.draw_box(self.rect, &style));
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Dims `area` behind a modal (a dialog, the settings page, a centered
/// quick pick, or a modal the app draws itself): the theme's background at
/// its [`Theme::backdrop_alpha`], drawn in the frame's overlay so it
/// covers everything the frame drew before, text and pictures too, and the
/// app shows through faintly. Draw the modal after it, in the overlay.
pub fn draw_backdrop(frame: &mut Frame, area: Rect, theme: &Theme) {
    let backdrop = theme.background.with_alpha(theme.backdrop_alpha);
    frame.overlay(|frame| frame.fill_rect(area, backdrop));
}

/// Draws `strip` as a popup's header titled `title`, as a dialog or a
/// picker heads itself: the raised color a little toward the border color,
/// the title as styled (a dialog's in bold, a picker's with its error
/// after it) in the theme's label font, centered when that font is
/// proportional and a cell in from the start on the cell grid, cut where
/// it does not fit, with an ellipsis when it asks for one. A popup free to
/// size its header gives the strip [`Popup::header_height`] at
/// [`Popup::header_strip`], from one side of the box to the other; the
/// strip's top corners are rounded as the box's are.
pub fn draw_popup_header(frame: &mut Frame, theme: &Theme, strip: Rect, title: &StyledText) {
    let metrics = frame.cell_metrics();
    let inset = metrics.width.round();
    let room = Rect::new(
        strip.x + inset,
        strip.y,
        (strip.width - 2.0 * inset).max(0.0),
        strip.height,
    );
    let x = match theme.label_font {
        LabelFont::Cells => room.x,
        LabelFont::Proportional => {
            let width = styled_label_width(frame, theme, title).min(room.width);
            (room.x + (room.width - width) / 2.0).round()
        }
    };
    let top = (strip.y + (strip.height - metrics.height) / 2.0).round();
    let fill = theme.raised.mix(theme.border, HEADER_BORDER_SHARE);
    let scale_factor = frame.scale_factor();
    // The box's corners inside its border, which the strip's top follows.
    let radius = (f32::from(theme.shape.corner_radius) * scale_factor as f32).round()
        - theme.shape.border(scale_factor);
    frame.overlay(|frame| {
        match radius > 0.0 {
            true => {
                // Rounded at all four corners a radius taller, then the
                // raised body painted back over its rounded bottom.
                let rounded = Rect::new(strip.x, strip.y, strip.width, strip.height + radius);
                let style = BoxStyle {
                    corner_radius: radius,
                    ..theme.shape.highlight(fill, scale_factor)
                };
                frame.draw_box(rounded, &style);
                let under = Rect::new(strip.x, strip.y + strip.height, strip.width, radius);
                frame.fill_rect(under, theme.raised);
            }
            false => frame.fill_rect(strip, fill),
        }
        draw_styled_label(frame, theme, title, Point::new(x, top), room);
    });
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::{Rgb, Rgba};
    use crate::base::geometry::Point;
    use crate::base::styled::TextColor;
    use crate::base::theme::ThemeToken;

    const WINDOW: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);

    #[test]
    fn test_a_header_strip_spans_the_box_inside_its_border() {
        let popup = Popup::with_rect(Rect::new(100.0, 50.0, 300.0, 200.0));
        assert_eq!(
            popup.header_strip(&Shape::MODERN, 1.0, 30.0),
            Rect::new(101.0, 51.0, 298.0, 30.0)
        );
        let borderless = Shape {
            border_width: 0,
            ..Shape::MODERN
        };
        assert_eq!(
            popup.header_strip(&borderless, 2.0, 30.0),
            Rect::new(100.0, 50.0, 300.0, 30.0)
        );
    }

    #[test]
    fn test_a_popup_header_centers_its_title_only_in_the_proportional_font() {
        let title_x = |theme: &Theme| {
            let mut frame = offscreen::plain_frame(800, 600);
            let strip = Rect::new(
                100.0,
                50.0,
                300.0,
                Popup::header_height(frame.cell_metrics()),
            );
            let title = StyledText::plain("Buffers", TextColor::Token(ThemeToken::Foreground));
            draw_popup_header(&mut frame, theme, strip, &title);
            (frame.overlay.text()[0].origin.x, frame.cell_metrics().width)
        };
        let (grid_x, cell) = title_x(&Theme::dark().with_grid_look());
        assert_eq!(grid_x, 100.0 + cell.round(), "a cell in on the grid");
        let (centered_x, _) = title_x(&Theme::dark());
        assert!(
            centered_x > 200.0 && centered_x < 250.0,
            "about half its width left of the middle at 250: {centered_x}"
        );
    }

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
        snapshot.assert_pixel(50.0, 25.0, theme.raised, "the inside");
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
        snapshot.assert_pixel(content.x, content.y, theme.raised, "the content's corner");
    }

    #[test]
    fn test_the_backdrop_dims_text_and_fills_drawn_before_it() {
        let theme = Theme::dark();
        let white = Rgb::new(0xff, 0xff, 0xff);
        let Some(snapshot) = offscreen::render(100, 60, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.fill_rect(Rect::new(0.0, 0.0, 50.0, 60.0), Rgba::from(white));
            // A full block covers its whole cell in the text color.
            frame.draw_plain_text("\u{2588}", Point::new(60.0, 10.0), white);
            draw_backdrop(frame, frame.bounds(), &theme);
        }) else {
            return;
        };
        let fill = snapshot.pixel(25.0, 30.0);
        assert!(fill.r < white.r, "the fill is dimmed: {fill:?}");
        let metrics = snapshot.metrics();
        let glyph = snapshot.pixel(60.0 + metrics.width / 2.0, 10.0 + metrics.height / 2.0);
        assert!(glyph.r < white.r, "the text is dimmed too: {glyph:?}");
    }
}
