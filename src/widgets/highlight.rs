//! [`Highlighter`], behind the `highlight` feature: [`Highlight`] spans
//! for a [`CodeView`](crate::CodeView) from the Sublime syntaxes bundled
//! with syntect (Rust, Python, C, JavaScript, Markdown, HTML, JSON, and
//! some seventy more), named with the highlight names misty's themes
//! color (`keyword`, `string`, `function`, `type`, ...).
//!
//! Loading the syntaxes takes a few tens of milliseconds, so make one
//! highlighter and keep it. Highlighting reads the whole text, line by
//! line, at about a few megabytes a second: for a large file, highlight on
//! a worker thread (a highlighter is `Send` and `Sync`) and send the spans
//! to the app through its [`Waker`](crate::Waker), showing the text plain
//! until they come.

use std::ops::Range;
use std::sync::Arc;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use super::code_view::Highlight;

// ========================================================================
// Constants
// ========================================================================

/// The highlight name of each syntect scope misty colors, matched as a
/// prefix of a text's innermost scope; the longest match wins. An empty
/// name passes the text to the scope around it: the quotes of a string and
/// the slashes of a comment take the string's and the comment's color.
const SCOPE_NAMES: &[(&str, &str)] = &[
    ("comment", "comment"),
    ("constant", "constant"),
    ("constant.character.escape", "string.escape"),
    ("constant.language.boolean", "boolean"),
    ("constant.numeric", "number"),
    ("entity.name.function", "function"),
    ("entity.name.macro", "function.macro"),
    ("entity.name.class", "type"),
    ("entity.name.enum", "type"),
    ("entity.name.struct", "type"),
    ("entity.name.tag", "label"),
    ("entity.name.type", "type"),
    ("entity.other.attribute-name", "attribute"),
    ("keyword", "keyword"),
    ("keyword.operator", "operator"),
    ("markup.heading", "text.title"),
    ("markup.raw", "text.literal"),
    ("markup.underline.link", "text.uri"),
    ("punctuation", "punctuation"),
    ("punctuation.definition", ""),
    ("storage", "keyword"),
    ("string", "string"),
    ("support.constant", "constant"),
    ("support.function", "function.builtin"),
    ("support.macro", "function.macro"),
    ("support.type", "type"),
    ("variable.language", "variable.builtin"),
    ("variable.parameter", "variable.parameter"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// The bundled syntaxes, ready to highlight text. Make one and keep it.
#[derive(Debug)]
pub struct Highlighter {
    /// The scope prefixes misty colors, longest first, with their names.
    names: Vec<(Scope, Arc<str>)>,
    /// The syntaxes.
    syntaxes: SyntaxSet,
}

// ========================================================================
// Highlighter Implementations
// ========================================================================

impl Highlighter {
    /// A highlighter of the bundled syntaxes.
    pub fn new() -> Self {
        let mut names: Vec<(Scope, Arc<str>)> = SCOPE_NAMES
            .iter()
            .filter_map(|&(scope, name)| Some((Scope::new(scope).ok()?, Arc::from(name))))
            .collect();
        names.sort_by_key(|(scope, _)| std::cmp::Reverse(scope.len()));
        Self {
            names,
            syntaxes: SyntaxSet::load_defaults_newlines(),
        }
    }

    /// The spans of `text` written in the language `token` names: a file
    /// extension (`rs`, `py`) or a syntax's name (`Rust`, `Python`), in
    /// any case. Empty for a language it does not know, so the text shows
    /// plain.
    pub fn highlight(&self, text: &str, token: &str) -> Vec<Highlight> {
        let Some(syntax) = self.syntax(token) else {
            return Vec::new();
        };
        let mut state = ParseState::new(syntax);
        let mut stack = ScopeStack::new();
        let mut spans: Vec<Highlight> = Vec::new();
        let mut line_start = 0;
        for line in text.split_inclusive('\n') {
            let Ok(ops) = state.parse_line(line, &self.syntaxes) else {
                break;
            };
            let mut at = 0;
            for (offset, op) in ops {
                self.push(&mut spans, &stack, line_start + at..line_start + offset);
                at = offset;
                if stack.apply(&op).is_err() {
                    return spans;
                }
            }
            self.push(&mut spans, &stack, line_start + at..line_start + line.len());
            line_start += line.len();
        }
        spans
    }

    /// The syntax `token` names, by extension or name, in any case.
    fn syntax(&self, token: &str) -> Option<&SyntaxReference> {
        self.syntaxes.find_syntax_by_token(token).or_else(|| {
            let lower = token.to_lowercase();
            self.syntaxes
                .syntaxes()
                .iter()
                .find(|syntax| syntax.name.to_lowercase() == lower)
        })
    }

    /// Adds `range`, in the scopes of `stack`, to `spans`: joined to the
    /// last span when it has the same name and touches it, left out when
    /// misty colors none of the scopes.
    fn push(&self, spans: &mut Vec<Highlight>, stack: &ScopeStack, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let Some(name) = self.name_of(stack) else {
            return;
        };
        if let Some(last) = spans.last_mut()
            && last.range.end == range.start
            && last.name == name
        {
            last.range.end = range.end;
            return;
        }
        spans.push(Highlight { name, range });
    }

    /// The highlight name of the innermost scope of `stack` misty colors.
    fn name_of(&self, stack: &ScopeStack) -> Option<Arc<str>> {
        stack.as_slice().iter().rev().find_map(|scope| {
            self.names
                .iter()
                .find(|(prefix, _)| prefix.is_prefix_of(*scope))
                .filter(|(_, name)| !name.is_empty())
                .map(|(_, name)| Arc::clone(name))
        })
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The names of the spans of `text` covering `word` (its first
    /// occurrence).
    fn names_over<'a>(spans: &'a [Highlight], text: &str, word: &str) -> Vec<&'a str> {
        let start = text.find(word).expect("the word is there");
        let end = start + word.len();
        spans
            .iter()
            .filter(|span| span.range.start < end && span.range.end > start)
            .map(|span| &*span.name)
            .collect()
    }

    #[test]
    fn test_a_rust_snippet_gets_misty_highlight_names() {
        let highlighter = Highlighter::new();
        let text = "// note\nfn main() {\n    let s = \"hi\\n\";\n    let n = 42;\n}\n";
        let spans = highlighter.highlight(text, "rs");
        assert_eq!(names_over(&spans, text, "// note"), ["comment"]);
        assert!(names_over(&spans, text, "fn").contains(&"keyword"));
        assert!(names_over(&spans, text, "main").contains(&"function"));
        assert!(names_over(&spans, text, "\"hi").contains(&"string"));
        assert!(names_over(&spans, text, "\\n").contains(&"string.escape"));
        assert_eq!(names_over(&spans, text, "42"), ["number"]);
        let mut ordered = spans.clone();
        ordered.sort_by_key(|span| span.range.start);
        assert_eq!(ordered, spans, "in order");
        assert!(
            spans
                .windows(2)
                .all(|pair| pair[0].range.end <= pair[1].range.start)
        );
    }

    #[test]
    fn test_a_language_is_found_by_name_in_any_case_and_an_unknown_is_plain() {
        let highlighter = Highlighter::new();
        assert!(!highlighter.highlight("x = 1\n", "PYTHON").is_empty());
        assert!(
            highlighter
                .highlight("x = 1\n", "no-such-language")
                .is_empty()
        );
    }
}
