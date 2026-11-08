//! [`CodeView`]: a read-only, monospace view of text as large as a whole
//! source file or log (100,000 lines and more), for a file preview or a
//! diff: line numbers in a gutter, syntax colors, the current line lit,
//! a selection to copy, a search, and marks in the gutter.
//!
//! The view keeps the text once and lays out only the rows in sight, so a
//! frame costs the same for a short file as for a huge one. Turning soft
//! wrap on (or resizing a wrapped view) measures every line once to count
//! its rows; scrolling never does.
//!
//! Colors come from [`Highlight`] spans the app gives
//! [`CodeView::set_highlights`]: byte ranges of the text, each with a
//! highlight name resolved through the theme's
//! [`syntax`](crate::Theme::syntax) colors, so any highlighter works
//! (tree-sitter in the app, or [`Highlighter`](crate::Highlighter) behind
//! the `highlight` feature). Tabs are drawn to the next multiple of four
//! cells.
//!
//! Keys, while the app says the view is focused: Shift and an arrow
//! extend the selection from the cursor a character or a line, Shift+Home
//! and Shift+End to the ends of the line, the primary modifier (Ctrl, or
//! Cmd on macOS) with A selects everything and with C (like Ctrl+Insert)
//! copies the selection; Esc drops the selection. Every other key scrolls
//! as a [`ScrollView`] does, so an app with vim-style keys drives the
//! cursor itself through [`CodeView::set_cursor`] and
//! [`CodeView::set_selection`]. The pointer: a left press puts the cursor
//! there, a drag selects, Shift+press extends the selection, and the wheel
//! and the scrollbars scroll.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use accesskit::{Node, Role, TextPosition, TextSelection};

use super::super::app::access::node_id;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::color::Rgba;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::text;
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::platform::clipboard::{ClipboardKind, SystemClipboard};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::find::{Find, find_all};
use super::flow_list::ScrollAlign;
use super::scroll_view::ScrollView;

// ========================================================================
// Constants
// ========================================================================

/// How many cells apart tab stops are.
pub(crate) const TAB_WIDTH: usize = 4;

/// The blank cells right of the line numbers, before the text.
const GUTTER_GAP_CELLS: usize = 1;

/// The cells a gutter mark takes, left of the line numbers.
const MARK_CELLS: usize = 1;

/// The width of the cursor, in logical pixels.
const CURSOR_WIDTH: f32 = 2.0;

/// The alpha of the warning color behind a match that is not the current
/// one.
const MATCH_ALPHA: u8 = 0x60;

/// The thickness of the frame around the current match, in logical
/// pixels.
const CURRENT_MATCH_BORDER: f32 = 1.0;

/// What ends a line.
const LINE_FEED: char = '\n';

/// What a line may end with before its line feed, left undrawn.
const CARRIAGE_RETURN: char = '\r';

// ========================================================================
// Data Structures
// ========================================================================

/// A read-only, monospace view of a large text. The app owns it, gives it
/// the text and its highlights, hands it the window's pointer events (and
/// its keys while it has the focus), and draws it each frame.
#[derive(Clone, Debug, Default)]
pub struct CodeView {
    /// Where a selection started, or `None` without one: the selection runs
    /// from here to the cursor.
    anchor: Option<usize>,
    /// The byte offset of the cursor.
    cursor: usize,
    /// Whether a left press is held, selecting as it moves.
    dragging: bool,
    /// Where it was drawn last, or `None` before the first frame.
    drawn: Option<Drawn>,
    /// The search, if any.
    find: Option<Find<Range<usize>>>,
    /// Whether the app gave the view the keyboard.
    focused: bool,
    /// Whether the gutter of line numbers and marks shows.
    gutter: bool,
    /// The syntax spans, sorted and apart.
    highlights: Vec<Highlight>,
    /// Where each line starts, a byte offset per line.
    line_starts: Vec<usize>,
    /// The gutter marks, by line.
    marks: BTreeMap<usize, GutterMark>,
    /// The cells of the widest line, for the width of an unwrapped view.
    max_cells: usize,
    /// A scroll asked for before the first frame, made on it.
    pending: Option<(usize, ScrollAlign)>,
    /// The row each line starts on when wrapped, for the width it was
    /// counted at; `None` unwrapped, where each line is a row.
    rows: Option<RowIndex>,
    /// The scroll of the text.
    scroll: ScrollView,
    /// The text.
    text: String,
    /// Whether long lines wrap (soft wrap) instead of scrolling sideways.
    wrap: bool,
}

/// A span of the text drawn in a highlight name's color.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Highlight {
    /// The highlight name (`keyword`, `string`, `function.method`), its
    /// color the theme's [`SyntaxColors::color`](crate::SyntaxColors::color).
    pub name: Arc<str>,
    /// The bytes of the text it covers.
    pub range: Range<usize>,
}

/// A mark in a line's gutter, left of its number: a diff sign, a
/// breakpoint, a warning.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GutterMark {
    /// Its color, by theme role.
    pub color: ThemeToken,
    /// What it shows: one cell wide, such as `+`, `~`, or `●`.
    pub glyph: char,
}

/// What became of an input offered to a [`CodeView`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CodeViewResponse {
    /// The selection was copied to the clipboard.
    Copied,
    /// The view used the input: it scrolled, or a drag goes on.
    Handled,
    /// The input is not the view's.
    Ignored,
    /// The cursor moved or the selection changed.
    Moved,
}

/// The row each line starts on, for wrapping at `cols`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RowIndex {
    /// The cells a row holds.
    cols: usize,
    /// The first row of each line, and the count of all rows last.
    starts: Vec<usize>,
}

/// Where the view was drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn {
    /// The whole area.
    area: Rect,
    /// The size of a cell.
    metrics: CellMetrics,
    /// How far the text is scrolled.
    offset: Point,
    /// Physical pixels per logical pixel.
    scale_factor: f64,
    /// Where the text is, right of the gutter.
    text: Rect,
}

/// One row of the text: a whole line, or a part of a wrapped one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Row {
    /// Where it ends: its last byte's end, before any line feed.
    end: usize,
    /// The line it is part of.
    line: usize,
    /// Where it starts.
    start: usize,
    /// Whether it is its line's first row, which shows the line number.
    first: bool,
}

// ========================================================================
// CodeView Implementations
// ========================================================================

impl CodeView {
    // --------------------------------------------------------------------
    // Text
    // --------------------------------------------------------------------

    /// A view of `text`, unwrapped, with line numbers, the cursor at its
    /// start.
    pub fn new(text: impl Into<String>) -> Self {
        let mut view = Self {
            gutter: true,
            ..Self::default()
        };
        view.set_text(text);
        view
    }

    /// Shows `text` instead, dropping the highlights, the marks, the
    /// search, and the selection, the cursor at its start.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.line_starts = std::iter::once(0)
            .chain(self.text.match_indices(LINE_FEED).map(|(at, _)| at + 1))
            .collect();
        self.max_cells = (0..self.line_starts.len())
            .map(|line| cells_of(self.line_text(line)))
            .max()
            .unwrap_or(0);
        self.highlights.clear();
        self.marks.clear();
        self.find = None;
        self.anchor = None;
        self.cursor = 0;
        self.rows = None;
    }

    /// The text shown.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// How many lines the text has: one more than its line feeds.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Colors the text with `highlights`, replacing those before. They may
    /// come in any order; where two overlap, the later one starting wins
    /// from its start.
    pub fn set_highlights(&mut self, mut highlights: Vec<Highlight>) {
        highlights.sort_by_key(|highlight| highlight.range.start);
        let mut apart: Vec<Highlight> = Vec::with_capacity(highlights.len());
        for mut highlight in highlights {
            highlight.range.end = highlight.range.end.min(self.text.len());
            if highlight.range.is_empty() {
                continue;
            }
            if let Some(last) = apart.last_mut() {
                last.range.end = last.range.end.min(highlight.range.start);
                if last.range.is_empty() {
                    apart.pop();
                }
            }
            apart.push(highlight);
        }
        self.highlights = apart;
    }

    /// Shows `marks` in the gutter, one per line (counting from 0),
    /// replacing those before.
    pub fn set_marks(&mut self, marks: impl IntoIterator<Item = (usize, GutterMark)>) {
        self.marks = marks.into_iter().collect();
    }

    /// Shows the gutter of line numbers and marks (`true`, the default) or
    /// hides it.
    pub fn set_gutter(&mut self, gutter: bool) {
        self.gutter = gutter;
    }

    /// Wraps long lines at the view's width (`true`) or scrolls them
    /// sideways.
    pub fn set_wrap(&mut self, wrap: bool) {
        self.wrap = wrap;
        if !wrap {
            self.rows = None;
        }
    }

    // --------------------------------------------------------------------
    // Cursor and selection
    // --------------------------------------------------------------------

    /// The cursor, a byte offset into the text.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Puts the cursor at byte `offset` (moved back to a character's
    /// start, held to the text), drops the selection, and scrolls the
    /// cursor into sight.
    pub fn set_cursor(&mut self, offset: usize) {
        self.anchor = None;
        self.cursor = self.boundary(offset);
        self.reveal_cursor();
    }

    /// The selected bytes, or `None` with nothing selected.
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor.filter(|&anchor| anchor != self.cursor)?;
        Some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    /// Selects the bytes `range` (held to the text and to characters), the
    /// cursor at its end, scrolled into sight.
    pub fn set_selection(&mut self, range: Range<usize>) {
        self.anchor = Some(self.boundary(range.start));
        self.cursor = self.boundary(range.end);
        self.reveal_cursor();
    }

    /// The selected text, line feeds as they are, or `None` with nothing
    /// selected.
    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.text[range])
    }

    /// The line (counting from 0) byte `offset` is on.
    pub fn line_of(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&start| start <= offset) - 1
    }

    /// Scrolls so that `line` (counting from 0) shows as `align` says, at
    /// once; made on the first frame when called before it.
    pub fn scroll_to_line(&mut self, line: usize, align: ScrollAlign) {
        let line = line.min(self.line_count() - 1);
        let Some(drawn) = self.drawn else {
            self.pending = Some((line, align));
            return;
        };
        let row_height = drawn.metrics.height;
        let top = self.first_row(line) as f32 * row_height;
        let height = self.rows_of(line) as f32 * row_height;
        let view = drawn.text.height;
        let y = match align {
            ScrollAlign::Bottom => top + height - view,
            ScrollAlign::Center => top + (height - view) / 2.0,
            ScrollAlign::Nearest if top < drawn.offset.y => top,
            ScrollAlign::Nearest if top + height > drawn.offset.y + view => top + height - view,
            ScrollAlign::Nearest => drawn.offset.y,
            ScrollAlign::Top => top,
        };
        self.scroll
            .scroll_to(Point::new(drawn.offset.x, y.max(0.0)));
    }

    // --------------------------------------------------------------------
    // Find
    // --------------------------------------------------------------------

    /// Highlights every match of `query` in the text, letters matching in
    /// either case unless `case_sensitive`; the current one is the first
    /// at or after the cursor, scrolled into sight. An empty query ends the
    /// search.
    pub fn set_find(&mut self, query: &str, case_sensitive: bool) {
        if query.is_empty() {
            self.find = None;
            return;
        }
        let matches = find_all(&self.text, query, case_sensitive);
        let cursor = self.cursor;
        self.find = Some(Find::new(query, case_sensitive, matches, |found| {
            found.start >= cursor
        }));
        self.reveal_match();
    }

    /// How many matches the search found; 0 without a search.
    pub fn match_count(&self) -> usize {
        self.find.as_ref().map_or(0, |find| find.matches().len())
    }

    /// The index of the current match, or `None` without one.
    pub fn current_match(&self) -> Option<usize> {
        self.find.as_ref()?.current()
    }

    /// Steps to the next match (after the last, the first), selects it,
    /// and scrolls it into sight.
    pub fn next_match(&mut self) {
        self.step_match(1);
    }

    /// Steps to the previous match (before the first, the last), selects
    /// it, and scrolls it into sight.
    pub fn previous_match(&mut self) {
        self.step_match(-1);
    }

    /// Moves the current match `delta` matches, wrapping.
    fn step_match(&mut self, delta: isize) {
        if self.find.as_mut().is_some_and(|find| find.step(delta)) {
            self.reveal_match();
        }
    }

    /// Selects the current match and scrolls it into sight.
    fn reveal_match(&mut self) {
        let found = self
            .find
            .as_ref()
            .and_then(|find| find.current_match().cloned());
        if let Some(found) = found {
            self.set_selection(found);
        }
    }

    // --------------------------------------------------------------------
    // Accessibility
    // --------------------------------------------------------------------

    /// Describes the view as last drawn to screen readers as `id`: a
    /// read-only multiline text holding the lines in sight (all of a huge
    /// file would cost every frame what scrolling saves), as a text run
    /// (child 0), with the selection when it lies in sight.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(drawn) = self.drawn else {
            return;
        };
        let lines = self.lines_in_sight(drawn);
        let start = self.line_starts[lines.start];
        let end = self.line_end(lines.end.saturating_sub(1).max(lines.start));
        let shown = &self.text[start..end];
        let run_id = id.child(0);
        let mut text = Node::new(Role::MultilineTextInput);
        text.set_read_only();
        text.set_value(shown);
        let in_sight = |offset: usize| (start..=end).contains(&offset);
        if let Some(anchor) = self
            .anchor
            .filter(|&anchor| in_sight(anchor) && in_sight(self.cursor))
        {
            let position = |offset: usize| TextPosition {
                character_index: self.text[start..offset].chars().count(),
                node: node_id(run_id),
            };
            text.set_text_selection(TextSelection {
                anchor: position(anchor),
                focus: position(self.cursor),
            });
        }
        frame.enter_access(id, drawn.area, text);
        let mut run = Node::new(Role::TextRun);
        run.set_value(shown);
        run.set_character_lengths(
            shown
                .chars()
                .map(|c| c.len_utf8() as u8)
                .collect::<Vec<_>>(),
        );
        frame.access(run_id, drawn.text, run);
        frame.leave_access();
    }

    // --------------------------------------------------------------------
    // Lines and rows
    // --------------------------------------------------------------------

    /// Where `line` ends: before its line feed and any carriage return.
    fn line_end(&self, line: usize) -> usize {
        let end = self
            .line_starts
            .get(line + 1)
            .map_or(self.text.len(), |next| next - 1);
        let start = self.line_starts[line];
        match self.text[start..end].ends_with(CARRIAGE_RETURN) {
            true => end - 1,
            false => end,
        }
    }

    /// The text of `line`, without its line ending.
    fn line_text(&self, line: usize) -> &str {
        &self.text[self.line_starts[line]..self.line_end(line)]
    }

    /// The first row of `line`.
    fn first_row(&self, line: usize) -> usize {
        self.rows.as_ref().map_or(line, |rows| rows.starts[line])
    }

    /// How many rows `line` takes.
    fn rows_of(&self, line: usize) -> usize {
        self.first_row(line + 1) - self.first_row(line)
    }

    /// How many rows the text takes.
    fn row_count(&self) -> usize {
        self.first_row(self.line_count())
    }

    /// The `row`th row, held to the last.
    fn row(&self, row: usize) -> Row {
        let row = row.min(self.row_count().saturating_sub(1));
        let line = match &self.rows {
            Some(rows) => rows.starts.partition_point(|&start| start <= row) - 1,
            None => row,
        };
        let sub = row - self.first_row(line);
        let start = self.line_starts[line];
        let text = self.line_text(line);
        let breaks = match &self.rows {
            Some(rows) => row_breaks(text, rows.cols),
            None => vec![0],
        };
        let from = breaks.get(sub).copied().unwrap_or(text.len());
        let to = breaks.get(sub + 1).copied().unwrap_or(text.len());
        Row {
            end: start + to,
            first: sub == 0,
            line,
            start: start + from,
        }
    }

    /// The row byte `offset` is on.
    fn row_of(&self, offset: usize) -> usize {
        let line = self.line_of(offset);
        let first = self.first_row(line);
        let Some(rows) = &self.rows else {
            return first;
        };
        let within = offset - self.line_starts[line];
        let breaks = row_breaks(self.line_text(line), rows.cols);
        first + breaks.partition_point(|&at| at <= within).saturating_sub(1)
    }

    /// Counts every line's rows for wrapping at `cols`, unless already
    /// counted for it.
    fn count_rows(&mut self, cols: usize) {
        let cols = cols.max(1);
        if self.rows.as_ref().is_some_and(|rows| rows.cols == cols) {
            return;
        }
        let mut starts = Vec::with_capacity(self.line_count() + 1);
        let mut row = 0;
        for line in 0..self.line_count() {
            starts.push(row);
            row += row_breaks(self.line_text(line), cols).len();
        }
        starts.push(row);
        self.rows = Some(RowIndex { cols, starts });
    }

    /// The lines with a row in sight in `drawn`.
    fn lines_in_sight(&self, drawn: Drawn) -> Range<usize> {
        let rows = self.rows_in_sight(drawn);
        let first = self.row(rows.start).line;
        let last = self.row(rows.end.saturating_sub(1).max(rows.start)).line;
        first..last + 1
    }

    /// The rows in sight in `drawn`.
    fn rows_in_sight(&self, drawn: Drawn) -> Range<usize> {
        let height = drawn.metrics.height;
        let first = (drawn.offset.y / height).floor().max(0.0) as usize;
        let end = ((drawn.offset.y + drawn.text.height) / height).ceil() as usize;
        first.min(self.row_count())..end.min(self.row_count())
    }

    /// `offset` moved back to the start of the character it is in, held
    /// to the text.
    fn boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// Scrolls the cursor into sight, once drawn.
    fn reveal_cursor(&mut self) {
        let Some(drawn) = self.drawn else {
            self.pending = Some((self.line_of(self.cursor), ScrollAlign::Center));
            return;
        };
        let row = self.row_of(self.cursor);
        let start = self.row(row).start;
        let x = cells_of(&self.text[start..self.cursor]) as f32 * drawn.metrics.width;
        let y = row as f32 * drawn.metrics.height;
        self.scroll
            .reveal(Rect::new(x, y, drawn.metrics.width, drawn.metrics.height));
    }

    /// The byte offset under `point` as last drawn: the nearest character
    /// boundary on the row there.
    fn offset_at(&self, point: Point) -> Option<usize> {
        let drawn = self.drawn?;
        let metrics = drawn.metrics;
        let y = point.y - drawn.text.y + drawn.offset.y;
        let row = self.row((y / metrics.height).floor().max(0.0) as usize);
        let x = point.x - drawn.text.x + drawn.offset.x;
        let wanted = (x / metrics.width).round().max(0.0) as usize;
        let text = &self.text[row.start..row.end];
        let mut cells = 0;
        for (at, c) in text.char_indices() {
            let width = advance(cells, c);
            if cells + width.div_ceil(2) > wanted {
                return Some(row.start + at);
            }
            cells += width;
        }
        Some(row.end)
    }

    /// Reacts to a key while focused, as the module documentation lists.
    fn handle_key(&mut self, key: Key, cx: &mut EventCx<'_>) -> CodeViewResponse {
        if !self.focused {
            return CodeViewResponse::Ignored;
        }
        let shift = key.mods == Mods::SHIFT;
        let moved = match key.code {
            KeyCode::Char('a') if key.mods == Mods::PRIMARY => {
                self.anchor = Some(0);
                self.cursor = self.text.len();
                return CodeViewResponse::Moved;
            }
            KeyCode::Char('c') if key.mods == Mods::PRIMARY => return self.copy(cx.clipboard()),
            KeyCode::Insert if key.mods == Mods::CTRL => return self.copy(cx.clipboard()),
            KeyCode::Esc if key.mods == Mods::NONE && self.selection().is_some() => {
                self.anchor = None;
                return CodeViewResponse::Moved;
            }
            KeyCode::Left if shift => self.step_char(false),
            KeyCode::Right if shift => self.step_char(true),
            KeyCode::Up if shift => self.step_row(false),
            KeyCode::Down if shift => self.step_row(true),
            KeyCode::Home if shift => self.row(self.row_of(self.cursor)).start,
            KeyCode::End if shift => self.row(self.row_of(self.cursor)).end,
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
            | KeyCode::Up => {
                return match self.scroll.handle(&Input::Key(key), cx).is_ignored() {
                    true => CodeViewResponse::Ignored,
                    false => CodeViewResponse::Handled,
                };
            }
        };
        self.anchor.get_or_insert(self.cursor);
        self.cursor = moved;
        self.reveal_cursor();
        CodeViewResponse::Moved
    }

    /// Where the cursor goes a character forward (or back).
    fn step_char(&self, forward: bool) -> usize {
        match forward {
            true => self.text[self.cursor..]
                .chars()
                .next()
                .map_or(self.cursor, |c| self.cursor + c.len_utf8()),
            false => self.text[..self.cursor]
                .chars()
                .next_back()
                .map_or(self.cursor, |c| self.cursor - c.len_utf8()),
        }
    }

    /// Where the cursor goes a row down (or up), at the same cell, or the
    /// row's end when it is shorter.
    fn step_row(&self, down: bool) -> usize {
        let row = self.row_of(self.cursor);
        let here = self.row(row);
        let cells = cells_of(&self.text[here.start..self.cursor]);
        let target = match down {
            true if row + 1 < self.row_count() => self.row(row + 1),
            false if row > 0 => self.row(row - 1),
            true => return self.text.len(),
            false => return 0,
        };
        let text = &self.text[target.start..target.end];
        let mut walked = 0;
        for (at, c) in text.char_indices() {
            if walked >= cells {
                return target.start + at;
            }
            walked += advance(walked, c);
        }
        target.end
    }

    /// Copies the selection to `clipboard`; ignored with nothing selected.
    fn copy(&self, clipboard: &mut SystemClipboard) -> CodeViewResponse {
        let Some(text) = self.selected_text() else {
            return CodeViewResponse::Ignored;
        };
        // Kept in the process when the system refuses it.
        let _ = clipboard.write_text(ClipboardKind::Clipboard, text);
        CodeViewResponse::Copied
    }

    /// Reacts to the pointer: a press over the text places the cursor (or
    /// extends the selection with Shift), a drag selects, and the rest
    /// goes to the scroll.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> CodeViewResponse {
        let over_text = |point: Point| self.drawn.is_some_and(|drawn| drawn.text.contains(point));
        match input {
            Input::PointerButton(button)
                if button.button == PointerButton::Left
                    && button.pressed
                    && over_text(button.position) =>
            {
                let Some(offset) = self.offset_at(button.position) else {
                    return CodeViewResponse::Ignored;
                };
                match button.mods == Mods::SHIFT {
                    true => {
                        self.anchor.get_or_insert(self.cursor);
                    }
                    false => self.anchor = Some(offset),
                }
                self.cursor = offset;
                self.dragging = true;
                CodeViewResponse::Moved
            }
            Input::PointerButton(button)
                if button.button == PointerButton::Left && !button.pressed && self.dragging =>
            {
                self.dragging = false;
                CodeViewResponse::Handled
            }
            Input::PointerMoved(point) if self.dragging => {
                let Some(offset) = self.offset_at(*point) else {
                    return CodeViewResponse::Handled;
                };
                if offset == self.cursor {
                    return CodeViewResponse::Handled;
                }
                self.cursor = offset;
                self.reveal_cursor();
                CodeViewResponse::Moved
            }
            input => match self.scroll.handle(input, cx).is_ignored() {
                true => CodeViewResponse::Ignored,
                false => CodeViewResponse::Handled,
            },
        }
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// How wide the gutter is in cells: a mark, the widest line number,
    /// and a gap; none without a gutter.
    fn gutter_cells(&self) -> usize {
        match self.gutter {
            true => MARK_CELLS + self.line_count().to_string().len() + GUTTER_GAP_CELLS,
            false => 0,
        }
    }

    /// Draws `row`, `index`th of the text, at `y`: its fills (the current
    /// line, the selection, the matches), its gutter, and its text.
    fn draw_row(&self, frame: &mut Frame, index: usize, y: f32, drawn: Drawn, theme: &Theme) {
        let row = self.row(index);
        let metrics = drawn.metrics;
        let left = drawn.text.x - drawn.offset.x;
        let x_of =
            |offset: usize| left + cells_of(&self.text[row.start..offset]) as f32 * metrics.width;
        let full = Rect::new(drawn.text.x, y, drawn.text.width, metrics.height);
        if self.line_of(self.cursor) == row.line {
            frame.fill_rect(full, theme.hover);
        }
        // A selection or match running past the row's end takes one more
        // cell, for its line feed.
        let span_rect = |range: &Range<usize>| -> Option<Rect> {
            let start = range.start.max(row.start);
            let end = range.end.min(row.end);
            let past = range.end > row.end && range.start <= row.end;
            if start > end || (start == end && !past) {
                return None;
            }
            let width = x_of(end) - x_of(start) + if past { metrics.width } else { 0.0 };
            Some(Rect::new(x_of(start), y, width, metrics.height))
        };
        if let Some(rect) = self.selection().as_ref().and_then(span_rect) {
            frame.fill_rect(rect, theme.selection);
        }
        if let Some(find) = &self.find {
            let first = find
                .matches()
                .partition_point(|found| found.end <= row.start);
            for (index, found) in find.matches()[first..].iter().enumerate() {
                if found.start > row.end {
                    break;
                }
                let Some(rect) = span_rect(found) else {
                    continue;
                };
                let is_current = find.current() == Some(first + index);
                let scale_factor = drawn.scale_factor;
                let style = match is_current {
                    true => BoxStyle {
                        border: Some(Border {
                            color: Rgba::from(theme.warning),
                            width: logical(CURRENT_MATCH_BORDER, scale_factor),
                        }),
                        ..theme.shape.highlight(theme.selection, scale_factor)
                    },
                    false => BoxStyle {
                        fill: theme.warning.with_alpha(MATCH_ALPHA),
                        ..theme.shape.highlight(theme.warning, scale_factor)
                    },
                };
                frame.draw_box(rect, &style);
            }
        }
        frame.draw_text(
            &self.styled(row, theme),
            Point::new(left, y),
            drawn.text,
            theme,
        );
        if self.focused
            && (row.start..=row.end).contains(&self.cursor)
            && self.row_of(self.cursor) == index
        {
            let width = logical(CURSOR_WIDTH, drawn.scale_factor);
            frame.fill_rect_over_text(
                Rect::new(x_of(self.cursor), y, width, metrics.height),
                theme.cursor,
            );
        }
        if row.first && self.gutter {
            self.draw_gutter(frame, row.line, y, drawn, theme);
        }
    }

    /// Draws the gutter of `line` at `y`: its mark and its number, right
    /// aligned, the cursor's line in the foreground and the rest muted.
    fn draw_gutter(&self, frame: &mut Frame, line: usize, y: f32, drawn: Drawn, theme: &Theme) {
        let metrics = drawn.metrics;
        let cells = self.gutter_cells();
        let gutter = Rect::new(
            drawn.area.x,
            y,
            cells as f32 * metrics.width,
            metrics.height,
        );
        let number = (line + 1).to_string();
        let digits = cells - MARK_CELLS - GUTTER_GAP_CELLS;
        let color = match self.line_of(self.cursor) == line {
            true => ThemeToken::Foreground,
            false => ThemeToken::Muted,
        };
        let mut shown = StyledText::new();
        match self.marks.get(&line) {
            Some(mark) => shown.push(Span::new(
                mark.glyph.to_string(),
                TextColor::Token(mark.color),
            )),
            None => shown.push(Span::new(" ", TextColor::Token(color))),
        }
        let pad = " ".repeat(digits.saturating_sub(number.len()));
        shown.push(Span::new(format!("{pad}{number}"), TextColor::Token(color)));
        frame.draw_text(&shown, Point::new(gutter.x, y), gutter, theme);
    }

    /// The text of `row` in its highlight colors from `theme`, its tabs drawn as
    /// spaces to the next stop.
    fn styled(&self, row: Row, theme: &Theme) -> StyledText {
        let mut styled = StyledText::new();
        let mut cells = 0;
        let mut push = |styled: &mut StyledText, text: &str, color: TextColor| {
            let mut shown = String::with_capacity(text.len());
            for c in text.chars() {
                let width = advance(cells, c);
                match c {
                    '\t' => shown.extend(std::iter::repeat_n(' ', width)),
                    c => shown.push(c),
                }
                cells += width;
            }
            styled.push(Span::new(shown, color));
        };
        let plain = TextColor::Token(ThemeToken::Foreground);
        let first = self
            .highlights
            .partition_point(|highlight| highlight.range.end <= row.start);
        let mut at = row.start;
        for highlight in &self.highlights[first..] {
            if highlight.range.start >= row.end {
                break;
            }
            let start = highlight.range.start.max(row.start);
            let end = highlight.range.end.min(row.end);
            if start > at {
                push(&mut styled, &self.text[at..start], plain);
            }
            let color = theme
                .syntax
                .color(&highlight.name)
                .map_or(plain, |color| TextColor::Rgba(Rgba::from(color)));
            push(&mut styled, &self.text[start..end], color);
            at = end;
        }
        if at < row.end {
            push(&mut styled, &self.text[at..row.end], plain);
        }
        styled
    }
}

impl Widget for CodeView {
    type Output = ();
    type Response = CodeViewResponse;

    /// Reacts to `input`: keys as the module documentation lists while
    /// focused, the pointer placing the cursor and selecting, and the rest
    /// scrolling. Asks for a redraw whenever it used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> CodeViewResponse {
        let response = match input {
            Input::Key(key) => self.handle_key(*key, cx),
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => self.handle_pointer(input, cx),
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the view over `area` in `theme`'s colors: the gutter at the
    /// left, the rows in sight, and the scrollbars.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let gutter = self.gutter_cells() as f32 * metrics.width;
        let text_width = (area.width - gutter).max(0.0);
        if self.wrap {
            self.count_rows(metrics.cols_in(text_width));
        }
        let content_width = match self.wrap {
            true => text_width,
            false => (self.max_cells + 1) as f32 * metrics.width + gutter,
        };
        let content_height = self.row_count() as f32 * metrics.height;
        self.scroll.set_content_size(content_width, content_height);
        let viewport = self.scroll.draw_at(frame, area, theme, Instant::now());
        let text = Rect::new(
            viewport.x + gutter,
            viewport.y,
            (viewport.width - gutter).max(0.0),
            viewport.height,
        );
        self.drawn = Some(Drawn {
            area: viewport,
            metrics,
            offset: self.scroll.offset(),
            scale_factor: frame.scale_factor(),
            text,
        });
        if let Some((line, align)) = self.pending.take() {
            self.scroll_to_line(line, align);
            self.drawn = self.drawn.map(|drawn| Drawn {
                offset: self.scroll.offset(),
                ..drawn
            });
        }
        let Some(drawn) = self.drawn else {
            return;
        };
        frame.clipped(viewport, |frame| {
            for index in self.rows_in_sight(drawn) {
                let y = viewport.y + index as f32 * metrics.height - drawn.offset.y;
                self.draw_row(frame, index, y, drawn, theme);
            }
        });
    }

    /// Tells the view whether it has the keyboard; it takes keys and draws
    /// its cursor only while it does.
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.scroll.set_focused(focused);
    }

    /// Whether the app last said the view has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }

    /// Asks for the next frame of a scroll glide under way; call it from
    /// [`Application::about_to_wait`](crate::Application::about_to_wait).
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.scroll.about_to_wait(cx)
    }
}

impl Outcome for CodeViewResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Highlight Implementations
// ========================================================================

impl Highlight {
    /// The bytes `range` drawn in the color of highlight `name`.
    pub fn new(range: Range<usize>, name: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            range,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The cells `c` takes drawn at cell `at` of its row: a tab to the next
/// stop, a control character none, others their width.
pub(crate) fn advance(at: usize, c: char) -> usize {
    match c {
        '\t' => TAB_WIDTH - at % TAB_WIDTH,
        c => text::char_width(c),
    }
}

/// The cells `text` takes drawn from the start of a row.
fn cells_of(text: &str) -> usize {
    text.chars().fold(0, |cells, c| cells + advance(cells, c))
}

/// Where each row of `line` starts when it wraps at `cols` cells: byte
/// offsets, the first 0. A character too wide for what is left of a row
/// starts the next.
fn row_breaks(line: &str, cols: usize) -> Vec<usize> {
    let mut breaks = vec![0];
    let mut cells = 0;
    for (at, c) in line.char_indices() {
        let width = advance(cells, c);
        if cells + width > cols && cells > 0 {
            breaks.push(at);
            cells = 0;
        }
        cells += advance(cells, c);
    }
    breaks
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgb;
    use crate::base::key::parse_key;
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, drawn_text, ui_text};

    const SOURCE: &str = "fn main() {\n\tlet x = 1;\n}\n";

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    /// `view` drawn over all of `window`, returning the frame.
    fn draw(window: &TestWindow, view: &mut CodeView) -> Frame {
        let mut frame = window.frame();
        let bounds = frame.bounds();
        view.draw(&mut frame, bounds, &Theme::dark());
        frame
    }

    /// The point at the middle of cell `col` of row `row` of the text.
    fn cell(view: &CodeView, row: usize, col: usize) -> Point {
        let drawn = view.drawn.expect("drawn");
        let metrics = drawn.metrics;
        Point::new(
            drawn.text.x + (col as f32 + 0.25) * metrics.width,
            drawn.text.y + (row as f32 + 0.5) * metrics.height,
        )
    }

    #[test]
    fn test_a_huge_text_lays_out_only_the_rows_in_sight() {
        let text: String = (0..100_000).map(|line| format!("line {line}\n")).collect();
        let window = TestWindow::new(400, 200);
        let mut view = CodeView::new(text);
        let frame = draw(&window, &mut view);
        let rows = view.rows_in_sight(view.drawn.expect("drawn")).len();
        assert!(rows <= 200, "{rows} rows laid out");
        let drawn = drawn_text(&frame);
        assert!(
            drawn.len() <= 2 * rows + 2,
            "{} pieces of text",
            drawn.len()
        );
        view.scroll_to_line(99_000, ScrollAlign::Top);
        let frame = draw(&window, &mut view);
        assert!(
            drawn_text(&frame).iter().any(|text| text == "line 99000"),
            "scrolled to the line asked for"
        );
    }

    #[test]
    fn test_highlights_take_their_theme_colors_and_overlaps_cut_the_earlier() {
        let red = Rgb::new(255, 0, 0);
        let theme = Theme {
            syntax: crate::base::syntax::SyntaxColors::new().with("keyword", red),
            ..Theme::dark()
        };
        let mut view = CodeView::new("fn main");
        view.set_highlights(vec![
            Highlight::new(3..7, "function"),
            Highlight::new(0..5, "keyword"),
        ]);
        assert_eq!(view.highlights[0].range, 0..3, "cut at the next start");
        let styled = view.styled(view.row(0), &theme);
        let spans = styled.spans();
        assert_eq!(spans[0].text, "fn ");
        assert_eq!(spans[0].color, TextColor::Rgba(Rgba::from(red)));
        assert_eq!(spans[1].text, "main");
        assert_eq!(
            spans[1].color,
            TextColor::Token(ThemeToken::Foreground),
            "a name the theme does not color is plain"
        );
    }

    #[test]
    fn test_tabs_reach_the_next_stop_and_rows_break_before_a_wide_character() {
        assert_eq!(cells_of("\tx"), TAB_WIDTH + 1);
        assert_eq!(cells_of("ab\tx"), TAB_WIDTH + 1);
        assert_eq!(row_breaks("abcdef", 4), [0, 4]);
        assert_eq!(row_breaks("abc漢x", 4), [0, 3], "the wide one starts a row");
        assert_eq!(row_breaks("", 4), [0], "an empty line is one row");
    }

    #[test]
    fn test_wrapping_counts_rows_and_maps_offsets_onto_them() {
        let window = TestWindow::new(400, 200);
        let long = "x".repeat(100);
        let mut view = CodeView::new(format!("short\n{long}\nend"));
        view.set_gutter(false);
        view.set_wrap(true);
        draw(&window, &mut view);
        let cols = view.rows.as_ref().expect("counted").cols;
        let wrapped = 100usize.div_ceil(cols);
        assert_eq!(view.row_count(), 2 + wrapped);
        assert_eq!(
            view.row(1 + wrapped).line,
            2,
            "the last line after the wrapped ones"
        );
        let offset = 6 + cols + 1;
        assert_eq!(view.row_of(offset), 2, "on the long line's second row");
        view.set_wrap(false);
        assert_eq!(view.row_count(), 3);
    }

    #[test]
    fn test_find_selects_each_match_in_turn_and_wraps() {
        let mut view = CodeView::new("one two one\nthree ONE");
        view.set_find("one", false);
        assert_eq!(view.match_count(), 3);
        assert_eq!(view.selected_text(), Some("one"));
        assert_eq!(view.selection(), Some(0..3));
        view.next_match();
        view.next_match();
        assert_eq!(view.selection(), Some(18..21), "either case");
        view.next_match();
        assert_eq!(view.current_match(), Some(0), "after the last, the first");
        view.set_find("one", true);
        assert_eq!(view.match_count(), 2);
    }

    #[test]
    fn test_shift_arrows_select_and_copy_takes_the_lines_as_they_are() {
        let mut window = TestWindow::new(400, 200);
        let mut view = CodeView::new(SOURCE);
        view.set_focused(true);
        draw(&window, &mut view);
        assert_eq!(
            window.feed(&mut view, &key("S-<down>")),
            CodeViewResponse::Moved
        );
        assert_eq!(
            window.feed(&mut view, &key("S-<right>")),
            CodeViewResponse::Moved
        );
        assert_eq!(view.selected_text(), Some("fn main() {\n\t"));
        assert_eq!(
            window.feed(&mut view, &key("C-c")),
            CodeViewResponse::Copied
        );
        assert_eq!(
            window
                .clipboard()
                .read_text(ClipboardKind::Clipboard)
                .as_deref(),
            Some("fn main() {\n\t")
        );
        assert_eq!(
            window.feed(&mut view, &key("<esc>")),
            CodeViewResponse::Moved
        );
        assert_eq!(view.selection(), None);
        assert_eq!(
            window.feed(&mut view, &key("C-c")),
            CodeViewResponse::Ignored
        );
    }

    #[test]
    fn test_a_press_places_the_cursor_and_a_drag_selects_past_a_tab() {
        let mut window = TestWindow::new(400, 200);
        let mut view = CodeView::new(SOURCE);
        draw(&window, &mut view);
        let press = ButtonInput::press(PointerButton::Left, cell(&view, 1, TAB_WIDTH));
        assert_eq!(
            window.feed(&mut view, &Input::PointerButton(press)),
            CodeViewResponse::Moved
        );
        assert_eq!(
            view.cursor(),
            13,
            "just past the tab, which takes four cells"
        );
        let to = cell(&view, 1, TAB_WIDTH + 3);
        window.feed(&mut view, &Input::PointerMoved(to));
        let release = ButtonInput::release(PointerButton::Left, to);
        window.feed(&mut view, &Input::PointerButton(release));
        assert_eq!(view.selected_text(), Some("let"));
    }

    #[test]
    fn test_the_gutter_numbers_lines_and_shows_their_marks() {
        let window = TestWindow::new(400, 200);
        let mut view = CodeView::new(SOURCE);
        view.set_marks([(
            1,
            GutterMark {
                color: ThemeToken::Accent,
                glyph: '+',
            },
        )]);
        let text = drawn_text(&draw(&window, &mut view));
        assert!(text.iter().any(|piece| piece == " 1"), "{text:?}");
        assert!(text.iter().any(|piece| piece == "+2"), "{text:?}");
        assert!(
            text.iter().any(|piece| piece == "    let x = 1;"),
            "the tab drawn as spaces"
        );
        view.set_gutter(false);
        let text = drawn_text(&draw(&window, &mut view));
        assert!(!text.iter().any(|piece| piece == " 1"));
    }

    #[test]
    fn test_screen_readers_get_the_lines_in_sight() {
        let window = TestWindow::new(400, 200);
        let mut view = CodeView::new("alpha\nbeta");
        view.set_selection(0..5);
        let mut frame = draw(&window, &mut view);
        view.access(&mut frame, FocusId::new("code"));
        let text = ui_text(&frame, "Code");
        assert!(
            text.contains(r#"MultilineTextInput value="alpha\nbeta""#),
            "{text}"
        );
    }

    #[test]
    fn test_the_selection_and_the_current_line_are_filled() {
        let theme = Theme::dark();
        let mut view = CodeView::new("alpha\nbeta\ngamma");
        view.set_gutter(false);
        view.set_selection(6..10);
        let Some(snapshot) = offscreen::render(300, 120, |frame| {
            frame.clear(Rgba::from(theme.background));
            let bounds = frame.bounds();
            view.draw(frame, bounds, &theme);
        }) else {
            return;
        };
        let drawn = view.drawn.expect("drawn");
        let metrics = drawn.metrics;
        let row_y = |row: f32| drawn.text.y + (row + 0.5) * metrics.height;
        let past_text = drawn.text.x + 8.0 * metrics.width;
        snapshot.assert_pixel(past_text, row_y(1.0), theme.hover, "the cursor's line");
        snapshot.assert_pixel(past_text, row_y(2.0), theme.background, "another line");
        assert!(
            snapshot.contains_color(
                Rect::new(drawn.text.x, row_y(1.0) - 2.0, 4.0 * metrics.width, 4.0),
                theme.selection
            ),
            "the selection"
        );
    }
}
