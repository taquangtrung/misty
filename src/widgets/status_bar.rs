//! [`StatusBar`]: the one-line bar along the bottom of a window, showing
//! what the app hands it each frame in a [`StatusLine`], and the messages
//! and tasks of the window's [`Notifier`].
//!
//! From the left: a [`ModeBadge`] when the app is in a mode worth naming,
//! the keys of a pending sequence or count, the left segments (a project, a
//! branch), then the primary text (a location, a title), the one part that
//! is cut, with an ellipsis, when room runs out, and the segments that follow
//! it ([`StatusLine::with_after_primary`]), never cut. A prompt being typed into
//! the bar ([`StatusLine::with_prompt`]) takes the primary text's place,
//! its caret drawn where it is. The right segments (counts, a position) sit at the right
//! edge and are never cut: when they do not all fit, the leftmost go first.
//! A right segment ([`StatusSegment`]) may show an icon before its text,
//! one from the bar's [`IconSet`] or SVG data of its own
//! ([`StatusSegment::with_svg_icon`]), parsed once and kept while segments
//! show it. A segment marked stale ([`StatusSegment::with_stale`]), what it
//! says perhaps out of date, is drawn dimmed. One given an id is clickable: it lights
//! under the pointer, and a click on it answers
//! [`StatusBarResponse::Clicked`] with its id, for the app to act on (open
//! a picker of branches, cycle the line endings). One only named
//! ([`StatusSegment::with_name`]) neither lights nor takes clicks: the app
//! reads which it is under the pointer, for its tooltip.
//!
//! Everything on the bar is set upright at the regular weight, its color
//! alone telling one part from another: what the app hands it as bold or
//! italic is drawn plain, and its own messages, prompt label, and mode
//! badge are never stressed.
//!
//! The notifier's newest toast its routing puts on the status bar takes
//! the primary text's place while it lasts; the line is passed in on every
//! draw, so nothing the app changed under it is lost. While tasks run,
//! their item shows where the app put [`StatusSegment::progress`], on
//! either side or after the primary text, and nowhere if it put none: a
//! spinner, the oldest task's title and step, and how far it has come, or
//! how many run; a click on it
//! answers [`StatusBarResponse::Progress`], for the app to show the tasks
//! as cards ([`Notifier::show_task_cards`]). The spinner turns on the event
//! loop's deadlines: the lent bar's [`Widget::about_to_wait`] returns when
//! it turns next, and it stands still while motion is reduced
//! ([`Frame::reduced_motion`]).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::clock;
use super::super::base::color::Rgb;
use super::super::base::geometry::{Point, Rect};
use super::super::base::spinner;
use super::super::base::strings;
use super::super::base::styled::{Span, StyledText};
use super::super::base::text;
use super::super::base::theme::{LabelFont, Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::super::render::image::{IconSet, Svg};
use super::super::widgets::label::{draw_styled_label, styled_label_cells};
use crate::base::notice::{Notifier, Severity, Task};

// ========================================================================
// Constants
// ========================================================================

/// The space above and below the bar's text, in logical pixels.
const PADDING_Y: f32 = 3.0;

/// The thickness of the line along the bar's top, in logical pixels.
const STROKE: f32 = 1.0;

/// Upward optical shift for proportional label text inside a cell-height
/// line, in logical pixels: sans-serif ascent+descent sits slightly low.
const PROPORTIONAL_OPTICAL_SHIFT: f32 = 1.0;

/// The blank cells at each end of the bar.
const EDGE_CELLS: usize = 1;

/// The blank cells between two parts on the left.
const LEFT_GAP_CELLS: usize = 1;

/// The blank cells between two right segments.
const RIGHT_GAP_CELLS: usize = 1;

/// The thickness of a prompt's caret, in logical pixels.
const CARET: f32 = 2.0;

/// The blank cells at each side of a mode badge's label.
const BADGE_PADDING_CELLS: usize = 1;

/// The blank cells between a segment's icon and its text.
const ICON_GAP_CELLS: f32 = 0.5;

/// The icon's side as a share of the cell height.
const ICON_SHARE: f32 = 0.75;

/// The id of the progress item's segment, which no app segment takes.
const PROGRESS_SEGMENT: &str = StatusSegment::PROGRESS_ID;

/// What stands between a task's title and its step on the progress item.
const STEP_SEPARATOR: &str = " \u{00b7} ";

/// How many percent a whole task is.
const PERCENT: f32 = 100.0;

/// The opacity a stale segment's icon in its own colors is drawn at; a
/// tinted icon takes the muted color instead.
const STALE_ICON_OPACITY: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// A window's status bar. The app owns it and draws it with the frame's
/// [`StatusLine`] and the window's [`Notifier`], offering it the window's
/// pointer input for its clickable segments.
#[derive(Clone, Debug, Default)]
pub struct StatusBar {
    /// The theme color the bar is filled with, or `None` for the bars'
    /// surface color.
    background: Option<ThemeToken>,
    /// The bar as last drawn, or `None` before the first frame.
    bar: Option<Rect>,
    /// Each clickable segment shown in the last frame drawn, by id, with
    /// where it is.
    clickable: Vec<(&'static str, Rect)>,
    /// Each segment only named shown in the last frame drawn, by name, with
    /// where it is.
    named: Vec<(&'static str, Rect)>,
    /// The segment under the pointer, clickable or named, by id.
    hovered: Option<&'static str>,
    /// The icons segments name.
    icons: IconSet,
    /// The clickable segment pressed and not yet released, by id.
    pressed: Option<&'static str>,
    /// The SVG data segments carry as their icons, parsed, or `None` when
    /// it is not an SVG document; only the data shown in the last frame
    /// drawn is kept.
    svg_icons: HashMap<Arc<[u8]>, Option<Svg>>,
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
    /// The segments right after the primary text, left to right, never
    /// cut.
    after: Vec<StatusSegment>,
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
/// where the caret is. Made with [`StatusLine::with_prompt`] and read back
/// with [`StatusLine::prompt`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusPrompt {
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
    /// The icon before its text, or `None`.
    icon: Option<SegmentIcon>,
    /// What a click on it answers with, or the name the app knows it by
    /// under the pointer, or `None` for neither.
    id: Option<&'static str>,
    /// Whether a click on it answers with its id, and the pointer lights
    /// it.
    pressable: bool,
    /// How much it matters: short of room, the right side leaves out its
    /// least important segments first, the leftmost of equals.
    priority: u32,
    /// Whether it only marks where the notifier's progress item goes, and
    /// whether that item shows its details.
    progress: Option<ProgressMark>,
    /// A shorter text shown in its place when the right side does not fit
    /// whole, before any segment is left out.
    short: Option<StyledText>,
    /// Whether what it says may be out of date, drawn dimmed.
    stale: bool,
    /// The text shown.
    text: StyledText,
}

/// Where a [`StatusLine`] puts the notifier's progress item, made with
/// [`StatusSegment::progress`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct ProgressMark {
    /// Whether the item shows the oldest task's step after its title.
    details: bool,
}

/// The icon before a [`StatusSegment`]'s text.
#[derive(Clone, Debug, PartialEq)]
enum SegmentIcon {
    /// The icon of this name in the bar's [`IconSet`], tinted unless the
    /// set says it keeps its own colors.
    Named(String),
    /// This SVG document's data, drawn in its own colors.
    Svg(Arc<[u8]>),
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
    /// Each segment after the primary text, by index, with its first cell.
    after: Vec<(usize, usize)>,
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

    /// This bar filled with the theme color `background` rather than the
    /// bars' surface color: [`ThemeToken::Background`] for a bar that
    /// reads as part of the content above it, as a terminal's does.
    pub fn with_background(self, background: ThemeToken) -> Self {
        Self {
            background: Some(background),
            ..self
        }
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

    /// Where the clickable or named segment `id` was last drawn, for a
    /// tooltip hung over it; `None` when it was not drawn.
    pub fn segment_rect(&self, id: &str) -> Option<Rect> {
        self.clickable
            .iter()
            .chain(&self.named)
            .find(|(segment, _)| *segment == id)
            .map(|(_, rect)| *rect)
    }

    /// The clickable or named segment under the pointer, by id, for a
    /// tooltip the app shows for it.
    pub fn hovered(&self) -> Option<&'static str> {
        self.hovered
    }

    /// The height of the bar in a window whose cells are `metrics` in size
    /// on a display scaled by `scale_factor`, for an app laying out what
    /// sits above it.
    pub fn height(metrics: CellMetrics, scale_factor: f64) -> f32 {
        (logical(STROKE, scale_factor) + metrics.height + 2.0 * logical(PADDING_Y, scale_factor))
            .round()
    }

    /// Draws `line` along the bottom of `area` in `theme`'s colors,
    /// `notifier`'s message in place of its primary text and its tasks'
    /// item where the line marks it, and returns the part of `area` above
    /// the bar, which is the app's to draw content in.
    fn draw_line(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        line: &StatusLine,
        notifier: &Notifier,
        theme: &Theme,
    ) -> Rect {
        let now = (!frame.reduced_motion()).then(Instant::now);
        let tasks = notifier.tasks();
        let left = with_progress(&line.left, tasks, now);
        let after = with_progress(&line.after, tasks, now);
        let right = with_progress(&line.right, tasks, now);
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let height = Self::height(metrics, scale_factor).min(area.height);
        let bar = Rect::new(area.x, area.y + area.height - height, area.width, height);
        let fill = self
            .background
            .map_or(theme.surface, |token| theme.color(token));
        frame.fill_rect(bar, fill);
        let stroke = logical(STROKE, scale_factor);
        let interior_y = bar.y + stroke;
        let interior_h = (bar.height - stroke).max(0.0);
        let top = interior_y + ((interior_h - metrics.height) / 2.0).round();
        let optical_shift = match theme.label_font {
            LabelFont::Cells => 0.0,
            LabelFont::Proportional => logical(PROPORTIONAL_OPTICAL_SHIFT, scale_factor),
        };
        let at = |cell: usize| Point::new(bar.x + cell as f32 * metrics.width, top - optical_shift);
        // Measured as they are drawn, so each label fits the cells it is
        // laid out in whatever the label font.
        let cells = |segment: &StatusSegment| segment.width(frame, theme);
        let left_widths: Vec<usize> = left.iter().map(cells).collect();
        let after_widths: Vec<usize> = after.iter().map(cells).collect();
        let badge = line.mode.as_ref().map(|mode| mode.width(frame, theme));
        let pending = line
            .pending
            .as_deref()
            .map(|pending| styled_label_cells(frame, theme, &pending_label(pending)));
        let primary = primary_text(line, notifier, theme);
        let primary_width = styled_label_cells(frame, theme, &primary);
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
            )
            .with_after(primary_width, &after_widths);
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
            let label = mode.styled_label();
            draw_styled_label(frame, theme, &label, at(cell + BADGE_PADDING_CELLS), bar);
        }
        if let (Some(pending), Some((cell, _))) = (&line.pending, layout.pending) {
            draw_styled_label(frame, theme, &pending_label(pending), at(cell), bar);
        }
        let (cell, room) = layout.primary;
        let clip = Rect::new(at(cell).x, bar.y, room as f32 * metrics.width, bar.height);
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
        self.named.clear();
        let mut shown_svgs = HashSet::new();
        let mut segments: Vec<(&StatusSegment, usize, usize)> = Vec::new();
        for &(index, cell) in &layout.left {
            segments.push((&left[index], cell, left_widths[index]));
        }
        for &(index, cell) in &layout.after {
            segments.push((&after[index], cell, after_widths[index]));
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
                match segment.pressable {
                    true => {
                        if self.hovered == Some(id) || self.pressed == Some(id) {
                            frame.fill_rect(rect, theme.hover);
                        }
                        self.clickable.push((id, rect));
                    }
                    false => self.named.push((id, rect)),
                }
            }
            let mut text_origin = origin;
            if let Some(icon) = &segment.icon {
                let size = (metrics.height * ICON_SHARE).round();
                let gap = (ICON_GAP_CELLS * metrics.width).round();
                text_origin.x += size + gap;
                let y = (interior_y + (interior_h - size) / 2.0).round();
                let rect = Rect::new(origin.x, y, size, size);
                let svg = match icon {
                    SegmentIcon::Named(_) => None,
                    SegmentIcon::Svg(data) => {
                        shown_svgs.insert(Arc::clone(data));
                        self.parsed_svg(data)
                    }
                };
                let own_colors = match icon {
                    SegmentIcon::Named(name) => self.icons.is_colored(name),
                    SegmentIcon::Svg(_) => true,
                };
                let opacity = match segment.stale && own_colors {
                    true => STALE_ICON_OPACITY,
                    false => 1.0,
                };
                let tint = segment.color(theme);
                let icons = &self.icons;
                frame.with_opacity(opacity, |frame| match (icon, &svg) {
                    (SegmentIcon::Named(name), _) => {
                        frame.draw_named_icon(icons, name, rect, tint);
                    }
                    (SegmentIcon::Svg(_), Some(svg)) => frame.draw_svg(svg, rect),
                    (SegmentIcon::Svg(_), None) => {}
                });
            }
            draw_styled_label(frame, theme, &segment.shown_text(), text_origin, bar);
        }
        self.svg_icons.retain(|data, _| shown_svgs.contains(data));
        // Last, so the badge's and a hovered segment's fills leave it whole.
        frame.fill_rect_over_text(Rect::new(bar.x, bar.y, bar.width, stroke), theme.border);
        Rect::new(area.x, area.y, area.width, (area.height - height).max(0.0))
    }

    /// The SVG document `data` holds, parsed on its first use and kept
    /// from then on; `None` when it is not an SVG document.
    fn parsed_svg(&mut self, data: &Arc<[u8]>) -> Option<Svg> {
        self.svg_icons
            .entry(Arc::clone(data))
            .or_insert_with(|| Svg::parse(data).ok())
            .clone()
    }

    /// The clickable segment at `point` as last drawn, by id.
    fn segment_at(&self, point: Point) -> Option<&'static str> {
        self.clickable
            .iter()
            .find(|(_, rect)| rect.contains(point))
            .map(|(id, _)| *id)
    }

    /// The clickable or named segment at `point` as last drawn, by id.
    fn part_at(&self, point: Point) -> Option<&'static str> {
        self.clickable
            .iter()
            .chain(&self.named)
            .find(|(_, rect)| rect.contains(point))
            .map(|(id, _)| *id)
    }

    /// Reacts to `input`: the light under the pointer, and a click on a
    /// clickable segment.
    fn handle_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> StatusBarResponse {
        match input {
            Input::PointerMoved(point) => {
                let hovered = self.part_at(*point);
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
    /// item where the line marks it, and returns the part of `area` above the bar,
    /// which is the app's to draw content in.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        let (line, notifier) = self.model;
        self.widget.draw_line(frame, area, line, notifier, theme)
    }

    /// Asks for a redraw while the lent notifier has tasks running and the
    /// line shows their item, and returns when its spinner turns next;
    /// `None` while none runs, the line marks no place for it, or motion is
    /// reduced, when the spinner stands still.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let (line, notifier) = self.model;
        if !line.shows_progress() {
            return None;
        }
        let task = notifier.tasks().first()?;
        if cx.reduced_motion() {
            return None;
        }
        cx.request_redraw();
        Some(spinner::next_turn(task.started, clock::now()))
    }
}

// ========================================================================
// StatusLine Implementations
// ========================================================================

impl StatusLine {
    /// A line showing `primary` and nothing else.
    pub fn new(primary: impl Into<StyledText>) -> Self {
        Self {
            primary: primary.into().upright(),
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

    /// This line with `segment` added at the right end of the segments
    /// right after its primary text, which is cut before they are.
    pub fn with_after_primary(mut self, segment: StatusSegment) -> Self {
        self.after.push(segment);
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

    /// The text cut to fit the room the rest leaves, as given to
    /// [`StatusLine::new`].
    pub fn primary(&self) -> &StyledText {
        &self.primary
    }

    /// The mode named at the left, or `None` in the app's default mode.
    pub fn mode(&self) -> Option<&ModeBadge> {
        self.mode.as_ref()
    }

    /// The keys of a pending sequence or count, or `None`.
    pub fn pending(&self) -> Option<&str> {
        self.pending.as_deref()
    }

    /// The prompt in the primary text's place, or `None`.
    pub fn prompt(&self) -> Option<&StatusPrompt> {
        self.prompt.as_ref()
    }

    /// The segments at the left, left to right.
    pub fn left(&self) -> &[StatusSegment] {
        &self.left
    }

    /// The segments right after the primary text, left to right.
    pub fn after_primary(&self) -> &[StatusSegment] {
        &self.after
    }

    /// The segments at the right, left to right.
    pub fn right(&self) -> &[StatusSegment] {
        &self.right
    }

    /// Whether any of its segments marks where the progress item goes.
    fn shows_progress(&self) -> bool {
        self.left
            .iter()
            .chain(&self.after)
            .chain(&self.right)
            .any(|segment| segment.progress.is_some())
    }
}

// ========================================================================
// StatusPrompt Implementations
// ========================================================================

impl StatusPrompt {
    /// What it asks (`Rename:`).
    pub fn label(&self) -> &str {
        &self.label
    }

    /// What is typed.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The caret, in characters into [`StatusPrompt::text`].
    pub fn caret(&self) -> usize {
        self.caret
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
    /// The id the progress item answers to under the pointer, for a
    /// tooltip the app shows for it.
    pub const PROGRESS_ID: &'static str = "misty.status-bar.progress";

    /// The place the notifier's progress item goes: shown here while tasks
    /// run, a click on it answering [`StatusBarResponse::Progress`], and
    /// left out while none does. It says what runs (a spinner, the oldest
    /// task's title, how far it has come, how many more run), then the
    /// task's step, its details, unless [`StatusSegment::with_details`]
    /// leaves them out.
    pub fn progress() -> Self {
        Self {
            progress: Some(ProgressMark { details: true }),
            ..Self::new(StyledText::new())
        }
    }

    /// This progress item showing the oldest task's step after the rest, or
    /// not; a segment that is no progress item is left as it is.
    pub fn with_details(self, details: bool) -> Self {
        Self {
            progress: self.progress.map(|_| ProgressMark { details }),
            ..self
        }
    }

    /// A segment showing `text`, neither clickable nor with an icon.
    pub fn new(text: impl Into<StyledText>) -> Self {
        Self {
            icon: None,
            id: None,
            pressable: false,
            priority: 0,
            progress: None,
            short: None,
            stale: false,
            text: text.into().upright(),
        }
    }

    /// This segment with the icon named `icon` in the bar's [`IconSet`]
    /// before its text, in place of any icon given before.
    pub fn with_icon(self, icon: impl Into<String>) -> Self {
        Self {
            icon: Some(SegmentIcon::Named(icon.into())),
            ..self
        }
    }

    /// This segment with the SVG document `data` as its icon, drawn in its
    /// own colors before its text, in place of any icon given before: a
    /// logo the app holds as bytes, needing no name in the bar's
    /// [`IconSet`]. The bar parses it the first time it is drawn and keeps
    /// it while segments show it, so passing the same data every frame
    /// costs no parse. Data that is not an SVG document draws no icon, its
    /// cells left blank.
    pub fn with_svg_icon(self, data: impl Into<Arc<[u8]>>) -> Self {
        Self {
            icon: Some(SegmentIcon::Svg(data.into())),
            ..self
        }
    }

    /// This segment marked stale (`true`): what it says may be out of
    /// date, such as a figure whose refresh failed. Its text is drawn in
    /// the theme's muted color, as misty dims secondary and disabled
    /// items, its tinted icon too, and an icon in its own colors faded.
    pub fn with_stale(self, stale: bool) -> Self {
        Self { stale, ..self }
    }

    /// This segment clickable, a click on it answering
    /// [`StatusBarResponse::Clicked`] with `id`.
    pub fn with_id(self, id: &'static str) -> Self {
        Self {
            id: Some(id),
            pressable: true,
            ..self
        }
    }

    /// This segment known to the app as `name` while the pointer is over
    /// it, for a tooltip, and no more: it neither lights nor takes clicks.
    pub fn with_name(self, name: &'static str) -> Self {
        Self {
            id: Some(name),
            pressable: false,
            ..self
        }
    }

    /// This segment with `short` shown in its place when the right side
    /// does not fit whole: a quota as `3%` rather than `3% of 5h, resets
    /// 20:39`.
    pub fn with_short(self, short: impl Into<StyledText>) -> Self {
        Self {
            short: Some(short.into().upright()),
            ..self
        }
    }

    /// This segment kept over those of a lower `priority` when the right
    /// side does not fit; all are 0 unless given one.
    pub fn with_priority(self, priority: u32) -> Self {
        Self { priority, ..self }
    }

    /// The text shown.
    pub fn text(&self) -> &StyledText {
        &self.text
    }

    /// The shorter text shown in its place short of room, or `None`.
    pub fn short(&self) -> Option<&StyledText> {
        self.short.as_ref()
    }

    /// The id a click on it answers with, or the name it is known by under
    /// the pointer; `None` for neither.
    pub fn id(&self) -> Option<&'static str> {
        self.id
    }

    /// How much it matters when the right side is short of room.
    pub fn priority(&self) -> u32 {
        self.priority
    }

    /// The name of its icon in the bar's [`IconSet`], or `None` when it
    /// has none or carries SVG data instead.
    pub fn icon(&self) -> Option<&str> {
        match &self.icon {
            Some(SegmentIcon::Named(name)) => Some(name),
            Some(SegmentIcon::Svg(_)) | None => None,
        }
    }

    /// The SVG data of its icon, or `None` when it has none or names one
    /// in the bar's [`IconSet`] instead.
    pub fn svg_icon(&self) -> Option<&Arc<[u8]>> {
        match &self.icon {
            Some(SegmentIcon::Svg(data)) => Some(data),
            Some(SegmentIcon::Named(_)) | None => None,
        }
    }

    /// Whether it is marked stale, drawn dimmed.
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// The text as drawn: in the muted color when stale.
    fn shown_text(&self) -> Cow<'_, StyledText> {
        if !self.stale {
            return Cow::Borrowed(&self.text);
        }
        let muted = self
            .text
            .spans()
            .iter()
            .map(|span| Span {
                color: ThemeToken::Muted.into(),
                ..span.clone()
            })
            .fold(StyledText::new(), StyledText::with);
        Cow::Owned(muted)
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

    /// The cells it takes as drawn in `theme`'s label font, its icon and
    /// the gap after it included.
    fn width(&self, frame: &Frame, theme: &Theme) -> usize {
        let metrics = frame.cell_metrics();
        let icon = match self.icon {
            Some(_) => {
                let size = (metrics.height * ICON_SHARE).round();
                let gap = (ICON_GAP_CELLS * metrics.width).round();
                ((size + gap) / metrics.width).ceil() as usize
            }
            None => 0,
        };
        icon + styled_label_cells(frame, theme, &self.text)
    }

    /// The color a tinted icon takes: the muted color when stale, else its
    /// text's first color, or the theme's foreground.
    fn color(&self, theme: &Theme) -> Rgb {
        if self.stale {
            return theme.muted;
        }
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

    /// The mode's name shown.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The badge's color.
    pub fn color(&self) -> ThemeToken {
        self.color
    }

    /// The cells the badge takes as drawn in `theme`'s label font, its
    /// padding included.
    fn width(&self, frame: &Frame, theme: &Theme) -> usize {
        styled_label_cells(frame, theme, &self.styled_label()) + 2 * BADGE_PADDING_CELLS
    }

    /// The badge's label: its mode on the badge's color.
    fn styled_label(&self) -> StyledText {
        StyledText::plain(&self.label, ThemeToken::Background.into())
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
            after: Vec::new(),
            badge,
            left,
            pending,
            primary: (primary_start, primary_end.saturating_sub(primary_start)),
            right: shown,
        }
    }

    /// This layout with segments of `after` widths right after a primary
    /// text `primary` cells wide, which keeps only the room they leave it.
    /// With none, the primary text keeps every cell between.
    fn with_after(self, primary: usize, after: &[usize]) -> Self {
        if after.is_empty() {
            return self;
        }
        let (start, room) = self.primary;
        let span: usize = after.iter().map(|width| width + LEFT_GAP_CELLS).sum();
        let kept = primary.min(room.saturating_sub(span));
        let mut next = match kept {
            0 => start,
            _ => start + kept + LEFT_GAP_CELLS,
        };
        let after = after
            .iter()
            .enumerate()
            .map(|(index, &width)| {
                let cell = next;
                next += width + LEFT_GAP_CELLS;
                (index, cell)
            })
            .collect();
        Self {
            after,
            primary: (start, kept),
            ..self
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

/// What `line` shows as its primary text: `notifier`'s message while it
/// lasts and no prompt is typed, the prompt with its label, or the line's
/// own text, with an ellipsis when cut.
fn primary_text(line: &StatusLine, notifier: &Notifier, theme: &Theme) -> StyledText {
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
            StyledText::from(Span::new(text, color.into()))
        }
        None => match &line.prompt {
            Some(prompt) => StyledText::new()
                .with(Span::new(
                    format!("{} ", prompt.label),
                    ThemeToken::Accent.into(),
                ))
                .with(Span::new(
                    prompt.text.as_str(),
                    ThemeToken::Foreground.into(),
                )),
            None => line.primary.clone(),
        },
    };
    primary.with_ellipsis()
}

/// `segments` with the progress item for `tasks` at `now` in place of any
/// segment marking it, and those left out while no task runs.
fn with_progress(
    segments: &[StatusSegment],
    tasks: &[Task],
    now: Option<Instant>,
) -> Vec<StatusSegment> {
    segments
        .iter()
        .filter_map(|segment| match segment.progress {
            Some(mark) => progress_segment(tasks, now, mark.details),
            None => Some(segment.clone()),
        })
        .collect()
}

/// The progress item for `tasks` at `now`, or `None` while none runs: a
/// spinner turning from the oldest's start (standing at its first glyph
/// for no `now`, while motion is reduced), its title, how far it has come,
/// and how many more run, then with `details` its step.
fn progress_segment(tasks: &[Task], now: Option<Instant>, details: bool) -> Option<StatusSegment> {
    let task = tasks.first()?;
    let now = now.unwrap_or(task.started);
    let mut text = format!("{} {}", spinner::glyph_at(task.started, now), task.title);
    if let Some(fraction) = task.fraction {
        text.push_str(&format!(" {:.0}%", fraction * PERCENT));
    }
    if tasks.len() > 1 {
        text.push_str(&format!(" +{}", tasks.len() - 1));
    }
    if let Some(step) = task.step.as_ref().filter(|_| details) {
        text.push_str(STEP_SEPARATOR);
        text.push_str(step);
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
    use crate::base::theme::LabelFont;
    use crate::input::event::ButtonInput;
    use crate::render::draw::PictureSource;
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
    fn test_nothing_on_the_bar_is_bold_or_italic_whatever_it_was_handed() {
        let badge = ModeBadge::new("NORMAL", ThemeToken::Accent);
        assert!(!badge.styled_label().spans()[0].bold, "the mode badge");
        let stressed = || {
            StyledText::from(
                Span::new("main", ThemeToken::Foreground.into())
                    .bold()
                    .italic(),
            )
        };
        let line = StatusLine::new(stressed())
            .with_segment(StatusSegment::new(stressed()).with_short(stressed()));
        let segment = &line.right()[0];
        for text in [
            line.primary(),
            segment.text(),
            segment.short().expect("short"),
        ] {
            let span = &text.spans()[0];
            assert!(!span.bold && !span.italic, "{span:?}");
        }
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
        let icon_cells = (((metrics.height * ICON_SHARE).round()
            + (ICON_GAP_CELLS * metrics.width).round())
            / metrics.width)
            .ceil();
        // "main" (4 cells) + its icon and gap, and half a cell either side.
        assert_eq!(
            rect.width,
            (4.0 + icon_cells) * metrics.width + 2.0 * (metrics.width / 2.0).round()
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
    fn test_a_named_segment_is_found_under_the_pointer_but_takes_no_click() {
        let mut window = TestWindow::new(400, 100);
        let mut bar = StatusBar::new();
        let notifier = Notifier::new();
        let line = StatusLine::new(plain("file"))
            .with_segment(StatusSegment::new(plain("main")).with_name("branch"));
        let area = Rect::new(0.0, 0.0, 400.0, 100.0);
        bar.lend(&line, &notifier)
            .draw(&mut window.frame(), area, &Theme::dark());
        assert!(bar.clickable.is_empty(), "not clickable");
        let rect = bar.segment_rect("branch").expect("found by its name");
        let (x, y) = (rect.x + 2.0, rect.y + 2.0);
        let mut send = |input: Input, bar: &mut StatusBar| {
            window.send(&input, |input, cx| {
                bar.lend(&line, &notifier).handle(input, cx)
            })
        };
        send(Input::PointerMoved(Point::new(x, y)), &mut bar);
        assert_eq!(bar.hovered(), Some("branch"), "for its tooltip");
        assert_eq!(send(left(true, x, y), &mut bar), StatusBarResponse::Ignored);
        assert_eq!(
            send(left(false, x, y), &mut bar),
            StatusBarResponse::Ignored
        );
    }

    #[test]
    fn test_a_wide_segment_in_the_proportional_font_is_laid_out_cells_enough_to_hold_it() {
        let frame = offscreen::plain_frame(400, 100);
        let theme = Theme {
            label_font: LabelFont::Proportional,
            ..Theme::dark()
        };
        let segment = StatusSegment::new(plain("WWWWWW"));
        let width = styled_label_width(&frame, &theme, &segment.text);
        let cells = segment.width(&frame, &theme);
        assert!(cells > 6, "more than its characters: {cells}");
        assert!(cells as f32 * frame.cell_metrics().width >= width);
    }

    #[test]
    fn test_an_icon_takes_space_and_a_gap_before_its_segments_text() {
        let frame = offscreen::plain_frame(400, 100);
        let theme = Theme::dark();
        let metrics = frame.cell_metrics();
        let plain = StatusSegment::new(plain("main"));
        let plain_cells = plain.width(&frame, &theme);
        let with_icon_cells = plain.with_icon("branch").width(&frame, &theme);
        let icon_px = (metrics.height * ICON_SHARE).round();
        let gap_px = (ICON_GAP_CELLS * metrics.width).round();
        assert_eq!(
            with_icon_cells - plain_cells,
            ((icon_px + gap_px) / metrics.width).ceil() as usize
        );
    }

    #[test]
    fn test_left_parts_stack_and_right_segments_sit_at_the_right_end() {
        // 1 edge, a 8-cell badge at 1, keys at 10, primary from 13.
        let layout = StatusLayout::new(60, Some(8), Some(2), &[], &[(5, 0), (3, 0)]);
        assert_eq!(layout.badge, Some((1, 8)));
        assert_eq!(layout.pending, Some((10, 2)));
        // The last segment ends at 59, the edge; the first 1 cell before it.
        assert_eq!(layout.right, [(0, 50), (1, 56)]);
        assert_eq!(layout.primary, (13, 36));
    }

    #[test]
    fn test_right_segments_that_do_not_fit_are_dropped_leftmost_first_never_cut() {
        let layout = StatusLayout::new(20, None, None, &[], &[(10, 0), (4, 0), (6, 0)]);
        // 6 at 13..19, 4 at 8..12; 10 more would start before the left.
        assert_eq!(layout.right, [(1, 8), (2, 13)]);
        assert_eq!(layout.primary, (1, 6));
        let narrow = StatusLayout::new(4, None, None, &[], &[(6, 0)]);
        assert!(narrow.right.is_empty());
        assert_eq!(narrow.primary, (1, 2));
    }

    #[test]
    fn test_the_progress_item_names_the_oldest_task_how_far_how_many_more_then_its_step() {
        let now = Instant::now();
        let mut notifier = Notifier::new();
        let build = notifier.start(Progress::new("Building"), now);
        notifier.set_step(build, Some("3/10".into()));
        notifier.set_fraction(build, Some(0.4));
        notifier.start(Progress::new("Indexing"), now);
        let text = |details: bool| -> String {
            let segment = progress_segment(notifier.tasks(), Some(now), details).expect("an item");
            assert_eq!(segment.id, Some(PROGRESS_SEGMENT));
            segment
                .text
                .spans()
                .iter()
                .map(|span| span.text.as_str())
                .collect()
        };
        let full = text(true);
        assert!(full.ends_with("Building 40% +1 \u{00b7} 3/10"), "{full}");
        let brief = text(false);
        assert!(brief.ends_with("Building 40% +1"), "{brief}");
        assert_eq!(progress_segment(&[], Some(now), true), None);
    }

    #[test]
    fn test_with_motion_reduced_the_spinner_stands_still_and_asks_no_frames() {
        let started = Instant::now()
            .checked_sub(spinner::FRAME * 3)
            .expect("a clock past three frames");
        let mut notifier = Notifier::new();
        notifier.start(Progress::new("Building"), started);
        let line = StatusLine::new(plain("f")).with_after_primary(StatusSegment::progress());
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
            .draw(&mut frame, area, &Theme::dark().with_grid_look());
        let first = spinner::glyph_at(started, started).to_string();
        let turned = spinner::glyph_at(started, started + spinner::FRAME * 3).to_string();
        let pieces = crate::testing::drawn_text(&frame);
        assert!(pieces.contains(&first), "the first glyph: {pieces:?}");
        assert!(!pieces.contains(&turned), "not turned on: {pieces:?}");
    }

    #[test]
    fn test_segments_after_the_primary_text_follow_it_and_cut_it_first() {
        let layout = StatusLayout::new(60, None, None, &[], &[(5, 0)]);
        // The primary text from 1, 52 cells to the gap before the right segment at 54.
        let short = layout.clone().with_after(10, &[4, 6]);
        assert_eq!(short.primary, (1, 10));
        assert_eq!(short.after, [(0, 12), (1, 17)]);
        // Too long to fit beside them: cut to the 52 less their 12 cells.
        let long = layout.clone().with_after(80, &[4, 6]);
        assert_eq!(long.primary, (1, 40));
        assert_eq!(long.after, [(0, 42), (1, 47)]);
        // An empty primary text leaves no gap before them.
        assert_eq!(layout.clone().with_after(0, &[4]).after, [(0, 1)]);
        assert_eq!(layout.clone().with_after(80, &[]).primary, (1, 52));
    }

    #[test]
    fn test_the_progress_item_shows_only_where_the_line_marks_it() {
        let mut notifier = Notifier::new();
        notifier.start(Progress::new("Building"), Instant::now());
        let drawn = |line: &StatusLine, notifier: &Notifier| {
            let mut bar = StatusBar::new();
            let window = TestWindow::new(600, 100);
            let mut frame = window.frame();
            let area = Rect::new(0.0, 0.0, 600.0, 100.0);
            bar.lend(line, notifier)
                .draw(&mut frame, area, &Theme::dark().with_grid_look());
            let shown = crate::testing::drawn_text(&frame).concat();
            (shown, bar.segment_rect(PROGRESS_SEGMENT))
        };
        let unmarked = StatusLine::new(plain("main.rs"));
        let (shown, rect) = drawn(&unmarked, &notifier);
        assert!(!shown.contains("Building") && rect.is_none(), "{shown}");
        let marked = unmarked.with_after_primary(StatusSegment::progress());
        let (shown, rect) = drawn(&marked, &notifier);
        let primary_end = shown.find("main.rs").expect("the primary text") + "main.rs".len();
        let item = shown.find("Building").expect("the item");
        assert!(rect.is_some() && primary_end <= item, "{shown}");
        let (shown, rect) = drawn(&marked, &Notifier::new());
        assert!(!shown.contains("Building") && rect.is_none(), "{shown}");
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
                .draw(&mut frame, area, &Theme::dark().with_grid_look());
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

    /// A 16 pixel square SVG document filled red.
    const RED_SQUARE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="red"/></svg>"#;

    /// `line` drawn by `bar` on a plain frame 400 by 100 in the dark theme.
    fn draw_plain(bar: &mut StatusBar, line: &StatusLine) -> Frame {
        let mut frame = offscreen::plain_frame(400, 100);
        let area = Rect::new(0.0, 0.0, 400.0, 100.0);
        bar.lend(line, &Notifier::new())
            .draw(&mut frame, area, &Theme::dark());
        frame
    }

    /// The color the text run saying `text` was drawn in, in `frame`.
    fn run_color(frame: &Frame, text: &str) -> Option<Rgb> {
        frame
            .draw
            .text()
            .iter()
            .flat_map(|segment| &segment.runs)
            .find(|run| run.text.contains(text))
            .map(|run| run.color)
    }

    #[test]
    fn test_a_line_and_its_segments_read_back_what_they_were_built_with() {
        let segment = StatusSegment::new(plain("42%"))
            .with_short(plain("4"))
            .with_id("quota")
            .with_priority(3)
            .with_stale(true)
            .with_svg_icon(RED_SQUARE);
        let line = StatusLine::new(plain("/query"))
            .with_mode(ModeBadge::new("INSERT", ThemeToken::Warning))
            .with_pending("2d")
            .with_prompt("Rename:", "abc", 1)
            .with_left(StatusSegment::new(plain("main")).with_icon("branch"))
            .with_segment(segment);
        assert_eq!(line.primary(), &plain("/query"));
        let mode = line.mode().expect("a mode");
        assert_eq!(
            (mode.label(), mode.color()),
            ("INSERT", ThemeToken::Warning)
        );
        assert_eq!(line.pending(), Some("2d"));
        let prompt = line.prompt().expect("a prompt");
        assert_eq!(
            (prompt.label(), prompt.text(), prompt.caret()),
            ("Rename:", "abc", 1)
        );
        let [left] = line.left() else {
            panic!("one left segment: {:?}", line.left());
        };
        assert_eq!(left.icon(), Some("branch"));
        assert_eq!(left.svg_icon(), None);
        assert!(!left.is_stale());
        let [right] = line.right() else {
            panic!("one right segment: {:?}", line.right());
        };
        assert_eq!(right.text(), &plain("42%"));
        assert_eq!(right.short(), Some(&plain("4")));
        assert_eq!(right.id(), Some("quota"));
        assert_eq!(right.priority(), 3);
        assert!(right.is_stale());
        assert_eq!(
            right.icon(),
            None,
            "the SVG data took the named icon's place"
        );
        assert_eq!(right.svg_icon().map(|data| &data[..]), Some(RED_SQUARE));
    }

    #[test]
    fn test_a_stale_segment_is_drawn_in_the_muted_color() {
        let theme = Theme::dark();
        assert_ne!(theme.muted, theme.foreground);
        let line = StatusLine::new(plain("f"))
            .with_segment(StatusSegment::new(plain("fresh")))
            .with_segment(StatusSegment::new(plain("stale")).with_stale(true));
        let frame = draw_plain(&mut StatusBar::new(), &line);
        assert_eq!(run_color(&frame, "fresh"), Some(theme.foreground));
        assert_eq!(run_color(&frame, "stale"), Some(theme.muted));
    }

    #[test]
    fn test_a_segments_svg_icon_is_drawn_unregistered_and_parsed_once_while_shown() {
        let data: Arc<[u8]> = RED_SQUARE.into();
        let line = StatusLine::new(plain("f"))
            .with_segment(StatusSegment::new(plain("logo")).with_svg_icon(Arc::clone(&data)));
        let mut bar = StatusBar::new();
        let svg_drawn = |frame: &Frame| {
            let [picture] = frame.draw.pictures() else {
                panic!("one picture: {}", frame.draw.pictures().len());
            };
            assert_eq!(picture.tint, None, "in its own colors");
            match &picture.source {
                PictureSource::Svg(svg) => (svg.id(), picture.opacity),
                other => panic!("an SVG: {other:?}"),
            }
        };
        let (first, opacity) = svg_drawn(&draw_plain(&mut bar, &line));
        assert_eq!(opacity, 1.0);
        let (again, _) = svg_drawn(&draw_plain(&mut bar, &line));
        assert_eq!(first, again, "the parsed document is kept");
        assert!(bar.icons.get("logo").is_none(), "nothing registered");
        let stale = StatusLine::new(plain("f")).with_segment(
            StatusSegment::new(plain("logo"))
                .with_svg_icon(data)
                .with_stale(true),
        );
        let (_, faded) = svg_drawn(&draw_plain(&mut bar, &stale));
        assert_eq!(
            faded, STALE_ICON_OPACITY,
            "a stale icon in its own colors is faded"
        );
        draw_plain(&mut bar, &StatusLine::new(plain("f")));
        assert!(bar.svg_icons.is_empty(), "dropped once no segment shows it");
    }

    #[test]
    fn test_with_pending_ignores_an_empty_prefix() {
        assert_eq!(
            StatusLine::new(StyledText::new()).with_pending("").pending,
            None
        );
    }

    #[test]
    fn test_a_bar_given_a_background_is_filled_with_it() {
        let theme = Theme::dark();
        let mut bar = StatusBar::new().with_background(ThemeToken::Background);
        let notifier = Notifier::new();
        let line = StatusLine::new(Span::new("", ThemeToken::Foreground.into()));
        let area = Rect::new(0.0, 0.0, 400.0, 100.0);
        let mut above = Rect::default();
        let Some(snapshot) = offscreen::render(400, 100, |frame| {
            frame.clear(Rgba::from(theme.raised));
            above = bar.lend(&line, &notifier).draw(frame, area, &theme);
        }) else {
            return;
        };
        snapshot.assert_pixel(1.0, above.height + 2.0, theme.background, "the bar");
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

    #[test]
    fn test_status_bar_text_is_vertically_centered_below_the_border() {
        let theme = Theme::dark();
        let mut bar = StatusBar::new();
        let notifier = Notifier::new();
        let line = StatusLine::new(plain("")).with_prompt("Rename:", "abcd", 2);
        let area = Rect::new(0.0, 0.0, 400.0, 100.0);
        let mut above = Rect::default();
        let Some(snapshot) = offscreen::render(400, 100, |frame| {
            frame.clear(Rgba::from(theme.background));
            above = bar.lend(&line, &notifier).draw(frame, area, &theme);
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        let caret_x = 11.0 * metrics.width + 0.5;
        let interior_top = above.height + logical(STROKE, 1.0);
        let pad = logical(PADDING_Y, 1.0);
        assert_eq!(
            100.0 - interior_top,
            metrics.height + 2.0 * pad,
            "equal top and bottom padding below the border"
        );
        // Just inside the top and bottom padding is background; just inside
        // the text line's bounds is the caret.
        snapshot.assert_pixel(caret_x, interior_top + pad - 1.0, theme.surface, "top pad");
        snapshot.assert_pixel(
            caret_x,
            interior_top + pad + 0.5,
            theme.foreground,
            "caret top",
        );
        snapshot.assert_pixel(caret_x, 100.0 - pad - 0.5, theme.foreground, "caret bottom");
        snapshot.assert_pixel(caret_x, 100.0 - pad + 0.5, theme.surface, "bottom pad");

        let prop_line = StatusLine::new(plain("HHHH"));
        let mut prop_above = Rect::default();
        if let Some(prop_snap) = offscreen::render(400, 100, |frame| {
            frame.clear(Rgba::from(theme.background));
            frame.with_font(&crate::Font::new(13.5), |frame| {
                prop_above = bar.lend(&prop_line, &notifier).draw(frame, area, &theme);
            });
        }) {
            let mut min_y = 100u32;
            let mut max_y = 0u32;
            for y in (prop_above.height as u32 + 1)..100 {
                for x in 0..100 {
                    let p = prop_snap.pixel(x as f32 + 0.5, y as f32 + 0.5);
                    if p != theme.surface {
                        min_y = min_y.min(y);
                        max_y = max_y.max(y);
                    }
                }
            }
            let top_gap = min_y as f32 - (prop_above.height + logical(STROKE, 1.0));
            let bottom_gap = 99.0 - max_y as f32;
            assert_eq!(top_gap, bottom_gap, "min_y={min_y}, max_y={max_y}");
        }
    }
}
