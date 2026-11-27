//! [`TextArea`]: a text input of several lines, for a commit message, a
//! note, or a setting that takes a paragraph. It is the
//! [`TextField`](super::text_field::TextField)'s sibling and answers the
//! same keys, plus the ones a text of several lines needs:
//!
//! - typing, Backspace and Delete, over the selection when there is one;
//!   Enter starts a new line;
//! - Left and Right, Ctrl+Left and Ctrl+Right by words; Up and Down (and
//!   `C-p`, `C-n`) by rows, keeping the column the caret started from;
//!   PageUp and PageDown by the rows in sight; Home and End to the ends of
//!   the row, Ctrl+Home and Ctrl+End to the ends of the text; each
//!   extending the selection with Shift;
//! - the Emacs and readline keys of a text field, `C-a` and `C-e` going to
//!   the ends of the caret's line and `C-k` and `C-u` killing to them;
//! - copy, cut, paste, undo, and redo on the primary modifier (Ctrl, or
//!   Cmd on macOS), as a text field takes them.
//!
//! Tab, Esc, and chords it does not know are [`TextAreaResponse::Ignored`],
//! for the app to move the focus or cancel with. Long lines wrap at the
//! area's width, after a space when the next word fits (see
//! [`TextArea::with_wrap`] to scroll sideways instead), and the area
//! scrolls to keep the caret in sight; the wheel scrolls it too. The
//! pointer places the caret, a drag selects, a double click selects a word
//! and a triple click the line. On X11 and Wayland the selected text goes
//! to the primary selection, and a middle click pastes it where it lands.
//! Text an input method composes shows underlined at the caret. An empty
//! area shows its placeholder.
//!
//! Right-to-left text (Hebrew, Arabic) shows in its visual order, but the
//! caret, the selection, and the arrow keys still follow the text's
//! logical order cell by cell, so editing it is only fit for short runs
//! inside left-to-right text for now.
//!
//! An area lays its text out on the cell grid, every character a cell or
//! two wide, unless built [`TextArea::with_proportional_font`]: then its
//! text, its placeholder, and a composition are set in the proportional
//! font, wrapped where a paragraph wraps them, and the caret, Up and Down,
//! Home and End, the selection, pointer presses, and horizontal scrolling
//! follow the glyphs through the paragraph's layout; rows stay a line
//! tall. The choice is the area's, not the theme's
//! [`ChromeText`](crate::ChromeText).

mod wrap;

use std::borrow::Cow;
use std::ops::Range;
use std::time::Instant;

use accesskit::{Node, Role, TextPosition, TextSelection};

use super::super::app::access::node_id;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::ime::{Preedit, PreeditChange};
use super::super::app::widget::{Outcome, Widget};
use super::super::base::clock;
use super::super::base::cursor::CursorIcon;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::list::scroll_to_show;
use super::super::base::motion::Blink;
use super::super::base::styled::{Span, StyledText};
use super::super::base::text;
use super::super::base::theme::Theme;
use super::super::input::event::{ImeInput, Input, PointerButton, WheelDelta};
use super::super::platform::clipboard::{ClipboardKind, HAS_PRIMARY_SELECTION, SystemClipboard};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::super::render::measurer::ParagraphMeasurer;
use super::super::render::paragraph::{Paragraph, ParagraphFont, ParagraphLayout};
use super::label::{fit_proportional, styled_paragraph};
use super::text_field::line::{Edit, LineEdit};
use super::text_field::{
    BORDER, CARET_WIDTH, Clicks, PADDING_Y, PLACEHOLDER_COLOR, TEXT_COLOR, TRIPLE_CLICK,
    UNWRAPPED_WIDTH, draw_frame, edit_by_key, frame_inset_x, publish_primary, scroll_to_caret,
    scroll_x_to_caret,
};

use wrap::Row;

// ========================================================================
// Constants
// ========================================================================

/// The rows a notch of the wheel scrolls.
const WHEEL_ROWS: f32 = 3.0;

/// What ends a line of the text.
const LINE_FEED: char = '\n';

// ========================================================================
// Data Structures
// ========================================================================

/// A text input of several lines. The app owns it, hands it the keys and
/// pointer events of the window while it has the focus, and draws it each
/// frame.
#[derive(Clone, Debug)]
pub struct TextArea {
    /// The caret's blink.
    blink: Blink,
    /// The last press and how many quick presses on one spot led to it.
    clicks: Option<Clicks>,
    /// Whether a left press in the area is held, selecting as it moves.
    dragging: bool,
    /// The text, caret, and selection.
    edit: LineEdit,
    /// Whether the app gave the area the focus.
    focused: bool,
    /// Whether the next frame scrolls to bring the caret into sight: after
    /// the caret moved, not after the wheel scrolled.
    follow_caret: bool,
    /// The proportional font of the window last drawn in, to lay the text
    /// out between frames; `None` before the first frame.
    font: Option<ParagraphFont>,
    /// Whether the area draws its own background and border.
    framed: bool,
    /// The column Up and Down keep to, from the first of a run of them: in
    /// cells, or in pixels in the proportional font.
    goal: Option<f32>,
    /// Whether the pointer is over the area, as last told the window.
    hovered: bool,
    /// The name screen readers give it, if any.
    label: Option<String>,
    /// Where the area was drawn last, or `None` before the first frame.
    layout: Option<AreaLayout>,
    /// Whether long lines scroll sideways instead of wrapping.
    no_wrap: bool,
    /// What an empty area shows.
    placeholder: String,
    /// Whether the text is laid out in the proportional font rather than
    /// on the cell grid.
    proportional: bool,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// Text an input method is composing at the caret.
    preedit: Option<Preedit>,
    /// The selection last written to the primary selection, until the
    /// selection is dropped.
    primary_range: Option<Range<usize>>,
    /// The first cell in sight of each row, without wrapping.
    scroll_col: usize,
    /// The pixels of touchpad scrolling not yet a whole row.
    scroll_rest: f32,
    /// The first row in sight.
    scroll_row: usize,
    /// The pixels of each row out of sight to the left, without wrapping,
    /// in the proportional font.
    scroll_x: f32,
    /// The text laid out in the proportional font at the last frame.
    shaped: Option<Shaped>,
    /// The version of the app's text last synced, or `None` before the
    /// first [`Control::sync`].
    version: Option<u64>,
}

/// What became of a key or event offered to a [`TextArea`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum TextAreaResponse {
    /// The text changed: read it with [`TextArea::text`].
    Changed,
    /// The area used it without changing the text (the caret or the
    /// selection moved, it scrolled, or something was copied).
    Handled,
    /// Not the area's: the app may act on it.
    Ignored,
}

/// What the app's state says a [`TextArea`] shows, for
/// [`Control::sync`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextAreaProps<'a> {
    /// The name screen readers give it, if any.
    pub label: Option<&'a str>,
    /// What it shows while empty.
    pub placeholder: &'a str,
    /// The text, its lines ended by line feeds.
    pub text: &'a str,
    /// A number the app changes whenever it changes the text, its own
    /// edits and the ones it stores from the area alike: the area compares
    /// the text only when the version changed, so a long text is not
    /// compared every frame.
    pub version: u64,
}

/// An area's text laid out in the proportional font.
#[derive(Clone, Debug)]
struct Shaped {
    /// Where its characters landed, from a top left at 0, 0.
    layout: ParagraphLayout,
    /// The text laid out.
    text: String,
    /// The width its lines wrap at.
    width: f32,
}

/// The rows an area's text shows in, and what places a column in them.
struct Lines<'a> {
    /// The text laid out in the proportional font, or `None` on cells.
    layout: Option<Cow<'a, ParagraphLayout>>,
    /// The height of a row.
    line: f32,
    /// The rows, top to bottom.
    rows: Vec<Row>,
    /// The text.
    text: &'a str,
}

/// Where an area's text was drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AreaLayout {
    /// The whole area, frame included.
    area: Rect,
    /// The caret's cell.
    caret: Rect,
    /// The cells across the text's room.
    cols: usize,
    /// The window's cell size.
    metrics: CellMetrics,
    /// The rows in sight.
    rows: usize,
    /// The window's scale factor.
    scale_factor: f64,
    /// The text's room.
    text: Rect,
}

// ========================================================================
// TextArea Implementations
// ========================================================================

impl TextArea {
    // --------------------------------------------------------------------
    // Building
    // --------------------------------------------------------------------

    /// An empty, unframed, wrapping area without a placeholder.
    pub fn new() -> Self {
        Self::with_text("")
    }

    /// An area holding `text` (control characters but line feeds left
    /// out), the caret at its end.
    pub fn with_text(text: &str) -> Self {
        Self {
            blink: Blink::default(),
            clicks: None,
            dragging: false,
            edit: LineEdit::multiline(text),
            focused: false,
            follow_caret: true,
            font: None,
            framed: false,
            goal: None,
            hovered: false,
            label: None,
            layout: None,
            no_wrap: false,
            placeholder: String::new(),
            pointer: None,
            proportional: false,
            preedit: None,
            primary_range: None,
            scroll_col: 0,
            scroll_rest: 0.0,
            scroll_row: 0,
            scroll_x: 0.0,
            shaped: None,
            version: None,
        }
    }

    /// This area named `label` for screen readers, as the label beside it
    /// says what it holds.
    pub fn with_label(self, label: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            ..self
        }
    }

    /// This area showing `placeholder` while it is empty.
    pub fn with_placeholder(self, placeholder: impl Into<String>) -> Self {
        Self {
            placeholder: placeholder.into(),
            ..self
        }
    }

    /// This area drawing its own background and a border around the text
    /// (accent while focused), as a form input does.
    pub fn with_frame(self) -> Self {
        Self {
            framed: true,
            ..self
        }
    }

    /// This area wrapping long lines at its width (`true`, the default),
    /// or scrolling sideways to keep the caret in sight (`false`), as a
    /// code snippet wants.
    pub fn with_wrap(self, wrap: bool) -> Self {
        Self {
            no_wrap: !wrap,
            ..self
        }
    }

    /// This area laying its text, placeholder, and composition out in the
    /// proportional font, as prose wants, rather than on the cell grid:
    /// lines wrap where a paragraph of that width wraps them, and the
    /// caret, vertical moves, the selection, and pointer presses follow
    /// the glyphs. Rows stay a line tall.
    pub fn with_proportional_font(self) -> Self {
        Self {
            proportional: true,
            ..self
        }
    }

    // --------------------------------------------------------------------
    // Text
    // --------------------------------------------------------------------

    /// The text, its lines ended by line feeds.
    pub fn text(&self) -> &str {
        self.edit.text()
    }

    /// How many lines the text has, counting those its line feeds end and
    /// the one after the last; wrapping adds none.
    pub fn line_count(&self) -> usize {
        self.edit.text().split(LINE_FEED).count()
    }

    /// Replaces the text with `text` (control characters but line feeds
    /// left out), the caret at its end, nothing selected, undo forgotten.
    pub fn set_text(&mut self, text: &str) {
        self.edit.set_text(text);
        self.preedit = None;
        self.follow_caret = true;
    }

    /// The selected part of the text as a byte range, or `None` when
    /// nothing is selected.
    pub fn selection(&self) -> Option<Range<usize>> {
        self.edit.selection()
    }

    /// Selects the whole text.
    pub fn select_all(&mut self) {
        self.edit.select_all();
    }

    /// Selects the bytes `range` of the text, the caret at its end.
    /// Ignored when the range is out of the text or off a char boundary.
    pub fn select(&mut self, range: Range<usize>) {
        let text = self.edit.text();
        if range.start > range.end
            || !text.is_char_boundary(range.start)
            || !text.is_char_boundary(range.end)
        {
            return;
        }
        self.edit.move_to(range.start, false);
        self.edit.move_to(range.end, true);
        self.follow_caret = true;
    }

    // --------------------------------------------------------------------
    // Focus
    // --------------------------------------------------------------------

    /// Makes the caret blink (`true`, the default) or show steady, as the
    /// user's system setting for caret blinking says.
    pub fn set_caret_blink(&mut self, blink: bool) {
        self.blink.set_enabled(blink);
    }

    /// The caret's cell in the last frame, in physical pixels, for
    /// [`ImeFocus::follow`](crate::ImeFocus::follow) while the area is
    /// focused; `None` before the first frame.
    pub fn cursor_area(&self) -> Option<Rect> {
        self.layout.map(|layout| layout.caret)
    }

    // --------------------------------------------------------------------
    // Rows
    // --------------------------------------------------------------------

    /// The rows the text shows in at the width last drawn (only its line
    /// feeds break it before the first frame or without wrapping).
    fn rows(&self) -> Vec<Row> {
        self.lines().rows
    }

    /// The rows the text shows in, as [`TextArea::rows`] says, with what
    /// places a column in them: in the proportional font once a frame
    /// laid it out, else on cells.
    fn lines(&self) -> Lines<'_> {
        let text = self.edit.text();
        let line = self.layout.map_or(0.0, |layout| layout.metrics.height);
        if let Some(layout) = self.proportional_layout() {
            return Lines {
                rows: wrap::paragraph_rows(layout.lines().map(|line| line.text), text),
                layout: Some(layout),
                line,
                text,
            };
        }
        let cols = match self.no_wrap {
            true => None,
            false => self.layout.map(|layout| layout.cols.max(1)),
        };
        Lines {
            layout: None,
            line,
            rows: wrap::rows(text, cols),
            text,
        }
    }

    /// The text laid out in the proportional font at the width last
    /// drawn: the last frame's layout while the text is the same, else a
    /// new one in the window's font. `None` on cells or before the first
    /// frame.
    fn proportional_layout(&self) -> Option<Cow<'_, ParagraphLayout>> {
        let shaped = self.shaped.as_ref().filter(|_| self.proportional)?;
        let text = self.edit.text();
        if shaped.text == text {
            return Some(Cow::Borrowed(&shaped.layout));
        }
        let measurer = ParagraphMeasurer::new(self.font?);
        Some(Cow::Owned(measurer.layout(
            &text_paragraph(text),
            Rect::new(0.0, 0.0, shaped.width, 0.0),
        )))
    }

    /// Moves the caret `delta` rows down (up when negative), to the cell
    /// column the run of vertical moves started from, extending the
    /// selection when `extend`. Past the first or last row it goes to the
    /// text's start or end.
    fn move_rows(&mut self, delta: isize, extend: bool) -> Edit {
        let (goal, offset) = {
            let lines = self.lines();
            let caret = self.edit.caret();
            let row = wrap::row_of(&lines.rows, caret);
            let goal = self.goal.unwrap_or_else(|| lines.x_at(row, caret));
            let target = row as isize + delta;
            let offset = if target < 0 {
                0
            } else if target >= lines.rows.len() as isize {
                lines.text.len()
            } else {
                lines.offset_at(target as usize, goal)
            };
            (goal, offset)
        };
        self.goal = Some(goal);
        self.edit.move_to(offset, extend)
    }

    /// Moves the caret to the start of its row, or its end when `end`,
    /// extending the selection when `extend`.
    fn move_to_row_edge(&mut self, end: bool, extend: bool) -> Edit {
        let rows = self.rows();
        let text = self.edit.text();
        let row = &rows[wrap::row_of(&rows, self.edit.caret())];
        let offset = match end {
            true => row.last_offset(text),
            false => row.range.start,
        };
        self.edit.move_to(offset, extend)
    }

    /// The rows a page key moves: those in sight, at least one.
    fn page_rows(&self) -> isize {
        self.layout.map_or(1, |layout| layout.rows.max(1)) as isize
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// Reacts to `key`, copying to and pasting from the context's
    /// clipboard and writing a new selection to its primary selection.
    /// Hand it the keys of the window while the area is focused.
    pub(crate) fn handle_key(&mut self, key: Key, cx: &mut EventCx<'_>) -> TextAreaResponse {
        let emacs = cx.key_scheme().emacs;
        let clipboard = cx.clipboard();
        let response = self.edit_by_key(key, emacs, clipboard);
        if !matches!(response, TextAreaResponse::Ignored) {
            self.follow_caret = true;
        }
        // Any other key starts a new column for the next vertical move.
        if !is_vertical(key) {
            self.goal = None;
        }
        publish_primary(&self.edit, &mut self.primary_range, clipboard);
        response
    }

    /// Carries out `key` as [`TextArea::handle_key`] does, but for the
    /// primary selection, taking the Emacs keys where `emacs`.
    fn edit_by_key(
        &mut self,
        key: Key,
        emacs: bool,
        clipboard: &mut SystemClipboard,
    ) -> TextAreaResponse {
        let mods = key.mods;
        let plain = mods == Mods::NONE;
        let shift = mods == Mods::SHIFT;
        let to_text_edge = mods == Mods::CTRL || mods == Mods::CTRL | Mods::SHIFT;
        let emacs_ctrl = emacs && mods == Mods::CTRL;
        let extend = mods.contains(Mods::SHIFT);
        let caret = self.edit.caret();
        let page = self.page_rows();
        let edit = match key.code {
            KeyCode::Enter if plain => Some(self.edit.insert_newline()),
            KeyCode::Up if plain || shift => Some(self.move_rows(-1, extend)),
            KeyCode::Down if plain || shift => Some(self.move_rows(1, extend)),
            KeyCode::Char('p') if emacs_ctrl => Some(self.move_rows(-1, false)),
            KeyCode::Char('n') if emacs_ctrl => Some(self.move_rows(1, false)),
            KeyCode::PageUp if plain || shift => Some(self.move_rows(-page, extend)),
            KeyCode::PageDown if plain || shift => Some(self.move_rows(page, extend)),
            KeyCode::Home if plain || shift => Some(self.move_to_row_edge(false, extend)),
            KeyCode::End if plain || shift => Some(self.move_to_row_edge(true, extend)),
            KeyCode::Home if to_text_edge => Some(self.edit.move_to(0, extend)),
            KeyCode::End if to_text_edge => Some(self.edit.move_to(self.edit.text().len(), extend)),
            KeyCode::Char('a') if emacs_ctrl => {
                Some(self.edit.move_to(self.edit.line_start(caret), false))
            }
            KeyCode::Char('e') if emacs_ctrl => {
                Some(self.edit.move_to(self.edit.line_end(caret), false))
            }
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::End
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::Up => None,
        };
        edit.or_else(|| edit_by_key(&mut self.edit, key, emacs, clipboard))
            .map_or(TextAreaResponse::Ignored, response_of)
    }

    /// Reacts to an input method event: text being composed is shown
    /// underlined at the caret, and committed text is typed over the
    /// selection. [`TextAreaResponse::Ignored`] when nothing changed.
    pub(crate) fn handle_ime(&mut self, ime: &ImeInput) -> TextAreaResponse {
        let response = match Preedit::apply(&mut self.preedit, ime) {
            PreeditChange::Changed => TextAreaResponse::Handled,
            PreeditChange::Commit(text) => response_of(self.edit.insert(&text)),
            PreeditChange::Unchanged => TextAreaResponse::Ignored,
        };
        if !matches!(response, TextAreaResponse::Ignored) {
            self.follow_caret = true;
        }
        response
    }

    /// Reacts to pointer `input`: a left press in the area places the
    /// caret (extending the selection with Shift), two quick presses select
    /// a word and three the line, and moving with the button held selects;
    /// the wheel scrolls; the pointer is a text beam over the area. On X11
    /// and Wayland a selection made is written to the primary selection
    /// once the button is let go, and a middle press pastes the primary
    /// selection where it lands. A press outside the area is
    /// [`TextAreaResponse::Ignored`], for the app to move the focus.
    pub(crate) fn handle_pointer(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> TextAreaResponse {
        let response = match input {
            Input::PointerButton(button) => {
                self.pointer = Some(button.position);
                match (button.button, button.pressed) {
                    (PointerButton::Left, true) => match self.pointer_point() {
                        Some(point) => {
                            let shift = button.mods.contains(Mods::SHIFT);
                            self.press_at(point, clock::now(), shift)
                        }
                        None => TextAreaResponse::Ignored,
                    },
                    (PointerButton::Left, false) if self.dragging => {
                        self.dragging = false;
                        TextAreaResponse::Handled
                    }
                    (PointerButton::Middle, true) if HAS_PRIMARY_SELECTION => {
                        match self.pointer_point() {
                            Some(point) => self.paste_primary_at(point, cx.clipboard()),
                            None => TextAreaResponse::Ignored,
                        }
                    }
                    (
                        PointerButton::Back
                        | PointerButton::Forward
                        | PointerButton::Left
                        | PointerButton::Middle
                        | PointerButton::Other(_)
                        | PointerButton::Right,
                        _,
                    ) => TextAreaResponse::Ignored,
                }
            }
            Input::PointerLeft => {
                self.hover(cx, None);
                TextAreaResponse::Ignored
            }
            Input::PointerMoved(point) => {
                self.hover(cx, Some(*point));
                self.drag_to(*point)
            }
            Input::Wheel(wheel) => {
                self.pointer = Some(wheel.position);
                match self.pointer_point() {
                    Some(_) => self.scroll_by(wheel.delta),
                    None => TextAreaResponse::Ignored,
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
            | Input::Key(_) => TextAreaResponse::Ignored,
        };
        if !self.dragging {
            publish_primary(&self.edit, &mut self.primary_range, cx.clipboard());
        }
        if !matches!(response, TextAreaResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Scrolls by the wheel's `delta`: [`WHEEL_ROWS`] rows a notch, or a
    /// row per row height of pixels, leaving the caret where it is.
    fn scroll_by(&mut self, delta: WheelDelta) -> TextAreaResponse {
        let Some(layout) = self.layout else {
            return TextAreaResponse::Ignored;
        };
        let rows = match delta {
            WheelDelta::Lines(lines) => (-lines.y * WHEEL_ROWS).round(),
            WheelDelta::Pixels(pixels) => {
                self.scroll_rest -= pixels.y;
                let row_height = layout.metrics.height.max(1.0);
                let whole = (self.scroll_rest / row_height).trunc();
                self.scroll_rest -= whole * row_height;
                whole
            }
        };
        let last = self.rows().len().saturating_sub(layout.rows);
        let scrolled = (self.scroll_row as f32 + rows).clamp(0.0, last as f32) as usize;
        if scrolled == self.scroll_row {
            return TextAreaResponse::Handled;
        }
        self.scroll_row = scrolled;
        self.follow_caret = false;
        TextAreaResponse::Handled
    }

    /// A middle press at `point`: types `clipboard`'s primary selection at
    /// the character boundary nearest `point`, leaving any selection.
    fn paste_primary_at(
        &mut self,
        point: Point,
        clipboard: &mut SystemClipboard,
    ) -> TextAreaResponse {
        let Some(text) = clipboard.read_text(ClipboardKind::Primary) else {
            return TextAreaResponse::Handled;
        };
        self.edit.move_to(self.offset_at(point), false);
        self.follow_caret = true;
        response_of(self.edit.insert(&text))
    }

    /// Follows the pointer to `pointer` (`None` outside the window),
    /// showing a text beam over the area.
    fn hover(&mut self, cx: &mut EventCx<'_>, pointer: Option<Point>) {
        self.pointer = pointer;
        let over = self.pointer_point().is_some();
        if over != self.hovered {
            self.hovered = over;
            let icon = match over {
                true => CursorIcon::Text,
                false => CursorIcon::Default,
            };
            cx.set_cursor(icon);
        }
    }

    /// Where the pointer is over the area as last drawn, or `None` when it
    /// is not.
    fn pointer_point(&self) -> Option<Point> {
        let area = self.layout?.area;
        self.pointer.filter(|&point| area.contains(point))
    }

    /// A left press at `point` at `now`, with Shift held when `shift`.
    fn press_at(&mut self, point: Point, now: Instant, shift: bool) -> TextAreaResponse {
        let Some(layout) = self.layout else {
            return TextAreaResponse::Ignored;
        };
        let clicks = Clicks::after(self.clicks, now, point, layout.scale_factor);
        self.clicks = Some(clicks);
        self.goal = None;
        self.follow_caret = true;
        let offset = self.offset_at(point);
        match clicks.count {
            1 => {
                self.edit.move_to(offset, shift);
                self.dragging = true;
            }
            TRIPLE_CLICK => {
                let start = self.edit.line_start(offset);
                let end = self.edit.line_end(offset);
                self.edit.move_to(start, false);
                self.edit.move_to(end, true);
            }
            _ => {
                self.edit.select_word_at(offset);
            }
        }
        TextAreaResponse::Handled
    }

    /// Extends the selection to `point` while a press is held.
    fn drag_to(&mut self, point: Point) -> TextAreaResponse {
        if !self.dragging {
            return TextAreaResponse::Ignored;
        }
        let offset = self.offset_at(point);
        if offset == self.edit.caret() {
            return TextAreaResponse::Ignored;
        }
        self.edit.move_to(offset, true);
        self.follow_caret = true;
        TextAreaResponse::Handled
    }

    /// The char boundary of the text nearest `point`, as last drawn: in the
    /// row under it (the first or last row above or below the text).
    fn offset_at(&self, point: Point) -> usize {
        let Some(layout) = self.layout else {
            return self.edit.caret();
        };
        let lines = self.lines();
        let row = ((point.y - layout.text.y) / layout.metrics.height.max(1.0)).floor();
        let last = (lines.rows.len() - 1) as f32;
        let index = (self.scroll_row as f32 + row).clamp(0.0, last) as usize;
        let x = match lines.layout {
            Some(_) => point.x - layout.text.x + self.scroll_x,
            None => {
                (point.x - layout.text.x) / layout.metrics.width.max(1.0) + self.scroll_col as f32
            }
        };
        lines.offset_at(index, x)
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Draws the rows in sight on the cell grid in `room`, `visible` rows
    /// tall, scrolled to keep the caret in sight when it last moved, with
    /// the selection and a composition. Returns the caret's cell.
    fn draw_cells(&mut self, frame: &mut Frame, room: Rect, visible: usize, theme: &Theme) -> Rect {
        let metrics = frame.cell_metrics();
        let cols = metrics.cols_in(room.width);
        let rows = self.rows();
        let text = self.edit.text();
        let caret = self.edit.caret();
        let caret_row = wrap::row_of(&rows, caret);
        let preedit_cells = self
            .preedit
            .as_ref()
            .map_or(0, |preedit| text::width(preedit.text()));
        let shown_caret = rows[caret_row].cells_to(text, caret)
            + self
                .preedit
                .as_ref()
                .map_or(Some(0), Preedit::caret_cells)
                .unwrap_or(preedit_cells);
        self.scroll_row = self.scrolled_row(caret_row, rows.len(), visible);
        self.scroll_col = match (self.no_wrap, self.follow_caret) {
            (false, _) => 0,
            (true, true) => {
                let row = &rows[caret_row];
                let total = row.cells_to(text, row.range.end) + preedit_cells;
                scroll_to_caret(self.scroll_col, shown_caret, total, cols)
            }
            (true, false) => self.scroll_col,
        };
        self.follow_caret = false;
        let left = room.x - self.scroll_col as f32 * metrics.width;
        let row_y = |row: usize| room.y + (row as f32 - self.scroll_row as f32) * metrics.height;
        let caret_area = Rect::new(
            left + shown_caret as f32 * metrics.width,
            row_y(caret_row),
            metrics.width,
            metrics.height,
        );
        frame.clipped(room, |frame| {
            if text.is_empty() && self.preedit.is_none() {
                let placeholder =
                    StyledText::plain(self.placeholder.as_str(), PLACEHOLDER_COLOR).with_ellipsis();
                frame.draw_text(&placeholder, Point::new(room.x, room.y), room, theme);
                return;
            }
            let shown = self.scroll_row..(self.scroll_row + visible).min(rows.len());
            for (index, row) in rows[shown.clone()].iter().enumerate() {
                let y = row_y(shown.start + index);
                // A preedit moves the text after the caret: the selection
                // is left undrawn while one shows, since typing replaces it.
                if self.preedit.is_none() {
                    self.draw_row_selection(frame, row, left, y, theme);
                }
                let line = match (&self.preedit, shown.start + index == caret_row) {
                    (Some(preedit), true) => {
                        let mut line = StyledText::plain(&text[row.range.start..caret], TEXT_COLOR);
                        line.push(Span::new(preedit.text(), TEXT_COLOR).underlined());
                        line.push(Span::new(&text[caret..row.range.end], TEXT_COLOR));
                        line
                    }
                    (Some(_) | None, _) => StyledText::plain(&text[row.range.clone()], TEXT_COLOR),
                };
                frame.draw_text(&line, Point::new(left, y), room, theme);
            }
        });
        caret_area
    }

    /// Draws the text in the proportional font in `room`, `visible` rows
    /// tall, scrolled to keep the caret in sight when it last moved, with
    /// the selection and a composition, or the placeholder while it is
    /// empty. Returns the caret's area, a cell wide.
    fn draw_proportional(
        &mut self,
        frame: &mut Frame,
        room: Rect,
        visible: usize,
        theme: &Theme,
    ) -> Rect {
        let metrics = frame.cell_metrics();
        let width = match self.no_wrap {
            true => UNWRAPPED_WIDTH,
            false => room.width.max(1.0),
        };
        let text = self.edit.text();
        self.font = Some(frame.paragraph_font());
        let fresh = self
            .shaped
            .as_ref()
            .is_some_and(|shaped| shaped.width == width && shaped.text == text);
        if !fresh {
            let layout =
                frame.paragraph_layout(&text_paragraph(text), Rect::new(0.0, 0.0, width, 0.0));
            self.shaped = Some(Shaped {
                layout,
                text: text.to_owned(),
                width,
            });
        }
        if text.is_empty() && self.preedit.is_none() {
            (self.scroll_row, self.scroll_x, self.follow_caret) = (0, 0.0, false);
            let placeholder =
                StyledText::plain(self.placeholder.as_str(), PLACEHOLDER_COLOR).with_ellipsis();
            let placeholder = styled_paragraph(&fit_proportional(frame, &placeholder, room.width));
            frame.clipped(room, |frame| {
                frame.draw_paragraph(
                    &placeholder,
                    Rect::new(room.x, room.y, width, room.height),
                    theme,
                );
            });
            return Rect::new(room.x, room.y, metrics.width, metrics.height);
        }
        let plain = self
            .shaped
            .as_ref()
            .map(|shaped| shaped.layout.clone())
            .unwrap_or_default();
        let rows = wrap::paragraph_rows(plain.lines().map(|line| line.text), text);
        let caret = self.edit.caret();
        let caret_row = wrap::row_of(&rows, caret);
        // A composition shows in the text at the caret, moving what follows.
        let (paragraph, shown_caret, shown_len) = match &self.preedit {
            Some(preedit) => (
                Paragraph::plain(&text[..caret], TEXT_COLOR)
                    .with(Span::new(preedit.text(), TEXT_COLOR).underlined())
                    .with(Span::new(&text[caret..], TEXT_COLOR)),
                caret + preedit.caret().unwrap_or(preedit.text().len()),
                text.len() + preedit.text().len(),
            ),
            None => (text_paragraph(text), caret, text.len()),
        };
        let shown = match &self.preedit {
            Some(_) => frame.paragraph_layout(&paragraph, Rect::new(0.0, 0.0, width, 0.0)),
            None => plain.clone(),
        };
        let caret_at = shown.caret_rect(shown_caret);
        self.scroll_row = self.scrolled_row(caret_row, rows.len(), visible);
        self.scroll_x = match (self.no_wrap, self.follow_caret) {
            (false, _) => 0.0,
            (true, true) => {
                let total = shown
                    .range_rects(0..shown_len)
                    .iter()
                    .map(|rect| rect.x + rect.width)
                    .fold(0.0, f32::max);
                let caret_width = logical(CARET_WIDTH, frame.scale_factor());
                scroll_x_to_caret(self.scroll_x, caret_at.x, caret_width, total, room.width)
            }
            (true, false) => self.scroll_x,
        };
        self.follow_caret = false;
        let left = room.x - self.scroll_x;
        let top = room.y - self.scroll_row as f32 * metrics.height;
        let shift = |rect: Rect| Rect::new(left + rect.x, top + rect.y, rect.width, rect.height);
        frame.clipped(room, |frame| {
            // A preedit moves the text after the caret: the selection is
            // left undrawn while one shows, since typing replaces it.
            if let (Some(selection), None) = (self.edit.selection(), &self.preedit) {
                for rect in plain.range_rects(selection.clone()) {
                    frame.fill_rect(shift(rect), theme.selection);
                }
                // A line feed selected shows as a cell after its line.
                for row in rows.iter().filter(|row| {
                    !row.wrapped
                        && selection.start <= row.range.end
                        && selection.end > row.range.end
                }) {
                    let end = plain.caret_rect(row.range.end);
                    let cell = Rect::new(end.x, end.y, metrics.width, end.height);
                    frame.fill_rect(shift(cell), theme.selection);
                }
            }
            let height = rows.len() as f32 * metrics.height;
            frame.draw_paragraph(&paragraph, Rect::new(left, top, width, height), theme);
        });
        shift(Rect::new(
            caret_at.x,
            caret_at.y,
            metrics.width,
            metrics.height,
        ))
    }

    /// How many rows the text shows in when laid out in the proportional
    /// font at `width` (unbounded for `None`), in the font of the window
    /// last drawn in; `None` on cells or before the first frame.
    fn proportional_rows(&self, width: Option<f32>) -> Option<usize> {
        let font = self.font.filter(|_| self.proportional)?;
        let text = self.edit.text();
        let width = width.unwrap_or(UNWRAPPED_WIDTH).max(1.0);
        let shaped = self
            .shaped
            .as_ref()
            .filter(|shaped| shaped.width == width && shaped.text == text);
        let layout = match shaped {
            Some(shaped) => Cow::Borrowed(&shaped.layout),
            None => Cow::Owned(
                ParagraphMeasurer::new(font)
                    .layout(&text_paragraph(text), Rect::new(0.0, 0.0, width, 0.0)),
            ),
        };
        Some(wrap::paragraph_rows(layout.lines().map(|line| line.text), text).len())
    }

    /// Describes the area as last drawn to screen readers as `id`: a
    /// multiline text input named by its label, with its text as value and
    /// one text run (`id`'s first child), its placeholder, and the
    /// selection or caret as its text selection. Nothing before the first
    /// frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = self.layout else {
            return;
        };
        let text = self.edit.text();
        let run_id = id.child(0);
        let position = |offset: usize| TextPosition {
            character_index: text[..offset].chars().count(),
            node: node_id(run_id),
        };
        let mut input = Node::new(Role::MultilineTextInput);
        input.set_value(text);
        if let Some(label) = &self.label {
            input.set_label(label.as_str());
        }
        if !self.placeholder.is_empty() {
            input.set_placeholder(self.placeholder.as_str());
        }
        let caret = self.edit.caret();
        input.set_text_selection(TextSelection {
            anchor: position(self.edit.anchor().unwrap_or(caret)),
            focus: position(caret),
        });
        frame.enter_access(id, layout.area, input);
        let mut run = Node::new(Role::TextRun);
        run.set_value(text);
        run.set_character_lengths(text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<_>>());
        frame.access(run_id, layout.text, run);
        frame.leave_access();
    }

    /// The first row in sight once scrolled from where it was only as far
    /// as keeps row `caret_row` in sight (when following the caret) and the
    /// last of `rows` rows no higher than the bottom of `visible` rows.
    fn scrolled_row(&self, caret_row: usize, rows: usize, visible: usize) -> usize {
        let mut first = self.scroll_row.min(rows.saturating_sub(visible));
        if self.follow_caret {
            first = scroll_to_show(first, caret_row, visible);
        }
        first
    }

    /// Draws the selected part of `row` (its line feed as one more cell
    /// when the selection runs past it) on the line at `y`, its first cell
    /// at `left`.
    fn draw_row_selection(&self, frame: &mut Frame, row: &Row, left: f32, y: f32, theme: &Theme) {
        let Some(selection) = self.edit.selection() else {
            return;
        };
        let start = selection.start.max(row.range.start);
        let end = selection.end.min(row.range.end);
        if start > end || (start == end && selection.end <= row.range.end) {
            return;
        }
        let metrics = frame.cell_metrics();
        let text = self.edit.text();
        let line_feed = usize::from(selection.end > row.range.end && !row.wrapped);
        let from = row.cells_to(text, start);
        let cells = row.cells_to(text, end) - from + line_feed;
        let rect = Rect::new(
            left + from as f32 * metrics.width,
            y,
            cells as f32 * metrics.width,
            metrics.height,
        );
        frame.fill_rect(rect, theme.selection);
    }
}

// ========================================================================
// Lines Implementations
// ========================================================================

impl Lines<'_> {
    /// Where the caret before byte offset `offset` of row `row` is across
    /// the text: in cells from the row's start, or in pixels in the
    /// proportional font.
    fn x_at(&self, row: usize, offset: usize) -> f32 {
        match &self.layout {
            Some(layout) => layout.caret_rect(offset).x,
            None => self.rows[row].cells_to(self.text, offset) as f32,
        }
    }

    /// The char boundary in row `row` nearest `x` across the text, in the
    /// unit of [`Lines::x_at`], no further than the row's last offset.
    fn offset_at(&self, row: usize, x: f32) -> usize {
        let shown = &self.rows[row];
        match &self.layout {
            Some(layout) => {
                let middle = (row as f32 + 0.5) * self.line;
                let offset = layout.offset_at(Point::new(x, middle));
                offset.clamp(shown.range.start, shown.last_offset(self.text))
            }
            None => shown.offset_at_cells(self.text, x),
        }
    }
}

impl Default for TextArea {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for TextArea {
    type Output = ();
    type Response = TextAreaResponse;

    /// Reacts to `input`: keys edit the text, as the module documentation
    /// lists, copying to and pasting from the context's clipboard; input
    /// method text is composed at the caret and typed once committed; the
    /// pointer places the caret, selects, and pastes the primary selection
    /// on a middle click; the wheel scrolls. Hand it keys and input method
    /// text only while the area has the keyboard. Asks for a redraw
    /// whenever it used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TextAreaResponse {
        if matches!(
            input,
            Input::Ime(_) | Input::Key(_) | Input::PointerButton(_)
        ) {
            self.blink.reset(clock::now());
        }
        let response = match input {
            Input::Ime(ime) => self.handle_ime(ime),
            Input::Key(key) => self.handle_key(*key, cx),
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => return self.handle_pointer(input, cx),
        };
        if !matches!(response, TextAreaResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Draws the area in `area` in `theme`'s colors: its rows from the top,
    /// cut at its edges, scrolled to keep the caret in sight when the caret
    /// last moved.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let room = match self.framed {
            true => {
                let inset = logical(BORDER, scale_factor) + logical(PADDING_Y, scale_factor);
                let sides = draw_frame(frame, area, self.focused, theme);
                Rect::new(
                    sides.x,
                    sides.y + inset,
                    sides.width,
                    (sides.height - 2.0 * inset).max(0.0),
                )
            }
            false => area,
        };
        let cols = metrics.cols_in(room.width);
        let visible = metrics.rows_in(room.height).max(1);
        // The width wrapping follows, before the rows are measured.
        self.layout = Some(AreaLayout {
            area,
            caret: room,
            cols,
            metrics,
            rows: visible,
            scale_factor,
            text: room,
        });
        let caret_area = match self.proportional {
            true => self.draw_proportional(frame, room, visible, theme),
            false => self.draw_cells(frame, room, visible, theme),
        };
        if let Some(layout) = &mut self.layout {
            layout.caret = caret_area;
        }
        let caret_shown = self
            .preedit
            .as_ref()
            .is_none_or(|preedit| preedit.caret_cells().is_some());
        let in_sight = caret_area.x < room.x + room.width
            && caret_area.y >= room.y
            && caret_area.y < room.y + room.height;
        self.blink.set_interval(theme.motion.caret_blink);
        if self.focused && caret_shown && self.blink.is_visible(clock::now()) && in_sight {
            let width = logical(CARET_WIDTH, scale_factor);
            frame.fill_rect_over_text(
                Rect::new(caret_area.x, caret_area.y, width, metrics.height),
                theme.cursor,
            );
        }
    }

    /// Tells the area whether it has the keyboard: it draws its caret (and
    /// an accent frame) only then. An area losing the focus drops a
    /// preedit.
    fn set_focused(&mut self, focused: bool) {
        if focused && !self.focused {
            self.blink.reset(clock::now());
        }
        self.focused = focused;
        if !focused {
            self.preedit = None;
            self.dragging = false;
        }
    }

    /// Whether the app said the area has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }

    /// Asks for the redraw that next shows or hides the caret while the
    /// area is focused, returning when it is due; `None` while it shows
    /// steady (unfocused, or idle long enough to rest). Call it from
    /// [`Application::about_to_wait`](crate::Application::about_to_wait):
    /// the caret of a area whose app never does shows steady.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        if !self.focused {
            return None;
        }
        self.blink.drive();
        let next = self.blink.next_frame(clock::now())?;
        cx.request_redraw_at(next);
        Some(next)
    }
}

impl Control for TextArea {
    type Props<'a> = TextAreaProps<'a>;
    type Value = String;

    /// Takes the placeholder and the name from `props`, and the text when
    /// its version changed: a text other than the area's replaces it as
    /// [`TextArea::set_text`] does, the same text keeps the caret, the
    /// selection, the scroll, and any composition.
    fn sync(&mut self, props: TextAreaProps<'_>) {
        if self.version != Some(props.version) {
            self.version = Some(props.version);
            if self.edit.text() != props.text {
                self.set_text(props.text);
            }
        }
        if self.label.as_deref() != props.label {
            self.label = props.label.map(str::to_owned);
        }
        if self.placeholder != props.placeholder {
            props.placeholder.clone_into(&mut self.placeholder);
        }
    }

    /// The height of every row its text shows in, wrapped at `width` (at
    /// its line feeds only for an unbounded width or without wrapping),
    /// plus the frame when it has one: what an area growing with its text
    /// takes. An app holding it to a few rows sizes it itself and lets it
    /// scroll.
    fn measure(&self, width: f32, metrics: CellMetrics, scale_factor: f64) -> f32 {
        let (inset_x, inset_y) = match self.framed {
            true => (
                frame_inset_x(metrics, scale_factor),
                logical(BORDER, scale_factor) + logical(PADDING_Y, scale_factor),
            ),
            false => (0.0, 0.0),
        };
        let wraps = !self.no_wrap && width.is_finite();
        let rows = match self.proportional_rows(wraps.then_some(width - 2.0 * inset_x)) {
            Some(rows) => rows,
            None => {
                let cols = wraps.then(|| metrics.cols_in(width - 2.0 * inset_x).max(1));
                wrap::rows(self.edit.text(), cols).len()
            }
        };
        (rows as f32 * metrics.height + 2.0 * inset_y).ceil()
    }

    /// The text the user changed it to.
    fn changed(&self, response: TextAreaResponse) -> Option<String> {
        match response {
            TextAreaResponse::Changed => Some(self.text().to_owned()),
            TextAreaResponse::Handled | TextAreaResponse::Ignored => None,
        }
    }
}

impl Outcome for TextAreaResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `key` moves the caret by rows, keeping its column: Up, Down,
/// PageUp, and PageDown, with or without Shift, and `C-p` and `C-n`.
fn is_vertical(key: Key) -> bool {
    let plain_or_shift = key.mods == Mods::NONE || key.mods == Mods::SHIFT;
    match key.code {
        KeyCode::Down | KeyCode::PageDown | KeyCode::PageUp | KeyCode::Up => plain_or_shift,
        KeyCode::Char('n' | 'p') => key.mods == Mods::CTRL,
        KeyCode::Backspace
        | KeyCode::Char(_)
        | KeyCode::Delete
        | KeyCode::End
        | KeyCode::Enter
        | KeyCode::Esc
        | KeyCode::F(_)
        | KeyCode::Home
        | KeyCode::Insert
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Tab => false,
    }
}

/// `text` as the paragraph an area in the proportional font shows.
fn text_paragraph(text: &str) -> Paragraph {
    Paragraph::plain(text, TEXT_COLOR)
}

/// The response to an edit of the text.
fn response_of(edit: Edit) -> TextAreaResponse {
    match edit {
        Edit::Changed => TextAreaResponse::Changed,
        Edit::Moved => TextAreaResponse::Handled,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::key::parse_key;
    use crate::input::event::{ButtonInput, WheelInput};
    use crate::testing::{TestWindow, ui_text};

    /// The primary modifier with `c`: Ctrl+`c`, or Cmd+`c` on macOS.
    fn primary(c: char) -> Key {
        Key::new(KeyCode::Char(c), Mods::PRIMARY)
    }

    /// Hands `area` each key of `specs`, returning the last response.
    fn keys(area: &mut TextArea, window: &mut TestWindow, specs: &[&str]) -> TextAreaResponse {
        let mut last = TextAreaResponse::Ignored;
        for spec in specs {
            last = area.handle_key(parse_key(spec).expect("a valid key"), &mut window.cx());
        }
        last
    }

    /// `area` drawn `cols` cells wide and `rows` rows high at the top left
    /// corner of a frame, returning the frame and the cell size.
    fn draw(area: &mut TextArea, cols: f32, rows: f32) -> (Frame, CellMetrics) {
        let mut frame = offscreen::plain_frame(800, 600);
        let metrics = frame.cell_metrics();
        let rect = Rect::new(0.0, 0.0, cols * metrics.width, rows * metrics.height);
        area.draw(&mut frame, rect, &Theme::dark());
        (frame, metrics)
    }

    #[test]
    fn test_up_and_down_keep_the_column_across_a_short_line() {
        let mut window = TestWindow::keyboard_first(800, 600);
        let mut area = TextArea::with_text("first line\nab\nthird line");
        area.select(7..7);
        keys(&mut area, &mut window, &["<down>"]);
        assert_eq!(area.edit.caret(), 13, "the short line's end");
        keys(&mut area, &mut window, &["<down>"]);
        assert_eq!(area.edit.caret(), 21, "back to column 7");
        keys(&mut area, &mut window, &["<left>", "<up>"]);
        assert_eq!(area.edit.caret(), 13, "a sideways move starts a new column");
        keys(&mut area, &mut window, &["C-p"]);
        assert_eq!(area.edit.caret(), 6, "still column 6");
        keys(&mut area, &mut window, &["<up>"]);
        assert_eq!(area.edit.caret(), 0, "past the first row, the start");
    }

    #[test]
    fn test_vertical_moves_follow_the_wrapped_rows() {
        let mut window = TestWindow::keyboard_first(800, 600);
        let mut area = TextArea::with_text("the quick brown fox");
        draw(&mut area, 10.0, 5.0);
        keys(&mut area, &mut window, &["C-<home>", "<right>", "<right>"]);
        keys(&mut area, &mut window, &["<down>"]);
        assert_eq!(area.edit.caret(), 12, "column 2 of the second row");
        keys(&mut area, &mut window, &["<home>"]);
        assert_eq!(area.edit.caret(), 10, "the row's start, not the line's");
        keys(&mut area, &mut window, &["<up>", "<end>"]);
        assert_eq!(area.edit.caret(), 9, "before the space the row hangs");
        keys(&mut area, &mut window, &["C-e"]);
        assert_eq!(area.edit.caret(), 19, "the line's end");
    }

    #[test]
    fn test_enter_starts_a_line_and_tab_and_esc_are_left_to_the_app() {
        let mut window = TestWindow::keyboard_first(800, 600);
        let mut area = TextArea::new();
        keys(&mut area, &mut window, &["a", "b"]);
        assert_eq!(
            keys(&mut area, &mut window, &["<cr>"]),
            TextAreaResponse::Changed
        );
        keys(&mut area, &mut window, &["c"]);
        assert_eq!((area.text(), area.line_count()), ("ab\nc", 2));
        area.handle_key(primary('z'), &mut window.cx());
        assert_eq!(area.text(), "ab\n", "the new line undoes on its own");
        for spec in ["<tab>", "<esc>", "M-x", "<f2>"] {
            assert_eq!(
                keys(&mut area, &mut window, &[spec]),
                TextAreaResponse::Ignored,
                "{spec}"
            );
        }
    }

    #[test]
    fn test_shift_down_selects_across_lines_and_copy_keeps_the_line_feed() {
        let mut window = TestWindow::keyboard_first(800, 600);
        let mut area = TextArea::with_text("alpha\nbeta\ngamma");
        keys(&mut area, &mut window, &["C-<home>", "<right>", "S-<down>"]);
        assert_eq!(area.selection(), Some(1..7));
        area.handle_key(primary('c'), &mut window.cx());
        assert_eq!(
            window
                .clipboard()
                .read_text(ClipboardKind::Clipboard)
                .as_deref(),
            Some("lpha\nb")
        );
        keys(&mut area, &mut window, &["C-S-<end>"]);
        assert_eq!(area.selection(), Some(1..16), "to the text's end");
    }

    #[test]
    fn test_the_area_scrolls_to_the_caret_and_the_wheel_scrolls_without_snapping_back() {
        let mut window = TestWindow::keyboard_first(800, 600);
        let lines: Vec<String> = (0..10).map(|line| format!("line {line}")).collect();
        let mut area = TextArea::with_text(&lines.join("\n"));
        draw(&mut area, 20.0, 3.0);
        assert_eq!(area.scroll_row, 7, "the caret's row at the bottom");
        keys(&mut area, &mut window, &["C-<home>"]);
        draw(&mut area, 20.0, 3.0);
        assert_eq!(area.scroll_row, 0);
        let (frame, metrics) = draw(&mut area, 20.0, 3.0);
        assert_eq!(
            crate::testing::drawn_text(&frame),
            ["line 0", "line 1", "line 2"]
        );
        let over = Point::new(metrics.width, metrics.height);
        let wheel = WheelInput {
            delta: WheelDelta::Lines(Point::new(0.0, -1.0)),
            mods: Mods::NONE,
            position: over,
        };
        window.feed(&mut area, &Input::Wheel(wheel));
        draw(&mut area, 20.0, 3.0);
        assert_eq!(area.scroll_row, 3, "three rows a notch, caret out of sight");
        let press = ButtonInput::press(PointerButton::Left, over);
        window.feed(&mut area, &Input::PointerButton(press));
        assert_eq!(area.edit.caret(), "line 0\nline 1\nline 2\nline 3\nl".len());
    }

    #[test]
    fn test_the_area_describes_itself_as_a_multiline_input() {
        let mut area = TextArea::with_text("one\ntwo")
            .with_label("Notes")
            .with_placeholder("Write here");
        area.select(0..3);
        let (mut frame, _) = draw(&mut area, 20.0, 3.0);
        area.access(&mut frame, FocusId::new("notes"));
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\" focused\n\
             \x20 MultilineTextInput \"Notes\" value=\"one\\ntwo\"\n\
             \x20   TextRun value=\"one\\ntwo\"\n"
        );
    }

    #[test]
    fn test_a_focused_framed_area_draws_a_selection_over_two_rows_and_the_caret() {
        let theme = Theme::dark();
        let mut area = TextArea::with_text("hello\nworld").with_frame();
        area.set_focused(true);
        area.select(3..8);
        let Some(snapshot) = offscreen::render(300, 120, |frame| {
            frame.clear(Rgba::from(theme.surface));
            area.draw(frame, Rect::new(0.0, 0.0, 300.0, 120.0), &theme);
        }) else {
            return;
        };
        let layout = area.layout.expect("drawn");
        let (text, metrics) = (layout.text, layout.metrics);
        snapshot.assert_pixel(0.0, 60.0, theme.accent, "the focused border");
        let first = Rect::new(text.x + 3.5 * metrics.width, text.y + 2.0, 1.0, 4.0);
        assert!(snapshot.contains_color(first, theme.selection), "row one");
        let second = Rect::new(
            text.x + 0.5 * metrics.width,
            text.y + metrics.height + 2.0,
            1.0,
            4.0,
        );
        assert!(snapshot.contains_color(second, theme.selection), "row two");
        let caret = Rect::new(layout.caret.x, layout.caret.y, 3.0, metrics.height);
        assert!(snapshot.contains_color(caret, theme.cursor), "the caret");
    }

    #[test]
    fn test_sync_compares_the_text_only_when_its_version_changes() {
        let mut area = TextArea::with_text("draft");
        area.select(1..3);
        let props = |text, version| TextAreaProps {
            label: None,
            placeholder: "",
            text,
            version,
        };
        area.sync(props("draft", 1));
        assert_eq!(
            area.selection(),
            Some(1..3),
            "the same text keeps the selection"
        );
        area.sync(props("other", 1));
        assert_eq!(area.text(), "draft", "the same version is not compared");
        area.sync(props("other", 2));
        assert_eq!(area.text(), "other");
        assert_eq!(area.selection(), None);
    }

    #[test]
    fn test_measure_counts_the_rows_the_text_wraps_into_at_the_width() {
        let metrics = CellMetrics {
            baseline: 15.0,
            height: 20.0,
            width: 10.0,
        };
        let area = TextArea::with_text("the quick brown fox");
        assert_eq!(
            area.measure(100.0, metrics, 1.0),
            40.0,
            "two rows of ten cells"
        );
        assert_eq!(area.measure(f32::INFINITY, metrics, 1.0), 20.0, "one line");
        let area = area.with_wrap(false);
        assert_eq!(area.measure(100.0, metrics, 1.0), 20.0, "no wrapping");
    }

    /// Where the caret before `offset` is in the window, in `area`'s
    /// proportional layout as last drawn: the middle of its line.
    fn glyph_point(area: &TextArea, offset: usize) -> Point {
        let shaped = area.shaped.as_ref().expect("laid out");
        let text = area.layout.expect("drawn").text;
        let caret = shaped.layout.caret_rect(offset);
        Point::new(
            text.x + caret.x - area.scroll_x,
            text.y + caret.y + caret.height / 2.0
                - area.scroll_row as f32 * area.layout.expect("drawn").metrics.height,
        )
    }

    #[test]
    fn test_proportional_up_and_down_keep_the_pixel_column_not_the_cell_column() {
        let mut window = TestWindow::keyboard_first(400, 200);
        let mut area = TextArea::with_text("iiiiiiii\nWWWWWWWW").with_proportional_font();
        draw(&mut area, 40.0, 5.0);
        area.select(4..4);
        keys(&mut area, &mut window, &["<down>"]);
        let caret = area.edit.caret();
        assert!(
            (9..9 + 3).contains(&caret),
            "below four narrow letters are fewer wide ones, not four: {caret}"
        );
        keys(&mut area, &mut window, &["<up>"]);
        assert_eq!(area.edit.caret(), 4, "back to the column it started from");
        keys(&mut area, &mut window, &["<end>"]);
        assert_eq!(area.edit.caret(), 8);
    }

    #[test]
    fn test_a_proportional_press_lands_between_the_glyphs_under_it() {
        let mut window = TestWindow::keyboard_first(400, 200);
        let mut area = TextArea::with_text("iiiiWWWW\nmore").with_proportional_font();
        draw(&mut area, 40.0, 5.0);
        let point = glyph_point(&area, 6);
        let press = Input::PointerButton(ButtonInput::press(PointerButton::Left, point));
        window.feed(&mut area, &press);
        assert_eq!(area.edit.caret(), 6);
        let below = glyph_point(&area, 11);
        let press = Input::PointerButton(ButtonInput::press(PointerButton::Left, below));
        window.feed(&mut area, &press);
        assert_eq!(area.edit.caret(), 11, "on the second line");
    }

    #[test]
    fn test_proportional_text_wraps_where_its_paragraph_wraps_and_measures_so() {
        let mut window = TestWindow::keyboard_first(400, 200);
        let text = "the quick brown fox jumps over the lazy dog again and again";
        let mut area = TextArea::with_text(text).with_proportional_font();
        let (frame, metrics) = draw(&mut area, 12.0, 8.0);
        let width = 12.0 * metrics.width;
        let lines = frame
            .paragraph_layout(&text_paragraph(text), Rect::new(0.0, 0.0, width, 0.0))
            .lines()
            .len();
        assert!(lines > 1, "it wraps");
        assert_eq!(area.rows().len(), lines);
        assert_eq!(
            area.measure(width, metrics, frame.scale_factor()),
            (lines as f32 * metrics.height).ceil()
        );
        let cells = TextArea::with_text(text);
        assert_eq!(
            cells.measure(width, metrics, frame.scale_factor()),
            (wrap::rows(text, Some(12)).len() as f32 * metrics.height).ceil(),
            "an area on cells is measured on cells"
        );
        keys(&mut area, &mut window, &["C-<home>", "<down>"]);
        let second = area.rows()[1].range.clone();
        assert!(
            second.contains(&area.edit.caret()),
            "down to the next wrapped row"
        );
    }

    #[test]
    fn test_proportional_keys_between_frames_follow_the_text_as_typed() {
        let mut window = TestWindow::keyboard_first(400, 200);
        let mut area = TextArea::new().with_proportional_font();
        draw(&mut area, 40.0, 5.0);
        area.edit.insert("WWWW\niiiiiiii");
        keys(&mut area, &mut window, &["<up>"]);
        let caret = area.edit.caret();
        assert!(
            caret < 4,
            "on the first line, fewer wide letters above eight narrow ones: {caret}"
        );
    }

    #[test]
    fn test_a_proportional_selection_fills_behind_its_glyphs() {
        let theme = Theme::dark();
        let mut area = TextArea::with_text("WWWW iiii\nnext").with_proportional_font();
        area.select(0..4);
        let Some(snapshot) = offscreen::render(300, 120, |frame| {
            frame.clear(Rgba::from(theme.background));
            area.draw(frame, Rect::new(0.0, 0.0, 300.0, 120.0), &theme);
        }) else {
            return;
        };
        let shaped = area.shaped.as_ref().expect("laid out");
        let selected = shaped.layout.range_rects(0..4)[0];
        let inside = Rect::new(
            selected.x + selected.width - 2.0,
            selected.y + 1.0,
            1.0,
            2.0,
        );
        assert!(
            snapshot.contains_color(inside, theme.selection),
            "the selection"
        );
        let after = shaped.layout.caret_rect(6);
        let past = Rect::new(after.x, after.y + 1.0, 4.0, 2.0);
        assert!(
            !snapshot.contains_color(past, theme.selection),
            "only its glyphs"
        );
    }
}
