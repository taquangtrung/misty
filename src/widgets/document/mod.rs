//! [`Document`]: rich prose as blocks, for [`RichText`](crate::widgets::RichText)
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
//! carry an id, an anchor [`RichText::scroll_to_anchor`](crate::widgets::RichText::scroll_to_anchor)
//! jumps to.
//!
//! A [`Block::Aligned`] sets the lines of the paragraphs and headings
//! inside it (in lists, quotes, boxes, and table cells too) at the start,
//! centered, at the end, or justified, as CSS's inherited `text-align`
//! does; code keeps to the start, and the innermost alignment wins. A
//! [`Block::Styled`] sets the blocks inside it as its [`BlockStyle`] says:
//! a line height of their own, which wins over the view's
//! [`RichText::set_line_height`](crate::widgets::RichText::set_line_height), and
//! the scale of their headings, in place of the level's
//! [`Heading::SCALES`].
//!
//! Images are blocks of their own, scaled down to the width; a document
//! does not set an image inside a paragraph's text. An app laying out its
//! own [`Paragraph`](crate::Paragraph)s places one there with an
//! [`InlineBox`](crate::InlineBox).
//!
//! A [`Block::Custom`] is room the app paints itself (a formula, a chart,
//! a page of another format): the document gives it the column's width and
//! the height its [`CustomHeight`] asks for, and
//! [`RichText::custom_blocks`](crate::widgets::RichText::custom_blocks) says where
//! each one in sight landed. A [`Block::HighlightedCode`] is a code block
//! colored by the [`Highlight`] spans a
//! [`Highlighter`](crate::widgets::Highlighter) (or any highlighter) gives, in the
//! theme's [`SyntaxColors`](crate::SyntaxColors).

use std::fmt;
use std::sync::Arc;

#[cfg(any(feature = "html", feature = "markdown"))]
mod build;
#[cfg(feature = "html")]
pub(crate) mod html;
#[cfg(feature = "markdown")]
pub(crate) mod markdown;

use super::super::app::frame::Frame;
use super::super::base::color::Rgba;
use super::super::base::geometry::Rect;
use super::super::base::styled::{FontFamily, Span, TextColor, TextSize};
use super::super::base::syntax::SyntaxColors;
use super::super::base::theme::{Emphasis, ThemeToken};
use super::super::render::image::Image;
use super::super::render::paragraph::{Paragraph, ParagraphLayout, TextAlign};
use super::code_view::Highlight;

// ========================================================================
// Constants
// ========================================================================

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
    /// Blocks whose paragraphs and headings set their lines as the
    /// alignment says: centered, against the end, or justified to both
    /// edges ([`TextAlign`]), within whatever holds them. Code keeps to the
    /// start; an alignment inside it wins over this one.
    Aligned(TextAlign, Vec<Block>),
    /// Blocks set in, behind a bar, as a quotation.
    BlockQuote(Vec<Block>),
    /// Lines of monospace text on the surface color, kept as they are.
    CodeBlock(String),
    /// Blocks in a box of their own.
    Container(Container),
    /// Room the app paints itself, the column's width and as tall as it
    /// asks; to keys and the selection it is one empty block.
    Custom(CustomBlock),
    /// A heading.
    Heading(Heading),
    /// A code block as [`Block::CodeBlock`] draws it, its text colored by
    /// highlight spans.
    HighlightedCode(HighlightedCode),
    /// An image, scaled down to the width.
    Image(BlockImage),
    /// A list of items.
    List(ListBlock),
    /// Text that wraps, its spans styled; line feeds in a span break its
    /// line.
    Paragraph(Vec<Inline>),
    /// A horizontal line across the width.
    Rule,
    /// Blocks set as the style says (their own line height, their
    /// headings' scale), within
    /// whatever holds them. Each setting the style leaves unset is the
    /// one around it; for each it sets, the innermost style wins, and a
    /// style's wins over the view's.
    Styled(BlockStyle, Vec<Block>),
    /// A table of blocks.
    Table(TableBlock),
}

/// How a [`Block::Styled`] sets the blocks inside it, every setting
/// unset until the app sets it: made with [`BlockStyle::new`] and its
/// `with_` methods.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlockStyle {
    /// How much larger headings set their spans than their own size, if
    /// set.
    heading_scale: Option<f32>,
    /// How tall each line of text is as a multiple of its font size, if
    /// set.
    line_height: Option<f32>,
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
    /// audio button: [`RichTextResponse::Data`](crate::widgets::RichTextResponse::Data).
    Data(DataLink),
    /// A URL or another target the app resolves:
    /// [`RichTextResponse::Link`](crate::widgets::RichTextResponse::Link).
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

/// Room in a [`Document`] the app paints itself: a formula, a chart, an
/// embedded page. It takes the whole width of the column it is in;
/// [`RichText::custom_blocks`](crate::widgets::RichText::custom_blocks) gives the
/// rectangle of each one in sight after a frame, to paint in.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomBlock {
    /// How tall it is for the width it is given.
    pub height: CustomHeight,
    /// The app's own key for it, handed back with its rectangle.
    pub id: u64,
    /// What it shows, for screen readers.
    pub label: String,
}

/// How tall a [`CustomBlock`] is for the width it is given. Sizes are in
/// logical pixels, multiplied by the display's scale factor as an
/// [`InlineBox`](crate::InlineBox)'s are.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum CustomHeight {
    /// This many times its width: `0.75` for a 4:3 picture.
    Aspect(f32),
    /// This many logical pixels, whatever its width.
    Fixed(f32),
    /// What the function answers for its width, both in logical pixels.
    ForWidth(HeightForWidth),
}

/// A function from a width to a height, both in logical pixels, for
/// [`CustomHeight::ForWidth`]. Two are equal when they are the same
/// function (clones of one another).
#[derive(Clone)]
pub struct HeightForWidth(Arc<dyn Fn(f32) -> f32 + Send + Sync>);

/// A code block colored by highlight spans.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HighlightedCode {
    /// The code, its lines kept as they are.
    pub code: String,
    /// The spans to color, by byte range of `code` and highlight name,
    /// each colored as the theme's
    /// [`SyntaxColors::color`](crate::SyntaxColors::color) says (the rest,
    /// and a span with a name the theme has no color for, in the body's
    /// color). Spans off a character boundary, past the code, or
    /// overlapping one before them are left out.
    pub highlights: Vec<Highlight>,
}

/// A heading, levels 1 (the largest) to 6, its spans set
/// [`Heading::SCALES`] times their own size by level (a
/// [`BlockStyle::with_heading_scale`] around it sets another scale) and
/// in bold, or at the body's weight when the theme's
/// [`Emphasis`](crate::Emphasis) is [`Emphasis::Regular`](crate::Emphasis::Regular)
/// (a span the app made bold stays bold either way).
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
    /// Each highlighted code block's text block, its code, and its spans,
    /// to color in a theme's syntax colors.
    pub(crate) highlighted: Vec<(usize, HighlightedCode)>,
    /// Each text block's paragraph, in reading order.
    pub(crate) paragraphs: Vec<Paragraph>,
    /// Each text block's rect.
    pub(crate) rects: Vec<Rect>,
    /// What each text block is to screen readers.
    pub(crate) roles: Vec<BlockRole>,
}

/// How a view sets a document's text, beyond the document itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FlowStyle {
    /// How headings stress their words: in bold, or at the body's weight.
    pub(crate) emphasis: Emphasis,
    /// How tall each line of text is as a multiple of its font size, or
    /// `None` for the font's own line height.
    pub(crate) line_height: Option<f32>,
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
    /// Room the app paints, with its key and its text for screen readers.
    Custom(u64, String),
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
    /// The height of a line of body text in the font's own line height,
    /// the unit of every spacing.
    line: f32,
    /// How the text is set.
    style: FlowStyle,
    /// The tables placed so far.
    tables: usize,
    /// What it placed.
    out: Flow,
}

/// What the blocks being placed are part of.
#[derive(Clone, Copy, Debug, Default)]
struct Context {
    /// Where their paragraphs' and headings' lines sit.
    align: TextAlign,
    /// How deep in lists they are.
    depth: usize,
    /// The table cell they are in, if any.
    cell: Option<CellRole>,
    /// How much larger their headings set their spans, when a style
    /// around them set it; else the level's.
    heading_scale: Option<f32>,
    /// Whether they are a list item's.
    item: bool,
    /// How tall their lines are as a multiple of the font size, when a
    /// style around them set it; else the view's.
    line_height: Option<f32>,
}

// ========================================================================
// Document Implementations
// ========================================================================

impl Document {
    /// A document of `blocks`.
    pub fn new(blocks: Vec<Block>) -> Self {
        Self { blocks }
    }

    /// The document's text blocks in reading order, set in `style`,
    /// without measuring: their paragraphs, what they are to screen
    /// readers, and the anchors.
    pub(crate) fn collect(&self, style: FlowStyle) -> Flow {
        let mut placer = Placer::new(None, 0.0, style);
        placer.blocks(&self.blocks, 0.0, 0.0, 0.0, Context::default());
        placer.out
    }

    /// The document laid out `width` pixels wide in `frame`'s fonts, set
    /// in `style`.
    pub(crate) fn lay_out(&self, frame: &Frame, width: f32, style: FlowStyle) -> Flow {
        let mut placer = Placer::new(Some(frame), frame.cell_metrics().height, style);
        let height = placer.blocks(&self.blocks, 0.0, width, 0.0, Context::default());
        placer.out.height = height;
        placer.out
    }
}

// ========================================================================
// BlockStyle Implementations
// ========================================================================

impl BlockStyle {
    /// A style setting nothing: blocks inside it are set as they would be
    /// without it.
    pub fn new() -> Self {
        Self::default()
    }

    /// This style setting each line of text inside it `multiple` times its
    /// font size tall, as [`Paragraph::with_line_height`] does (`1.25` for
    /// a heading's tight lines), whatever line height the view sets. A
    /// multiple that is not a positive number leaves the line height
    /// unset.
    pub fn with_line_height(self, multiple: f32) -> Self {
        Self {
            line_height: positive(multiple),
            ..self
        }
    }

    /// This style setting the spans of the headings inside it `scale`
    /// times their own size, whatever their level, in place of
    /// [`Heading::SCALES`]: `1.0` sets them at the sizes the app gave
    /// them, so an app sizing its headings itself (`TextSize::Pixels`, or
    /// a scale of the body) gets exactly those. The headings keep their
    /// level, role, anchor, and weight. A scale that is not a positive
    /// number leaves it unset.
    pub fn with_heading_scale(self, scale: f32) -> Self {
        Self {
            heading_scale: positive(scale),
            ..self
        }
    }

    /// The context of blocks inside this style, inside `cx`.
    fn within(&self, cx: Context) -> Context {
        Context {
            heading_scale: self.heading_scale.or(cx.heading_scale),
            line_height: self.line_height.or(cx.line_height),
            ..cx
        }
    }
}

// ========================================================================
// Heading Implementations
// ========================================================================

impl Heading {
    /// How much larger each level, 1 to 6, sets its spans than their own
    /// size (a span at the body's size at level 1 is set 1.8 times as
    /// large), unless a [`BlockStyle::with_heading_scale`] around it says
    /// otherwise.
    pub const SCALES: [f32; 6] = [1.8, 1.5, 1.25, 1.1, 1.0, 0.9];

    /// [`Heading::SCALES`]' scale for `level`, held to 1 to 6.
    pub fn level_scale(level: u8) -> f32 {
        let level = level.clamp(1, Self::SCALES.len() as u8);
        Self::SCALES[usize::from(level) - 1]
    }
}

// ========================================================================
// Decor Implementations
// ========================================================================

impl Decor {
    /// Where it is drawn.
    pub(crate) fn rect(&self) -> Rect {
        match self {
            Self::Fill(fill) | Self::Frame(fill) => fill.rect,
            Self::Image(image) => image.rect,
            Self::Text(text) => text.rect,
        }
    }

    /// This decoration moved `dy` pixels down (up for a negative one).
    pub(crate) fn moved_down(mut self, dy: f32) -> Self {
        let rect = match &mut self {
            Self::Fill(fill) | Self::Frame(fill) => &mut fill.rect,
            Self::Image(image) => &mut image.rect,
            Self::Text(text) => &mut text.rect,
        };
        rect.y += dy;
        self
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
// CustomHeight Implementations
// ========================================================================

impl CustomHeight {
    /// The height in physical pixels for `width` physical pixels on a
    /// display scaled by `scale`: never negative, 0 for a size that is
    /// not a number.
    fn of(&self, width: f32, scale: f32) -> f32 {
        let height = match self {
            Self::Aspect(ratio) => width * ratio,
            Self::Fixed(height) => height * scale,
            Self::ForWidth(function) => (function.0)(width / scale) * scale,
        };
        match height.is_finite() {
            true => height.max(0.0).round(),
            false => 0.0,
        }
    }
}

// ========================================================================
// HeightForWidth Implementations
// ========================================================================

impl HeightForWidth {
    /// The height `function` answers for each width.
    pub fn new(function: impl Fn(f32) -> f32 + Send + Sync + 'static) -> Self {
        Self(Arc::new(function))
    }
}

impl fmt::Debug for HeightForWidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HeightForWidth(..)")
    }
}

impl PartialEq for HeightForWidth {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

// ========================================================================
// HighlightedCode Implementations
// ========================================================================

impl HighlightedCode {
    /// The code `code` colored by `highlights`.
    pub fn new(code: impl Into<String>, highlights: Vec<Highlight>) -> Self {
        Self {
            code: code.into(),
            highlights,
        }
    }

    /// Its text as a monospace paragraph, a span per highlight and per run
    /// between them, the highlights colored by `syntax` (all in the body's
    /// color without it, as for measuring).
    pub(crate) fn paragraph(&self, syntax: Option<&SyntaxColors>) -> Paragraph {
        let code = self.code.trim_end_matches('\n');
        let span = |text: &str, color: Option<TextColor>| {
            Span::new(
                text,
                color.unwrap_or(TextColor::Token(ThemeToken::Foreground)),
            )
            .family(FontFamily::Monospace)
        };
        let mut paragraph = Paragraph::new();
        let mut at = 0;
        for highlight in &self.highlights {
            let range = highlight.range.start..highlight.range.end.min(code.len());
            if range.start < at
                || range.start >= range.end
                || !code.is_char_boundary(range.start)
                || !code.is_char_boundary(range.end)
            {
                continue;
            }
            if range.start > at {
                paragraph = paragraph.with(span(&code[at..range.start], None));
            }
            let color = syntax
                .and_then(|syntax| syntax.color(&highlight.name))
                .map(|color| TextColor::Rgba(Rgba::from(color)));
            paragraph = paragraph.with(span(&code[range.clone()], color));
            at = range.end;
        }
        if at < code.len() || at == 0 {
            paragraph = paragraph.with(span(&code[at..], None));
        }
        paragraph
    }
}

// ========================================================================
// Placer Implementations
// ========================================================================

impl<'a> Placer<'a> {
    /// A placer measuring with `frame` (or only collecting), a line
    /// `line` pixels tall, setting text in `style`.
    fn new(frame: Option<&'a Frame>, line: f32, style: FlowStyle) -> Self {
        Self {
            frame,
            line,
            out: Flow::default(),
            style,
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
            Block::Aligned(align, blocks) => {
                let cx = Context {
                    align: *align,
                    ..cx
                };
                self.blocks(blocks, x, width, top, cx)
            }
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
                let span = Span::new(
                    code.trim_end_matches('\n'),
                    TextColor::Token(ThemeToken::Foreground),
                )
                .family(FontFamily::Monospace);
                self.code(Paragraph::new().with(span), x, width, top, cx)
            }
            Block::HighlightedCode(code) => {
                self.out
                    .highlighted
                    .push((self.out.paragraphs.len(), code.clone()));
                self.code(code.paragraph(None), x, width, top, cx)
            }
            Block::Container(container) => self.container(container, x, width, top, cx),
            Block::Custom(custom) => {
                let scale = self.frame.map_or(1.0, |frame| frame.scale_factor() as f32);
                let width = width.max(0.0);
                let height = match self.frame {
                    Some(_) => custom.height.of(width, scale),
                    None => 0.0,
                };
                let rect = Rect::new(x, top, width, height);
                self.out.paragraphs.push(Paragraph::new());
                self.out
                    .roles
                    .push(BlockRole::Custom(custom.id, custom.label.clone()));
                self.out.rects.push(rect);
                if let Some(frame) = self.frame {
                    self.out.layouts.push(
                        frame.paragraph_layout(&Paragraph::new(), Rect::new(x, top, width, 0.0)),
                    );
                }
                top + height
            }
            Block::Heading(heading) => {
                let level = heading.level.clamp(1, Heading::SCALES.len() as u8);
                let scale = cx
                    .heading_scale
                    .unwrap_or_else(|| Heading::level_scale(level));
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
                    let span = span.size(size);
                    match self.style.emphasis {
                        Emphasis::Bold => span.bold(),
                        Emphasis::Regular => span,
                    }
                })
                .with_align(cx.align);
                self.text(paragraph, BlockRole::Heading(level), x, width, top, cx)
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
                let paragraph = paragraph_of(inlines, |span| span).with_align(cx.align);
                self.text(paragraph, self.role(cx), x, width, top, cx)
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
            Block::Styled(style, blocks) => self.blocks(blocks, x, width, top, style.within(cx)),
            Block::Table(table) => self.table(table, x, width, top, cx),
        }
    }

    /// Places `paragraph`, a code block's text, padded on the surface
    /// color at `top` in the column at `x`, `width` wide; returns its
    /// bottom.
    fn code(&mut self, paragraph: Paragraph, x: f32, width: f32, top: f32, cx: Context) -> f32 {
        let pad = self.lines(PADDING_LINES);
        let fill = self.out.decor.len();
        let bottom = self.text(
            paragraph,
            self.role(cx),
            x + pad,
            width - 2.0 * pad,
            top + pad,
            cx,
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
            let paragraph = self.styled(
                Paragraph::plain(marker, TextColor::Token(ThemeToken::Muted)),
                cx,
            );
            // As tall as its own line, which a line height may make taller
            // than a line of the font.
            let height = self
                .frame
                .map_or(self.line, |frame| {
                    frame.measure_paragraph(&paragraph, indent)
                })
                .max(self.line);
            self.decor(Decor::Text(DecorText {
                paragraph,
                rect: Rect::new(x, start, indent, height),
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
                        Block::Aligned(..)
                        | Block::BlockQuote(_)
                        | Block::CodeBlock(_)
                        | Block::Container(_)
                        | Block::Custom(_)
                        | Block::Heading(_)
                        | Block::HighlightedCode(_)
                        | Block::Image(_)
                        | Block::List(_)
                        | Block::Rule
                        | Block::Styled(..)
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

    /// `paragraph` set in `cx`'s line height, else the placer's style's.
    fn styled(&self, mut paragraph: Paragraph, cx: Context) -> Paragraph {
        paragraph.set_line_height(cx.line_height.or(self.style.line_height));
        paragraph
    }

    /// Adds `decor` over what was drawn under the text so far.
    fn decor(&mut self, decor: Decor) {
        self.out.decor.push(decor);
    }

    /// Places `paragraph`, a text block that is `role` in `cx`, at `top`
    /// in the column at `x`, `width` wide; returns its bottom, at least a
    /// line below its top.
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        paragraph: Paragraph,
        role: BlockRole,
        x: f32,
        width: f32,
        top: f32,
        cx: Context,
    ) -> f32 {
        let paragraph = self.styled(paragraph, cx);
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

/// `value` when it is a positive number.
fn positive(value: f32) -> Option<f32> {
    Some(value).filter(|value| value.is_finite() && *value > 0.0)
}

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
