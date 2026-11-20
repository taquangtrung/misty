//! Styled text: [`StyledText`], a line of [`Span`]s in theme or palette
//! colors that a renderer draws cell by cell, measured and cut to fit in
//! grid cells as plain [`text`](super::text) is.
//!
//! A [`Span`] in a proportional paragraph takes more styles than the cell
//! grid draws: a [`TextSize`], a [`FontFamily`], a background color, and a
//! strikethrough. The grid draws only its color, weight, slant, and
//! underline.

use std::hash::{Hash, Hasher};

use super::cell::CellColor;
use super::color::{Rgb, Rgba};
use super::text::{ELLIPSIS, truncate, width};
use super::theme::{Emphasis, Theme, ThemeToken};

// ========================================================================
// Constants
// ========================================================================

/// The largest size a span is drawn at, in logical pixels: larger sizes,
/// asked for in pixels or as a scale, are drawn at it.
const MAX_SPAN_SIZE: f32 = 288.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A line of text in runs of one style each, drawn left to right. Too long
/// for the room it gets, it is cut at a whole character, ending in an
/// [`ELLIPSIS`] when made [`StyledText::with_ellipsis`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StyledText {
    /// Whether a line cut short ends in an ellipsis.
    ellipsis: bool,
    /// The runs, left to right.
    spans: Vec<Span>,
}

/// A run of text in one color and face. Make one with [`Span::new`] and
/// style it with the builder methods ([`Span::bold`], [`Span::size`],
/// [`Span::background`], ...); the size, family, background, and
/// strikethrough show only in a [`Paragraph`](crate::Paragraph).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span {
    /// The color filled behind the glyphs, such as a search match's, or
    /// `None` for none.
    pub background: Option<TextColor>,
    /// Whether the text is bold.
    pub bold: bool,
    /// The glyphs' color.
    pub color: TextColor,
    /// The font family the text is set in.
    pub family: FontFamily,
    /// Whether the text is italic.
    pub italic: bool,
    /// How large the text is.
    pub size: TextSize,
    /// Whether a line is struck through the text.
    pub strikethrough: bool,
    /// The text itself, one line.
    pub text: String,
    /// Whether the text is underlined.
    pub underline: bool,
    /// The underline's color when it is underlined (or a link), or `None`
    /// for the glyphs' color: a spelling mark, a highlight in the margin's
    /// color.
    pub underline_color: Option<TextColor>,
}

/// How large a [`Span`] is drawn in a paragraph. Each line is as tall as
/// its largest text, so a larger span makes its line taller.
#[derive(Clone, Copy, Debug)]
pub enum TextSize {
    /// This many logical pixels, whatever the window's font size.
    Pixels(f32),
    /// This multiple of the window's font size: `1.0` is the size of the
    /// rest of the paragraph, `2.0` twice it.
    Scale(f32),
}

/// The font family a [`Span`] is set in, in a paragraph.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub enum FontFamily {
    /// The system's monospace font, for code.
    Monospace,
    /// The family of this name: an installed font, or one the app loaded
    /// with [`load_font`](crate::load_font). When no font has the name,
    /// the text is set in the proportional font.
    Named(String),
    /// The system's proportional (sans-serif) font, as paragraphs are by
    /// default.
    #[default]
    Proportional,
    /// The system's serif font, for long reading.
    Serif,
}

/// The color of a [`Span`]: a named theme color, or a cell color resolved
/// through the theme's palette, so a theme change recolors either; or an
/// exact color the theme leaves alone, such as a document's own.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TextColor {
    /// A cell color: [`CellColor::Default`] is the theme's foreground, an
    /// indexed color the theme's palette entry.
    Palette(CellColor),
    /// This exact color, translucent below an alpha of 255 in a paragraph
    /// (the cell grid draws it opaque).
    Rgba(Rgba),
    /// One of the theme's named colors.
    Token(ThemeToken),
}

// ========================================================================
// StyledText Implementations
// ========================================================================

impl StyledText {
    /// An empty line.
    pub fn new() -> Self {
        Self::default()
    }

    /// A line of one span of `text` in `color`.
    pub fn plain(text: impl Into<String>, color: TextColor) -> Self {
        Self::new().with(Span::new(text, color))
    }

    /// This line with `span` added at its end.
    pub fn with(mut self, span: Span) -> Self {
        self.push(span);
        self
    }

    /// This line ending in an ellipsis when it is cut short.
    pub fn with_ellipsis(self) -> Self {
        Self {
            ellipsis: true,
            ..self
        }
    }

    /// This line upright and at the regular weight throughout: every run
    /// neither bold nor italic, as the status bar sets all it shows.
    pub(crate) fn upright(mut self) -> Self {
        for span in &mut self.spans {
            span.bold = false;
            span.italic = false;
        }
        self
    }

    /// Adds `span` at the end of the line.
    pub fn push(&mut self, span: Span) {
        self.spans.push(span);
    }

    /// The runs, left to right.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// How many cells the whole line takes.
    pub fn width(&self) -> usize {
        self.spans.iter().map(|span| width(&span.text)).sum()
    }

    /// The line cut to at most `cells` cells at a whole character, styles
    /// kept, ending in an ellipsis in the style of the last character kept
    /// when the line asks for one. A line that fits is returned whole.
    pub fn fit(&self, cells: usize) -> Self {
        if self.width() <= cells {
            return self.clone();
        }
        let mut budget = match self.ellipsis {
            true => cells.saturating_sub(1),
            false => cells,
        };
        let mut spans = Vec::new();
        for span in &self.spans {
            let kept = truncate(&span.text, budget);
            budget -= width(kept);
            let whole = kept.len() == span.text.len();
            if !kept.is_empty() {
                spans.push(Span {
                    text: kept.to_owned(),
                    ..span.clone()
                });
            }
            if !whole {
                break;
            }
        }
        if self.ellipsis && cells > 0 {
            match spans.last_mut() {
                Some(last) => last.text.push(ELLIPSIS),
                None => spans.extend(self.spans.first().map(|first| Span {
                    text: ELLIPSIS.to_string(),
                    ..first.clone()
                })),
            }
        }
        Self {
            ellipsis: self.ellipsis,
            spans,
        }
    }
}

impl From<Span> for StyledText {
    fn from(span: Span) -> Self {
        Self::new().with(span)
    }
}

// ========================================================================
// Span Implementations
// ========================================================================

impl Span {
    /// A span of `text` in `color`, in the paragraph's font and size, with
    /// no other style.
    pub fn new(text: impl Into<String>, color: TextColor) -> Self {
        Self {
            background: None,
            bold: false,
            color,
            family: FontFamily::Proportional,
            italic: false,
            size: TextSize::default(),
            strikethrough: false,
            text: text.into(),
            underline: false,
            underline_color: None,
        }
    }

    /// This span on a background of `color`, filled behind its glyphs
    /// line by line (a search match, a `<mark>`).
    pub fn background(self, color: impl Into<TextColor>) -> Self {
        Self {
            background: Some(color.into()),
            ..self
        }
    }

    /// This span set in `family`.
    pub fn family(self, family: FontFamily) -> Self {
        Self { family, ..self }
    }

    /// This span drawn at `size`.
    pub fn size(self, size: TextSize) -> Self {
        Self { size, ..self }
    }

    /// This span with a line struck through it.
    pub fn strikethrough(self) -> Self {
        Self {
            strikethrough: true,
            ..self
        }
    }

    /// This span in bold.
    pub fn bold(self) -> Self {
        Self { bold: true, ..self }
    }

    /// This span in bold when `emphasis` is [`Emphasis::Bold`], as misty's
    /// chrome stresses words the theme's way.
    pub(crate) fn emphasized(self, emphasis: Emphasis) -> Self {
        Self {
            bold: emphasis == Emphasis::Bold,
            ..self
        }
    }

    /// This span in italics.
    pub fn italic(self) -> Self {
        Self {
            italic: true,
            ..self
        }
    }

    /// This span underlined.
    pub fn underlined(self) -> Self {
        Self {
            underline: true,
            ..self
        }
    }

    /// This span underlined in `color` rather than its glyphs' color, in a
    /// paragraph and on the cell grid alike; a link's underline takes it
    /// too.
    pub fn underline_color(self, color: impl Into<TextColor>) -> Self {
        Self {
            underline: true,
            underline_color: Some(color.into()),
            ..self
        }
    }
}

// ========================================================================
// TextSize Implementations
// ========================================================================

impl TextSize {
    /// The size in physical pixels, for text whose own size is `base`
    /// physical pixels on a display scaled by `scale_factor`; `None` when
    /// it is `base` itself. A size that is not a positive number is
    /// `base`; one past the largest drawn is held to it.
    pub(crate) fn resolve(self, base: f32, scale_factor: f32) -> Option<f32> {
        let size = match self {
            Self::Pixels(pixels) => pixels * scale_factor,
            Self::Scale(scale) => scale * base,
        };
        let valid = size.is_finite() && size > 0.0;
        (valid && size != base).then(|| size.min(MAX_SPAN_SIZE * scale_factor))
    }

    /// Its kind and the bits of its number, to compare and hash it.
    fn bits(self) -> (bool, u32) {
        match self {
            Self::Pixels(pixels) => (true, pixels.to_bits()),
            Self::Scale(scale) => (false, scale.to_bits()),
        }
    }
}

impl Default for TextSize {
    fn default() -> Self {
        Self::Scale(1.0)
    }
}

impl PartialEq for TextSize {
    fn eq(&self, other: &Self) -> bool {
        self.bits() == other.bits()
    }
}

// Compared by bits, so every size, NaN too, equals itself.
impl Eq for TextSize {}

impl Hash for TextSize {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bits().hash(state);
    }
}

// ========================================================================
// TextColor Implementations
// ========================================================================

impl TextColor {
    /// The exact color this is in `theme`, its alpha dropped.
    pub fn resolve(self, theme: &Theme) -> Rgb {
        match self {
            Self::Palette(color) => theme.foreground_of(color),
            Self::Rgba(color) => Rgb::new(color.r, color.g, color.b),
            Self::Token(token) => theme.color(token),
        }
    }

    /// The exact color this is in `theme`, with its alpha: opaque but for
    /// a translucent [`TextColor::Rgba`].
    pub(crate) fn resolve_rgba(self, theme: &Theme) -> Rgba {
        match self {
            Self::Rgba(color) => color,
            Self::Palette(_) | Self::Token(_) => Rgba::from(self.resolve(theme)),
        }
    }
}

impl From<ThemeToken> for TextColor {
    fn from(token: ThemeToken) -> Self {
        Self::Token(token)
    }
}

impl From<Rgba> for TextColor {
    fn from(color: Rgba) -> Self {
        Self::Rgba(color)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const MUTED: TextColor = TextColor::Token(ThemeToken::Muted);

    #[test]
    fn test_fitting_styled_text_cuts_across_spans_and_keeps_their_styles() {
        let text = StyledText::new()
            .with(Span::new("ab", MUTED))
            .with(Span::new("漢字", MUTED).bold())
            .with_ellipsis();
        let fitted = text.fit(5);
        assert_eq!(fitted.width(), 5);
        let spans = fitted.spans();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "ab");
        assert_eq!(
            spans[1].text, "漢\u{2026}",
            "the ellipsis takes the cut span's style"
        );
        assert!(spans[1].bold);
        let tight = text.fit(4);
        assert_eq!(tight.spans().len(), 1, "no room for half a wide char");
        assert_eq!(tight.spans()[0].text, "ab\u{2026}");
        assert_eq!(text.fit(6), text, "it fits whole");
    }

    #[test]
    fn test_styled_text_without_an_ellipsis_is_clipped_at_a_whole_character() {
        let text = StyledText::plain("a漢字", MUTED);
        assert_eq!(text.fit(4).spans()[0].text, "a漢");
        assert!(text.fit(0).spans().is_empty());
    }

    #[test]
    fn test_an_ellipsis_with_nothing_kept_takes_the_first_spans_style() {
        let text = StyledText::from(Span::new("漢字", MUTED).italic()).with_ellipsis();
        let fitted = text.fit(1);
        assert_eq!(fitted.spans()[0].text, "\u{2026}");
        assert!(fitted.spans()[0].italic);
    }
}
