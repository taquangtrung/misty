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
//!
//! [`RichText::set_paged`] lays the text out as pages instead, a spread of
//! one or more side by side ([`Paged`]), cut where a line would cross a
//! page's foot; [`RichText::page_count`], [`RichText::page`],
//! [`RichText::page_of`], [`RichText::page_start`], [`RichText::set_page`],
//! and [`RichText::turn_pages`] read and turn them, and
//! [`RichText::place_rect`] says where a place shows on them.
//!
//! [`RichText::set_line_height`] sets every line a multiple of its font
//! size tall, as a reader's loose lines are.
//!
//! Places in the text are [`RichTextPos`] values: a text block (each
//! paragraph, heading, code block, list item's paragraph, table cell's
//! paragraph, image, and custom block of a document, in reading order;
//! [`RichText::block_count`] and [`RichText::block_text`] list them) and a
//! byte offset into its text. With them the app:
//!
//! - lays [`Mark`]s over ranges, tinted or underlined, in sets it replaces
//!   whole by owner ([`RichText::set_marks`]), and asks which are under a
//!   point ([`RichText::marks_at`]);
//! - shows a caret of its own ([`RichText::set_caret`], shaped by
//!   [`RichText::set_caret_shape`]), reads where it is drawn
//!   ([`RichText::caret_rect`]), and scrolls a place into view
//!   ([`RichText::reveal`]);
//! - reads and sets the selection ([`RichText::selection`],
//!   [`RichText::set_selection`]);
//! - saves the place scrolled to and gives it back
//!   ([`RichText::scroll_position`], [`RichText::set_scroll_position`]),
//!   scrolls by an amount of its own ([`RichText::scroll_by`]), and reads
//!   how tall the laid out text is ([`RichText::content_height`]).
//!
//! A right press (the secondary button) over the text is
//! [`RichTextResponse::ContextPress`], for the app to open a context menu
//! at; it leaves the selection as it is. A document's custom blocks are
//! painted by the app in the rectangles [`RichText::custom_blocks`] gives
//! after each frame.

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
use super::super::base::cell::CursorShape;
use super::super::base::clock;
use super::super::base::color::Rgba;
use super::super::base::cursor::CursorIcon;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::styled::TextColor;
use super::super::base::syntax::SyntaxColors;
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::platform::clipboard::{ClipboardKind, HAS_PRIMARY_SELECTION, SystemClipboard};
use super::super::render::icon::logical;
use super::super::render::paragraph::{Paragraph, ParagraphLayout};
use super::document::{
    BlockRole, DataLink, Decor, Document, Flow, FlowStyle, HighlightedCode, Link,
};
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

/// The alpha a mark's tint in a theme color is filled at: light enough to
/// read the text through.
const MARK_ALPHA: u8 = 0x60;

/// The thickness of a mark's underline, in logical pixels.
const MARK_UNDERLINE: f32 = 2.0;

/// The alpha a block caret is filled at under the text, so the character
/// it covers stays readable.
const BLOCK_CARET_ALPHA: u8 = 0xa0;

/// The width of a bar caret and the thickness of an underline or hollow
/// caret, in logical pixels.
const CARET_LINE: f32 = 2.0;

/// How wide a block caret past the end of its text is, in lines.
const EMPTY_CARET_LINES: f32 = 0.5;

/// The room between two pages of a spread unless the app sets its own,
/// in logical pixels.
const PAGE_GAP: f32 = 32.0;

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
    /// The app's caret, if it shows one.
    caret: Option<RichTextPos>,
    /// How the app's caret is drawn.
    caret_shape: CursorShape,
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
    /// The first page shown in paged mode, the left of its spread.
    page: usize,
    /// The first place on the page shown, to find the page again when the
    /// text is paged anew (another size, another line height).
    page_anchor: Option<RichTextPos>,
    /// How the text is laid out as pages, or `None` to scroll it.
    paged: Option<Paged>,
    /// The text blocks that start a new page when paged, ascending.
    page_breaks: Vec<usize>,
    /// The runs of blocks whose pages have their own size when paged,
    /// by first block.
    page_sizes: Vec<PageSize>,
    /// Where each column of the text showed in the last frame: the one
    /// viewport when scrolled, a page each when paged.
    panes: Vec<Pane>,
    /// Each highlighted code block's text block and its code.
    highlighted: Vec<(usize, HighlightedCode)>,
    /// Each plain paragraph's own line height, as the app gave it, by
    /// block; empty for a document.
    own_lines: Vec<Option<f32>>,
    /// The app's marks, set by set, each with its owner, in the order the
    /// sets were first given; each set sorted by where its marks start.
    marks: Vec<(String, Vec<Mark>)>,
    /// The link a held press began on, followed when let go over it.
    pending_link: Option<String>,
    /// A place waiting for a frame to be scrolled into view.
    pending_reveal: Option<RichTextPos>,
    /// A scroll position waiting for a frame to be scrolled to: a text
    /// block and the pixels of it above the top edge.
    pending_scroll: Option<(usize, f32)>,
    /// The selection last written to the primary selection.
    primary: Option<Selection>,
    /// Whether the current match waits for a frame to be scrolled into
    /// view.
    reveal_pending: bool,
    /// The scrolled viewport and its scrollbar.
    scroll: ScrollView,
    /// Whether the scrollbar is left out, the text taking the whole width.
    scrollbar_hidden: bool,
    /// What each block is to screen readers; empty for plain paragraphs.
    roles: Vec<BlockRole>,
    /// The document shown, or `None` for plain paragraphs.
    document: Option<Document>,
    /// The selection, its anchor where it began.
    selection: Option<Selection>,
    /// How the text is set: its line height, and its headings' weight as
    /// the theme last drawn in stresses words.
    style: FlowStyle,
    /// The syntax colors the highlighted code blocks were last colored
    /// in, or `None` when they are not colored yet.
    syntax: Option<SyntaxColors>,
    /// Each paragraph's text.
    texts: Vec<String>,
}

/// A place in a [`RichText`]'s text: a text block and a byte offset into
/// its text. The text blocks are a view's paragraphs, or a document's
/// paragraphs, headings, code blocks, list items' and table cells'
/// paragraphs, images, and custom blocks, in reading order (an image or a
/// custom block an empty one). Places order in reading order.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RichTextPos {
    /// The text block, from 0 at the top.
    pub block: usize,
    /// The byte offset into its text; a view rounds one off a character
    /// boundary down to the boundary before it, and one past the text to
    /// its end.
    pub offset: usize,
}

/// How a [`RichText`] lays its text out as pages of the size of its
/// columns, set with [`RichText::set_paged`], rather than one column it
/// scrolls: `columns` pages side by side make a spread, `gap` apart.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Paged {
    /// How many pages show side by side: 1 for a single page, 2 for a
    /// book's spread; at least 1.
    pub columns: usize,
    /// The room between two pages of a spread, in logical pixels.
    pub gap: f32,
}

/// A run of a paged [`RichText`]'s text blocks whose pages have a size of
/// their own, set with [`RichText::set_page_sizes`]: a word processor's
/// section on other paper.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PageSize {
    /// The text block the run starts at; it runs to the next run's first
    /// block, or to the end.
    pub first_block: usize,
    /// Its pages' height, in physical pixels.
    pub height: f32,
    /// Its pages' width, which its text wraps at, in physical pixels.
    pub width: f32,
}

/// How far [`RichText::scroll_by`] scrolls: down for a positive amount,
/// up for a negative one.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ScrollAmount {
    /// This many lines, each the line Up and Down scroll by: a line of the
    /// window's font.
    Lines(f32),
    /// This many physical pixels.
    Pixels(f32),
}

/// A range of a [`RichText`]'s text the app marks: a highlight, a search
/// result, a note's anchor. Marks are set in sets with
/// [`RichText::set_marks`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Mark {
    /// The app's own key for it, handed back by [`RichText::marks_at`].
    pub id: u64,
    /// The text it covers.
    pub range: Range<RichTextPos>,
    /// How it is drawn.
    pub style: MarkStyle,
}

/// How a [`Mark`] is drawn.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum MarkStyle {
    /// Filled behind the text: a theme or palette color lightly, so the
    /// text reads through it, an exact color at its own alpha.
    Tint(TextColor),
    /// A line under the text, over the glyphs, in this color.
    Underline(TextColor),
}

/// A right press (the secondary button) over a [`RichText`]'s text, for
/// the app to open a context menu at.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ContextPress {
    /// Where the pointer was in the window.
    pub point: Point,
    /// The place in the text nearest it.
    pub pos: RichTextPos,
}

/// Where a document's [`CustomBlock`](crate::widgets::CustomBlock) in sight was
/// drawn, from [`RichText::custom_blocks`], for the app to paint it.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PlacedBlock {
    /// The part of the window the view shows its text in: clip the
    /// painting to it, as the block may be scrolled partly out of sight.
    pub clip: Rect,
    /// The app's key of the block.
    pub id: u64,
    /// Where the whole block is in the window.
    pub rect: Rect,
}

/// What became of an input offered to a [`RichText`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RichTextResponse {
    /// A right press over the text: open a context menu there.
    ContextPress(ContextPress),
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

/// A part of the view showing a stretch of the content: the one viewport
/// when scrolled, a page when paged.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pane {
    /// How far down the content its top edge is.
    top: f32,
    /// Where it is in the window, as tall as the content it shows.
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
    /// The pages cut from them, once paged.
    pages: Option<Pages>,
    /// Where each one is: plain paragraphs stacked full width, a
    /// document's blocks set in or side by side.
    rects: Vec<Rect>,
}

/// The content cut into pages of one height.
#[derive(Clone, Debug, PartialEq)]
struct Pages {
    /// The page height they were cut for.
    height: f32,
    /// Each page, top to bottom; at least one.
    spans: Vec<PageSpan>,
}

/// The stretch of the content a page shows: from a line's top to the top
/// of the first line that did not fit, so no line is cut.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageSpan {
    /// Where it ends, at most a page below its top.
    bottom: f32,
    /// Where it starts.
    top: f32,
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

/// A selection: from where it began to where it ends now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Selection {
    /// Where it began.
    anchor: RichTextPos,
    /// Where it ends now, which keys move.
    focus: RichTextPos,
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
        let flow = document.collect(self.style);
        self.set_blocks(flow.paragraphs);
        self.own_lines.clear();
        self.anchors = flow.anchors;
        self.highlighted = flow.highlighted;
        self.roles = flow.roles;
        self.document = Some(document);
    }

    /// Scrolls the document's block with the anchor `id` (a heading's or a
    /// container's) to the top of the view, at once (when paged, turns to
    /// the spread holding it); `false` when the
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
        let rect = *rect;
        match self.paged {
            Some(_) => self.show(rect),
            None => self.scroll.scroll_to(Point::new(0.0, rect.y)),
        }
        true
    }

    /// Shows `blocks` instead, dropping the selection; a search finds its
    /// query in them anew.
    pub fn set_blocks(&mut self, blocks: Vec<Paragraph>) {
        self.document = None;
        self.anchors.clear();
        self.highlighted.clear();
        self.syntax = None;
        self.roles.clear();
        self.page_breaks.clear();
        self.page_sizes.clear();
        self.texts = blocks.iter().map(Paragraph::text).collect();
        self.own_lines = blocks.iter().map(Paragraph::line_height).collect();
        self.blocks = blocks;
        self.set_plain_line_heights();
        self.selection = None;
        self.laid_out = None;
        self.page = 0;
        self.page_anchor = None;
        if let Some(find) = self.find.take() {
            self.set_find(find.query(), find.is_case_sensitive());
        }
    }

    /// Sets each line of text `multiple` times its font size tall (`1.7`
    /// for loose reading text, as CSS's unitless `line-height`), or at the
    /// font's own line height for `None` (the default) or a multiple that
    /// is not a positive number. A document's text takes it throughout
    /// (headings, lists, tables, code); a paragraph the app gave its own
    /// [`Paragraph::with_line_height`] keeps that, and so do a document's
    /// blocks inside a [`Block::Styled`](crate::widgets::Block::Styled) whose
    /// [`BlockStyle::with_line_height`](crate::widgets::BlockStyle::with_line_height)
    /// sets one. Selections, carets,
    /// hit tests, and scrolling follow the taller lines; the gaps between
    /// blocks stay as they were. The place in the text, the selection,
    /// and the marks are kept.
    pub fn set_line_height(&mut self, multiple: Option<f32>) {
        let multiple = multiple.filter(|multiple| multiple.is_finite() && *multiple > 0.0);
        if multiple == self.style.line_height {
            return;
        }
        self.set_style(FlowStyle {
            line_height: multiple,
            ..self.style
        });
    }

    /// Sets the text in `style` from now on, laying it out anew: a
    /// document's paragraphs are made again, plain paragraphs take its
    /// line height.
    fn set_style(&mut self, style: FlowStyle) {
        self.style = style;
        match &self.document {
            Some(document) => {
                self.blocks = document.collect(style).paragraphs;
                self.syntax = None;
            }
            None => self.set_plain_line_heights(),
        }
        self.laid_out = None;
    }

    /// Sets each plain paragraph's line height: its own, else the view's.
    fn set_plain_line_heights(&mut self) {
        for (paragraph, own) in self.blocks.iter_mut().zip(&self.own_lines) {
            paragraph.set_line_height(own.or(self.style.line_height));
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

    /// How many text blocks there are: places run from block 0 to the
    /// one before this.
    pub fn block_count(&self) -> usize {
        self.texts.len()
    }

    /// The text of text block `block`, which [`RichTextPos::offset`]
    /// counts bytes into; `None` past the last block.
    pub fn block_text(&self, block: usize) -> Option<&str> {
        self.texts.get(block).map(String::as_str)
    }

    /// The selected range in reading order, or `None` when nothing is
    /// selected.
    pub fn selection(&self) -> Option<Range<RichTextPos>> {
        let (start, end) = self.selection.map(Selection::ordered)?;
        (start != end).then_some(start..end)
    }

    /// Selects `range` (from where it starts to where it ends, which keys
    /// then move; an end before the start selects backward), or nothing
    /// for `None`. Places past the text are pulled back to it. It does not
    /// scroll: call [`RichText::reveal`] for that.
    pub fn set_selection(&mut self, range: Option<Range<RichTextPos>>) {
        self.goal_x = None;
        self.selection = range.and_then(|range| {
            Some(Selection {
                anchor: self.clamp(range.start)?,
                focus: self.clamp(range.end)?,
            })
        });
    }

    // --------------------------------------------------------------------
    // Marks
    // --------------------------------------------------------------------

    /// Replaces the marks `owner` set (a name of the app's choosing, such
    /// as `"find"` or `"highlights"`) with `marks`; no marks removes the
    /// set. Sets are drawn in the order they were first given, later ones
    /// over earlier ones, and a mark over the marks before it in its set.
    /// Marks keep to their places as the view scrolls, and to their text
    /// blocks when the text is replaced, so an app replacing the text
    /// replaces its marks too.
    pub fn set_marks(&mut self, owner: &str, mut marks: Vec<Mark>) {
        for mark in &mut marks {
            if mark.range.start > mark.range.end {
                mark.range = mark.range.end..mark.range.start;
            }
        }
        marks.sort_by_key(|mark| mark.range.start);
        let at = self.marks.iter().position(|(name, _)| name == owner);
        match (at, marks.is_empty()) {
            (Some(at), true) => {
                self.marks.remove(at);
            }
            (Some(at), false) => self.marks[at].1 = marks,
            (None, true) => {}
            (None, false) => self.marks.push((owner.to_owned(), marks)),
        }
    }

    /// The marks drawn under `point` in the window, as last drawn, the
    /// topmost first: each one's owner and id. Empty before the first
    /// frame.
    pub fn marks_at(&self, point: Point) -> Vec<(&str, u64)> {
        let Some(laid) = &self.laid_out else {
            return Vec::new();
        };
        if self.pane_at(point).is_none() {
            return Vec::new();
        }
        let content = self.to_content(point);
        let under: Vec<usize> = laid
            .rects
            .iter()
            .enumerate()
            .filter(|(_, rect)| rect.contains(content))
            .map(|(block, _)| block)
            .collect();
        let mut hits = Vec::new();
        for (owner, marks) in &self.marks {
            for mark in marks {
                let hit = under.iter().any(|&block| {
                    self.mark_rects(mark, block)
                        .iter()
                        .any(|rect| rect.contains(content))
                });
                if hit {
                    hits.push((owner.as_str(), mark.id));
                }
            }
        }
        hits.reverse();
        hits
    }

    /// The place in the text nearest `point` in the window, as last drawn:
    /// where a click there would put the caret, for an app that drops
    /// something on a word (a note). `None` before the first frame, or
    /// when `point` is over none of the text's panes.
    pub fn place_at(&self, point: Point) -> Option<RichTextPos> {
        self.pane_at(point)?;
        self.pos_at(self.to_content(point))
    }

    /// Where `mark` covers text block `block`, in the content: a rectangle
    /// per line; empty when it does not reach the block.
    fn mark_rects(&self, mark: &Mark, block: usize) -> Vec<Rect> {
        let (Some(start), Some(end), Some(layout)) = (
            self.clamp(mark.range.start),
            self.clamp(mark.range.end),
            self.layout(block),
        ) else {
            return Vec::new();
        };
        if block < start.block || block > end.block || start == end {
            return Vec::new();
        }
        layout.range_rects(self.block_range(block, start, end))
    }

    // --------------------------------------------------------------------
    // Caret and scrolling
    // --------------------------------------------------------------------

    /// Shows the app's caret at `caret`, or none for `None` (the default):
    /// a caret the app moves itself, such as a keyboard cursor reading
    /// through the text, drawn whether or not the view has the keyboard,
    /// steady. It does not scroll: call [`RichText::reveal`] for that.
    pub fn set_caret(&mut self, caret: Option<RichTextPos>) {
        self.caret = caret;
    }

    /// Draws the app's caret as `shape` says: a bar before the character,
    /// the character's box filled ([`CursorShape::Block`], the default) or
    /// outlined, or a line under it.
    pub fn set_caret_shape(&mut self, shape: CursorShape) {
        self.caret_shape = shape;
    }

    /// Where the app's caret was drawn in the window in the last frame
    /// (scrolled out of sight or not): to scroll it into view or anchor a
    /// popup to. `None` without a caret or before the first frame.
    pub fn caret_rect(&self) -> Option<Rect> {
        let drawn = self.drawn?;
        let rect = self.caret_shape_rect(self.clamp(self.caret?)?, drawn.scale_factor)?;
        Some(moved(rect, self.shift_of(rect.y)?))
    }

    /// Scrolls the least that shows the line `pos` is on, gliding there,
    /// or when paged turns to the spread holding it; before the first
    /// frame, once it is drawn.
    pub fn reveal(&mut self, pos: RichTextPos) {
        let rect = self.clamp(pos).and_then(|pos| self.text_caret_rect(pos));
        match (self.drawn, rect) {
            (Some(_), Some(rect)) => {
                self.show(rect);
                self.pending_reveal = None;
            }
            (Some(_) | None, _) => self.pending_reveal = Some(pos),
        }
    }

    /// The place scrolled to: the text block at the top edge (the first
    /// whose bottom is below it) and how many physical pixels of it are
    /// scrolled out of sight above the edge (negative in the gap above
    /// it), to save and give back to [`RichText::set_scroll_position`];
    /// when paged, the top of the first page shown. Before the first
    /// frame, the place it was told to scroll to, or the top.
    pub fn scroll_position(&self) -> (usize, f32) {
        if let Some(position) = self.pending_scroll {
            return position;
        }
        self.position_at(self.top_offset())
    }

    /// The text block at content height `y` (the first whose bottom is
    /// below it) and how far above `y` its top is.
    fn position_at(&self, y: f32) -> (usize, f32) {
        let Some(laid) = &self.laid_out else {
            return (0, 0.0);
        };
        let block = laid
            .rects
            .iter()
            .position(|rect| rect.y + rect.height > y)
            .or_else(|| laid.rects.len().checked_sub(1));
        match block {
            Some(block) => (block, y - laid.rects[block].y),
            None => (0, 0.0),
        }
    }

    /// Scrolls, from the next frame, to text block `block` at the top edge
    /// with `offset` physical pixels of it above the edge, as
    /// [`RichText::scroll_position`] said; it may come before the first
    /// frame. The same text at the same width comes back to the same
    /// pixel; a block past the last counts from the last, and a place
    /// past the end is pulled back. When paged, the spread holding that
    /// place shows. Ask for a redraw after it.
    pub fn set_scroll_position(&mut self, block: usize, offset: f32) {
        self.pending_scroll = Some((block, offset));
        self.pending_reveal = None;
    }

    /// Scrolls by `amount`, gliding there (jumping while motion is
    /// reduced), held inside the text; says whether the view moved, so a
    /// scroll that hit the top or the bottom (or one of nothing) is
    /// `false` and the app can go on to the text before or after. While a
    /// [`RichText::set_scroll_position`] waits for a frame, the amount is
    /// added to it, and the answer is whether there was any amount.
    /// `false` before the first frame otherwise, and when paged (turn the
    /// pages with [`RichText::turn_pages`]). Ask for a redraw after it.
    pub fn scroll_by(&mut self, amount: ScrollAmount) -> bool {
        if self.paged.is_some() {
            return false;
        }
        let line = self.laid_out.as_ref().map_or(0.0, |laid| laid.key.line);
        let delta = match amount {
            ScrollAmount::Lines(lines) => lines * line,
            ScrollAmount::Pixels(pixels) => pixels,
        };
        if !delta.is_finite() {
            return false;
        }
        if let Some((block, offset)) = self.pending_scroll {
            self.pending_scroll = Some((block, offset + delta));
            return delta != 0.0;
        }
        if self.drawn.is_none() {
            return false;
        }
        self.pending_reveal = None;
        self.scroll.scroll_by(delta)
    }

    /// How tall the text is laid out at the width last drawn, in physical
    /// pixels, gaps between blocks included: with the view's height, how
    /// far it scrolls. `None` before the first frame.
    pub fn content_height(&self) -> Option<f32> {
        self.laid_out.as_ref().map(|laid| laid.height)
    }

    /// Leaves the scrollbar out (`true`), the text laid out across the
    /// whole width, as for columns side by side that would each show one;
    /// keys, the wheel, and [`RichText::scroll_by`] still scroll it. The
    /// scrollbar shows by default.
    pub fn set_scrollbar_hidden(&mut self, hidden: bool) {
        self.scrollbar_hidden = hidden;
        self.scroll.set_bars_hidden(hidden);
    }

    /// `pos` held to the text: a block past the last at the end of the
    /// last, an offset past its block's text at its end, and one off a
    /// character boundary at the boundary before it. `None` without text
    /// blocks.
    fn clamp(&self, pos: RichTextPos) -> Option<RichTextPos> {
        let last = self.texts.len().checked_sub(1)?;
        if pos.block > last {
            return Some(RichTextPos {
                block: last,
                offset: self.texts[last].len(),
            });
        }
        let text = &self.texts[pos.block];
        let mut offset = pos.offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        Some(RichTextPos { offset, ..pos })
    }

    // --------------------------------------------------------------------
    // Pages
    // --------------------------------------------------------------------

    /// Lays the text out as pages (`Some`), as [`Paged`] says, or scrolls
    /// it in one column (`None`, the default), keeping the place read.
    /// Paged, the view's area holds a spread of `columns` pages side by
    /// side, each a column wide (the area's width less the gaps, shared
    /// out) and as tall as the area, cut where a line would cross a
    /// page's foot so no line is split: a page ends at the top of the
    /// first line that does not fit, and the next begins at that line
    /// (or at the block after a gap), and an image or custom block that
    /// fits a page moves whole to the next. No scrollbar shows. Space and
    /// Page Down turn to the next spread, Shift+Space and Page Up to the
    /// one before, Home and End to the first and the last, while the view
    /// has the keyboard; the wheel is left to the app, to turn pages with
    /// [`RichText::turn_pages`] at the pace it likes. Selecting, marks,
    /// the caret, find (turning to each match's spread), links, custom
    /// blocks, and screen readers work on every page shown as they do
    /// scrolled.
    pub fn set_paged(&mut self, paged: Option<Paged>) {
        let paged = paged.map(|paged| Paged {
            columns: paged.columns.max(1),
            ..paged
        });
        match (self.paged.is_some(), paged.is_some()) {
            (false, true) => {
                let top = self.top_offset();
                self.page_anchor = self.pos_at(Point::new(0.0, top + LINE_NUDGE));
            }
            (true, false) => {
                let top = self.top_offset();
                if self.laid_out.is_some() && self.pending_scroll.is_none() {
                    self.pending_scroll = Some(self.position_at(top));
                }
            }
            (false, false) | (true, true) => {}
        }
        // Paged anew, so the place kept is found on the new pages; page
        // sizes lay the text out at their widths only while paged.
        if let Some(laid) = &mut self.laid_out {
            laid.pages = None;
        }
        if !self.page_sizes.is_empty() && self.paged.is_some() != paged.is_some() {
            self.laid_out = None;
        }
        self.paged = paged;
    }

    /// Starts each of the text blocks `blocks` on a new page when paged,
    /// as a word processor's page breaks do; scrolled, they change
    /// nothing. Set after the text: showing other text drops them.
    pub fn set_page_breaks(&mut self, mut blocks: Vec<usize>) {
        blocks.sort_unstable();
        blocks.dedup();
        if blocks == self.page_breaks {
            return;
        }
        self.page_breaks = blocks;
        if let Some(laid) = &mut self.laid_out {
            laid.pages = None;
        }
    }

    /// Gives runs of the text blocks pages of their own size when paged,
    /// as a word processor's sections on other paper: each run's text
    /// wraps at its width and is cut into pages its height, and starts on
    /// a page of its own. Blocks before the first run take the size the
    /// view is drawn in. A spread puts each page at its own size, side by
    /// side from the left of the area drawn in, which is otherwise
    /// ignored. Scrolled, they change nothing. Set after the text: showing
    /// other text drops them.
    pub fn set_page_sizes(&mut self, mut sizes: Vec<PageSize>) {
        sizes.sort_by_key(|size| size.first_block);
        sizes.dedup_by_key(|size| size.first_block);
        if sizes == self.page_sizes {
            return;
        }
        self.page_sizes = sizes;
        if self.paged.is_some() {
            self.laid_out = None;
        }
    }

    /// Which of the runs [`RichText::set_page_sizes`] gave page `page` is
    /// in, by its place among them; `None` before a frame drawn paged,
    /// past the last page, and on a page before the first run.
    pub fn page_size_of(&self, page: usize) -> Option<usize> {
        self.paged?;
        let span = self.laid_out.as_ref()?.pages.as_ref()?.spans.get(page)?;
        self.run_at(span.top)
    }

    /// The first page shown, the left one of its spread (a multiple of
    /// [`Paged::columns`]), counting from 0, as last drawn or as
    /// [`RichText::set_page`] or [`RichText::turn_pages`] set it since.
    /// 0 when not paged.
    pub fn page(&self) -> usize {
        match self.paged {
            Some(_) => self.page,
            None => 0,
        }
    }

    /// How many pages the text was cut into in the last frame (at least
    /// one), or `None` before a frame drawn paged.
    pub fn page_count(&self) -> Option<usize> {
        self.paged?;
        let pages = self.laid_out.as_ref()?.pages.as_ref()?;
        Some(pages.spans.len())
    }

    /// Shows the spread holding page `page` from the next frame (it may
    /// come before the first): a page past the last shows the last
    /// spread. Ask for a redraw after it.
    pub fn set_page(&mut self, page: usize) {
        self.page = page;
        self.page_anchor = None;
        self.pending_scroll = None;
        self.pending_reveal = None;
    }

    /// Turns `spreads` spreads on (back for negative), stopping at the
    /// first and the last; says whether the page shown changed, so a turn
    /// from the last spread (or back from the first) is `false` and the
    /// app can go on to the next chapter or the one before. `false`
    /// before a frame drawn paged. Ask for a redraw after it.
    pub fn turn_pages(&mut self, spreads: isize) -> bool {
        let (Some(paged), Some(count)) = (self.paged, self.page_count()) else {
            return false;
        };
        let columns = paged.columns.max(1);
        let current = spread_start(self.page.min(count - 1), columns);
        let step = spreads.saturating_mul(columns as isize);
        let target = current.saturating_add_signed(step).min(count - 1);
        let target = spread_start(target, columns);
        if target == current {
            return false;
        }
        self.set_page(target);
        true
    }

    /// The page `pos` is on, as last laid out: the page its line starts
    /// on. `None` before a frame drawn paged.
    pub fn page_of(&self, pos: RichTextPos) -> Option<usize> {
        self.paged?;
        let rect = self.text_caret_rect(self.clamp(pos)?)?;
        self.page_at(rect.y)
    }

    /// The first place on page `page`, as last laid out: where a reader's
    /// place is saved, or what a page's readout names. `None` before a
    /// frame drawn paged and past the last page.
    pub fn page_start(&self, page: usize) -> Option<RichTextPos> {
        self.paged?;
        let span = self.laid_out.as_ref()?.pages.as_ref()?.spans.get(page)?;
        self.pos_at(Point::new(0.0, span.top + LINE_NUDGE))
    }

    /// Where the caret before `pos` (a rectangle no wide, as tall as its
    /// line) was in the window in the last frame, if its line was in
    /// sight there, on any page shown: to place a label over a link, or to
    /// tell which of some places show. `None` out of sight and before the
    /// first frame.
    pub fn place_rect(&self, pos: RichTextPos) -> Option<Rect> {
        let rect = self.text_caret_rect(self.clamp(pos)?)?;
        let pane = self
            .panes
            .iter()
            .find(|pane| rect.y >= pane.top && rect.y < pane.top + pane.viewport.height)?;
        let shift = Point::new(pane.viewport.x, pane.viewport.y - pane.top);
        Some(moved(rect, shift))
    }

    /// The page holding content height `y`, as last cut.
    fn page_at(&self, y: f32) -> Option<usize> {
        let pages = self.laid_out.as_ref()?.pages.as_ref()?;
        Some(
            pages
                .spans
                .partition_point(|span| span.top <= y)
                .saturating_sub(1),
        )
    }

    /// How far down the content the view's top edge is: the scroll, or
    /// the top of the first page shown.
    fn top_offset(&self) -> f32 {
        match self.paged {
            Some(_) => self.panes.first().map_or(0.0, |pane| pane.top),
            None => self.scroll.offset().y,
        }
    }

    /// Brings `rect` of the content into sight: scrolls the least to it,
    /// or turns to the spread holding its top.
    fn show(&mut self, rect: Rect) {
        let Some(paged) = self.paged else {
            self.scroll.reveal(rect);
            return;
        };
        if let Some(page) = self.page_at(rect.y) {
            let page = spread_start(page, paged.columns.max(1));
            if page != self.page {
                self.page = page;
                self.page_anchor = None;
            }
        }
    }

    /// Turns pages by `key` while paged, as [`RichText::set_paged`] lists;
    /// ignored for any other key and when the pages cannot turn that way.
    fn turn_by_key(&mut self, key: Key) -> RichTextResponse {
        let plain = key.mods == Mods::NONE;
        let shifted = key.mods == Mods::SHIFT;
        let turned = match key.code {
            KeyCode::Char(' ') if shifted => self.turn_pages(-1),
            KeyCode::Char(' ') | KeyCode::PageDown if plain => self.turn_pages(1),
            KeyCode::PageUp if plain => self.turn_pages(-1),
            KeyCode::Home if plain => self.turn_pages(isize::MIN),
            KeyCode::End if plain => self.turn_pages(isize::MAX),
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
            | KeyCode::Up => false,
        };
        match turned {
            true => RichTextResponse::Handled,
            false => RichTextResponse::Ignored,
        }
    }

    // --------------------------------------------------------------------
    // Custom blocks
    // --------------------------------------------------------------------

    /// Where each of the document's custom blocks in sight was drawn in
    /// the last frame, top to bottom (page by page when paged, a block
    /// cut across two pages given on each): paint each one there, after
    /// drawing the view, clipped to its `clip`. Empty before the first
    /// frame.
    pub fn custom_blocks(&self) -> Vec<PlacedBlock> {
        let Some(laid) = &self.laid_out else {
            return Vec::new();
        };
        let mut placed = Vec::new();
        for pane in &self.panes {
            let shift = Point::new(pane.viewport.x, pane.viewport.y - pane.top);
            for (block, role) in self.roles.iter().enumerate() {
                let (BlockRole::Custom(id, _), Some(rect)) = (role, laid.rects.get(block)) else {
                    continue;
                };
                let rect = moved(*rect, shift);
                if rect.intersection(&pane.viewport).is_some() {
                    placed.push(PlacedBlock {
                        clip: pane.viewport,
                        id: *id,
                        rect,
                    });
                }
            }
        }
        placed
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
                self.show(rect);
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
        // The table and the row of it the blocks described are in, each
        // with the block it began at.
        let mut table: Option<(usize, usize)> = None;
        let mut row: Option<usize> = None;
        for (block, text) in self.texts.iter().enumerate() {
            let content = laid.rects.get(block).copied().unwrap_or_default();
            let shift = self.shift_of(content.y).unwrap_or_default();
            let rect = moved(content, shift);
            let role = self.roles.get(block).cloned().unwrap_or_default();
            let cell = match &role {
                BlockRole::Cell(cell) => Some(*cell),
                BlockRole::Custom(..)
                | BlockRole::Heading(_)
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
                BlockRole::Custom(_, label) => {
                    let mut node = Node::new(Role::Figure);
                    node.set_label(label.as_str());
                    frame.access(id.child(block as u64), rect, node);
                    continue;
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
    fn text_position(&self, id: FocusId, pos: RichTextPos) -> TextPosition {
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
        let response = match (self.select_by_key(key, cx.clipboard()), self.paged) {
            (Some(response), _) => response,
            (None, Some(_)) => self.turn_by_key(key),
            (None, None) => match self.scroll.handle(&Input::Key(key), cx).is_ignored() {
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
            anchor: RichTextPos::default(),
            focus: RichTextPos {
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
        if let Some(caret) = self.text_caret_rect(focus) {
            self.show(caret);
        }
    }

    /// Where `motion` takes `from`.
    fn moved(&mut self, from: RichTextPos, motion: Motion) -> RichTextPos {
        let last = self.texts.len().saturating_sub(1);
        let text = self.texts.get(from.block).map_or("", String::as_str);
        match motion {
            Motion::Left => match text[..from.offset].chars().next_back() {
                Some(c) => RichTextPos {
                    offset: from.offset - c.len_utf8(),
                    ..from
                },
                None if from.block > 0 => RichTextPos {
                    block: from.block - 1,
                    offset: self.texts[from.block - 1].len(),
                },
                None => from,
            },
            Motion::Right => match text[from.offset..].chars().next() {
                Some(c) => RichTextPos {
                    offset: from.offset + c.len_utf8(),
                    ..from
                },
                None if from.block < last => RichTextPos {
                    block: from.block + 1,
                    offset: 0,
                },
                None => from,
            },
            Motion::Start => RichTextPos::default(),
            Motion::End => RichTextPos {
                block: last,
                offset: self.texts.get(last).map_or(0, String::len),
            },
            Motion::LineStart | Motion::LineEnd => {
                let Some((caret, layout)) = self.text_caret_rect(from).zip(self.layout(from.block))
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
                RichTextPos {
                    offset: layout.offset_at(Point::new(x, middle)),
                    ..from
                }
            }
            Motion::Up | Motion::Down => self.vertical(from, motion == Motion::Down),
        }
    }

    /// The place a line below `from` (`down`) or above it, at the left
    /// edge the run of vertical moves keeps to; across paragraphs too.
    fn vertical(&mut self, from: RichTextPos, down: bool) -> RichTextPos {
        let (Some(caret), Some(laid)) = (self.text_caret_rect(from), &self.laid_out) else {
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
        // Paged, nothing scrolls: the wheel is the app's to turn pages by.
        if !selecting && self.paged.is_none() && !self.scroll.handle(input, cx).is_ignored() {
            return RichTextResponse::Handled;
        }
        let response = match input {
            Input::PointerButton(button) => match (button.button, button.pressed) {
                (PointerButton::Left, true) => {
                    self.press(button.position, button.mods.contains(Mods::SHIFT))
                }
                (PointerButton::Left, false) => self.release(button.position),
                (PointerButton::Right, true) => self.context_press(button.position),
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
        let Some(drawn) = self.drawn.filter(|_| self.pane_at(point).is_some()) else {
            return RichTextResponse::Ignored;
        };
        let Some(pos) = self.pos_at(self.to_content(point)) else {
            return RichTextResponse::Ignored;
        };
        let clicks = Clicks::after(self.clicks, clock::now(), point, drawn.scale_factor);
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

    /// A right press at `point`: the place under it, for a context menu,
    /// when it is over the text.
    fn context_press(&self, point: Point) -> RichTextResponse {
        if self.pane_at(point).is_none() {
            return RichTextResponse::Ignored;
        }
        match self.pos_at(self.to_content(point)) {
            Some(pos) => RichTextResponse::ContextPress(ContextPress { point, pos }),
            None => RichTextResponse::Ignored,
        }
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
        let outside = self.drawn.is_some() && self.pane_at(point).is_none();
        if outside
            && self.paged.is_none()
            && let Some(caret) = self.text_caret_rect(pos)
        {
            self.scroll.reveal(caret);
        }
        RichTextResponse::Handled
    }

    /// Follows the pointer to `point` (`None` outside the window): a hand
    /// over a link, a text beam over the rest of the text.
    fn hover(&mut self, point: Option<Point>, cx: &mut EventCx<'_>) {
        let over = point.filter(|point| self.pane_at(*point).is_some());
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

    /// How far the content at height `y` was moved into the window in the
    /// last frame: by the pane showing it, else the first (a place out of
    /// sight lies above or below it); `None` before the first frame.
    fn shift_of(&self, y: f32) -> Option<Point> {
        let pane = self
            .panes
            .iter()
            .find(|pane| y >= pane.top && y < pane.top + pane.viewport.height)
            .or_else(|| self.panes.first())?;
        Some(Point::new(pane.viewport.x, pane.viewport.y - pane.top))
    }

    /// The pane `point` in the window is over, as last drawn.
    fn pane_at(&self, point: Point) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.viewport.contains(point))
    }

    /// Where the app's caret at `pos` is drawn in the content, in its
    /// shape, on a display scaled by `scale_factor`: a bar before the
    /// character, or the character's box (as wide as half a line past the
    /// end of the text), or a line along that box's bottom.
    fn caret_shape_rect(&self, pos: RichTextPos, scale_factor: f64) -> Option<Rect> {
        let layout = self.layout(pos.block)?;
        let caret = layout.caret_rect(pos.offset);
        let line = logical(CARET_LINE, scale_factor);
        let text = &self.texts[pos.block];
        let next = text[pos.offset..]
            .chars()
            .next()
            .map(|c| pos.offset + c.len_utf8());
        let cell = next
            .and_then(|next| layout.range_rects(pos.offset..next).into_iter().next())
            .unwrap_or_else(|| {
                let width = (caret.height * EMPTY_CARET_LINES).round();
                Rect::new(caret.x, caret.y, width, caret.height)
            });
        Some(match self.caret_shape {
            CursorShape::Bar => Rect::new(caret.x, caret.y, line, caret.height),
            CursorShape::Block | CursorShape::Hollow => cell,
            CursorShape::Underline => {
                Rect::new(cell.x, cell.y + cell.height - line, cell.width, line)
            }
        })
    }

    /// `point` in the window as a point in the content.
    fn to_content(&self, point: Point) -> Point {
        // Outside every pane (a drag past the edge), the one nearest across.
        let across = |pane: &&Pane| {
            let viewport = pane.viewport;
            (viewport.x - point.x)
                .max(point.x - (viewport.x + viewport.width))
                .max(0.0)
        };
        let pane = self.pane_at(point).or_else(|| {
            self.panes
                .iter()
                .min_by(|a, b| across(a).total_cmp(&across(b)))
        });
        match pane {
            Some(pane) => Point::new(
                point.x - pane.viewport.x,
                point.y - pane.viewport.y + pane.top,
            ),
            None => point,
        }
    }

    /// The place in the text nearest `point` in the content; `None`
    /// without paragraphs or before the first frame.
    fn pos_at(&self, point: Point) -> Option<RichTextPos> {
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
        Some(RichTextPos {
            block,
            offset: laid.layouts[block].offset_at(point),
        })
    }

    /// Where the caret before `pos` is in the content.
    fn text_caret_rect(&self, pos: RichTextPos) -> Option<Rect> {
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
    fn first_visible(&self) -> RichTextPos {
        let (Some(laid), Some(drawn)) = (&self.laid_out, self.drawn) else {
            return RichTextPos::default();
        };
        let block = laid
            .rects
            .iter()
            .position(|rect| rect.y + rect.height > drawn.offset)
            .unwrap_or(0);
        RichTextPos { block, offset: 0 }
    }

    /// The blank space between two paragraphs as last laid out.
    fn gap(&self) -> f32 {
        self.laid_out
            .as_ref()
            .map_or(0.0, |laid| (laid.key.line * PARAGRAPH_GAP_LINES).round())
    }

    /// The characters of paragraph `block` between `start` and `end`.
    fn block_range(&self, block: usize, start: RichTextPos, end: RichTextPos) -> Range<usize> {
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
        if self.is_sized()
            && let Some(document) = &self.document
        {
            self.laid_out = Some(self.lay_out_sized(document, frame, key));
            return;
        }
        if let Some(document) = &self.document {
            let flow = document.lay_out(frame, width, self.style);
            self.laid_out = Some(Laid {
                decor: flow.decor,
                height: flow.height,
                key,
                layouts: flow.layouts,
                pages: None,
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
            .enumerate()
            .map(|(block, paragraph)| {
                let width = self.width_of(block, width);
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
            pages: None,
            rects,
        });
    }

    /// Whether the text is paged with runs of their own page size.
    fn is_sized(&self) -> bool {
        self.paged.is_some() && !self.page_sizes.is_empty()
    }

    /// The run of [`RichText::set_page_sizes`] block `block` is in, while
    /// paged.
    fn run_of(&self, block: usize) -> Option<usize> {
        if !self.is_sized() {
            return None;
        }
        self.page_sizes
            .iter()
            .rposition(|size| size.first_block <= block)
    }

    /// The run of [`RichText::set_page_sizes`] whose content holds `y` (a
    /// page's top), as last laid out, while paged.
    fn run_at(&self, y: f32) -> Option<usize> {
        if !self.is_sized() {
            return None;
        }
        let laid = self.laid_out.as_ref()?;
        self.page_sizes.iter().rposition(|size| {
            laid.rects
                .get(size.first_block)
                .is_some_and(|rect| rect.y <= y)
        })
    }

    /// The width block `block` wraps at: its run's page width, else
    /// `width`.
    fn width_of(&self, block: usize, width: f32) -> f32 {
        self.run_of(block)
            .map_or(width, |run| self.page_sizes[run].width)
    }

    /// `document` laid out with each run of [`RichText::set_page_sizes`]
    /// at its own width (the blocks before the first at `key`'s): laid out
    /// once a width, each run's blocks and what is drawn under them taken
    /// from the layout at its width and stacked under the run before.
    fn lay_out_sized(&self, document: &Document, frame: &Frame, key: LayoutKey) -> Laid {
        let mut flows: Vec<(f32, Flow)> = Vec::new();
        let count = self.texts.len();
        let mut starts: Vec<usize> = vec![0];
        starts.extend(
            self.page_sizes
                .iter()
                .map(|size| size.first_block)
                .filter(|&first| first > 0 && first < count),
        );
        starts.dedup();
        let (mut decor, mut layouts, mut rects) = (Vec::new(), Vec::new(), Vec::new());
        let mut cursor = 0.0;
        for (index, &start) in starts.iter().enumerate() {
            let end = starts.get(index + 1).copied().unwrap_or(count);
            let width = self.width_of(start, key.width);
            if !flows.iter().any(|(laid_at, _)| *laid_at == width) {
                flows.push((width, document.lay_out(frame, width, self.style)));
            }
            let Some((_, flow)) = flows.iter().find(|(laid_at, _)| *laid_at == width) else {
                continue;
            };
            let top = run_boundary(&flow.rects, start);
            let bottom = match end < count {
                true => run_boundary(&flow.rects, end),
                false => flow.height,
            };
            let dy = cursor - top;
            let end = end.min(flow.rects.len()).min(flow.layouts.len());
            let start = start.min(end);
            rects.extend(
                flow.rects[start..end]
                    .iter()
                    .map(|rect| Rect::new(rect.x, rect.y + dy, rect.width, rect.height)),
            );
            layouts.extend(
                flow.layouts[start..end]
                    .iter()
                    .cloned()
                    .map(|layout| layout.moved_down(dy)),
            );
            decor.extend(
                flow.decor
                    .iter()
                    .filter(|item| (top..bottom).contains(&item.rect().y))
                    .cloned()
                    .map(|item| item.moved_down(dy)),
            );
            cursor += (bottom - top).max(0.0);
        }
        Laid {
            decor,
            height: cursor,
            key,
            layouts,
            pages: None,
            rects,
        }
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// Draws the blocks `pane` shows, clipped to it: what is under the
    /// text, the text, and the marks, matches, selection, and caret on it.
    fn draw_pane(&self, frame: &mut Frame, pane: Pane, theme: &Theme) {
        let (offset, viewport) = (pane.top, pane.viewport);
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
                frame.draw_paragraph_over(paragraph, moved(*rect, shift), theme, |frame| {
                    self.draw_marks(frame, block, shift, theme, false);
                    self.draw_matches(frame, block, shift, theme);
                    self.draw_selection(frame, block, shift, theme);
                    self.draw_caret(frame, block, shift, theme, false);
                });
                self.draw_marks(frame, block, shift, theme, true);
                self.draw_caret(frame, block, shift, theme, true);
            }
        });
    }

    /// Draws the text as `paged` says in `area`: laid out a column wide,
    /// cut into pages as tall as `area`, the spread shown side by side.
    fn draw_paged(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, paged: Paged) {
        let scale_factor = frame.scale_factor();
        let columns = paged.columns.max(1);
        let gap = logical(paged.gap, scale_factor);
        let between = gap * (columns - 1) as f32;
        let width = ((area.width - between) / columns as f32).floor().max(0.0);
        self.lay_out(frame, width);
        self.color_code(&theme.syntax);
        let fresh = self.paginate(area.height.max(0.0));
        if let Some((block, offset)) = self.pending_scroll.take() {
            let top = self.laid_out.as_ref().and_then(|laid| {
                let rect = laid.rects.get(block).or_else(|| laid.rects.last())?;
                Some(rect.y)
            });
            self.page = self.page_at(top.unwrap_or(0.0) + offset).unwrap_or(0);
        } else if fresh && let Some(anchor) = self.page_anchor {
            self.page = self.page_of(anchor).unwrap_or(0);
        }
        // Drawn first, so the reveals waiting for a frame turn to their
        // pages rather than wait again.
        self.drawn = Some(Drawn {
            area,
            offset: 0.0,
            scale_factor,
            viewport: area,
        });
        if self.reveal_pending {
            self.reveal_current();
        }
        if let Some(pos) = self.pending_reveal.take() {
            self.reveal(pos);
        }
        let Some(spans) = self.laid_out.as_ref().and_then(|laid| laid.pages.as_ref()) else {
            return;
        };
        let last = spans.spans.len().saturating_sub(1);
        self.page = spread_start(self.page.min(last), columns);
        let mut panes = Vec::with_capacity(columns);
        let mut x = area.x;
        for span in spans.spans.iter().skip(self.page).take(columns) {
            let (page_width, page_height) = match self.run_at(span.top) {
                Some(run) => (self.page_sizes[run].width, self.page_sizes[run].height),
                None => (width, area.height),
            };
            let shown = (span.bottom - span.top).min(page_height).max(0.0);
            panes.push(Pane {
                top: span.top,
                viewport: Rect::new(x, area.y, page_width, shown),
            });
            x += page_width + gap;
        }
        self.panes = panes;
        if let Some(first) = self.panes.first() {
            self.drawn = Some(Drawn {
                area,
                offset: first.top,
                scale_factor,
                viewport: first.viewport,
            });
        }
        self.page_anchor = self.page_start(self.page);
        for index in 0..self.panes.len() {
            self.draw_pane(frame, self.panes[index], theme);
        }
    }

    /// Cuts the laid out text into pages `height` tall, unless it already
    /// is; says whether it cut them anew.
    fn paginate(&mut self, height: f32) -> bool {
        let Some(laid) = &self.laid_out else {
            return false;
        };
        if laid
            .pages
            .as_ref()
            .is_some_and(|pages| pages.height == height)
        {
            return false;
        }
        let mut breaks = self.page_breaks.clone();
        if self.paged.is_some() {
            breaks.extend(self.page_sizes.iter().map(|size| size.first_block));
        }
        let height_at = |top: f32| {
            self.run_at(top)
                .map_or(height, |run| self.page_sizes[run].height)
        };
        let spans = cut_pages(laid, &self.roles, &breaks, &height_at);
        if let Some(laid) = &mut self.laid_out {
            laid.pages = Some(Pages { height, spans });
        }
        true
    }

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

    /// Colors the highlighted code blocks in `syntax`, unless they already
    /// are.
    fn color_code(&mut self, syntax: &SyntaxColors) {
        if self.highlighted.is_empty() || self.syntax.as_ref() == Some(syntax) {
            return;
        }
        for (block, code) in &self.highlighted {
            if let Some(paragraph) = self.blocks.get_mut(*block) {
                // The line height it was collected in: the view's, or a
                // style's around it.
                let line_height = paragraph.line_height();
                *paragraph = code.paragraph(Some(syntax));
                paragraph.set_line_height(line_height);
            }
        }
        self.syntax = Some(syntax.clone());
    }

    /// Draws the marks over paragraph `block` drawn shifted by `shift`:
    /// their underlines when `over` (after the text), else their tints.
    fn draw_marks(&self, frame: &mut Frame, block: usize, shift: Point, theme: &Theme, over: bool) {
        let scale_factor = frame.scale_factor();
        for (_, marks) in &self.marks {
            for mark in marks {
                if mark.range.start.block > block {
                    break;
                }
                match (mark.style, over) {
                    (MarkStyle::Tint(color), false) => {
                        let fill = match color {
                            TextColor::Rgba(rgba) => rgba,
                            TextColor::Palette(_) | TextColor::Token(_) => {
                                color.resolve(theme).with_alpha(MARK_ALPHA)
                            }
                        };
                        for rect in self.mark_rects(mark, block) {
                            frame.fill_rect(moved(rect, shift), fill);
                        }
                    }
                    (MarkStyle::Underline(color), true) => {
                        let thickness = logical(MARK_UNDERLINE, scale_factor);
                        let line = color.resolve_rgba(theme);
                        for rect in self.mark_rects(mark, block) {
                            let under = Rect::new(
                                rect.x,
                                rect.y + rect.height - thickness,
                                rect.width,
                                thickness,
                            );
                            frame.fill_rect_over_text(moved(under, shift), line);
                        }
                    }
                    (MarkStyle::Tint(_), true) | (MarkStyle::Underline(_), false) => {}
                }
            }
        }
    }

    /// Draws the app's caret when it is in paragraph `block` drawn shifted
    /// by `shift`: a block or hollow caret under the text, a bar or an
    /// underline over it (`over`).
    fn draw_caret(&self, frame: &mut Frame, block: usize, shift: Point, theme: &Theme, over: bool) {
        let Some(caret) = self.caret.and_then(|caret| self.clamp(caret)) else {
            return;
        };
        if caret.block != block {
            return;
        }
        let scale_factor = frame.scale_factor();
        let Some(rect) = self
            .caret_shape_rect(caret, scale_factor)
            .map(|rect| moved(rect, shift))
        else {
            return;
        };
        match (self.caret_shape, over) {
            (CursorShape::Bar | CursorShape::Underline, true) => {
                frame.fill_rect_over_text(rect, theme.cursor);
            }
            (CursorShape::Block, false) => {
                frame.fill_rect(rect, theme.cursor.with_alpha(BLOCK_CARET_ALPHA));
            }
            (CursorShape::Hollow, false) => {
                let style = BoxStyle {
                    border: Some(Border {
                        color: Rgba::from(theme.cursor),
                        width: logical(CARET_LINE, scale_factor),
                    }),
                    corner_radius: 0.0,
                    fill: Rgba::new(0, 0, 0, 0),
                    shadow: None,
                };
                frame.draw_box(rect, &style);
            }
            (CursorShape::Bar | CursorShape::Underline, false)
            | (CursorShape::Block | CursorShape::Hollow, true) => {}
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
    /// when they are taller than it, unless it is hidden), scrolled, with the search's matches
    /// and the selection filled behind their text.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if self.style.emphasis != theme.emphasis && self.document.is_some() {
            self.set_style(FlowStyle {
                emphasis: theme.emphasis,
                ..self.style
            });
        }
        if let Some(paged) = self.paged {
            self.draw_paged(frame, area, theme, paged);
            return;
        }
        let narrow = match self.scrollbar_hidden {
            true => area.width,
            false => (area.width - logical(TRACK_WIDTH, frame.scale_factor())).max(0.0),
        };
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
        self.color_code(&theme.syntax);
        self.scroll.set_content_size(width, height);
        if let Some((block, offset)) = self.pending_scroll.take() {
            let top = self.laid_out.as_ref().and_then(|laid| {
                let rect = laid.rects.get(block).or_else(|| laid.rects.last())?;
                Some(rect.y)
            });
            self.scroll
                .scroll_to(Point::new(0.0, top.unwrap_or(0.0) + offset));
        }
        let viewport = self.scroll.draw_at(frame, area, theme, clock::now());
        self.drawn = Some(Drawn {
            area,
            offset: self.scroll.offset().y,
            scale_factor: frame.scale_factor(),
            viewport,
        });
        if self.reveal_pending {
            self.reveal_current();
        }
        if let Some(pos) = self.pending_reveal.take() {
            self.reveal(pos);
        }
        let offset = self.scroll.offset().y;
        if let Some(drawn) = &mut self.drawn {
            drawn.offset = offset;
        }
        self.panes.clear();
        self.panes.push(Pane {
            top: offset,
            viewport,
        });
        self.draw_pane(frame, self.panes[0], theme);
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
// PageSize and Paged Implementations
// ========================================================================

impl PageSize {
    /// Pages `width` by `height` physical pixels from text block
    /// `first_block` on.
    pub fn new(first_block: usize, width: f32, height: f32) -> Self {
        Self {
            first_block,
            height,
            width,
        }
    }
}

impl Paged {
    /// Pages `columns` side by side (at least one), the default gap
    /// apart.
    pub fn new(columns: usize) -> Self {
        Self {
            columns: columns.max(1),
            gap: PAGE_GAP,
        }
    }

    /// These pages `gap` logical pixels apart.
    pub fn with_gap(self, gap: f32) -> Self {
        Self {
            gap: gap.max(0.0),
            ..self
        }
    }
}

// ========================================================================
// Mark Implementations
// ========================================================================

impl Mark {
    /// A mark of the app's `id` over `range`, drawn as `style` says.
    pub fn new(id: u64, range: Range<RichTextPos>, style: MarkStyle) -> Self {
        Self { id, range, style }
    }
}

// ========================================================================
// ContextPress Implementations
// ========================================================================

// Compared by the bits of the point, as sizes are elsewhere, so a
// response stays `Eq`.
impl PartialEq for ContextPress {
    fn eq(&self, other: &Self) -> bool {
        let bits = |press: &Self| (press.point.x.to_bits(), press.point.y.to_bits());
        self.pos == other.pos && bits(self) == bits(other)
    }
}

impl Eq for ContextPress {}

// ========================================================================
// Selection Implementations
// ========================================================================

impl Selection {
    /// Its ends in reading order.
    fn ordered(self) -> (RichTextPos, RichTextPos) {
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
fn positions(block: usize, range: Range<usize>) -> (RichTextPos, RichTextPos) {
    let at = |offset| RichTextPos { block, offset };
    (at(range.start), at(range.end))
}

/// Where a run of blocks starting at block `start` begins in a layout
/// whose blocks are at `rects`: the foot of the blocks above it, so what
/// is drawn under its first block (a table's frame) comes with it; 0 for
/// the first block.
fn run_boundary(rects: &[Rect], start: usize) -> f32 {
    rects[..start.min(rects.len())]
        .iter()
        .map(|rect| rect.y + rect.height)
        .fold(0.0, f32::max)
}

/// The first page of the spread of `columns` pages holding `page`.
fn spread_start(page: usize, columns: usize) -> usize {
    page - page % columns.max(1)
}

/// `laid`'s content, whose blocks are `roles` (empty for plain
/// paragraphs), cut into pages `height` tall: each ends at the top of the
/// first line (or image, or custom block) that would cross its foot,
/// unless that is its first, which is cut through, or sooner at the top
/// of the next of the blocks `breaks`; the next starts at the first line
/// or block at or below that, what is drawn under the text (a code
/// block's fill) included. A page starting at `top` is `height_at(top)`
/// tall.
fn cut_pages(
    laid: &Laid,
    roles: &[BlockRole],
    breaks: &[usize],
    height_at: &dyn Fn(f32) -> f32,
) -> Vec<PageSpan> {
    let forced: Vec<f32> = breaks
        .iter()
        .filter_map(|&block| laid.rects.get(block).map(|rect| rect.y))
        .collect();
    // What a page may not cut, each its top and bottom: text lines, and
    // images and custom blocks whole.
    let mut atoms: Vec<(f32, f32)> = Vec::new();
    for (block, (rect, layout)) in laid.rects.iter().zip(&laid.layouts).enumerate() {
        match roles.get(block) {
            Some(BlockRole::Custom(..) | BlockRole::Image(_)) => {
                atoms.push((rect.y, rect.y + rect.height));
            }
            Some(BlockRole::Cell(_) | BlockRole::Heading(_) | BlockRole::ListItem)
            | Some(BlockRole::Paragraph)
            | None => {
                atoms.extend(
                    layout
                        .lines()
                        .map(|line| (line.top, line.top + line.height)),
                );
            }
        }
    }
    atoms.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut starts: Vec<f32> = atoms.iter().map(|atom| atom.0).collect();
    starts.extend(laid.decor.iter().map(|decor| decor.rect().y));
    starts.sort_by(f32::total_cmp);
    let end = laid.height;
    let mut spans = Vec::new();
    let mut top = 0.0;
    let mut first = 0;
    loop {
        let height = height_at(top);
        let next_break = forced.iter().copied().find(|&at| at > top);
        let cut = next_break.map_or(top + height, |at| at.min(top + height));
        let mut bottom = cut;
        while first < atoms.len() && atoms[first].0 < top {
            first += 1;
        }
        for &(atom_top, atom_bottom) in atoms[first..].iter().take_while(|atom| atom.0 < cut) {
            if atom_top > top && atom_bottom > cut {
                bottom = bottom.min(atom_top);
            }
        }
        spans.push(PageSpan { bottom, top });
        if bottom >= end || height <= 0.0 {
            return spans;
        }
        match starts.get(starts.partition_point(|start| *start < bottom)) {
            Some(next) if *next < end => top = *next,
            Some(_) | None => return spans,
        }
    }
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
    use crate::base::color::Rgb;
    use crate::base::key::parse_key;
    use crate::base::styled::{Span, TextColor, TextSize};
    use crate::base::theme::ThemeToken;
    use crate::input::event::ButtonInput;
    use crate::render::paragraph::TextAlign;
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
        let caret = view
            .text_caret_rect(RichTextPos { block, offset })
            .expect("drawn");
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
            anchor: RichTextPos::default(),
            focus: RichTextPos {
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
        let flow = entry().lay_out(&frame, 380.0, FlowStyle::default());
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

    fn at(block: usize, offset: usize) -> RichTextPos {
        RichTextPos { block, offset }
    }

    fn right_press(point: Point) -> Input {
        Input::PointerButton(ButtonInput::press(PointerButton::Right, point))
    }

    #[test]
    fn test_marks_are_replaced_by_owner_and_found_under_a_point_after_scrolling() {
        let window = still(400, 120);
        let mut view = view(&window);
        let tint = MarkStyle::Tint(TextColor::Token(ThemeToken::Warning));
        let line = MarkStyle::Underline(TextColor::Token(ThemeToken::Accent));
        view.set_marks("notes", vec![Mark::new(7, at(0, 0)..at(0, 5), tint)]);
        view.set_marks(
            "find",
            vec![
                Mark::new(1, at(0, 6)..at(0, 15), line),
                Mark::new(2, at(0, 2)..at(0, 8), line),
            ],
        );
        draw(&mut view, &window);
        assert_eq!(view.marks_at(point_of(&view, 0, 1)), [("notes", 7)]);
        assert_eq!(
            view.marks_at(point_of(&view, 0, 7)),
            [("find", 1), ("find", 2)],
            "the topmost first: the later set, and the later mark in it"
        );
        assert_eq!(
            view.marks_at(point_of(&view, 0, 3)),
            [("find", 2), ("notes", 7)]
        );
        view.set_marks("find", vec![Mark::new(3, at(22, 4)..at(22, 10), line)]);
        assert_eq!(
            view.marks_at(point_of(&view, 0, 7)),
            [],
            "the set was replaced"
        );
        view.reveal(at(22, 4));
        draw(&mut view, &window);
        assert!(view.scroll.offset().y > 0.0, "scrolled down");
        assert_eq!(view.marks_at(point_of(&view, 22, 6)), [("find", 3)]);
        view.set_marks("find", Vec::new());
        assert_eq!(view.marks_at(point_of(&view, 22, 6)), []);
        assert_eq!(view.marks.len(), 1, "an empty set is removed");
    }

    #[test]
    fn test_place_at_finds_the_place_under_a_point_after_scrolling() {
        let window = still(400, 120);
        let mut view = view(&window);
        draw(&mut view, &window);
        assert_eq!(view.place_at(point_of(&view, 0, 3)), Some(at(0, 3)));
        view.reveal(at(22, 4));
        draw(&mut view, &window);
        assert_eq!(view.place_at(point_of(&view, 22, 6)), Some(at(22, 6)));
    }

    #[test]
    fn test_a_tint_is_filled_behind_the_text_and_an_underline_drawn_under_it() {
        let theme = Theme::dark();
        let green = Rgba::new(0, 200, 0, 255);
        let red = Rgba::new(220, 0, 0, 255);
        let mut view = RichText::new(vec![Paragraph::plain("Mark some words here", TEXT)]);
        view.set_marks(
            "a",
            vec![
                Mark::new(1, at(0, 0)..at(0, 4), MarkStyle::Tint(green.into())),
                Mark::new(2, at(0, 10)..at(0, 15), MarkStyle::Underline(red.into())),
            ],
        );
        let Some(snapshot) = offscreen::render(400, 120, |frame| {
            frame.clear(Rgba::from(theme.background));
            view.draw(frame, AREA, &theme);
        }) else {
            return;
        };
        let layout = view.layout(0).expect("laid out");
        let tinted = layout.range_rects(0..4)[0];
        assert!(
            snapshot.contains_color(tinted, Rgb::new(0, 200, 0)),
            "the tint"
        );
        let underlined = layout.range_rects(10..15)[0];
        let bottom = Rect::new(
            underlined.x,
            underlined.y + underlined.height - 2.0,
            underlined.width,
            2.0,
        );
        assert!(
            snapshot.contains_color(bottom, Rgb::new(220, 0, 0)),
            "the underline"
        );
        let top = Rect::new(underlined.x, underlined.y, underlined.width, 2.0);
        assert!(
            !snapshot.contains_color(top, Rgb::new(220, 0, 0)),
            "only along the bottom"
        );
    }

    #[test]
    fn test_the_apps_caret_is_placed_at_its_offset_and_shaped() {
        let window = still(400, 120);
        let mut view = RichText::new(blocks());
        view.set_caret(Some(at(0, 6)));
        assert_eq!(view.caret_rect(), None, "before the first frame");
        draw(&mut view, &window);
        let start = point_of(&view, 0, 6);
        let block = view.caret_rect().expect("a caret");
        assert_eq!(block.x, start.x, "the box of the character after it");
        let next = point_of(&view, 0, 7);
        assert!((block.width - (next.x - start.x)).abs() < 0.5, "{block:?}");
        view.set_caret_shape(CursorShape::Bar);
        let bar = view.caret_rect().expect("a caret");
        assert_eq!((bar.x, bar.width), (start.x, 2.0));
        view.set_caret_shape(CursorShape::Underline);
        let under = view.caret_rect().expect("a caret");
        assert_eq!(under.y + under.height, block.y + block.height);
        view.set_caret(Some(at(0, 999)));
        let end = view.caret_rect().expect("held to the text");
        assert_eq!(end.x, point_of(&view, 0, 21).x);
        view.set_caret(None);
        assert_eq!(view.caret_rect(), None);
    }

    #[test]
    fn test_reveal_scrolls_a_place_into_view_even_before_the_first_frame() {
        let window = still(400, 120);
        let mut view = RichText::new(blocks());
        let last = blocks().len() - 1;
        view.reveal(at(last, 4));
        draw(&mut view, &window);
        draw(&mut view, &window);
        assert!(view.scroll.offset().y > 0.0, "scrolled down");
        assert!(AREA.contains(point_of(&view, last, 4)));
        view.reveal(at(0, 0));
        draw(&mut view, &window);
        assert_eq!(view.scroll.offset().y, 0.0, "and back up");
    }

    #[test]
    fn test_a_bar_caret_is_drawn_over_the_text_in_the_cursor_color() {
        let theme = Theme::dark();
        let mut view = RichText::new(vec![Paragraph::plain("Caret here", TEXT)]);
        view.set_caret(Some(at(0, 6)));
        view.set_caret_shape(CursorShape::Bar);
        let Some(snapshot) = offscreen::render(400, 120, |frame| {
            frame.clear(Rgba::from(theme.background));
            view.draw(frame, AREA, &theme);
        }) else {
            return;
        };
        let caret = view.caret_rect().expect("a caret");
        assert!(snapshot.contains_color(caret, theme.cursor));
    }

    #[test]
    fn test_the_app_reads_and_sets_the_selection_held_to_the_text() {
        let window = still(400, 120);
        let mut view = view(&window);
        assert_eq!(view.selection(), None);
        view.set_selection(Some(at(1, 3)..at(0, 6)));
        assert_eq!(
            view.selection(),
            Some(at(0, 6)..at(1, 3)),
            "in reading order"
        );
        assert_eq!(
            view.selected_text().as_deref(),
            Some("paragraph here.\nSee")
        );
        view.set_selection(Some(at(0, 0)..at(99, 0)));
        let last = blocks().len() - 1;
        assert_eq!(
            view.selection(),
            Some(at(0, 0)..at(last, view.texts[last].len()))
        );
        view.set_selection(None);
        assert_eq!(view.selection(), None);
    }

    #[test]
    fn test_a_right_press_reports_the_place_under_it_and_keeps_the_selection() {
        let mut window = still(400, 120);
        let mut view = view(&window);
        view.set_selection(Some(at(0, 0)..at(0, 5)));
        let point = point_of(&view, 1, 2);
        assert_eq!(
            window.feed(&mut view, &right_press(point)),
            RichTextResponse::ContextPress(ContextPress {
                point,
                pos: at(1, 2)
            })
        );
        assert_eq!(view.selected_text().as_deref(), Some("First"));
        assert_eq!(
            window.feed(&mut view, &right_press(Point::new(10.0, 500.0))),
            RichTextResponse::Ignored,
            "outside the view"
        );
    }

    #[test]
    fn test_a_custom_block_takes_the_width_and_its_height_and_keys_step_over_it() {
        use crate::widgets::document::{Block, CustomBlock, CustomHeight, HeightForWidth, Inline};
        let mut window = still(400, 300);
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let asked = std::sync::Arc::new(std::sync::Mutex::new(0.0_f32));
        let seen = std::sync::Arc::clone(&asked);
        let document = Document::new(vec![
            Block::Paragraph(vec![Inline::plain("Before.")]),
            Block::Custom(CustomBlock {
                height: CustomHeight::Aspect(0.25),
                id: 41,
                label: "A chart".into(),
            }),
            Block::Custom(CustomBlock {
                height: CustomHeight::ForWidth(HeightForWidth::new(move |width| {
                    *seen.lock().expect("unpoisoned") = width;
                    30.0
                })),
                id: 42,
                label: "A formula".into(),
            }),
            Block::Paragraph(vec![Inline::plain("After.")]),
        ]);
        let mut view = RichText::document(document);
        let mut frame = window.frame();
        view.draw(&mut frame, area, &Theme::dark());
        let placed = view.custom_blocks();
        assert_eq!(placed.iter().map(|p| p.id).collect::<Vec<_>>(), [41, 42]);
        let (chart, formula) = (placed[0].rect, placed[1].rect);
        assert_eq!(chart.height, (chart.width * 0.25).round());
        let scale = frame.scale_factor() as f32;
        assert_eq!(*asked.lock().expect("unpoisoned"), chart.width / scale);
        assert_eq!(formula.height, 30.0 * scale);
        assert!(formula.y >= chart.y + chart.height, "stacked");
        assert_eq!(placed[0].clip, view.drawn.expect("drawn").viewport);
        view.access(&mut frame, FocusId::new("doc"));
        let text = ui_text(&frame, "t");
        assert!(text.contains("Figure \"A chart\""), "{text}");
        drop(frame);
        view.set_focused(true);
        window.feed(&mut view, &key("S-<end>"));
        window.feed(&mut view, &key("S-<right>"));
        assert_eq!(view.selection().expect("a selection").end, at(1, 0));
        window.feed(&mut view, &key("S-<right>"));
        window.feed(&mut view, &key("S-<right>"));
        assert_eq!(view.selection().expect("a selection").end, at(3, 0));
        assert_eq!(view.selected_text().as_deref(), Some("Before.\n\n\n"));
    }

    #[test]
    fn test_highlighted_code_is_colored_by_the_themes_syntax_colors() {
        use crate::base::styled::FontFamily;
        use crate::widgets::code_view::Highlight;
        use crate::widgets::document::{Block, HighlightedCode};
        let code = HighlightedCode::new(
            "let x = 1;\n",
            vec![
                Highlight {
                    name: "keyword".into(),
                    range: 0..3,
                },
                Highlight {
                    name: "number".into(),
                    range: 8..9,
                },
                Highlight {
                    name: "keyword".into(),
                    range: 1..2,
                },
                Highlight {
                    name: "nothing.colors.this".into(),
                    range: 9..99,
                },
            ],
        );
        let mut view = RichText::document(Document::new(vec![Block::HighlightedCode(code)]));
        let window = still(400, 120);
        let colored = |theme: &Theme| {
            let span =
                |text: &str, color: TextColor| Span::new(text, color).family(FontFamily::Monospace);
            let syntax = |name: &str| {
                TextColor::Rgba(Rgba::from(theme.syntax.color(name).expect("a color")))
            };
            Paragraph::new()
                .with(span("let", syntax("keyword")))
                .with(span(" x = ", TEXT))
                .with(span("1", syntax("number")))
                .with(span(";", TEXT))
        };
        let dark = Theme::dark();
        view.draw(&mut window.frame(), AREA, &dark);
        assert_eq!(view.blocks[0], colored(&dark));
        assert_eq!(view.texts[0], "let x = 1;");
        let light = Theme::light();
        view.draw(&mut window.frame(), AREA, &light);
        assert_eq!(view.blocks[0], colored(&light), "recolored for a new theme");
        assert_ne!(colored(&dark), colored(&light));
    }

    #[test]
    fn test_a_scroll_position_comes_back_exactly_across_a_rebuild_before_its_first_frame() {
        let window = still(400, 120);
        let mut view = view(&window);
        view.scroll.scroll_to(Point::new(0.0, 157.0));
        draw(&mut view, &window);
        let offset = view.scroll.offset().y;
        let (block, within) = view.scroll_position();
        assert!(block > 0, "{block}");
        let top = view.laid_out.as_ref().expect("laid out").rects[block].y;
        assert_eq!(top + within, offset);
        let mut rebuilt = RichText::new(blocks());
        rebuilt.set_scroll_position(block, within);
        assert_eq!(rebuilt.scroll_position(), (block, within), "before a frame");
        draw(&mut rebuilt, &window);
        assert_eq!(rebuilt.scroll.offset().y, offset);
        assert_eq!(rebuilt.scroll_position(), (block, within));
        rebuilt.set_scroll_position(999, 0.0);
        draw(&mut rebuilt, &window);
        let laid = rebuilt.laid_out.as_ref().expect("laid out");
        assert!(rebuilt.scroll.offset().y <= laid.height, "held inside");
    }

    #[test]
    fn test_scroll_by_moves_by_pixels_and_lines_and_says_when_it_hit_an_end() {
        let window = still(400, 120);
        let mut view = RichText::new(blocks());
        assert!(!view.scroll_by(ScrollAmount::Pixels(10.0)), "not drawn");
        assert_eq!(view.content_height(), None);
        draw(&mut view, &window);
        let height = view.content_height().expect("laid out");
        let viewport = view.drawn.expect("drawn").viewport.height;
        assert!(height > viewport, "{height}");
        assert!(!view.scroll_by(ScrollAmount::Lines(-1.0)), "at the top");
        assert!(view.scroll_by(ScrollAmount::Pixels(30.0)));
        draw(&mut view, &window);
        assert_eq!(view.scroll.offset().y, 30.0);
        let line = window.frame().cell_metrics().height;
        assert!(view.scroll_by(ScrollAmount::Lines(2.0)));
        draw(&mut view, &window);
        assert_eq!(view.scroll.offset().y, 30.0 + 2.0 * line);
        assert!(view.scroll_by(ScrollAmount::Pixels(f32::MAX)));
        draw(&mut view, &window);
        assert_eq!(view.scroll.offset().y, (height - viewport).round());
        assert!(!view.scroll_by(ScrollAmount::Pixels(1.0)), "at the bottom");
        view.set_scroll_position(0, 0.0);
        assert!(
            view.scroll_by(ScrollAmount::Pixels(12.0)),
            "added to the wait"
        );
        assert_eq!(view.scroll_position(), (0, 12.0));
    }

    #[test]
    fn test_a_hidden_scrollbar_leaves_the_text_the_whole_width_and_still_scrolls() {
        let window = still(400, 120);
        let shown = view(&window);
        let narrow = shown.drawn.expect("drawn").viewport.width;
        assert!(narrow < AREA.width, "a scrollbar takes room: {narrow}");
        let mut hidden = RichText::new(blocks());
        hidden.set_scrollbar_hidden(true);
        draw(&mut hidden, &window);
        assert_eq!(hidden.drawn.expect("drawn").viewport.width, AREA.width);
        assert_eq!(
            hidden.laid_out.as_ref().expect("laid out").key.width,
            AREA.width
        );
        assert!(hidden.scroll_by(ScrollAmount::Pixels(20.0)));
        hidden.set_scrollbar_hidden(false);
        draw(&mut hidden, &window);
        assert_eq!(hidden.drawn.expect("drawn").viewport.width, narrow);
    }

    #[test]
    fn test_a_line_height_sets_every_line_taller_keeping_a_paragraph_s_own() {
        let window = still(400, 120);
        let size = window.frame().paragraph_font().size;
        let mut blocks = blocks();
        blocks[1] = blocks[1].clone().with_line_height(3.0);
        let mut view = RichText::new(blocks);
        view.set_line_height(Some(2.0));
        draw(&mut view, &window);
        let rects = view.laid_out.as_ref().expect("laid out").rects.clone();
        assert_eq!(rects[0].height, 2.0 * size);
        assert_eq!(rects[1].height, 3.0 * size, "its own kept");
        let low = view.to_content(Point::new(4.0, rects[0].y + rects[0].height - 2.0));
        assert_eq!(
            view.pos_at(low).map(|pos| pos.block),
            Some(0),
            "hit low in the line"
        );
        view.set_line_height(None);
        draw(&mut view, &window);
        let natural = view.laid_out.as_ref().expect("laid out").rects[0].height;
        assert!(natural < 2.0 * size, "{natural}");
        let mut document = RichText::document(entry());
        document.set_line_height(Some(2.0));
        draw(&mut document, &window);
        let laid = document.laid_out.as_ref().expect("laid out");
        let item = document
            .roles
            .iter()
            .position(|role| *role == BlockRole::ListItem);
        let item = laid.rects[item.expect("a list item")];
        let marker = laid.decor.iter().find_map(|decor| match decor {
            Decor::Text(text) => Some(text.rect),
            Decor::Fill(_) | Decor::Frame(_) | Decor::Image(_) => None,
        });
        assert_eq!(item.height, 2.0 * size);
        assert_eq!(
            marker.map(|rect| rect.height),
            Some(item.height),
            "its marker's line too"
        );
    }

    #[test]
    fn test_a_styled_block_s_line_height_wins_over_the_view_s_the_innermost_first() {
        use crate::widgets::document::{Block, BlockStyle, Heading, HighlightedCode, Inline};
        let paragraph = |text: &str| Block::Paragraph(vec![Inline::plain(text)]);
        let tight = BlockStyle::new().with_line_height(1.6);
        let document = Document::new(vec![
            Block::Styled(
                tight,
                vec![
                    Block::Heading(Heading {
                        id: Some("top".into()),
                        inlines: vec![Inline::plain("Title")],
                        level: 5,
                    }),
                    Block::Styled(
                        BlockStyle::new().with_line_height(3.0),
                        vec![paragraph("inner")],
                    ),
                    Block::Styled(BlockStyle::new(), vec![paragraph("inherited")]),
                    Block::HighlightedCode(HighlightedCode::new("let x = 1;", Vec::new())),
                ],
            ),
            Block::Styled(
                BlockStyle::new().with_line_height(-1.0),
                vec![paragraph("not a line height")],
            ),
        ]);
        let window = still(400, 200);
        let size = window.frame().paragraph_font().size;
        let mut view = RichText::document(document);
        view.set_line_height(Some(2.0));
        draw(&mut view, &window);
        let laid = view.laid_out.as_ref().expect("laid out");
        let heights: Vec<f32> = laid.rects.iter().map(|rect| rect.height).collect();
        assert_eq!(heights[0], 1.6 * size, "the heading's own");
        assert_eq!(heights[1], 3.0 * size, "the innermost wins");
        assert_eq!(heights[2], 1.6 * size, "an unset style inherits");
        assert_eq!(heights[4], 2.0 * size, "the view's otherwise");
        assert_eq!(
            view.blocks[3].line_height(),
            Some(1.6),
            "code keeps it once colored"
        );
        assert_eq!(view.roles[0], BlockRole::Heading(5));
        assert!(view.scroll_to_anchor("top"));
        view.set_line_height(None);
        draw(&mut view, &window);
        let laid = view.laid_out.as_ref().expect("laid out");
        assert_eq!(laid.rects[0].height, 1.6 * size, "with no view's at all");
    }

    #[test]
    fn test_a_styled_block_sets_its_headings_scale_in_place_of_the_level_s() {
        use crate::widgets::document::{Block, BlockStyle, Heading, Inline};
        let heading = |level: u8, size: TextSize| {
            Block::Heading(Heading {
                id: Some(format!("h{level}")),
                inlines: vec![Inline::styled(Span::new("Title", TEXT).size(size))],
                level,
            })
        };
        let own = BlockStyle::new().with_heading_scale(1.0);
        let document = Document::new(vec![
            heading(1, TextSize::Scale(1.0)),
            Block::Styled(
                own,
                vec![
                    heading(1, TextSize::Pixels(24.0)),
                    Block::Styled(
                        BlockStyle::new().with_line_height(1.5),
                        vec![heading(2, TextSize::Scale(2.0))],
                    ),
                    Block::Styled(
                        BlockStyle::new().with_heading_scale(0.5),
                        vec![heading(3, TextSize::Pixels(40.0))],
                    ),
                ],
            ),
        ]);
        let window = still(400, 200);
        let mut view = RichText::document(document);
        draw(&mut view, &window);
        let shown =
            |size: TextSize| Paragraph::new().with(Span::new("Title", TEXT).size(size).bold());
        assert_eq!(
            view.blocks[0],
            shown(TextSize::Scale(Heading::level_scale(1))),
            "the level's by default"
        );
        assert_eq!(
            view.blocks[1],
            shown(TextSize::Pixels(24.0)),
            "its own size"
        );
        assert_eq!(
            view.blocks[2],
            shown(TextSize::Scale(2.0)).with_line_height(1.5),
            "kept through a style that sets only a line height"
        );
        assert_eq!(
            view.blocks[3],
            shown(TextSize::Pixels(20.0)),
            "the innermost wins"
        );
        assert_eq!(view.roles[1], BlockRole::Heading(1));
        assert!(view.scroll_to_anchor("h3"));
        assert_eq!(
            Heading::level_scale(0),
            Heading::SCALES[0],
            "held to 1 to 6"
        );
        assert_eq!(Heading::level_scale(9), Heading::SCALES[5]);
    }

    #[test]
    fn test_an_aligned_block_centers_ends_or_justifies_its_text_but_not_its_code() {
        use crate::widgets::document::{Block, Inline};
        let paragraph = |text: &str| Block::Paragraph(vec![Inline::plain(text)]);
        let long = "words that wrap across the narrow column more than once ".repeat(3);
        let document = Document::new(vec![
            Block::Aligned(TextAlign::Center, vec![paragraph("centered")]),
            Block::Aligned(
                TextAlign::End,
                vec![
                    paragraph("at the end"),
                    Block::Aligned(TextAlign::Start, vec![paragraph("back at the start")]),
                    Block::CodeBlock("code".into()),
                ],
            ),
            Block::Aligned(TextAlign::Justify, vec![paragraph(&long)]),
            paragraph(&long),
        ]);
        let window = still(400, 120);
        let mut view = RichText::document(document);
        draw(&mut view, &window);
        let laid = view.laid_out.as_ref().expect("laid out");
        let start = |block: usize| laid.layouts[block].caret_rect(0).x - laid.rects[block].x;
        let end = |block: usize| {
            let text = &view.texts[block];
            let rect = laid.rects[block];
            rect.x + rect.width - laid.layouts[block].caret_rect(text.len()).x
        };
        assert!(
            start(0) > 10.0 && (start(0) - end(0)).abs() <= 1.0,
            "centered"
        );
        assert!(end(1) <= 1.0 && start(1) > 10.0, "at the end");
        assert_eq!(start(2), 0.0, "the innermost alignment wins");
        assert!(start(3) <= 1.0, "code keeps to the start");
        let first_line = |block: usize| {
            let line = laid.layouts[block].lines().next().expect("a line");
            let last = line.text.end - 1;
            laid.layouts[block].caret_rect(last).x
        };
        assert!(first_line(4) > first_line(5), "justified to the edge");
    }

    #[test]
    fn test_headings_follow_the_theme_s_emphasis_keeping_their_role_and_anchor() {
        use crate::base::theme::Emphasis;
        use crate::widgets::document::{Block, Heading, Inline};
        let document = Document::new(vec![
            Block::Heading(Heading {
                id: Some("top".into()),
                inlines: vec![
                    Inline::plain("Plain "),
                    Inline::styled(Span::new("strong", TEXT).bold()),
                ],
                level: 2,
            }),
            Block::Paragraph(vec![Inline::plain("Body")]),
        ]);
        let window = still(400, 120);
        let mut view = RichText::document(document);
        let scale = TextSize::Scale(1.5);
        let strong = Span::new("strong", TEXT).bold().size(scale);
        let heading = |plain: Span| Paragraph::new().with(plain).with(strong.clone());
        let bold = |view: &RichText| -> [bool; 2] {
            let plain = Span::new("Plain ", TEXT).size(scale);
            let shown = &view.blocks[0];
            [
                *shown == heading(plain.clone().bold()),
                *shown == heading(plain),
            ]
        };
        draw(&mut view, &window);
        assert_eq!(bold(&view), [true, false], "bold by default");
        let regular = Theme {
            emphasis: Emphasis::Regular,
            ..Theme::dark()
        };
        view.set_selection(Some(
            RichTextPos::default()..RichTextPos {
                block: 1,
                offset: 2,
            },
        ));
        view.draw(&mut window.frame(), AREA, &regular);
        assert_eq!(bold(&view), [false, true], "the app's own bold kept");
        assert_eq!(view.roles[0], BlockRole::Heading(2));
        assert!(view.scroll_to_anchor("top"));
        assert_eq!(view.selected_text().as_deref(), Some("Plain strong\nBo"));
        draw(&mut view, &window);
        assert_eq!(bold(&view), [true, false], "bold again");
    }

    /// A chapter of long wrapped paragraphs, in loose lines, paged two
    /// columns to a spread.
    fn paged_chapter(window: &TestWindow) -> RichText {
        use crate::widgets::document::{Block, CustomBlock, CustomHeight, Inline};
        let sentence = "Each line of a page shows whole and none is cut at a foot. ";
        let mut blocks: Vec<Block> = (0..12)
            .map(|index| {
                let text = format!("{index}: {}", sentence.repeat(1 + index % 4));
                Block::Paragraph(vec![Inline::plain(text)])
            })
            .collect();
        blocks.insert(
            5,
            Block::Custom(CustomBlock {
                height: CustomHeight::Fixed(70.0),
                id: 7,
                label: "figure".into(),
            }),
        );
        let mut view = RichText::document(Document::new(blocks));
        view.set_line_height(Some(1.7));
        view.set_paged(Some(Paged::new(2).with_gap(10.0)));
        draw(&mut view, window);
        view
    }

    fn spans(view: &RichText) -> Vec<PageSpan> {
        let laid = view.laid_out.as_ref().expect("laid out");
        laid.pages.as_ref().expect("paged").spans.clone()
    }

    #[test]
    fn test_pages_cut_at_lines_so_every_line_shows_whole_on_one_page() {
        let window = still(400, 120);
        let view = paged_chapter(&window);
        let spans = spans(&view);
        let count = view.page_count().expect("paged");
        assert_eq!(count, spans.len());
        assert!(count > 4, "{count}");
        let laid = view.laid_out.as_ref().expect("laid out");
        assert_eq!(
            laid.key.width,
            ((AREA.width - 10.0) / 2.0).floor(),
            "a column wide"
        );
        for pair in spans.windows(2) {
            assert!(pair[0].bottom <= pair[1].top, "in order: {pair:?}");
            assert!(pair[0].bottom - pair[0].top <= AREA.height);
        }
        for layout in &laid.layouts {
            for line in layout.lines() {
                let bottom = line.top + line.height;
                let holds = |span: &PageSpan| line.top >= span.top && bottom <= span.bottom;
                assert!(spans.iter().any(holds), "a line cut: {} {bottom}", line.top);
            }
        }
        let figure = view
            .roles
            .iter()
            .position(|role| matches!(role, BlockRole::Custom(..)));
        let figure = laid.rects[figure.expect("a custom block")];
        let whole =
            |span: &PageSpan| figure.y >= span.top && figure.y + figure.height <= span.bottom;
        assert!(spans.iter().any(whole), "moved whole to a page: {figure:?}");
        let last = spans.last().expect("a page");
        assert_eq!(
            last.bottom.min(laid.height),
            laid.height,
            "the text ends on the last"
        );
    }

    #[test]
    fn test_a_page_break_starts_its_block_at_the_top_of_a_page() {
        let window = still(400, 120);
        let mut view = paged_chapter(&window);
        let before = view.page_count().expect("paged");
        let starts = |view: &RichText, block| {
            let at = RichTextPos { block, offset: 0 };
            view.page_of(at).and_then(|page| view.page_start(page)) == Some(at)
        };
        let block = (1..12)
            .find(|&block| !starts(&view, block))
            .expect("a block starting below a page's top");
        view.set_page_breaks(vec![block, 0]);
        draw(&mut view, &window);
        let count = view.page_count().expect("paged");
        assert!(count >= before, "{count} pages, {before} before");
        assert!(starts(&view, block), "block {block} now starts its page");
        view.set_blocks(Vec::new());
        assert!(view.page_breaks.is_empty(), "other text drops them");
    }

    #[test]
    fn test_a_run_of_blocks_takes_pages_of_its_own_size() {
        use crate::widgets::document::{Block, Inline};
        let window = still(400, 300);
        let sentence = "A section on its own paper wraps at its own width. ";
        let blocks: Vec<Block> = (0..10)
            .map(|index| {
                let text = format!("{index}: {}", sentence.repeat(2 + index % 3));
                Block::Paragraph(vec![Inline::plain(text)])
            })
            .collect();
        let mut view = RichText::document(Document::new(blocks));
        view.set_paged(Some(Paged::new(1)));
        view.set_page_sizes(vec![
            PageSize::new(6, 380.0, 200.0),
            PageSize::new(0, 200.0, 100.0),
        ]);
        draw(&mut view, &window);
        let laid = view.laid_out.as_ref().expect("laid out");
        assert!(laid.rects[..6].iter().all(|rect| rect.width <= 200.0));
        assert!(laid.rects[6..].iter().all(|rect| rect.width > 200.0));
        let wide = RichTextPos {
            block: 6,
            offset: 0,
        };
        let page = view.page_of(wide).expect("on a page");
        assert_eq!(view.page_start(page), Some(wide), "the run starts a page");
        assert_eq!(view.page_size_of(page), Some(1));
        assert_eq!(view.page_size_of(0), Some(0));
        let spans = spans(&view);
        for (index, span) in spans.iter().enumerate() {
            let tall = match index < page {
                true => 100.0,
                false => 200.0,
            };
            assert!(span.bottom - span.top <= tall, "page {index}: {span:?}");
        }
        view.set_page(page);
        draw(&mut view, &window);
        assert_eq!(view.panes[0].viewport.width, 380.0, "drawn at its width");
    }

    #[test]
    fn test_pages_turn_by_spreads_keys_and_reveals_and_stop_at_the_ends() {
        let mut window = still(400, 120);
        let mut view = paged_chapter(&window);
        let count = view.page_count().expect("paged");
        assert_eq!(view.page(), 0);
        assert!(!view.turn_pages(-1), "nothing before the first spread");
        assert!(view.turn_pages(1));
        draw(&mut view, &window);
        assert_eq!(view.page(), 2);
        let start = view.page_start(2).expect("a page");
        assert_eq!(view.page_of(start), Some(2));
        assert!(view.turn_pages(isize::MAX));
        assert_eq!(
            view.page(),
            (count - 1) - (count - 1) % 2,
            "the last spread"
        );
        assert!(!view.turn_pages(1), "nothing past the last");
        view.set_page(3);
        draw(&mut view, &window);
        assert_eq!(view.page(), 2, "a spread starts at its left page");
        view.set_focused(true);
        assert_eq!(
            window.feed(&mut view, &key("<space>")),
            RichTextResponse::Handled
        );
        assert_eq!(view.page(), 4);
        window.feed(&mut view, &key("<home>"));
        assert_eq!(view.page(), 0);
        let last = RichTextPos {
            block: view.block_count() - 1,
            offset: 0,
        };
        view.reveal(last);
        draw(&mut view, &window);
        let page = view.page_of(last).expect("paged");
        assert_eq!(view.page(), page - page % 2, "revealed on its spread");
        assert!(
            !view.scroll_by(ScrollAmount::Pixels(10.0)),
            "pages do not scroll"
        );
        assert!(view.place_rect(last).is_some(), "in sight");
        assert_eq!(
            view.place_rect(RichTextPos::default()),
            None,
            "on another spread"
        );
    }

    #[test]
    fn test_a_press_on_the_right_page_selects_there_and_the_place_survives_a_resize() {
        let mut window = still(400, 120);
        let mut view = paged_chapter(&window);
        let right = view.page_start(1).expect("a right page");
        let rect = view.place_rect(right).expect("shown on the right");
        assert!(rect.x > AREA.width / 2.0, "{rect:?}");
        let point = Point::new(rect.x + 1.0, rect.y + rect.height / 2.0);
        window.feed(&mut view, &press(point));
        window.feed(&mut view, &release(point));
        let selection = view.selection.expect("pressed");
        assert_eq!(selection.focus.block, right.block);
        assert!(view.turn_pages(2));
        draw(&mut view, &window);
        let kept = view.page_start(view.page()).expect("a page");
        let tall = Rect::new(0.0, 0.0, 400.0, 90.0);
        view.draw(&mut window.frame(), tall, &Theme::dark());
        let page = view.page_of(kept).expect("paged");
        assert_eq!(view.page(), page - page % 2, "the place read is kept");
        view.set_paged(None);
        draw(&mut view, &window);
        assert_eq!(view.page_count(), None);
        let top = view.first_visible().block;
        assert!(
            top.abs_diff(kept.block) <= 1,
            "scrolled to it: {top} {}",
            kept.block
        );
    }
}
