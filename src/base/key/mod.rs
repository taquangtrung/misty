//! Keys as misty knows them, independent of any window system and of any
//! keymap library: a [`KeyCode`] and the [`Mods`] held with it, normalized
//! so that one physical chord has exactly one [`Key`] value however the
//! window system reported it.
//!
//! Widgets match on these keys, and the window runtime turns the window
//! system's key presses into them. [`parse_key`] and [`parse_key_sequence`]
//! read the key notation (`C-x`, `G`, `S-<tab>`, `<esc>`, `g g`), and
//! `Display` writes it back, for command defaults, tests, and help;
//! [`key_label`] writes keys for people (`Ctrl+Shift+T`).
//!
//! The values are normalized as [`Key::new`] describes, so an app converts
//! them to its keymap library's keys without loss.
//!
//! A [`Key`] says what a key typed, so a shifted symbol drops Shift (`?`,
//! not Shift+/), and which character comes depends on the layout. A
//! [`PhysicalKey`] says where the key sits instead, every modifier held
//! with it, Shift included, for an app whose chords are written by key
//! place (`Ctrl+Shift+/`) whatever the layout types there; the runtime
//! reports it beside each key through
//! [`EventCx::physical_key`](crate::EventCx::physical_key).

mod label;
mod notation;

use std::ops::{BitOr, BitOrAssign};

pub use label::key_label;
#[cfg(any(test, feature = "testing"))]
pub(crate) use notation::folded_shift_place;
pub use notation::{KeyParseError, parse_key, parse_key_sequence};

// ========================================================================
// Data Structures
// ========================================================================

/// A key and the modifiers held with it, in normalized form. Build one with
/// [`Key::new`], which applies the normalization rules; two keys compare equal
/// exactly when they are the same chord.
///
/// # Notation
///
/// The key notation is how a key is written in keymaps, docs, and help:
/// [`parse_key`] and [`parse_key_sequence`] read it, and `Display` writes it.
///
/// A key is its modifiers, each a letter and a dash (`C-` Control, `M-` Alt,
/// `S-` Shift, `s-` Super), followed by either one character or a named key in
/// angle brackets: `x`, `G`, `C-x`, `C-S-f`, `M-?`, `S-<tab>`, `<esc>`,
/// `<f5>`. A sequence is keys separated by spaces: `C-x C-f`, `g g`.
///
/// Parsing also accepts the Emacs spellings, so keymaps written for Emacs
/// read as they are: the bare names `RET`, `TAB`, `SPC`, `ESC`, and `DEL`
/// (uppercase, as Emacs writes them, and with modifiers: `C-RET`, `S-TAB`),
/// and the bracketed names `<return>`, `<escape>`, `<backspace>`, `<delete>`,
/// `<next>` (Page Down), and `<prior>` (Page Up). Mind one asymmetry, inherited
/// from Emacs: bare `DEL` is Backspace, while bracketed `<del>` is Delete.
/// Formatting never writes these spellings; it always writes the canonical
/// form, so `RET` reads back as `<cr>`.
///
/// Parsing applies the normalization of [`Key::new`], so `S-g` and `G` are the
/// same key, and formatting writes each key in its one canonical form, with
/// modifiers in the order `C-M-S-s-`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Key {
    /// The key pressed.
    pub code: KeyCode,
    /// The modifiers held.
    pub mods: Mods,
}

/// A key, independent of the modifiers held with it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum KeyCode {
    /// Backspace, written `<bs>`.
    Backspace,
    /// A character, as typed: `a`, `G`, `?`, or `' '` for the space bar.
    Char(char),
    /// Delete, written `<del>`.
    Delete,
    /// The down arrow.
    Down,
    /// End.
    End,
    /// Enter, written `<cr>`.
    Enter,
    /// Escape, written `<esc>`.
    Esc,
    /// A function key, `F1` and up.
    F(u8),
    /// Home.
    Home,
    /// Insert.
    Insert,
    /// The left arrow.
    Left,
    /// Page Down.
    PageDown,
    /// Page Up.
    PageUp,
    /// The right arrow.
    Right,
    /// Tab.
    Tab,
    /// The up arrow.
    Up,
}

/// A key by where it sits on the keyboard, named by what a US layout types
/// there without Shift (`/` for the key that types `?` with Shift, a
/// lowercase letter, `<tab>`), and every modifier held with it, Shift
/// included: Ctrl+Shift+/ is the code `Char('/')` with Ctrl and Shift,
/// on any layout and however the window system reported the character.
/// Build one with [`PhysicalKey::new`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PhysicalKey {
    /// The key at that place on a US layout, as it types without Shift.
    pub code: KeyCode,
    /// The modifiers held, Shift included.
    pub mods: Mods,
}

/// The modifiers held with a key, as a set.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Mods(u8);

// ========================================================================
// Key Implementations
// ========================================================================

impl Key {
    /// The key `code` with `mods` held, normalized so that every way a toolkit
    /// may report one chord yields the same key:
    ///
    /// - A letter with Shift and no other modifier is the uppercase letter, with
    ///   Shift dropped: Shift+g is `G`, as Vim writes it.
    /// - A letter with Ctrl, Alt, or Super keeps Shift as a modifier on the
    ///   lowercase letter: Ctrl+Shift+f is `C-S-f`, whether the toolkit
    ///   reported `f` with Shift or `F`.
    /// - Any other character drops Shift, since the character already says
    ///   what Shift did: Alt+Shift+/ is `M-?`.
    /// - Every other key keeps its modifiers as given: Shift+Tab is `S-<tab>`.
    ///
    /// Only ASCII letters change case; any other character is taken as typed.
    pub fn new(code: KeyCode, mods: Mods) -> Self {
        if let KeyCode::Char(c) = code {
            return Self::normalized_char(c, mods);
        }
        Self { code, mods }
    }

    /// The character `c` with `mods` held, normalized as [`Key::new`] describes.
    fn normalized_char(c: char, mods: Mods) -> Self {
        let unshifted = mods.without(Mods::SHIFT);
        if !c.is_ascii_alphabetic() {
            return Self {
                code: KeyCode::Char(c),
                mods: unshifted,
            };
        }
        let shifted = mods.contains(Mods::SHIFT) || c.is_ascii_uppercase();
        let chorded = mods.intersects(Mods::CTRL | Mods::ALT | Mods::SUPER);
        match (chorded, shifted) {
            (true, true) => Self {
                code: KeyCode::Char(c.to_ascii_lowercase()),
                mods: unshifted | Mods::SHIFT,
            },
            (false, true) => Self {
                code: KeyCode::Char(c.to_ascii_uppercase()),
                mods: unshifted,
            },
            (_, false) => Self {
                code: KeyCode::Char(c),
                mods: unshifted,
            },
        }
    }
}

// ========================================================================
// PhysicalKey Implementations
// ========================================================================

impl PhysicalKey {
    /// The key at the place `code` names, with `mods` held as given. An
    /// ASCII letter is taken in lowercase, the way a key's place is named;
    /// any other code is kept as given.
    pub fn new(code: KeyCode, mods: Mods) -> Self {
        let code = match code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        Self { code, mods }
    }
}

// ========================================================================
// Mods Implementations
// ========================================================================

impl Mods {
    /// Alt, also called Meta, written `M-`.
    pub const ALT: Self = Self(1 << 1);
    /// Control, written `C-`.
    pub const CTRL: Self = Self(1 << 0);
    /// No modifier.
    pub const NONE: Self = Self(0);
    /// The platform's command modifier: [`Mods::SUPER`] (Cmd) on macOS,
    /// [`Mods::CTRL`] elsewhere. Copy, paste, select all, and zooming with
    /// the wheel take it, so `Key::new(KeyCode::Char('c'), Mods::PRIMARY)`
    /// is copy on every platform.
    pub const PRIMARY: Self = if cfg!(target_os = "macos") {
        Self::SUPER
    } else {
        Self::CTRL
    };
    /// Shift, written `S-`.
    pub const SHIFT: Self = Self(1 << 2);
    /// Super (the Windows or Command key), written `s-`.
    pub const SUPER: Self = Self(1 << 3);

    /// Whether every modifier in `other` is in this set.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether this set and `other` share any modifier.
    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// This set with the modifiers in `other` taken out.
    pub fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl BitOr for Mods {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn char_key(c: char, mods: Mods) -> Key {
        Key::new(KeyCode::Char(c), mods)
    }

    #[test]
    fn test_shift_on_a_plain_letter_is_the_uppercase_letter() {
        assert_eq!(char_key('g', Mods::SHIFT), char_key('G', Mods::NONE));
        assert_eq!(char_key('g', Mods::SHIFT).mods, Mods::NONE);
    }

    #[test]
    fn test_a_chorded_letter_keeps_shift_however_the_toolkit_reported_it() {
        let from_flag = char_key('f', Mods::CTRL | Mods::SHIFT);
        let from_capital = char_key('F', Mods::CTRL);
        assert_eq!(from_flag, from_capital);
        assert_eq!(from_flag.code, KeyCode::Char('f'));
        assert!(from_flag.mods.contains(Mods::SHIFT));
        assert_ne!(from_flag, char_key('f', Mods::CTRL), "C-S-f is not C-f");
    }

    #[test]
    fn test_a_shifted_symbol_drops_shift_even_in_a_chord() {
        assert_eq!(
            char_key('?', Mods::ALT | Mods::SHIFT),
            char_key('?', Mods::ALT)
        );
        assert_eq!(char_key('?', Mods::SHIFT).mods, Mods::NONE);
    }

    #[test]
    fn test_a_non_character_key_keeps_shift() {
        assert!(
            Key::new(KeyCode::Tab, Mods::SHIFT)
                .mods
                .contains(Mods::SHIFT)
        );
    }

    #[test]
    fn test_non_ascii_letters_keep_their_case() {
        assert_eq!(char_key('é', Mods::SHIFT).code, KeyCode::Char('é'));
    }
}
