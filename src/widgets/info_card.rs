//! [`InfoCard`]: a card of text an editor hangs over a word, telling about
//! it: what a language server says of a name, the problems found there, the
//! signature being typed. It opens above the word, or under it where there
//! is no room above, in the cell grid's font, each line in the styles the
//! app gives it (code in its syntax colors, docs with their emphasis).
//!
//! A long line is folded at a space before [`MAX_LINE_CELLS`] cells, or at
//! the limit where it has none, and a card longer than [`MAX_LINES`] lines
//! is cut there, ending in an ellipsis. What kind of thing the card is about
//! (a problem's kind and what found it) can sit apart at its foot, dimmed,
//! under a hairline: [`InfoCard::with_footer`].
//!
//! The card takes no input; the app shows it while the pointer rests on the
//! word, or while the cursor is in it, and drops it after.

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::Widget;
use super::super::base::geometry::{Point, Rect};
use super::super::base::styled::{Span, StyledText, TextColor};
use super::super::base::text::{self, ELLIPSIS};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::Input;
use super::super::render::icon::logical;
use super::popup::{Popup, PopupSide};

// ========================================================================
// Constants
// ========================================================================

/// The most lines a card shows; past them it ends in an ellipsis.
const MAX_LINES: usize = 18;

/// The widest a line is drawn, in cells, before it is folded.
const MAX_LINE_CELLS: usize = 90;

/// The space above and below the body and the foot, in logical pixels.
const PADDING_Y: f32 = 5.0;

/// The thickness of the hairline over the foot, in logical pixels.
const DIVIDER: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A card of text over the word it tells about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InfoCard {
    /// The word's first cell: the card opens over it.
    anchor: Rect,
    /// What kind of thing the card is about, said at its foot.
    footer: Option<String>,
    /// What it says, one entry per line.
    lines: Vec<StyledText>,
}

// ========================================================================
// InfoCard Implementations
// ========================================================================

impl InfoCard {
    /// A card saying `lines`, each in its own styles, over `anchor`, the
    /// first cell of the word it tells about.
    pub fn new(lines: Vec<StyledText>, anchor: Rect) -> Self {
        Self {
            anchor,
            footer: None,
            lines,
        }
    }

    /// A card saying `text`, one line per line, in the foreground color,
    /// over `anchor`.
    pub fn plain(text: &str, anchor: Rect) -> Self {
        let color = TextColor::Token(ThemeToken::Foreground);
        let lines = text
            .lines()
            .map(|line| StyledText::plain(line, color))
            .collect();
        Self::new(lines, anchor)
    }

    /// This card with `footer` at its foot, dimmed under a hairline: what
    /// the card is about, as `rustc(E0308)` under a problem.
    pub fn with_footer(self, footer: impl Into<String>) -> Self {
        Self {
            footer: Some(footer.into()),
            ..self
        }
    }

    /// Moves the card over `anchor`.
    pub fn set_anchor(&mut self, anchor: Rect) {
        self.anchor = anchor;
    }
}

impl Widget for InfoCard {
    type Output = ();
    type Response = ();

    /// Takes no input: a card shows only what the app tells it.
    fn handle(&mut self, _input: &Input, _cx: &mut EventCx<'_>) {}

    /// Draws the card in the overlay over its anchor, inside `area`.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let padding = metrics.width.round();
        let padding_y = logical(PADDING_Y, scale_factor);
        let divider = logical(DIVIDER, scale_factor);
        let inset = Popup::inset(&theme.shape, scale_factor);
        let room = metrics.cols_in(area.width - 2.0 * (padding + inset));
        let cells = MAX_LINE_CELLS.min(room).max(1);
        let body = card_lines(&self.lines, cells);
        if body.is_empty() {
            return;
        }
        let muted = TextColor::Token(ThemeToken::Muted);
        let footer: Vec<StyledText> = self
            .footer
            .iter()
            .flat_map(|footer| footer.lines())
            .flat_map(|line| fold(&StyledText::plain(line, muted), cells))
            .collect();
        let widest = body.iter().chain(&footer).map(StyledText::width).max();
        let width = (widest.unwrap_or(0) as f32 * metrics.width + 2.0 * (padding + inset)).ceil();
        let body_height = body.len() as f32 * metrics.height + 2.0 * padding_y;
        let footer_height = match footer.len() {
            0 => 0.0,
            lines => divider + lines as f32 * metrics.height + 2.0 * padding_y,
        };
        let height = body_height + footer_height + 2.0 * inset;
        let popup = Popup::new(self.anchor, width, height, PopupSide::Above, area);
        let style = theme
            .shape
            .floating_box(theme.raised, theme.border, scale_factor);
        let content = popup.content(&theme.shape, scale_factor);
        let left = content.x + padding;
        frame.overlay(|frame| {
            frame.draw_box(popup.rect(), &style);
            for (row, line) in body.iter().enumerate() {
                let top = content.y + padding_y + row as f32 * metrics.height;
                frame.draw_text(line, Point::new(left, top), content, theme);
            }
            if footer.is_empty() {
                return;
            }
            let rule = content.y + body_height;
            frame.fill_rect(
                Rect::new(content.x, rule, content.width, divider),
                theme.border,
            );
            for (row, line) in footer.iter().enumerate() {
                let top = rule + divider + padding_y + row as f32 * metrics.height;
                frame.draw_text(line, Point::new(left, top), content, theme);
            }
        });
    }
}

// ========================================================================
// Free Functions
// ========================================================================

/// `lines` as a card draws them: each folded to `cells`, blank lines at the
/// end dropped, and the whole cut to [`MAX_LINES`], ending in an ellipsis
/// line in the style of the last line kept.
fn card_lines(lines: &[StyledText], cells: usize) -> Vec<StyledText> {
    let mut folded: Vec<StyledText> = lines.iter().flat_map(|line| fold(line, cells)).collect();
    while folded.last().is_some_and(|line| line.width() == 0) {
        folded.pop();
    }
    if folded.len() > MAX_LINES {
        folded.truncate(MAX_LINES);
        let more = folded
            .last()
            .and_then(|line| line.spans().first())
            .map(|span| Span {
                text: ELLIPSIS.to_string(),
                ..span.clone()
            });
        folded.extend(more.map(StyledText::from));
    }
    folded
}

/// `line` folded into lines of at most `cells` cells, its trailing blanks
/// dropped: each cut after the last space that fits, or at the limit where
/// none does, the spaces at the fold dropped and every char keeping its
/// span's style.
fn fold(line: &StyledText, cells: usize) -> Vec<StyledText> {
    let mut rest: Vec<(char, usize)> = line
        .spans()
        .iter()
        .enumerate()
        .flat_map(|(index, span)| span.text.chars().map(move |c| (c, index)))
        .collect();
    while rest.last().is_some_and(|(c, _)| *c == ' ') {
        rest.pop();
    }
    let mut lines = Vec::new();
    loop {
        let mut used = 0;
        let fits = rest
            .iter()
            .take_while(|(c, _)| {
                used += text::char_width(*c);
                used <= cells
            })
            .count();
        if fits == rest.len() {
            lines.push(styled(line, &rest));
            return lines;
        }
        let cut = rest[..=fits]
            .iter()
            .rposition(|(c, _)| *c == ' ')
            .filter(|at| *at > 0)
            .unwrap_or(fits.max(1));
        lines.push(styled(line, &rest[..cut]));
        let spaces = rest[cut..].iter().take_while(|(c, _)| *c == ' ').count();
        rest.drain(..cut + spaces);
    }
}

/// `chars`, each with the index of the span of `line` it came from, as a
/// line of runs in those spans' styles.
fn styled(line: &StyledText, chars: &[(char, usize)]) -> StyledText {
    let mut styled = StyledText::new();
    for run in chars.chunk_by(|a, b| a.1 == b.1) {
        styled.push(Span {
            text: run.iter().map(|(c, _)| c).collect(),
            ..line.spans()[run[0].1].clone()
        });
    }
    styled
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::Rgba;

    fn texts(lines: &[StyledText]) -> Vec<String> {
        let text = |line: &StyledText| line.spans().iter().map(|span| span.text.clone()).collect();
        lines.iter().map(text).collect()
    }

    fn plain(text: &str) -> StyledText {
        StyledText::plain(text, TextColor::Token(ThemeToken::Foreground))
    }

    #[test]
    fn test_a_long_line_folds_at_its_last_space_that_fits() {
        let lines = fold(&plain("fn parse(input: &str)   "), 12);
        assert_eq!(texts(&lines), ["fn", "parse(input:", "&str)"]);
    }

    #[test]
    fn test_a_line_with_no_space_folds_at_the_limit() {
        let lines = fold(&plain("abcdefghij"), 4);
        assert_eq!(texts(&lines), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn test_a_folded_line_keeps_each_chars_style() {
        let line = StyledText::new()
            .with(Span::new("let ", TextColor::Token(ThemeToken::Accent)).bold())
            .with(Span::new("value", TextColor::Token(ThemeToken::Foreground)));
        let lines = fold(&line, 6);
        assert_eq!(texts(&lines), ["let", "value"]);
        assert!(lines[0].spans()[0].bold, "the keyword stays bold");
        assert!(!lines[1].spans()[0].bold, "the name stays plain");
    }

    #[test]
    fn test_a_wide_char_counts_two_cells() {
        let lines = fold(&plain("漢字漢字"), 5);
        assert_eq!(texts(&lines), ["漢字", "漢字"]);
    }

    #[test]
    fn test_a_long_card_is_cut_to_the_most_lines_and_ends_in_an_ellipsis() {
        let lines: Vec<StyledText> = (0..30).map(|n| plain(&n.to_string())).collect();
        let card = card_lines(&lines, MAX_LINE_CELLS);
        assert_eq!(card.len(), MAX_LINES + 1);
        assert_eq!(texts(&card).last().map(String::as_str), Some("\u{2026}"));
    }

    #[test]
    fn test_blank_lines_at_the_end_are_dropped() {
        let card = card_lines(&[plain("docs"), plain(""), plain("  ")], MAX_LINE_CELLS);
        assert_eq!(texts(&card), ["docs"]);
    }

    #[test]
    fn test_the_card_opens_over_its_word_with_a_hairline_over_its_foot() {
        let theme = Theme::dark();
        let word = Rect::new(20.0, 150.0, 8.0, 16.0);
        let mut card = InfoCard::plain("a value", word).with_footer("rustc(E0308)");
        let Some(snapshot) = offscreen::render(300, 200, |frame| {
            frame.clear(Rgba::from(theme.background));
            card.draw(frame, frame.bounds(), &theme);
        }) else {
            return;
        };
        snapshot.assert_pixel(22.0, 149.0, theme.border, "the card's bottom edge");
        snapshot.assert_pixel(22.0, 152.0, theme.background, "the word is uncovered");
        let metrics = snapshot.metrics();
        let inset = Popup::inset(&theme.shape, 1.0);
        let top = 150.0 - (2.0 * metrics.height + 4.0 * PADDING_Y + DIVIDER + 2.0 * inset);
        let rule = top + inset + metrics.height + 2.0 * PADDING_Y;
        snapshot.assert_pixel(22.0, rule, theme.border, "the hairline over the foot");
        snapshot.assert_pixel(22.0, rule - 2.0, theme.raised, "the card's body");
    }
}
