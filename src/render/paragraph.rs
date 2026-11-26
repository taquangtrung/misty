//! Rich text in the proportional font: a [`Paragraph`] of styled spans
//! (bold, italic, underlined, struck through, in theme or exact colors, on
//! a background, larger or smaller, in another family, some of them
//! links) wrapped to a width and aligned, measured for layout with
//! [`Frame::measure_paragraph`] and drawn with [`Frame::draw_paragraph`],
//! which hands back a [`ParagraphLayout`]: where its links are, which
//! character is under a point, and where a caret or a selection goes.
//! [`Frame::paragraph_layout`] finds the same without drawing, and a
//! [`ParagraphMeasurer`] finds it on any thread.
//!
//! A paragraph may hold [`InlineBox`]es among its spans: room of a set
//! size (for an image, a formula, an icon) that wraps and lifts its line
//! as a glyph would. Misty draws nothing there; the app reads where each
//! box landed from [`ParagraphLayout::inline_boxes`] and paints it.
//!
//! [`Paragraph::with_line_height`] sets its lines a multiple of their font
//! size tall, as CSS's unitless `line-height` does.
//!
//! [`Paragraph::with_first_line_indent`] starts its first line further in
//! than the others, as CSS's `text-indent` does, and
//! [`Paragraph::with_hanging_indent`] the lines after the first, for a
//! bullet or a number hanging left of the text it heads. Measuring,
//! drawing, and hit testing follow the indents alike.
//!
//! [`ParagraphLayout::lines`] lists the wrapped lines (the text each holds,
//! its top, height, and baseline), so an app can split text into pages at
//! line boundaries.
//!
//! Character offsets are byte offsets into [`Paragraph::text`], always on
//! a character boundary.
//!
//! The cell grid's text stays monospace; paragraphs are for prose, such
//! as a dialog's body or a dictionary entry.
//!
//! Shaping a paragraph at a width is kept across frames in the shared
//! fonts, so measuring it for layout and then drawing it at the width
//! found shape it once, not once a call.
//!
//! [`Frame::measure_paragraph`]: crate::Frame::measure_paragraph
//! [`Frame::draw_paragraph`]: crate::Frame::draw_paragraph
//! [`Frame::paragraph_layout`]: crate::Frame::paragraph_layout
//! [`ParagraphMeasurer`]: crate::ParagraphMeasurer

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use glyphon::{Buffer, Color, FontSystem, Metrics, SwashCache};

use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::styled::{FontFamily, Span, TextColor};
use super::super::base::theme::{Theme, ThemeToken};
use super::draw::{
    Face, InlineMetrics, Quad, RunStyle, SegmentAlign, TextRun, TextSegment, Wrapping,
};
use super::font::FaceWeights;
use super::image::{BYTES_PER_PIXEL, Image};
use super::inline_font;
use super::{Fonts, shape_text};

// ========================================================================
// Constants
// ========================================================================

/// The share of the font size an underline is thick, at least a pixel.
const UNDERLINE_SHARE: f32 = 0.07;

/// The width a paragraph is shaped at to measure it on one line: wider
/// than any line a window shows, so nothing wraps.
const UNWRAPPED_WIDTH: f32 = 1.0e6;

/// How far above the baseline a strikethrough's middle is, as a share of
/// its text's font size: about half the height of a lowercase letter.
const STRIKE_RAISE: f32 = 0.28;

/// How close, in pixels, two glyphs' edges are to count as touching, so
/// a selection over them is one rectangle.
const TOUCHING: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// Styled text that wraps to the width it is drawn in.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Paragraph {
    /// Where each line sits in the width.
    align: TextAlign,
    /// The box each span stands for, by span: `None` for text.
    boxes: Vec<Option<InlineBox>>,
    /// How far in from the left the first line starts, in logical pixels,
    /// by its bits; `None` for not at all.
    first_indent: Option<u32>,
    /// How far in from the left the lines after the first start, in
    /// logical pixels, by its bits; `None` for not at all.
    hanging_indent: Option<u32>,
    /// How tall a line is as a multiple of the font size, by its bits, or
    /// `None` for the font's own line height.
    line_height: Option<u32>,
    /// The target of each span that is a link, by span.
    links: Vec<Option<String>>,
    /// The runs, in order; an inline box is a span of its one character.
    spans: Vec<Span>,
}

/// Room of a set size inside a [`Paragraph`]'s text, for the app to paint
/// an image, a formula, or an icon in: it wraps with the words around it
/// and makes its line as tall as it needs, as one character would, and
/// [`ParagraphLayout::inline_boxes`] says where it landed. Sizes are in
/// logical pixels, multiplied by the display's scale factor as a span's
/// [`TextSize::Pixels`](crate::TextSize::Pixels) is.
///
/// By default its bottom edge sits on the line's baseline, as an image in
/// a web page's text does; [`InlineBox::with_baseline`] lowers it, for a
/// formula with a part below the baseline or an icon centered on the
/// text. In the paragraph's text it is the character U+FFFC (object
/// replacement, three bytes), so offsets, carets, selections, and
/// [`ParagraphLayout::offset_at`] treat it as one character.
#[derive(Clone, Copy, Debug)]
pub struct InlineBox {
    /// How far its top is above the baseline.
    ascent: f32,
    /// Its height.
    height: f32,
    /// Its width.
    width: f32,
}

/// Where the lines of a [`Paragraph`] sit in its width.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextAlign {
    /// Centered.
    Center,
    /// Against the end edge: the right in left-to-right text, the left in
    /// right-to-left text.
    End,
    /// Spread to both edges: the spaces of every wrapped line widen so it
    /// fills the width, as CSS `text-align: justify` does. The last line
    /// (and a line ending the text before a line break) keeps to the start
    /// edge. Text set on one line, as a table cell's, is at the start.
    Justify,
    /// Against the start edge: the left in left-to-right text, the right
    /// in right-to-left text (Hebrew, Arabic), line by line.
    #[default]
    Start,
}

/// Where a paragraph's text and links landed, in the window's physical
/// pixels: hit test links with [`ParagraphLayout::link_at`], map a point
/// to a character with [`ParagraphLayout::offset_at`], and a character to
/// a caret or a selection with [`ParagraphLayout::caret_rect`] and
/// [`ParagraphLayout::range_rects`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParagraphLayout {
    /// Where each inline box landed, in the order added.
    boxes: Vec<Rect>,
    /// The height of all its lines.
    height: f32,
    /// Its lines, relative to `origin`, shared with the cached shape.
    lines: Arc<Vec<LineGeometry>>,
    /// Each piece of a link on a line, with its target.
    links: Vec<LinkArea>,
    /// Its top left corner.
    origin: Point,
}

/// One wrapped line of a laid out [`Paragraph`], from
/// [`ParagraphLayout::lines`], in the window's physical pixels.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ParagraphLine {
    /// Where its baseline is, down from the window's top.
    pub baseline: f32,
    /// Its height, the room from its top to the next line's.
    pub height: f32,
    /// The byte range of [`Paragraph::text`] it holds. The lines' ranges
    /// follow one another without gaps: a line runs to where the next one
    /// starts (a line break or the spaces where it wrapped included), the
    /// last to the end of the text.
    pub text: Range<usize>,
    /// Where its top is, down from the window's top.
    pub top: f32,
}

/// One line of a shaped paragraph, relative to the paragraph's top left.
#[derive(Clone, Debug, PartialEq)]
struct LineGeometry {
    /// Its baseline.
    baseline: f32,
    /// Where its text ends in the paragraph's text: where the next line
    /// starts, or the text's end.
    end: usize,
    /// Its glyphs, left to right as they show.
    glyphs: Vec<GlyphGeometry>,
    /// Its height.
    height: f32,
    /// Where its text starts in the paragraph's text.
    start: usize,
    /// Its top.
    top: f32,
}

/// Where one glyph (or a cluster of characters drawn as one) of a line is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GlyphGeometry {
    /// Where its characters end in the paragraph's text.
    end: usize,
    /// Its left edge.
    left: f32,
    /// Its right edge.
    right: f32,
    /// Whether it reads right to left.
    rtl: bool,
    /// Where its characters start in the paragraph's text.
    start: usize,
}

/// One line's piece of a link.
#[derive(Clone, Debug, PartialEq)]
struct LinkArea {
    /// Where it is.
    rect: Rect,
    /// What it links to.
    target: String,
}

/// A paragraph to shape, borrowed: what its shape depends on (its text
/// and faces, alignment, width, and font), to look up a
/// [`ParagraphKey`] without copying.
pub(crate) struct ParagraphQuery<'a> {
    /// The font.
    font: ParagraphFont,
    /// The paragraph; its colors and links play no part.
    paragraph: &'a Paragraph,
    /// The width its lines wrap at.
    width: f32,
}

/// What a cached [`ParagraphShape`] was shaped from.
pub(crate) struct ParagraphKey {
    /// Where each line sits in the width.
    align: TextAlign,
    /// The line height's bits.
    line_height: u32,
    /// The paragraph's own line height multiple's bits, if it has one.
    multiple: Option<u32>,
    /// The text and faces, in the default color.
    runs: Vec<TextRun>,
    /// The font size's bits.
    size: u32,
    /// The weights of its regular and bold text.
    weights: FaceWeights,
    /// The width's bits.
    width: u32,
}

/// A paragraph with indents shaped at a width: its first line and the
/// rest shaped apart, each at the width its indent leaves, and put
/// together with the indents in place.
struct IndentedShape {
    /// The first line's indent, in pixels.
    first: f32,
    /// Where the lines after the first start down from the top, the first
    /// line's bottom.
    first_bottom: f32,
    /// The lines after the first, as a paragraph of their own, and their
    /// indent in pixels; `None` when the text fits on the first line.
    rest: Option<(Paragraph, f32)>,
    /// Both shapes put together, the indents in place, the spans those of
    /// the whole paragraph.
    shape: ParagraphShape,
}

/// A paragraph shaped at a width: where each span landed on each line,
/// relative to the paragraph's top left.
#[derive(Clone)]
pub(crate) struct ParagraphShape {
    /// Where each inline box landed, in the order added.
    boxes: Vec<Rect>,
    /// Consecutive glyphs of one span on one line, line by line.
    groups: Vec<GlyphGroup>,
    /// The height of all its lines.
    height: f32,
    /// Its lines and their glyphs.
    lines: Arc<Vec<LineGeometry>>,
}

/// Consecutive glyphs of one span on one line.
#[derive(Clone)]
struct GlyphGroup {
    /// The left edge of its first glyph.
    left: f32,
    /// The height of its line.
    line_height: f32,
    /// The top of its line.
    line_top: f32,
    /// The baseline of its line.
    line_y: f32,
    /// The right edge of its last glyph.
    right: f32,
    /// The font size of its glyphs.
    size: f32,
    /// The span its glyphs come from.
    span: usize,
}

/// A paragraph shaped at a width, with its decorations and links found.
pub(crate) struct ShapedParagraph {
    /// The underlines of underlined spans and links, and strikethroughs.
    pub(crate) decorations: Vec<Quad>,
    /// The backgrounds of spans that have one, drawn under the text.
    pub(crate) fills: Vec<Quad>,
    /// Where its text and links landed.
    pub(crate) layout: ParagraphLayout,
    /// The text for the renderer: one segment, or two for a paragraph
    /// with indents (its first line and the rest).
    pub(crate) segments: Vec<TextSegment>,
}

/// The pixels a paragraph is rendered into, four bytes a pixel with
/// straight alpha, row by row from the top left.
struct Canvas {
    /// The height in pixels.
    height: u32,
    /// The pixels.
    pixels: Vec<u8>,
    /// The width in pixels.
    width: u32,
}

/// What a paragraph's spans draw besides their glyphs.
struct SpanQuads {
    /// The underlines of underlined spans and links, and strikethroughs,
    /// drawn over the text.
    decorations: Vec<Quad>,
    /// The backgrounds of spans that have one, drawn under the text.
    fills: Vec<Quad>,
}

/// The proportional font a paragraph is shaped in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ParagraphFont {
    /// The line height, in pixels.
    pub(crate) line_height: f32,
    /// Physical pixels per logical pixel, for spans sized in pixels.
    pub(crate) scale_factor: f32,
    /// The font size, in pixels.
    pub(crate) size: f32,
    /// The weights of its regular and bold text: the window font's, so a
    /// page's words are as light or heavy as its cells.
    pub(crate) weights: FaceWeights,
}

// ========================================================================
// Paragraph Implementations
// ========================================================================

impl Paragraph {
    /// An empty paragraph, its lines at the start edge.
    pub fn new() -> Self {
        Self::default()
    }

    /// The paragraph's text, its spans joined without their styles: what a
    /// screen reader reads of it.
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    /// A paragraph of one span of `text` in `color`.
    pub fn plain(text: impl Into<String>, color: TextColor) -> Self {
        Self::new().with(Span::new(text, color))
    }

    /// This paragraph with `span` added at its end.
    pub fn with(mut self, span: Span) -> Self {
        self.spans.push(span);
        self.links.push(None);
        self.boxes.push(None);
        self
    }

    /// This paragraph with `span` added at its end as a link to `target`
    /// (a URL, an id, whatever the app resolves), drawn underlined.
    pub fn with_link(mut self, span: Span, target: impl Into<String>) -> Self {
        self.spans.push(span);
        self.links.push(Some(target.into()));
        self.boxes.push(None);
        self
    }

    /// This paragraph with `inline` added at its end: room the app paints
    /// in, set among the text as one character.
    pub fn with_box(mut self, inline: InlineBox) -> Self {
        let text = inline_font::CHAR.to_string();
        self.spans
            .push(Span::new(text, ThemeToken::Foreground.into()));
        self.links.push(None);
        self.boxes.push(Some(inline));
        self
    }

    /// This paragraph with its lines placed as `align` says.
    pub fn with_align(self, align: TextAlign) -> Self {
        Self { align, ..self }
    }

    /// This paragraph with each line `multiple` times its font size tall,
    /// as CSS's unitless `line-height` (`1.7` for loose reading text): the
    /// room past the text's own height is shared above and below it, a
    /// span set larger makes its line taller in step, and carets,
    /// selections, and hit tests follow the taller lines. A multiple that
    /// is not a positive number keeps the font's own line height, the
    /// default.
    pub fn with_line_height(mut self, multiple: f32) -> Self {
        self.set_line_height(Some(multiple));
        self
    }

    /// This paragraph with its first line starting `indent` logical
    /// pixels in from the left of the width it is drawn in, as CSS's
    /// `text-indent` does, the line wrapping in the room left. An indent
    /// that is not a positive number is none, the default.
    pub fn with_first_line_indent(self, indent: f32) -> Self {
        Self {
            first_indent: positive_bits(indent),
            ..self
        }
    }

    /// This paragraph with the lines after its first starting `indent`
    /// logical pixels in from the left, wrapping in the room left, while
    /// the first starts at its own indent: with a first-line indent of 0,
    /// a bullet or a number and a gap at the start of the first line hang
    /// left of the text the other lines line up with. An indent that is
    /// not a positive number is none, the default.
    pub fn with_hanging_indent(self, indent: f32) -> Self {
        Self {
            hanging_indent: positive_bits(indent),
            ..self
        }
    }

    /// The first line's indent and the other lines', in pixels in `font`,
    /// or `None` when both are 0.
    fn indents(&self, font: ParagraphFont) -> Option<(f32, f32)> {
        if self.first_indent.is_none() && self.hanging_indent.is_none() {
            return None;
        }
        let pixels =
            |bits: Option<u32>| bits.map_or(0.0, |bits| f32::from_bits(bits) * font.scale_factor);
        Some((pixels(self.first_indent), pixels(self.hanging_indent)))
    }

    /// The paragraph from byte `at` of its text on, in the same styles,
    /// and the index of the span its first span is (part of); `at` is on
    /// a character boundary.
    fn split_from(&self, at: usize) -> (Paragraph, usize) {
        let mut start = 0;
        let mut rest = Paragraph {
            align: self.align,
            line_height: self.line_height,
            ..Paragraph::default()
        };
        let mut first = self.spans.len();
        for (index, span) in self.spans.iter().enumerate() {
            let end = start + span.text.len();
            if end > at {
                first = first.min(index);
                let mut span = span.clone();
                if start < at {
                    span.text = span.text[at - start..].to_owned();
                }
                rest.spans.push(span);
                rest.links.push(self.links.get(index).cloned().flatten());
                rest.boxes.push(self.boxes.get(index).copied().flatten());
            }
            start = end;
        }
        (rest, first)
    }

    /// How tall a line is as a multiple of the font size, or `None` for
    /// the font's own line height.
    pub(crate) fn line_height(&self) -> Option<f32> {
        self.line_height.map(f32::from_bits)
    }

    /// Sets each line `multiple` times the font size tall, or the font's
    /// own line height for `None` or a multiple that is not a positive
    /// number.
    pub(crate) fn set_line_height(&mut self, multiple: Option<f32>) {
        self.line_height = multiple
            .filter(|multiple| multiple.is_finite() && *multiple > 0.0)
            .map(f32::to_bits);
    }

    /// The runs of text in `font` and `theme`'s colors, one per span.
    fn runs(&self, theme: Option<&Theme>, font: ParagraphFont) -> Vec<TextRun> {
        self.spans
            .iter()
            .enumerate()
            .map(|(index, span)| {
                // Measuring needs no colors.
                let color = theme.map(|theme| span.color.resolve_rgba(theme));
                let alpha = color.map(|color| color.a).filter(|alpha| *alpha < u8::MAX);
                TextRun {
                    color: color
                        .map_or(Rgb::default(), |color| Rgb::new(color.r, color.g, color.b)),
                    face: face_of(span),
                    style: RunStyle {
                        alpha,
                        family: (span.family != FontFamily::Proportional)
                            .then(|| span.family.clone()),
                        inline: self.inline_of(index, font),
                        size: size_bits(span, font),
                    },
                    text: span.text.clone(),
                }
            })
            .collect()
    }

    /// The size of the inline box span `index` stands for in `font`, or
    /// `None` when it is text.
    fn inline_of(&self, index: usize, font: ParagraphFont) -> Option<InlineMetrics> {
        let inline = self.boxes.get(index).copied().flatten()?;
        Some(inline.metrics(font.scale_factor))
    }

    /// Whether span `index` is drawn underlined: underlined or a link.
    fn is_underlined(&self, index: usize) -> bool {
        let link = self.links.get(index).is_some_and(Option::is_some);
        link || self.spans.get(index).is_some_and(|span| span.underline)
    }
}

// ========================================================================
// InlineBox Implementations
// ========================================================================

impl InlineBox {
    /// A box `width` by `height` logical pixels, its bottom on the
    /// baseline. A size that is not a positive number is 0.
    pub fn new(width: f32, height: f32) -> Self {
        let height = non_negative(height);
        Self {
            ascent: height,
            height,
            width: non_negative(width),
        }
    }

    /// This box with the baseline `ascent` logical pixels below its top,
    /// held to its height: its height for a bottom on the baseline, half of
    /// it for a box centered on the baseline, a formula's height above its
    /// baseline for a formula.
    pub fn with_baseline(self, ascent: f32) -> Self {
        Self {
            ascent: non_negative(ascent).min(self.height),
            ..self
        }
    }

    /// Its size in physical pixels on a display scaled by `scale_factor`.
    fn metrics(self, scale_factor: f32) -> InlineMetrics {
        InlineMetrics {
            ascent: (self.ascent * scale_factor).to_bits(),
            descent: ((self.height - self.ascent) * scale_factor).to_bits(),
            width: (self.width * scale_factor).to_bits(),
        }
    }

    /// Its numbers' bits, to compare and hash it.
    fn bits(self) -> [u32; 3] {
        [self.ascent, self.height, self.width].map(f32::to_bits)
    }
}

impl PartialEq for InlineBox {
    fn eq(&self, other: &Self) -> bool {
        self.bits() == other.bits()
    }
}

// Compared by bits, as sizes are elsewhere.
impl Eq for InlineBox {}

impl Hash for InlineBox {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bits().hash(state);
    }
}

// ========================================================================
// ParagraphLayout Implementations
// ========================================================================

impl ParagraphLayout {
    /// This layout moved `dy` pixels down (up for a negative one): its
    /// lines, links, and inline boxes with it.
    pub(crate) fn moved_down(mut self, dy: f32) -> Self {
        self.origin.y += dy;
        for rect in &mut self.boxes {
            rect.y += dy;
        }
        for link in &mut self.links {
            link.rect.y += dy;
        }
        self
    }

    /// The height of all the paragraph's lines, in pixels.
    pub fn height(&self) -> f32 {
        self.height
    }

    /// The paragraph's wrapped lines, top to bottom: the text each holds,
    /// and where its top, height, and baseline are, to split a long text
    /// into pages at line boundaries. A paragraph laid out off the UI
    /// thread by a [`ParagraphMeasurer`](crate::ParagraphMeasurer) lists
    /// the same lines. An empty paragraph has one empty line.
    pub fn lines(&self) -> impl ExactSizeIterator<Item = ParagraphLine> + '_ {
        let origin = self.origin;
        self.lines.iter().map(move |line| ParagraphLine {
            baseline: origin.y + line.baseline,
            height: line.height,
            text: line.start..line.end,
            top: origin.y + line.top,
        })
    }

    /// Where each [`InlineBox`] of the paragraph landed, in the order they
    /// were added: the rectangle to paint its image, formula, or icon in.
    pub fn inline_boxes(&self) -> &[Rect] {
        &self.boxes
    }

    /// The target of the link under `point`, or `None` when no link is
    /// there.
    pub fn link_at(&self, point: Point) -> Option<&str> {
        self.links
            .iter()
            .find(|link| link.rect.contains(point))
            .map(|link| link.target.as_str())
    }

    /// The character offset nearest `point`: the boundary between
    /// characters closest to it on the line at its height. A point above
    /// the first line counts as on it, one below the last on the last, and
    /// one beside a line as at its nearer end. 0 for an empty paragraph.
    pub fn offset_at(&self, point: Point) -> usize {
        let (x, y) = (point.x - self.origin.x, point.y - self.origin.y);
        let Some(line) = self
            .lines
            .iter()
            .find(|line| y < line.top + line.height)
            .or_else(|| self.lines.last())
        else {
            return 0;
        };
        let Some(glyph) = line
            .glyphs
            .iter()
            .find(|glyph| x < glyph.right)
            .or_else(|| line.glyphs.last())
        else {
            return line.start;
        };
        let before = x < (glyph.left + glyph.right) / 2.0;
        // Before a left-to-right glyph is its start; before a
        // right-to-left one, its end.
        match before != glyph.rtl {
            true => glyph.start,
            false => glyph.end,
        }
    }

    /// Where a caret before the character at `offset` goes: a rectangle no
    /// wide, as tall as its line. An offset where a line wraps is at the
    /// start of the next line; one past the end, after the last character.
    pub fn caret_rect(&self, offset: usize) -> Rect {
        let lines = self.lines.iter();
        let at_start = || {
            lines.clone().find_map(|line| {
                let glyph = line.glyphs.iter().find(|glyph| glyph.start == offset)?;
                Some((line, glyph.leading_edge()))
            })
        };
        let on_empty_line = || {
            lines
                .clone()
                .find(|line| line.glyphs.is_empty() && line.start == offset)
                .map(|line| (line, 0.0))
        };
        let at_end = || {
            lines.clone().find_map(|line| {
                let glyph = line.glyphs.iter().find(|glyph| glyph.end == offset)?;
                Some((line, glyph.trailing_edge()))
            })
        };
        let inside = || {
            lines.clone().find_map(|line| {
                let glyph = line
                    .glyphs
                    .iter()
                    .find(|glyph| glyph.start < offset && offset < glyph.end)?;
                Some((line, glyph.edge_at(offset)))
            })
        };
        let past_end = || {
            self.lines.last().map(|line| {
                let right = line
                    .glyphs
                    .iter()
                    .map(|glyph| glyph.right)
                    .fold(0.0, f32::max);
                (line, right)
            })
        };
        match at_start()
            .or_else(on_empty_line)
            .or_else(at_end)
            .or_else(inside)
            .or_else(past_end)
        {
            Some((line, x)) => Rect::new(
                self.origin.x + x,
                self.origin.y + line.top,
                0.0,
                line.height,
            ),
            None => Rect::new(self.origin.x, self.origin.y, 0.0, self.height),
        }
    }

    /// The rectangles that cover the characters in `range`, one per line
    /// (more where right-to-left text splits it), each as tall as its
    /// line: where to fill a selection or a search match behind the text.
    pub fn range_rects(&self, range: Range<usize>) -> Vec<Rect> {
        let mut rects = Vec::new();
        for line in self.lines.iter() {
            let mut current: Option<(f32, f32)> = None;
            for glyph in &line.glyphs {
                let covered = glyph.start < range.end && glyph.end > range.start;
                current = match (current, covered) {
                    (Some((left, right)), true) if (glyph.left - right).abs() < TOUCHING => {
                        Some((left, glyph.right))
                    }
                    (last, true) => {
                        rects.extend(last.map(|(left, right)| self.line_rect(line, left, right)));
                        Some((glyph.left, glyph.right))
                    }
                    (last, false) => {
                        rects.extend(last.map(|(left, right)| self.line_rect(line, left, right)));
                        None
                    }
                };
            }
            rects.extend(current.map(|(left, right)| self.line_rect(line, left, right)));
        }
        rects
    }

    /// The part of `line` from `left` to `right`, in the window.
    fn line_rect(&self, line: &LineGeometry, left: f32, right: f32) -> Rect {
        Rect::new(
            self.origin.x + left,
            self.origin.y + line.top,
            right - left,
            line.height,
        )
    }
}

// ========================================================================
// GlyphGeometry Implementations
// ========================================================================

impl GlyphGeometry {
    /// The edge its first character starts at: the left, or the right
    /// when it reads right to left.
    fn leading_edge(self) -> f32 {
        match self.rtl {
            true => self.right,
            false => self.left,
        }
    }

    /// The edge its last character ends at.
    fn trailing_edge(self) -> f32 {
        match self.rtl {
            true => self.left,
            false => self.right,
        }
    }

    /// Where the caret before `offset`, inside its characters, goes: its
    /// width shared out by bytes.
    fn edge_at(self, offset: usize) -> f32 {
        let share = (offset - self.start) as f32 / (self.end - self.start).max(1) as f32;
        let along = (self.right - self.left) * share;
        match self.rtl {
            true => self.right - along,
            false => self.left + along,
        }
    }
}

// ========================================================================
// ParagraphKey Implementations
// ========================================================================

impl ParagraphKey {
    /// The key of what `query` asks for.
    pub(crate) fn of(query: &ParagraphQuery<'_>) -> Self {
        Self {
            align: query.paragraph.align,
            line_height: query.font.line_height.to_bits(),
            multiple: query.paragraph.line_height,
            runs: query.paragraph.runs(None, query.font),
            size: query.font.size.to_bits(),
            weights: query.font.weights,
            width: query.width.to_bits(),
        }
    }

    /// Whether `key` is what `query` asks for.
    pub(crate) fn matches(key: &Self, query: &ParagraphQuery<'_>) -> bool {
        let spans = &query.paragraph.spans;
        key.align == query.paragraph.align
            && key.multiple == query.paragraph.line_height
            && key.line_height == query.font.line_height.to_bits()
            && key.size == query.font.size.to_bits()
            && key.weights == query.font.weights
            && key.width == query.width.to_bits()
            && key.runs.len() == spans.len()
            && key.runs.iter().zip(spans).all(|(run, span)| {
                run.text == span.text
                    && run.face == face_of(span)
                    && run.style.size == size_bits(span, query.font)
                    && run
                        .style
                        .family
                        .as_ref()
                        .unwrap_or(&FontFamily::Proportional)
                        == &span.family
            })
            && key.runs.iter().enumerate().all(|(index, run)| {
                run.style.inline == query.paragraph.inline_of(index, query.font)
            })
    }
}

// ========================================================================
// Canvas Implementations
// ========================================================================

impl Canvas {
    /// A canvas `width` by `height` pixels of `color`.
    fn filled(width: u32, height: u32, color: Rgba) -> Self {
        let pixel = [color.r, color.g, color.b, color.a];
        let count = width as usize * height as usize;
        Self {
            height,
            pixels: pixel.repeat(count),
            width,
        }
    }

    /// Draws `color` over the whole pixels of `rect` inside the canvas.
    fn fill(&mut self, rect: Rect, color: Rgba) {
        if color.a == 0 {
            return;
        }
        let span = |start: f32, length: f32, limit: u32| {
            let first = start.round().clamp(0.0, limit as f32) as u32;
            let end = (start + length).round().clamp(0.0, limit as f32) as u32;
            first..end
        };
        let columns = span(rect.x, rect.width, self.width);
        for row in span(rect.y, rect.height, self.height) {
            for column in columns.clone() {
                let at = (row as usize * self.width as usize + column as usize) * BYTES_PER_PIXEL;
                if let Some(pixel) = self.pixels.get_mut(at..at + BYTES_PER_PIXEL) {
                    blend_over(pixel, color);
                }
            }
        }
    }
}

// ========================================================================
// ParagraphQuery Implementations
// ========================================================================

impl ParagraphQuery<'_> {
    /// Shapes the paragraph with `font_system`, finding where each span
    /// landed on each line.
    pub(crate) fn shape(&self, font_system: &mut FontSystem) -> ParagraphShape {
        let metrics = Metrics::new(self.font.size, self.font.line_height);
        let runs = self.paragraph.runs(None, self.font);
        let buffer = shape_text(
            font_system,
            metrics,
            None,
            &runs,
            Some(self.wrapping()),
            true,
            self.font.weights,
        );
        let groups = buffer
            .layout_runs()
            .flat_map(|run| {
                let glyphs = run.glyphs.iter().map(|g| (g.metadata, g.x, g.w));
                let size = |span| {
                    run.glyphs
                        .iter()
                        .find(|glyph| glyph.metadata == span)
                        .map_or(self.font.size, |glyph| glyph.font_size)
                };
                glyph_groups(glyphs)
                    .into_iter()
                    .map(move |(span, left, right)| GlyphGroup {
                        left,
                        line_height: run.line_height,
                        line_top: run.line_top,
                        line_y: run.line_y,
                        right,
                        size: size(span),
                        span,
                    })
            })
            .collect();
        ParagraphShape {
            boxes: self.box_rects(&buffer),
            groups,
            height: height_of(&buffer),
            lines: Arc::new(lines_of(&buffer)),
        }
    }

    /// Where each inline box of the paragraph landed in `buffer`, shaped
    /// from it, in the order added: on its glyph, from its ascent above the
    /// baseline to its descent below.
    fn box_rects(&self, buffer: &Buffer) -> Vec<Rect> {
        let mut boxes: Vec<(usize, Rect)> = buffer
            .layout_runs()
            .flat_map(|run| {
                run.glyphs.iter().filter_map(move |glyph| {
                    let inline = self.paragraph.inline_of(glyph.metadata, self.font)?;
                    let ascent = f32::from_bits(inline.ascent);
                    let height = ascent + f32::from_bits(inline.descent);
                    let rect = Rect::new(glyph.x, run.line_y - ascent, glyph.w, height);
                    Some((glyph.metadata, rect))
                })
            })
            .collect();
        boxes.sort_by_key(|(span, _)| *span);
        boxes.into_iter().map(|(_, rect)| rect).collect()
    }

    /// How the paragraph wraps and aligns.
    fn wrapping(&self) -> Wrapping {
        Wrapping {
            align: self.paragraph.align,
            line_height: self.paragraph.line_height,
            width: self.width,
        }
    }
}

impl Hash for ParagraphQuery<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for (span, inline) in self.paragraph.spans.iter().zip(&self.paragraph.boxes) {
            span.text.hash(state);
            face_of(span).hash(state);
            span.family.hash(state);
            size_bits(span, self.font).hash(state);
            inline.hash(state);
        }
        self.paragraph.align.hash(state);
        self.paragraph.line_height.hash(state);
        self.font.line_height.to_bits().hash(state);
        self.font.size.to_bits().hash(state);
        self.width.to_bits().hash(state);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The height of `paragraph` in `font` wrapped at `width`.
pub(crate) fn measure(
    fonts: &mut Fonts,
    font: ParagraphFont,
    paragraph: &Paragraph,
    width: f32,
) -> f32 {
    if let Some(indents) = paragraph.indents(font) {
        let cached = |query: &ParagraphQuery<'_>| fonts.paragraph_shape(query).clone();
        return shape_indented(paragraph, font, width, indents, cached)
            .shape
            .height;
    }
    let query = ParagraphQuery {
        font,
        paragraph,
        width,
    };
    fonts.paragraph_shape(&query).height
}

/// The width of `paragraph` in `font` set on one line, unwrapped: the
/// right edge of its last glyph.
pub(crate) fn measure_line(fonts: &mut Fonts, font: ParagraphFont, paragraph: &Paragraph) -> f32 {
    let right = |shape: &ParagraphShape| {
        shape
            .groups
            .iter()
            .map(|group| group.right)
            .fold(0.0, f32::max)
    };
    if let Some(indents) = paragraph.indents(font) {
        let cached = |query: &ParagraphQuery<'_>| fonts.paragraph_shape(query).clone();
        return right(&shape_indented(paragraph, font, UNWRAPPED_WIDTH, indents, cached).shape);
    }
    let query = ParagraphQuery {
        font,
        paragraph,
        width: UNWRAPPED_WIDTH,
    };
    right(fonts.paragraph_shape(&query))
}

/// Where `paragraph` in `font` lands shaped to fill `rect`'s width from
/// its top left, without drawing it.
pub(crate) fn layout_of(
    fonts: &mut Fonts,
    font: ParagraphFont,
    paragraph: &Paragraph,
    rect: Rect,
) -> ParagraphLayout {
    if let Some(indents) = paragraph.indents(font) {
        let cached = |query: &ParagraphQuery<'_>| fonts.paragraph_shape(query).clone();
        let indented = shape_indented(paragraph, font, rect.width, indents, cached);
        return layout_from(&indented.shape, paragraph, rect);
    }
    let query = ParagraphQuery {
        font,
        paragraph,
        width: rect.width,
    };
    layout_from(fonts.paragraph_shape(&query), paragraph, rect)
}

/// Where `paragraph` in `font` lands shaped with `font_system` to fill
/// `rect`'s width from its top left, shaped now and kept nowhere: for a
/// [`ParagraphMeasurer`](crate::ParagraphMeasurer)'s own font system.
pub(crate) fn layout_alone(
    font_system: &mut FontSystem,
    font: ParagraphFont,
    paragraph: &Paragraph,
    rect: Rect,
) -> ParagraphLayout {
    if let Some(indents) = paragraph.indents(font) {
        let alone = |query: &ParagraphQuery<'_>| query.shape(font_system);
        let indented = shape_indented(paragraph, font, rect.width, indents, alone);
        return layout_from(&indented.shape, paragraph, rect);
    }
    let query = ParagraphQuery {
        font,
        paragraph,
        width: rect.width,
    };
    layout_from(&query.shape(font_system), paragraph, rect)
}

/// `paragraph` in `font` and `theme`'s colors, shaped to fill `rect`'s
/// width from its top left, clipped to it.
pub(crate) fn lay_out(
    fonts: &mut Fonts,
    font: ParagraphFont,
    paragraph: &Paragraph,
    rect: Rect,
    theme: &Theme,
) -> ShapedParagraph {
    if let Some(indents) = paragraph.indents(font) {
        return lay_out_indented(fonts, font, paragraph, rect, theme, indents);
    }
    let query = ParagraphQuery {
        font,
        paragraph,
        width: rect.width,
    };
    let shape = fonts.paragraph_shape(&query);
    let runs = paragraph.runs(Some(theme), font);
    let SpanQuads { decorations, fills } = span_quads(shape, paragraph, &runs, rect, theme);
    let layout = layout_from(shape, paragraph, rect);
    let segment = TextSegment {
        align: SegmentAlign::Start,
        clip: Some(rect),
        origin: Point::new(rect.x, rect.y),
        runs,
        transform: None,
        wrap: Some(query.wrapping()),
    };
    ShapedParagraph {
        decorations,
        fills,
        layout,
        segments: vec![segment],
    }
}

/// [`lay_out`] for `paragraph`, which has `indents`: its first line drawn
/// from the full text shaped at the width the first indent leaves and cut
/// to that line, the rest drawn as a paragraph of their own below it at
/// the width the other indent leaves.
fn lay_out_indented(
    fonts: &mut Fonts,
    font: ParagraphFont,
    paragraph: &Paragraph,
    rect: Rect,
    theme: &Theme,
    indents: (f32, f32),
) -> ShapedParagraph {
    let cached = |query: &ParagraphQuery<'_>| fonts.paragraph_shape(query).clone();
    let indented = shape_indented(paragraph, font, rect.width, indents, cached);
    let runs = paragraph.runs(Some(theme), font);
    let SpanQuads { decorations, fills } =
        span_quads(&indented.shape, paragraph, &runs, rect, theme);
    let layout = layout_from(&indented.shape, paragraph, rect);
    let bottom = indented.first_bottom;
    let wrapping = |paragraph: &Paragraph, indent: f32| Wrapping {
        align: paragraph.align,
        line_height: paragraph.line_height,
        width: (rect.width - indent).max(0.0),
    };
    let first_line = Rect::new(rect.x, rect.y, rect.width, bottom.min(rect.height));
    let mut segments = vec![TextSegment {
        align: SegmentAlign::Start,
        clip: Some(first_line),
        origin: Point::new(rect.x + indented.first, rect.y),
        runs,
        transform: None,
        wrap: Some(wrapping(paragraph, indented.first)),
    }];
    if let Some((tail, indent)) = &indented.rest {
        let below = Rect::new(
            rect.x,
            rect.y + bottom,
            rect.width,
            (rect.height - bottom).max(0.0),
        );
        segments.push(TextSegment {
            align: SegmentAlign::Start,
            clip: Some(below),
            origin: Point::new(rect.x + indent, rect.y + bottom),
            runs: tail.runs(Some(theme), font),
            transform: None,
            wrap: Some(wrapping(tail, *indent)),
        });
    }
    ShapedParagraph {
        decorations,
        fills,
        layout,
        segments,
    }
}

/// `paragraph`, which has `indents` (the first line's and the others', in
/// pixels), shaped at `width` by `shape`: shaped whole at the width the
/// first indent leaves for its first line, then the text after that line
/// shaped on its own at the width the other indent leaves, the two put
/// together with each line moved in by its indent and the second's spans
/// numbered as the whole paragraph's.
fn shape_indented(
    paragraph: &Paragraph,
    font: ParagraphFont,
    width: f32,
    (first, rest): (f32, f32),
    mut shape: impl FnMut(&ParagraphQuery<'_>) -> ParagraphShape,
) -> IndentedShape {
    let whole = shape(&ParagraphQuery {
        font,
        paragraph,
        width: (width - first).max(0.0),
    });
    let text_len: usize = paragraph.spans.iter().map(|span| span.text.len()).sum();
    let line = match whole.lines.first() {
        Some(line) if whole.lines.len() > 1 && line.end < text_len => line.clone(),
        Some(_) | None => {
            return IndentedShape {
                first,
                first_bottom: whole.height,
                rest: None,
                shape: moved(&whole, first, 0.0, 0, 0),
            };
        }
    };
    let bottom = line.top + line.height;
    let (tail, base) = paragraph.split_from(line.end);
    let after = shape(&ParagraphQuery {
        font,
        paragraph: &tail,
        width: (width - rest).max(0.0),
    });
    let first_only = ParagraphShape {
        boxes: whole
            .boxes
            .iter()
            .filter(|inline| inline.y < bottom)
            .copied()
            .collect(),
        groups: whole
            .groups
            .iter()
            .filter(|group| group.line_top == line.top)
            .cloned()
            .collect(),
        height: bottom,
        lines: Arc::new(vec![line.clone()]),
    };
    let head = moved(&first_only, first, 0.0, 0, 0);
    let tail_shape = moved(&after, rest, bottom, line.end, base);
    let lines = head
        .lines
        .iter()
        .chain(tail_shape.lines.iter())
        .cloned()
        .collect();
    IndentedShape {
        first,
        first_bottom: bottom,
        rest: Some((tail, rest)),
        shape: ParagraphShape {
            boxes: head.boxes.into_iter().chain(tail_shape.boxes).collect(),
            groups: head.groups.into_iter().chain(tail_shape.groups).collect(),
            height: bottom + after.height,
            lines: Arc::new(lines),
        },
    }
}

/// `shape` moved `across` pixels right and `down` pixels down, its text
/// offsets `offset` bytes on and its spans `spans` on, as a part of a
/// larger paragraph starting there.
fn moved(
    shape: &ParagraphShape,
    across: f32,
    down: f32,
    offset: usize,
    spans: usize,
) -> ParagraphShape {
    let lines = shape
        .lines
        .iter()
        .map(|line| LineGeometry {
            baseline: line.baseline + down,
            end: line.end + offset,
            glyphs: line
                .glyphs
                .iter()
                .map(|glyph| GlyphGeometry {
                    end: glyph.end + offset,
                    left: glyph.left + across,
                    right: glyph.right + across,
                    start: glyph.start + offset,
                    ..*glyph
                })
                .collect(),
            height: line.height,
            start: line.start + offset,
            top: line.top + down,
        })
        .collect();
    ParagraphShape {
        boxes: shape
            .boxes
            .iter()
            .map(|inline| {
                Rect::new(
                    inline.x + across,
                    inline.y + down,
                    inline.width,
                    inline.height,
                )
            })
            .collect(),
        groups: shape
            .groups
            .iter()
            .map(|group| GlyphGroup {
                left: group.left + across,
                line_top: group.line_top + down,
                line_y: group.line_y + down,
                right: group.right + across,
                span: group.span + spans,
                ..group.clone()
            })
            .collect(),
        height: shape.height + down,
        lines: Arc::new(lines),
    }
}

/// `paragraph` in `font`, wrapped at `width` pixels and drawn in `theme`'s
/// colors over `background`, shaped with `font_system`: an image as wide as
/// `width` (at least a pixel) and as tall as its lines, holding its span
/// backgrounds, glyphs, and lines as a frame draws them. An inline box's
/// room is left as the background.
pub(crate) fn render_alone(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    font: ParagraphFont,
    paragraph: &Paragraph,
    width: f32,
    theme: &Theme,
    background: Rgba,
) -> Image {
    if let Some(indents) = paragraph.indents(font) {
        return render_indented(
            (font_system, swash_cache),
            font,
            paragraph,
            (width, indents),
            theme,
            background,
        );
    }
    let query = ParagraphQuery {
        font,
        paragraph,
        width,
    };
    let shape = query.shape(font_system);
    let canvas_width = (non_negative(width).ceil() as u32).max(1);
    let canvas_height = (shape.height.ceil() as u32).max(1);
    let rect = Rect::new(0.0, 0.0, canvas_width as f32, canvas_height as f32);
    let runs = paragraph.runs(Some(theme), font);
    let SpanQuads { decorations, fills } = span_quads(&shape, paragraph, &runs, rect, theme);
    let mut canvas = Canvas::filled(canvas_width, canvas_height, background);
    for quad in &fills {
        canvas.fill(quad.rect, quad.color);
    }
    let metrics = Metrics::new(font.size, font.line_height);
    let wrap = Some(query.wrapping());
    let mut buffer = shape_text(font_system, metrics, None, &runs, wrap, true, font.weights);
    let foreground = theme.foreground;
    let default = Color::rgb(foreground.r, foreground.g, foreground.b);
    buffer.draw(font_system, swash_cache, default, |x, y, w, h, color| {
        let color = Rgba::new(color.r(), color.g(), color.b(), color.a());
        canvas.fill(Rect::new(x as f32, y as f32, w as f32, h as f32), color);
    });
    for quad in &decorations {
        canvas.fill(quad.rect, quad.color);
    }
    Image::from_rgba(canvas_width, canvas_height, canvas.pixels)
        .expect("a paragraph's image is at least a pixel, its pixels filling it")
}

/// [`render_alone`] for `paragraph`, which has `indents`, wrapped at
/// `width`: its first line from the full text shaped at the width the
/// first indent leaves, cut to that line, and the rest shaped on their own
/// below it, each moved in by its indent.
fn render_indented(
    (font_system, swash_cache): (&mut FontSystem, &mut SwashCache),
    font: ParagraphFont,
    paragraph: &Paragraph,
    (width, indents): (f32, (f32, f32)),
    theme: &Theme,
    background: Rgba,
) -> Image {
    let alone = |query: &ParagraphQuery<'_>| query.shape(font_system);
    let indented = shape_indented(paragraph, font, width, indents, alone);
    let canvas_width = (non_negative(width).ceil() as u32).max(1);
    let canvas_height = (indented.shape.height.ceil() as u32).max(1);
    let rect = Rect::new(0.0, 0.0, canvas_width as f32, canvas_height as f32);
    let runs = paragraph.runs(Some(theme), font);
    let SpanQuads { decorations, fills } =
        span_quads(&indented.shape, paragraph, &runs, rect, theme);
    let mut canvas = Canvas::filled(canvas_width, canvas_height, background);
    for quad in &fills {
        canvas.fill(quad.rect, quad.color);
    }
    let metrics = Metrics::new(font.size, font.line_height);
    let foreground = theme.foreground;
    let default = Color::rgb(foreground.r, foreground.g, foreground.b);
    let bottom = indented.first_bottom;
    let tail = indented
        .rest
        .as_ref()
        .map(|(tail, indent)| (tail.runs(Some(theme), font), tail, *indent));
    let pieces = [(runs, paragraph, indented.first, 0.0, Some(bottom))]
        .into_iter()
        .chain(tail.map(|(runs, tail, indent)| (runs, tail, indent, bottom, None)));
    for (runs, piece, indent, down, cut) in pieces {
        let wrap = Wrapping {
            align: piece.align,
            line_height: piece.line_height,
            width: (width - indent).max(0.0),
        };
        let mut buffer = shape_text(
            font_system,
            metrics,
            None,
            &runs,
            Some(wrap),
            true,
            font.weights,
        );
        buffer.draw(font_system, swash_cache, default, |x, y, w, h, color| {
            if cut.is_some_and(|cut| y as f32 >= cut) {
                return;
            }
            let color = Rgba::new(color.r(), color.g(), color.b(), color.a());
            let at = Rect::new(x as f32 + indent, y as f32 + down, w as f32, h as f32);
            canvas.fill(at, color);
        });
    }
    for quad in &decorations {
        canvas.fill(quad.rect, quad.color);
    }
    Image::from_rgba(canvas_width, canvas_height, canvas.pixels)
        .expect("a paragraph's image is at least a pixel, its pixels filling it")
}

/// The backgrounds and lines of `paragraph`'s spans, shaped as `shape`
/// from `rect`'s top left in the colors `runs` resolved, clipped to it.
fn span_quads(
    shape: &ParagraphShape,
    paragraph: &Paragraph,
    runs: &[TextRun],
    rect: Rect,
    theme: &Theme,
) -> SpanQuads {
    let mut decorations = Vec::new();
    let mut fills = Vec::new();
    for group in &shape.groups {
        let Some(span) = paragraph.spans.get(group.span) else {
            continue;
        };
        let color = runs
            .get(group.span)
            .map_or(theme.foreground, |run| run.color);
        let color = color.with_alpha(span.color.resolve_rgba(theme).a);
        let width = group.right - group.left;
        let thickness = (group.size * UNDERLINE_SHARE).round().max(1.0);
        let underline_color = span
            .underline_color
            .map_or(color, |line| line.resolve_rgba(theme));
        let mut lines = Vec::new();
        if paragraph.is_underlined(group.span) {
            lines.push((group.line_y + thickness, underline_color));
        }
        if span.strikethrough {
            let top = group.line_y - group.size * STRIKE_RAISE - thickness / 2.0;
            lines.push((top, color));
        }
        for (top, color) in lines {
            let line = Rect::new(rect.x + group.left, rect.y + top, width, thickness);
            if let Some(line) = line.intersection(&rect) {
                decorations.push(Quad::solid(line, color));
            }
        }
        if let Some(background) = span.background {
            let area = Rect::new(
                rect.x + group.left,
                rect.y + group.line_top,
                width,
                group.line_height,
            );
            if let Some(area) = area.intersection(&rect) {
                fills.push(Quad::solid(area, background.resolve_rgba(theme)));
            }
        }
    }
    SpanQuads { decorations, fills }
}

/// Where `paragraph`, shaped as `shape`, lands from `rect`'s top left.
fn layout_from(shape: &ParagraphShape, paragraph: &Paragraph, rect: Rect) -> ParagraphLayout {
    let links = shape
        .groups
        .iter()
        .filter_map(|group| {
            let target = paragraph.links.get(group.span)?.as_ref()?;
            Some(LinkArea {
                rect: Rect::new(
                    rect.x + group.left,
                    rect.y + group.line_top,
                    group.right - group.left,
                    group.line_height,
                ),
                target: target.clone(),
            })
        })
        .collect();
    let boxes = shape
        .boxes
        .iter()
        .map(|inline| {
            Rect::new(
                rect.x + inline.x,
                rect.y + inline.y,
                inline.width,
                inline.height,
            )
        })
        .collect();
    ParagraphLayout {
        boxes,
        height: shape.height,
        lines: Arc::clone(&shape.lines),
        links,
        origin: Point::new(rect.x, rect.y),
    }
}

/// The bits of `span`'s font size in `font`, or `None` when it is
/// `font`'s own.
fn size_bits(span: &Span, font: ParagraphFont) -> Option<u32> {
    span.size
        .resolve(font.size, font.scale_factor)
        .map(f32::to_bits)
}

/// The lines of `buffer`, their glyphs placed in the text the buffer was
/// set from (its lines joined by their line endings).
fn lines_of(buffer: &Buffer) -> Vec<LineGeometry> {
    let mut starts = Vec::with_capacity(buffer.lines.len());
    let mut start = 0;
    for line in &buffer.lines {
        starts.push(start);
        start += line.text().len() + line.ending().as_str().len();
    }
    // The last line's ending is the buffer's, past the paragraph's text.
    let text_end = buffer
        .lines
        .last()
        .map_or(0, |line| start - line.ending().as_str().len());
    let mut lines: Vec<LineGeometry> = buffer
        .layout_runs()
        .map(|run| {
            let base = starts.get(run.line_i).copied().unwrap_or(0);
            let glyphs: Vec<GlyphGeometry> = run
                .glyphs
                .iter()
                .map(|glyph| GlyphGeometry {
                    end: base + glyph.end,
                    left: glyph.x,
                    right: glyph.x + glyph.w,
                    rtl: glyph.level.is_rtl(),
                    start: base + glyph.start,
                })
                .collect();
            LineGeometry {
                baseline: run.line_y,
                // Filled in below, from the next line's start.
                end: text_end,
                start: glyphs.iter().map(|glyph| glyph.start).min().unwrap_or(base),
                glyphs,
                height: run.line_height,
                top: run.line_top,
            }
        })
        .collect();
    let next_starts: Vec<usize> = lines.iter().skip(1).map(|line| line.start).collect();
    for (line, next) in lines.iter_mut().zip(next_starts) {
        line.end = next.max(line.start);
    }
    lines
}

/// The bits of `value`, or `None` when it is not a positive number.
fn positive_bits(value: f32) -> Option<u32> {
    (value.is_finite() && value > 0.0).then(|| value.to_bits())
}

/// `value`, or 0 when it is not a positive number.
fn non_negative(value: f32) -> f32 {
    match value.is_finite() && value > 0.0 {
        true => value,
        false => 0.0,
    }
}

/// The weight and slant of `span`.
fn face_of(span: &Span) -> Face {
    Face {
        bold: span.bold,
        italic: span.italic,
    }
}

/// The height of every line of `buffer`.
fn height_of(buffer: &Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_top + run.line_height)
        .fold(0.0, f32::max)
}

/// Consecutive glyphs of one line from the same span, as `(span, left,
/// right)`, from glyphs given as `(span, x, width)`.
fn glyph_groups(glyphs: impl Iterator<Item = (usize, f32, f32)>) -> Vec<(usize, f32, f32)> {
    let mut groups: Vec<(usize, f32, f32)> = Vec::new();
    for (span, x, width) in glyphs {
        match groups.last_mut() {
            Some((last, left, right)) if *last == span => {
                *left = left.min(x);
                *right = right.max(x + width);
            }
            Some(_) | None => groups.push((span, x, x + width)),
        }
    }
    groups
}

/// `color` drawn over the straight-alpha RGBA `pixel`.
fn blend_over(pixel: &mut [u8], color: Rgba) {
    let alpha = f32::from(color.a) / 255.0;
    let below = f32::from(pixel[3]) / 255.0 * (1.0 - alpha);
    let out = alpha + below;
    if out <= 0.0 {
        return;
    }
    let mix = |over: u8, under: u8| {
        ((f32::from(over) * alpha + f32::from(under) * below) / out).round() as u8
    };
    pixel[0] = mix(color.r, pixel[0]);
    pixel[1] = mix(color.g, pixel[1]);
    pixel[2] = mix(color.b, pixel[2]);
    pixel[3] = (out * 255.0).round() as u8;
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock, PoisonError};

    use super::super::KEEP_FRAMES;
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::styled::TextSize;
    use crate::base::theme::ThemeToken;

    const FONT: ParagraphFont = ParagraphFont {
        line_height: 20.0,
        scale_factor: 1.0,
        size: 15.0,
        weights: FaceWeights::DEFAULT,
    };

    const TEXT: TextColor = TextColor::Token(ThemeToken::Foreground);

    /// The system fonts, loaded once for these tests.
    fn with_fonts<R>(use_fonts: impl FnOnce(&mut Fonts) -> R) -> R {
        static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
        let fonts = FONTS.get_or_init(|| Mutex::new(Fonts::new()));
        use_fonts(&mut fonts.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn sentence() -> Paragraph {
        Paragraph::plain("The quick brown fox jumps over the lazy dog. ", TEXT)
            .with(Span::new("Bold words follow ", TEXT).bold())
            .with_link(
                Span::new("a link", TextColor::Token(ThemeToken::Accent)),
                "misty:docs",
            )
            .with(Span::new(" and the end.", TEXT).italic())
    }

    #[test]
    fn test_a_paragraph_is_shaped_once_per_width_across_measuring_and_drawing() {
        let mut fonts = Fonts::new();
        let rect = Rect::new(10.0, 20.0, 300.0, 100.0);
        let height = measure(&mut fonts, FONT, &sentence(), 300.0);
        assert_eq!(measure(&mut fonts, FONT, &sentence(), 300.0), height);
        let drawn = lay_out(&mut fonts, FONT, &sentence(), rect, &Theme::light());
        assert_eq!(drawn.layout.height(), height);
        assert_eq!(fonts.paragraph_count(), 1, "one shape for one width");
        measure(&mut fonts, FONT, &sentence(), 120.0);
        measure(
            &mut fonts,
            FONT,
            &sentence().with_align(TextAlign::End),
            120.0,
        );
        assert_eq!(fonts.paragraph_count(), 3, "another width or alignment");
        let recolored = Paragraph::plain("The quick brown fox jumps over the lazy dog. ", TEXT)
            .with(Span::new("Bold words follow ", TextColor::Token(ThemeToken::Danger)).bold())
            .with_link(Span::new("a link", TEXT), "misty:docs")
            .with(Span::new(" and the end.", TEXT).italic());
        measure(&mut fonts, FONT, &recolored, 300.0);
        assert_eq!(fonts.paragraph_count(), 3, "colors do not change the shape");
        for _ in 0..=KEEP_FRAMES + 1 {
            fonts.finish_frame();
        }
        assert_eq!(fonts.paragraph_count(), 0, "unused shapes are dropped");
    }

    #[test]
    fn test_a_narrower_width_wraps_to_more_lines() {
        let (wide, narrow) = with_fonts(|fonts| {
            (
                measure(fonts, FONT, &sentence(), 2000.0),
                measure(fonts, FONT, &sentence(), 120.0),
            )
        });
        assert_eq!(wide, FONT.line_height, "one line when it fits");
        assert!(narrow >= 4.0 * FONT.line_height, "{narrow}");
        assert_eq!(narrow % FONT.line_height, 0.0, "whole lines");
    }

    #[test]
    fn test_a_link_is_found_where_it_is_drawn_and_nowhere_else() {
        let rect = Rect::new(10.0, 20.0, 2000.0, 100.0);
        let shaped = with_fonts(|fonts| lay_out(fonts, FONT, &sentence(), rect, &Theme::dark()));
        let link = shaped.layout.links.first().expect("a link area").rect;
        let inside = Point::new(link.x + link.width / 2.0, link.y + link.height / 2.0);
        assert_eq!(shaped.layout.link_at(inside), Some("misty:docs"));
        assert_eq!(
            shaped.layout.link_at(Point::new(rect.x + 1.0, inside.y)),
            None,
            "plain text"
        );
        assert_eq!(shaped.decorations.len(), 1, "only the link is underlined");
        assert!(
            shaped.decorations[0].rect.y > link.y
                && shaped.decorations[0].rect.y < link.y + link.height
        );
    }

    #[test]
    fn test_end_alignment_pushes_a_short_line_to_the_right_edge() {
        let paragraph = Paragraph::new()
            .with_link(Span::new("right", TEXT), "r")
            .with_align(TextAlign::End);
        let rect = Rect::new(0.0, 0.0, 400.0, 40.0);
        let shaped = with_fonts(|fonts| lay_out(fonts, FONT, &paragraph, rect, &Theme::dark()));
        let link = shaped.layout.links[0].rect;
        assert!((link.x + link.width - 400.0).abs() < 1.0, "{link:?}");
    }

    #[test]
    fn test_start_alignment_follows_each_line_s_direction() {
        let rect = Rect::new(0.0, 0.0, 400.0, 40.0);
        let edges = |text: &str, align| {
            let paragraph = Paragraph::new()
                .with_link(Span::new(text, TEXT), "x")
                .with_align(align);
            let shaped = with_fonts(|fonts| lay_out(fonts, FONT, &paragraph, rect, &Theme::dark()));
            let link = shaped.layout.links[0].rect;
            (link.x, link.x + link.width)
        };
        let (left, _) = edges("left", TextAlign::Start);
        assert!(left.abs() < 1.0, "left-to-right text starts at the left");
        let (_, right) = edges("שלום עולם", TextAlign::Start);
        assert!(
            (right - 400.0).abs() < 1.0,
            "Hebrew starts at the right: {right}"
        );
        let (left, _) = edges("שלום עולם", TextAlign::End);
        assert!(left.abs() < 1.0, "and ends at the left: {left}");
    }

    #[test]
    fn test_glyphs_group_by_span_along_a_line() {
        let groups = glyph_groups(
            [(0, 0.0, 5.0), (0, 5.0, 5.0), (1, 10.0, 4.0), (0, 14.0, 2.0)].into_iter(),
        );
        assert_eq!(groups, [(0, 0.0, 10.0), (1, 10.0, 14.0), (0, 14.0, 16.0)]);
    }

    #[test]
    fn test_a_paragraph_draws_wrapped_lines_in_its_colors_with_an_underlined_link() {
        let theme = Theme::dark();
        let paragraph = Paragraph::plain("Wrapped words in the foreground color ", TEXT).with_link(
            Span::new("then a link", TextColor::Token(ThemeToken::Accent)),
            "x",
        );
        let rect = Rect::new(8.0, 8.0, 150.0, 120.0);
        let mut drawn = ParagraphLayout::default();
        let Some(snapshot) = offscreen::render(200, 140, |frame| {
            frame.clear(Rgba::from(theme.background));
            drawn = frame.draw_paragraph(&paragraph, rect, &theme);
        }) else {
            return;
        };
        let line = snapshot.metrics().height;
        assert!(drawn.height() >= 2.0 * line, "it wrapped");
        let first = Rect::new(rect.x, rect.y, rect.width, line);
        assert!(
            snapshot.contains_color(first, theme.foreground),
            "the first line"
        );
        let link = drawn.links[0].rect;
        assert!(
            snapshot.contains_color(link, theme.accent),
            "the link in its color"
        );
        let below = Rect::new(rect.x, rect.y + drawn.height(), rect.width, 8.0);
        assert!(
            !snapshot.contains_color(below, theme.foreground),
            "nothing past it"
        );
    }

    #[test]
    fn test_every_caret_maps_back_to_its_offset_and_points_off_the_text_clamp() {
        let paragraph = Paragraph::plain("Hello, wide world", TEXT);
        let rect = Rect::new(10.0, 20.0, 2000.0, 100.0);
        let layout = with_fonts(|fonts| layout_of(fonts, FONT, &paragraph, rect));
        let text = paragraph.text();
        let mut last_x = f32::NEG_INFINITY;
        for (offset, _) in text.char_indices().chain([(text.len(), ' ')]) {
            let caret = layout.caret_rect(offset);
            assert!(caret.x > last_x, "carets move right: {offset}");
            last_x = caret.x;
            assert_eq!((caret.y, caret.height), (rect.y, FONT.line_height));
            let middle = Point::new(caret.x, caret.y + caret.height / 2.0);
            assert_eq!(layout.offset_at(middle), offset, "{offset}");
        }
        assert_eq!(layout.offset_at(Point::new(0.0, 0.0)), 0, "above and left");
        assert_eq!(
            layout.offset_at(Point::new(5000.0, 500.0)),
            text.len(),
            "below and right"
        );
    }

    #[test]
    fn test_a_wrapped_paragraph_puts_later_offsets_on_lower_lines_and_a_range_per_line() {
        let rect = Rect::new(0.0, 0.0, 120.0, 400.0);
        let layout = with_fonts(|fonts| layout_of(fonts, FONT, &sentence(), rect));
        let text = sentence().text();
        let last = layout.caret_rect(text.len());
        assert!(last.y >= 3.0 * FONT.line_height, "{last:?}");
        let below = Point::new(1.0, last.y + 1.0);
        assert!(layout.offset_at(below) > text.len() / 2, "a lower line");
        let rects = layout.range_rects(0..text.len());
        assert_eq!(rects.len() as f32, layout.height() / FONT.line_height);
        assert!(rects.windows(2).all(|pair| pair[1].y > pair[0].y));
        assert!(layout.range_rects(3..3).is_empty(), "an empty range");
        let word = layout.range_rects(4..9);
        assert_eq!(word.len(), 1);
        let (start, end) = (layout.caret_rect(4), layout.caret_rect(9));
        assert!(
            (word[0].x - start.x).abs() < 0.5 && (word[0].x + word[0].width - end.x).abs() < 0.5
        );
    }

    #[test]
    fn test_wrapped_lines_cover_the_text_without_gaps_each_where_it_is_drawn() {
        let rect = Rect::new(7.0, 100.0, 120.0, 400.0);
        let layout = with_fonts(|fonts| layout_of(fonts, FONT, &sentence(), rect));
        let text = sentence().text();
        let lines: Vec<ParagraphLine> = layout.lines().collect();
        assert_eq!(lines.len() as f32, layout.height() / FONT.line_height);
        assert_eq!(lines[0].text.start, 0);
        assert_eq!(lines[lines.len() - 1].text.end, text.len());
        for pair in lines.windows(2) {
            assert_eq!(pair[0].text.end, pair[1].text.start, "{lines:?}");
            assert_eq!(pair[0].top + pair[0].height, pair[1].top);
        }
        assert_eq!(lines[0].top, 100.0, "from the window's top");
        for line in &lines {
            assert!(line.top < line.baseline && line.baseline < line.top + line.height);
            assert!(!line.text.is_empty(), "{line:?}");
            let caret = layout.caret_rect(line.text.start);
            assert_eq!(caret.y, line.top, "line {line:?} starts where its caret is");
            let inside = Point::new(rect.x + 1.0, line.top + 1.0);
            assert!(line.text.contains(&layout.offset_at(inside)));
        }
        let empty = with_fonts(|fonts| layout_of(fonts, FONT, &Paragraph::new(), rect));
        let empty: Vec<ParagraphLine> = empty.lines().collect();
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].text, 0..0);
    }

    #[test]
    fn test_an_underline_color_draws_the_line_in_it_and_the_strike_in_the_text_s() {
        let theme = Theme::dark();
        let paragraph = Paragraph::new()
            .with(
                Span::new("marked", TEXT)
                    .underline_color(ThemeToken::Danger)
                    .strikethrough(),
            )
            .with_link(
                Span::new(" link", TEXT).underline_color(ThemeToken::Warning),
                "x",
            )
            .with(Span::new(" plain", TEXT).underlined());
        let rect = Rect::new(0.0, 0.0, 2000.0, 100.0);
        let shaped = with_fonts(|fonts| lay_out(fonts, FONT, &paragraph, rect, &theme));
        let colors: Vec<Rgba> = shaped.decorations.iter().map(|quad| quad.color).collect();
        let text = TEXT.resolve_rgba(&theme);
        let error = TextColor::from(ThemeToken::Danger).resolve_rgba(&theme);
        let warning = TextColor::from(ThemeToken::Warning).resolve_rgba(&theme);
        assert_eq!(colors, [error, text, warning, text], "{colors:?}");
    }

    #[test]
    fn test_a_larger_span_makes_its_line_taller_and_a_smaller_one_does_not() {
        let measure_with = |size| {
            let paragraph =
                Paragraph::plain("Heading ", TEXT).with(Span::new("big", TEXT).size(size));
            with_fonts(|fonts| measure(fonts, FONT, &paragraph, 2000.0))
        };
        let plain = measure_with(TextSize::Scale(1.0));
        assert_eq!(plain, FONT.line_height);
        assert!(measure_with(TextSize::Scale(2.0)) >= 1.9 * plain);
        assert!(measure_with(TextSize::Pixels(40.0)) >= 2.5 * plain);
        assert_eq!(
            measure_with(TextSize::Scale(0.5)),
            plain,
            "the line keeps its height"
        );
        assert_eq!(
            measure_with(TextSize::Scale(f32::NAN)),
            plain,
            "an invalid size"
        );
    }

    #[test]
    fn test_a_monospace_span_is_set_in_another_font_and_an_unknown_family_falls_back() {
        let width = |family| {
            let paragraph = Paragraph::new().with(Span::new("iiiiiiii", TEXT).family(family));
            with_fonts(|fonts| measure_line(fonts, FONT, &paragraph))
        };
        let proportional = width(FontFamily::Proportional);
        assert!(width(FontFamily::Monospace) > proportional * 1.2);
        assert_eq!(
            width(FontFamily::Named("No Such Family Anywhere".to_owned())),
            proportional
        );
    }

    #[test]
    fn test_a_background_fills_behind_its_span_and_a_strikethrough_crosses_it() {
        let paragraph = Paragraph::plain("plain ", TEXT).with(
            Span::new("marked", TEXT)
                .background(ThemeToken::Warning)
                .strikethrough(),
        );
        let rect = Rect::new(0.0, 0.0, 2000.0, 100.0);
        let shaped = with_fonts(|fonts| lay_out(fonts, FONT, &paragraph, rect, &Theme::dark()));
        assert_eq!(shaped.fills.len(), 1);
        let fill = shaped.fills[0].rect;
        let marked = shaped.layout.range_rects(6..12)[0];
        assert!((fill.x - marked.x).abs() < 0.5 && (fill.width - marked.width).abs() < 0.5);
        assert_eq!(shaped.decorations.len(), 1, "the strikethrough");
        let strike = shaped.decorations[0].rect;
        assert!(
            strike.y > fill.y + fill.height / 4.0 && strike.y < fill.y + fill.height * 0.75,
            "through the middle: {strike:?} in {fill:?}"
        );
    }

    #[test]
    fn test_a_highlighted_span_draws_its_background_behind_its_text_only() {
        let theme = Theme::dark();
        let paragraph = Paragraph::plain("before ", TEXT)
            .with(Span::new("found", TEXT).background(ThemeToken::Warning));
        let rect = Rect::new(8.0, 8.0, 300.0, 40.0);
        let mut drawn = ParagraphLayout::default();
        let Some(snapshot) = offscreen::render(320, 60, |frame| {
            frame.clear(Rgba::from(theme.background));
            drawn = frame.draw_paragraph(&paragraph, rect, &theme);
        }) else {
            return;
        };
        let found = drawn.range_rects(7..12)[0];
        assert!(
            snapshot.contains_color(found, theme.warning),
            "the highlight"
        );
        let before = drawn.range_rects(0..6)[0];
        assert!(
            !snapshot.contains_color(before, theme.warning),
            "not before it"
        );
    }

    /// The right edge of the last glyph that is not a space, line by line.
    fn inked_right_edges(layout: &ParagraphLayout, text: &str) -> Vec<f32> {
        layout
            .lines
            .iter()
            .map(|line| {
                line.glyphs
                    .iter()
                    .filter(|glyph| !text[glyph.start..glyph.end].trim().is_empty())
                    .map(|glyph| glyph.right)
                    .fold(0.0, f32::max)
            })
            .collect()
    }

    #[test]
    fn test_justified_lines_fill_the_width_but_for_the_last() {
        let width = 160.0;
        let rect = Rect::new(0.0, 0.0, width, 400.0);
        let text = sentence().text();
        let (justified, start) = with_fonts(|fonts| {
            (
                layout_of(
                    fonts,
                    FONT,
                    &sentence().with_align(TextAlign::Justify),
                    rect,
                ),
                layout_of(fonts, FONT, &sentence(), rect),
            )
        });
        let edges = inked_right_edges(&justified, &text);
        assert!(edges.len() >= 3, "it wrapped: {edges:?}");
        let (last, wrapped) = edges.split_last().expect("lines");
        for edge in wrapped {
            assert!((edge - width).abs() < 1.0, "a full line: {edges:?}");
        }
        assert!(
            *last < width - 10.0,
            "the last line is not spread: {edges:?}"
        );
        assert_eq!(
            inked_right_edges(&start, &text).last(),
            Some(last),
            "it keeps to the start edge"
        );
        assert_eq!(justified.height(), start.height(), "the same lines");
    }

    #[test]
    fn test_an_inline_box_is_as_wide_as_asked_sits_on_the_baseline_and_lifts_its_line() {
        let inline = InlineBox::new(40.0, 50.0);
        let paragraph = Paragraph::plain("before ", TEXT)
            .with_box(inline)
            .with(Span::new(" after", TEXT));
        let rect = Rect::new(10.0, 20.0, 2000.0, 200.0);
        let (layout, plain) = with_fonts(|fonts| {
            (
                layout_of(fonts, FONT, &paragraph, rect),
                layout_of(fonts, FONT, &Paragraph::plain("before x after", TEXT), rect),
            )
        });
        let [placed] = layout.inline_boxes() else {
            panic!("one box: {:?}", layout.inline_boxes());
        };
        assert!((placed.width - 40.0).abs() < 0.01, "{placed:?}");
        assert_eq!(placed.height, 50.0);
        assert!(placed.y >= rect.y - 0.01, "inside its line: {placed:?}");
        let before = layout.range_rects(0..6)[0];
        assert!(
            (placed.x - (before.x + before.width)).abs() < 8.0,
            "after the word"
        );
        // The box's bottom is the baseline: the text's baseline is as far
        // above the line's bottom as on a line of text alone.
        let line = layout.lines[0].clone();
        let text_descent = plain.height() - plain_baseline(&plain);
        let bottom = placed.y + placed.height - rect.y;
        assert!(
            (line.height - bottom - text_descent).abs() < 0.5,
            "on the baseline: {bottom} in {line:?}, descent {text_descent}"
        );
        assert!(layout.height() > FONT.line_height, "a taller line");
        assert!(
            layout.height() < 50.0 + FONT.line_height,
            "{}",
            layout.height()
        );
        let middle = Point::new(placed.x + placed.width * 0.25, placed.y + 1.0);
        assert_eq!(layout.offset_at(middle), 7, "before the box");
        let right = Point::new(placed.x + placed.width * 0.75, placed.y + 1.0);
        assert_eq!(
            layout.offset_at(right),
            7 + inline_font::CHAR.len_utf8(),
            "after it"
        );
        assert_eq!(
            layout.range_rects(7..10),
            vec![Rect::new(placed.x, rect.y, placed.width, line.height)]
        );
    }

    /// How far below the top of a one-line layout its baseline is, from a
    /// box at the baseline's height zero.
    fn plain_baseline(layout: &ParagraphLayout) -> f32 {
        let paragraph = Paragraph::plain("before x after", TEXT).with_box(InlineBox::new(1.0, 0.0));
        let rect = Rect::new(layout.origin.x, layout.origin.y, 2000.0, 200.0);
        let probe = with_fonts(|fonts| layout_of(fonts, FONT, &paragraph, rect));
        assert_eq!(
            probe.height(),
            layout.height(),
            "a flat box changes nothing"
        );
        probe.inline_boxes()[0].y - rect.y
    }

    #[test]
    fn test_a_box_below_the_baseline_fits_its_line_and_wraps_like_a_word() {
        let inline = InlineBox::new(30.0, 40.0).with_baseline(10.0);
        let paragraph = Paragraph::plain("one two three four five six ", TEXT)
            .with_box(inline)
            .with(Span::new(" seven", TEXT));
        let rect = Rect::new(0.0, 0.0, 2000.0, 400.0);
        let wide = with_fonts(|fonts| layout_of(fonts, FONT, &paragraph, rect));
        let placed = wide.inline_boxes()[0];
        assert!(placed.y >= -0.01 && placed.y + placed.height <= wide.height() + 0.01);
        let narrow_width = placed.x + placed.width / 2.0;
        let narrow = Rect::new(0.0, 0.0, narrow_width, 400.0);
        let wrapped = with_fonts(|fonts| layout_of(fonts, FONT, &paragraph, narrow));
        let moved = wrapped.inline_boxes()[0];
        let line = wrapped
            .lines
            .iter()
            .find(|line| line.top <= moved.y && moved.y < line.top + line.height)
            .expect("the box's line");
        assert!(line.top > 0.0, "the box went to a later line");
        assert!(
            moved.y + moved.height <= line.top + line.height + 0.01,
            "{moved:?} {line:?}"
        );
        let first = &wrapped.lines[0];
        assert_eq!(
            first.height, FONT.line_height,
            "lines without it keep their height"
        );
    }

    #[test]
    fn test_an_inline_box_draws_nothing_and_is_where_its_layout_says() {
        let theme = Theme::dark();
        let paragraph = Paragraph::plain("Text then ", TEXT)
            .with_box(InlineBox::new(30.0, 30.0))
            .with(Span::new(" more text", TEXT));
        let rect = Rect::new(8.0, 8.0, 300.0, 60.0);
        let mut drawn = ParagraphLayout::default();
        let mut laid_out = ParagraphLayout::default();
        let Some(snapshot) = offscreen::render(320, 80, |frame| {
            frame.clear(Rgba::from(theme.background));
            laid_out = frame.paragraph_layout(&paragraph, rect);
            drawn = frame.draw_paragraph(&paragraph, rect, &theme);
        }) else {
            return;
        };
        assert_eq!(drawn, laid_out);
        let placed = drawn.inline_boxes()[0];
        assert!(snapshot.is_plain(placed, theme.background), "{placed:?}");
        let words = drawn.range_rects(0..4)[0];
        assert!(!snapshot.is_plain(words, theme.background), "the text drew");
    }

    #[test]
    fn test_a_line_height_multiple_makes_every_line_that_tall_for_carets_and_hits() {
        const MULTIPLE: f32 = 2.0;
        let tall = sentence().with_line_height(MULTIPLE);
        let rect = Rect::new(0.0, 0.0, 120.0, 0.0);
        let (plain, layout) = with_fonts(|fonts| {
            (
                layout_of(fonts, FONT, &sentence(), rect),
                layout_of(fonts, FONT, &tall, rect),
            )
        });
        let line = FONT.size * MULTIPLE;
        assert_eq!(
            layout.lines().len(),
            plain.lines().len(),
            "the same wrapping"
        );
        assert_eq!(layout.height(), line * layout.lines().len() as f32);
        let second = layout.lines().nth(1).expect("two lines");
        assert_eq!((second.top, second.height), (line, line));
        let caret = layout.caret_rect(second.text.start);
        assert_eq!((caret.y, caret.height), (line, line));
        let inside = Point::new(caret.x + 1.0, line + line / 2.0);
        assert_eq!(layout.offset_at(inside), second.text.start);
        let bigger = Paragraph::plain("Big", TEXT)
            .with(Span::new(" text", TEXT).size(TextSize::Scale(2.0)))
            .with_line_height(MULTIPLE);
        let height = with_fonts(|fonts| measure(fonts, FONT, &bigger, 1000.0));
        assert_eq!(height, 2.0 * FONT.size * MULTIPLE, "a larger span in step");
        let ignored = sentence().with_line_height(f32::NAN);
        assert_eq!(ignored, sentence(), "not a positive number");
    }

    #[test]
    fn test_a_taller_line_draws_its_text_in_the_middle_of_the_line() {
        let theme = Theme::dark();
        let paragraph = Paragraph::plain("Tall", TEXT).with_line_height(3.0);
        let rect = Rect::new(8.0, 8.0, 200.0, 120.0);
        let mut drawn = ParagraphLayout::default();
        let Some(snapshot) = offscreen::render(240, 140, |frame| {
            frame.clear(Rgba::from(theme.background));
            drawn = frame.draw_paragraph(&paragraph, rect, &theme);
        }) else {
            return;
        };
        let line = drawn.lines().next().expect("a line");
        let natural = snapshot.metrics().height;
        assert!(line.height > 2.0 * natural, "{} {natural}", line.height);
        let leading = (line.height - natural) / 2.0;
        let above = Rect::new(rect.x, line.top, rect.width, leading - 1.0);
        assert!(snapshot.is_plain(above, theme.background), "room above");
        let text = Rect::new(rect.x, line.top + leading, rect.width, natural);
        assert!(snapshot.contains_color(text, theme.foreground), "the text");
    }

    #[test]
    fn test_a_hanging_indent_lines_up_the_wrapped_lines_and_keeps_offsets_and_hits_whole() {
        let rect = Rect::new(10.0, 50.0, 160.0, 400.0);
        let hanging = sentence().with_hanging_indent(24.0);
        let (layout, plain, height) = with_fonts(|fonts| {
            (
                layout_of(fonts, FONT, &hanging, rect),
                layout_of(fonts, FONT, &sentence(), rect),
                measure(fonts, FONT, &hanging, rect.width),
            )
        });
        let text = sentence().text();
        let lines: Vec<ParagraphLine> = layout.lines().collect();
        assert!(lines.len() > 2, "{lines:?}");
        assert_eq!(height, layout.height(), "measured as laid out");
        assert!(height >= plain.height(), "narrower lines are no fewer");
        assert_eq!(
            (lines[0].text.start, lines[lines.len() - 1].text.end),
            (0, text.len())
        );
        for pair in lines.windows(2) {
            assert_eq!(pair[0].text.end, pair[1].text.start, "{lines:?}");
            assert_eq!(pair[0].top + pair[0].height, pair[1].top);
        }
        assert!(
            (layout.caret_rect(0).x - rect.x).abs() < 0.5,
            "the first line at the left"
        );
        for line in &lines[1..] {
            let caret = layout.caret_rect(line.text.start);
            assert!(caret.x >= rect.x + 24.0 - 0.5, "{line:?} at {caret:?}");
            let inside = Point::new(caret.x + 1.0, line.top + 1.0);
            assert!(line.text.contains(&layout.offset_at(inside)), "{line:?}");
        }
        let link = lines
            .iter()
            .find(|line| line.text.contains(&text.find("a link").expect("linked")))
            .expect("a line holds the link");
        let start = layout.caret_rect(text.find("a link").expect("linked"));
        let on_link = Point::new(start.x + 2.0, link.top + 2.0);
        assert_eq!(
            layout.link_at(on_link),
            Some("misty:docs"),
            "links move with their line"
        );
    }

    #[test]
    fn test_a_first_line_indent_moves_only_the_first_line_and_draws_two_pieces() {
        let theme = Theme::dark();
        let rect = Rect::new(0.0, 0.0, 160.0, 400.0);
        let indented = sentence().with_first_line_indent(30.0);
        let (layout, shaped, width) = with_fonts(|fonts| {
            (
                layout_of(fonts, FONT, &indented, rect),
                lay_out(fonts, FONT, &indented, rect, &theme),
                measure_line(
                    fonts,
                    FONT,
                    &Paragraph::plain("word", TEXT).with_first_line_indent(30.0),
                ),
            )
        });
        let plain_width =
            with_fonts(|fonts| measure_line(fonts, FONT, &Paragraph::plain("word", TEXT)));
        assert!(
            (width - plain_width - 30.0).abs() < 0.5,
            "{width} against {plain_width}"
        );
        assert!((layout.caret_rect(0).x - 30.0).abs() < 0.5);
        let second = layout.lines().nth(1).expect("wrapped");
        assert!(
            layout.caret_rect(second.text.start).x < 0.5,
            "the others at the left"
        );
        assert_eq!(shaped.segments.len(), 2);
        assert_eq!(shaped.segments[0].origin.x, 30.0);
        assert_eq!(shaped.segments[1].origin.y, second.top);
        assert_eq!(shaped.layout, layout);
        let none = sentence()
            .with_first_line_indent(-4.0)
            .with_hanging_indent(f32::NAN);
        assert_eq!(none, sentence(), "no indent at all");
    }

    #[test]
    fn test_a_hanging_indent_draws_the_wrapped_lines_clear_of_the_bullets_room() {
        let theme = Theme::dark();
        let paragraph = Paragraph::plain(
            "\u{2022} Wrapped words in the foreground color run on",
            TEXT,
        )
        .with_hanging_indent(24.0);
        let rect = Rect::new(8.0, 8.0, 150.0, 120.0);
        let mut drawn = ParagraphLayout::default();
        let Some(snapshot) = offscreen::render(200, 140, |frame| {
            frame.clear(Rgba::from(theme.background));
            drawn = frame.draw_paragraph(&paragraph, rect, &theme);
        }) else {
            return;
        };
        let lines: Vec<ParagraphLine> = drawn.lines().collect();
        assert!(lines.len() >= 2, "{lines:?}");
        let room = |line: &ParagraphLine| Rect::new(rect.x, line.top, 22.0, line.height);
        assert!(
            snapshot.contains_color(room(&lines[0]), theme.foreground),
            "the bullet"
        );
        for line in &lines[1..] {
            assert!(
                !snapshot.contains_color(room(line), theme.foreground),
                "{line:?} clear of the bullet's room"
            );
            let text = Rect::new(rect.x + 24.0, line.top, rect.width - 24.0, line.height);
            assert!(
                snapshot.contains_color(text, theme.foreground),
                "{line:?} drawn"
            );
        }
    }
}
