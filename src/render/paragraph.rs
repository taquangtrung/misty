//! Rich text in the proportional font: a [`Paragraph`] of styled spans
//! (bold, italic, underlined, struck through, in theme or exact colors, on
//! a background, larger or smaller, in another family, some of them
//! links) wrapped to a width and aligned, measured for layout with
//! [`Frame::measure_paragraph`] and drawn with [`Frame::draw_paragraph`],
//! which hands back a [`ParagraphLayout`]: where its links are, which
//! character is under a point, and where a caret or a selection goes.
//! [`Frame::paragraph_layout`] finds the same without drawing.
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

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use glyphon::{Buffer, FontSystem, Metrics};

use super::super::base::color::Rgb;
use super::super::base::geometry::{Point, Rect};
use super::super::base::styled::{FontFamily, Span, TextColor};
use super::super::base::theme::Theme;
use super::draw::{Face, Quad, RunStyle, SegmentAlign, TextRun, TextSegment, Wrapping};
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
    /// The target of each span that is a link, by span.
    links: Vec<Option<String>>,
    /// The runs, in order.
    spans: Vec<Span>,
}

/// Where the lines of a [`Paragraph`] sit in its width.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextAlign {
    /// Centered.
    Center,
    /// Against the end edge: the right in left-to-right text, the left in
    /// right-to-left text.
    End,
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
    /// The height of all its lines.
    height: f32,
    /// Its lines, relative to `origin`, shared with the cached shape.
    lines: Arc<Vec<LineGeometry>>,
    /// Each piece of a link on a line, with its target.
    links: Vec<LinkArea>,
    /// Its top left corner.
    origin: Point,
}

/// One line of a shaped paragraph, relative to the paragraph's top left.
#[derive(Clone, Debug, PartialEq)]
struct LineGeometry {
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
    /// The text and faces, in the default color.
    runs: Vec<TextRun>,
    /// The font size's bits.
    size: u32,
    /// The width's bits.
    width: u32,
}

/// A paragraph shaped at a width: where each span landed on each line,
/// relative to the paragraph's top left.
pub(crate) struct ParagraphShape {
    /// Consecutive glyphs of one span on one line, line by line.
    groups: Vec<GlyphGroup>,
    /// The height of all its lines.
    height: f32,
    /// Its lines and their glyphs.
    lines: Arc<Vec<LineGeometry>>,
}

/// Consecutive glyphs of one span on one line.
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
    /// The text for the renderer.
    pub(crate) segment: TextSegment,
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
        self
    }

    /// This paragraph with `span` added at its end as a link to `target`
    /// (a URL, an id, whatever the app resolves), drawn underlined.
    pub fn with_link(mut self, span: Span, target: impl Into<String>) -> Self {
        self.spans.push(span);
        self.links.push(Some(target.into()));
        self
    }

    /// This paragraph with its lines placed as `align` says.
    pub fn with_align(self, align: TextAlign) -> Self {
        Self { align, ..self }
    }

    /// The runs of text in `font` and `theme`'s colors, one per span.
    fn runs(&self, theme: Option<&Theme>, font: ParagraphFont) -> Vec<TextRun> {
        self.spans
            .iter()
            .map(|span| {
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
                        size: size_bits(span, font),
                    },
                    text: span.text.clone(),
                }
            })
            .collect()
    }

    /// Whether span `index` is drawn underlined: underlined or a link.
    fn is_underlined(&self, index: usize) -> bool {
        let link = self.links.get(index).is_some_and(Option::is_some);
        link || self.spans.get(index).is_some_and(|span| span.underline)
    }
}

// ========================================================================
// ParagraphLayout Implementations
// ========================================================================

impl ParagraphLayout {
    /// The height of all the paragraph's lines, in pixels.
    pub fn height(&self) -> f32 {
        self.height
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
            runs: query.paragraph.runs(None, query.font),
            size: query.font.size.to_bits(),
            width: query.width.to_bits(),
        }
    }

    /// Whether `key` is what `query` asks for.
    pub(crate) fn matches(key: &Self, query: &ParagraphQuery<'_>) -> bool {
        let spans = &query.paragraph.spans;
        key.align == query.paragraph.align
            && key.line_height == query.font.line_height.to_bits()
            && key.size == query.font.size.to_bits()
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
            groups,
            height: height_of(&buffer),
            lines: Arc::new(lines_of(&buffer)),
        }
    }

    /// How the paragraph wraps and aligns.
    fn wrapping(&self) -> Wrapping {
        Wrapping {
            align: self.paragraph.align,
            width: self.width,
        }
    }
}

impl Hash for ParagraphQuery<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for span in &self.paragraph.spans {
            span.text.hash(state);
            face_of(span).hash(state);
            span.family.hash(state);
            size_bits(span, self.font).hash(state);
        }
        self.paragraph.align.hash(state);
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
    let query = ParagraphQuery {
        font,
        paragraph,
        width: UNWRAPPED_WIDTH,
    };
    fonts
        .paragraph_shape(&query)
        .groups
        .iter()
        .map(|group| group.right)
        .fold(0.0, f32::max)
}

/// Where `paragraph` in `font` lands shaped to fill `rect`'s width from
/// its top left, without drawing it.
pub(crate) fn layout_of(
    fonts: &mut Fonts,
    font: ParagraphFont,
    paragraph: &Paragraph,
    rect: Rect,
) -> ParagraphLayout {
    let query = ParagraphQuery {
        font,
        paragraph,
        width: rect.width,
    };
    layout_from(fonts.paragraph_shape(&query), paragraph, rect)
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
    let query = ParagraphQuery {
        font,
        paragraph,
        width: rect.width,
    };
    let shape = fonts.paragraph_shape(&query);
    let runs = paragraph.runs(Some(theme), font);
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
        let mut lines = Vec::new();
        if paragraph.is_underlined(group.span) {
            lines.push(group.line_y + thickness);
        }
        if span.strikethrough {
            lines.push(group.line_y - group.size * STRIKE_RAISE - thickness / 2.0);
        }
        for top in lines {
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
    let layout = layout_from(shape, paragraph, rect);
    let segment = TextSegment {
        align: SegmentAlign::Start,
        clip: Some(rect),
        origin: Point::new(rect.x, rect.y),
        runs,
        wrap: Some(query.wrapping()),
    };
    ShapedParagraph {
        decorations,
        fills,
        layout,
        segment,
    }
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
    ParagraphLayout {
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
    buffer
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
                start: glyphs.iter().map(|glyph| glyph.start).min().unwrap_or(base),
                glyphs,
                height: run.line_height,
                top: run.line_top,
            }
        })
        .collect()
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
}
