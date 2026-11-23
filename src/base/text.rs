//! Text measured in grid cells: how many columns a string takes
//! ([`width`], with wide CJK characters and emoji taking two and combining
//! marks none) and cutting it to fit ([`truncate`], [`fit`]). Styled text
//! is measured the same way (see [`StyledText`](crate::StyledText)).
//!
//! Widths follow `unicode-width`, the same table terminals use, so text
//! drawn here lines up with a cell grid drawn from the same string.

use std::borrow::Cow;

use unicode_width::UnicodeWidthChar;

// ========================================================================
// Constants
// ========================================================================

/// What ends text cut short: one cell wide.
pub const ELLIPSIS: char = '\u{2026}';

// ========================================================================
// Functions
// ========================================================================

/// How many cells `c` takes: two for a wide character (most CJK and emoji),
/// none for a combining mark or a control character, one otherwise.
pub fn char_width(c: char) -> usize {
    match c.is_control() {
        true => 0,
        false => c.width().unwrap_or(0),
    }
}

/// How many cells `text` takes, the sum of its characters' widths.
pub fn width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// The longest start of `text` that fits in `cells` cells, never splitting
/// a wide character: one that would stick out is left off whole, with
/// everything after it. Combining marks stay with the character they
/// follow.
pub fn truncate(text: &str, cells: usize) -> &str {
    let mut used = 0;
    for (index, c) in text.char_indices() {
        used += char_width(c);
        if used > cells {
            return &text[..index];
        }
    }
    text
}

/// `text` cut to fit `cells` cells, ending in an [`ELLIPSIS`] when it was
/// longer; returned as is when it fits.
pub fn fit(text: &str, cells: usize) -> Cow<'_, str> {
    if width(text) <= cells {
        return Cow::Borrowed(text);
    }
    if cells == 0 {
        return Cow::Borrowed("");
    }
    let mut fitted = truncate(text, cells - 1).to_owned();
    fitted.push(ELLIPSIS);
    Cow::Owned(fitted)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_width_counts_wide_characters_twice_and_marks_and_controls_never() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("漢字"), 4);
        assert_eq!(width("e\u{301}"), 1, "a combining acute takes no cell");
        assert_eq!(width("a\tb\u{7}"), 2, "controls take no cell");
    }

    #[test]
    fn test_truncate_never_splits_a_wide_character() {
        assert_eq!(truncate("a漢b", 2), "a");
        assert_eq!(truncate("a漢b", 3), "a漢");
        assert_eq!(truncate("e\u{301}x", 1), "e\u{301}", "the mark stays");
        assert_eq!(truncate("abc", 9), "abc");
    }

    #[test]
    fn test_fit_ends_in_an_ellipsis_inside_the_cells_with_wide_characters() {
        assert_eq!(fit("short", 5), "short");
        assert_eq!(fit("too long", 5), "too \u{2026}");
        assert_eq!(fit("漢字漢字", 5), "漢字\u{2026}");
        let narrower = fit("漢字漢字", 4);
        assert_eq!(
            narrower, "漢\u{2026}",
            "the second wide char would stick out"
        );
        assert!(width(&narrower) <= 4);
        assert_eq!(fit("abc", 0), "");
        assert_eq!(fit("漢字", 1), "\u{2026}");
    }
}
