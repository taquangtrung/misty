//! [`HexView`]: a read-only hex dump of bytes, for a file preview or a
//! binary inspector: an offset column, the bytes in hex (16 a row unless
//! the app says otherwise, a wider gap after every 8), and the same bytes
//! as text, a dot for each that is not printable ASCII.
//!
//! The app keeps the bytes and lends them for each call through
//! [`HexView::lend`], as a byte slice or as any [`ByteSource`], such as a
//! file read where it is drawn, so a file larger than memory scrolls as a
//! small one does: a frame reads only the rows in sight.
//!
//! The cursor and the selection show in both columns at once. Keys, while
//! the app says the view is focused: the arrows move the cursor a byte or
//! a row, PageUp and PageDown a screen, Home and End to the ends of the
//! row, Ctrl+Home and Ctrl+End to the ends of the bytes; with Shift held
//! they extend the selection. The primary modifier (Ctrl, or Cmd on macOS)
//! with C copies the selection as hex (`de ad be ef`), and with Shift and
//! C as text; Esc drops the selection. The pointer: a press on a byte in
//! either column puts the cursor there, a drag selects, and the wheel and
//! the scrollbar scroll.

use std::fmt::Write;
use std::ops::Range;
use std::time::Instant;

use accesskit::{Node, Role};

use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::clock;
use super::super::base::color::Rgba;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::platform::clipboard::ClipboardKind;
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::scroll_view::ScrollView;

// ========================================================================
// Constants
// ========================================================================

/// How many bytes a row shows unless the app says otherwise.
const DEFAULT_BYTES_PER_ROW: usize = 16;

/// After how many bytes a wider gap falls unless the app says otherwise.
const DEFAULT_GROUP: usize = 8;

/// The fewest hex digits of an offset.
const MIN_OFFSET_DIGITS: usize = 8;

/// The cells between the offsets and the bytes, and between the bytes and
/// the text.
const COLUMN_GAP_CELLS: usize = 2;

/// The cells a byte takes in hex, with the blank after it.
const HEX_CELLS: usize = 3;

/// What stands for a byte that is not printable ASCII in the text column.
const UNPRINTABLE: char = '.';

/// The thickness of the cursor's frame, in logical pixels.
const CURSOR_BORDER: f32 = 1.0;

/// Bits per hex digit.
const HEX_DIGIT_BITS: u32 = 4;

// ========================================================================
// Data Structures
// ========================================================================

/// A read-only hex dump of bytes the app lends it. The app owns it, hands
/// it the window's pointer events (and its keys while it has the focus)
/// through [`HexView::lend`], and draws it each frame the same way.
#[derive(Clone, Debug)]
pub struct HexView {
    /// Where a selection started, or `None` without one: the selection runs
    /// from here to the cursor, both included.
    anchor: Option<u64>,
    /// How many bytes a row shows.
    bytes_per_row: usize,
    /// The byte under the cursor.
    cursor: u64,
    /// Whether a left press is held, selecting as it moves.
    dragging: bool,
    /// Where it was drawn last, or `None` before the first frame.
    drawn: Option<Drawn>,
    /// Whether the app gave the view the keyboard.
    focused: bool,
    /// After how many bytes a wider gap falls.
    group: usize,
    /// How many bytes were lent last.
    len: u64,
    /// Whether the cursor is to be scrolled into sight on the next frame.
    reveal: bool,
    /// The scroll of the rows.
    scroll: ScrollView,
    /// The bytes of the rows in sight, read for the last frame.
    shown: Vec<u8>,
}

/// What became of an input offered to a [`HexView`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HexViewResponse {
    /// The selection was copied to the clipboard.
    Copied,
    /// The view used the input: it scrolled, or a drag goes on.
    Handled,
    /// The input is not the view's.
    Ignored,
    /// The cursor moved or the selection changed.
    Moved,
}

/// Where the view was drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn {
    /// The rows' area, beside the scrollbar.
    area: Rect,
    /// The size of a cell.
    metrics: CellMetrics,
    /// How many hex digits an offset takes.
    offset_digits: usize,
    /// How far down the rows are scrolled, in pixels.
    scrolled: f32,
}

// ========================================================================
// Traits
// ========================================================================

/// Bytes a [`HexView`] shows, read a range at a time: a byte slice, or a
/// file the app reads where it is asked (with
/// `std::os::unix::fs::FileExt::read_at` or a memory map).
pub trait ByteSource {
    /// How many bytes there are.
    fn byte_len(&self) -> u64;

    /// Fills `buf` with the bytes from `offset` on, returning how many it
    /// read: fewer only at the end, or when the bytes cannot be read.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> usize;
}

// ========================================================================
// HexView Implementations
// ========================================================================

impl HexView {
    /// A view of 16 bytes a row, grouped by 8, the cursor at the first.
    pub fn new() -> Self {
        Self {
            anchor: None,
            bytes_per_row: DEFAULT_BYTES_PER_ROW,
            cursor: 0,
            dragging: false,
            drawn: None,
            focused: false,
            group: DEFAULT_GROUP,
            len: 0,
            reveal: false,
            scroll: ScrollView::new(),
            shown: Vec::new(),
        }
    }

    /// This view showing `bytes_per_row` bytes a row (at least one), with
    /// a wider gap after every `group` (none for 0).
    pub fn with_layout(self, bytes_per_row: usize, group: usize) -> Self {
        Self {
            bytes_per_row: bytes_per_row.max(1),
            group,
            ..self
        }
    }

    /// The view lent `bytes`, as a [`Widget`] to hand inputs and draw.
    pub fn lend<'a, S: ByteSource + ?Sized>(&'a mut self, bytes: &'a S) -> Lent<'a, Self, &'a S> {
        Lent::new(self, bytes)
    }

    /// The offset of the byte under the cursor.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// The selected bytes, cursor included, or `None` with nothing
    /// selected.
    pub fn selection(&self) -> Option<Range<u64>> {
        let anchor = self.anchor?;
        Some(anchor.min(self.cursor)..anchor.max(self.cursor) + 1)
    }

    /// Puts the cursor on the byte at `offset` (held to the last byte lent),
    /// drops the selection, and scrolls it into sight on the next frame.
    pub fn go_to(&mut self, offset: u64) {
        self.anchor = None;
        self.cursor = offset.min(self.len.saturating_sub(1));
        self.reveal = true;
    }

    /// Selects the bytes `range`, the cursor on its last, scrolled into
    /// sight on the next frame.
    pub fn set_selection(&mut self, range: Range<u64>) {
        if range.is_empty() {
            return self.go_to(range.start);
        }
        self.anchor = Some(range.start);
        self.cursor = range.end - 1;
        self.reveal = true;
    }

    /// The selection read from `bytes` as hex, a blank between bytes; the
    /// byte under the cursor without a selection.
    pub fn selected_hex(&self, bytes: &(impl ByteSource + ?Sized)) -> String {
        let read = self.read_selection(bytes);
        let mut hex = String::with_capacity(read.len() * HEX_CELLS);
        for (index, byte) in read.iter().enumerate() {
            if index > 0 {
                hex.push(' ');
            }
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }

    /// The selection read from `bytes` as text, bytes that are not UTF-8
    /// replaced; the byte under the cursor without a selection.
    pub fn selected_text(&self, bytes: &(impl ByteSource + ?Sized)) -> String {
        String::from_utf8_lossy(&self.read_selection(bytes)).into_owned()
    }

    /// Describes the view as last drawn to screen readers as `id`: read-only
    /// text holding the rows in sight as drawn, offsets, hex, and text.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(drawn) = self.drawn else {
            return;
        };
        let first = self.first_row_in_sight(drawn);
        let rows: Vec<String> = self
            .shown
            .chunks(self.bytes_per_row)
            .enumerate()
            .map(|(index, row)| {
                let offset = (first + index as u64) * self.bytes_per_row as u64;
                self.row_text(offset, row, drawn.offset_digits)
            })
            .collect();
        let mut node = Node::new(Role::MultilineTextInput);
        node.set_read_only();
        node.set_value(rows.join("\n"));
        frame.access(id, drawn.area, node);
    }

    /// The bytes of the selection (or the cursor's byte) read from `bytes`.
    fn read_selection(&self, bytes: &(impl ByteSource + ?Sized)) -> Vec<u8> {
        let range = self.selection().unwrap_or(self.cursor..self.cursor + 1);
        let end = range.end.min(bytes.byte_len());
        let mut read = vec![0; end.saturating_sub(range.start) as usize];
        let count = bytes.read_at(range.start, &mut read);
        read.truncate(count);
        read
    }

    // --------------------------------------------------------------------
    // Layout
    // --------------------------------------------------------------------

    /// The cell of the hex of byte `index` of a row, from the start of the
    /// hex column.
    fn hex_cell(&self, index: usize) -> usize {
        let gaps = match self.group {
            0 => 0,
            group => index / group,
        };
        index * HEX_CELLS + gaps
    }

    /// The cells of the hex column: up to the end of its last byte.
    fn hex_cells(&self) -> usize {
        self.hex_cell(self.bytes_per_row - 1) + HEX_CELLS - 1
    }

    /// The first cell of the hex column.
    fn hex_start(&self, offset_digits: usize) -> usize {
        offset_digits + COLUMN_GAP_CELLS
    }

    /// The first cell of the text column.
    fn text_start(&self, offset_digits: usize) -> usize {
        self.hex_start(offset_digits) + self.hex_cells() + COLUMN_GAP_CELLS
    }

    /// The rows the bytes take.
    fn row_count(&self) -> u64 {
        self.len.div_ceil(self.bytes_per_row as u64).max(1)
    }

    /// The first row in sight in `drawn`.
    fn first_row_in_sight(&self, drawn: Drawn) -> u64 {
        (drawn.scrolled / drawn.metrics.height).floor().max(0.0) as u64
    }

    /// Row `row` of `bytes` (at `offset`) as drawn: its offset, its hex,
    /// and its text.
    fn row_text(&self, offset: u64, bytes: &[u8], offset_digits: usize) -> String {
        let mut row = format!("{offset:0offset_digits$x}");
        let hex_start = self.hex_start(offset_digits);
        for (index, byte) in bytes.iter().enumerate() {
            pad_to(&mut row, hex_start + self.hex_cell(index));
            let _ = write!(row, "{byte:02x}");
        }
        pad_to(&mut row, self.text_start(offset_digits));
        row.extend(bytes.iter().map(|&byte| printable(byte)));
        row
    }

    /// The byte under `point` in either column as last drawn.
    fn byte_at(&self, point: Point) -> Option<u64> {
        let drawn = self.drawn?;
        let metrics = drawn.metrics;
        let row = ((point.y - drawn.area.y + drawn.scrolled) / metrics.height).floor();
        let cell = ((point.x - drawn.area.x) / metrics.width).floor();
        if row < 0.0 || cell < 0.0 {
            return None;
        }
        let cell = cell as usize;
        let hex_start = self.hex_start(drawn.offset_digits);
        let text_start = self.text_start(drawn.offset_digits);
        let index = if (hex_start..hex_start + self.hex_cells()).contains(&cell) {
            (0..self.bytes_per_row)
                .rev()
                .find(|&index| hex_start + self.hex_cell(index) <= cell)?
        } else if (text_start..text_start + self.bytes_per_row).contains(&cell) {
            cell - text_start
        } else {
            return None;
        };
        let offset = row as u64 * self.bytes_per_row as u64 + index as u64;
        Some(offset.min(self.len.saturating_sub(1)))
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// Reacts to a key while focused, as the module documentation lists.
    fn handle_key(
        &mut self,
        key: Key,
        bytes: &(impl ByteSource + ?Sized),
        cx: &mut EventCx<'_>,
    ) -> HexViewResponse {
        if !self.focused || self.len == 0 {
            return HexViewResponse::Ignored;
        }
        let row = self.bytes_per_row as u64;
        let page = self
            .drawn
            .map_or(1, |drawn| drawn.metrics.rows_in(drawn.area.height).max(1))
            as u64
            * row;
        let last = self.len - 1;
        let at = self.cursor;
        let extend = key.mods.contains(Mods::SHIFT);
        let mods = key.mods.without(Mods::SHIFT);
        let target = match (key.code, mods) {
            (KeyCode::Char('c') | KeyCode::Char('C'), m) if m == Mods::PRIMARY => {
                let text = match extend {
                    true => self.selected_text(bytes),
                    false => self.selected_hex(bytes),
                };
                // Kept in the process when the system refuses it.
                let _ = cx.clipboard().write_text(ClipboardKind::Clipboard, &text);
                return HexViewResponse::Copied;
            }
            (KeyCode::Esc, Mods::NONE) if self.anchor.is_some() && !extend => {
                self.anchor = None;
                return HexViewResponse::Moved;
            }
            (KeyCode::Left, Mods::NONE) => at.saturating_sub(1),
            (KeyCode::Right, Mods::NONE) => (at + 1).min(last),
            (KeyCode::Up, Mods::NONE) => at.checked_sub(row).unwrap_or(at),
            (KeyCode::Down, Mods::NONE) => match at + row <= last {
                true => at + row,
                false => at,
            },
            (KeyCode::PageUp, Mods::NONE) => at.saturating_sub(page),
            (KeyCode::PageDown, Mods::NONE) => (at + page).min(last),
            (KeyCode::Home, Mods::NONE) => at - at % row,
            (KeyCode::End, Mods::NONE) => (at - at % row + row - 1).min(last),
            (KeyCode::Home, Mods::CTRL) => 0,
            (KeyCode::End, Mods::CTRL) => last,
            (
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
                | KeyCode::Up,
                _,
            ) => return HexViewResponse::Ignored,
        };
        match extend {
            true => {
                self.anchor.get_or_insert(at);
            }
            false => self.anchor = None,
        }
        self.cursor = target;
        self.reveal = true;
        HexViewResponse::Moved
    }

    /// Reacts to the pointer: a press on a byte puts the cursor there, a
    /// drag selects, and the rest goes to the scroll.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> HexViewResponse {
        match input {
            Input::PointerButton(button)
                if button.button == PointerButton::Left && button.pressed =>
            {
                let Some(offset) = self.byte_at(button.position) else {
                    return self.scroll_input(input, cx);
                };
                match button.mods == Mods::SHIFT {
                    true => {
                        self.anchor.get_or_insert(self.cursor);
                    }
                    false => self.anchor = None,
                }
                self.cursor = offset;
                self.dragging = true;
                HexViewResponse::Moved
            }
            Input::PointerButton(button)
                if button.button == PointerButton::Left && !button.pressed && self.dragging =>
            {
                self.dragging = false;
                HexViewResponse::Handled
            }
            Input::PointerMoved(point) if self.dragging => {
                let Some(offset) = self.byte_at(*point) else {
                    return HexViewResponse::Handled;
                };
                if offset == self.cursor {
                    return HexViewResponse::Handled;
                }
                self.anchor.get_or_insert(self.cursor);
                self.cursor = offset;
                self.reveal = true;
                HexViewResponse::Moved
            }
            input => self.scroll_input(input, cx),
        }
    }

    /// Scrolls the least that shows the cursor's row, at once, in a view
    /// `view` pixels tall of rows `row_height` pixels tall.
    fn reveal_cursor(&mut self, view: f32, row_height: f32) {
        let top = (self.cursor / self.bytes_per_row as u64) as f32 * row_height;
        let offset = self.scroll.offset();
        let y = if top < offset.y {
            top
        } else if top + row_height > offset.y + view {
            top + row_height - view
        } else {
            return;
        };
        self.scroll.scroll_to(Point::new(offset.x, y));
    }

    /// Hands `input` to the scroll.
    fn scroll_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> HexViewResponse {
        match self.scroll.handle(input, cx).is_ignored() {
            true => HexViewResponse::Ignored,
            false => HexViewResponse::Handled,
        }
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Draws the rows in sight of `bytes` over `area` in `theme`'s colors.
    fn draw_at(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        bytes: &(impl ByteSource + ?Sized),
        theme: &Theme,
    ) {
        self.len = bytes.byte_len();
        self.cursor = self.cursor.min(self.len.saturating_sub(1));
        let metrics = frame.cell_metrics();
        let offset_digits = offset_digits(self.len);
        let width = (self.text_start(offset_digits) + self.bytes_per_row) as f32 * metrics.width;
        let height = self.row_count() as f32 * metrics.height;
        self.scroll.set_content_size(width, height);
        let viewport = self.scroll.draw_at(frame, area, theme, clock::now());
        if self.reveal {
            self.reveal = false;
            self.reveal_cursor(viewport.height, metrics.height);
        }
        let drawn = Drawn {
            area: viewport,
            metrics,
            offset_digits,
            scrolled: self.scroll.offset().y,
        };
        self.drawn = Some(drawn);
        let first = self.first_row_in_sight(drawn);
        let rows = metrics.rows_in(viewport.height) as u64 + 2;
        let start = first * self.bytes_per_row as u64;
        let end = ((first + rows) * self.bytes_per_row as u64).min(self.len);
        self.shown.resize(end.saturating_sub(start) as usize, 0);
        let read = bytes.read_at(start, &mut self.shown);
        self.shown.truncate(read);
        let left = viewport.x - self.scroll.offset().x;
        frame.clipped(viewport, |frame| {
            for (index, row) in self.shown.chunks(self.bytes_per_row).enumerate() {
                let row_index = first + index as u64;
                let y = viewport.y + row_index as f32 * metrics.height - drawn.scrolled;
                let offset = row_index * self.bytes_per_row as u64;
                self.draw_marks(frame, offset, row.len(), Point::new(left, y), drawn, theme);
                self.draw_row(frame, offset, row, Point::new(left, y), drawn, theme);
            }
        });
    }

    /// Fills the selection and frames the cursor on the row of `count`
    /// bytes at `offset`, drawn from `origin`, in both columns.
    fn draw_marks(
        &self,
        frame: &mut Frame,
        offset: u64,
        count: usize,
        origin: Point,
        drawn: Drawn,
        theme: &Theme,
    ) {
        let metrics = drawn.metrics;
        let scale_factor = frame.scale_factor();
        let hex_start = self.hex_start(drawn.offset_digits);
        let text_start = self.text_start(drawn.offset_digits);
        let cell_rect = |cell: usize, cells: usize| {
            Rect::new(
                origin.x + cell as f32 * metrics.width,
                origin.y,
                cells as f32 * metrics.width,
                metrics.height,
            )
        };
        let selection = self.selection();
        for index in 0..count {
            let byte = offset + index as u64;
            let hex = cell_rect(hex_start + self.hex_cell(index), 2);
            let text = cell_rect(text_start + index, 1);
            if selection
                .as_ref()
                .is_some_and(|range| range.contains(&byte))
            {
                frame.fill_rect(hex, theme.selection);
                frame.fill_rect(text, theme.selection);
            }
            if byte == self.cursor {
                let style = BoxStyle {
                    border: Some(Border {
                        color: Rgba::from(theme.accent),
                        width: logical(CURSOR_BORDER, scale_factor),
                    }),
                    fill: theme.accent.with_alpha(0),
                    ..theme.shape.highlight(theme.accent, scale_factor)
                };
                frame.draw_box(hex, &style);
                frame.draw_box(text, &style);
            }
        }
    }

    /// Draws the row of `bytes` at `offset` from `origin`, cut to the area
    /// of `drawn`: the offset muted, the hex and the text in the
    /// foreground.
    fn draw_row(
        &self,
        frame: &mut Frame,
        offset: u64,
        bytes: &[u8],
        origin: Point,
        drawn: Drawn,
        theme: &Theme,
    ) {
        let text = self.row_text(offset, bytes, drawn.offset_digits);
        let (offset_part, rest) = text.split_at(drawn.offset_digits);
        let styled = StyledText::plain(offset_part, TextColor::Token(ThemeToken::Muted))
            .with(Span::new(rest, TextColor::Token(ThemeToken::Foreground)));
        frame.draw_text(&styled, origin, drawn.area, theme);
    }
}

impl Default for HexView {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: ByteSource + ?Sized> Widget for Lent<'_, HexView, &S> {
    type Output = ();
    type Response = HexViewResponse;

    /// Reacts to `input`: keys as the module documentation lists while
    /// focused, the pointer placing the cursor and selecting, and the rest
    /// scrolling. Asks for a redraw whenever it used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> HexViewResponse {
        let response = match input {
            Input::Key(key) => match self.widget.handle_key(*key, self.model, cx) {
                HexViewResponse::Ignored => self.widget.scroll_input(input, cx),
                response => response,
            },
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
            | Input::Wheel(_) => self.widget.handle_pointer(input, cx),
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the rows in sight over `area` in `theme`'s colors: the
    /// offsets muted, the selection filled and the cursor framed in the
    /// accent in both columns.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.widget.draw_at(frame, area, self.model, theme);
    }

    /// Tells the view whether it has the keyboard; it takes keys only
    /// while it does.
    fn set_focused(&mut self, focused: bool) {
        self.widget.focused = focused;
        self.widget.scroll.set_focused(focused);
    }

    /// Whether the app last said the view has the keyboard.
    fn is_focused(&self) -> bool {
        self.widget.focused
    }

    /// Asks for the next frame of a scroll glide under way, returning when
    /// it is due; `None` at rest.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.widget.scroll.about_to_wait(cx)
    }
}

impl Outcome for HexViewResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// ByteSource Implementations
// ========================================================================

impl ByteSource for [u8] {
    fn byte_len(&self) -> u64 {
        self.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> usize {
        let start = (offset as usize).min(self.len());
        let count = buf.len().min(self.len() - start);
        buf[..count].copy_from_slice(&self[start..start + count]);
        count
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How many hex digits the offsets of `len` bytes take: at least 8.
fn offset_digits(len: u64) -> usize {
    let bits = u64::BITS - len.saturating_sub(1).leading_zeros();
    (bits.div_ceil(HEX_DIGIT_BITS) as usize).max(MIN_OFFSET_DIGITS)
}

/// `byte` as the text column shows it: itself when printable ASCII, else
/// a dot.
fn printable(byte: u8) -> char {
    match byte.is_ascii_graphic() || byte == b' ' {
        true => char::from(byte),
        false => UNPRINTABLE,
    }
}

/// Pads `row` with blanks to `cells` characters.
fn pad_to(row: &mut String, cells: usize) {
    let len = row.chars().count();
    row.extend(std::iter::repeat_n(' ', cells.saturating_sub(len)));
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::app::offscreen;
    use crate::base::key::parse_key;
    use crate::input::event::ButtonInput;
    use crate::testing::TestWindow;

    const BYTES: &[u8] = b"Hello, misty!\x00\x01\xff and more bytes";

    /// A source of `len` zero bytes that remembers the most it was asked
    /// for at once.
    struct Lazy {
        len: u64,
        most: Cell<usize>,
    }

    impl ByteSource for Lazy {
        fn byte_len(&self) -> u64 {
            self.len
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> usize {
            self.most.set(self.most.get().max(buf.len()));
            let count = buf.len().min(self.len.saturating_sub(offset) as usize);
            buf[..count].fill(0);
            count
        }
    }

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    fn draw(window: &TestWindow, view: &mut HexView, bytes: &(impl ByteSource + ?Sized)) {
        let mut frame = window.frame();
        let bounds = frame.bounds();
        view.lend(bytes).draw(&mut frame, bounds, &Theme::dark());
    }

    /// The middle of cell `cell` of row `row` as last drawn.
    fn cell(view: &HexView, row: usize, cell: usize) -> Point {
        let drawn = view.drawn.expect("drawn");
        Point::new(
            drawn.area.x + (cell as f32 + 0.5) * drawn.metrics.width,
            drawn.area.y + (row as f32 + 0.5) * drawn.metrics.height,
        )
    }

    #[test]
    fn test_a_row_shows_its_offset_grouped_hex_and_dotted_text() {
        let view = HexView::new();
        let row = view.row_text(0x10, &BYTES[..16], MIN_OFFSET_DIGITS);
        assert_eq!(
            row,
            "00000010  48 65 6c 6c 6f 2c 20 6d  69 73 74 79 21 00 01 ff  Hello, misty!..."
        );
        assert_eq!(offset_digits(16), MIN_OFFSET_DIGITS);
        assert_eq!(offset_digits(1 << 36), 9, "past 4 GiB, a ninth digit");
    }

    #[test]
    fn test_a_press_in_either_column_lands_on_the_same_byte() {
        let mut window = TestWindow::new(900, 200);
        let mut view = HexView::new();
        draw(&window, &mut view, BYTES);
        let hex = view.hex_start(MIN_OFFSET_DIGITS) + view.hex_cell(9);
        let press = ButtonInput::press(PointerButton::Left, cell(&view, 0, hex + 1));
        window.feed(&mut view.lend(BYTES), &Input::PointerButton(press));
        assert_eq!(view.cursor(), 9);
        let text = view.text_start(MIN_OFFSET_DIGITS) + 3;
        let press = ButtonInput::press(PointerButton::Left, cell(&view, 1, text));
        window.feed(&mut view.lend(BYTES), &Input::PointerButton(press));
        assert_eq!(view.cursor(), 19);
    }

    #[test]
    fn test_shift_moves_select_and_copy_gives_hex_or_text() {
        let mut window = TestWindow::new(900, 200);
        let mut view = HexView::new();
        view.lend(BYTES).set_focused(true);
        draw(&window, &mut view, BYTES);
        for _ in 0..4 {
            window.feed(&mut view.lend(BYTES), &key("S-<right>"));
        }
        assert_eq!(view.selection(), Some(0..5));
        assert_eq!(
            window.feed(&mut view.lend(BYTES), &key("C-c")),
            HexViewResponse::Copied
        );
        let read = |window: &mut TestWindow| window.clipboard().read_text(ClipboardKind::Clipboard);
        assert_eq!(read(&mut window).as_deref(), Some("48 65 6c 6c 6f"));
        window.feed(&mut view.lend(BYTES), &key("C-S-c"));
        assert_eq!(read(&mut window).as_deref(), Some("Hello"));
        window.feed(&mut view.lend(BYTES), &key("<down>"));
        assert_eq!(
            (view.cursor(), view.selection()),
            (20, None),
            "a row down, unselected"
        );
        window.feed(&mut view.lend(BYTES), &key("C-<end>"));
        assert_eq!(view.cursor(), BYTES.len() as u64 - 1);
    }

    #[test]
    fn test_going_to_an_offset_scrolls_there_and_reads_only_the_rows_in_sight() {
        let window = TestWindow::new(900, 200);
        let lazy = Lazy {
            len: 1 << 30,
            most: Cell::new(0),
        };
        let mut view = HexView::new();
        draw(&window, &mut view, &lazy);
        view.go_to(0x0123_4567);
        draw(&window, &mut view, &lazy);
        let drawn = view.drawn.expect("drawn");
        let first = view.first_row_in_sight(drawn) * DEFAULT_BYTES_PER_ROW as u64;
        let rows = drawn.metrics.rows_in(drawn.area.height) as u64;
        let last = first + rows * DEFAULT_BYTES_PER_ROW as u64;
        assert!(
            (first..last).contains(&0x0123_4567),
            "{first:#x}..{last:#x}"
        );
        assert!(
            lazy.most.get() < 4096,
            "read {} bytes at once",
            lazy.most.get()
        );
    }

    #[test]
    fn test_the_selection_fills_both_columns() {
        let theme = Theme::dark();
        let mut view = HexView::new();
        view.set_selection(0..2);
        let Some(snapshot) = offscreen::render(900, 60, |frame| {
            frame.clear(Rgba::from(theme.background));
            let bounds = frame.bounds();
            view.lend(BYTES).draw(frame, bounds, &theme);
        }) else {
            return;
        };
        let drawn = view.drawn.expect("drawn");
        let metrics = drawn.metrics;
        let at = |cell: usize| {
            Rect::new(
                drawn.area.x + cell as f32 * metrics.width,
                drawn.area.y,
                metrics.width,
                metrics.height,
            )
        };
        let hex = view.hex_start(MIN_OFFSET_DIGITS);
        let text = view.text_start(MIN_OFFSET_DIGITS);
        assert!(snapshot.contains_color(at(hex), theme.selection), "the hex");
        assert!(
            snapshot.contains_color(at(text), theme.selection),
            "the text"
        );
        assert!(
            !snapshot.contains_color(at(text + 3), theme.selection),
            "past the selection"
        );
    }
}
