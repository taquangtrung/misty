//! The [`Builder`] the Markdown and HTML converters share: the blocks
//! open now (a quote, a list item, a table cell), the paragraph's text so
//! far, and the inline styles in effect, so each converter only maps its
//! events onto opening, closing, and adding.

use super::super::super::base::styled::{FontFamily, Span, TextColor, TextSize};
use super::super::super::base::theme::ThemeToken;
use super::{Block, Container, Document, Heading, Inline, Link, ListBlock, TableBlock, TableRow};

// ========================================================================
// Constants
// ========================================================================

/// The size of subscript and superscript text, drawn smaller in line as
/// the layout cannot raise or lower it.
pub(crate) const SMALL_SCALE: f32 = 0.75;

// ========================================================================
// Data Structures
// ========================================================================

/// A document being built from a converter's events.
#[derive(Debug, Default)]
pub(crate) struct Builder {
    /// The heading being read, if any: its level and anchor.
    heading: Option<(u8, Option<String>)>,
    /// The text of the paragraph or heading being read.
    inlines: Vec<Inline>,
    /// The blocks open now, the document's own at the bottom.
    open: Vec<Open>,
    /// The inline styles in effect, innermost last.
    styles: Vec<InlineStyle>,
}

/// A block open while its content is read.
#[derive(Debug)]
struct Open {
    /// The blocks read inside it.
    blocks: Vec<Block>,
    /// The cells of a table row.
    cells: Vec<Vec<Block>>,
    /// The items of a list.
    items: Vec<Vec<Block>>,
    /// What it is.
    kind: OpenKind,
    /// The rows of a table.
    rows: Vec<TableRow>,
}

/// What kind of block is open.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum OpenKind {
    /// A table cell.
    Cell,
    /// A box of blocks, its blocks to come.
    Container(Container),
    /// A list item.
    Item,
    /// A list, numbered from this or bulleted.
    List(Option<u64>),
    /// A block quote.
    Quote,
    /// The document itself.
    Root,
    /// A table row, a header row or not.
    Row(bool),
    /// A table.
    Table,
}

/// A change of style one element or emphasis makes to the text inside it;
/// `None` and `false` leave the style outside as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct InlineStyle {
    /// The fill behind the text.
    pub(crate) background: Option<TextColor>,
    /// Bold.
    pub(crate) bold: bool,
    /// The text's color.
    pub(crate) color: Option<TextColor>,
    /// Whether the text is left out.
    pub(crate) hidden: bool,
    /// Italic.
    pub(crate) italic: bool,
    /// What a click reports.
    pub(crate) link: Option<Link>,
    /// Monospace, for code.
    pub(crate) monospace: bool,
    /// The size, as a multiple of the size outside.
    pub(crate) scale: Option<f32>,
    /// Struck through.
    pub(crate) strike: bool,
    /// Underlined.
    pub(crate) underline: bool,
}

// ========================================================================
// Builder Implementations
// ========================================================================

impl Builder {
    /// A builder of an empty document.
    pub(crate) fn new() -> Self {
        Self {
            open: vec![Open::new(OpenKind::Root)],
            ..Self::default()
        }
    }

    /// Ends every block still open and returns the document.
    pub(crate) fn finish(mut self) -> Document {
        while self.open.len() > 1 {
            self.close_top();
        }
        self.flush();
        let root = self.open.pop().map(|open| open.blocks).unwrap_or_default();
        Document::new(root)
    }

    /// Opens a block of `kind`, ending the paragraph before it.
    pub(crate) fn open(&mut self, kind: OpenKind) {
        self.flush();
        self.open.push(Open::new(kind));
    }

    /// Closes the innermost open block that `is_it` says is the one, and
    /// every block opened inside it; does nothing when none is open.
    pub(crate) fn close(&mut self, is_it: impl Fn(&OpenKind) -> bool) {
        let Some(index) = self.open.iter().rposition(|open| is_it(&open.kind)) else {
            return;
        };
        if index == 0 {
            return;
        }
        while self.open.len() > index {
            self.close_top();
        }
    }

    /// Adds `block` where the content goes now, ending the paragraph
    /// before it.
    pub(crate) fn push(&mut self, block: Block) {
        self.flush();
        self.top().blocks.push(block);
    }

    /// Starts reading a heading of `level` with the anchor `id`.
    pub(crate) fn start_heading(&mut self, level: u8, id: Option<String>) {
        self.flush();
        self.heading = Some((level, id));
    }

    /// Ends the heading being read, or the paragraph when none is.
    pub(crate) fn end_heading(&mut self) {
        let Some((level, id)) = self.heading.take() else {
            return self.flush();
        };
        let inlines = std::mem::take(&mut self.inlines);
        self.top()
            .blocks
            .push(Block::Heading(Heading { id, inlines, level }));
    }

    /// Ends the paragraph being read, if it has any text but blanks.
    pub(crate) fn flush(&mut self) {
        if self.heading.is_some() {
            return self.end_heading();
        }
        let inlines = std::mem::take(&mut self.inlines);
        let has_text = inlines
            .iter()
            .any(|inline| !inline.span.text.trim().is_empty());
        if has_text {
            self.top().blocks.push(Block::Paragraph(inlines));
        }
    }

    /// Applies `style` to the text added until [`Builder::pop_style`].
    pub(crate) fn push_style(&mut self, style: InlineStyle) {
        self.styles.push(style);
    }

    /// Ends the style pushed last.
    pub(crate) fn pop_style(&mut self) {
        self.styles.pop();
    }

    /// Whether the text added now is left out.
    pub(crate) fn is_hidden(&self) -> bool {
        self.styles.iter().any(|style| style.hidden)
    }

    /// Whether a blank before more text is wanted: the paragraph has text
    /// that does not end in a blank or a line break.
    #[cfg(feature = "html")]
    pub(crate) fn wants_space(&self) -> bool {
        self.inlines
            .last()
            .is_some_and(|inline| !inline.span.text.ends_with([' ', '\n']))
    }

    /// Adds `text` in the styles in effect.
    pub(crate) fn text(&mut self, text: &str) {
        if text.is_empty() || self.is_hidden() {
            return;
        }
        let mut span = Span::new(text, TextColor::Token(ThemeToken::Foreground));
        let mut scale = 1.0;
        let mut link = None;
        for style in &self.styles {
            if let Some(color) = style.color {
                span.color = color;
            }
            if let Some(background) = style.background {
                span.background = Some(background);
            }
            span.bold |= style.bold;
            span.italic |= style.italic;
            span.strikethrough |= style.strike;
            span.underline |= style.underline;
            if style.monospace {
                span.family = FontFamily::Monospace;
            }
            scale *= style.scale.unwrap_or(1.0);
            if style.link.is_some() {
                link.clone_from(&style.link);
            }
        }
        if scale != 1.0 {
            span.size = TextSize::Scale(scale);
        }
        self.inlines.push(Inline { link, span });
    }

    /// The innermost open block.
    fn top(&mut self) -> &mut Open {
        if self.open.is_empty() {
            self.open.push(Open::new(OpenKind::Root));
        }
        let last = self.open.len() - 1;
        &mut self.open[last]
    }

    /// Closes the innermost open block, adding it to the one around it.
    fn close_top(&mut self) {
        self.flush();
        let Some(open) = self.open.pop() else {
            return;
        };
        let parent = self.top();
        match open.kind {
            OpenKind::Cell => match matches!(parent.kind, OpenKind::Row(_)) {
                true => parent.cells.push(open.blocks),
                false => parent.blocks.extend(open.blocks),
            },
            OpenKind::Container(container) => parent.blocks.push(Block::Container(Container {
                blocks: open.blocks,
                ..container
            })),
            OpenKind::Item => match matches!(parent.kind, OpenKind::List(_)) {
                true => parent.items.push(open.blocks),
                false => parent.blocks.extend(open.blocks),
            },
            OpenKind::List(start) => parent.blocks.push(Block::List(ListBlock {
                items: open.items,
                start,
            })),
            OpenKind::Quote => parent.blocks.push(Block::BlockQuote(open.blocks)),
            OpenKind::Root => parent.blocks.extend(open.blocks),
            OpenKind::Row(header) => match matches!(parent.kind, OpenKind::Table) {
                true => parent.rows.push(TableRow {
                    cells: open.cells,
                    header,
                }),
                false => parent.blocks.extend(open.cells.into_iter().flatten()),
            },
            OpenKind::Table => parent
                .blocks
                .push(Block::Table(TableBlock { rows: open.rows })),
        }
    }
}

// ========================================================================
// Open Implementations
// ========================================================================

impl Open {
    /// An empty open block of `kind`.
    fn new(kind: OpenKind) -> Self {
        Self {
            blocks: Vec::new(),
            cells: Vec::new(),
            items: Vec::new(),
            kind,
            rows: Vec::new(),
        }
    }
}
