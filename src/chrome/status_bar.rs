//! [`StatusBar`]: the one-line bar along the bottom of a window, showing
//! what the app hands it each frame in a [`StatusLine`], and the messages
//! and tasks of the window's [`Notifier`].
//!
//! From the left: a [`ModeBadge`] when the app is in a mode worth naming,
//! the keys of a pending sequence or count, the left segments (a project, a
//! branch), then the primary text (a location, a title), the one part that
//! is cut, with an ellipsis, when room runs out. A prompt being typed into
//! the bar ([`StatusLine::with_prompt`]) takes the primary text's place,
//! its caret drawn where it is. The right segments (counts, a position) sit at the right
//! edge and are never cut: when they do not all fit, the leftmost go first.
//! A right segment ([`StatusSegment`]) may show an icon from the bar's
//! [`IconSet`] before its text, and one given an id is clickable: it lights
//! under the pointer, and a click on it answers
//! [`StatusBarResponse::Clicked`] with its id, for the app to act on (open
//! a picker of branches, cycle the line endings).
//!
//! The notifier's newest toast its routing puts on the status bar takes
//! the primary text's place while it lasts; the line is passed in on every
//! draw, so nothing the app changed under it is lost. While tasks run, the
//! first right segment is their item: a spinner, the oldest task's title
//! and step, and how far it has come, or how many run; a click on it
//! answers [`StatusBarResponse::Progress`], for the app to show the tasks
//! as cards ([`Notifier::show_task_cards`]). The spinner turns on the event
//! loop's deadlines: the lent bar's [`Widget::about_to_wait`] returns when
//! it turns next, and it stands still while motion is reduced
//! ([`Frame::reduced_motion`]).

use std::time::Instant;

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::color::Rgb;
use super::super::base::geometry::{Point, Rect};
use super::super::base::spinner;
use super::super::base::strings;
use super::super::base::styled::{Span, StyledText};
use super::super::base::text;
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::super::render::image::IconSet;
use super::super::widgets::label::{draw_styled_label, styled_label_cells};
use crate::base::notice::{Notifier, Severity, Task};

// ========================================================================
// Constants
// ========================================================================

/// The space above and below the bar's text, in logical pixels.
const PADDING_Y: f32 = 3.0;

/// The thickness of the line along the bar's top, in logical pixels.
const STROKE: f32 = 1.0;

/// The blank cells at each end of the bar.
const EDGE_CELLS: usize = 1;

/// The blank cells between two parts on the left.
const LEFT_GAP_CELLS: usize = 1;

/// The blank cells between two right segments.
const RIGHT_GAP_CELLS: usize = 2;

/// The thickness of a prompt's caret, in logical pixels.
const CARET: f32 = 2.0;

/// The blank cells at each side of a mode badge's label.
const BADGE_PADDING_CELLS: usize = 1;

/// The cells a segment's icon takes before its text, the gap after it
/// included.
const ICON_CELLS: usize = 2;

/// The icon's side as a share of the cell height.
const ICON_SHARE: f32 = 0.75;

/// The id of the progress item's segment, which no app segment takes.
const PROGRESS_SEGMENT: &str = "misty.status-bar.progress";

/// What stands between a task's title and its step on the progress item.
const STEP_SEPARATOR: &str = " \u{00b7} ";

/// How many percent a whole task is.
const PERCENT: f32 = 100.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A window's status bar. The app owns it and draws it with the frame's
/// [`StatusLine`] and the window's [`Notifier`], offering it the window's
/// pointer input for its clickable segments.
#[derive(Clone, Debug, Default)]
pub struct StatusBar {
    /// The bar as last drawn, or `None` before the first frame.
    bar: Option<Rect>,
    /// Each clickable segment shown in the last frame drawn, by id, with
    /// where it is.
    clickable: Vec<(&'static str, Rect)>,
    /// The clickable segment under the pointer, by id.
    hovered: Option<&'static str>,
    /// The icons segments name.
    icons: IconSet,
    /// The clickable segment pressed and not yet released, by id.
    pressed: Option<&'static str>,
}

/// What became of an input offered to a [`StatusBar`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum StatusBarResponse {
    /// The segment with this id was clicked: do what it stands for.
    Clicked(&'static str),
    /// The progress item was clicked: show the tasks as cards.
    Progress,
    /// The bar used the event; the app should not act on it again.
    Handled,
    /// The event is not the bar's; the app handles it as usual.
    Ignored,
}

/// What the bar shows in one frame, built fresh by the app from its state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusLine {
    /// The mode named at the left, or `None` in the app's default mode.
    mode: Option<ModeBadge>,
    /// The segments at the left, after the mode and the pending keys, left
    /// to right, never cut.
    left: Vec<StatusSegment>,
    /// The keys typed so far of a pending sequence or count, or `None`.
    pending: Option<String>,
    /// The text cut to fit the room the rest leaves.
    primary: StyledText,
    /// A prompt being typed into the bar, in the primary text's place.
    prompt: Option<StatusPrompt>,
    /// The segments at the right edge, left to right, never cut.
    right: Vec<StatusSegment>,
}

/// A prompt typed into a [`StatusLine`]: what it asks, what is typed, and
/// where the caret is.
#[derive(Clone, Debug, Default, PartialEq)]
struct StatusPrompt {
    /// The caret, in characters into `text`.
    caret: usize,
    /// What it asks (`Rename:`).
    label: String,
    /// What is typed.
    text: String,
}

/// One segment of a [`StatusLine`], at the left or the right: its text, the
/// icon before it, and the id a click on it answers with.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusSegment {
    /// The name of its icon in the bar's [`IconSet`], or `None`.
    icon: Option<String>,
    /// What a click on it answers with, or `None` when it is not
    /// clickable.
    id: Option<&'static str>,
    /// How much it matters: short of room, the right side leaves out its
    /// least important segments first, the leftmost of equals.
    priority: u32,
    /// A shorter text shown in its place when the right side does not fit
    /// whole, before any segment is left out.
    short: Option<StyledText>,
    /// The text shown.
    text: StyledText,
}

/// A mode's name on a colored badge, such as `INSERT` on the accent color.
#[derive(Clone, Debug, PartialEq)]
pub struct ModeBadge {
    /// The badge's color; the label is drawn in the background color.
    color: ThemeToken,
    /// The name shown.
    label: String,
}

/// Where each part of a line goes, in cells from the bar's left end.
#[derive(Clone, Debug, PartialEq)]
struct StatusLayout {
    /// The badge's first cell and width, padding included.
    badge: Option<(usize, usize)>,
    /// Each left segment, by index, with its first cell.
    left: Vec<(usize, usize)>,
    /// The pending keys' first cell and width.
    pending: Option<(usize, usize)>,
    /// The primary text's first cell and the cells it may take.
    primary: (usize, usize),
    /// Each right segment shown, by index, with its first cell; the
    /// segments left out are not listed.
    right: Vec<(usize, usize)>,
}

// ========================================================================
// StatusBar Implementations
// ========================================================================

impl StatusBar {
    /// A bar with no icons.
    pub fn new() -> Self {
        Self::default()
    }

    /// This bar drawing the icons segments name from `icons`.
    pub fn with_icons(self, icons: IconSet) -> Self {
        Self { icons, ..self }
    }

    /// The bar lent `line`, what it shows this frame, and `notifier`, whose
    /// message and tasks it shows, as a [`Widget`].
    pub fn lend<'a>(
        &'a mut self,
        line: &'a StatusLine,
        notifier: &'a Notifier,
    ) -> Lent<'a, Self, (&'a StatusLine, &'a Notifier)> {
        Lent::new(self, (line, notifier))
    }

    /// The icons segments name, for an app that adds them as segments
    /// naming them come in rather than all up front.
    pub fn icons_mut(&mut self) -> &mut IconSet {
        &mut self.icons
    }

    /// Where the clickable segment `id` was last drawn, for a tooltip hung
    /// over it; `None` when it was not drawn.
    pub fn segment_rect(&self, id: &str) -> Option<Rect> {
        self.clickable
            .iter()
            .find(|(segment, _)| *segment == id)
            .map(|(_, rect)| *rect)
    }

    /// The clickable segment under the pointer, by id, for a tooltip the
    /// app shows for it.
    pub fn hovered(&self) -> Option<&'static str> {
        self.hovered
    }

    /// The height of the bar in a window whose cells are `metrics` in size
    /// on a display scaled by `scale_factor`, for an app laying out what
    /// sits above it.
    pub fn height(metrics: CellMetrics, scale_factor: f64) -> f32 {
        (metrics.height + 2.0 * logical(PADDING_Y, scale_factor)).round()
    }

    /// Draws `line` along the bottom of `area` in `theme`'s colors,
    /// `notifier`'s message in place of its primary text and its tasks'
    /// item first among its right segments, and returns the part of `area`
    /// above the bar, which is the app's to draw content in.
    fn draw_line(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        line: &StatusLine,
        notifier: &Notifier,
        theme: &Theme,
    ) -> Rect {
        let now = (!frame.reduced_motion()).then(Instant::now);
        let right: Vec<StatusSegment> = progress_segment(notifier.tasks(), now)
            .into_iter()
            .chain(line.right.iter().cloned())
            .collect();
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let height = Self::height(metrics, scale_factor).min(area.height);
        let bar = Rect::new(area.x, area.y + area.height - height, area.width, height);
        frame.fill_rect(bar, theme.surface);
        let stroke = logical(STROKE, scale_factor);
        frame.fill_rect_over_text(Rect::new(bar.x, bar.y, bar.width, stroke), theme.border);
        let top = bar.y + ((bar.height - metrics.height) / 2.0).round();
        let at = |cell: usize| Point::new(bar.x + cell as f32 * metrics.width, top);
        // Measured as they are drawn, so each label fits the cells it is
        // laid out in whatever the chrome font.
        let cells = |segment: &StatusSegment| segment.width(frame, theme);
        let left_widths: Vec<usize> = line.left.iter().map(cells).collect();
        let badge = line.mode.as_ref().map(|mode| mode.width(frame, theme));
        let pending = line
            .pending
            .as_deref()
            .map(|pending| styled_label_cells(frame, theme, &pending_label(pending)));
        let lay_out = |right: &[StatusSegment]| {
            let widths: Vec<usize> = right.iter().map(cells).collect();
            let weighed: Vec<(usize, u32)> = right
                .iter()
                .zip(&widths)
                .map(|(segment, &width)| (width, segment.priority))
                .collect();
            let layout = StatusLayout::new(
                metrics.cols_in(bar.width),
                badge,
                pending,
                &left_widths,
                &weighed,
            );
            (layout, widths)
        };
        // Short of room, the segments take their short texts before any is
        // left out.
        let (mut layout, mut right_widths) = lay_out(&right);
        let mut right = right;
        if layout.right.len() < right.len() {
            right = right.into_iter().map(StatusSegment::shortened).collect();
            (layout, right_widths) = lay_out(&right);
        }
        if let (Some(mode), Some((cell, width))) = (&line.mode, layout.badge) {
            let origin = at(cell);
            let badge = Rect::new(origin.x, bar.y, width as f32 * metrics.width, bar.height);
            frame.fill_rect(badge, theme.color(mode.color));
            let label = mode.label();
            draw_styled_label(frame, theme, &label, at(cell + BADGE_PADDING_CELLS), bar);
        }
        if let (Some(pending), Some((cell, _))) = (&line.pending, layout.pending) {
            draw_styled_label(frame, theme, &pending_label(pending), at(cell), bar);
        }
        let (cell, room) = layout.primary;
        let clip = Rect::new(at(cell).x, bar.y, room as f32 * metrics.width, bar.height);
        let message = match &line.prompt {
            Some(_) => None,
            None => notifier.status_message(),
        };
        let primary = match message {
            Some(toast) => {
                let color = match toast.severity {
                    Severity::Error => ThemeToken::Danger,
                    Severity::Info => ThemeToken::Foreground,
                    Severity::Refusal | Severity::Warning => ThemeToken::Warning,
                };
                let said = match &toast.source {
                    Some(source) => format!("{source}: {}", toast.text),
                    None => toast.text.clone(),
                };
                let text = strings::repeated(&theme.strings, &said, toast.repeats);
                StyledText::from(Span::new(text, color.into()).bold())
            }
            None => match &line.prompt {
                Some(prompt) => StyledText::new()
                    .with(Span::new(format!("{} ", prompt.label), ThemeToken::Accent.into()).bold())
                    .with(Span::new(
                        prompt.text.as_str(),
                        ThemeToken::Foreground.into(),
                    )),
                None => line.primary.clone(),
            },
        };
        let primary = primary.with_ellipsis();
        match line.prompt {
            // A prompt's caret is placed by cells, so its line stays on them.
            Some(_) => {
                frame.draw_text(&primary, at(cell), clip, theme);
            }
            None => {
                draw_styled_label(frame, theme, &primary, at(cell), clip);
            }
        }
        if let Some(prompt) = &line.prompt {
            let typed: String = prompt.text.chars().take(prompt.caret).collect();
            let caret_cell = cell + text::width(&prompt.label) + 1 + text::width(&typed);
            if caret_cell < cell + room {
                let x = at(caret_cell).x;
                let caret = Rect::new(x, top, logical(CARET, scale_factor), metrics.height);
                frame.fill_rect_over_text(caret, theme.foreground);
            }
        }
        self.bar = Some(bar);
        self.clickable.clear();
        let mut segments: Vec<(&StatusSegment, usize, usize)> = Vec::new();
        for &(index, cell) in &layout.left {
            segments.push((&line.left[index], cell, left_widths[index]));
        }
        for &(index, cell) in &layout.right {
            segments.push((&right[index], cell, right_widths[index]));
        }
        for (segment, cell, cells) in segments {
            let origin = at(cell);
            let width = cells as f32 * metrics.width;
            if let Some(id) = segment.id {
                // Half a cell either side, so the light does not touch the text.
                let half = (metrics.width / 2.0).round();
                let rect = Rect::new(origin.x - half, bar.y, width + 2.0 * half, bar.height);
                if self.hovered == Some(id) || self.pressed == Some(id) {
                    frame.fill_rect(rect, theme.hover);
                }
                self.clickable.push((id, rect));
            }
            let mut text_cell = cell;
            if let Some(name) = &segment.icon {
                text_cell += ICON_CELLS;
                let size = (metrics.height * ICON_SHARE).round();
                let y = (bar.y + (bar.height - size) / 2.0).round();
                let rect = Rect::new(origin.x, y, size, size);
                frame.draw_named_icon(&self.icons, name, rect, segment.color(theme));
            }
            draw_styled_label(frame, theme, &segment.text, at(text_cell), bar);
        }
        Rect::new(area.x, area.y, area.width, (area.height - height).max(0.0))
    }

    /// The clickable segment at `point` as last drawn, by id.
    fn segment_at(&self, point: Point) -> Option<&'static str> {
        self.clickable
            .iter()
            .find(|(_, rect)| rect.contains(point))
            .map(|(id, _)| *id)
    }

    /// Reacts to `input`: the light under the pointer, and a click on a
    /// clickable segment.
    fn handle_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> StatusBarResponse {
        match input {
            Input::PointerMoved(point) => {
                let hovered = self.segment_at(*point);
                if hovered != self.hovered {
                    self.hovered = hovered;
                    cx.request_redraw();
                }
                match self.bar.is_some_and(|bar| bar.contains(*point)) {
                    true => StatusBarResponse::Handled,
                    false => StatusBarResponse::Ignored,
                }
            }
            Input::PointerLeft => {
                if self.hovered.take().is_some() {
                    cx.request_redraw();
                }
                StatusBarResponse::Ignored
            }
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                let over = self.segment_at(button.position);
                match (button.pressed, self.pressed.take()) {
                    (true, _) => {
                        self.pressed = over;
                        if over.is_some() {
                            cx.request_redraw();
                            return StatusBarResponse::Handled;
                        }
                        StatusBarResponse::Ignored
                    }
                    (false, Some(pressed)) => {
                        cx.request_redraw();
                        match over == Some(pressed) {
                            true if pressed == PROGRESS_SEGMENT => StatusBarResponse::Progress,
                            true => StatusBarResponse::Clicked(pressed),
                            false => StatusBarResponse::Handled,
                        }
                    }
                    (false, None) => StatusBarResponse::Ignored,
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
            | Input::Wheel(_) => StatusBarResponse::Ignored,
        }
    }
}

impl Widget for Lent<'_, StatusBar, (&StatusLine, &Notifier)> {
    type Output = Rect;
    type Response = StatusBarResponse;

    /// Reacts to the pointer over the clickable segments as last drawn:
    /// the one under it lights, and a left press and release on the same
    /// one answers [`StatusBarResponse::Clicked`].
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> StatusBarResponse {
        self.widget.handle_input(input, cx)
    }

    /// Draws the lent line along the bottom of `area` in `theme`'s colors,
    /// the notifier's message in place of its primary text and its tasks'
    /// item at the right, and returns the part of `area` above the bar,
    /// which is the app's to draw content in.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        let (line, notifier) = self.model;
        self.widget.draw_line(frame, area, line, notifier, theme)
    }

    /// Asks for a redraw while the lent notifier has tasks running, and
    /// returns when the progress item's spinner turns next; `None` while
    /// none runs or motion is reduced, when the spinner stands still.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let (_, notifier) = self.model;
        let task = notifier.tasks().first()?;
        if cx.reduced_motion() {
            return None;
        }
        cx.request_redraw();
        Some(spinner::next_turn(task.started, Instant::now()))
    }
}

// ========================================================================
// StatusLine Implementations
// ========================================================================

impl StatusLine {
    /// A line showing `primary` and nothing else.
    pub fn new(primary: impl Into<StyledText>) -> Self {
        Self {
            primary: primary.into(),
            ..Self::default()
        }
    }

    /// This line with `mode` named at its left.
    pub fn with_mode(self, mode: ModeBadge) -> Self {
        Self {
            mode: Some(mode),
            ..self
        }
    }

    /// This line showing `keys`, the keys typed so far of a pending
    /// sequence or count, after the mode. Empty keys show nothing.
    pub fn with_pending(self, keys: impl Into<String>) -> Self {
        let keys = keys.into();
        Self {
            pending: (!keys.is_empty()).then_some(keys),
            ..self
        }
    }

    /// This line with `segment` added at the right end of its left
    /// segments, after the mode and the pending keys.
    pub fn with_left(mut self, segment: StatusSegment) -> Self {
        self.left.push(segment);
        self
    }

    /// This line with a prompt in its primary text's place: `label` (what
    /// it asks, drawn in the accent), then `text`, what is typed, its caret
    /// `caret` characters into it. The app edits the text; the bar draws it.
    pub fn with_prompt(
        self,
        label: impl Into<String>,
        text: impl Into<String>,
        caret: usize,
    ) -> Self {
        Self {
            prompt: Some(StatusPrompt {
                caret,
                label: label.into(),
                text: text.into(),
            }),
            ..self
        }
    }

    /// This line with `text` added at the right end of its right
    /// segments, neither clickable nor with an icon.
    pub fn with_right(self, text: impl Into<StyledText>) -> Self {
        self.with_segment(StatusSegment::new(text))
    }

    /// This line with `segment` added at the right end of its right
    /// segments.
    pub fn with_segment(mut self, segment: StatusSegment) -> Self {
        self.right.push(segment);
        self
    }
}

impl Outcome for StatusBarResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// StatusSegment Implementations
// ========================================================================

impl StatusSegment {
    /// A segment showing `text`, neither clickable nor with an icon.
    pub fn new(text: impl Into<StyledText>) -> Self {
        Self {
            icon: None,
            id: None,
            priority: 0,
            short: None,
            text: text.into(),
        }
    }

    /// This segment with the icon named `icon` in the bar's [`IconSet`]
    /// before its text.
    pub fn with_icon(self, icon: impl Into<String>) -> Self {
        Self {
            icon: Some(icon.into()),
            ..self
        }
    }

    /// This segment clickable, a click on it answering
    /// [`StatusBarResponse::Clicked`] with `id`.
    pub fn with_id(self, id: &'static str) -> Self {
        Self {
            id: Some(id),
            ..self
        }
    }

    /// This segment with `short` shown in its place when the right side
    /// does not fit whole: a quota as `3%` rather than `3% of 5h, resets
    /// 20:39`.
    pub fn with_short(self, short: impl Into<StyledText>) -> Self {
        Self {
            short: Some(short.into()),
            ..self
        }
    }

    /// This segment kept over those of a lower `priority` when the right
    /// side does not fit; all are 0 unless given one.
    pub fn with_priority(self, priority: u32) -> Self {
        Self { priority, ..self }
    }

    /// This segment with its short text in place of its text, if it has
    /// one.
    fn shortened(self) -> Self {
        match self.short {
            Some(short) => Self {
                short: None,
                text: short,
                ..self
            },
            None => self,
        }
    }

    /// The cells it takes as drawn in `theme`'s chrome font, its icon
    /// included.
    fn width(&self, frame: &Frame, theme: &Theme) -> usize {
        let icon = match self.icon {
            Some(_) => ICON_CELLS,
            None => 0,
        };
        icon + styled_label_cells(frame, theme, &self.text)
    }

    /// The color its icon takes: its text's first color, or the theme's
    /// foreground.
    fn color(&self, theme: &Theme) -> Rgb {
        self.text
            .spans()
            .first()
            .map_or(theme.foreground, |span| span.color.resolve(theme))
    }
}

// ========================================================================
// ModeBadge Implementations
// ========================================================================

impl ModeBadge {
    /// A badge naming the mode `label` on the theme's `color`.
    pub fn new(label: impl Into<String>, color: ThemeToken) -> Self {
        Self {
            color,
            label: label.into(),
        }
    }

    /// The cells the badge takes as drawn in `theme`'s chrome font, its
    /// padding included.
    fn width(&self, frame: &Frame, theme: &Theme) -> usize {
        styled_label_cells(frame, theme, &self.label()) + 2 * BADGE_PADDING_CELLS
    }

    /// The badge's label: its mode, bold on the badge's color.
    fn label(&self) -> StyledText {
        StyledText::from(Span::new(self.label.as_str(), ThemeToken::Background.into()).bold())
    }
}

// ========================================================================
// StatusLayout Implementations
// ========================================================================

impl StatusLayout {
    /// The layout of a bar `cols` cells wide holding a badge `badge` cells
    /// wide, pending keys `pending` cells wide, and right segments of
    /// `right` widths. The badge and the pending keys come first; the right
    /// segments that fit sit at the right end, dropped leftmost first; the
    /// primary text gets the cells between.
    fn new(
        cols: usize,
        badge: Option<usize>,
        pending: Option<usize>,
        left: &[usize],
        right: &[(usize, u32)],
    ) -> Self {
        let mut next = EDGE_CELLS;
        let mut place = |width: usize| {
            let cell = next;
            next += width + LEFT_GAP_CELLS;
            (cell, width)
        };
        let badge = badge.map(&mut place);
        let pending = pending.map(&mut place);
        let left = left
            .iter()
            .enumerate()
            .map(|(index, &width)| (index, place(width).0))
            .collect();
        let primary_start = next;
        let end = cols.saturating_sub(EDGE_CELLS);
        // The right segments that fit: short of room, the least important
        // go first, the leftmost of equals.
        let room = end.saturating_sub(primary_start);
        let mut kept: Vec<usize> = (0..right.len()).collect();
        let span = |kept: &[usize]| -> usize {
            let widths: usize = kept.iter().map(|&index| right[index].0).sum();
            widths + RIGHT_GAP_CELLS * kept.len().saturating_sub(1)
        };
        while !kept.is_empty() && span(&kept) > room {
            let drop = kept
                .iter()
                .enumerate()
                .min_by_key(|(_, index)| right[**index].1)
                .map(|(at, _)| at)
                .unwrap_or(0);
            kept.remove(drop);
        }
        let mut shown = Vec::new();
        let mut left_edge = end;
        for &index in kept.iter().rev() {
            let gap = match shown.is_empty() {
                true => 0,
                false => RIGHT_GAP_CELLS,
            };
            left_edge = left_edge.saturating_sub(right[index].0 + gap);
            shown.push((index, left_edge));
        }
        shown.reverse();
        let primary_end = match shown.is_empty() {
            true => end,
            false => left_edge.saturating_sub(RIGHT_GAP_CELLS),
        };
        Self {
            badge,
            left,
            pending,
            primary: (primary_start, primary_end.saturating_sub(primary_start)),
            right: shown,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The pending keys `pending` as their label, in the accent color.
fn pending_label(pending: &str) -> StyledText {
    StyledText::plain(pending, ThemeToken::Accent.into())
}

/// The progress item for `tasks` at `now`, or `None` while none runs: a
/// spinner turning from the oldest's start (standing at its first glyph
/// for no `now`, while motion is reduced), its title and step and how far
/// it has come, and how many more run.
fn progress_segment(tasks: &[Task], now: Option<Instant>) -> Option<StatusSegment> {
    let task = tasks.first()?;
    let now = now.unwrap_or(task.started);
    let mut text = format!("{} {}", spinner::glyph_at(task.started, now), task.title);
    if let Some(step) = &task.step {
        text.push_str(STEP_SEPARATOR);
        text.push_str(step);
    }
    if let Some(fraction) = task.fraction {
        text.push_str(&format!(" {:.0}%", fraction * PERCENT));
    }
    if tasks.len() > 1 {
        text.push_str(&format!(" +{}", tasks.len() - 1));
    }
    let text = StyledText::plain(text, ThemeToken::Muted.into());
    Some(StatusSegment::new(text).with_id(PROGRESS_SEGMENT))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::notice::{Notice, Progress, Routing};
    use crate::base::theme::ChromeText;
    use crate::input::event::ButtonInput;
    use crate::testing::TestWindow;
    use crate::widgets::label::styled_label_width;

    /// A press, or a release when not `pressed`, of the left button at `x`, `y`.
    fn left(pressed: bool, x: f32, y: f32) -> Input {
        let press = ButtonInput::press(PointerButton::Left, Point::new(x, y));
        Input::PointerButton(ButtonInput { pressed, ..press })
    }

    /// `text` in the foreground color.
    fn plain(text: &str) -> StyledText {
        StyledText::plain(text, ThemeToken::Foreground.into())
    }

    #[test]
    fn test_a_segment_with_an_id_answers_a_click_with_it_and_lights_under_the_pointer() {
        let mut window = TestWindow::new(400, 100);
        let mut bar = StatusBar::new();
        let notifier = Notifier::new();
        let line = StatusLine::new(plain("file"))
            .with_segment(
                StatusSegment::new(plain("main"))
                    .with_icon("branch")
                    .with_id("branch"),
            )
            .with_right(plain("1:1"));
        let area = Rect::new(0.0, 0.0, 400.0, 100.0);
        bar.lend(&line, &notifier)
            .draw(&mut window.frame(), area, &Theme::dark());
        let [(id, rect)] = bar.clickable[..] else {
            panic!("one clickable segment: {:?}", bar.clickable);
        };
        assert_eq!(id, "branch");
        assert_eq!(bar.segment_rect("branch"), Some(rect));
        assert_eq!(bar.segment_rect("nothing"), None);
        let metrics = window.frame().cell_metrics();
        // "main" and its icon: 6 cells, and half a cell either side.
        assert_eq!(
            rect.width,
            6.0 * metrics.width + 2.0 * (metrics.width / 2.0).round()
        );
        let (x, y) = (rect.x + 2.0, rect.y + 2.0);
        let mut send = |input: Input, bar: &mut StatusBar| {
            window.send(&input, |input, cx| {
                bar.lend(&line, &notifier).handle(input, cx)
            })
        };
        assert_eq!(
            send(Input::PointerMoved(Point::new(x, y)), &mut bar),
            StatusBarResponse::Handled
        );
        assert_eq!(bar.hovered, Some("branch"));
        send(left(true, x, y), &mut bar);
        assert_eq!(
            send(left(false, x, y), &mut bar),
            StatusBarResponse::Clicked("branch")
        );
        send(left(true, x, y), &mut bar);
        assert_eq!(
            send(left(false, 1.0, y), &mut bar),
            StatusBarResponse::Handled,
            "released off the segment"
        );
        assert_eq!(
            send(left(true, 1.0, 10.0), &mut bar),
            StatusBarResponse::Ignored,
            "above the bar"
        );
    }

    #[test]
    fn test_a_wide_segment_in_the_proportional_font_is_laid_out_cells_enough_to_hold_it() {
        let frame = offscreen::plain_frame(400, 100);
        let theme = Theme {
            chrome_text: ChromeText::Proportional,
            ..Theme::dark()
        };
        let segment = StatusSegment::new(plain("WWWWWW"));
        let width = styled_label_width(&frame, &theme, &segment.text);
        let cells = segment.width(&frame, &theme);
        assert!(cells > 6, "more than its characters: {cells}");
        assert!(cells as f32 * frame.cell_metrics().width >= width);
    }

    #[test]
    fn test_an_icon_takes_two_cells_before_its_segments_text() {
        let frame = offscreen::plain_frame(400, 100);
        let theme = Theme::dark();
        let plain = StatusSegment::new(plain("main"));
        assert_eq!(plain.width(&frame, &theme), 4);
        assert_eq!(plain.with_icon("branch").width(&frame, &theme), 6);
    }

    #[test]
    fn test_left_parts_stack_and_right_segments_sit_at_the_right_end() {
        // 1 edge, a 8-cell badge at 1, keys at 10, primary from 13.
        let layout = StatusLayout::new(60, Some(8), Some(2), &[], &[(5, 0), (3, 0)]);
        assert_eq!(layout.badge, Some((1, 8)));
        assert_eq!(layout.pending, Some((10, 2)));
        // The last segment ends at 59, the edge; the first 2 cells before it.
        assert_eq!(layout.right, [(0, 49), (1, 56)]);
        assert_eq!(layout.primary, (13, 34));
    }

    #[test]
    fn test_right_segments_that_do_not_fit_are_dropped_leftmost_first_never_cut() {
        let layout = StatusLayout::new(20, None, None, &[], &[(10, 0), (4, 0), (6, 0)]);
        // 6 at 13..19, 4 at 7..11; 10 more would start before the left.
        assert_eq!(layout.right, [(1, 7), (2, 13)]);
        assert_eq!(layout.primary, (1, 4));
        let narrow = StatusLayout::new(4, None, None, &[], &[(6, 0)]);
        assert!(narrow.right.is_empty());
        assert_eq!(narrow.primary, (1, 2));
    }

    #[test]
    fn test_the_progress_item_names_the_oldest_task_how_far_and_how_many_more() {
        let now = Instant::now();
        let mut notifier = Notifier::new();
        let build = notifier.start(Progress::new("Building"), now);
        notifier.set_step(build, Some("3/10".into()));
        notifier.set_fraction(build, Some(0.4));
        notifier.start(Progress::new("Indexing"), now);
        let segment = progress_segment(notifier.tasks(), Some(now)).expect("an item");
        let text: String = segment
            .text
            .spans()
            .iter()
            .map(|span| span.text.as_str())
            .collect();
        assert!(text.ends_with("Building \u{00b7} 3/10 40% +1"), "{text}");
        assert_eq!(segment.id, Some(PROGRESS_SEGMENT));
        assert_eq!(progress_segment(&[], Some(now)), None);
    }

    #[test]
    fn test_with_motion_reduced_the_spinner_stands_still_and_asks_no_frames() {
        let started = Instant::now()
            .checked_sub(spinner::FRAME * 3)
            .expect("a clock past three frames");
        let mut notifier = Notifier::new();
        notifier.start(Progress::new("Building"), started);
        let line = StatusLine::new(plain("f"));
        let mut bar = StatusBar::new();
        let mut window = TestWindow::new(400, 100);
        let area = Rect::new(0.0, 0.0, 400.0, 100.0);
        let turning = bar.lend(&line, &notifier).about_to_wait(&mut window.cx());
        assert!(turning.is_some(), "a running task's spinner turns");
        window.set_reduced_motion(true);
        let still = bar.lend(&line, &notifier).about_to_wait(&mut window.cx());
        assert_eq!(still, None);
        let mut frame = window.frame();
        bar.lend(&line, &notifier)
            .draw(&mut frame, area, &Theme::dark());
        let first = spinner::glyph_at(started, started).to_string();
        let turned = spinner::glyph_at(started, started + spinner::FRAME * 3).to_string();
        let pieces = crate::testing::drawn_text(&frame);
        assert!(pieces.contains(&first), "the first glyph: {pieces:?}");
        assert!(!pieces.contains(&turned), "not turned on: {pieces:?}");
    }

    #[test]
    fn test_left_segments_follow_the_badge_and_push_the_primary_text_right() {
        // A 1 cell edge, the badge at 1 (8 cells), the segments at 10 and 15.
        let layout = StatusLayout::new(60, Some(8), None, &[4, 6], &[(5, 0)]);
        assert_eq!(layout.left, [(0, 10), (1, 15)]);
        assert_eq!(layout.primary.0, 22);
    }

    #[test]
    fn test_a_prompt_takes_the_primary_texts_place_with_its_caret_where_it_is() {
        let theme = Theme::dark();
        let mut bar = StatusBar::new();
        let notifier = Notifier::new();
        let line = StatusLine::new(plain("hidden")).with_prompt("Rename:", "abcd", 2);
        let area = Rect::new(0.0, 0.0, 400.0, 100.0);
        let mut above = Rect::default();
        let Some(snapshot) = offscreen::render(400, 100, |frame| {
            frame.clear(Rgba::from(theme.background));
            above = bar.lend(&line, &notifier).draw(frame, area, &theme);
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        // The edge, `Rename:` and a space, then two characters: cell 11.
        let caret_x = 11.0 * metrics.width;
        let middle = above.height + (100.0 - above.height) / 2.0;
        snapshot.assert_pixel(caret_x + 0.5, middle, theme.foreground, "the caret");
        let primary = Rect::new(metrics.width, above.height, 300.0, 100.0 - above.height);
        assert!(snapshot.contains_color(primary, theme.accent), "the label");
    }

    #[test]
    fn test_short_of_room_segments_take_their_short_texts_before_any_is_left_out() {
        let notifier = Notifier::new();
        let line = StatusLine::new(plain("f"))
            .with_segment(StatusSegment::new(plain("wide-quota")).with_short(plain("q")))
            .with_segment(StatusSegment::new(plain("tail")));
        let drawn = |width: u32| {
            let mut bar = StatusBar::new();
            let mut frame = offscreen::plain_frame(width, 100);
            let cols = frame.cell_metrics().cols_in(width as f32);
            let area = Rect::new(0.0, 0.0, width as f32, 100.0);
            bar.lend(&line, &notifier)
                .draw(&mut frame, area, &Theme::dark());
            (cols, crate::testing::drawn_text(&frame).join("|"))
        };
        let (_, wide) = drawn(800);
        assert!(
            wide.contains("wide-quota") && wide.contains("tail"),
            "{wide}"
        );
        // An edge, the primary text, then 10 + 2 + 4 cells and an edge do not
        // fit 16 cells; the short text does.
        let metrics = offscreen::plain_frame(800, 100).cell_metrics();
        let narrow = (16.0 * metrics.width) as u32;
        let (cols, text) = drawn(narrow);
        assert!(cols < 1 + 1 + 1 + 10 + 2 + 4 + 1, "{cols} cells");
        assert!(!text.contains("wide-quota"), "{text}");
        assert!(text.contains('q') && text.contains("tail"), "{text}");
    }

    #[test]
    fn test_short_of_room_the_least_important_segment_goes_first_wherever_it_sits() {
        // 20 cells hold two 6 cell segments and a gap, not three.
        let layout = StatusLayout::new(20, None, None, &[], &[(6, 2), (6, 0), (6, 1)]);
        let kept: Vec<usize> = layout.right.iter().map(|(index, _)| *index).collect();
        assert_eq!(kept, [0, 2], "the middle one, least important, went");
        let equal = StatusLayout::new(20, None, None, &[], &[(6, 0), (6, 0), (6, 0)]);
        let kept: Vec<usize> = equal.right.iter().map(|(index, _)| *index).collect();
        assert_eq!(kept, [1, 2], "the leftmost of equals goes first");
    }

    #[test]
    fn test_with_pending_ignores_an_empty_prefix() {
        assert_eq!(
            StatusLine::new(StyledText::new()).with_pending("").pending,
            None
        );
    }

    #[test]
    fn test_the_bar_paints_its_badge_and_a_message_over_the_primary_text() {
        let theme = Theme::dark();
        let mut bar = StatusBar::new();
        let mut notifier = Notifier::new();
        notifier.set_routing(Routing {
            cards_from: None,
            status_bar: true,
        });
        notifier.notify(Notice::error("failed"));
        let line = StatusLine::new(Span::new("MMMM", ThemeToken::Accent.into()))
            .with_mode(ModeBadge::new("INSERT", ThemeToken::Warning));
        let area = Rect::new(0.0, 0.0, 400.0, 100.0);
        let mut above = Rect::default();
        let Some(snapshot) = offscreen::render(400, 100, |frame| {
            frame.clear(Rgba::from(theme.background));
            above = bar.lend(&line, &notifier).draw(frame, area, &theme);
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        let top = above.height;
        snapshot.assert_pixel(1.0, top, theme.border, "the top border");
        snapshot.assert_pixel(1.0, top + 2.0, theme.surface, "the bar's left edge");
        snapshot.assert_pixel(metrics.width, top + 2.0, theme.warning, "the badge");
        snapshot.assert_pixel(1.0, top - 1.0, theme.background, "above the bar");
        // The primary text starts after the badge (8 cells) and a gap.
        let primary = Rect::new(10.0 * metrics.width, top, 200.0, 100.0 - top);
        assert!(
            snapshot.contains_color(primary, theme.danger),
            "the message"
        );
        assert!(
            !snapshot.contains_color(primary, theme.accent),
            "the hidden text"
        );
    }
}
