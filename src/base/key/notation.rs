//! The key notation: how a [`Key`] is written in keymaps, docs, and help.
//!
//! A key is its modifiers, each a letter and a dash (`C-` Control, `M-` Alt,
//! `S-` Shift, `s-` Super), followed by either one character or a named key in
//! angle brackets: `x`, `G`, `C-x`, `C-S-f`, `M-?`, `S-<tab>`, `<esc>`,
//! `<f5>`. A sequence is keys separated by spaces: `C-x C-f`, `g g`.
//!
//! Parsing also accepts the Emacs spellings, so keymaps written for Emacs
//! read as they are: the bare names `RET`, `TAB`, `SPC`, `ESC`, and `DEL`
//! (uppercase, as Emacs writes them, and with modifiers: `C-RET`, `S-TAB`),
//! and the bracketed names `<return>`, `<escape>`, `<backspace>`, `<delete>`,
//! `<next>` (Page Down), and `<prior>` (Page Up). Mind one asymmetry, inherited
//! from Emacs: bare `DEL` is Backspace, while bracketed `<del>` is Delete.
//! Formatting never writes these spellings; it always writes the canonical
//! form, so `RET` reads back as `<cr>`.
//!
//! Parsing applies the normalization of [`Key::new`], so `S-g` and `G` are the
//! same key, and formatting writes each key in its one canonical form, with
//! modifiers in the order `C-M-S-s-`.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use super::{Key, KeyCode, Mods};

// ========================================================================
// Constants
// ========================================================================

/// The modifier prefixes, in the order a key is written with them.
const MODIFIER_PREFIXES: [(char, Mods); 4] = [
    ('C', Mods::CTRL),
    ('M', Mods::ALT),
    ('S', Mods::SHIFT),
    ('s', Mods::SUPER),
];

/// The named keys, as written between angle brackets. The first name listed
/// for a key is the one it is formatted with.
const NAMED_KEYS: [(&str, KeyCode); 15] = [
    ("bs", KeyCode::Backspace),
    ("cr", KeyCode::Enter),
    ("del", KeyCode::Delete),
    ("down", KeyCode::Down),
    ("end", KeyCode::End),
    ("esc", KeyCode::Esc),
    ("home", KeyCode::Home),
    ("insert", KeyCode::Insert),
    ("left", KeyCode::Left),
    ("pagedown", KeyCode::PageDown),
    ("pageup", KeyCode::PageUp),
    ("right", KeyCode::Right),
    ("space", KeyCode::Char(' ')),
    ("tab", KeyCode::Tab),
    ("up", KeyCode::Up),
];

/// The Emacs spellings of named keys, accepted between angle brackets when
/// parsing but never written when formatting.
const NAME_ALIASES: [(&str, KeyCode); 6] = [
    ("backspace", KeyCode::Backspace),
    ("delete", KeyCode::Delete),
    ("escape", KeyCode::Esc),
    ("next", KeyCode::PageDown),
    ("prior", KeyCode::PageUp),
    ("return", KeyCode::Enter),
];

/// The Emacs names written bare, without angle brackets, matched only as the
/// whole rest of a token and with their case exactly as listed. `DEL` is
/// Backspace, as in Emacs, not Delete.
const BARE_NAMES: [(&str, KeyCode); 5] = [
    ("DEL", KeyCode::Backspace),
    ("ESC", KeyCode::Esc),
    ("RET", KeyCode::Enter),
    ("SPC", KeyCode::Char(' ')),
    ("TAB", KeyCode::Tab),
];

/// The highest function key the notation names (`<f24>`).
const MAX_FUNCTION_KEY: u8 = 24;

// ========================================================================
// Data Structures
// ========================================================================

/// Why a key or a sequence could not be read.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum KeyParseError {
    /// Nothing was written.
    #[error("no key given")]
    Empty,
    /// Modifiers were written with no key after them, as in `C-`.
    #[error("no key after the modifiers in {0:?}")]
    MissingKey(String),
    /// More than one character was written where one key was expected, as in
    /// `ab`; a sequence separates its keys with spaces.
    #[error("{0:?} is more than one key; separate keys with spaces")]
    MultipleChars(String),
    /// An angle-bracketed name that names no key, as in `<nope>`.
    #[error("no key is named <{0}>")]
    UnknownName(String),
}

// ========================================================================
// Key Implementations
// ========================================================================

impl fmt::Display for Key {
    /// Writes the key in its canonical form: `C-S-f`, `M-?`, `S-<tab>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (letter, modifier) in MODIFIER_PREFIXES {
            if self.mods.contains(modifier) {
                write!(f, "{letter}-")?;
            }
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("<space>"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::F(n) => write!(f, "<f{n}>"),
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::End
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::Up => write!(f, "<{}>", name_of(self.code)),
        }
    }
}

impl FromStr for Key {
    type Err = KeyParseError;

    /// Reads one key in the key notation, as [`parse_key`] does, so
    /// `"C-x".parse::<Key>()` works.
    fn from_str(token: &str) -> Result<Self, Self::Err> {
        parse_key(token)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Reads one key, such as `C-x`, `G`, or `S-<tab>`.
pub fn parse_key(token: &str) -> Result<Key, KeyParseError> {
    if token.is_empty() {
        return Err(KeyParseError::Empty);
    }
    let mut mods = Mods::NONE;
    let mut rest = token;
    while let Some(modifier) = leading_modifier(rest) {
        mods |= modifier;
        rest = &rest[2..];
    }
    if rest.is_empty() {
        return Err(KeyParseError::MissingKey(token.to_string()));
    }
    Ok(Key::new(parse_code(rest)?, mods))
}

/// Reads a sequence of keys separated by whitespace, such as `C-x C-f`.
pub fn parse_key_sequence(spec: &str) -> Result<Vec<Key>, KeyParseError> {
    let keys = spec
        .split_whitespace()
        .map(parse_key)
        .collect::<Result<Vec<_>, _>>()?;
    if keys.is_empty() {
        return Err(KeyParseError::Empty);
    }
    Ok(keys)
}

/// The modifier `rest` opens with, when it opens with a modifier prefix that
/// still has a key after it: in `C--`, the first `C-` is a prefix and the
/// last `-` is the key.
fn leading_modifier(rest: &str) -> Option<Mods> {
    let mut chars = rest.chars();
    let (letter, dash) = (chars.next()?, chars.next()?);
    if dash != '-' {
        return None;
    }
    MODIFIER_PREFIXES
        .iter()
        .find(|(prefix, _)| *prefix == letter)
        .map(|(_, modifier)| *modifier)
}

/// The key `rest` names: one character, a bare Emacs name, or a name in angle
/// brackets.
fn parse_code(rest: &str) -> Result<KeyCode, KeyParseError> {
    if let Some((_, code)) = BARE_NAMES.iter().find(|(bare, _)| *bare == rest) {
        return Ok(*code);
    }
    if let Some(name) = rest.strip_prefix('<').and_then(|r| r.strip_suffix('>')) {
        return parse_name(name);
    }
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(KeyCode::Char(c)),
        (Some(_), Some(_)) | (None, _) => Err(KeyParseError::MultipleChars(rest.to_string())),
    }
}

/// The key a bracketed `name` names, ignoring case: `esc`, `F5`, `PageUp`,
/// or an Emacs spelling such as `next`.
fn parse_name(name: &str) -> Result<KeyCode, KeyParseError> {
    let lower = name.to_ascii_lowercase();
    let mut known_names = NAMED_KEYS.iter().chain(NAME_ALIASES.iter());
    if let Some((_, code)) = known_names.find(|(known, _)| *known == lower) {
        return Ok(*code);
    }
    lower
        .strip_prefix('f')
        .and_then(|digits| digits.parse::<u8>().ok())
        .filter(|n| (1..=MAX_FUNCTION_KEY).contains(n))
        .map(KeyCode::F)
        .ok_or_else(|| KeyParseError::UnknownName(name.to_string()))
}

/// The name a named key is written with. Only called for keys in
/// [`NAMED_KEYS`], so the lookup always finds one.
fn name_of(code: KeyCode) -> &'static str {
    NAMED_KEYS
        .iter()
        .find(|(_, named)| *named == code)
        .map_or("?", |(name, _)| name)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn key(token: &str) -> Key {
        parse_key(token).unwrap_or_else(|err| panic!("{token:?}: {err}"))
    }

    #[test]
    fn test_every_canonical_key_formats_back_to_itself() {
        for token in [
            "a",
            "G",
            "?",
            "-",
            "<",
            "C-x",
            "C-S-f",
            "M-?",
            "C--",
            "S-<tab>",
            "<space>",
            "C-<space>",
            "<esc>",
            "<cr>",
            "<bs>",
            "<del>",
            "<pagedown>",
            "C-M-S-s-<f24>",
        ] {
            assert_eq!(key(token).to_string(), token);
        }
    }

    #[test]
    fn test_modifiers_may_be_written_in_any_order() {
        assert_eq!(key("M-C-x"), key("C-M-x"));
        assert_eq!(key("s-S-C-<up>").to_string(), "C-S-s-<up>");
    }

    #[test]
    fn test_shift_is_normalized_the_way_key_new_does() {
        assert_eq!(key("S-g"), key("G"));
        assert_eq!(key("C-F").to_string(), "C-S-f");
        assert_eq!(key("M-S-?").to_string(), "M-?");
    }

    #[test]
    fn test_names_ignore_case() {
        assert_eq!(key("<ESC>"), key("<esc>"));
        assert_eq!(key("<PageUp>").code, KeyCode::PageUp);
        assert_eq!(key("<F5>").code, KeyCode::F(5));
    }

    #[test]
    fn test_malformed_keys_say_what_is_wrong() {
        assert_eq!(parse_key(""), Err(KeyParseError::Empty));
        assert_eq!(parse_key("C-"), Err(KeyParseError::MissingKey("C-".into())));
        assert_eq!(
            parse_key("ab"),
            Err(KeyParseError::MultipleChars("ab".into()))
        );
        assert_eq!(
            parse_key("<nope>"),
            Err(KeyParseError::UnknownName("nope".into()))
        );
        assert_eq!(
            parse_key("<f0>"),
            Err(KeyParseError::UnknownName("f0".into()))
        );
        assert_eq!(
            parse_key("<f25>"),
            Err(KeyParseError::UnknownName("f25".into()))
        );
    }

    #[test]
    fn test_emacs_bare_names_parse_to_the_canonical_keys() {
        for (alias, canonical) in [
            ("RET", "<cr>"),
            ("TAB", "<tab>"),
            ("SPC", "<space>"),
            ("ESC", "<esc>"),
            ("DEL", "<bs>"),
            ("C-RET", "C-<cr>"),
            ("M-DEL", "M-<bs>"),
            ("S-TAB", "S-<tab>"),
        ] {
            assert_eq!(key(alias), key(canonical), "{alias}");
            assert_eq!(key(alias).to_string(), canonical, "{alias}");
        }
    }

    #[test]
    fn test_emacs_bracketed_names_parse_to_the_canonical_keys() {
        for (alias, canonical) in [
            ("<next>", "<pagedown>"),
            ("<prior>", "<pageup>"),
            ("<return>", "<cr>"),
            ("<escape>", "<esc>"),
            ("<backspace>", "<bs>"),
            ("<delete>", "<del>"),
            ("C-<Next>", "C-<pagedown>"),
        ] {
            assert_eq!(key(alias).to_string(), canonical, "{alias}");
        }
    }

    #[test]
    fn test_bare_del_is_backspace_but_bracketed_del_is_delete() {
        assert_eq!(key("DEL").code, KeyCode::Backspace);
        assert_eq!(key("<del>").code, KeyCode::Delete);
    }

    #[test]
    fn test_only_whole_tokens_are_emacs_names() {
        assert_eq!(key("R"), Key::new(KeyCode::Char('R'), Mods::NONE));
        assert_eq!(key("S"), Key::new(KeyCode::Char('S'), Mods::NONE));
        assert_eq!(
            parse_key("ret"),
            Err(KeyParseError::MultipleChars("ret".into()))
        );
        assert_eq!(
            parse_key("RETX"),
            Err(KeyParseError::MultipleChars("RETX".into()))
        );
    }

    #[test]
    fn test_a_sequence_is_keys_separated_by_whitespace() {
        assert_eq!(
            parse_key_sequence("C-x  C-f"),
            Ok(vec![key("C-x"), key("C-f")])
        );
        assert_eq!(parse_key_sequence("   "), Err(KeyParseError::Empty));
        assert_eq!(
            parse_key_sequence("g gg"),
            Err(KeyParseError::MultipleChars("gg".into()))
        );
    }
}
