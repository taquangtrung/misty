//! [`ProgressBar`]: how far a long task has come, as a track filled in the
//! accent color. A task whose length is
//! not known shows indeterminate: a segment sweeping along the track. Call
//! its [`Widget::about_to_wait`] so each step of the sweep is drawn; while
//! motion is reduced ([`Frame::reduced_motion`]) it holds still instead.
//! The bar takes no input.

use std::convert::Infallible;
use std::time::{Duration, Instant};

use accesskit::{Node, Role};

use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::Widget;
use super::super::base::clock;
use super::super::base::geometry::Rect;
use super::super::base::motion::FRAME_INTERVAL;
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;

// ========================================================================
// Constants
// ========================================================================

/// How thick the track is, in logical pixels.
const TRACK_HEIGHT: f32 = 6.0;

/// The share of the track an indeterminate segment covers.
const SEGMENT_SHARE: f32 = 0.3;

/// How long an indeterminate segment takes to sweep across the track once.
const SWEEP: Duration = Duration::from_millis(1500);

/// The highest percentage, what screen readers hear for a full bar.
const PERCENT: f32 = 100.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A progress bar. The app owns it, sets how far the task has come, and
/// draws it each frame.
#[derive(Clone, Debug)]
pub struct ProgressBar {
    /// How far the task has come, from 0 to 1, or `None` when not known.
    fraction: Option<f32>,
    /// What the bar says before its track, if anything.
    label: Option<String>,
    /// When the sweep drawn last wants its next frame; `None` while the
    /// bar is still.
    next_frame: Option<Instant>,
    /// Where the bar was drawn last, for screen readers.
    rect: Option<Rect>,
    /// When the indeterminate sweep started.
    started: Instant,
}

/// What the app's state says a [`ProgressBar`] shows, for
/// [`Control::sync`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgressProps<'a> {
    /// How far the task has come, from 0 to 1, or `None` when not known.
    pub fraction: Option<f32>,
    /// The name screen readers give it, if any.
    pub label: Option<&'a str>,
}

// ========================================================================
// ProgressBar Implementations
// ========================================================================

impl ProgressBar {
    /// An indeterminate bar with no label, its sweep starting now.
    pub fn new() -> Self {
        Self {
            fraction: None,
            label: None,
            next_frame: None,
            rect: None,
            started: clock::now(),
        }
    }

    /// This bar named `label` for screen readers (the bar does not
    /// draw it; the app draws its own label beside it, as for every
    /// control but a checkbox or a radio button).
    pub fn with_label(self, label: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            ..self
        }
    }

    /// Sets how far the task has come, from 0 to 1 (held to that range),
    /// or `None` for a task whose length is not known.
    pub fn set_fraction(&mut self, fraction: Option<f32>) {
        self.fraction = fraction.map(|fraction| match fraction.is_nan() {
            true => 0.0,
            false => fraction.clamp(0.0, 1.0),
        });
    }

    /// How far the task has come, as last set.
    pub fn fraction(&self) -> Option<f32> {
        self.fraction
    }

    /// Describes the bar as last drawn to screen readers as `id`: a
    /// progress indicator named by its label, at its percentage when
    /// known. Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(rect) = self.rect else {
            return;
        };
        let mut node = Node::new(Role::ProgressIndicator);
        if let Some(label) = &self.label {
            node.set_label(label.as_str());
        }
        if let Some(fraction) = self.fraction {
            node.set_numeric_value(f64::from((fraction * PERCENT).round()));
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(f64::from(PERCENT));
        }
        frame.access(id, rect, node);
    }

    /// Draws the bar in `area` of `frame` as of `now`: the track centered on the area's height, filled as far as the task has
    /// come, or with the sweeping segment, noting when the sweep's next
    /// frame is due.
    pub(crate) fn draw_at(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, now: Instant) {
        self.rect = Some(area);
        let scale_factor = frame.scale_factor();
        let left = area.x;
        let height = logical(TRACK_HEIGHT, scale_factor);
        let width = (area.x + area.width - left).max(0.0);
        let track = Rect::new(
            left,
            (area.y + (area.height - height) / 2.0).round(),
            width,
            height,
        );
        frame.draw_box(track, &theme.shape.highlight(theme.hover, scale_factor));
        let (start, length, next) = match self.fraction {
            Some(fraction) => (0.0, fraction, None),
            None if frame.reduced_motion() => ((1.0 - SEGMENT_SHARE) / 2.0, SEGMENT_SHARE, None),
            None => {
                let (start, length) = sweep(now.saturating_duration_since(self.started));
                (start, length, Some(now + FRAME_INTERVAL))
            }
        };
        let fill = Rect::new(
            track.x + (start * width).round(),
            track.y,
            (length * width).round(),
            height,
        );
        if fill.width > 0.0 {
            frame.clipped(track, |frame| {
                frame.draw_box(fill, &theme.shape.highlight(theme.accent, scale_factor));
            });
        }
        self.next_frame = next;
    }
}

impl Default for ProgressBar {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for ProgressBar {
    type Output = ();
    type Response = ();

    /// A progress bar takes no input.
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) {}

    /// Draws the bar in `area`.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.draw_at(frame, area, theme, clock::now());
    }

    /// Asks for the next frame of an indeterminate sweep, returning when
    /// it is due; `None` while the bar is still.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let next = self.next_frame?;
        cx.request_redraw_at(next);
        Some(next)
    }
}

impl Control for ProgressBar {
    type Props<'a> = ProgressProps<'a>;
    type Value = Infallible;

    /// Takes how far the task has come and its name from `props`.
    fn sync(&mut self, props: ProgressProps<'_>) {
        self.set_fraction(props.fraction);
        if self.label.as_deref() != props.label {
            self.label = props.label.map(str::to_owned);
        }
    }

    /// A row, whatever the width.
    fn measure(&self, _width: f32, metrics: CellMetrics, _scale_factor: f64) -> f32 {
        metrics.height
    }

    /// Never: the bar takes no input.
    fn changed(&self, _response: ()) -> Option<Infallible> {
        None
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Where the sweeping segment of an indeterminate bar starts on the track
/// `elapsed` after the sweep started, and how long it is, both as shares
/// of the track: it enters from the left edge, crosses, and leaves past
/// the right edge, once every [`SWEEP`]. The share before the track's
/// start is cut off, so the segment never pokes past either end.
fn sweep(elapsed: Duration) -> (f32, f32) {
    let phase = (elapsed.as_secs_f32() / SWEEP.as_secs_f32()).fract();
    let start = phase * (1.0 + SEGMENT_SHARE) - SEGMENT_SHARE;
    let end = (start + SEGMENT_SHARE).min(1.0);
    let start = start.max(0.0);
    (start, (end - start).max(0.0))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::testing::{TestWindow, ui_text};

    const AREA: Rect = Rect::new(0.0, 0.0, 200.0, 20.0);

    #[test]
    fn test_the_sweep_enters_crosses_and_leaves_within_the_track() {
        assert_eq!(sweep(Duration::ZERO), (0.0, 0.0), "about to enter");
        let (start, length) = sweep(SWEEP / 2);
        assert!(
            (start + length / 2.0 - 0.5).abs() < 0.01,
            "centered halfway"
        );
        assert!((length - SEGMENT_SHARE).abs() < 1e-6, "whole in the middle");
        let (start, length) = sweep(SWEEP.mul_f32(0.95));
        assert!(
            start + length <= 1.0 && length < SEGMENT_SHARE,
            "cut at the end"
        );
        assert_eq!(sweep(SWEEP), sweep(Duration::ZERO), "it repeats");
    }

    #[test]
    fn test_a_fraction_is_held_to_its_range() {
        let mut bar = ProgressBar::new();
        bar.set_fraction(Some(1.5));
        assert_eq!(bar.fraction(), Some(1.0));
        bar.set_fraction(Some(f32::NAN));
        assert_eq!(bar.fraction(), Some(0.0));
    }

    #[test]
    fn test_only_a_moving_sweep_asks_for_another_frame() {
        let mut window = TestWindow::new(300, 100);
        let mut frame = window.frame();
        let theme = Theme::dark();
        let mut bar = ProgressBar::new();
        let now = Instant::now();
        let mut next = |bar: &mut ProgressBar, frame: &mut Frame| {
            bar.draw_at(frame, AREA, &theme, now);
            bar.about_to_wait(&mut window.cx())
        };
        assert_eq!(next(&mut bar, &mut frame), Some(now + FRAME_INTERVAL));
        frame.set_reduced_motion(true);
        assert_eq!(next(&mut bar, &mut frame), None);
        frame.set_reduced_motion(false);
        bar.set_fraction(Some(0.5));
        assert_eq!(next(&mut bar, &mut frame), None);
    }

    #[test]
    fn test_screen_readers_hear_the_percentage_only_when_it_is_known() {
        let window = TestWindow::new(300, 100);
        let mut frame = window.frame();
        let theme = Theme::dark();
        let mut bar = ProgressBar::new().with_label("Copying");
        bar.draw(&mut frame, AREA, &theme);
        bar.access(&mut frame, FocusId::new("copy"));
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\" focused\n  ProgressIndicator \"Copying\"\n"
        );
        let mut frame = window.frame();
        bar.set_fraction(Some(0.426));
        bar.draw(&mut frame, AREA, &theme);
        bar.access(&mut frame, FocusId::new("copy"));
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\" focused\n  ProgressIndicator \"Copying\" value=43\n"
        );
    }

    #[test]
    fn test_the_track_fills_with_the_accent_as_far_as_the_task_has_come() {
        let theme = Theme::dark();
        let mut bar = ProgressBar::new();
        bar.set_fraction(Some(0.25));
        let Some(snapshot) = offscreen::render(300, 100, |frame| {
            frame.clear(Rgba::from(theme.background));
            bar.draw(frame, AREA, &theme);
        }) else {
            return;
        };
        let y = AREA.height / 2.0;
        snapshot.assert_pixel(10.0, y, theme.accent, "the done quarter");
        snapshot.assert_pixel(150.0, y, theme.hover, "the track still to go");
        snapshot.assert_pixel(150.0, 1.0, theme.background, "above the track");
    }
}
