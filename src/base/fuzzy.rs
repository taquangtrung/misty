//! Fuzzy matching for pickers: [`score`] says whether a query matches a
//! text as a subsequence and how well, with the matched characters for
//! highlighting, [`ranges`] groups those characters into runs, and
//! [`highlight`] styles a text with them for a list row.
//!
//! A query is split on whitespace into terms, each of which must match on
//! its own, in any order relative to the others. A term matches when its
//! characters appear in the text in order, not necessarily side by side.
//! Each term is scored over every alignment (by dynamic programming), not
//! only the leftmost, so a tight run later in the text beats a scattered
//! match at its start:
//!
//! - every matched character scores [`MATCH`] points;
//! - one right after the previous match scores [`CONSECUTIVE`] more;
//! - one at a word start (the text's start, after a separator such as
//!   space, `/`, `.`, `_`, `-`, or an uppercase letter after a lowercase
//!   one) scores [`BOUNDARY`] more;
//! - each character skipped between two matches costs [`GAP`];
//! - the text's length costs a point per [`LENGTH_UNIT`] characters, so the
//!   shorter of two otherwise equal texts ranks first.
//!
//! Case follows the smart-case rule: a term in lowercase matches either
//! case, a term with an uppercase letter matches case exactly.

use std::cmp::Reverse;
use std::ops::Range;

use super::styled::{Span, StyledText, TextColor};

// ========================================================================
// Constants
// ========================================================================

/// What each matched character scores.
const MATCH: i32 = 16;

/// What a matched character right after the previous one scores on top.
const CONSECUTIVE: i32 = 12;

/// What a matched character at a word start scores on top.
const BOUNDARY: i32 = 10;

/// What each character skipped between two matched characters costs.
const GAP: i32 = 1;

/// How many characters of the text's length cost one point.
const LENGTH_UNIT: i32 = 8;

/// The characters after which a word starts.
const SEPARATORS: &[char] = &[' ', '/', '\\', '.', '_', '-', ':', ',', '(', '['];

/// A score no alignment reaches, for cells no alignment fills.
const UNREACHED: i32 = i32::MIN / 2;

// ========================================================================
// Data Structures
// ========================================================================

/// How well a query matched a text.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FuzzyMatch {
    /// The matched characters, as char indices into the text, ascending
    /// and without repeats.
    pub positions: Vec<usize>,
    /// The score: higher is better.
    pub score: i32,
}

/// The best alignment of one term found so far, by its last character.
#[derive(Clone, Copy, Debug)]
struct Cell {
    /// Where the term's previous character matched, or `None` for its
    /// first.
    from: Option<usize>,
    /// The best score of the term so far ending here.
    score: i32,
}

// ========================================================================
// Functions
// ========================================================================

/// How well `query` matches `text`, or `None` when some term of it does
/// not. An empty query (or only whitespace) matches everything with score
/// 0 and nothing highlighted.
pub fn score(query: &str, text: &str) -> Option<FuzzyMatch> {
    let chars: Vec<char> = text.chars().collect();
    let mut found = FuzzyMatch::default();
    for term in query.split_whitespace() {
        let (score, positions) = score_term(term, &chars)?;
        found.score += score;
        found.positions.extend(positions);
    }
    if found.positions.is_empty() {
        return Some(found);
    }
    found.positions.sort_unstable();
    found.positions.dedup();
    let length = i32::try_from(chars.len()).unwrap_or(i32::MAX);
    found.score -= length / LENGTH_UNIT;
    Some(found)
}

/// `positions` (ascending char indices) grouped into runs of consecutive
/// indices: `[0, 1, 2, 5]` is `[0..3, 5..6]`.
pub fn ranges(positions: &[usize]) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for &position in positions {
        match runs.last_mut() {
            Some(run) if run.end == position => run.end += 1,
            Some(_) | None => runs.push(position..position + 1),
        }
    }
    runs
}

/// `text` styled to show a match: the characters at `positions` (char
/// indices, as [`FuzzyMatch::positions`] holds them) bold in `matched`,
/// the rest in `color`. Positions past the text are ignored.
pub fn highlight(
    text: &str,
    positions: &[usize],
    color: TextColor,
    matched: TextColor,
) -> StyledText {
    let offsets: Vec<usize> = text
        .char_indices()
        .map(|(offset, _)| offset)
        .chain([text.len()])
        .collect();
    let byte = |index: usize| offsets[index.min(offsets.len() - 1)];
    let mut styled = StyledText::new();
    let mut done = 0;
    for run in ranges(positions) {
        let (start, end) = (byte(run.start), byte(run.end));
        if start > byte(done) {
            styled.push(Span::new(&text[byte(done)..start], color));
        }
        if end > start {
            styled.push(Span::new(&text[start..end], matched).bold());
        }
        done = run.end;
    }
    if byte(done) < text.len() {
        styled.push(Span::new(&text[byte(done)..], color));
    }
    styled
}

/// The best score of `term` against `text` and its matched positions.
fn score_term(term: &str, text: &[char]) -> Option<(i32, Vec<usize>)> {
    let exact = term.chars().any(char::is_uppercase);
    let term: Vec<char> = term.chars().collect();
    let same = |a: char, b: char| match exact {
        true => a == b,
        false => a == b || a.to_lowercase().eq(b.to_lowercase()),
    };
    let unreached = Cell {
        from: None,
        score: UNREACHED,
    };
    let mut rows: Vec<Vec<Cell>> = Vec::with_capacity(term.len());
    for (index, &wanted) in term.iter().enumerate() {
        let previous = index.checked_sub(1).map(|row| &rows[row]);
        let mut row = vec![unreached; text.len()];
        // The best previous cell at least one character back, less the gap
        // to the current column: (score, column).
        let mut carried: Option<(i32, usize)> = None;
        for (col, &c) in text.iter().enumerate() {
            if let (Some(previous), Some(back)) = (previous, col.checked_sub(2)) {
                let candidate = (previous[back].score, back);
                carried = match carried {
                    Some((score, at)) if score - GAP >= candidate.0 - GAP => {
                        Some((score - GAP, at))
                    }
                    Some(_) | None => Some((candidate.0 - GAP, back)),
                };
            }
            if !same(wanted, c) {
                continue;
            }
            let gain = MATCH + bonus(text, col);
            row[col] = match previous {
                None => Cell {
                    from: None,
                    score: gain,
                },
                Some(previous) => {
                    let diagonal = col
                        .checked_sub(1)
                        .map(|back| (previous[back].score + CONSECUTIVE, back));
                    let best = [diagonal, carried]
                        .into_iter()
                        .flatten()
                        .max_by_key(|(score, _)| *score)
                        .filter(|(score, _)| *score > UNREACHED / 2);
                    match best {
                        Some((score, from)) => Cell {
                            from: Some(from),
                            score: score + gain,
                        },
                        None => unreached,
                    }
                }
            };
        }
        rows.push(row);
    }
    let last = rows.last()?;
    let (mut col, best) = last
        .iter()
        .enumerate()
        .filter(|(_, cell)| cell.score > UNREACHED / 2)
        .max_by_key(|(col, cell)| (cell.score, Reverse(*col)))?;
    let score = best.score;
    let mut positions = vec![0; term.len()];
    for (row, slot) in rows.iter().zip(positions.iter_mut()).rev() {
        *slot = col;
        col = row[col].from.unwrap_or(col);
    }
    Some((score, positions))
}

/// The word-start bonus of the character at `col` of `text`.
fn bonus(text: &[char], col: usize) -> i32 {
    let Some(before) = col.checked_sub(1).map(|back| text[back]) else {
        return BOUNDARY;
    };
    let camel = before.is_lowercase() && text[col].is_uppercase();
    match SEPARATORS.contains(&before) || camel {
        true => BOUNDARY,
        false => 0,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(query: &str, text: &str) -> Vec<usize> {
        score(query, text).expect("a match").positions
    }

    /// `texts` ordered best first for `query`, non-matching ones left out.
    fn ranked<'a>(query: &str, texts: &[&'a str]) -> Vec<&'a str> {
        let mut scored: Vec<(i32, &str)> = texts
            .iter()
            .filter_map(|text| score(query, text).map(|found| (found.score, *text)))
            .collect();
        scored.sort_by_key(|(score, _)| Reverse(*score));
        scored.into_iter().map(|(_, text)| text).collect()
    }

    #[test]
    fn test_a_query_matches_as_a_subsequence_not_a_substring() {
        assert_eq!(positions("auto64", "autoruns64.exe"), [0, 1, 2, 3, 8, 9]);
        assert_eq!(score("ba", "abc"), None, "order matters");
        assert_eq!(score("abcd", "abc"), None);
    }

    #[test]
    fn test_a_contiguous_run_beats_scattered_characters() {
        assert_eq!(ranked("abc", &["axbxcx", "xabcxx"]), ["xabcxx", "axbxcx"]);
    }

    #[test]
    fn test_word_starts_beat_the_middle_of_words() {
        assert_eq!(
            ranked("fb", &["xfoobar", "foobar", "foo_bar"]),
            ["foo_bar", "foobar", "xfoobar"]
        );
        let camel = score("fb", "fooBar").expect("a match").score;
        assert!(camel > score("fb", "foobar").expect("a match").score);
    }

    #[test]
    fn test_the_best_alignment_wins_over_the_leftmost() {
        // Greedy would take a_b_c at 0, 2, 4; the run at 6 scores more.
        assert_eq!(positions("abc", "a_b_c_abc"), [6, 7, 8]);
    }

    #[test]
    fn test_a_lowercase_term_ignores_case_and_an_uppercase_term_does_not() {
        assert_eq!(positions("fb", "FooBar"), [0, 3], "positions in the text");
        assert_eq!(score("FB", "foobar"), None);
        assert_eq!(positions("FB", "FooBar"), [0, 3]);
    }

    #[test]
    fn test_terms_match_independently_in_any_order() {
        assert_eq!(positions("tab new", "New Tab"), [0, 1, 2, 4, 5, 6]);
        assert_eq!(score("tab old", "New Tab"), None);
    }

    #[test]
    fn test_an_empty_query_matches_everything_unhighlighted() {
        assert_eq!(score("  ", "anything"), Some(FuzzyMatch::default()));
    }

    #[test]
    fn test_a_shorter_text_wins_a_tie() {
        let long = format!("tab{}", " ".repeat(40));
        assert_eq!(
            ranked("tab", &[long.as_str(), "tab"]),
            ["tab", long.as_str()]
        );
    }

    #[test]
    fn test_ranges_group_consecutive_positions() {
        assert_eq!(ranges(&[0, 1, 2, 5, 7, 8]), [0..3, 5..6, 7..9]);
        assert!(ranges(&[]).is_empty());
    }

    #[test]
    fn test_highlight_bolds_the_matched_runs_by_char_not_byte() {
        let plain = TextColor::Token(crate::base::theme::ThemeToken::Foreground);
        let hit = TextColor::Token(crate::base::theme::ThemeToken::Accent);
        let styled = highlight("äb_cd", &[1, 3, 4, 9], plain, hit);
        let runs: Vec<(&str, bool)> = styled
            .spans()
            .iter()
            .map(|span| (span.text.as_str(), span.bold))
            .collect();
        assert_eq!(
            runs,
            [("ä", false), ("b", true), ("_", false), ("cd", true)]
        );
    }
}
