//! [`Document`]: rich prose as blocks, for [`RichText`](crate::RichText)
//! to lay out, scroll, select, and search: a dictionary entry, a chapter
//! of a book, a Markdown preview. It is a plain model, not HTML: an app
//! builds it in code, or converts Markdown (the `markdown` feature,
//! [`Document::from_markdown`]) or a safe subset of HTML (the `html`
//! feature, [`Document::from_html`]) into it.
//!
//! A document is a list of [`Block`]s: paragraphs and headings of
//! [`Inline`] text (each a styled [`Span`], perhaps a [`Link`]), lists,
//! block quotes, code blocks, rules, tables of blocks, images, and
//! [`Container`]s that set a group of blocks in a box (indented, filled,
//! bordered), as a dictionary's section is. Headings and containers may
//! carry an id, an anchor [`RichText::scroll_to_anchor`](crate::RichText::scroll_to_anchor)
//! jumps to.
//!
//! Images are blocks of their own, scaled down to the width; an image
//! inside a paragraph's text is not supported (the text layout places
//! glyphs only).

#[cfg(any(feature = "html", feature = "markdown"))]
mod build;
#[cfg(feature = "html")]
pub(crate) mod html;
#[cfg(feature = "markdown")]
pub(crate) mod markdown;

use super::super::app::frame::Frame;
use super::super::base::geometry::Rect;
use super::super::base::styled::{FontFamily, Span, TextColor, TextSize};
use super::super::base::theme::ThemeToken;
use super::super::render::image::Image;
use super::super::render::paragraph::{Paragraph, ParagraphLayout};

// ========================================================================
// Constants
// ========================================================================

/// The size of each heading level's text, 1 to 6, as a multiple of the
/// body's.
const HEADING_SCALES: [f32; 6] = [1.8, 1.5, 1.25, 1.1, 1.0, 0.9];

/// The blank between two blocks, in lines.
const BLOCK_GAP_LINES: f32 = 0.5;

/// How far a list item, a block quote, or a container's indent step sets
/// its blocks in, in lines.
const INDENT_LINES: f32 = 1.5;

/// The padding inside a code block, a table cell, and a container, in
/// lines.
const PADDING_LINES: f32 = 0.35;

/// The marks of unordered list items, by depth, round again past the
/// last.
const BULLETS: [&str; 3] = ["•", "◦", "▪"];

/// The thickness of a rule, a quote's bar, and a table's lines, in lines.
const LINE_LINES: f32 = 0.08;

/// The narrowest a table column is laid out, in lines.
const MIN_COLUMN_LINES: f32 = 2.0;

/// What starts a link target that is a data link, which no URL starts
/// with: a private use character.
pub(crate) const DATA_PREFIX: char = '\u{e000}';

/// What separates a data link's attribute name from its value.
const DATA_SEPARATOR: char = '\u{e001}';

// ========================================================================
// Data Structures
// ========================================================================

/// Rich prose as blocks, top to bottom.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    /// The blocks, top to bottom.
    pub blocks: Vec<Block>,
}

/// A block of a [`Document`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Block {
    /// Blocks set in, behind a bar, as a quotation.
    BlockQuote(Vec<Block>),
    /// Lines of monospace text on the surface color, kept as they are.
    CodeBlock(String),
    /// Blocks in a box of their own.
    Container(Container),
    /// A heading.
    Heading(Heading),
    /// An image, scaled down to the width.
    Image(BlockImage),
    /// A list of items.
    List(ListBlock),
    /// Text that wraps, its spans styled; line feeds in a span break its
    /// line.
    Paragraph(Vec<Inline>),
    /// A horizontal line across the width.
    Rule,
    /// A table of blocks.
    Table(TableBlock),
}

/// A run of text in a paragraph or heading: a styled span, perhaps a
/// link.
#[derive(Clone, Debug, PartialEq)]
pub struct Inline {
    /// What a click on it reports, if anything.
    pub link: Option<Link>,
    /// The text and its style.
    pub span: Span,
}

/// What a click on an [`Inline`] reports.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Link {
    /// An app-defined key, such as a dictionary cross-reference or an
    /// audio button: [`RichTextResponse::Data`](crate::RichTextResponse::Data).
    Data(DataLink),
    /// A URL or another target the app resolves:
    /// [`RichTextResponse::Link`](crate::RichTextResponse::Link).
    Href(String),
}

/// An app-defined key on an [`Inline`], as an HTML `data-*` attribute
/// gives one.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DataLink {
    /// What kind of key it is, such as `data-lookup`.
    pub name: String,
    /// The key, such as the word to look up.
    pub value: String,
}

/// A group of blocks in a box: set in, padded, filled, and bordered as
/// its fields say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Container {
    /// The fill behind it, if any.
    pub background: Option<ThemeToken>,
    /// The blocks inside the box, top to bottom.
    pub blocks: Vec<Block>,
    /// The line around it, if any.
    pub border: Option<ThemeToken>,
    /// The anchor it can be scrolled to by, if any.
    pub id: Option<String>,
    /// How many indent steps it is set in.
    pub indent: u8,
    /// Whether it has padding inside its box.
    pub padded: bool,
}

/// A heading, levels 1 (the largest) to 6.
#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    /// The anchor it can be scrolled to by, if any.
    pub id: Option<String>,
    /// Its text.
    pub inlines: Vec<Inline>,
    /// Its level, 1 to 6.
    pub level: u8,
}

/// An image block.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockImage {
    /// What it shows, for screen readers.
    pub alt: String,
    /// The image, drawn at its size in logical pixels or scaled down to
    /// the width.
    pub image: Image,
}

/// A list: its items, each blocks, numbered or bulleted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListBlock {
    /// Each item's blocks, in order; an item may hold several blocks.
    pub items: Vec<Vec<Block>>,
    /// The number of the first item, for a numbered list; `None` for
    /// bullets.
    pub start: Option<u64>,
}

/// A table: rows of cells, each blocks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableBlock {
    /// The rows, top to bottom.
    pub rows: Vec<TableRow>,
}

/// A row of a [`TableBlock`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableRow {
    /// Its cells, left to right, each blocks.
    pub cells: Vec<Vec<Block>>,
    /// Whether it is a header row: its cells on the hover color.
    pub header: bool,
}

/// A document laid out at one width, in content coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Flow {
    /// The anchors, each with its first text block.
    pub(crate) anchors: Vec<(String, usize)>,
    /// What is drawn under the text: fills, lines, markers, images.
    pub(crate) decor: Vec<Decor>,
    /// The height of it all.
    pub(crate) height: f32,
    /// Each text block's layout; empty when laid out without a frame.
    pub(crate) layouts: Vec<ParagraphLayout>,
    /// Each text block's paragraph, in reading order.
    pub(crate) paragraphs: Vec<Paragraph>,
    /// Each text block's rect.
    pub(crate) rects: Vec<Rect>,
    /// What each text block is to screen readers.
    pub(crate) roles: Vec<BlockRole>,
}

/// Something drawn under a document's text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Decor {
    /// A fill of a theme color.
    Fill(DecorFill),
    /// A line around a rect, in a theme color.
    Frame(DecorFill),
    /// An image.
    Image(DecorImage),
    /// A paragraph that is not part of the text: a list item's marker.
    Text(DecorText),
}

/// A rect in a theme color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DecorFill {
    /// The color.
    pub(crate) color: ThemeToken,
    /// Where.
    pub(crate) rect: Rect,
}

/// An image over a rect.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DecorImage {
    /// The image.
    pub(crate) image: Image,
    /// Where.
    pub(crate) rect: Rect,
}

/// A paragraph over a rect.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DecorText {
    /// The paragraph.
    pub(crate) paragraph: Paragraph,
    /// Where.
    pub(crate) rect: Rect,
}

/// What a text block of a document is to screen readers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum BlockRole {
    /// A cell of a table.
    Cell(CellRole),
    /// A heading of this level.
    Heading(u8),
    /// An image, with its text for screen readers.
    Image(String),
    /// A list item's text.
    ListItem,
    /// Any other text.
    #[default]
    Paragraph,
}

/// Where a text block is in a table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CellRole {
    /// Whether its row is a header row.
    pub(crate) header: bool,
    /// Which row of the table it is in.
    pub(crate) row: usize,
    /// Which of the document's tables it is in.
    pub(crate) table: usize,
}

/// Lays a document out, or, without a frame, only collects its text.
struct Placer<'a> {
    /// What measures text; `None` to collect the text only.
    frame: Option<&'a Frame>,
    /// The height of a line of body text, the unit of every spacing.
    line: f32,
    /// The tables placed so far.
    tables: usize,
    /// What it placed.
    out: Flow,
}

/// What the blocks being placed are part of.
#[derive(Clone, Copy, Debug, Default)]
struct Context {
    /// How deep in lists they are.
    depth: usize,
    /// The table cell they are in, if any.
    cell: Option<CellRole>,
    /// Whether they are a list item's.
    item: bool,
}

// ========================================================================
// Document Implementations
// ========================================================================

impl Document {
    /// A document of `blocks`.
    pub fn new(blocks: Vec<Block>) -> Self {
        Self { blocks }
    }

    /// The document's text blocks in reading order, without measuring:
    /// their paragraphs, what they are to screen readers, and the anchors.
    pub(crate) fn collect(&self) -> Flow {
        let mut placer = Placer::new(None, 0.0);
        placer.blocks(&self.blocks, 0.0, 0.0, 0.0, Context::default());
        placer.out
    }

    /// The document laid out `width` pixels wide in `frame`'s fonts.
    pub(crate) fn lay_out(&self, frame: &Frame, width: f32) -> Flow {
        let mut placer = Placer::new(Some(frame), frame.cell_metrics().height);
        let height = placer.blocks(&self.blocks, 0.0, width, 0.0, Context::default());
        placer.out.height = height;
        placer.out
    }
}

// ========================================================================
// Inline Implementations
// ========================================================================

impl Inline {
    /// `text` in the body's color, unstyled.
    pub fn plain(text: impl Into<String>) -> Self {
        Self::styled(Span::new(text, TextColor::Token(ThemeToken::Foreground)))
    }

    /// The span `span`, not a link.
    pub fn styled(span: Span) -> Self {
        Self { link: None, span }
    }

    /// This text reporting `link` when clicked.
    pub fn with_link(self, link: Link) -> Self {
        Self {
            link: Some(link),
            ..self
        }
    }
}

// ========================================================================
// Link Implementations
// ========================================================================

impl Link {
    /// The link target a paragraph keeps for it.
    pub(crate) fn target(&self) -> String {
        match self {
            Self::Data(data) => format!("{DATA_PREFIX}{}{DATA_SEPARATOR}{}", data.name, data.value),
            Self::Href(href) => href.clone(),
        }
    }

    /// The link a paragraph's link `target` stands for.
    pub(crate) fn from_target(target: &str) -> Self {
        let Some(data) = target.strip_prefix(DATA_PREFIX) else {
            return Self::Href(target.to_owned());
        };
        let (name, value) = data.split_once(DATA_SEPARATOR).unwrap_or(("", data));
        Self::Data(DataLink {
            name: name.to_owned(),
            value: value.to_owned(),
        })
    }
}

// ========================================================================
// Placer Implementations
// ========================================================================

impl<'a> Placer<'a> {
    /// A placer measuring with `frame` (or only collecting), a line
    /// `line` pixels tall.
    fn new(frame: Option<&'a Frame>, line: f32) -> Self {
        Self {
            frame,
            line,
            out: Flow::default(),
            tables: 0,
        }
    }

    /// `lines` lines in pixels, rounded.
    fn lines(&self, lines: f32) -> f32 {
        (self.line * lines).round()
    }

    /// Places `blocks` from `top` in the column at `x`, `width` wide,
    /// a gap between each two; returns the bottom of the last.
    fn blocks(&mut self, blocks: &[Block], x: f32, width: f32, top: f32, cx: Context) -> f32 {
        let gap = self.lines(BLOCK_GAP_LINES);
        let mut bottom = top;
        for (index, block) in blocks.iter().enumerate() {
            let start = match index {
                0 => top,
                _ => bottom + gap,
            };
            bottom = self.block(block, x, width, start, cx);
        }
        bottom
    }

    /// Places `block` at `top` in the column at `x`, `width` wide; returns
    /// its bottom.
    fn block(&mut self, block: &Block, x: f32, width: f32, top: f32, cx: Context) -> f32 {
        match block {
            Block::BlockQuote(blocks) => {
                let indent = self.lines(INDENT_LINES);
                let bottom = self.blocks(blocks, x + indent, width - indent, top, cx);
                let bar = self.lines(LINE_LINES).max(1.0) * 2.0;
                self.decor(Decor::Fill(DecorFill {
                    color: ThemeToken::Border,
                    rect: Rect::new(x, top, bar, bottom - top),
                }));
                bottom
            }
            Block::CodeBlock(code) => {
                let pad = self.lines(PADDING_LINES);
                let span = Span::new(
                    code.trim_end_matches('\n'),
                    TextColor::Token(ThemeToken::Foreground),
                )
                .family(FontFamily::Monospace);
                let paragraph = Paragraph::new().with(span);
                let fill = self.out.decor.len();
                let bottom = self.text(
                    paragraph,
                    self.role(cx),
                    x + pad,
                    width - 2.0 * pad,
                    top + pad,
                );
                let rect = Rect::new(x, top, width, bottom + pad - top);
                self.out.decor.insert(
                    fill,
                    Decor::Fill(DecorFill {
                        color: ThemeToken::Surface,
                        rect,
                    }),
                );
                rect.y + rect.height
            }
            Block::Container(container) => self.container(container, x, width, top, cx),
            Block::Heading(heading) => {
                let level = heading.level.clamp(1, HEADING_SCALES.len() as u8);
                let scale = HEADING_SCALES[usize::from(level) - 1];
                if let Some(id) = &heading.id {
                    self.out
                        .anchors
                        .push((id.clone(), self.out.paragraphs.len()));
                }
                let paragraph = paragraph_of(&heading.inlines, |span| {
                    let size = match span.size {
                        TextSize::Pixels(pixels) => TextSize::Pixels(pixels * scale),
                        TextSize::Scale(own) => TextSize::Scale(own * scale),
                    };
                    span.size(size).bold()
                });
                self.text(paragraph, BlockRole::Heading(level), x, width, top)
            }
            Block::Image(image) => {
                let scale = self.frame.map_or(1.0, |frame| frame.scale_factor() as f32);
                let natural = image.image.width() as f32 * scale;
                let shown = natural.min(width.max(0.0));
                let height = match image.image.width() {
                    0 => 0.0,
                    w => (shown * image.image.height() as f32 / w as f32).round(),
                };
                let rect = Rect::new(x, top, shown, height);
                self.decor(Decor::Image(DecorImage {
                    image: image.image.clone(),
                    rect,
                }));
                self.out.paragraphs.push(Paragraph::new());
                self.out.roles.push(BlockRole::Image(image.alt.clone()));
                self.out.rects.push(rect);
                if let Some(frame) = self.frame {
                    self.out.layouts.push(
                        frame.paragraph_layout(&Paragraph::new(), Rect::new(x, top, shown, 0.0)),
                    );
                }
                top + height
            }
            Block::List(list) => self.list(list, x, width, top, cx),
            Block::Paragraph(inlines) => {
                let paragraph = paragraph_of(inlines, |span| span);
                self.text(paragraph, self.role(cx), x, width, top)
            }
            Block::Rule => {
                let thickness = self.lines(LINE_LINES).max(1.0);
                let middle = (top + self.lines(BLOCK_GAP_LINES) / 2.0).round();
                self.decor(Decor::Fill(DecorFill {
                    color: ThemeToken::Border,
                    rect: Rect::new(x, middle, width, thickness),
                }));
                top + self.lines(BLOCK_GAP_LINES)
            }
            Block::Table(table) => self.table(table, x, width, top, cx),
        }
    }

    /// Places a container's blocks in its box.
    fn container(
        &mut self,
        container: &Container,
        x: f32,
        width: f32,
        top: f32,
        cx: Context,
    ) -> f32 {
        let indent = self.lines(INDENT_LINES) * f32::from(container.indent);
        let pad = match container.padded {
            true => self.lines(PADDING_LINES),
            false => 0.0,
        };
        if let Some(id) = &container.id {
            self.out
                .anchors
                .push((id.clone(), self.out.paragraphs.len()));
        }
        let first = self.out.decor.len();
        let (left, inner) = (x + indent, width - indent);
        let bottom = self.blocks(
            &container.blocks,
            left + pad,
            inner - 2.0 * pad,
            top + pad,
            cx,
        ) + pad;
        let rect = Rect::new(left, top, inner, bottom - top);
        let mut boxes = Vec::new();
        if let Some(color) = container.background {
            boxes.push(Decor::Fill(DecorFill { color, rect }));
        }
        if let Some(color) = container.border {
            boxes.push(Decor::Frame(DecorFill { color, rect }));
        }
        self.out.decor.splice(first..first, boxes);
        bottom
    }

    /// Places a list's items, each with its marker at the left.
    fn list(&mut self, list: &ListBlock, x: f32, width: f32, top: f32, cx: Context) -> f32 {
        let indent = self.lines(INDENT_LINES);
        let gap = self.lines(BLOCK_GAP_LINES);
        let item_cx = Context {
            depth: cx.depth + 1,
            item: true,
            ..cx
        };
        let mut bottom = top;
        for (index, item) in list.items.iter().enumerate() {
            let start = match index {
                0 => top,
                _ => bottom + gap,
            };
            let marker = match list.start {
                Some(first) => format!("{}.", first + index as u64),
                None => BULLETS[cx.depth % BULLETS.len()].to_owned(),
            };
            let paragraph = Paragraph::plain(marker, TextColor::Token(ThemeToken::Muted));
            self.decor(Decor::Text(DecorText {
                paragraph,
                rect: Rect::new(x, start, indent, self.line),
            }));
            bottom = self.blocks(item, x + indent, width - indent, start, item_cx);
            bottom = bottom.max(start + self.line);
        }
        bottom
    }

    /// Places a table's cells, columns as wide as their text wants within
    /// the width, lined in the border color, header rows filled.
    fn table(&mut self, table: &TableBlock, x: f32, width: f32, top: f32, cx: Context) -> f32 {
        let index = self.tables;
        self.tables += 1;
        let columns = table
            .rows
            .iter()
            .map(|row| row.cells.len())
            .max()
            .unwrap_or(0);
        if columns == 0 {
            return top;
        }
        let pad = self.lines(PADDING_LINES);
        let widths = self.column_widths(table, columns, width, pad);
        let mut y = top;
        for (row_index, row) in table.rows.iter().enumerate() {
            let first = self.out.decor.len();
            let mut cell_x = x;
            let mut row_bottom = y + self.line;
            let mut cells = Vec::with_capacity(columns);
            for (column, cell) in row.cells.iter().enumerate() {
                let cell_cx = Context {
                    cell: Some(CellRole {
                        header: row.header,
                        row: row_index,
                        table: index,
                    }),
                    ..cx
                };
                let inner = widths[column] - 2.0 * pad;
                let bottom = self.blocks(cell, cell_x + pad, inner, y + pad, cell_cx) + pad;
                row_bottom = row_bottom.max(bottom);
                cells.push(Rect::new(cell_x, y, widths[column], 0.0));
                cell_x += widths[column];
            }
            let mut boxes = Vec::new();
            for mut rect in cells {
                rect.height = row_bottom - y;
                if row.header {
                    boxes.push(Decor::Fill(DecorFill {
                        color: ThemeToken::Hover,
                        rect,
                    }));
                }
                boxes.push(Decor::Frame(DecorFill {
                    color: ThemeToken::Border,
                    rect,
                }));
            }
            self.out.decor.splice(first..first, boxes);
            y = row_bottom;
        }
        y
    }

    /// How wide each of a table's `columns` is in `width`: as wide as its
    /// widest text on one line, shrunk in proportion when they do not all
    /// fit, none narrower than [`MIN_COLUMN_LINES`].
    fn column_widths(&self, table: &TableBlock, columns: usize, width: f32, pad: f32) -> Vec<f32> {
        let even = vec![width / columns as f32; columns];
        let Some(frame) = self.frame else {
            return even;
        };
        let mut natural = vec![0.0_f32; columns];
        for row in &table.rows {
            for (column, cell) in row.cells.iter().enumerate() {
                let widest = cell
                    .iter()
                    .map(|block| match block {
                        Block::Paragraph(inlines) => {
                            frame.paragraph_width(&paragraph_of(inlines, |span| span))
                        }
                        Block::BlockQuote(_)
                        | Block::CodeBlock(_)
                        | Block::Container(_)
                        | Block::Heading(_)
                        | Block::Image(_)
                        | Block::List(_)
                        | Block::Rule
                        | Block::Table(_) => width / columns as f32,
                    })
                    .fold(0.0, f32::max);
                natural[column] = natural[column].max(widest.ceil() + 2.0 * pad);
            }
        }
        let min = self.lines(MIN_COLUMN_LINES) + 2.0 * pad;
        let total: f32 = natural.iter().sum();
        if total <= width {
            return natural
                .into_iter()
                .map(|natural| natural.max(min))
                .collect();
        }
        natural
            .into_iter()
            .map(|natural| (width * natural / total).max(min).floor())
            .collect()
    }

    /// What a text block in `cx` is to screen readers.
    fn role(&self, cx: Context) -> BlockRole {
        match (cx.cell, cx.item) {
            (Some(cell), _) => BlockRole::Cell(cell),
            (None, true) => BlockRole::ListItem,
            (None, false) => BlockRole::Paragraph,
        }
    }

    /// Adds `decor` over what was drawn under the text so far.
    fn decor(&mut self, decor: Decor) {
        self.out.decor.push(decor);
    }

    /// Places `paragraph`, a text block that is `role`, at `top` in the
    /// column at `x`, `width` wide; returns its bottom, at least a line
    /// below its top.
    fn text(&mut self, paragraph: Paragraph, role: BlockRole, x: f32, width: f32, top: f32) -> f32 {
        let width = width.max(0.0);
        let height = match self.frame {
            Some(frame) => {
                let layout = frame.paragraph_layout(&paragraph, Rect::new(x, top, width, 0.0));
                let height = layout.height().max(self.line);
                self.out.layouts.push(layout);
                height
            }
            None => 0.0,
        };
        self.out.paragraphs.push(paragraph);
        self.out.roles.push(role);
        self.out.rects.push(Rect::new(x, top, width, height));
        top + height
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The paragraph of `inlines`, each span passed through `style`.
fn paragraph_of(inlines: &[Inline], style: impl Fn(Span) -> Span) -> Paragraph {
    inlines.iter().fold(Paragraph::new(), |paragraph, inline| {
        let span = style(inline.span.clone());
        match &inline.link {
            Some(link) => paragraph.with_link(span, link.target()),
            None => paragraph.with(span),
        }
    })
}
