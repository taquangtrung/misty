//! [`Find`]: a search through a widget's text, the matches it found and
//! the one stepped to, shared by the views that search their text (the
//! code view, the rich text view). What a match is (a byte range, a range
//! in one paragraph) and how the current one is scrolled into view stay
//! with each widget.

use std::ops::Range;

// ========================================================================
// Data Structures
// ========================================================================

/// A search and its matches, of type `M`, in reading order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Find<M> {
    /// Whether letters match only in the same case.
    case_sensitive: bool,
    /// The match stepped to, or `None` when there are none.
    current: Option<usize>,
    /// Every match, in reading order.
    matches: Vec<M>,
    /// What is searched for.
    query: String,
}

// ========================================================================
// Find Implementations
// ========================================================================

impl<M> Find<M> {
    /// A search for `query` that found `matches`; the current one is the
    /// first `is_start` accepts (the first at the cursor, or in sight), or
    /// the first of all when it accepts none.
    pub(crate) fn new(
        query: &str,
        case_sensitive: bool,
        matches: Vec<M>,
        is_start: impl Fn(&M) -> bool,
    ) -> Self {
        let current = matches
            .iter()
            .position(is_start)
            .or_else(|| (!matches.is_empty()).then_some(0));
        Self {
            case_sensitive,
            current,
            matches,
            query: query.to_owned(),
        }
    }

    /// Whether letters match only in the same case.
    pub(crate) fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// What is searched for.
    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    /// Every match, in reading order.
    pub(crate) fn matches(&self) -> &[M] {
        &self.matches
    }

    /// The index of the current match, or `None` when there are none.
    pub(crate) fn current(&self) -> Option<usize> {
        self.current
    }

    /// The current match, or `None` when there are none.
    pub(crate) fn current_match(&self) -> Option<&M> {
        self.matches.get(self.current?)
    }

    /// Moves the current match `delta` matches, wrapping round either end;
    /// with none current, a step forward lands on the first and a step back
    /// on the last. Returns whether there was a match to step to.
    pub(crate) fn step(&mut self, delta: isize) -> bool {
        let count = self.matches.len() as isize;
        if count == 0 {
            return false;
        }
        let from = self
            .current
            .map_or(if delta > 0 { -1 } else { 0 }, |current| current as isize);
        self.current = Some((from + delta).rem_euclid(count) as usize);
        true
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The ranges of `text` that match `query`, in order and not overlapping:
/// exactly when `case_sensitive`, else each character matching in either
/// case.
pub(crate) fn find_all(text: &str, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    if case_sensitive {
        return text
            .match_indices(query)
            .map(|(start, found)| start..start + found.len())
            .collect();
    }
    let folded: Vec<char> = query.chars().collect();
    let same = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let mut ranges = Vec::new();
    let mut next_start = 0;
    for (start, _) in text.char_indices() {
        if start < next_start {
            continue;
        }
        let mut chars = text[start..].char_indices();
        let matched = folded
            .iter()
            .all(|wanted| chars.next().is_some_and(|(_, found)| same(found, *wanted)));
        if matched {
            let end = chars.next().map_or(text.len(), |(index, _)| start + index);
            ranges.push(start..end);
            next_start = end;
        }
    }
    ranges
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_case_folding_matches_whole_characters() {
        let folded = find_all("Straße STRASSE", "straße", false);
        assert_eq!(folded.first(), Some(&(0..7)));
        assert_eq!(folded.len(), 1, "ß is no ss, character by character");
        assert_eq!(find_all("aAa", "a", false), [0..1, 1..2, 2..3]);
        assert_eq!(
            find_all("aaaa", "aa", true),
            [0..2, 2..4],
            "never overlapping"
        );
    }

    #[test]
    fn test_stepping_wraps_round_both_ends() {
        let mut find = Find::new("a", false, vec![10, 20, 30], |&at| at >= 25);
        assert_eq!(find.current(), Some(2), "the first at the start");
        assert!(find.step(1));
        assert_eq!(find.current(), Some(0), "past the last to the first");
        assert!(find.step(-1));
        assert_eq!(
            find.current_match(),
            Some(&30),
            "before the first to the last"
        );
        let mut none = Find::new("a", false, Vec::<usize>::new(), |_| true);
        assert!(!none.step(1), "nothing to step to");
        assert_eq!(none.current(), None);
    }
}
