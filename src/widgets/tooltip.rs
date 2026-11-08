//! [`Tooltip`]: a line of help shown under what the pointer rests on, once
//! it has rested there for a moment.
//!
//! The app says what the pointer is over with [`Tooltip::hover`] and
//! [`Tooltip::leave`]. The delay runs on the event loop's deadlines, not by
//! polling: [`Tooltip::about_to_wait`], called from
//! [`Application::about_to_wait`](crate::Application::about_to_wait),
//! returns when the tooltip is due and asks for the redraw that shows it.

use std::time::{Duration, Instant};

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::Widget;
use super::super::base::geometry::{Point, Rect};
use super::super::base::styled::StyledText;
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::Input;
use super::super::render::icon::logical;
use super::label::{draw_styled_label, label_width};
use super::popup::{Popup, PopupSide};

// ========================================================================
// Constants
// ========================================================================

/// How long the pointer rests before a tooltip shows, unless
/// [`Tooltip::with_delay`] says otherwise.
const DEFAULT_DELAY: Duration = Duration::from_millis(600);

/// The space above and below a tooltip's text, in logical pixels.
const PADDING_Y: f32 = 3.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A tooltip: what the pointer rests on, and when its help shows.
#[derive(Clone, Debug)]
pub struct Tooltip {
    /// How long the pointer rests before the tooltip shows.
    delay: Duration,
    /// What the pointer rests on, or `None` when it rests on nothing with
    /// a tooltip.
    hover: Option<Hover>,
}

/// What the pointer rests on.
#[derive(Clone, Debug, PartialEq)]
struct Hover {
    /// The area the tooltip is about; it shows under it.
    anchor: Rect,
    /// Whether the redraw that shows the tooltip has been asked for.
    revealed: bool,
    /// When the tooltip shows.
    shows_at: Instant,
    /// The help text.
    text: String,
}

// ========================================================================
// Tooltip Implementations
// ========================================================================

impl Tooltip {
    /// A tooltip shown after the pointer rests for 600 ms.
    pub fn new() -> Self {
        Self {
            delay: DEFAULT_DELAY,
            hover: None,
        }
    }

    /// This tooltip shown after the pointer rests for `delay`.
    pub fn with_delay(self, delay: Duration) -> Self {
        Self { delay, ..self }
    }

    /// Says the pointer is over `anchor`, whose help is `text`. Call it on
    /// every pointer move over the area; the delay starts only when the
    /// area or text changes. Moving from one area to another while a
    /// tooltip shows switches to the new one at once.
    pub fn hover(&mut self, anchor: Rect, text: impl Into<String>) {
        self.hover_at(anchor, text.into(), Instant::now());
    }

    /// Says the pointer left whatever had a tooltip. Returns whether a
    /// tooltip was showing, so the app knows to redraw.
    pub fn leave(&mut self) -> bool {
        let was_shown = self.is_shown_at(Instant::now());
        self.hover = None;
        was_shown
    }

    /// [`Tooltip::hover`] at `now`.
    fn hover_at(&mut self, anchor: Rect, text: String, now: Instant) {
        if let Some(hover) = &self.hover
            && hover.anchor == anchor
            && hover.text == text
        {
            return;
        }
        let shows_at = match self.is_shown_at(now) {
            true => now,
            false => now + self.delay,
        };
        self.hover = Some(Hover {
            anchor,
            revealed: false,
            shows_at,
            text,
        });
    }

    /// Whether the tooltip shows at `now`.
    fn is_shown_at(&self, now: Instant) -> bool {
        self.hover
            .as_ref()
            .is_some_and(|hover| now >= hover.shows_at)
    }

    /// At `now`: whether the tooltip has just come due and needs a redraw
    /// to show, and when it will come due if it has not yet.
    fn advance(&mut self, now: Instant) -> (bool, Option<Instant>) {
        let Some(hover) = &mut self.hover else {
            return (false, None);
        };
        if hover.revealed {
            return (false, None);
        }
        if now >= hover.shows_at {
            hover.revealed = true;
            return (true, None);
        }
        (false, Some(hover.shows_at))
    }
}

impl Widget for Tooltip {
    type Output = ();
    type Response = ();

    /// Takes no input: a tooltip shows only what the app tells it.
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) {}

    /// Draws the tooltip under its anchor, kept inside `area`, in the
    /// overlay, once it is due.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let Some(hover) = self
            .hover
            .as_ref()
            .filter(|_| self.is_shown_at(Instant::now()))
        else {
            return;
        };
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let padding = metrics.width.round();
        let padding_y = logical(PADDING_Y, scale_factor);
        let inset = Popup::inset(&theme.shape, scale_factor);
        let label = label_width(frame, theme, &hover.text);
        let width = (label + 2.0 * (padding + inset)).ceil();
        let height = metrics.height + 2.0 * (padding_y + inset);
        let popup = Popup::new(hover.anchor, width, height, PopupSide::Below, area);
        popup.draw(frame, theme);
        let content = popup.content(&theme.shape, scale_factor);
        let origin = Point::new(content.x + padding, content.y + padding_y);
        let clip = Rect::new(
            origin.x,
            content.y,
            (content.width - 2.0 * padding).max(0.0),
            content.height,
        );
        let text = StyledText::plain(hover.text.as_str(), ThemeToken::Foreground.into());
        frame.overlay(|frame| {
            draw_styled_label(frame, theme, &text, origin, clip);
        });
    }

    /// Asks `cx` for a redraw of its window when the tooltip has just come
    /// due,
    /// and returns when it next will, for the app's
    /// [`Application::about_to_wait`](crate::Application::about_to_wait)
    /// to hand back.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let (reveal, next) = self.advance(Instant::now());
        if reveal {
            cx.request_redraw();
        }
        next
    }
}

impl Default for Tooltip {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const DELAY: Duration = Duration::from_millis(500);

    const BUTTON: Rect = Rect::new(10.0, 10.0, 40.0, 20.0);

    const OTHER: Rect = Rect::new(60.0, 10.0, 40.0, 20.0);

    fn tooltip() -> Tooltip {
        Tooltip::new().with_delay(DELAY)
    }

    #[test]
    fn test_a_tooltip_waits_for_its_delay_and_asks_to_show_once() {
        let start = Instant::now();
        let mut tooltip = tooltip();
        tooltip.hover_at(BUTTON, "Save".into(), start);
        assert!(!tooltip.is_shown_at(start));
        assert_eq!(tooltip.advance(start), (false, Some(start + DELAY)));
        assert!(tooltip.is_shown_at(start + DELAY));
        assert_eq!(tooltip.advance(start + DELAY), (true, None));
        assert_eq!(tooltip.advance(start + DELAY * 2), (false, None));
    }

    #[test]
    fn test_moving_within_the_same_area_does_not_restart_the_delay() {
        let start = Instant::now();
        let mut tooltip = tooltip();
        tooltip.hover_at(BUTTON, "Save".into(), start);
        tooltip.hover_at(BUTTON, "Save".into(), start + DELAY / 2);
        assert!(tooltip.is_shown_at(start + DELAY));
    }

    #[test]
    fn test_a_new_area_restarts_the_delay_unless_a_tooltip_shows() {
        let start = Instant::now();
        let mut tooltip = tooltip();
        tooltip.hover_at(BUTTON, "Save".into(), start);
        tooltip.hover_at(OTHER, "Open".into(), start + DELAY / 2);
        assert!(!tooltip.is_shown_at(start + DELAY), "restarted");
        let shown = start + DELAY * 2;
        assert!(tooltip.is_shown_at(shown));
        tooltip.hover_at(BUTTON, "Save".into(), shown);
        assert!(tooltip.is_shown_at(shown), "switched at once");
        assert_eq!(tooltip.advance(shown), (true, None), "redraw to switch");
    }

    #[test]
    fn test_leaving_hides_and_cancels_the_deadline() {
        let start = Instant::now();
        let mut tooltip = tooltip();
        tooltip.hover_at(BUTTON, "Save".into(), start);
        assert!(!tooltip.leave(), "it had not shown yet");
        assert_eq!(tooltip.advance(start + DELAY), (false, None));
        assert!(!tooltip.is_shown_at(start + DELAY));
    }
}
