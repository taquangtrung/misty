//! [`Toasts`]: the cards of a [`Notifier`] stacked in a corner of the window
//! (the bottom right unless [`Toasts::with_corner`] says another): the
//! toasts its [`Routing`](crate::Routing) makes cards, newest
//! nearest the corner, and above them each task under way shown as a card,
//! its bar filling as it goes.
//!
//! The notifier keeps what is shown and for how long; the cards only draw
//! it, lent to them with [`Toasts::lend`], and hand it the pointer. A toast
//! the pointer is over stays while it does, and shows a close button at its
//! right end; a click on it dismisses the toast, and a click elsewhere on
//! the toast answers [`ToastsResponse::Opened`]. A task that may be stopped
//! shows a Cancel button, whose click answers [`ToastsResponse::Cancelled`]
//! for the app to stop the work and finish the task. Expiry runs on the
//! event loop's deadlines, not by polling: [`Toasts::about_to_wait`],
//! called from [`Application::about_to_wait`](crate::Application::about_to_wait),
//! drops the toasts whose time is up and returns when the next goes, or
//! when a bar whose task has no known length moves next.

use std::collections::HashMap;
use std::time::Instant;

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::clock;
use super::super::base::color::Rgb;
use super::super::base::geometry::{Point, Rect};
use super::super::base::strings::{self, Strings};
use super::super::base::styled::{Span, StyledText};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::floating_panel::Corner;
use super::label::{draw_label, draw_styled_label, label_width, styled_label_width};
use super::popup::Popup;
use super::progress::ProgressBar;
use crate::base::notice::{Notifier, ProgressId, Task, Toast, ToastId};

// ========================================================================
// Constants
// ========================================================================

/// The space between the stack and the window's edges, in logical pixels.
const MARGIN: f32 = 12.0;

/// The space between two cards, in logical pixels.
const GAP: f32 = 8.0;

/// The space above and below a card's text, in logical pixels.
const PADDING_Y: f32 = 6.0;

/// The width of the severity bar at a card's left, in logical pixels.
const SEVERITY_BAR: f32 = 3.0;

/// The cells a severity icon takes before a card's text, the gap after it
/// included.
const SEVERITY_ICON_CELLS: usize = 2;

/// The fewest cells of text a card is wide.
const MIN_TEXT_CELLS: usize = 20;

/// The most cells of text a card shows; a longer message ends in an
/// ellipsis.
const MAX_TEXT_CELLS: usize = 48;

/// What a toast's close button shows.
const CLOSE_GLYPH: &str = "\u{00d7}";

/// The cells a toast's close button takes at its right end.
const CLOSE_CELLS: usize = 2;

/// The blank cells at each side of a task's Cancel button's label.
const CANCEL_PADDING_CELLS: usize = 1;

/// What follows a card's source before its message.
const SOURCE_SEPARATOR: &str = ": ";

/// What stands between a task's title and its step.
const STEP_SEPARATOR: &str = " \u{00b7} ";

// ========================================================================
// Data Structures
// ========================================================================

/// The cards of a window's notifier. It keeps only what drawing them
/// needs: the corner, where each card was drawn, the one under the
/// pointer, and each task's bar.
#[derive(Clone, Debug)]
pub struct Toasts {
    /// Each task card's bar, by task.
    bars: HashMap<ProgressId, ProgressBar>,
    /// The corner the stack grows from.
    corner: Corner,
    /// Where each card was drawn last, nearest the corner first.
    drawn: Vec<DrawnCard>,
    /// The card under the pointer, if any.
    hovered: Option<Card>,
    /// How a card shows its severity.
    mark: SeverityMark,
}

/// How a [`Toasts`] card shows how serious it is.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SeverityMark {
    /// A thin bar in the severity's color along the card's left edge. The
    /// default.
    #[default]
    Bar,
    /// An icon in the severity's color before the text: an information
    /// sign, a warning sign, a cross for an error, a slashed circle for a
    /// refusal. A task's card keeps the room empty, so the texts line up.
    Icon,
}

/// One card: a toast or a task.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Card {
    /// A task under way.
    Task(ProgressId),
    /// A toast.
    Toast(ToastId),
}

/// Where a card was drawn, for the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DrawnCard {
    /// Its button: a toast's close, a task's Cancel; `None` for a task that
    /// cannot be stopped.
    button: Option<Rect>,
    /// What it shows.
    card: Card,
    /// Its box.
    rect: Rect,
}

/// What became of an input offered to the [`Toasts`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ToastsResponse {
    /// The task's Cancel button was clicked: stop its work, then finish it
    /// on the notifier.
    Cancelled(ProgressId),
    /// The toast's close button was clicked, and it is gone.
    Closed(ToastId),
    /// The input was the cards' (the pointer moving over them).
    Handled,
    /// The input was not the cards'.
    Ignored,
    /// The toast was clicked outside its close button: show more of it,
    /// such as the notifications it is one of.
    Opened(ToastId),
}

/// What an input over the cards does to the notifier's toasts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ToastEffect {
    /// The toast whose close button was clicked, to dismiss.
    pub(crate) dismiss: Option<ToastId>,
    /// The toast to hold while the pointer is over it (`Some(None)`: hold
    /// none), when that changed.
    pub(crate) hold: Option<Option<ToastId>>,
}

/// A card as drawn: the toast or the task it shows.
#[derive(Clone, Copy, Debug)]
enum Shown<'a> {
    /// A task under way.
    Task(&'a Task),
    /// A toast.
    Toast(&'a Toast),
}

// ========================================================================
// Toasts Implementations
// ========================================================================

impl Toasts {
    /// Cards stacked from the bottom right corner.
    pub fn new() -> Self {
        Self {
            bars: HashMap::new(),
            corner: Corner::BottomRight,
            drawn: Vec::new(),
            hovered: None,
            mark: SeverityMark::default(),
        }
    }

    /// These cards showing their severity as `mark` says.
    pub fn with_severity_mark(self, mark: SeverityMark) -> Self {
        Self { mark, ..self }
    }

    /// These cards stacked from `corner`: down from a top corner, up from a
    /// bottom one.
    pub fn with_corner(self, corner: Corner) -> Self {
        Self { corner, ..self }
    }

    /// The cards lent `notifier`, whose toasts and tasks they draw and
    /// whose toasts they hold and dismiss, as a [`Widget`].
    pub fn lend<'a>(&'a mut self, notifier: &'a mut Notifier) -> Lent<'a, Self, &'a mut Notifier> {
        Lent::new(self, notifier)
    }

    /// The card drawn last at `point`, if any.
    fn card_at(&self, point: Point) -> Option<Card> {
        self.drawn
            .iter()
            .find(|drawn| drawn.rect.contains(point))
            .map(|drawn| drawn.card)
    }

    /// Follows `input` over the cards of `notifier`.
    fn handle_input(
        &mut self,
        notifier: &mut Notifier,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> ToastsResponse {
        let (response, effect) = self.follow(input, cx);
        effect.apply(notifier, clock::now());
        response
    }

    /// Follows `input` over the cards as last drawn: what it answers, and
    /// what it does to the notifier's toasts, for the caller to apply to
    /// the notifier (the element tree applies it once the tree is
    /// dropped).
    pub(crate) fn follow(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> (ToastsResponse, ToastEffect) {
        let mut effect = ToastEffect::default();
        let response = match input {
            Input::PointerMoved(point) => {
                let hovered = self.card_at(*point);
                if hovered != self.hovered {
                    self.hovered = hovered;
                    effect.hold = Some(held_toast(hovered));
                    cx.request_redraw();
                }
                match hovered {
                    Some(_) => ToastsResponse::Handled,
                    None => ToastsResponse::Ignored,
                }
            }
            Input::PointerLeft => {
                if self.hovered.take().is_some() {
                    effect.hold = Some(None);
                    cx.request_redraw();
                }
                ToastsResponse::Ignored
            }
            Input::PointerButton(button)
                if button.button == PointerButton::Left && button.pressed =>
            {
                let pressed = self.drawn.iter().find(|drawn| {
                    drawn
                        .button
                        .is_some_and(|rect| rect.contains(button.position))
                });
                match pressed.copied() {
                    None => match self.card_at(button.position) {
                        Some(Card::Toast(toast)) => ToastsResponse::Opened(toast),
                        Some(Card::Task(_)) => ToastsResponse::Handled,
                        None => ToastsResponse::Ignored,
                    },
                    Some(drawn) => {
                        cx.request_redraw();
                        match drawn.card {
                            Card::Task(task) => ToastsResponse::Cancelled(task),
                            Card::Toast(toast) => {
                                self.hovered = None;
                                effect.dismiss = Some(toast);
                                effect.hold = Some(None);
                                ToastsResponse::Closed(toast)
                            }
                        }
                    }
                }
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_)
            | Input::PointerButton(_)
            | Input::Wheel(_) => ToastsResponse::Ignored,
        };
        (response, effect)
    }

    /// The smallest rectangle around every card drawn last, or `None` when
    /// none was.
    pub(crate) fn cards_rect(&self) -> Option<Rect> {
        self.drawn.iter().map(|drawn| drawn.rect).reduce(|a, b| {
            let x = a.x.min(b.x);
            let y = a.y.min(b.y);
            let right = (a.x + a.width).max(b.x + b.width);
            let bottom = (a.y + a.height).max(b.y + b.height);
            Rect::new(x, y, right - x, bottom - y)
        })
    }

    /// Steps the task bars' sweeps, returning when one moves next.
    pub(crate) fn step_bars(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.bars
            .values_mut()
            .filter_map(|bar| bar.about_to_wait(cx))
            .min()
    }

    /// Draws `notifier`'s cards in the corner of `area`, in the overlay.
    pub(crate) fn draw_cards(
        &mut self,
        notifier: &Notifier,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
    ) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let padding = metrics.width.round();
        let padding_y = logical(PADDING_Y, scale_factor);
        let inset = Popup::inset(&theme.shape, scale_factor);
        // The room the severity mark takes before the text's padding.
        let lead = match self.mark {
            SeverityMark::Bar => logical(SEVERITY_BAR, scale_factor),
            SeverityMark::Icon => SEVERITY_ICON_CELLS as f32 * metrics.width,
        };
        let cards: Vec<Shown> = notifier
            .cards()
            .map(Shown::Toast)
            .chain(
                notifier
                    .tasks()
                    .iter()
                    .filter(|task| task.card)
                    .map(Shown::Task),
            )
            .collect();
        let line = metrics.height;
        let sizes: Vec<(f32, f32)> = cards
            .iter()
            .map(|shown| {
                let room = text_room(
                    styled_label_width(frame, theme, &shown.line(theme)),
                    metrics,
                );
                let button = shown.button_width(frame, theme);
                let width = room + button + 2.0 * (padding + inset) + lead;
                let lines = shown.lines() as f32;
                (width.ceil(), lines * line + 2.0 * (padding_y + inset))
            })
            .collect();
        let rects = stack_rects(
            area,
            &sizes,
            self.corner,
            logical(MARGIN, scale_factor),
            logical(GAP, scale_factor),
        );
        self.bars
            .retain(|id, _| notifier.tasks().iter().any(|task| task.id == *id));
        self.drawn.clear();
        // Every card's box first, so a card's shadow stays under the text
        // of the card next to it.
        for rect in &rects {
            Popup::with_rect(*rect).draw(frame, theme);
        }
        for (shown, rect) in cards.iter().zip(rects) {
            let popup = Popup::with_rect(rect);
            let content = popup.content(&theme.shape, scale_factor);
            let origin = Point::new(content.x + lead + padding, content.y + padding_y);
            let label = shown.line(theme);
            let room = text_room(styled_label_width(frame, theme, &label), metrics);
            let hovered = self.hovered == Some(shown.card());
            let button_width = shown.button_width(frame, theme);
            let button = shown.button_rect(rect, origin.y, button_width, metrics);
            let color = shown.color(theme);
            let bars = &mut self.bars;
            let mark = self.mark;
            frame.overlay(|frame| {
                match (mark, shown.icon()) {
                    (SeverityMark::Bar, _) => {
                        frame.fill_rect(
                            Rect::new(content.x, content.y, lead, content.height),
                            color,
                        );
                    }
                    (SeverityMark::Icon, Some(icon)) => {
                        let at = Point::new(content.x + padding, origin.y);
                        frame.draw_plain_text(icon, at, color);
                    }
                    (SeverityMark::Icon, None) => {}
                }
                let at = origin;
                let clip = Rect::new(at.x, at.y, room, metrics.height);
                draw_styled_label(frame, theme, &label, at, clip);
                match shown {
                    Shown::Toast(_) => {
                        if let (true, Some(close)) = (hovered, button) {
                            frame.draw_plain_text(
                                CLOSE_GLYPH,
                                Point::new(close.x, at.y),
                                theme.muted,
                            );
                        }
                    }
                    Shown::Task(task) => {
                        if let Some(cancel) = button {
                            let fill = match hovered {
                                true => theme.hover,
                                false => theme.raised,
                            };
                            frame.fill_rect(cancel, fill);
                            let label_x = cancel.x + CANCEL_PADDING_CELLS as f32 * metrics.width;
                            let label = theme.strings.cancel.as_ref();
                            let label_at = Point::new(label_x, at.y);
                            draw_label(frame, theme, label, label_at, theme.foreground);
                        }
                        let track = Rect::new(
                            origin.x,
                            origin.y + line,
                            (content.x + content.width - padding - origin.x).max(0.0),
                            line,
                        );
                        let progress = bars.entry(task.id).or_default();
                        progress.set_fraction(task.fraction);
                        progress.draw(frame, track, theme);
                    }
                }
            });
            self.drawn.push(DrawnCard {
                button,
                card: shown.card(),
                rect,
            });
        }
    }
}

impl Outcome for ToastsResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

impl Default for Toasts {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Lent<'_, Toasts, &mut Notifier> {
    type Output = ();
    type Response = ToastsResponse;

    /// Follows the pointer over the cards: the toast under it stays while it
    /// does, a click on its close button dismisses it, and a click on a
    /// task's Cancel button answers [`ToastsResponse::Cancelled`].
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> ToastsResponse {
        self.widget.handle_input(self.model, input, cx)
    }

    /// Draws the cards in the corner of `area`, in the overlay.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.widget.draw_cards(self.model, frame, area, theme);
    }

    /// Drops the lent notifier's toasts whose time is up, asking for a
    /// redraw when any went, steps the task bars' sweeps, and returns when
    /// the next toast goes or a bar moves next.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let notifier = &mut *self.model;
        if notifier.toasts_mut().expire(clock::now()) {
            cx.request_redraw();
        }
        let bars = self.widget.step_bars(cx);
        bars.into_iter()
            .chain(notifier.toasts().next_expiry())
            .min()
    }
}

// ========================================================================
// ToastEffect Implementations
// ========================================================================

impl ToastEffect {
    /// Does to `notifier` what the input asked, at `now`.
    pub(crate) fn apply(&self, notifier: &mut Notifier, now: Instant) {
        if let Some(toast) = self.dismiss {
            notifier.toasts_mut().dismiss(toast);
        }
        if let Some(held) = self.hold {
            notifier.toasts_mut().hold(held, now);
        }
    }
}

// ========================================================================
// Shown Implementations
// ========================================================================

impl Shown<'_> {
    /// The card it is.
    fn card(&self) -> Card {
        match self {
            Self::Task(task) => Card::Task(task.id),
            Self::Toast(toast) => Card::Toast(toast.id),
        }
    }

    /// Who says it, if named.
    fn source(&self) -> Option<&str> {
        match self {
            Self::Task(task) => task.source.as_deref(),
            Self::Toast(toast) => toast.source.as_deref(),
        }
    }

    /// What it says after its source: a toast's text, counted in
    /// `strings`' words when it came more than once; a task's title and
    /// step.
    fn message(&self, strings: &Strings) -> String {
        match self {
            Self::Task(task) => match &task.step {
                Some(step) => format!("{}{STEP_SEPARATOR}{step}", task.title),
                None => task.title.clone(),
            },
            Self::Toast(toast) => strings::repeated(strings, &toast.text, toast.repeats),
        }
    }

    /// All it says as its label, in `theme`'s words and colors: its
    /// source muted, then its message, ending in an ellipsis when cut.
    fn line(&self, theme: &Theme) -> StyledText {
        let mut line = StyledText::new().with_ellipsis();
        if let Some(source) = self.source() {
            let lead = format!("{source}{SOURCE_SEPARATOR}");
            line.push(Span::new(lead, ThemeToken::Muted.into()));
        }
        let message = self.message(&theme.strings);
        line.push(Span::new(message, ThemeToken::Foreground.into()));
        line
    }

    /// The lines it takes: a task's second is its bar.
    fn lines(&self) -> usize {
        match self {
            Self::Task(_) => 2,
            Self::Toast(_) => 1,
        }
    }

    /// How wide its button at the right end is, in pixels: a Cancel label
    /// measured as it is drawn, padded on both sides; nothing for a task
    /// that cannot be cancelled.
    fn button_width(&self, frame: &Frame, theme: &Theme) -> f32 {
        let cell = frame.cell_metrics().width;
        match self {
            Self::Task(task) if task.cancellable => {
                let padding = 2.0 * CANCEL_PADDING_CELLS as f32 * cell;
                (label_width(frame, theme, &theme.strings.cancel) + padding).ceil()
            }
            Self::Task(_) => 0.0,
            Self::Toast(_) => CLOSE_CELLS as f32 * cell,
        }
    }

    /// Its button `width` wide in the card at `rect`, on the line starting
    /// at `top`: a task's Cancel on its first line, a toast's close down its
    /// right end; `None` when it has none.
    fn button_rect(&self, rect: Rect, top: f32, width: f32, metrics: CellMetrics) -> Option<Rect> {
        (width > 0.0).then(|| match self {
            Self::Task(_) => Rect::new(
                rect.x + rect.width - width - metrics.width,
                top,
                width,
                metrics.height,
            ),
            Self::Toast(_) => Rect::new(rect.x + rect.width - width, rect.y, width, rect.height),
        })
    }

    /// The icon [`SeverityMark::Icon`] marks it with; `None` for a task,
    /// which has no severity.
    fn icon(&self) -> Option<&'static str> {
        match self {
            Self::Task(_) => None,
            Self::Toast(toast) => Some(toast.severity.glyph()),
        }
    }

    /// The color of its severity mark.
    fn color(&self, theme: &Theme) -> Rgb {
        match self {
            Self::Task(_) => theme.accent,
            Self::Toast(toast) => toast.severity.color(theme),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The toast to keep on screen while the pointer is over `card`.
fn held_toast(card: Option<Card>) -> Option<ToastId> {
    match card {
        Some(Card::Toast(toast)) => Some(toast),
        Some(Card::Task(_)) | None => None,
    }
}

/// How wide the room for a card's text is, in pixels, when its label is
/// `width` wide and the grid's cells are `metrics`: the label's width,
/// held between [`MIN_TEXT_CELLS`] and [`MAX_TEXT_CELLS`] cells.
fn text_room(width: f32, metrics: CellMetrics) -> f32 {
    let cell = metrics.width;
    width.clamp(MIN_TEXT_CELLS as f32 * cell, MAX_TEXT_CELLS as f32 * cell)
}

/// Boxes of `sizes` (width, height), the first in `corner` of `window` and
/// each next one stacked away from it, `margin` from the edges and `gap`
/// apart, aligned on the corner's side.
fn stack_rects(
    window: Rect,
    sizes: &[(f32, f32)],
    corner: Corner,
    margin: f32,
    gap: f32,
) -> Vec<Rect> {
    let left = window.x + margin;
    let right = window.x + window.width - margin;
    let (top, bottom) = (window.y + margin, window.y + window.height - margin);
    let mut along = 0.0;
    sizes
        .iter()
        .map(|&(width, height)| {
            let x = match corner {
                Corner::BottomLeft | Corner::TopLeft => left,
                Corner::BottomRight | Corner::TopRight => right - width,
            };
            let y = match corner {
                Corner::BottomLeft | Corner::BottomRight => bottom - along - height,
                Corner::TopLeft | Corner::TopRight => top + along,
            };
            along += height + gap;
            Rect::new(x, y, width, height)
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::notice::{Notice, Progress, Severity};
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, drawn_text};

    /// A left press at `point`.
    fn press(point: Point) -> Input {
        Input::PointerButton(ButtonInput::press(PointerButton::Left, point))
    }

    /// The middle of `rect`.
    fn middle(rect: Rect) -> Point {
        Point::new(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    /// The button drawn on `card`.
    fn button(toasts: &Toasts, card: Card) -> Rect {
        toasts
            .drawn
            .iter()
            .find(|drawn| drawn.card == card)
            .and_then(|drawn| drawn.button)
            .expect("a button")
    }

    #[test]
    fn test_the_stack_grows_up_from_the_bottom_right_corner() {
        let window = Rect::new(0.0, 0.0, 800.0, 600.0);
        let sizes = [(200.0, 40.0), (300.0, 40.0)];
        let rects = stack_rects(window, &sizes, Corner::BottomRight, 12.0, 8.0);
        assert_eq!(rects[0], Rect::new(588.0, 548.0, 200.0, 40.0));
        assert_eq!(rects[1], Rect::new(488.0, 500.0, 300.0, 40.0));
        let rects = stack_rects(window, &sizes, Corner::TopLeft, 12.0, 8.0);
        assert_eq!(rects[0], Rect::new(12.0, 12.0, 200.0, 40.0));
        assert_eq!(
            rects[1],
            Rect::new(12.0, 60.0, 300.0, 40.0),
            "down from the top"
        );
    }

    #[test]
    fn test_a_card_makes_room_for_wide_characters_within_its_bounds() {
        let frame = crate::app::offscreen::plain_frame(400, 100);
        let metrics = frame.cell_metrics();
        let room = |text: &str| {
            let label = StyledText::plain(text, ThemeToken::Foreground.into());
            let width = styled_label_width(&frame, &Theme::dark().with_grid_look(), &label);
            text_room(width, metrics) / metrics.width
        };
        assert_eq!(room(&"x".repeat(60)), MAX_TEXT_CELLS as f32);
        assert_eq!(room("hi"), MIN_TEXT_CELLS as f32);
        assert_eq!(room(&"漢".repeat(15)), 30.0, "two cells each");
    }

    #[test]
    fn test_an_icon_marked_card_shows_its_severitys_icon_before_its_text() {
        let drawn = |mark: SeverityMark| {
            let mut notifier = Notifier::new();
            notifier.notify(Notice::error("push failed"));
            let mut toasts = Toasts::new().with_severity_mark(mark);
            let mut frame = offscreen::plain_frame(800, 600);
            let bounds = frame.bounds();
            toasts
                .lend(&mut notifier)
                .draw(&mut frame, bounds, &Theme::dark());
            drawn_text(&frame)
        };
        let icons = drawn(SeverityMark::Icon);
        assert_eq!(
            icons.first().map(String::as_str),
            Some(Severity::Error.glyph()),
            "{icons:?}"
        );
        assert!(icons.iter().any(|text| text.contains("push failed")));
        let bars = drawn(SeverityMark::Bar);
        assert!(
            !bars.iter().any(|text| text == Severity::Error.glyph()),
            "{bars:?}"
        );
    }

    #[test]
    fn test_only_the_toasts_routed_to_cards_and_the_carded_tasks_are_drawn() {
        let mut notifier = Notifier::new();
        notifier.notify(Notice::info("copied"));
        notifier.notify(Notice::error("push failed").from_source("git"));
        let now = Instant::now();
        notifier.start(Progress::new("Indexing"), now);
        notifier.start(Progress::new("Building").as_card(), now);
        let mut toasts = Toasts::new();
        let mut frame = offscreen::plain_frame(800, 600);
        let bounds = frame.bounds();
        toasts
            .lend(&mut notifier)
            .draw(&mut frame, bounds, &Theme::dark());
        let text = drawn_text(&frame).join("|");
        assert!(text.contains("push failed"), "{text}");
        assert!(text.contains("Building"), "{text}");
        assert!(!text.contains("copied"), "a status bar message: {text}");
        assert!(!text.contains("Indexing"), "a status bar task: {text}");
        assert!(
            toasts
                .lend(&mut notifier)
                .about_to_wait(&mut TestWindow::new(800, 600).cx())
                .is_some(),
            "a bar of unknown length sweeps"
        );
    }

    #[test]
    fn test_close_dismisses_a_toast_and_cancel_asks_to_stop_a_task() {
        let mut notifier = Notifier::new();
        notifier.notify(Notice::error("push failed"));
        let build = notifier.start(
            Progress::new("Building").cancellable().as_card(),
            Instant::now(),
        );
        let mut toasts = Toasts::new();
        let mut window = TestWindow::new(800, 600);
        let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
        toasts
            .lend(&mut notifier)
            .draw(&mut window.frame(), bounds, &Theme::dark());
        let cancel = button(&toasts, Card::Task(build));
        let answer = window.send(&press(middle(cancel)), |input, cx| {
            toasts.lend(&mut notifier).handle(input, cx)
        });
        assert_eq!(answer, ToastsResponse::Cancelled(build));
        let toast = notifier
            .cards()
            .next()
            .map(|toast| toast.id)
            .expect("the toast");
        let close = button(&toasts, Card::Toast(toast));
        let answer = window.send(&press(middle(close)), |input, cx| {
            toasts.lend(&mut notifier).handle(input, cx)
        });
        assert_eq!(answer, ToastsResponse::Closed(toast));
        assert_eq!(notifier.cards().count(), 0);
        notifier.notify(Notice::error("pull failed"));
        toasts
            .lend(&mut notifier)
            .draw(&mut window.frame(), bounds, &Theme::dark());
        let card = toasts.drawn[0].rect;
        let body = Point::new(card.x + 4.0, card.y + card.height / 2.0);
        let answer = window.send(&press(body), |input, cx| {
            toasts.lend(&mut notifier).handle(input, cx)
        });
        assert!(matches!(answer, ToastsResponse::Opened(_)), "{answer:?}");
    }

    #[test]
    fn test_a_toast_under_the_pointer_stays_past_its_time() {
        let mut notifier = Notifier::new();
        notifier.notify(Notice::error("push failed"));
        let mut toasts = Toasts::new();
        let mut window = TestWindow::new(800, 600);
        let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
        toasts
            .lend(&mut notifier)
            .draw(&mut window.frame(), bounds, &Theme::dark());
        let card = toasts.drawn[0].rect;
        window.send(&Input::PointerMoved(middle(card)), |input, cx| {
            toasts.lend(&mut notifier).handle(input, cx)
        });
        assert!(notifier.toasts().held().is_some());
        assert_eq!(notifier.toasts().next_expiry(), None, "held");
    }
}
