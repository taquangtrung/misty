//! [`Highlighter`], behind the `highlight` feature: [`Highlight`] spans
//! for a [`CodeView`](crate::widgets::CodeView) from the TextMate grammars VS Code
//! ships, through iris (Rust, Python, C, JavaScript, Markdown, HTML, JSON,
//! and some eighty more, embedded languages included), named with the
//! highlight names misty's themes color (`keyword`, `string`, `function`,
//! `type`, ...).
//!
//! Make one highlighter and keep it: it compiles each language's grammar
//! the first time it highlights that language, and its patterns as they
//! first match. Highlighting reads the whole text, line by line, at tens
//! of megabytes a second once warm: for a large file, highlight on a
//! worker thread (a highlighter is `Send` and `Sync`) and send the spans
//! to the app through its [`Waker`](crate::Waker), showing the text plain
//! until they come.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};

use iris::{Grammar, HIGHLIGHT_NAMES, Languages, Registry, Tokenizer};

use super::code_view::Highlight;

// ========================================================================
// Constants
// ========================================================================

/// iris's highlight names misty's themes call otherwise; every other name
/// is misty's too, a theme lacking a dotted name falling back to its first
/// parts (`keyword.control` to `keyword`).
const RENAMED: &[(&str, &str)] = &[
    ("escape", "string.escape"),
    ("markup.heading", "text.title"),
    ("markup.link", "text.uri"),
    ("markup.raw", "text.literal"),
    ("tag", "label"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// VS Code's grammars, ready to highlight text. Make one and keep it.
pub struct Highlighter {
    /// Each language's grammar, compiled the first time it highlights, by
    /// grammar scope; `None` for a scope with no grammar.
    grammars: Mutex<HashMap<String, Option<Arc<Grammar>>>>,
    /// The languages, to find one by name or extension.
    languages: Languages,
    /// The highlight name of each of iris's, by index.
    names: Vec<Arc<str>>,
    /// Resolves scopes to iris's highlight names.
    resolver: iris::Highlighter,
    /// The grammars.
    registry: Registry,
}

// ========================================================================
// Highlighter Implementations
// ========================================================================

impl Highlighter {
    /// A highlighter of VS Code's grammars.
    pub fn new() -> Self {
        let names = HIGHLIGHT_NAMES
            .iter()
            .map(|&name| {
                let renamed = RENAMED.iter().find(|(from, _)| *from == name);
                Arc::from(renamed.map_or(name, |(_, to)| to))
            })
            .collect();
        Self {
            grammars: Mutex::new(HashMap::new()),
            languages: Languages::builtin(),
            names,
            resolver: iris::Highlighter::new(HIGHLIGHT_NAMES),
            registry: Registry::bundled(),
        }
    }

    /// The spans of `text` written in the language `token` names: a file
    /// extension (`rs`, `py`) or a language's name (`Rust`, `Python`), in
    /// any case. Empty for a language it does not know, so the text shows
    /// plain.
    pub fn highlight(&self, text: &str, token: &str) -> Vec<Highlight> {
        let Some(grammar) = self.grammar(token) else {
            return Vec::new();
        };
        let mut tokenizer = Tokenizer::new(grammar);
        let mut spans: Vec<Highlight> = Vec::new();
        let mut state = None;
        let mut line_start = 0;
        for line in text.split_inclusive('\n') {
            let content = line.strip_suffix('\n').unwrap_or(line);
            let tokenized = tokenizer.tokenize_line(content, state.as_ref());
            for token in &tokenized.tokens {
                let name = self.resolver.highlight(&token.scopes);
                let range = line_start + token.start..line_start + token.end;
                self.push(&mut spans, name, range);
            }
            state = Some(tokenized.state);
            line_start += line.len();
        }
        spans
    }

    /// The grammar of the language `token` names, by name, alias, or
    /// extension, compiled the first time it is asked for.
    fn grammar(&self, token: &str) -> Option<Arc<Grammar>> {
        let language = self
            .languages
            .get(token)
            .or_else(|| self.languages.detect(format!("file.{token}"), ""))?;
        let scope = language.scope.as_deref()?;
        let mut grammars = self.grammars.lock().unwrap_or_else(PoisonError::into_inner);
        grammars
            .entry(scope.to_owned())
            .or_insert_with(|| self.registry.grammar(scope).ok())
            .clone()
    }

    /// Adds `range`, highlighted as iris's name `name`, to `spans`: joined
    /// to the last span when it has the same name and touches it, left out
    /// when it is plain.
    fn push(&self, spans: &mut Vec<Highlight>, name: Option<usize>, range: Range<usize>) {
        let Some(name) = name.and_then(|index| self.names.get(index)) else {
            return;
        };
        if range.is_empty() {
            return;
        }
        if let Some(last) = spans.last_mut()
            && last.range.end == range.start
            && last.name == *name
        {
            last.range.end = range.end;
            return;
        }
        spans.push(Highlight {
            name: Arc::clone(name),
            range,
        });
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Highlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Highlighter").finish_non_exhaustive()
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
