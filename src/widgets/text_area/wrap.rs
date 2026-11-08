//! The rows a [`TextArea`](super::TextArea) shows its text in: the text
//! split at its line feeds and, when the area wraps, where a line grows
//! wider than the area, measured in cells. A line breaks after a space
//! when the word that follows fits on the next row, and inside a word only
//! when the word is wider than a row.

use std::ops::Range;

use super::super::super::base::text;

// ========================================================================
// Constants
// ========================================================================

/// What ends a line of the text.
const LINE_FEED: char = '\n';

/// Where a row may break: after a space.
const SPACE: char = ' ';

// ========================================================================
// Data Structures
// ========================================================================

/// One row an area shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Row {
    /// The bytes of the text it shows, without the line feed ending it.
    pub(super) range: Range<usize>,
    /// Whether the row was broken off to wrap, the next row going on with
    /// the same line, rather than ended by a line feed or the text's end.
    pub(super) wrapped: bool,
}

// ========================================================================
// Row Implementations
// ========================================================================

impl Row {
    /// The cells of the row before byte offset `offset` of `text`, an
    /// offset inside the row.
    pub(super) fn cells_to(&self, text: &str, offset: usize) -> usize {
        text::width(&text[self.range.start..offset])
    }

    /// The last offset of `text` the caret takes in this row: its end, or
    /// on a wrapped row the boundary before its last character, since the
    /// end itself is the start of the next row.
    pub(super) fn last_offset(&self, text: &str) -> usize {
        match self.wrapped {
            true => text[self.range.clone()]
                .char_indices()
                .next_back()
                .map_or(self.range.start, |(index, _)| self.range.start + index),
            false => self.range.end,
        }
    }

    /// The char boundary of `text` in this row nearest `cells` cells from
    /// the row's start: before a character when `cells` falls in its left
    /// half, after it otherwise, and no further than
    /// [`Row::last_offset`].
    pub(super) fn offset_at_cells(&self, text: &str, cells: f32) -> usize {
        let mut start = 0.0;
        let mut offset = self.range.end;
        for (index, c) in text[self.range.clone()].char_indices() {
            let width = text::char_width(c) as f32;
            if cells < start + width / 2.0 {
                offset = self.range.start + index;
                break;
            }
            start += width;
        }
        offset.min(self.last_offset(text))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The rows `text` shows in, wrapped at `cols` cells, or only at its line
/// feeds when `cols` is `None`. Never empty: an empty text shows one empty
/// row, and so does a line feed at the end.
pub(super) fn rows(text: &str, cols: Option<usize>) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut start = 0;
    for line in text.split(LINE_FEED) {
        wrap_line(line, start, cols, &mut rows);
        start += line.len() + LINE_FEED.len_utf8();
    }
    rows
}

/// The index of the row of `rows` that shows the caret at byte offset
/// `offset`: the row it falls in, the next one at a wrapped row's end.
pub(super) fn row_of(rows: &[Row], offset: usize) -> usize {
    rows.partition_point(|row| row.range.start <= offset)
        .saturating_sub(1)
}

/// Adds to `rows` the rows of `line`, which starts at byte offset `base` of
/// the text, wrapped at `cols` cells when there are any.
fn wrap_line(line: &str, base: usize, cols: Option<usize>, rows: &mut Vec<Row>) {
    let row = |range: Range<usize>, wrapped| Row {
        range: base + range.start..base + range.end,
        wrapped,
    };
    let Some(cols) = cols.filter(|&cols| cols > 0) else {
        rows.push(row(0..line.len(), false));
        return;
    };
    let mut start = 0;
    let mut width = 0;
    // Where the row may break: just after its last space.
    let mut after_space = None;
    for (index, c) in line.char_indices() {
        let char_width = text::char_width(c);
        if width + char_width > cols && index > start {
            if c == SPACE {
                // A space past the edge hangs there, unseen, rather than
                // starting the next row.
                let end = index + SPACE.len_utf8();
                rows.push(row(start..end, true));
                (start, width, after_space) = (end, 0, None);
                continue;
            }
            let end = after_space.filter(|&end| end > start).unwrap_or(index);
            rows.push(row(start..end, true));
            width = text::width(&line[end..index]);
            (start, after_space) = (end, None);
        }
        width += char_width;
        if c == SPACE {
            after_space = Some(index + SPACE.len_utf8());
        }
    }
    rows.push(row(start..line.len(), false));
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of each row of `text` wrapped at `cols`.
    fn shown(text: &str, cols: Option<usize>) -> Vec<&str> {
        rows(text, cols)
            .into_iter()
            .map(|row| &text[row.range])
            .collect()
    }

    #[test]
    fn test_a_line_breaks_after_a_space_when_the_next_word_fits() {
        assert_eq!(
            shown("the quick brown fox", Some(10)),
            ["the quick ", "brown fox"]
        );
        assert_eq!(
            shown("ab cdefghijkl", Some(5)),
            ["ab ", "cdefg", "hijkl"],
            "a word wider than a row breaks inside it"
        );
    }

    #[test]
    fn test_wide_characters_take_two_cells_and_never_split() {
        assert_eq!(shown("ab漢字", Some(3)), ["ab", "漢", "字"]);
        assert_eq!(
            shown("漢", Some(1)),
            ["漢"],
            "a character wider than a row still takes one"
        );
    }

    #[test]
    fn test_line_feeds_end_rows_and_unwrapped_lines_stay_whole() {
        assert_eq!(shown("one\n\ntwo\n", None), ["one", "", "two", ""]);
        assert_eq!(shown("", Some(4)), [""]);
        assert_eq!(shown("abcdefgh", None), ["abcdefgh"]);
    }

    #[test]
    fn test_the_caret_at_a_wrapped_row_end_shows_on_the_next_row() {
        let text = "abcd efgh\nij";
        let rows = rows(text, Some(5));
        assert_eq!(row_of(&rows, 4), 0);
        assert_eq!(row_of(&rows, 5), 1, "the wrap point starts the next row");
        assert_eq!(row_of(&rows, 9), 1, "a line's end stays on its row");
        assert_eq!(row_of(&rows, 10), 2);
        assert_eq!(rows[0].last_offset(text), 4, "before the hanging space");
        assert_eq!(rows[0].offset_at_cells(text, 99.0), 4);
        assert_eq!(rows[1].offset_at_cells(text, 99.0), 9);
    }
}
