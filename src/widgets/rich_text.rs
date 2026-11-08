//! [`RichText`]: a read-only view of rich [`Paragraph`]s, top to bottom,
//! or of a whole [`Document`] (headings, lists, quotes, code, tables,
//! images, boxed sections), for a dictionary entry, a chapter of a book,
//! or a help page. It wraps them to its width, scrolls when they are
//! taller than its area, lets the user select and copy their text across
//! every block, reports clicked links and data links, finds text in them,
//! and scrolls to a document's anchors. Only the blocks in sight are
//! drawn, so a long chapter scrolls as a short entry does.
//!
//! Keys, while the app says the view is focused: Shift+Left and
//! Shift+Right extend the selection a character, Shift+Up and Shift+Down a
//! line, Shift+Home and Shift+End to the ends of the line, and
//! Ctrl+Shift+Home and Ctrl+Shift+End to the ends of the text, starting
//! from the top of the view when nothing is selected. The primary modifier
//! (Ctrl, or Cmd on macOS) with A selects everything (on every platform:
//! read-only text has no line start for Ctrl+A to move to), and with C,
//! like Ctrl+Insert, copies the selection through the
//! [`SystemClipboard`](crate::SystemClipboard). Esc drops the selection.
//! Every other key scrolls as a [`ScrollView`] does.
//!
//! The pointer: a left press places the start of a selection and a drag
//! extends it (scrolling when dragged past the top or the bottom), a
//! Shift+press extends the selection, a double click selects a word and a
//! triple click a paragraph. A click on a link, let go over the same link,
//! is [`RichTextResponse::Link`]. The wheel and the scrollbar scroll. On
//! X11 and Wayland the selection goes to the primary selection.
//!
//! [`RichText::set_find`] highlights every match of a query in the
//! theme's warning color, the current one on the selection color framed
//! in the warning color; [`RichText::next_match`] and
//! [`RichText::previous_match`] step through them, scrolling each into
//! view.

use std::cmp::Ordering;
use std::ops::Range;
use std::time::Instant;

use accesskit::{Node, Role, TextPosition, TextSelection};

use super::super::app::access::node_id;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::color::Rgba;
use super::super::base::cursor::CursorIcon;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::platform::clipboard::{ClipboardKind, HAS_PRIMARY_SELECTION, SystemClipboard};
use super::super::render::icon::logical;
use super::super::render::paragraph::{Paragraph, ParagraphLayout};
use super::document::{BlockRole, DataLink, Decor, Document, Link};
use super::find::{Find, find_all};
use super::scroll_view::{ScrollView, TRACK_WIDTH};
use super::text_field::{Clicks, TRIPLE_CLICK};

// ========================================================================
// Constants
// ========================================================================

/// The blank space between two paragraphs, in lines.
const PARAGRAPH_GAP_LINES: f32 = 0.5;

/// How far into a line, in pixels, a point is put to land on it.
const LINE_NUDGE: f32 = 1.0;

/// The alpha of the warning color filled behind a match that is not the
/// current one: light enough to read the text through.
const MATCH_ALPHA: u8 = 0x60;

/// The thickness of the frame around the current match, in logical
/// pixels.
const CURRENT_MATCH_BORDER: f32 = 1.0;

/// What a copy puts between two paragraphs.
const PARAGRAPH_BREAK: char = '\n';

/// The child index of a table's accessibility node under its first
/// block's id.
const TABLE_NODE: u64 = 1;

/// The child index of a table row's accessibility node under its first
/// block's id.
const ROW_NODE: u64 = 2;

/// The thickness of a document's table lines and box borders, in logical
/// pixels.
const DECOR_BORDER: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A read-only, selectable view of rich paragraphs. The app owns it, hands
/// it the window's pointer events (and its keys while it has the focus),
/// and draws it each frame.
#[derive(Clone, Debug, Default)]
pub struct RichText {
    /// The document's anchors, each with its first text block.
    anchors: Vec<(String, usize)>,
    /// The paragraphs, top to bottom.
    blocks: Vec<Paragraph>,
    /// The last press and how many quick presses on one spot led to it.
    clicks: Option<Clicks>,
    /// The pointer shape last asked for.
    cursor: Option<CursorIcon>,
    /// Where the parts were drawn last, for the pointer.
    drawn: Option<Drawn>,
    /// Whether a left press is held, selecting as it moves.
    dragging: bool,
    /// The search, when there is one.
    find: Option<Find<Match>>,
    /// Whether the app gave the view the keyboard.
    focused: bool,
    /// The left edge a run of Shift+Up and Shift+Down keeps to.
    goal_x: Option<f32>,
    /// The paragraphs laid out at the width last drawn.
    laid_out: Option<Laid>,
    /// The link a held press began on, followed when let go over it.
    pending_link: Option<String>,
    /// The selection last written to the primary selection.
    primary: Option<Selection>,
    /// Whether the current match waits for a frame to be scrolled into
    /// view.
    reveal_pending: bool,
    /// The scrolled viewport and its scrollbar.
    scroll: ScrollView,
    /// What each block is to screen readers; empty for plain paragraphs.
    roles: Vec<BlockRole>,
    /// The document shown, or `None` for plain paragraphs.
    document: Option<Document>,
    /// The selection, its anchor where it began.
    selection: Option<Selection>,
    /// Each paragraph's text.
    texts: Vec<String>,
}

/// What became of an input offered to a [`RichText`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RichTextResponse {
    /// The view used the input: it selected, copied, or scrolled.
    Handled,
    /// A document's data link was clicked, such as a cross-reference to
    /// look up or an audio button: act on its key.
    Data(DataLink),
    /// The input is not the view's.
    Ignored,
    /// The link to this target was clicked: follow it.
    Link(String),
}

/// Where a view was drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn {
    /// The whole view.
    area: Rect,
    /// How far the content was scrolled down.
    offset: f32,
    /// The display's scale factor.
    scale_factor: f64,
    /// The part of the view the paragraphs show in.
    viewport: Rect,
}

/// The paragraphs laid out at one width, in content coordinates: from
/// the top left of the first paragraph.
#[derive(Clone, Debug, PartialEq)]
struct Laid {
    /// The height of all of them, gaps between them included.
    height: f32,
    /// What they were laid out for.
    key: LayoutKey,
    /// What a document draws under its text: fills, lines, list markers,
    /// images; empty for plain paragraphs.
    decor: Vec<Decor>,
    /// Where each one's text landed.
    layouts: Vec<ParagraphLayout>,
    /// Where each one is: plain paragraphs stacked full width, a
    /// document's blocks set in or side by side.
    rects: Vec<Rect>,
}

/// What a layout depends on beyond the paragraphs.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LayoutKey {
    /// The line height of the window's font.
    line: f32,
    /// The display's scale factor.
    scale_factor: f64,
    /// The width the paragraphs wrap at.
    width: f32,
}

/// A place in the text: a paragraph and a character offset in it.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct TextPos {
    /// The paragraph.
    block: usize,
    /// The byte offset into its text, on a character boundary.
    offset: usize,
}

/// A selection: from where it began to where it ends now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Selection {
    /// Where it began.
    anchor: TextPos,
    /// Where it ends now, which keys move.
    focus: TextPos,
}

/// One match of a search.
#[derive(Clone, Debug, PartialEq)]
struct Match {
    /// The paragraph it is in.
    block: usize,
    /// Its characters in the paragraph's text.
    range: Range<usize>,
}

/// Where a key moves the end of the selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Motion {
    /// A line down.
    Down,
    /// To the end of the text.
    End,
    /// A character back.
    Left,
    /// To the end of the line.
    LineEnd,
    /// To the start of the line.
    LineStart,
    /// A character forward.
    Right,
    /// To the start of the text.
    Start,
    /// A line up.
    Up,
}

// ========================================================================
// RichText Implementations
// ========================================================================

impl RichText {
    // --------------------------------------------------------------------
    // State
    // --------------------------------------------------------------------

    /// A view of `blocks`, top to bottom, scrolled to the top, nothing
    /// selected.
    pub fn new(blocks: Vec<Paragraph>) -> Self {
        let mut view = Self::default();
        view.set_blocks(blocks);
        view
    }

    /// A view of `document`, scrolled to the top, nothing selected.
    pub fn document(document: Document) -> Self {
        let mut view = Self::default();
        view.set_document(document);
        view
    }

    /// Shows `document` instead, dropping the selection; a search finds
    /// its query in it anew.
    pub fn set_document(&mut self, document: Document) {
        let flow = document.collect();
        self.set_blocks(flow.paragraphs);
        self.anchors = flow.anchors;
        self.roles = flow.roles;
        self.document = Some(document);
    }

    /// Scrolls the document's block with the anchor `id` (a heading's or a
    /// container's) to the top of the view, at once; `false` when the
    /// document has no such anchor or the view was not drawn yet.
    pub fn scroll_to_anchor(&mut self, id: &str) -> bool {
        let Some(&(_, block)) = self.anchors.iter().find(|(anchor, _)| anchor == id) else {
            return false;
        };
        let Some(rect) = self
            .laid_out
            .as_ref()
            .and_then(|laid| laid.rects.get(block))
        else {
            return false;
        };
        self.scroll.scroll_to(Point::new(0.0, rect.y));
        true
    }

    /// Shows `blocks` instead, dropping the selection; a search finds its
    /// query in them anew.
    pub fn set_blocks(&mut self, blocks: Vec<Paragraph>) {
        self.document = None;
        self.anchors.clear();
        self.roles.clear();
        self.texts = blocks.iter().map(Paragraph::text).collect();
        self.blocks = blocks;
        self.selection = None;
        self.laid_out = None;
        if let Some(find) = self.find.take() {
            self.set_find(find.query(), find.is_case_sensitive());
        }
    }

    /// The selected text, paragraphs joined by a line feed, or `None` when
    /// nothing is selected.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection.map(Selection::ordered)?;
        if start == end {
            return None;
        }
        let mut text = String::new();
        for block in start.block..=end.block {
            if block > start.block {
                text.push(PARAGRAPH_BREAK);
            }
            text.push_str(&self.texts[block][self.block_range(block, start, end)]);
        }
        Some(text)
    }

    // --------------------------------------------------------------------
    // Find
    // --------------------------------------------------------------------

    /// Searches the paragraphs for `query`, letters matching in either
    /// case unless `case_sensitive`, and highlights every match; the
    /// current one is the first at or below the top of the view, scrolled
    /// into view. An empty query ends the search.
    pub fn set_find(&mut self, query: &str, case_sensitive: bool) {
        if query.is_empty() {
            self.find = None;
            return;
        }
        let matches: Vec<Match> = self
            .texts
            .iter()
            .enumerate()
            .flat_map(|(block, text)| {
                find_all(text, query, case_sensitive)
                    .into_iter()
                    .map(move |range| Match { block, range })
            })
            .collect();
        let top = self.first_visible().block;
        self.find = Some(Find::new(query, case_sensitive, matches, |found| {
            found.block >= top
        }));
        self.reveal_current();
    }

    /// How many matches the search found; 0 without a search.
    pub fn match_count(&self) -> usize {
        self.find.as_ref().map_or(0, |find| find.matches().len())
    }

    /// The index of the current match among them, or `None` without one.
    pub fn current_match(&self) -> Option<usize> {
        self.find.as_ref()?.current()
    }

    /// Steps to the next match, after the last back to the first, and
    /// scrolls it into view.
    pub fn next_match(&mut self) {
        self.step_match(1);
    }

    /// Steps to the previous match, before the first round to the last,
    /// and scrolls it into view.
    pub fn previous_match(&mut self) {
        self.step_match(-1);
    }

    /// Moves the current match `delta` matches, wrapping, and scrolls it
    /// into view.
    fn step_match(&mut self, delta: isize) {
        if self.find.as_mut().is_some_and(|find| find.step(delta)) {
            self.reveal_current();
        }
    }

    /// Scrolls the current match into view, or waits for the first frame
    /// to.
    fn reveal_current(&mut self) {
        let rect = self.find.as_ref().and_then(|find| {
            let found = find.current_match()?;
            let layout = self.laid_out.as_ref()?.layouts.get(found.block)?;
            layout.range_rects(found.range.clone()).into_iter().next()
        });
        match rect {
            Some(rect) if self.drawn.is_some() => {
                self.scroll.reveal(rect);
                self.reveal_pending = false;
            }
            Some(_) | None => self.reveal_pending = self.current_match().is_some(),
        }
    }

    // --------------------------------------------------------------------
    // Accessibility
    // --------------------------------------------------------------------

    /// Describes the view as last drawn to screen readers as `id`: a
    /// document holding a paragraph per block (the `index`th is child
    /// `index` of `id`), each holding its text as a text run (its child
    /// 0), and the selection as the document's text selection. Nothing
    /// before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let (Some(drawn), Some(laid)) = (self.drawn, &self.laid_out) else {
            return;
        };
        let mut document = Node::new(Role::Document);
        if let Some(selection) = self.selection {
            document.set_text_selection(TextSelection {
                anchor: self.text_position(id, selection.anchor),
                focus: self.text_position(id, selection.focus),
            });
        }
        frame.enter_access(id, drawn.area, document);
        let shift = Point::new(drawn.viewport.x, drawn.viewport.y - drawn.offset);
        // The table and the row of it the blocks described are in, each
        // with the block it began at.
        let mut table: Option<(usize, usize)> = None;
        let mut row: Option<usize> = None;
        for (block, text) in self.texts.iter().enumerate() {
            let rect = moved(laid.rects.get(block).copied().unwrap_or_default(), shift);
            let role = self.roles.get(block).cloned().unwrap_or_default();
            let cell = match &role {
                BlockRole::Cell(cell) => Some(*cell),
                BlockRole::Heading(_)
                | BlockRole::Image(_)
                | BlockRole::ListItem
                | BlockRole::Paragraph => None,
            };
            if row.is_some()
                && cell.is_none_or(|cell| {
                    Some((cell.table, cell.row)) != table.zip(row).map(|((t, _), r)| (t, r))
                })
            {
                frame.leave_access();
                row = None;
            }
            if table.is_some_and(|(t, _)| cell.is_none_or(|cell| cell.table != t)) {
                frame.leave_access();
                table = None;
            }
            if let Some(cell) = cell {
                if table.is_none() {
                    frame.enter_access(
                        id.child(block as u64).child(TABLE_NODE),
                        rect,
                        Node::new(Role::Table),
                    );
                    table = Some((cell.table, block));
                }
                if row.is_none() {
                    frame.enter_access(
                        id.child(block as u64).child(ROW_NODE),
                        rect,
                        Node::new(Role::Row),
                    );
                    row = Some(cell.row);
                }
            }
            let node = match &role {
                BlockRole::Cell(cell) if cell.header => Node::new(Role::ColumnHeader),
                BlockRole::Cell(_) => Node::new(Role::Cell),
                BlockRole::Heading(level) => {
                    let mut node = Node::new(Role::Heading);
                    node.set_level(usize::from(*level));
                    node
                }
                BlockRole::Image(alt) => {
                    let mut node = Node::new(Role::Image);
                    node.set_label(alt.as_str());
                    frame.access(id.child(block as u64), rect, node);
                    continue;
                }
                BlockRole::ListItem => Node::new(Role::ListItem),
                BlockRole::Paragraph => Node::new(Role::Paragraph),
            };
            let paragraph_id = id.child(block as u64);
            frame.enter_access(paragraph_id, rect, node);
            let mut run = Node::new(Role::TextRun);
            run.set_value(text.as_str());
            run.set_character_lengths(text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<_>>());
            frame.access(paragraph_id.child(0), rect, run);
            frame.leave_access();
        }
        if row.is_some() {
            frame.leave_access();
        }
        if table.is_some() {
            frame.leave_access();
        }
        frame.leave_access();
    }

    /// The screen reader's position of `pos` in the view described as
    /// `id`: a character of its paragraph's text run.
    fn text_position(&self, id: FocusId, pos: TextPos) -> TextPosition {
        let text = self.texts.get(pos.block).map_or("", String::as_str);
        TextPosition {
            character_index: text[..pos.offset.min(text.len())].chars().count(),
            node: node_id(id.child(pos.block as u64).child(0)),
        }
    }

    // --------------------------------------------------------------------
    // Keys
    // --------------------------------------------------------------------

    /// Reacts to `key` while focused, as the module documentation lists,
    /// writing a new selection to the primary selection.
    fn handle_key(&mut self, key: Key, cx: &mut EventCx<'_>) -> RichTextResponse {
        if !self.focused || self.drawn.is_none() {
            return RichTextResponse::Ignored;
        }
        let response = match self.select_by_key(key, cx.clipboard()) {
            Some(response) => response,
            None => match self.scroll.handle(&Input::Key(key), cx).is_ignored() {
                true => RichTextResponse::Ignored,
                false => RichTextResponse::Handled,
            },
        };
        self.publish_primary(cx.clipboard());
        response
    }

    /// Carries out `key` when it selects or copies, or `None` when it is
    /// no such key.
    fn select_by_key(
        &mut self,
        key: Key,
        clipboard: &mut SystemClipboard,
    ) -> Option<RichTextResponse> {
        let mods = key.mods;
        let shift = mods == Mods::SHIFT;
        let ctrl_shift = mods == Mods::CTRL | Mods::SHIFT;
        let primary = mods == Mods::PRIMARY;
        let motion = match key.code {
            KeyCode::Char('a') if primary => {
                self.select_all();
                return Some(RichTextResponse::Handled);
            }
            KeyCode::Char('c') if primary => return Some(self.copy(clipboard)),
            KeyCode::Insert if mods == Mods::CTRL => return Some(self.copy(clipboard)),
            KeyCode::Esc if mods == Mods::NONE && self.selected_text().is_some() => {
                self.selection = None;
                return Some(RichTextResponse::Handled);
            }
            KeyCode::Left if shift => Motion::Left,
            KeyCode::Right if shift => Motion::Right,
            KeyCode::Up if shift => Motion::Up,
            KeyCode::Down if shift => Motion::Down,
            KeyCode::Home if shift => Motion::LineStart,
            KeyCode::End if shift => Motion::LineEnd,
            KeyCode::Home if ctrl_shift => Motion::Start,
            KeyCode::End if ctrl_shift => Motion::End,
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
        self.extend(motion);
        Some(RichTextResponse::Handled)
    }

    /// Selects every paragraph.
    fn select_all(&mut self) {
        let Some(last) = self.texts.len().checked_sub(1) else {
            return;
        };
        self.selection = Some(Selection {
            anchor: TextPos::default(),
            focus: TextPos {
                block: last,
                offset: self.texts[last].len(),
            },
        });
    }

    /// Copies the selection to `clipboard`; ignored with nothing selected.
    fn copy(&self, clipboard: &mut SystemClipboard) -> RichTextResponse {
        let Some(text) = self.selected_text() else {
            return RichTextResponse::Ignored;
        };
        // Kept in the process when the system refuses it.
        let _ = clipboard.write_text(ClipboardKind::Clipboard, &text);
        RichTextResponse::Handled
    }

    /// Moves the end of the selection by `motion`, starting one at the top
    /// of the view when there is none, and scrolls it into view.
    fn extend(&mut self, motion: Motion) {
        let from = self
            .selection
            .map_or_else(|| self.first_visible(), |selection| selection.focus);
        let anchor = self.selection.map_or(from, |selection| selection.anchor);
        if !matches!(motion, Motion::Up | Motion::Down) {
            self.goal_x = None;
        }
        let focus = self.moved(from, motion);
        self.selection = Some(Selection { anchor, focus });
        if let Some(caret) = self.caret_rect(focus) {
            self.scroll.reveal(caret);
        }
    }

    /// Where `motion` takes `from`.
    fn moved(&mut self, from: TextPos, motion: Motion) -> TextPos {
        let last = self.texts.len().saturating_sub(1);
        let text = self.texts.get(from.block).map_or("", String::as_str);
        match motion {
            Motion::Left => match text[..from.offset].chars().next_back() {
                Some(c) => TextPos {
                    offset: from.offset - c.len_utf8(),
                    ..from
                },
                None if from.block > 0 => TextPos {
                    block: from.block - 1,
                    offset: self.texts[from.block - 1].len(),
                },
                None => from,
            },
            Motion::Right => match text[from.offset..].chars().next() {
                Some(c) => TextPos {
                    offset: from.offset + c.len_utf8(),
                    ..from
                },
                None if from.block < last => TextPos {
                    block: from.block + 1,
                    offset: 0,
                },
                None => from,
            },
            Motion::Start => TextPos::default(),
            Motion::End => TextPos {
                block: last,
                offset: self.texts.get(last).map_or(0, String::len),
            },
            Motion::LineStart | Motion::LineEnd => {
                let Some((caret, layout)) = self.caret_rect(from).zip(self.layout(from.block))
                else {
                    return from;
                };
                let x = match motion {
                    Motion::LineStart => f32::NEG_INFINITY,
                    Motion::Down
                    | Motion::End
                    | Motion::Left
                    | Motion::LineEnd
                    | Motion::Right
                    | Motion::Start
                    | Motion::Up => f32::INFINITY,
                };
                let middle = caret.y + caret.height / 2.0;
                TextPos {
                    offset: layout.offset_at(Point::new(x, middle)),
                    ..from
                }
            }
            Motion::Up | Motion::Down => self.vertical(from, motion == Motion::Down),
        }
    }

    /// The place a line below `from` (`down`) or above it, at the left
    /// edge the run of vertical moves keeps to; across paragraphs too.
    fn vertical(&mut self, from: TextPos, down: bool) -> TextPos {
        let (Some(caret), Some(laid)) = (self.caret_rect(from), &self.laid_out) else {
            return from;
        };
        let x = *self.goal_x.get_or_insert(caret.x);
        let rect = laid.rects.get(from.block).copied().unwrap_or_default();
        let top = rect.y;
        let bottom = top + rect.height;
        let y = match down {
            true if caret.y + caret.height + LINE_NUDGE < bottom => {
                caret.y + caret.height + LINE_NUDGE
            }
            true => match laid.rects.get(from.block + 1) {
                Some(next) => next.y.max(bottom) + LINE_NUDGE,
                None => return from,
            },
            false if caret.y - LINE_NUDGE > top => caret.y - LINE_NUDGE,
            false if from.block > 0 => top - LINE_NUDGE - self.gap(),
            false => return from,
        };
        self.pos_at(Point::new(x, y)).unwrap_or(from)
    }

    // --------------------------------------------------------------------
    // Pointer
    // --------------------------------------------------------------------

    /// Reacts to pointer `input`, as the module documentation lists,
    /// writing a selection made to the primary selection once the button
    /// is let go.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> RichTextResponse {
        let selecting = self.dragging && !matches!(input, Input::Wheel(_));
        if !selecting && !self.scroll.handle(input, cx).is_ignored() {
            return RichTextResponse::Handled;
        }
        let response = match input {
            Input::PointerButton(button) => match (button.button, button.pressed) {
                (PointerButton::Left, true) => {
                    self.press(button.position, button.mods.contains(Mods::SHIFT))
                }
                (PointerButton::Left, false) => self.release(button.position),
                (
                    PointerButton::Back
                    | PointerButton::Forward
                    | PointerButton::Middle
                    | PointerButton::Other(_)
                    | PointerButton::Right,
                    _,
                ) => RichTextResponse::Ignored,
            },
            Input::PointerMoved(point) => {
                self.hover(Some(*point), cx);
                self.drag_to(*point)
            }
            Input::PointerLeft => {
                self.hover(None, cx);
                RichTextResponse::Ignored
            }
            Input::FocusLost => {
                self.dragging = false;
                RichTextResponse::Ignored
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::Ime(_)
            | Input::Key(_)
            | Input::Wheel(_) => RichTextResponse::Ignored,
        };
        if !self.dragging {
            self.publish_primary(cx.clipboard());
        }
        response
    }

    /// A left press at `point`, with Shift held when `shift`.
    fn press(&mut self, point: Point, shift: bool) -> RichTextResponse {
        let Some(drawn) = self.drawn.filter(|drawn| drawn.viewport.contains(point)) else {
            return RichTextResponse::Ignored;
        };
        let Some(pos) = self.pos_at(self.to_content(point)) else {
            return RichTextResponse::Ignored;
        };
        let clicks = Clicks::after(self.clicks, Instant::now(), point, drawn.scale_factor);
        self.clicks = Some(clicks);
        self.goal_x = None;
        self.pending_link = None;
        let text = &self.texts[pos.block];
        let (anchor, focus) = match clicks.count {
            1 => {
                self.dragging = true;
                match (shift, self.selection) {
                    (true, Some(selection)) => (selection.anchor, pos),
                    (true, None) | (false, _) => {
                        self.pending_link = self.link_at(point).map(str::to_owned);
                        (pos, pos)
                    }
                }
            }
            TRIPLE_CLICK => positions(pos.block, 0..text.len()),
            _ => positions(pos.block, word_range(text, pos.offset)),
        };
        self.selection = Some(Selection { anchor, focus });
        RichTextResponse::Handled
    }

    /// The left button let go at `point`: a click on a link follows it.
    fn release(&mut self, point: Point) -> RichTextResponse {
        let was_dragging = std::mem::take(&mut self.dragging);
        if let Some(target) = self.pending_link.take()
            && self.link_at(point) == Some(target.as_str())
        {
            if let Some(selection) = &mut self.selection {
                selection.focus = selection.anchor;
            }
            return match Link::from_target(&target) {
                Link::Data(data) => RichTextResponse::Data(data),
                Link::Href(href) => RichTextResponse::Link(href),
            };
        }
        match was_dragging {
            true => RichTextResponse::Handled,
            false => RichTextResponse::Ignored,
        }
    }

    /// Extends the selection to `point` while a press is held, scrolling
    /// toward it when it is past the viewport.
    fn drag_to(&mut self, point: Point) -> RichTextResponse {
        if !self.dragging {
            return RichTextResponse::Ignored;
        }
        let Some(pos) = self.pos_at(self.to_content(point)) else {
            return RichTextResponse::Ignored;
        };
        let Some(selection) = self
            .selection
            .as_mut()
            .filter(|selection| selection.focus != pos)
        else {
            return RichTextResponse::Ignored;
        };
        selection.focus = pos;
        let outside = self
            .drawn
            .is_some_and(|drawn| !drawn.viewport.contains(point));
        if outside && let Some(caret) = self.caret_rect(pos) {
            self.scroll.reveal(caret);
        }
        RichTextResponse::Handled
    }

    /// Follows the pointer to `point` (`None` outside the window): a hand
    /// over a link, a text beam over the rest of the text.
    fn hover(&mut self, point: Option<Point>, cx: &mut EventCx<'_>) {
        let over = point.filter(|point| {
            self.drawn
                .is_some_and(|drawn| drawn.viewport.contains(*point))
        });
        let icon = over.map(|point| match self.link_at(point) {
            Some(_) => CursorIcon::Pointer,
            None => CursorIcon::Text,
        });
        if icon != self.cursor {
            cx.set_cursor(icon.unwrap_or(CursorIcon::Default));
            self.cursor = icon;
        }
    }

    /// Writes the selection to `clipboard`'s primary selection when it is
    /// not the one last written, on platforms that have one.
    fn publish_primary(&mut self, clipboard: &mut SystemClipboard) {
        if !HAS_PRIMARY_SELECTION || self.selection == self.primary {
            return;
        }
        if let Some(text) = self.selected_text() {
            // Kept in the process when the system refuses it.
            let _ = clipboard.write_text(ClipboardKind::Primary, &text);
        }
        self.primary = self.selection;
    }

    // --------------------------------------------------------------------
    // Geometry
    // --------------------------------------------------------------------

    /// `point` in the window as a point in the content.
    fn to_content(&self, point: Point) -> Point {
        match self.drawn {
            Some(drawn) => Point::new(
                point.x - drawn.viewport.x,
                point.y - drawn.viewport.y + drawn.offset,
            ),
            None => point,
        }
    }

    /// The place in the text nearest `point` in the content; `None`
    /// without paragraphs or before the first frame.
    fn pos_at(&self, point: Point) -> Option<TextPos> {
        let laid = self.laid_out.as_ref()?;
        let half_gap = self.gap() / 2.0;
        // The block on the line of `point` nearest it across (a table's
        // cells sit side by side), else the first below it, else the last.
        let across = |rect: &Rect| {
            let past = (rect.x - point.x).max(point.x - (rect.x + rect.width));
            past.max(0.0)
        };
        let block = laid
            .rects
            .iter()
            .enumerate()
            .filter(|(_, rect)| {
                point.y >= rect.y - half_gap && point.y < rect.y + rect.height + half_gap
            })
            .min_by(|(_, a), (_, b)| across(a).total_cmp(&across(b)))
            .map(|(block, _)| block)
            .or_else(|| {
                laid.rects
                    .iter()
                    .position(|rect| point.y < rect.y + rect.height + half_gap)
            })
            .or_else(|| laid.layouts.len().checked_sub(1))?;
        Some(TextPos {
            block,
            offset: laid.layouts[block].offset_at(point),
        })
    }

    /// Where the caret before `pos` is in the content.
    fn caret_rect(&self, pos: TextPos) -> Option<Rect> {
        Some(self.layout(pos.block)?.caret_rect(pos.offset))
    }

    /// The layout of paragraph `block`, in the content.
    fn layout(&self, block: usize) -> Option<&ParagraphLayout> {
        self.laid_out.as_ref()?.layouts.get(block)
    }

    /// The target of the link under `point` in the window.
    fn link_at(&self, point: Point) -> Option<&str> {
        let content = self.to_content(point);
        self.laid_out
            .as_ref()?
            .layouts
            .iter()
            .find_map(|layout| layout.link_at(content))
    }

    /// The start of the first paragraph in sight.
    fn first_visible(&self) -> TextPos {
        let (Some(laid), Some(drawn)) = (&self.laid_out, self.drawn) else {
            return TextPos::default();
        };
        let block = laid
            .rects
            .iter()
            .position(|rect| rect.y + rect.height > drawn.offset)
            .unwrap_or(0);
        TextPos { block, offset: 0 }
    }

    /// The blank space between two paragraphs as last laid out.
    fn gap(&self) -> f32 {
        self.laid_out
            .as_ref()
            .map_or(0.0, |laid| (laid.key.line * PARAGRAPH_GAP_LINES).round())
    }

    /// The characters of paragraph `block` between `start` and `end`.
    fn block_range(&self, block: usize, start: TextPos, end: TextPos) -> Range<usize> {
        let from = match block == start.block {
            true => start.offset,
            false => 0,
        };
        let to = match block == end.block {
            true => end.offset,
            false => self.texts[block].len(),
        };
        from..to
    }

    /// Lays the paragraphs out at `width` in `frame`'s font, unless they
    /// already are.
    fn lay_out(&mut self, frame: &Frame, width: f32) {
        let key = LayoutKey {
            line: frame.cell_metrics().height,
            scale_factor: frame.scale_factor(),
            width,
        };
        if self.laid_out.as_ref().is_some_and(|laid| laid.key == key) {
            return;
        }
        if let Some(document) = &self.document {
            let flow = document.lay_out(frame, width);
            self.laid_out = Some(Laid {
                decor: flow.decor,
                height: flow.height,
                key,
                layouts: flow.layouts,
                rects: flow.rects,
            });
            return;
        }
        let gap = (key.line * PARAGRAPH_GAP_LINES).round();
        let mut top = 0.0;
        let mut rects = Vec::with_capacity(self.blocks.len());
        let layouts = self
            .blocks
            .iter()
            .map(|paragraph| {
                let layout = frame.paragraph_layout(paragraph, Rect::new(0.0, top, width, 0.0));
                rects.push(Rect::new(0.0, top, width, layout.height()));
                top += layout.height() + gap;
                layout
            })
            .collect();
        self.laid_out = Some(Laid {
            decor: Vec::new(),
            height: (top - gap).max(0.0),
            key,
            layouts,
            rects,
        });
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Fills the matches of the search in paragraph `block` drawn shifted
    /// by `shift`: the current one on the selection color framed in the
    /// warning color, the rest in a light warning color.
    fn draw_matches(&self, frame: &mut Frame, block: usize, shift: Point, theme: &Theme) {
        let (Some(find), Some(layout)) = (&self.find, self.layout(block)) else {
            return;
        };
        let scale_factor = frame.scale_factor();
        let other = BoxStyle {
            fill: theme.warning.with_alpha(MATCH_ALPHA),
            ..theme.shape.highlight(theme.warning, scale_factor)
        };
        let current = BoxStyle {
            border: Some(Border {
                color: Rgba::from(theme.warning),
                width: logical(CURRENT_MATCH_BORDER, scale_factor),
            }),
            ..theme.shape.highlight(theme.selection, scale_factor)
        };
        for (index, found) in find.matches().iter().enumerate() {
            if found.block != block {
                continue;
            }
            let style = match find.current() == Some(index) {
                true => &current,
                false => &other,
            };
            for rect in layout.range_rects(found.range.clone()) {
                frame.draw_box(moved(rect, shift), style);
            }
        }
    }

    /// Fills the selected characters of paragraph `block` drawn shifted by
    /// `shift` with the selection color.
    fn draw_selection(&self, frame: &mut Frame, block: usize, shift: Point, theme: &Theme) {
        let (Some((start, end)), Some(layout)) =
            (self.selection.map(Selection::ordered), self.layout(block))
        else {
            return;
        };
        if start == end || block < start.block || block > end.block {
            return;
        }
        for rect in layout.range_rects(self.block_range(block, start, end)) {
            frame.fill_rect(moved(rect, shift), theme.selection);
        }
    }
}

impl Widget for RichText {
    type Output = ();
    type Response = RichTextResponse;

    /// Reacts to `input`: keys while focused and the pointer, as the
    /// module documentation lists. Asks for a redraw whenever it used the
    /// input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> RichTextResponse {
        let response = match input {
            Input::Key(key) => self.handle_key(*key, cx),
            Input::FocusLost
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => self.handle_pointer(input, cx),
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::Ime(_) => RichTextResponse::Ignored,
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the paragraphs wrapped to `area`'s width (less a scrollbar
    /// when they are taller than it), scrolled, with the search's matches
    /// and the selection filled behind their text.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let narrow = (area.width - logical(TRACK_WIDTH, frame.scale_factor())).max(0.0);
        let narrow_holds = self
            .laid_out
            .as_ref()
            .is_some_and(|laid| laid.key.width == narrow && laid.height > area.height);
        if !narrow_holds {
            self.lay_out(frame, area.width);
            if self
                .laid_out
                .as_ref()
                .is_some_and(|laid| laid.height > area.height)
            {
                self.lay_out(frame, narrow);
            }
        }
        let Some((width, height)) = self
            .laid_out
            .as_ref()
            .map(|laid| (laid.key.width, laid.height))
        else {
            return;
        };
        self.scroll.set_content_size(width, height);
        let viewport = self.scroll.draw_at(frame, area, theme, Instant::now());
        self.drawn = Some(Drawn {
            area,
            offset: self.scroll.offset().y,
            scale_factor: frame.scale_factor(),
            viewport,
        });
        if self.reveal_pending {
            self.reveal_current();
        }
        let offset = self.scroll.offset().y;
        let shift = Point::new(viewport.x, viewport.y - offset);
        frame.clipped(viewport, |frame| {
            let Some(laid) = &self.laid_out else {
                return;
            };
            let in_sight =
                |rect: &Rect| rect.y + rect.height >= offset && rect.y <= offset + viewport.height;
            for decor in &laid.decor {
                draw_decor(frame, decor, shift, &in_sight, theme);
            }
            for (block, paragraph) in self.blocks.iter().enumerate() {
                let Some(rect) = laid.rects.get(block).filter(|rect| in_sight(rect)) else {
                    continue;
                };
                frame.draw_paragraph(paragraph, moved(*rect, shift), theme);
                self.draw_matches(frame, block, shift, theme);
                self.draw_selection(frame, block, shift, theme);
            }
        });
    }

    /// Tells the view whether it has the keyboard: it takes keys only
    /// while it does (a [`FocusTree`](crate::FocusTree) draws the focus
    /// ring around it).
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.scroll.set_focused(focused);
    }

    /// Whether the app last said it has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }

    /// Asks for the next frame of a scroll still gliding, returning when
    /// it is due; `None` at rest. Call it from
    /// [`Application::about_to_wait`](crate::Application::about_to_wait).
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.scroll.about_to_wait(cx)
    }
}

impl Outcome for RichTextResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Selection Implementations
// ========================================================================

impl Selection {
    /// Its ends in reading order.
    fn ordered(self) -> (TextPos, TextPos) {
        match self.anchor.cmp(&self.focus) {
            Ordering::Greater => (self.focus, self.anchor),
            Ordering::Equal | Ordering::Less => (self.anchor, self.focus),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The ends of `range` in paragraph `block` as places in the text.
fn positions(block: usize, range: Range<usize>) -> (TextPos, TextPos) {
    let at = |offset| TextPos { block, offset };
    (at(range.start), at(range.end))
}

/// `rect` in the content moved by `shift` into the window.
fn moved(rect: Rect, shift: Point) -> Rect {
    Rect::new(rect.x + shift.x, rect.y + shift.y, rect.width, rect.height)
}

/// Draws `decor` moved by `shift` in `theme`'s colors, when `in_sight`
/// says its rect shows.
fn draw_decor(
    frame: &mut Frame,
    decor: &Decor,
    shift: Point,
    in_sight: &impl Fn(&Rect) -> bool,
    theme: &Theme,
) {
    match decor {
        Decor::Fill(fill) if in_sight(&fill.rect) => {
            frame.fill_rect(moved(fill.rect, shift), theme.color(fill.color));
        }
        Decor::Frame(line) if in_sight(&line.rect) => {
            let style = BoxStyle {
                border: Some(Border {
                    color: Rgba::from(theme.color(line.color)),
                    width: logical(DECOR_BORDER, frame.scale_factor()),
                }),
                fill: Rgba::new(0, 0, 0, 0),
                corner_radius: 0.0,
                shadow: None,
            };
            frame.draw_box(moved(line.rect, shift), &style);
        }
        Decor::Image(image) if in_sight(&image.rect) => {
            frame.draw_image(&image.image, moved(image.rect, shift));
        }
        Decor::Text(text) if in_sight(&text.rect) => {
            frame.draw_paragraph(&text.paragraph, moved(text.rect, shift), theme);
        }
        Decor::Fill(_) | Decor::Frame(_) | Decor::Image(_) | Decor::Text(_) => {}
    }
}

/// Whether `c` is part of a word: a letter, a digit, or an underscore.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word around `offset` in `text`, or the run of spaces or of other
/// characters there, as a double click selects.
fn word_range(text: &str, offset: usize) -> Range<usize> {
    let class = |c: char| (is_word_char(c), c.is_whitespace());
    let Some(target) = text[offset..]
        .chars()
        .next()
        .or_else(|| text[..offset].chars().next_back())
        .map(class)
    else {
        return offset..offset;
    };
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| class(c) == target)
        .last()
        .map_or(offset, |(index, _)| index);
    let end = text[offset..]
        .char_indices()
        .find(|&(_, c)| class(c) != target)
        .map_or(text.len(), |(index, _)| offset + index);
    start..end
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::key::parse_key;
    use crate::base::styled::{Span, TextColor};
    use crate::base::theme::ThemeToken;
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, ui_text};
    use crate::widgets::document::DataLink;

    const TEXT: TextColor = TextColor::Token(ThemeToken::Foreground);

    const AREA: Rect = Rect::new(0.0, 0.0, 400.0, 120.0);

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    fn press(point: Point) -> Input {
        Input::PointerButton(ButtonInput::press(PointerButton::Left, point))
    }

    fn release(point: Point) -> Input {
        Input::PointerButton(ButtonInput::release(PointerButton::Left, point))
    }

    /// Two short paragraphs, the second with a link, then many lines, so
    /// the view scrolls.
    fn blocks() -> Vec<Paragraph> {
        let mut blocks = vec![
            Paragraph::plain("First paragraph here.", TEXT),
            Paragraph::plain("See ", TEXT).with_link(Span::new("the link", TEXT), "misty:link"),
        ];
        blocks.extend((0..20).map(|index| Paragraph::plain(format!("Filler line {index}."), TEXT)));
        blocks.push(Paragraph::plain("The needle is at the bottom.", TEXT));
        blocks
    }

    fn draw(view: &mut RichText, window: &TestWindow) {
        let mut frame = window.frame();
        view.draw(&mut frame, AREA, &Theme::dark());
    }

    /// A window `width` by `height` whose motion is reduced, so scrolls
    /// jump.
    fn still(width: u32, height: u32) -> TestWindow {
        let mut window = TestWindow::new(width, height);
        window.set_reduced_motion(true);
        window
    }

    /// A view of [`blocks`] drawn in [`AREA`] of `window`.
    fn view(window: &TestWindow) -> RichText {
        let mut view = RichText::new(blocks());
        draw(&mut view, window);
        view
    }

    /// Where offset `offset` of paragraph `block` is in the window, as
    /// last drawn: the middle of its caret's height.
    fn point_of(view: &RichText, block: usize, offset: usize) -> Point {
        let caret = view.caret_rect(TextPos { block, offset }).expect("drawn");
        let drawn = view.drawn.expect("drawn");
        Point::new(
            caret.x + drawn.viewport.x,
            caret.y + caret.height / 2.0 + drawn.viewport.y - drawn.offset,
        )
    }

    #[test]
    fn test_a_drag_selects_across_paragraphs_and_copy_puts_it_on_the_clipboard() {
        let mut window = still(400, 120);
        let mut view = view(&window);
        view.set_focused(true);
        let start = point_of(&view, 0, 6);
        let end = point_of(&view, 1, 3);
        assert_eq!(
            window.feed(&mut view, &press(start)),
            RichTextResponse::Handled
        );
        window.feed(&mut view, &Input::PointerMoved(end));
        window.feed(&mut view, &release(end));
        assert_eq!(
            view.selected_text().as_deref(),
            Some("paragraph here.\nSee")
        );
        let copied = window.feed(&mut view, &key("C-c"));
        assert_eq!(copied, RichTextResponse::Handled);
        assert_eq!(
            window
                .clipboard()
                .read_text(ClipboardKind::Clipboard)
                .as_deref(),
            Some("paragraph here.\nSee")
        );
        assert_eq!(
            window.feed(&mut view, &key("<esc>")),
            RichTextResponse::Handled
        );
        assert_eq!(view.selected_text(), None);
        assert_eq!(
            window.feed(&mut view, &key("C-c")),
            RichTextResponse::Ignored,
            "nothing to copy"
        );
    }

    #[test]
    fn test_a_double_click_selects_a_word_and_a_triple_click_its_paragraph() {
        let mut window = still(400, 120);
        let mut view = view(&window);
        let inside = point_of(&view, 0, 8);
        for _ in 0..2 {
            window.feed(&mut view, &press(inside));
            window.feed(&mut view, &release(inside));
        }
        assert_eq!(view.selected_text().as_deref(), Some("paragraph"));
        window.feed(&mut view, &press(inside));
        assert_eq!(
            view.selected_text().as_deref(),
            Some("First paragraph here.")
        );
    }

    #[test]
    fn test_shifted_keys_extend_the_selection_by_characters_lines_and_everything() {
        let mut window = still(400, 120);
        let mut view = view(&window);
        assert_eq!(
            window.feed(&mut view, &key("S-<right>")),
            RichTextResponse::Ignored,
            "unfocused"
        );
        view.set_focused(true);
        for _ in 0..5 {
            window.feed(&mut view, &key("S-<right>"));
        }
        assert_eq!(view.selected_text().as_deref(), Some("First"));
        window.feed(&mut view, &key("S-<end>"));
        assert_eq!(
            view.selected_text().as_deref(),
            Some("First paragraph here.")
        );
        window.feed(&mut view, &key("S-<right>"));
        window.feed(&mut view, &key("S-<down>"));
        let down = view.selection.expect("a selection").focus;
        assert_eq!(down.block, 2, "a paragraph further down");
        window.feed(&mut view, &key("S-<up>"));
        assert_eq!(view.selection.expect("a selection").focus.block, 1);
        window.feed(&mut view, &key("C-a"));
        let all: Vec<String> = blocks().iter().map(Paragraph::text).collect();
        assert_eq!(view.selected_text(), Some(all.join("\n")));
    }

    #[test]
    fn test_a_click_on_a_link_reports_it_and_a_drag_from_it_selects_instead() {
        let mut window = still(400, 120);
        let mut view = view(&window);
        let on_link = point_of(&view, 1, 7);
        window.feed(&mut view, &press(on_link));
        assert_eq!(
            window.feed(&mut view, &release(on_link)),
            RichTextResponse::Link("misty:link".to_owned())
        );
        assert_eq!(view.selected_text(), None);
        let elsewhere = point_of(&view, 0, 2);
        // Away from the first click, so this press is no double click.
        let also_on_link = point_of(&view, 1, 10);
        window.feed(&mut view, &press(also_on_link));
        window.feed(&mut view, &Input::PointerMoved(elsewhere));
        assert_eq!(
            window.feed(&mut view, &release(elsewhere)),
            RichTextResponse::Handled
        );
        assert!(view.selected_text().is_some());
    }

    #[test]
    fn test_find_counts_matches_and_stepping_to_one_scrolls_it_into_view() {
        let window = still(400, 120);
        let mut view = view(&window);
        view.set_find("the", false);
        assert_eq!(view.match_count(), 3, "The, the, the");
        view.set_find("the", true);
        assert_eq!(view.match_count(), 2);
        view.set_find("NEEDLE", false);
        assert_eq!((view.match_count(), view.current_match()), (1, Some(0)));
        draw(&mut view, &window);
        draw(&mut view, &window);
        let needle = point_of(&view, blocks().len() - 1, 4);
        assert!(view.scroll.offset().y > 0.0, "scrolled down");
        assert!(AREA.contains(needle), "the match shows: {needle:?}");
        view.set_find("filler", false);
        assert_eq!(view.match_count(), 20);
        let first = view.current_match().expect("a match");
        assert!(
            first > 0,
            "the first match in sight, not the first above it"
        );
        view.set_find("", false);
        assert_eq!((view.match_count(), view.current_match()), (0, None));
        let mut top = RichText::new(blocks());
        top.set_find("filler", false);
        assert_eq!(top.current_match(), Some(0));
        top.previous_match();
        assert_eq!(top.current_match(), Some(19), "round to the last");
        top.next_match();
        assert_eq!(top.current_match(), Some(0), "and on to the first");
    }

    #[test]
    fn test_a_word_is_a_run_of_one_character_class() {
        assert_eq!(word_range("one, two", 1), 0..3);
        assert_eq!(word_range("one, two", 3), 3..4, "a run of punctuation");
    }

    #[test]
    fn test_the_view_describes_a_document_of_paragraphs_and_its_selection() {
        let mut window = still(400, 120);
        let mut view = RichText::new(vec![
            Paragraph::plain("One.", TEXT),
            Paragraph::plain("Two.", TEXT),
        ]);
        draw(&mut view, &window);
        view.set_focused(true);
        window.feed(&mut view, &key("S-<right>"));
        let mut frame = window.frame();
        view.draw(&mut frame, AREA, &Theme::dark());
        let id = FocusId::new("doc");
        view.access(&mut frame, id);
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\" focused\n\
             \x20 Document\n\
             \x20   Paragraph\n\
             \x20     TextRun value=\"One.\"\n\
             \x20   Paragraph\n\
             \x20     TextRun value=\"Two.\"\n"
        );
        let tree = crate::testing::access_tree(&frame, "t");
        let document = crate::testing::access_node(&tree, id).expect("the document");
        let selection = document.text_selection().expect("a selection");
        assert_eq!(selection.anchor.character_index, 0);
        assert_eq!(selection.focus.character_index, 1);
        assert_eq!(selection.focus.node, node_id(id.child(0).child(0)));
    }

    #[test]
    fn test_the_selection_and_the_current_match_are_filled_behind_the_text() {
        let theme = Theme::dark();
        let mut view = RichText::new(vec![Paragraph::plain("Select some words here", TEXT)]);
        view.selection = Some(Selection {
            anchor: TextPos::default(),
            focus: TextPos {
                block: 0,
                offset: 6,
            },
        });
        view.set_find("words", false);
        let Some(snapshot) = offscreen::render(400, 120, |frame| {
            frame.clear(Rgba::from(theme.background));
            view.draw(frame, AREA, &theme);
        }) else {
            return;
        };
        let layout = view.layout(0).expect("laid out");
        let selected = layout.range_rects(0..6)[0];
        assert!(
            snapshot.contains_color(selected, theme.selection),
            "the selection"
        );
        let found = layout.range_rects(12..17)[0];
        assert!(
            snapshot.contains_color(found, theme.warning),
            "the match's frame"
        );
        let end = layout.caret_rect(22);
        let past = Rect::new(end.x + 2.0, end.y, 40.0, end.height);
        assert!(
            !snapshot.contains_color(past, theme.selection),
            "nothing past the text"
        );
    }

    // A dictionary-like entry: a heading, a paragraph with a data link, a
    // bulleted list, a table, a code block, and a boxed section.
    fn entry() -> Document {
        use crate::widgets::document::{
            Block, Container, DataLink, Heading, Inline, Link, ListBlock, TableBlock, TableRow,
        };
        let cell = |text: &str| vec![Block::Paragraph(vec![Inline::plain(text)])];
        Document::new(vec![
            Block::Heading(Heading {
                id: Some("cat".into()),
                inlines: vec![Inline::plain("cat")],
                level: 1,
            }),
            Block::Paragraph(vec![
                Inline::plain("A small animal; see "),
                Inline::plain("kitten").with_link(Link::Data(DataLink {
                    name: "data-lookup".into(),
                    value: "kitten".into(),
                })),
                Inline::plain("."),
            ]),
            Block::List(ListBlock {
                items: vec![cell("purrs"), cell("hunts mice")],
                start: None,
            }),
            Block::Table(TableBlock {
                rows: vec![
                    TableRow {
                        cells: vec![cell("Form"), cell("Meaning")],
                        header: true,
                    },
                    TableRow {
                        cells: vec![
                            cell("cat"),
                            cell("a long explanation of what a cat is, wrapping"),
                        ],
                        header: false,
                    },
                ],
            }),
            Block::CodeBlock("let cat = Cat::new();".into()),
            Block::Container(Container {
                background: Some(ThemeToken::Surface),
                blocks: cell("Usage note about the cat."),
                border: Some(ThemeToken::Border),
                id: Some("usage".into()),
                indent: 0,
                padded: true,
            }),
        ])
    }

    #[test]
    fn test_a_document_lays_out_its_blocks_in_reading_order_with_markers_and_columns() {
        let window = still(400, 120);
        let frame = window.frame();
        let flow = entry().lay_out(&frame, 380.0);
        assert_eq!(
            flow.paragraphs.len(),
            10,
            "heading, paragraph, 2 items, 4 cells, code, note"
        );
        assert!(
            flow.rects.windows(2).all(|pair| pair[1].y >= pair[0].y),
            "top to bottom"
        );
        let (item, body) = (flow.rects[2], flow.rects[1]);
        assert!(item.x > body.x, "a list item is set in");
        let markers: Vec<String> = flow
            .decor
            .iter()
            .filter_map(|decor| match decor {
                Decor::Text(text) => Some(text.paragraph.text()),
                Decor::Fill(_) | Decor::Frame(_) | Decor::Image(_) => None,
            })
            .collect();
        assert_eq!(markers, ["•", "•"]);
        let (form, meaning) = (flow.rects[4], flow.rects[5]);
        assert_eq!(form.y, meaning.y, "cells of a row side by side");
        assert!(form.width < meaning.width, "the short column is narrower");
        assert!(
            meaning.x + meaning.width <= 380.0,
            "the table fits the width"
        );
        assert!(flow.height >= flow.rects[9].y + flow.rects[9].height);
    }

    #[test]
    fn test_find_and_copy_run_across_a_documents_blocks() {
        let window = still(400, 120);
        let mut view = RichText::document(entry());
        view.set_find("cat", false);
        assert_eq!(view.match_count(), 6, "heading, two cells, code, note");
        draw(&mut view, &window);
        view.select_all();
        let text = view.selected_text().expect("a selection");
        assert!(
            text.starts_with(
                "cat\nA small animal; see kitten.\npurrs\nhunts mice\nForm\nMeaning\n"
            ),
            "{text}"
        );
        assert!(
            text.ends_with("let cat = Cat::new();\nUsage note about the cat."),
            "{text}"
        );
    }

    #[test]
    fn test_a_click_on_a_data_link_reports_its_key_and_anchors_scroll() {
        let mut window = still(400, 120);
        let mut view = RichText::document(entry());
        draw(&mut view, &window);
        let on_link = point_of(&view, 1, 22);
        window.feed(&mut view, &press(on_link));
        assert_eq!(
            window.feed(&mut view, &release(on_link)),
            RichTextResponse::Data(DataLink {
                name: "data-lookup".into(),
                value: "kitten".into()
            })
        );
        assert!(view.scroll_to_anchor("usage"));
        draw(&mut view, &window);
        let note = view.laid_out.as_ref().expect("laid out").rects[9];
        assert!(view.drawn.expect("drawn").offset > 0.0);
        assert!(view.drawn.expect("drawn").offset <= note.y);
        assert!(!view.scroll_to_anchor("nowhere"));
    }

    #[test]
    fn test_a_document_describes_its_headings_lists_and_tables() {
        let window = still(400, 600);
        let mut view = RichText::document(entry());
        let mut frame = window.frame();
        view.draw(
            &mut frame,
            Rect::new(0.0, 0.0, 400.0, 600.0),
            &Theme::dark(),
        );
        view.access(&mut frame, FocusId::new("doc"));
        let text = ui_text(&frame, "t");
        for expected in [
            "  Document\n    Heading\n      TextRun value=\"cat\"\n",
            "    ListItem\n      TextRun value=\"purrs\"\n",
            "    Table\n      Row\n        ColumnHeader\n          TextRun value=\"Form\"\n",
            "      Row\n        Cell\n          TextRun value=\"cat\"\n",
        ] {
            assert!(text.contains(expected), "{expected:?} in\n{text}");
        }
        let tree = crate::testing::access_tree(&frame, "t");
        let heading =
            crate::testing::access_node(&tree, FocusId::new("doc").child(0)).expect("the heading");
        assert_eq!(heading.level(), Some(1));
    }

    #[test]
    fn test_a_code_block_and_a_box_are_filled_under_their_text() {
        let theme = Theme::dark();
        let mut view = RichText::document(entry());
        let Some(snapshot) = offscreen::render(400, 600, |frame| {
            frame.clear(Rgba::from(theme.background));
            view.draw(frame, Rect::new(0.0, 0.0, 400.0, 600.0), &theme);
        }) else {
            return;
        };
        let rects = &view.laid_out.as_ref().expect("laid out").rects;
        let code = rects[8];
        snapshot.assert_pixel(
            code.x + code.width - 2.0,
            code.y + 1.0,
            theme.surface,
            "the code block's fill",
        );
        let note = rects[9];
        snapshot.assert_pixel(
            note.x + note.width - 1.0,
            note.y + 1.0,
            theme.surface,
            "the box's fill",
        );
    }
}
