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
use super::super::base::clock;
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

/// The space between a centered tooltip and its anchor, in logical pixels.
const ANCHOR_GAP: f32 = 2.0;

/// The most cells wide a tooltip's text runs before it wraps.
const MAX_CELLS: f32 = 60.0;

/// The most lines a tooltip shows; longer text ends in an ellipsis.
const MAX_LINES: usize = 6;

// ========================================================================
// Data Structures
// ========================================================================

/// A tooltip: what the pointer rests on, and when its help shows.
#[derive(Clone, Debug)]
pub struct Tooltip {
    /// How long the pointer rests before the tooltip shows.
    delay: Duration,
    /// The area and help put away by [`Tooltip::dismiss`], kept away until
    /// the pointer rests on something else.
    dismissed: Option<(Rect, String)>,
    /// Where the tooltip was drawn last, or `None` when it did not show.
    drawn: Option<Rect>,
    /// What the pointer rests on, or `None` when it rests on nothing with
    /// a tooltip.
    hover: Option<Hover>,
    /// Where the tooltip shows beside its anchor.
    placement: TooltipPlacement,
}

/// Where a [`Tooltip`] shows beside the area it is about.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TooltipPlacement {
    /// Under the area, left edges aligned. The default.
    #[default]
    Below,
    /// Above the area, centered on it a little apart, as a toolbar
    /// button's tooltip sits; under it where only there it fits.
    CenteredAbove,
    /// Under the area, centered on it a little apart, as the tooltip of a
    /// button along a window's top sits; over it where only there it fits.
    CenteredBelow,
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
            dismissed: None,
            drawn: None,
            hover: None,
            placement: TooltipPlacement::default(),
        }
    }

    /// This tooltip shown beside its area as `placement` says.
    pub fn with_placement(self, placement: TooltipPlacement) -> Self {
        Self { placement, ..self }
    }

    /// Shows the tooltip beside its area as `placement` says from now on,
    /// for an app whose areas sit in different places to choose per area.
    pub fn set_placement(&mut self, placement: TooltipPlacement) {
        self.placement = placement;
    }

    /// This tooltip shown after the pointer rests for `delay`.
    pub fn with_delay(self, delay: Duration) -> Self {
        Self { delay, ..self }
    }

    /// Says the pointer is over `anchor`, whose help is `text`. Call it on
    /// every pointer move over the area; the delay starts only when the
    /// text changes, so an anchor that follows the pointer waits once.
    /// Moving from one area to another while a tooltip shows switches to
    /// the new one at once.
    pub fn hover(&mut self, anchor: Rect, text: impl Into<String>) {
        self.hover_at(anchor, text.into(), clock::now());
    }

    /// Says the pointer left whatever had a tooltip. Returns whether a
    /// tooltip was showing, so the app knows to redraw.
    pub fn leave(&mut self) -> bool {
        let was_shown = self.is_shown_at(clock::now());
        self.hover = None;
        self.dismissed = None;
        was_shown
    }

    /// Puts the tooltip away for a key, a press, or a scroll, and keeps it
    /// away while the pointer stays on the same area, as a desktop's
    /// tooltips do. Returns whether a tooltip was showing, so the app knows
    /// to redraw.
    pub fn dismiss(&mut self) -> bool {
        let was_shown = self.is_shown_at(clock::now());
        if let Some(hover) = self.hover.take() {
            self.dismissed = Some((hover.anchor, hover.text));
        }
        was_shown
    }

    /// When the tooltip comes due, while the pointer rests on something
    /// whose tooltip has not shown yet: for an app that asks for the redraw
    /// itself rather than through [`Widget::about_to_wait`].
    pub fn due(&self) -> Option<Instant> {
        self.hover
            .as_ref()
            .filter(|hover| !hover.revealed)
            .map(|hover| hover.shows_at)
    }

    /// The help shown and where, as last drawn, or `None` when the tooltip
    /// did not show.
    pub fn shown(&self) -> Option<(&str, Rect)> {
        let rect = self.drawn?;
        self.hover.as_ref().map(|hover| (hover.text.as_str(), rect))
    }

    /// [`Tooltip::hover`] at `now`.
    fn hover_at(&mut self, anchor: Rect, text: String, now: Instant) {
        if let Some((put_away, help)) = &self.dismissed {
            match *put_away == anchor && *help == text {
                true => return,
                false => self.dismissed = None,
            }
        }
        if let Some(hover) = &mut self.hover
            && hover.text == text
        {
            // The same help keeps its wait; one shown moves with its area.
            if hover.anchor != anchor {
                hover.anchor = anchor;
                hover.revealed = false;
            }
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

    /// Draws the tooltip beside its anchor as its placement says, kept
    /// inside `area`, in the overlay, once it is due.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.drawn = None;
        let Some(hover) = self
            .hover
            .as_ref()
            .filter(|_| self.is_shown_at(clock::now()))
        else {
            return;
        };
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let padding = metrics.width.round();
        let padding_y = logical(PADDING_Y, scale_factor);
        let inset = Popup::inset(&theme.shape, scale_factor);
        let room = (MAX_CELLS * metrics.width).min(area.width - 2.0 * (padding + inset));
        let lines = wrap(&hover.text, room.max(metrics.width), |part| {
            label_width(frame, theme, part)
        });
        let label = lines
            .iter()
            .map(|line| label_width(frame, theme, line))
            .fold(0.0, f32::max);
        let width = (label + 2.0 * (padding + inset)).ceil();
        let height = lines.len() as f32 * metrics.height + 2.0 * (padding_y + inset);
        let popup = match self.placement {
            TooltipPlacement::Below => {
                Popup::new(hover.anchor, width, height, PopupSide::Below, area)
            }
            TooltipPlacement::CenteredAbove => {
                let anchor = hover.anchor;
                let gap = logical(ANCHOR_GAP, scale_factor);
                let right = (area.x + area.width - width).max(area.x);
                let x = (anchor.x + (anchor.width - width) / 2.0)
                    .round()
                    .clamp(area.x, right);
                // The anchor widened to the tooltip and the gap, so the popup
                // opens over it, or under it where only there it fits.
                let spaced = Rect::new(x, anchor.y - gap, width, anchor.height + 2.0 * gap);
                Popup::new(spaced, width, height, PopupSide::Above, area)
            }
            TooltipPlacement::CenteredBelow => {
                let anchor = hover.anchor;
                let gap = logical(ANCHOR_GAP, scale_factor);
                let right = (area.x + area.width - width).max(area.x);
                let x = (anchor.x + (anchor.width - width) / 2.0)
                    .round()
                    .clamp(area.x, right);
                let spaced = Rect::new(x, anchor.y - gap, width, anchor.height + 2.0 * gap);
                Popup::new(spaced, width, height, PopupSide::Below, area)
            }
        };
        popup.draw(frame, theme);
        let drawn = popup.rect();
        let content = popup.content(&theme.shape, scale_factor);
        let origin = Point::new(content.x + padding, content.y + padding_y);
        let clip = Rect::new(
            origin.x,
            content.y,
            (content.width - 2.0 * padding).max(0.0),
            content.height,
        );
        frame.overlay(|frame| {
            for (row, line) in lines.iter().enumerate() {
                let text = StyledText::plain(line.as_str(), ThemeToken::Foreground.into());
                let at = Point::new(origin.x, origin.y + row as f32 * metrics.height);
                draw_styled_label(frame, theme, &text, at, clip);
            }
        });
        self.drawn = Some(drawn);
    }

    /// Asks `cx` for a redraw of its window when the tooltip has just come
    /// due,
    /// and returns when it next will, for the app's
    /// [`Application::about_to_wait`](crate::Application::about_to_wait)
    /// to hand back.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let (reveal, next) = self.advance(clock::now());
        if reveal {
            cx.request_redraw();
        }
        next
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `text` broken into lines no wider than `room` as `width` measures them:
/// at spaces where it can, inside a word too long for a line, at most
/// [`MAX_LINES`] of them with the last ending in an ellipsis when it cut.
fn wrap(text: &str, room: f32, width: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_inclusive(' ') {
        if width(format!("{line}{word}").trim_end()) <= room {
            line.push_str(word);
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line).trim_end().to_owned());
        }
        for ch in word.chars() {
            let longer = format!("{line}{ch}");
            if !line.is_empty() && width(longer.trim_end()) > room {
                lines.push(std::mem::take(&mut line));
            }
            line.push(ch);
        }
    }
    if !line.trim_end().is_empty() || lines.is_empty() {
        lines.push(line.trim_end().to_owned());
    }
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        let last = &mut lines[MAX_LINES - 1];
        while !last.is_empty() && width(&format!("{last}…")) > room {
            last.pop();
        }
        last.push('…');
    }
    lines
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
    use crate::app::offscreen;

    const DELAY: Duration = Duration::from_millis(500);

    const BUTTON: Rect = Rect::new(10.0, 10.0, 40.0, 20.0);

    const OTHER: Rect = Rect::new(60.0, 10.0, 40.0, 20.0);

    fn tooltip() -> Tooltip {
        Tooltip::new().with_delay(DELAY)
    }

    #[test]
    fn test_a_centered_tooltip_sits_over_its_anchor_or_under_it_at_the_windows_top() {
        let drawn = |anchor: Rect| {
            let mut tooltip = Tooltip::new().with_placement(TooltipPlacement::CenteredAbove);
            let shown = Instant::now() - DEFAULT_DELAY * 2;
            tooltip.hover_at(anchor, "Split right".to_owned(), shown);
            let mut frame = offscreen::plain_frame(800, 600);
            let area = frame.bounds();
            tooltip.draw(&mut frame, area, &Theme::dark());
            tooltip.drawn.expect("shown")
        };
        let button = Rect::new(400.0, 300.0, 30.0, 24.0);
        let over = drawn(button);
        assert!(over.y + over.height <= button.y, "above: {over:?}");
        let middle = over.x + over.width / 2.0;
        assert!((middle - 415.0).abs() <= 1.0, "centered: {over:?}");
        let top = drawn(Rect::new(400.0, 0.0, 30.0, 24.0));
        assert!(top.y >= 24.0, "under an anchor at the top: {top:?}");
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
    fn test_an_anchor_following_the_pointer_waits_once_for_the_same_help() {
        let start = Instant::now();
        let mut tooltip = tooltip();
        tooltip.hover_at(BUTTON, "Save".into(), start);
        tooltip.hover_at(OTHER, "Save".into(), start + DELAY / 2);
        assert!(tooltip.is_shown_at(start + DELAY), "kept its wait");
    }

    #[test]
    fn test_a_dismissed_tooltip_stays_away_until_the_pointer_moves_on() {
        let start = Instant::now();
        let mut tooltip = tooltip();
        tooltip.hover_at(BUTTON, "Save".into(), start);
        tooltip.dismiss();
        tooltip.hover_at(BUTTON, "Save".into(), start + DELAY);
        assert!(!tooltip.is_shown_at(start + DELAY * 3), "kept away");
        tooltip.hover_at(OTHER, "Open".into(), start + DELAY * 3);
        assert!(tooltip.is_shown_at(start + DELAY * 4), "shown for the next");
    }

    #[test]
    fn test_long_help_wraps_at_spaces_breaks_long_words_and_ends_in_an_ellipsis() {
        // One unit a character, ten to a line.
        let width = |text: &str| text.chars().count() as f32;
        assert_eq!(wrap("open the file", 10.0, width), ["open the", "file"]);
        assert_eq!(
            wrap("https://example.com", 10.0, width),
            ["https://ex", "ample.com"]
        );
        let long = "word ".repeat(40);
        let lines = wrap(&long, 10.0, width);
        assert_eq!(lines.len(), MAX_LINES);
        assert!(lines[MAX_LINES - 1].ends_with('…'), "{lines:?}");
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
