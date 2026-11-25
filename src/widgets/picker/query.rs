//! The query a picker matches its rows by: words separated by spaces, matched
//! in any order, each found anywhere in any case. A word written `/like
//! this/` is a regular expression, one starting with `!` leaves out the rows
//! it matches, and text in double quotes is matched as written, spaces
//! included.
//!
//! A row scores by how its matches sit: the words in the order typed, from
//! the row's start to its end, score highest; in order from the start next;
//! in order anywhere next; out of order last. A query that is the whole label,
//! in any case, scores as highly as anything can. Within each of those, a
//! match ending nearer the end of its field scores higher: the file name at
//! the end of a path over a directory early in it.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

use super::row::{Field, Highlights, MatchOn, Row};

// ========================================================================
// Constants
// ========================================================================

/// The levels a match earns by how its words sit, highest first.
const LEVEL_WHOLE: u8 = 3;
const LEVEL_FROM_START: u8 = 2;
const LEVEL_IN_ORDER: u8 = 1;
const LEVEL_OUT_OF_ORDER: u8 = 0;

/// The scores within one level, by how many chars of its field follow a
/// match: none scores highest, and as many as this or more lowest.
const NEAR_END_STEPS: usize = 64;

/// The highest score: the whole text, or the whole label.
const SCORE_WHOLE: u8 = scored(LEVEL_WHOLE, 0);

/// The score of a row matched without a word placed in it: every row of an
/// empty query, or of one holding only `!` words.
const SCORE_UNPLACED: u8 = scored(LEVEL_OUT_OF_ORDER, 0);

/// What leaves a word's rows out, before it.
const EXCLUDE: char = '!';

/// What opens and closes a phrase matched as written.
const QUOTE: char = '"';

/// What opens and closes a regular expression.
const SLASH: char = '/';

/// What the named fields of a row are joined by to be matched as one text:
/// a character no query can type, so no match spans two fields.
const FIELD_JOIN: char = '\u{1f}';

// ========================================================================
// Data Structures
// ========================================================================

/// A query parsed once, to score many rows.
#[derive(Clone, Debug)]
pub struct Query {
    /// The words after a `!`, which leave rows out.
    excludes: Vec<Term>,
    /// The words a row must hold, in the order typed.
    shown: Vec<Term>,
    /// Every word, in the order typed, to compare queries by.
    terms: Vec<Term>,
    /// The query as typed, trimmed, for the whole-label match.
    typed: String,
}

/// One word of a query.
#[derive(Clone, Debug)]
enum Term {
    /// Rows the inner word matches are left out; it lights nothing.
    Exclude(Box<Term>),
    /// Text to find as written, in any case: a plain word or a quoted
    /// phrase, already lowercased.
    Literal(String),
    /// A regular expression, kept with its source to compare queries by.
    Pattern(TermPattern),
}

/// A regular expression word, and the text it was compiled from.
#[derive(Clone, Debug)]
struct TermPattern {
    regex: Regex,
    source: String,
}

/// A row's matched fields, joined and lowercased once when it arrives, so
/// each query after scores it without doing either again.
#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    /// How many chars of `lower` the label takes, when the label leads it.
    label_bytes: Option<usize>,
    /// The fields, joined by `FIELD_JOIN`, lowercased.
    lower: String,
    /// How many chars `lower` holds.
    total: usize,
}

/// Where one word matched, in chars of the text matched.
#[derive(Clone, Copy, Debug)]
struct Span {
    end: usize,
    start: usize,
}

// ========================================================================
// Query Implementations
// ========================================================================

impl Query {
    /// Parse `typed` into its words.
    pub fn parse(typed: &str) -> Self {
        let terms: Vec<Term> = split_words(typed)
            .iter()
            .map(|word| parse_term(word))
            .collect();
        let (excludes, shown) = terms
            .iter()
            .cloned()
            .partition::<Vec<_>, _>(|term| matches!(term, Term::Exclude(_)));
        let excludes = excludes
            .into_iter()
            .map(|term| match term {
                Term::Exclude(inner) => *inner,
                Term::Literal(_) | Term::Pattern(_) => term,
            })
            .collect();
        Self {
            excludes,
            shown,
            terms,
            typed: typed.trim().to_lowercase(),
        }
    }

    /// Whether the query has no words, so every row matches.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// `row`'s score, and the letters lit in each field `on` names; `None`
    /// when the row does not match.
    pub fn score_row(&self, row: &Row, on: MatchOn) -> Option<(u8, Highlights)> {
        let fields = on.fields();
        if fields.contains(&Field::Label) && self.is_whole(&row.label) {
            let mut lit = Highlights::default();
            *lit.field_mut(Field::Label) = (0..row.label.chars().count()).collect();
            return Some((SCORE_WHOLE, lit));
        }
        let joined = fields
            .iter()
            .map(|&field| row.field(field))
            .collect::<Vec<_>>()
            .join(&FIELD_JOIN.to_string());
        let (score, positions) = self.score_text(&joined)?;
        Some((score, split_positions(row, &fields, &positions)))
    }

    /// One text's score and its matched char indices; `None` when it does
    /// not match.
    pub fn score_text(&self, text: &str) -> Option<(u8, Vec<usize>)> {
        if self.is_empty() {
            return Some((SCORE_UNPLACED, Vec::new()));
        }
        if self.is_whole(text) {
            return Some((SCORE_WHOLE, (0..text.chars().count()).collect()));
        }
        let lower = text.to_lowercase();
        if self.excluded(text, &lower) {
            return None;
        }
        if self.shown.is_empty() {
            return Some((SCORE_UNPLACED, Vec::new()));
        }
        let total = text.chars().count();
        let mut search = BestMatch::new(text, &lower, total, true);
        search.find(&self.shown, &mut Vec::new(), &mut vec![false; total]);
        search.best
    }

    /// Every place in `text` a word the query must hold is found, as char
    /// ranges, in order: what a view lights for a query searching it. A `!`
    /// word lights nothing.
    pub fn spans(&self, text: &str) -> Vec<Range<usize>> {
        let lower = text.to_lowercase();
        let mut spans: Vec<Range<usize>> = self
            .shown
            .iter()
            .flat_map(|term| term.spans(text, &lower))
            .map(|span| span.start..span.end)
            .collect();
        spans.sort_by_key(|span| (span.start, span.end));
        spans
    }

    /// `prepared`'s score alone, without where it matched: what ranking
    /// every row needs, the highlights being found for the rows on screen.
    pub(crate) fn score_prepared(&self, prepared: &Prepared) -> Option<u8> {
        if self.is_empty() {
            return Some(SCORE_UNPLACED);
        }
        let lower = prepared.lower.as_str();
        let label = prepared.label_bytes.map(|bytes| &lower[..bytes]);
        let whole = |text: &str| !self.typed.is_empty() && text == self.typed;
        if label.is_some_and(whole) || whole(lower) {
            return Some(SCORE_WHOLE);
        }
        if self.excluded(lower, lower) {
            return None;
        }
        match self.shown.as_slice() {
            [] => Some(SCORE_UNPLACED),
            [Term::Literal(word)] => score_one_word(lower, word),
            terms => {
                let mut search = BestMatch::new(lower, lower, prepared.total, false);
                search.find(terms, &mut Vec::new(), &mut vec![false; prepared.total]);
                search.best.map(|(score, _)| score)
            }
        }
    }

    /// Whether the query, as typed, is `text` itself.
    fn is_whole(&self, text: &str) -> bool {
        !self.typed.is_empty() && text.to_lowercase() == self.typed
    }

    /// Whether a `!` word matches the text, leaving it out.
    fn excluded(&self, text: &str, lower: &str) -> bool {
        self.excludes.iter().any(|term| term.occurs(text, lower))
    }
}

/// Whether `new` only narrows `old`: each of `old`'s words still there, a
/// plain or quoted word perhaps lengthened, and words added after them, so
/// every row `new` matches `old` matched too, and only `old`'s matches need
/// scoring again. A regular expression or a `!` word changed at all widens
/// as readily as it narrows, so it does not count.
pub(super) fn narrows(old: &Query, new: &Query) -> bool {
    new.terms.len() >= old.terms.len()
        && old
            .terms
            .iter()
            .zip(&new.terms)
            .all(|(was, now)| match (was, now) {
                (Term::Literal(was), Term::Literal(now)) => now.contains(was.as_str()),
                (Term::Pattern(was), Term::Pattern(now)) => was.source == now.source,
                (Term::Exclude(was), Term::Exclude(now)) => same_term(was, now),
                (Term::Exclude(_) | Term::Literal(_) | Term::Pattern(_), _) => false,
            })
}

// ========================================================================
// Term Implementations
// ========================================================================

impl Term {
    /// Whether the word is found in `text` at all; `lower` is `text`
    /// lowercased.
    fn occurs(&self, text: &str, lower: &str) -> bool {
        match self {
            Term::Exclude(inner) => inner.occurs(text, lower),
            Term::Literal(word) => lower.contains(word.as_str()),
            Term::Pattern(pattern) => pattern.regex.is_match(text),
        }
    }

    /// Every place the word is found in `text`, as char spans; `lower` is
    /// `text` lowercased, which a literal is found in.
    fn spans(&self, text: &str, lower: &str) -> Vec<Span> {
        match self {
            Term::Exclude(inner) => inner.spans(text, lower),
            Term::Literal(word) => lower
                .match_indices(word.as_str())
                .map(|(at, found)| char_span(lower, at, at + found.len()))
                .collect(),
            Term::Pattern(pattern) => pattern
                .regex
                .find_iter(text)
                .map(|found| char_span(text, found.start(), found.end()))
                .collect(),
        }
    }
}

// ========================================================================
// BestMatch Implementations
// ========================================================================

/// The search for the best-scoring way a query's words sit in one text,
/// each in a place of its own.
struct BestMatch<'a> {
    best: Option<(u8, Vec<usize>)>,
    /// Where each field but the last ends: the chars that join the fields.
    joins: Vec<usize>,
    lower: &'a str,
    /// Whether to gather the chars matched, or only the score.
    positions: bool,
    text: &'a str,
    total: usize,
}

impl<'a> BestMatch<'a> {
    fn new(text: &'a str, lower: &'a str, total: usize, positions: bool) -> Self {
        let joins = text
            .chars()
            .enumerate()
            .filter_map(|(at, c)| (c == FIELD_JOIN).then_some(at))
            .collect();
        Self {
            best: None,
            joins,
            lower,
            positions,
            text,
            total,
        }
    }

    /// Try every place for the first of `terms`, then the rest after it,
    /// keeping the best whole assignment found; a perfect one ends the search.
    fn find(&mut self, terms: &[Term], placed: &mut Vec<Span>, taken: &mut [bool]) {
        if self
            .best
            .as_ref()
            .is_some_and(|(score, _)| *score == SCORE_WHOLE)
        {
            return;
        }
        let Some((term, rest)) = terms.split_first() else {
            let found = evaluate(self.total, &self.joins, placed, self.positions);
            if self.best.as_ref().is_none_or(|(score, _)| found.0 > *score) {
                self.best = Some(found);
            }
            return;
        };
        for span in term.spans(self.text, self.lower) {
            let free = span.end <= taken.len() && taken[span.start..span.end].iter().all(|t| !t);
            if !free {
                continue;
            }
            taken[span.start..span.end].fill(true);
            placed.push(span);
            self.find(rest, placed, taken);
            placed.pop();
            taken[span.start..span.end].fill(false);
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `typed` split into its words at spaces, a quoted phrase kept whole with
/// its quotes, so `parse_term` can tell it from a plain word.
fn split_words(typed: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in typed.chars() {
        match c {
            QUOTE => {
                quoted = !quoted;
                word.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// One word as a term: `!` before anything but itself leaves rows out, a
/// quoted phrase is literal, `/…/` a regular expression (literal when it does
/// not parse), and anything else literal.
fn parse_term(word: &str) -> Term {
    if let Some(inner) = word.strip_prefix(EXCLUDE).filter(|inner| !inner.is_empty()) {
        return Term::Exclude(Box::new(parse_term(inner)));
    }
    if let Some(phrase) = word.strip_prefix(QUOTE) {
        let phrase = phrase.strip_suffix(QUOTE).unwrap_or(phrase);
        return Term::Literal(phrase.to_lowercase());
    }
    let pattern = word
        .strip_prefix(SLASH)
        .filter(|_| word.len() > 1)
        .map(|inner| inner.strip_suffix(SLASH).unwrap_or(inner));
    if let Some(source) = pattern {
        let compiled = RegexBuilder::new(source).case_insensitive(true).build();
        if let Ok(regex) = compiled {
            return Term::Pattern(TermPattern {
                regex,
                source: source.to_string(),
            });
        }
        return Term::Literal(source.to_lowercase());
    }
    Term::Literal(word.to_lowercase())
}

/// Whether two terms are the same word.
fn same_term(a: &Term, b: &Term) -> bool {
    match (a, b) {
        (Term::Exclude(a), Term::Exclude(b)) => same_term(a, b),
        (Term::Literal(a), Term::Literal(b)) => a == b,
        (Term::Pattern(a), Term::Pattern(b)) => a.source == b.source,
        (Term::Exclude(_) | Term::Literal(_) | Term::Pattern(_), _) => false,
    }
}

/// A row's matched fields as the matcher keeps them, ready to score.
pub(crate) fn prepare(row: &Row, on: MatchOn) -> Prepared {
    let fields = on.fields();
    let lower = fields
        .iter()
        .map(|&field| row.field(field).to_lowercase())
        .collect::<Vec<_>>()
        .join(&FIELD_JOIN.to_string());
    let label_bytes =
        (fields.first() == Some(&Field::Label)).then(|| row.label.to_lowercase().len());
    Prepared {
        label_bytes,
        total: lower.chars().count(),
        lower,
    }
}

/// The score of one plain word in `lower`, found at its best place: the
/// whole text, its start, or else as near the end of its field as it is
/// found.
fn score_one_word(lower: &str, word: &str) -> Option<u8> {
    if lower == word {
        return Some(SCORE_WHOLE);
    }
    // The chars of its field after a match ending at byte `end`.
    let after = |end: usize| {
        let rest = &lower[end..];
        let field = rest.find(FIELD_JOIN).map_or(rest, |join| &rest[..join]);
        field.chars().count()
    };
    if lower.starts_with(word) {
        return Some(scored(LEVEL_FROM_START, after(word.len())));
    }
    let at = lower.rfind(word)?;
    Some(scored(LEVEL_IN_ORDER, after(at + word.len())))
}

/// The score of words placed at `spans`, in the order typed, in a text of
/// `total` chars whose fields are joined at the chars `joins`, and every
/// char they cover when `positions` asks.
fn evaluate(total: usize, joins: &[usize], spans: &[Span], positions: bool) -> (u8, Vec<usize>) {
    let in_order = spans.windows(2).all(|pair| pair[0].end <= pair[1].start);
    let from_start = spans.first().is_some_and(|span| span.start == 0);
    let to_end = spans.last().is_some_and(|span| span.end == total);
    let level = match (in_order, from_start, to_end) {
        (false, _, _) => LEVEL_OUT_OF_ORDER,
        (true, true, true) => LEVEL_WHOLE,
        (true, true, false) => LEVEL_FROM_START,
        (true, false, _) => LEVEL_IN_ORDER,
    };
    // How many chars of its field follow the match ending last.
    let end = spans.iter().map(|span| span.end).max().unwrap_or(total);
    let field_end = joins
        .iter()
        .copied()
        .find(|&join| join >= end)
        .unwrap_or(total);
    let score = scored(level, field_end - end);
    if !positions {
        return (score, Vec::new());
    }
    let mut covered: Vec<usize> = spans.iter().flat_map(|span| span.start..span.end).collect();
    covered.sort_unstable();
    covered.dedup();
    (score, covered)
}

/// The score of a match of `level` followed by `after` chars of its field:
/// the level first, then the nearer the field's end the higher.
const fn scored(level: u8, after: usize) -> u8 {
    let after = match after < NEAR_END_STEPS {
        true => after,
        false => NEAR_END_STEPS - 1,
    };
    level * NEAR_END_STEPS as u8 + (NEAR_END_STEPS - 1 - after) as u8
}

/// The char span of the bytes `start..end` of `text`.
fn char_span(text: &str, start: usize, end: usize) -> Span {
    let start_chars = text[..start].chars().count();
    Span {
        end: start_chars + text[start..end].chars().count(),
        start: start_chars,
    }
}

/// Positions in the fields of `row` named by `fields`, joined by one char
/// each, sorted back to the field each falls in.
fn split_positions(row: &Row, fields: &[Field], positions: &[usize]) -> Highlights {
    let mut lit = Highlights::default();
    let mut offset = 0;
    for &field in fields {
        let length = row.field(field).chars().count();
        let end = offset + length;
        lit.field_mut(field).extend(
            positions
                .iter()
                .filter(|&&at| at >= offset && at < end)
                .map(|&at| at - offset),
        );
        offset = end + 1;
    }
    lit
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::row::RowKey;
    use super::*;

    fn score(text: &str, query: &str) -> Option<(u8, Vec<usize>)> {
        Query::parse(query).score_text(text)
    }

    /// `text`'s level for `query`, and the chars it lit.
    fn leveled(text: &str, query: &str) -> Option<(u8, Vec<usize>)> {
        score(text, query).map(|(points, lit)| (level(points), lit))
    }

    /// The level a score is in.
    fn level(points: u8) -> u8 {
        points / NEAR_END_STEPS as u8
    }

    fn row(label: &str) -> Row {
        Row::new(RowKey(0), label)
    }

    #[test]
    fn test_a_whole_label_scores_highest_and_lights_all() {
        let (points, lit) = score("Split Vertical", "split vertical").unwrap();
        assert_eq!(level(points), LEVEL_WHOLE);
        assert_eq!(lit.len(), "split vertical".len());
    }

    #[test]
    fn test_a_word_inside_scores_in_order_and_lights_a_run() {
        let (points, lit) = score("Split Vertical", "vert").unwrap();
        assert_eq!(level(points), LEVEL_IN_ORDER);
        assert_eq!(lit, [6, 7, 8, 9]);
    }

    #[test]
    fn test_a_word_is_found_whole_never_as_scattered_letters() {
        assert!(score("Split Vertical", "sv").is_none());
        assert!(score("a_big_cat", "abc").is_none());
        assert!(score("Split_Vertical", "splt vert").is_none());
        assert!(score("my abc file", "abc").is_some());
    }

    #[test]
    fn test_words_in_order_score_by_where_they_sit() {
        let (points, lit) = score("Split_Vertical", "split vertical").unwrap();
        assert_eq!(level(points), LEVEL_WHOLE);
        assert_eq!(lit, [0, 1, 2, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13]);
        assert_eq!(
            level(score("Split Vertical", "split vert").unwrap().0),
            LEVEL_FROM_START
        );
        let path = "crates/autumn-leaf/src/model/picker/mod.rs";
        let (points, lit) = score(path, "model picker").unwrap();
        assert_eq!(level(points), LEVEL_IN_ORDER);
        assert_eq!(lit, [23, 24, 25, 26, 27, 29, 30, 31, 32, 33, 34]);
        assert_eq!(
            leveled("windup-term", "win   term").unwrap(),
            (LEVEL_WHOLE, vec![0, 1, 2, 7, 8, 9, 10])
        );
        assert_eq!(
            leveled("terminal", "term ").unwrap(),
            (LEVEL_FROM_START, vec![0, 1, 2, 3])
        );
    }

    #[test]
    fn test_a_match_nearer_the_end_of_its_field_ranks_higher_within_its_level() {
        // The file name at the end of a path over a directory early in it.
        let near = score("src/view/model.rs", "model").unwrap();
        let far = score("src/model/view.rs", "model").unwrap();
        assert!(near.0 > far.0, "{near:?} over {far:?}");
        assert_eq!(
            level(near.0),
            level(far.0),
            "one level: neither is a prefix"
        );
        // A word found twice lights, and scores by, its place nearest the end.
        let (points, lit) = score("src/model/sub/model.rs", "model").unwrap();
        assert_eq!(lit, [14, 15, 16, 17, 18]);
        assert_eq!(
            points,
            Query::parse("model")
                .score_prepared(&prepare(&row("src/model/sub/model.rs"), MatchOn::LABEL))
                .unwrap()
        );
        // A prefix still outranks a match nearer the end.
        let prefix = score("model/a/very/long/path/to/the/end.rs", "model").unwrap();
        assert!(prefix.0 > near.0);
    }

    #[test]
    fn test_words_out_of_order_still_match_below_in_order() {
        let (points, lit) = score("my def and abc here", "abc def").unwrap();
        assert_eq!(level(points), LEVEL_OUT_OF_ORDER);
        assert_eq!(lit, [3, 4, 5, 11, 12, 13]);
        assert_eq!(
            level(score("my abc and def here", "abc def").unwrap().0),
            LEVEL_IN_ORDER
        );
        assert_eq!(
            level(score("Split Vertical", "vert split").unwrap().0),
            LEVEL_OUT_OF_ORDER
        );
    }

    #[test]
    fn test_a_plain_word_matches_its_symbols_literally() {
        assert_eq!(score("Press '+' to display", "+").unwrap().1, [7]);
        assert_eq!(score("C++ programming", "c++").unwrap().1, [0, 1, 2]);
        assert_eq!(
            score("my_file.rs", "file.rs").unwrap().1,
            [3, 4, 5, 6, 7, 8, 9]
        );
        assert!(score("my_file_rs", "file.rs").is_none());
    }

    #[test]
    fn test_a_slash_word_is_a_regular_expression() {
        assert_eq!(
            leveled("Split Vertical", "/^split/").unwrap(),
            (LEVEL_FROM_START, vec![0, 1, 2, 3, 4])
        );
        assert!(score("Split Vertical", "/^vertical/").is_none());
        assert_eq!(score("document_123.rs", r"/\d+/").unwrap().1, [9, 10, 11]);
        assert_eq!(score("color red", "/blue|red/").unwrap().1, [6, 7, 8]);
        assert!(
            score("a(b", "/(/").is_some(),
            "a pattern that does not parse is literal"
        );
    }

    #[test]
    fn test_a_bang_word_leaves_out_the_rows_it_matches_and_lights_nothing() {
        assert!(score("tests/mod.rs", "mod !test").is_none());
        assert_eq!(score("src/mod.rs", "mod !test").unwrap().1, [4, 5, 6]);
        assert!(score("tmp/a", "!/^tmp/").is_none());
        assert!(score("src/tmp", "!/^tmp/").is_some());
    }

    #[test]
    fn test_a_quoted_phrase_matches_as_written() {
        assert!(score("fn main()", "\"fn main\"").is_some());
        assert!(score("fn x main", "\"fn main\"").is_none());
        assert_eq!(score("x a/b", "\"a/b\"").unwrap().1, [2, 3, 4]);
    }

    #[test]
    fn test_narrowing_is_only_adding_or_lengthening_plain_words() {
        let q = Query::parse;
        assert!(narrows(&q("fo"), &q("foo")));
        assert!(narrows(&q("fo"), &q("fo !x")));
        assert!(narrows(&q("\"a b\""), &q("\"a bc\"")));
        assert!(!narrows(&q("/a|/"), &q("/a|b/")));
        assert!(!narrows(&q("!fo"), &q("!foo")));
        assert!(!narrows(&q("foo"), &q("fo")));
    }

    #[test]
    fn test_only_the_fields_named_are_met_and_lit() {
        let line = row("12").with_description("fn twelve");
        let query = Query::parse("12");
        assert!(query.score_row(&line, MatchOn::DESCRIPTION).is_none());
        let (_, lit) = Query::parse("twelve")
            .score_row(&line, MatchOn::DESCRIPTION)
            .unwrap();
        assert_eq!(lit.description, [3, 4, 5, 6, 7, 8]);
        assert!(lit.label.is_empty());
    }

    #[test]
    fn test_a_prepared_row_scores_as_the_row_does() {
        let cases = [
            ("Split Vertical", "split vertical"),
            ("Split Vertical", "split vert"),
            ("Split Vertical", "vert"),
            ("Split Vertical", "vert split"),
            ("terminal", "term"),
            ("src/mod.rs", "mod !test"),
            ("tests/mod.rs", "mod !test"),
            ("color red", "/blue|red/"),
            ("fn main()", "\"fn main\""),
            ("abc", "abc"),
            ("xabc", "abc"),
            ("abcx", "abc"),
        ];
        for (label, query) in cases {
            let query = Query::parse(query);
            let row = row(label);
            let prepared = prepare(&row, MatchOn::LABEL);
            let expected = query
                .score_row(&row, MatchOn::LABEL)
                .map(|(score, _)| score);
            assert_eq!(query.score_prepared(&prepared), expected, "{label}");
        }
    }

    #[test]
    fn test_spans_are_every_place_each_word_is_found_and_bang_words_none() {
        let query = Query::parse("ab !x c");
        assert_eq!(query.spans("ab c ab x"), [0..2, 3..4, 5..7]);
        assert!(Query::parse("!ab").spans("ab").is_empty());
    }

    #[test]
    fn test_a_word_never_spans_two_fields() {
        let hit = row("ab").with_detail("cd");
        let on = MatchOn::LABEL | MatchOn::DETAIL;
        assert!(Query::parse("bc").score_row(&hit, on).is_none());
        let (_, lit) = Query::parse("c").score_row(&hit, on).unwrap();
        assert_eq!(lit.detail, [0]);
    }
}
