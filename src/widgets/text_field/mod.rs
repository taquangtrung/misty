//! [`TextField`]: a one-line text input for a rename box, an address bar,
//! a picker's query, or a setting's text. It keeps its text, a caret, and
//! a selection, and answers the keys every text field does:
//!
//! - typing, Backspace and Delete, over the selection when there is one;
//! - Left, Right, Home, and End, Ctrl+Left and Ctrl+Right by words, each
//!   extending the selection with Shift;
//! - the Emacs and readline keys `C-a`, `C-e`, `C-b`, `C-f`, `C-d`, `C-h`,
//!   `C-k`, `C-u`, `C-w`, `C-y`, `M-b`, `M-f`, and `M-d`, kills kept for
//!   `C-y` apart from the clipboard;
//! - copy, cut, paste, undo, and redo on the primary modifier (Ctrl, or
//!   Cmd on macOS) with C, X, V, Z, and Shift+Z, plus Ctrl+Insert and
//!   Shift+Insert, through a [`SystemClipboard`]; Cmd+A selects all on
//!   macOS, where it is free (elsewhere Ctrl+A is the line start).
//!
//! Enter, Esc, Tab, the vertical keys, and chords it does not know are
//! [`TextFieldResponse::Ignored`], for the app to submit, cancel, or move
//! the focus with. The pointer places the caret, a drag selects, a double
//! click selects a word and a triple click everything. On X11 and Wayland
//! the selected text goes to the primary selection, and a middle click
//! pastes the primary selection where it lands. Text an input
//! method composes shows underlined at the caret. Text longer than the
//! field scrolls sideways to keep the caret in sight. An empty field shows
//! its placeholder. The field draws its caret only while the app says it
//! is focused, and with a frame its border turns the accent color then.
//!
//! Right-to-left text (Hebrew, Arabic) shows in its visual order, but the
//! caret, the selection, and the arrow keys still follow the text's
//! logical order cell by cell, so editing it is only fit for short runs
//! inside left-to-right text for now.

pub(super) mod line;

use std::ops::Range;
use std::time::Instant;

use accesskit::{Node, Role, TextPosition, TextSelection};

use super::super::app::access::node_id;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::ime::{Preedit, PreeditChange};
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::color::Rgba;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::list::scroll_to_show;
use super::super::base::motion::Blink;
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::text;
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::click::is_double_click;
use super::super::input::event::{ImeInput, Input, PointerButton};
use super::super::platform::clipboard::{ClipboardKind, HAS_PRIMARY_SELECTION, SystemClipboard};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use crate::base::cursor::CursorIcon;

use line::{Edit, LineEdit};

// ========================================================================
// Constants
// ========================================================================

/// The width of the caret, in logical pixels.
pub(super) const CARET_WIDTH: f32 = 2.0;

/// The thickness of a framed field's border, in logical pixels.
pub(super) const BORDER: f32 = 1.0;

/// The space above and below the text inside a frame, in logical pixels.
pub(super) const PADDING_Y: f32 = 3.0;

/// The blank cells between a frame and the text.
const PADDING_CELLS: f32 = 1.0;

/// The presses that make a triple click, which selects everything.
pub(super) const TRIPLE_CLICK: u8 = 3;

/// The color of the text.
pub(super) const TEXT_COLOR: TextColor = TextColor::Token(ThemeToken::Foreground);

/// The color of the placeholder.
pub(super) const PLACEHOLDER_COLOR: TextColor = TextColor::Token(ThemeToken::Muted);

// ========================================================================
// Data Structures
// ========================================================================

/// A one-line text input. The app owns it, hands it the keys and pointer
/// events of the window while it has the focus, and draws it each frame.
#[derive(Clone, Debug, Default)]
pub struct TextField {
    /// The caret's blink.
    blink: Blink,
    /// The last press and how many quick presses on one spot led to it.
    clicks: Option<Clicks>,
    /// Whether a left press in the field is held, selecting as it moves.
    dragging: bool,
    /// Whether the app gave the field the focus.
    focused: bool,
    /// Whether the field draws its own background and border.
    framed: bool,
    /// Whether the pointer is over the field, as last told the window.
    hovered: bool,
    /// Where the field was drawn last, or `None` before the first frame.
    layout: Option<FieldLayout>,
    /// The text, caret, and selection.
    line: LineEdit,
    /// What an empty field shows.
    placeholder: String,
    /// Where the pointer is, or `None` when it is outside the window.
    pointer: Option<Point>,
    /// Text an input method is composing at the caret.
    preedit: Option<Preedit>,
    /// The selection last written to the primary selection, until the
    /// selection is dropped.
    primary_range: Option<Range<usize>>,
    /// The first cell of the text in sight.
    scroll: usize,
}

/// What became of a key or event offered to a [`TextField`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum TextFieldResponse {
    /// The text changed: read it with [`TextField::text`].
    Changed,
    /// The field used it without changing the text (the caret or the
    /// selection moved, or something was copied).
    Handled,
    /// Not the field's: the app may act on it.
    Ignored,
}

/// The last press on a field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Clicks {
    /// When and where it was.
    at: (Instant, Point),
    /// How many quick presses on one spot it ended: 1, 2, or 3.
    pub(super) count: u8,
}

/// Where a field's text was drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FieldLayout {
    /// The whole field, frame included.
    area: Rect,
    /// The caret's cell.
    caret: Rect,
    /// The window's cell size.
    metrics: CellMetrics,
    /// The window's scale factor.
    scale_factor: f64,
    /// The text's room, where its first cell in sight starts.
    text: Rect,
}

// ========================================================================
// TextField Implementations
// ========================================================================

impl TextField {
    // --------------------------------------------------------------------
    // Building
    // --------------------------------------------------------------------

    /// An empty, unframed field without a placeholder.
    pub fn new() -> Self {
        Self::default()
    }

    /// A field holding `text` (line breaks and other control characters
    /// left out), the caret at its end.
    pub fn with_text(text: &str) -> Self {
        Self {
            line: LineEdit::with_text(text),
            ..Self::default()
        }
    }

    /// This field showing `placeholder` while it is empty.
    pub fn with_placeholder(self, placeholder: impl Into<String>) -> Self {
        Self {
            placeholder: placeholder.into(),
            ..self
        }
    }

    /// This field drawing its own background and a border around the text
    /// (accent while focused), as a form input does; without it, only the
    /// text, selection, and caret are drawn, for a field inside a bar.
    pub fn with_frame(self) -> Self {
        Self {
            framed: true,
            ..self
        }
    }

    /// The height a field takes: one row, plus the frame when it has one.
    pub fn height(&self, metrics: CellMetrics, scale_factor: f64) -> f32 {
        match self.framed {
            true => {
                let frame = logical(BORDER, scale_factor) + logical(PADDING_Y, scale_factor);
                (metrics.height + 2.0 * frame).round()
            }
            false => metrics.height,
        }
    }

    // --------------------------------------------------------------------
    // Text
    // --------------------------------------------------------------------

    /// The text typed so far, without any input method composition.
    pub fn text(&self) -> &str {
        self.line.text()
    }

    /// Replaces the text with `text` (line breaks and other control
    /// characters left out), the caret at its end, nothing selected, undo
    /// forgotten.
    pub fn set_text(&mut self, text: &str) {
        self.line.set_text(text);
        self.preedit = None;
    }

    /// Types `text` at the caret over the selection, as a paste does
    /// (control characters left out, undoable): for text the app pastes
    /// itself, such as a terminal's bracketed paste or a middle click's
    /// primary selection.
    pub fn insert(&mut self, text: &str) -> TextFieldResponse {
        self.preedit = None;
        response_of(self.line.insert(text))
    }

    /// The selected part of the text as a byte range, or `None` when
    /// nothing is selected.
    pub fn selection(&self) -> Option<Range<usize>> {
        self.line.selection()
    }

    /// Selects the whole text, as a rename box does when it opens.
    pub fn select_all(&mut self) {
        self.line.select_all();
    }

    /// Selects the bytes `range` of the text, the caret at its end, as a
    /// rename box selecting a file's name without its extension does.
    /// Ignored when the range is out of the text or off a char boundary.
    pub fn select(&mut self, range: Range<usize>) {
        let text = self.line.text();
        if range.start > range.end
            || !text.is_char_boundary(range.start)
            || !text.is_char_boundary(range.end)
        {
            return;
        }
        self.line.move_to(range.start, false);
        self.line.move_to(range.end, true);
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
    /// [`ImeFocus::follow`](crate::ImeFocus::follow) while the field is
    /// focused; `None` before the first frame.
    pub fn cursor_area(&self) -> Option<Rect> {
        self.layout.map(|layout| layout.caret)
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// Reacts to `key`, copying to and pasting from the context's
    /// clipboard and writing a new selection to its primary selection.
    /// Hand it the keys of the window while the field is focused.
    pub(crate) fn handle_key(&mut self, key: Key, cx: &mut EventCx<'_>) -> TextFieldResponse {
        let clipboard = cx.clipboard();
        let response = self.edit_by_key(key, clipboard);
        self.publish_primary(clipboard);
        response
    }

    /// Carries out `key` as [`TextField::handle_key`] does, but for the
    /// primary selection.
    fn edit_by_key(&mut self, key: Key, clipboard: &mut SystemClipboard) -> TextFieldResponse {
        edit_by_key(&mut self.line, key, clipboard).map_or(TextFieldResponse::Ignored, response_of)
    }

    /// Reacts to an input method event: text being composed is shown
    /// underlined at the caret, and committed text is typed over the
    /// selection. [`TextFieldResponse::Ignored`] when nothing changed.
    pub(crate) fn handle_ime(&mut self, ime: &ImeInput) -> TextFieldResponse {
        match Preedit::apply(&mut self.preedit, ime) {
            PreeditChange::Changed => TextFieldResponse::Handled,
            PreeditChange::Commit(text) => response_of(self.line.insert(&text)),
            PreeditChange::Unchanged => TextFieldResponse::Ignored,
        }
    }

    /// Reacts to pointer `input`: a left press in the field
    /// places the caret (extending the selection with Shift), two quick
    /// presses select a word and three the whole text, and moving with the
    /// button held selects; the pointer is a text beam over the field. On
    /// X11 and Wayland a selection made is written to the clipboard's
    /// primary selection once the button is let go, and a middle press pastes the
    /// primary selection where it lands, focused or not. A press outside
    /// the field is [`TextFieldResponse::Ignored`], for the app to move the
    /// focus.
    pub(crate) fn handle_pointer(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> TextFieldResponse {
        let response = match input {
            Input::PointerButton(button) => {
                self.pointer = Some(button.position);
                match (button.button, button.pressed) {
                    (PointerButton::Left, true) => match self.pointer_point() {
                        Some(point) => {
                            let shift = button.mods.contains(Mods::SHIFT);
                            self.press_at(point, Instant::now(), shift)
                        }
                        None => TextFieldResponse::Ignored,
                    },
                    (PointerButton::Left, false) if self.dragging => {
                        self.dragging = false;
                        TextFieldResponse::Handled
                    }
                    (PointerButton::Middle, true) if HAS_PRIMARY_SELECTION => {
                        match self.pointer_point() {
                            Some(point) => self.paste_primary_at(point, cx.clipboard()),
                            None => TextFieldResponse::Ignored,
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
                    ) => TextFieldResponse::Ignored,
                }
            }
            Input::PointerLeft => {
                self.hover(cx, None);
                TextFieldResponse::Ignored
            }
            Input::PointerMoved(point) => {
                self.hover(cx, Some(*point));
                self.drag_to(*point)
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
            | Input::Wheel(_) => TextFieldResponse::Ignored,
        };
        if !self.dragging {
            self.publish_primary(cx.clipboard());
        }
        if !matches!(response, TextFieldResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Writes the selection to `clipboard`'s primary selection when it is
    /// not the one last written, on platforms that have one; forgets the
    /// last one written when nothing is selected, so selecting it again
    /// writes it again.
    fn publish_primary(&mut self, clipboard: &mut SystemClipboard) {
        publish_primary(&self.line, &mut self.primary_range, clipboard);
    }

    /// A middle press at `point`: types `clipboard`'s primary selection at
    /// the character boundary nearest `point`, leaving any selection.
    fn paste_primary_at(
        &mut self,
        point: Point,
        clipboard: &mut SystemClipboard,
    ) -> TextFieldResponse {
        let Some(text) = clipboard.read_text(ClipboardKind::Primary) else {
            return TextFieldResponse::Handled;
        };
        self.line.move_to(self.offset_at(point), false);
        response_of(self.line.insert(&text))
    }

    /// Follows the pointer to `pointer` (`None` outside the window),
    /// showing a text beam over the field.
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

    /// Where the pointer is over the field as last drawn, or `None` when
    /// it is not.
    fn pointer_point(&self) -> Option<Point> {
        let area = self.layout?.area;
        self.pointer.filter(|&point| area.contains(point))
    }

    /// A left press at `point` at `now`, with Shift held when `shift`.
    fn press_at(&mut self, point: Point, now: Instant, shift: bool) -> TextFieldResponse {
        let Some(layout) = self.layout else {
            return TextFieldResponse::Ignored;
        };
        let clicks = Clicks::after(self.clicks, now, point, layout.scale_factor);
        self.clicks = Some(clicks);
        let count = clicks.count;
        let offset = self.offset_at(point);
        match count {
            1 => {
                self.line.move_to(offset, shift);
                self.dragging = true;
            }
            TRIPLE_CLICK => {
                self.line.select_all();
            }
            _ => {
                self.line.select_word_at(offset);
            }
        }
        TextFieldResponse::Handled
    }

    /// Extends the selection to `point` while a press is held.
    fn drag_to(&mut self, point: Point) -> TextFieldResponse {
        if !self.dragging {
            return TextFieldResponse::Ignored;
        }
        let offset = self.offset_at(point);
        if offset == self.line.caret() {
            return TextFieldResponse::Ignored;
        }
        self.line.move_to(offset, true);
        TextFieldResponse::Handled
    }

    /// The char boundary of the text nearest `point`, as last drawn.
    fn offset_at(&self, point: Point) -> usize {
        let Some(layout) = self.layout else {
            return self.line.caret();
        };
        let cells = (point.x - layout.text.x) / layout.metrics.width.max(1.0);
        self.line.offset_at_cells(cells + self.scroll as f32)
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Describes the field as last drawn to screen readers as `id`: a text
    /// input with its text as value and one text run (`id`'s first
    /// child), its placeholder, and the selection or caret as its text
    /// selection. Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = self.layout else {
            return;
        };
        let text = self.line.text();
        let run_id = id.child(0);
        let position = |offset: usize| TextPosition {
            character_index: text[..offset].chars().count(),
            node: node_id(run_id),
        };
        let mut input = Node::new(Role::TextInput);
        input.set_value(text);
        if !self.placeholder.is_empty() {
            input.set_placeholder(self.placeholder.as_str());
        }
        let caret = self.line.caret();
        input.set_text_selection(TextSelection {
            anchor: position(self.line.anchor().unwrap_or(caret)),
            focus: position(caret),
        });
        frame.enter_access(id, layout.area, input);
        let mut run = Node::new(Role::TextRun);
        run.set_value(text);
        run.set_character_lengths(text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<_>>());
        frame.access(run_id, layout.text, run);
        frame.leave_access();
    }
}

impl Widget for TextField {
    type Output = ();
    type Response = TextFieldResponse;

    /// Reacts to `input`: keys edit the text, as the module documentation
    /// lists, copying to and pasting from the context's clipboard; input
    /// method text is composed at the caret and typed once committed; the
    /// pointer places the caret, selects, and pastes the primary selection
    /// on a middle click. Hand it keys and input method text only while the field has
    /// the keyboard. Asks for a redraw whenever it used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> TextFieldResponse {
        if matches!(
            input,
            Input::Ime(_) | Input::Key(_) | Input::PointerButton(_)
        ) {
            self.blink.reset(Instant::now());
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
        if !matches!(response, TextFieldResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Draws the field in `area` (its text on one row, centered down
    /// `area`) in `theme`'s colors, scrolled sideways to keep the caret in
    /// sight.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let room = match self.framed {
            true => draw_frame(frame, area, self.focused, theme),
            false => area,
        };
        let y = (room.y + (room.height - metrics.height) / 2.0).round();
        let room = Rect::new(room.x, y, room.width, metrics.height);
        let caret = self.line.caret();
        let caret_cells = self.line.cells_to(caret);
        let preedit_cells = self
            .preedit
            .as_ref()
            .map_or(0, |preedit| text::width(preedit.text()));
        let shown_caret = caret_cells
            + self
                .preedit
                .as_ref()
                .map_or(Some(0), Preedit::caret_cells)
                .unwrap_or(preedit_cells);
        let total = self.line.cells_to(self.line.text().len()) + preedit_cells;
        let cols = metrics.cols_in(room.width);
        self.scroll = scroll_to_caret(self.scroll, shown_caret, total, cols);
        let left = room.x - self.scroll as f32 * metrics.width;
        let cell_x = |cells: usize| left + cells as f32 * metrics.width;
        let caret_area = Rect::new(cell_x(shown_caret), room.y, metrics.width, metrics.height);
        self.layout = Some(FieldLayout {
            area,
            caret: caret_area,
            metrics,
            scale_factor,
            text: room,
        });
        if self.line.text().is_empty() && self.preedit.is_none() {
            let placeholder =
                StyledText::plain(self.placeholder.as_str(), PLACEHOLDER_COLOR).with_ellipsis();
            frame.draw_text(
                &placeholder.fit(cols),
                Point::new(room.x, room.y),
                room,
                theme,
            );
        } else {
            // A preedit moves the text after the caret: the selection is
            // left undrawn while one shows, since typing replaces it.
            if let (Some(range), None) = (self.line.selection(), &self.preedit) {
                let start = cell_x(self.line.cells_to(range.start)).max(room.x);
                let end = cell_x(self.line.cells_to(range.end)).min(room.x + room.width);
                if end > start {
                    frame.fill_rect(
                        Rect::new(start, room.y, end - start, room.height),
                        theme.selection,
                    );
                }
            }
            let (before, after) = self.line.text().split_at(caret);
            let mut line = StyledText::plain(before, TEXT_COLOR);
            if let Some(preedit) = &self.preedit {
                line.push(Span::new(preedit.text(), TEXT_COLOR).underlined());
            }
            line.push(Span::new(after, TEXT_COLOR));
            frame.draw_text(&line, Point::new(left, room.y), room, theme);
        }
        let caret_shown = self
            .preedit
            .as_ref()
            .is_none_or(|preedit| preedit.caret_cells().is_some());
        self.blink.set_interval(theme.motion.caret_blink);
        if self.focused
            && caret_shown
            && self.blink.is_visible(Instant::now())
            && caret_area.x < room.x + room.width
        {
            let width = logical(CARET_WIDTH, scale_factor);
            frame.fill_rect_over_text(
                Rect::new(caret_area.x, room.y, width, room.height),
                theme.cursor,
            );
        }
    }

    /// Tells the field whether it has the keyboard: it draws its caret
    /// (and an accent frame) only then. A field losing the focus drops a
    /// preedit.
    fn set_focused(&mut self, focused: bool) {
        if focused && !self.focused {
            self.blink.reset(Instant::now());
        }
        self.focused = focused;
        if !focused {
            self.preedit = None;
            self.dragging = false;
        }
    }

    /// Whether the app said the field has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }

    /// Asks for the redraw that next shows or hides the caret while the
    /// field is focused, returning when it is due; `None` while it shows
    /// steady (unfocused, or idle long enough to rest). Call it from
    /// [`Application::about_to_wait`](crate::Application::about_to_wait):
    /// the caret of a field whose app never does shows steady.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        if !self.focused {
            return None;
        }
        self.blink.drive();
        let next = self.blink.next_frame(Instant::now())?;
        cx.request_redraw_at(next);
        Some(next)
    }
}

impl Outcome for TextFieldResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Clicks Implementations
// ========================================================================

impl Clicks {
    /// A left press at `point` at `now` after the press `last`, on a
    /// display scaled by `scale_factor`: one more of a run of quick presses
    /// on one spot, up to a triple click, or the first of a new run.
    pub(super) fn after(last: Option<Self>, now: Instant, point: Point, scale_factor: f64) -> Self {
        let count = match last {
            Some(clicks)
                if clicks.count < TRIPLE_CLICK
                    && is_double_click(Some(clicks.at), now, point, scale_factor) =>
            {
                clicks.count + 1
            }
            Some(_) | None => 1,
        };
        Self {
            at: (now, point),
            count,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Carries out `key` on `line` as every text field does (typing, deleting,
/// the motions along a line, the Emacs keys, copy, cut, paste, undo, and
/// redo through `clipboard`), or `None` when it is no such key.
pub(super) fn edit_by_key(
    line: &mut LineEdit,
    key: Key,
    clipboard: &mut SystemClipboard,
) -> Option<Edit> {
    let mods = key.mods;
    let plain = mods == Mods::NONE;
    let shift = mods == Mods::SHIFT;
    let ctrl = mods == Mods::CTRL;
    let alt = mods == Mods::ALT;
    let primary = mods == Mods::PRIMARY;
    let edit = match key.code {
        KeyCode::Char(c) if plain => line.type_char(c),
        // The Emacs keys first: on Linux and Windows the primary
        // modifier is Ctrl, and these win.
        KeyCode::Char('a') if ctrl => line.move_to(0, false),
        KeyCode::Char('b') if ctrl => line.move_left(false),
        KeyCode::Char('d') if ctrl => line.delete_forward(),
        KeyCode::Char('e') if ctrl => line.move_to(line.text().len(), false),
        KeyCode::Char('f') if ctrl => line.move_right(false),
        KeyCode::Char('h') if ctrl => line.delete_backward(),
        KeyCode::Char('k') if ctrl => line.kill_to_end(),
        KeyCode::Char('u') if ctrl => line.kill_to_start(),
        KeyCode::Char('w') if ctrl => line.kill_word_backward(),
        KeyCode::Char('y') if ctrl => line.yank(),
        KeyCode::Char('b') if alt => line.move_word_left(false),
        KeyCode::Char('d') if alt => line.kill_word_forward(),
        KeyCode::Char('f') if alt => line.move_word_right(false),
        KeyCode::Char('a') if primary => line.select_all(),
        KeyCode::Char('c') if primary => copy(line, clipboard),
        KeyCode::Char('v') if primary => paste(line, clipboard),
        KeyCode::Char('x') if primary => cut(line, clipboard),
        KeyCode::Char('z') if primary => line.undo(),
        KeyCode::Char('z') if mods == Mods::PRIMARY | Mods::SHIFT => line.redo(),
        KeyCode::Insert if ctrl => copy(line, clipboard),
        KeyCode::Insert if shift => paste(line, clipboard),
        KeyCode::Backspace if plain => line.delete_backward(),
        KeyCode::Backspace if ctrl || alt => line.kill_word_backward(),
        KeyCode::Delete if plain => line.delete_forward(),
        KeyCode::Home if plain || shift => line.move_to(0, shift),
        KeyCode::End if plain || shift => line.move_to(line.text().len(), shift),
        KeyCode::Left if plain || shift => line.move_left(shift),
        KeyCode::Right if plain || shift => line.move_right(shift),
        KeyCode::Left if is_word_motion(mods) => line.move_word_left(mods.contains(Mods::SHIFT)),
        KeyCode::Right if is_word_motion(mods) => line.move_word_right(mods.contains(Mods::SHIFT)),
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
        | KeyCode::Up => return None,
    };
    Some(edit)
}

/// Writes `line`'s selection to `clipboard`'s primary selection when it is
/// not `last`, the one last written, on platforms that have one; forgets
/// the last one written when nothing is selected, so selecting it again
/// writes it again.
pub(super) fn publish_primary(
    line: &LineEdit,
    last: &mut Option<Range<usize>>,
    clipboard: &mut SystemClipboard,
) {
    let selection = line.selection();
    if !HAS_PRIMARY_SELECTION || selection == *last {
        return;
    }
    if let Some(text) = line.selected_text() {
        // Kept in the process when the system refuses it.
        let _ = clipboard.write_text(ClipboardKind::Primary, text);
    }
    *last = selection;
}

/// Copies `line`'s selection to `clipboard`.
fn copy(line: &LineEdit, clipboard: &mut SystemClipboard) -> Edit {
    if let Some(text) = line.selected_text() {
        // Kept in the process when the system refuses it.
        let _ = clipboard.write_text(ClipboardKind::Clipboard, text);
    }
    Edit::Moved
}

/// Copies `line`'s selection to `clipboard` and deletes it.
fn cut(line: &mut LineEdit, clipboard: &mut SystemClipboard) -> Edit {
    copy(line, clipboard);
    match line.selection() {
        Some(_) => line.delete_backward(),
        None => Edit::Moved,
    }
}

/// Types the text on `clipboard` over `line`'s selection.
fn paste(line: &mut LineEdit, clipboard: &mut SystemClipboard) -> Edit {
    match clipboard.read_text(ClipboardKind::Clipboard) {
        Some(text) => line.insert(&text),
        None => Edit::Moved,
    }
}

/// Draws a field's frame filling `area`: the background and a border,
/// accent while `focused`. Returns the room inside it for the text, the
/// border and a blank cell in from each side, the whole height.
pub(super) fn draw_frame(frame: &mut Frame, area: Rect, focused: bool, theme: &Theme) -> Rect {
    let scale_factor = frame.scale_factor();
    let border = logical(BORDER, scale_factor);
    let color = match focused {
        true => theme.accent,
        false => theme.border,
    };
    let style = BoxStyle {
        border: Some(Border {
            color: Rgba::from(color),
            width: border,
        }),
        ..theme.shape.highlight(theme.background, scale_factor)
    };
    frame.draw_box(area, &style);
    let inset = border + (PADDING_CELLS * frame.cell_metrics().width).round();
    Rect::new(
        area.x + inset,
        area.y,
        (area.width - 2.0 * inset).max(0.0),
        area.height,
    )
}

/// Whether `mods` move by words with Left and Right: Ctrl (Alt on macOS,
/// as Option), with or without Shift.
fn is_word_motion(mods: Mods) -> bool {
    let word = match cfg!(target_os = "macos") {
        true => Mods::ALT,
        false => Mods::CTRL,
    };
    mods == word || mods == word | Mods::SHIFT
}

/// The response to an edit of the text.
fn response_of(edit: Edit) -> TextFieldResponse {
    match edit {
        Edit::Changed => TextFieldResponse::Changed,
        Edit::Moved => TextFieldResponse::Handled,
    }
}

/// The first cell in sight of a line `total` cells long in `cols` cells,
/// scrolled from `scroll` only as far as keeps the caret's cell `caret`
/// in sight, and no further right than leaves room for the caret after
/// the last character.
pub(super) fn scroll_to_caret(scroll: usize, caret: usize, total: usize, cols: usize) -> usize {
    if cols == 0 {
        return caret;
    }
    let scroll = scroll.min((total + 1).saturating_sub(cols));
    scroll_to_show(scroll, caret, cols)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::app::access::find_node;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::key::parse_key;
    use crate::base::theme::Shape;
    use crate::input::event::{ButtonInput, Composition};
    use crate::testing::TestWindow;

    /// A field holding `text` as if drawn 10 cells wide of 10 by 20
    /// pixels at the window's top left corner, scrolled to its start.
    fn drawn(text: &str) -> TextField {
        let mut field = TextField::with_text(text);
        let room = Rect::new(0.0, 0.0, 100.0, 20.0);
        field.layout = Some(FieldLayout {
            area: room,
            caret: room,
            metrics: CellMetrics {
                baseline: 15.0,
                height: 20.0,
                width: 10.0,
            },
            scale_factor: 1.0,
            text: room,
        });
        field
    }

    /// Hands `field` each key of `specs`, returning the last response.
    fn keys(field: &mut TextField, window: &mut TestWindow, specs: &[&str]) -> TextFieldResponse {
        let mut last = TextFieldResponse::Ignored;
        for spec in specs {
            last = field.handle_key(parse_key(spec).expect("a valid key"), &mut window.cx());
        }
        last
    }

    /// The primary modifier with `c`: Ctrl+`c`, or Cmd+`c` on macOS.
    fn primary(c: char) -> Key {
        Key::new(KeyCode::Char(c), Mods::PRIMARY)
    }

    #[test]
    fn test_shift_selects_and_copy_cut_and_paste_go_through_the_clipboard() {
        let mut window = TestWindow::new(400, 300);
        let mut field = TextField::with_text("report.txt");
        keys(
            &mut field,
            &mut window,
            &["<home>", "S-<right>", "S-<right>"],
        );
        assert_eq!(field.selection(), Some(0..2));
        assert_eq!(
            field.handle_key(primary('c'), &mut window.cx()),
            TextFieldResponse::Handled
        );
        assert_eq!(field.text(), "report.txt", "a copy changes nothing");
        assert_eq!(
            field.handle_key(primary('x'), &mut window.cx()),
            TextFieldResponse::Changed
        );
        assert_eq!(field.text(), "port.txt");
        keys(&mut field, &mut window, &["<end>"]);
        field.handle_key(primary('v'), &mut window.cx());
        assert_eq!(field.text(), "port.txtre");
        field.handle_key(primary('z'), &mut window.cx());
        assert_eq!(field.text(), "port.txt", "undo takes the paste back");
    }

    #[test]
    fn test_a_new_selection_goes_to_the_primary_selection_and_stays_after_it_is_dropped() {
        let mut window = TestWindow::new(400, 300);
        let mut field = TextField::with_text("report.txt");
        keys(
            &mut field,
            &mut window,
            &["S-<left>", "S-<left>", "S-<left>"],
        );
        assert_eq!(
            window
                .clipboard()
                .read_text(ClipboardKind::Primary)
                .as_deref(),
            Some("txt")
        );
        assert_eq!(
            window.clipboard().read_text(ClipboardKind::Clipboard),
            None,
            "never the clipboard"
        );
        window
            .clipboard()
            .write_text(ClipboardKind::Primary, "another app")
            .ok();
        keys(&mut field, &mut window, &["<right>"]);
        assert_eq!(
            window
                .clipboard()
                .read_text(ClipboardKind::Primary)
                .as_deref(),
            Some("another app"),
            "dropping the selection writes nothing"
        );
        keys(&mut field, &mut window, &["S-<home>"]);
        assert_eq!(
            window
                .clipboard()
                .read_text(ClipboardKind::Primary)
                .as_deref(),
            Some("report.txt")
        );
    }

    #[test]
    fn test_a_middle_click_pastes_the_primary_selection_where_it_lands() {
        let mut window = TestWindow::new(400, 300);
        window
            .clipboard()
            .write_text(ClipboardKind::Primary, "big ")
            .ok();
        let mut field = drawn("a cat");
        field.select_all();
        assert_eq!(
            field.paste_primary_at(Point::new(21.0, 5.0), window.clipboard()),
            TextFieldResponse::Changed
        );
        assert_eq!(
            field.text(),
            "a big cat",
            "at the click, not over the selection"
        );
        assert_eq!(field.selection(), None);
    }

    #[test]
    fn test_the_emacs_keys_stay_text_keys_and_the_rest_are_left_to_the_app() {
        let mut window = TestWindow::new(400, 300);
        let mut field = TextField::with_text("one two");
        assert_eq!(
            keys(&mut field, &mut window, &["C-w"]),
            TextFieldResponse::Changed
        );
        assert_eq!(field.text(), "one ");
        keys(&mut field, &mut window, &["C-a", "C-y", " "]);
        assert_eq!(field.text(), "two one ", "C-a is the line start, C-y yanks");
        keys(&mut field, &mut window, &["C-e", "M-b", "M-d"]);
        assert_eq!(field.text(), "two  ");
        for spec in ["<cr>", "<esc>", "<tab>", "<up>", "<down>", "M-x", "<f2>"] {
            let key = parse_key(spec).expect("a valid key");
            assert_eq!(
                field.handle_key(key, &mut window.cx()),
                TextFieldResponse::Ignored,
                "{spec}"
            );
        }
    }

    #[test]
    fn test_a_committed_composition_replaces_the_selection() {
        let mut field = TextField::with_text("ab");
        field.select_all();
        let compose = ImeInput::Preedit(Composition {
            cursor: Some((2, 2)),
            text: "ni".into(),
        });
        assert_eq!(field.handle_ime(&compose), TextFieldResponse::Handled);
        assert_eq!(field.text(), "ab", "composing types nothing yet");
        let commit = ImeInput::Commit("你".into());
        assert_eq!(field.handle_ime(&commit), TextFieldResponse::Changed);
        assert_eq!((field.text(), field.selection()), ("你", None));
    }

    #[test]
    fn test_presses_place_the_caret_and_quick_ones_select_a_word_then_everything() {
        let mut field = drawn("foo bar baz");
        let start = Instant::now();
        let at = |cells: f32| Point::new(cells * 10.0, 10.0);
        field.press_at(at(5.2), start, false);
        assert_eq!((field.line.caret(), field.selection()), (5, None));
        field.press_at(at(5.2), start + Duration::from_millis(100), false);
        assert_eq!(field.selection(), Some(4..7), "a double click selects bar");
        field.press_at(at(5.2), start + Duration::from_millis(200), false);
        assert_eq!(field.selection(), Some(0..11), "a triple click selects all");
        field.press_at(at(1.0), start + Duration::from_secs(2), false);
        field.press_at(at(9.0), start + Duration::from_secs(4), true);
        assert_eq!(
            field.selection(),
            Some(1..9),
            "Shift extends from the caret"
        );
    }

    #[test]
    fn test_a_drag_selects_from_the_press_and_stops_on_release() {
        let mut field = drawn("foo bar baz");
        field.press_at(Point::new(20.0, 10.0), Instant::now(), false);
        assert_eq!(
            field.drag_to(Point::new(60.0, 10.0)),
            TextFieldResponse::Handled
        );
        assert_eq!(field.selection(), Some(2..6));
        assert_eq!(
            field.drag_to(Point::new(-40.0, 10.0)),
            TextFieldResponse::Handled
        );
        assert_eq!(
            field.selection(),
            Some(0..2),
            "left of the field, the start"
        );
        field.dragging = false;
        assert_eq!(
            field.drag_to(Point::new(60.0, 10.0)),
            TextFieldResponse::Ignored
        );
    }

    #[test]
    fn test_the_scroll_moves_only_as_far_as_keeps_the_caret_in_sight() {
        assert_eq!(scroll_to_caret(0, 3, 20, 10), 0, "in sight: no move");
        assert_eq!(
            scroll_to_caret(0, 15, 20, 10),
            6,
            "the caret on the last column"
        );
        assert_eq!(
            scroll_to_caret(8, 4, 20, 10),
            4,
            "back to a caret on the left"
        );
        assert_eq!(
            scroll_to_caret(9, 12, 12, 10),
            3,
            "no blank beyond the caret's cell"
        );
        assert_eq!(scroll_to_caret(5, 0, 0, 10), 0, "a cleared line");
    }

    #[test]
    fn test_a_long_text_scrolls_to_the_caret_and_a_press_lands_on_what_is_drawn() {
        let mut frame = offscreen::plain_frame(400, 300);
        let width = frame.cell_metrics().width;
        let mut field = TextField::with_text(&"x".repeat(30));
        field.draw(
            &mut frame,
            Rect::new(0.0, 0.0, 10.0 * width, 40.0),
            &Theme::dark(),
        );
        assert_eq!(
            field.scroll, 21,
            "the caret after the last x on the last column"
        );
        field.press_at(Point::new(width * 0.2, 10.0), Instant::now(), false);
        assert_eq!(field.line.caret(), 21, "the first cell drawn");
    }

    #[test]
    fn test_the_field_describes_its_text_placeholder_and_selection() {
        let id = FocusId::new("rename");
        let mut frame = offscreen::plain_frame(400, 300);
        let mut field = TextField::with_text("aé.txt").with_placeholder("New name");
        field.select(0..3);
        field.draw(&mut frame, Rect::new(0.0, 0.0, 200.0, 30.0), &Theme::dark());
        field.access(&mut frame, id);
        let update = frame.access.tree_update("t");
        let input = find_node(&update, id).expect("described");
        assert_eq!(
            (input.role(), input.value(), input.placeholder()),
            (Role::TextInput, Some("aé.txt"), Some("New name"))
        );
        let selection = input.text_selection().expect("a selection");
        assert_eq!(
            (
                selection.anchor.character_index,
                selection.focus.character_index
            ),
            (0, 2),
            "characters, not bytes"
        );
        let run = find_node(&update, id.child(0)).expect("a text run");
        assert_eq!(run.character_lengths(), [1, 2, 1, 1, 1, 1]);
    }

    #[test]
    fn test_a_focused_framed_field_draws_its_selection_caret_and_accent_border() {
        let theme = Theme::dark();
        let mut field = TextField::with_text("hello world").with_frame();
        field.set_focused(true);
        field.select(0..5);
        let area = Rect::new(0.0, 0.0, 300.0, 40.0);
        let Some(snapshot) = offscreen::render(300, 40, |frame| {
            frame.clear(Rgba::from(theme.surface));
            field.draw(frame, area, &theme);
        }) else {
            return;
        };
        let cell = snapshot.metrics().width;
        snapshot.assert_pixel(0.0, 20.0, theme.accent, "the focused border");
        snapshot.assert_pixel(150.0, 2.0, theme.background, "the field's fill");
        let selected = Rect::new(cell * 1.5, 12.0, cell * 4.0, 16.0);
        assert!(
            snapshot.contains_color(selected, theme.selection),
            "the selection"
        );
        let caret = Rect::new(cell * 6.0, 12.0, 4.0, 16.0);
        assert!(snapshot.contains_color(caret, theme.cursor), "the caret");
    }

    #[test]
    fn test_a_drag_over_the_field_selects_under_a_text_beam_and_fills_the_primary_selection() {
        let mut window = TestWindow::new(400, 300);
        let mut field = drawn("report.txt");
        let mut send = |input: Input, field: &mut TextField| {
            window.send(&input, |input, cx| field.handle(input, cx))
        };
        send(Input::PointerMoved(Point::new(1.0, 5.0)), &mut field);
        let press = ButtonInput::press(PointerButton::Left, Point::new(1.0, 5.0));
        assert_eq!(
            send(Input::PointerButton(press), &mut field),
            TextFieldResponse::Handled
        );
        send(Input::PointerMoved(Point::new(61.0, 5.0)), &mut field);
        assert_eq!(field.selection(), Some(0..6));
        let release = ButtonInput::release(PointerButton::Left, Point::new(61.0, 5.0));
        send(Input::PointerButton(release), &mut field);
        assert_eq!(window.cursor(), Some(CursorIcon::Text));
        if HAS_PRIMARY_SELECTION {
            assert_eq!(
                window
                    .clipboard()
                    .read_text(ClipboardKind::Primary)
                    .as_deref(),
                Some("report"),
                "written once the button is let go"
            );
        }
    }

    #[test]
    fn test_one_handle_types_keys_and_committed_input_method_text() {
        let mut window = TestWindow::new(400, 300);
        let mut field = TextField::new();
        let key = |spec| Input::Key(parse_key(spec).expect("a valid key"));
        assert_eq!(
            window.feed(&mut field, &key("a")),
            TextFieldResponse::Changed
        );
        assert!(window.take_redraw());
        let compose = Input::Ime(ImeInput::Preedit(Composition {
            cursor: None,
            text: "ni".into(),
        }));
        assert_eq!(
            window.feed(&mut field, &compose),
            TextFieldResponse::Handled
        );
        let commit = Input::Ime(ImeInput::Commit("你".into()));
        assert_eq!(window.feed(&mut field, &commit), TextFieldResponse::Changed);
        assert_eq!(field.text(), "a你");
        assert_eq!(
            window.feed(&mut field, &key("<f2>")),
            TextFieldResponse::Ignored
        );
    }

    #[test]
    fn test_a_modern_framed_field_rounds_its_corners() {
        let theme = Theme {
            shape: Shape::MODERN,
            ..Theme::dark()
        };
        let mut field = TextField::new().with_frame();
        let area = Rect::new(0.0, 0.0, 300.0, 40.0);
        let Some(snapshot) = offscreen::render(300, 40, |frame| {
            frame.clear(Rgba::from(theme.surface));
            field.draw(frame, area, &theme);
        }) else {
            return;
        };
        snapshot.assert_pixel(0.0, 20.0, theme.border, "the left border");
        snapshot.assert_pixel(150.0, 20.0, theme.background, "the fill");
        snapshot.assert_pixel(0.0, 0.0, theme.surface, "the rounded-off corner");
    }
}
