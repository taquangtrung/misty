//! Window-system keys: the window runtime turns winit's key presses and
//! releases into misty [`Key`]s here, normalized by [`Key::new`], so however the
//! window system reported a chord there is one key value for it: on Linux a
//! shifted symbol can arrive without Shift in the modifiers (`?` for
//! Shift+/), and an uppercase letter with Ctrl but no Shift; both come out
//! as the same key the reported-with-Shift event gives. A letter or digit
//! key the layout gives no character for (as Windows does for some Ctrl+Alt
//! chords) is named by where it sits: the letter or digit printed there on
//! a US layout. Dead keys and keys with no misty name (media keys, lone
//! modifiers) are no key. Where each key sits is read apart, as a
//! [`PhysicalKey`]: what a US layout types there without Shift, every
//! modifier kept.

use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{self, KeyCode as Physical, ModifiersState, NamedKey};

use super::super::base::key::{Key, KeyCode, Mods, PhysicalKey};
use super::event::Input;

// ========================================================================
// Functions
// ========================================================================

/// What `event` means with `mods` held: the press (or repeat) of a key,
/// [`Input::Key`], or its release, [`Input::KeyReleased`], normalized by
/// [`Key::new`]; `None` for a dead key or a key misty has no name for.
/// Composition is not seen here: the input reader tracks it.
pub(crate) fn key_input(event: &KeyEvent, mods: Mods) -> Option<Input> {
    read_key(event.state, &event.logical_key, event.physical_key, mods)
}

/// What a key event of `state`, naming `logical` at `physical`, means with
/// `mods` held, as [`key_input`] says.
fn read_key(
    state: ElementState,
    logical: &keyboard::Key,
    physical: keyboard::PhysicalKey,
    mods: Mods,
) -> Option<Input> {
    let key = key_of(logical, physical, mods)?;
    Some(match state {
        ElementState::Pressed => Input::Key(key),
        ElementState::Released => Input::KeyReleased(key),
    })
}

/// The key `logical` names with `mods` held, or, when the layout names
/// none, the letter or digit key at `physical`.
fn key_of(logical: &keyboard::Key, physical: keyboard::PhysicalKey, mods: Mods) -> Option<Key> {
    let code = match logical {
        keyboard::Key::Character(text) => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                (Some(_), Some(_)) | (None, _) => return None,
            }
        }
        keyboard::Key::Named(named) => code_of(*named)?,
        keyboard::Key::Unidentified(_) => KeyCode::Char(char_at(physical)?),
        keyboard::Key::Dead(_) => return None,
    };
    Some(Key::new(code, mods))
}

/// Where the key of `event` sits, with `mods` held, Shift included: the
/// key a US layout has there, as it types without Shift; `None` for a
/// place misty has no name for.
pub(crate) fn physical_key(event: &KeyEvent, mods: Mods) -> Option<PhysicalKey> {
    physical_of(event.physical_key, mods)
}

/// The place `physical`, with `mods` held, as [`physical_key`] names it.
fn physical_of(physical: keyboard::PhysicalKey, mods: Mods) -> Option<PhysicalKey> {
    if let Some(c) = char_at(physical).or_else(|| symbol_at(physical)) {
        return Some(PhysicalKey::new(KeyCode::Char(c), mods));
    }
    let keyboard::PhysicalKey::Code(code) = physical else {
        return None;
    };
    let code = match code {
        Physical::ArrowDown => KeyCode::Down,
        Physical::ArrowLeft => KeyCode::Left,
        Physical::ArrowRight => KeyCode::Right,
        Physical::ArrowUp => KeyCode::Up,
        Physical::Backspace => KeyCode::Backspace,
        Physical::Delete => KeyCode::Delete,
        Physical::End => KeyCode::End,
        Physical::Enter | Physical::NumpadEnter => KeyCode::Enter,
        Physical::Escape => KeyCode::Esc,
        Physical::Home => KeyCode::Home,
        Physical::Insert => KeyCode::Insert,
        Physical::PageDown => KeyCode::PageDown,
        Physical::PageUp => KeyCode::PageUp,
        Physical::Tab => KeyCode::Tab,
        function => KeyCode::F(function_number(function)?),
    };
    Some(PhysicalKey::new(code, mods))
}

/// The number of the function key `code`, F1 to F24; `None` for any other
/// key.
fn function_number(code: Physical) -> Option<u8> {
    const KEYS: [Physical; 24] = [
        Physical::F1,
        Physical::F2,
        Physical::F3,
        Physical::F4,
        Physical::F5,
        Physical::F6,
        Physical::F7,
        Physical::F8,
        Physical::F9,
        Physical::F10,
        Physical::F11,
        Physical::F12,
        Physical::F13,
        Physical::F14,
        Physical::F15,
        Physical::F16,
        Physical::F17,
        Physical::F18,
        Physical::F19,
        Physical::F20,
        Physical::F21,
        Physical::F22,
        Physical::F23,
        Physical::F24,
    ];
    let index = KEYS.iter().position(|key| *key == code)?;
    u8::try_from(index + 1).ok()
}

/// The symbol or space a US layout types without Shift on the key at
/// `physical`; `None` for any other key.
fn symbol_at(physical: keyboard::PhysicalKey) -> Option<char> {
    let keyboard::PhysicalKey::Code(code) = physical else {
        return None;
    };
    let c = match code {
        Physical::Backquote => '`',
        Physical::Backslash => '\\',
        Physical::BracketLeft => '[',
        Physical::BracketRight => ']',
        Physical::Comma => ',',
        Physical::Equal => '=',
        Physical::Minus => '-',
        Physical::Period => '.',
        Physical::Quote => '\'',
        Physical::Semicolon => ';',
        Physical::Slash => '/',
        Physical::Space => ' ',
        _ => return None,
    };
    Some(c)
}

/// The letter or digit printed on the key at `physical` on a US layout, in
/// lowercase; `None` for any other key.
fn char_at(physical: keyboard::PhysicalKey) -> Option<char> {
    let keyboard::PhysicalKey::Code(code) = physical else {
        return None;
    };
    let c = match code {
        Physical::Digit0 => '0',
        Physical::Digit1 => '1',
        Physical::Digit2 => '2',
        Physical::Digit3 => '3',
        Physical::Digit4 => '4',
        Physical::Digit5 => '5',
        Physical::Digit6 => '6',
        Physical::Digit7 => '7',
        Physical::Digit8 => '8',
        Physical::Digit9 => '9',
        Physical::KeyA => 'a',
        Physical::KeyB => 'b',
        Physical::KeyC => 'c',
        Physical::KeyD => 'd',
        Physical::KeyE => 'e',
        Physical::KeyF => 'f',
        Physical::KeyG => 'g',
        Physical::KeyH => 'h',
        Physical::KeyI => 'i',
        Physical::KeyJ => 'j',
        Physical::KeyK => 'k',
        Physical::KeyL => 'l',
        Physical::KeyM => 'm',
        Physical::KeyN => 'n',
        Physical::KeyO => 'o',
        Physical::KeyP => 'p',
        Physical::KeyQ => 'q',
        Physical::KeyR => 'r',
        Physical::KeyS => 's',
        Physical::KeyT => 't',
        Physical::KeyU => 'u',
        Physical::KeyV => 'v',
        Physical::KeyW => 'w',
        Physical::KeyX => 'x',
        Physical::KeyY => 'y',
        Physical::KeyZ => 'z',
        _ => return None,
    };
    Some(c)
}

/// The key code of the named key `named`, if misty names it.
fn code_of(named: NamedKey) -> Option<KeyCode> {
    let code = match named {
        NamedKey::ArrowDown => KeyCode::Down,
        NamedKey::ArrowLeft => KeyCode::Left,
        NamedKey::ArrowRight => KeyCode::Right,
        NamedKey::ArrowUp => KeyCode::Up,
        NamedKey::Backspace => KeyCode::Backspace,
        NamedKey::Delete => KeyCode::Delete,
        NamedKey::End => KeyCode::End,
        NamedKey::Enter => KeyCode::Enter,
        NamedKey::Escape => KeyCode::Esc,
        NamedKey::F1 => KeyCode::F(1),
        NamedKey::F2 => KeyCode::F(2),
        NamedKey::F3 => KeyCode::F(3),
        NamedKey::F4 => KeyCode::F(4),
        NamedKey::F5 => KeyCode::F(5),
        NamedKey::F6 => KeyCode::F(6),
        NamedKey::F7 => KeyCode::F(7),
        NamedKey::F8 => KeyCode::F(8),
        NamedKey::F9 => KeyCode::F(9),
        NamedKey::F10 => KeyCode::F(10),
        NamedKey::F11 => KeyCode::F(11),
        NamedKey::F12 => KeyCode::F(12),
        NamedKey::F13 => KeyCode::F(13),
        NamedKey::F14 => KeyCode::F(14),
        NamedKey::F15 => KeyCode::F(15),
        NamedKey::F16 => KeyCode::F(16),
        NamedKey::F17 => KeyCode::F(17),
        NamedKey::F18 => KeyCode::F(18),
        NamedKey::F19 => KeyCode::F(19),
        NamedKey::F20 => KeyCode::F(20),
        NamedKey::F21 => KeyCode::F(21),
        NamedKey::F22 => KeyCode::F(22),
        NamedKey::F23 => KeyCode::F(23),
        NamedKey::F24 => KeyCode::F(24),
        NamedKey::Home => KeyCode::Home,
        NamedKey::Insert => KeyCode::Insert,
        NamedKey::PageDown => KeyCode::PageDown,
        NamedKey::PageUp => KeyCode::PageUp,
        NamedKey::Space => KeyCode::Char(' '),
        NamedKey::Tab => KeyCode::Tab,
        // `NamedKey` is non-exhaustive and mostly media, browser, and lone
        // modifier keys, none of which misty names.
        _ => return None,
    };
    Some(code)
}

/// The misty modifiers of `modifiers`.
pub(crate) fn mods_of(modifiers: ModifiersState) -> Mods {
    let mut mods = Mods::NONE;
    for (held, modifier) in [
        (modifiers.alt_key(), Mods::ALT),
        (modifiers.control_key(), Mods::CTRL),
        (modifiers.shift_key(), Mods::SHIFT),
        (modifiers.super_key(), Mods::SUPER),
    ] {
        if held {
            mods |= modifier;
        }
    }
    mods
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winit::keyboard::{NativeKey, NativeKeyCode, SmolStr};

    use super::*;
    use crate::base::key::parse_key;

    const NONE: Mods = Mods::NONE;

    fn character(text: &str) -> keyboard::Key {
        keyboard::Key::Character(SmolStr::new(text))
    }

    /// No key at all: what a physical key is when only the logical one
    /// matters.
    const NOWHERE: keyboard::PhysicalKey =
        keyboard::PhysicalKey::Unidentified(NativeKeyCode::Unidentified);

    fn key(spec: &str) -> Option<Key> {
        Some(parse_key(spec).expect("a valid key"))
    }

    #[test]
    fn test_a_shifted_symbol_is_one_key_with_or_without_shift_reported() {
        let question = character("?");
        assert_eq!(key_of(&question, NOWHERE, NONE), key("?"));
        assert_eq!(key_of(&question, NOWHERE, Mods::SHIFT), key("?"));
        let alt = Mods::ALT;
        assert_eq!(key_of(&question, NOWHERE, alt | Mods::SHIFT), key("M-?"));
    }

    #[test]
    fn test_an_uppercase_letter_means_shift_even_when_shift_is_not_reported() {
        assert_eq!(key_of(&character("G"), NOWHERE, NONE), key("G"));
        assert_eq!(key_of(&character("g"), NOWHERE, Mods::SHIFT), key("G"));
        let ctrl = Mods::CTRL;
        assert_eq!(key_of(&character("F"), NOWHERE, ctrl), key("C-S-f"));
        assert_eq!(
            key_of(&character("f"), NOWHERE, ctrl | Mods::SHIFT),
            key("C-S-f")
        );
        assert_eq!(key_of(&character("f"), NOWHERE, ctrl), key("C-f"));
    }

    #[test]
    fn test_named_keys_keep_their_modifiers_and_space_is_a_character() {
        let tab = keyboard::Key::Named(NamedKey::Tab);
        let ctrl_shift = Mods::CTRL | Mods::SHIFT;
        assert_eq!(key_of(&tab, NOWHERE, ctrl_shift), key("C-S-<tab>"));
        let space = keyboard::Key::Named(NamedKey::Space);
        assert_eq!(key_of(&space, NOWHERE, Mods::CTRL), key("C-<space>"));
        assert_eq!(
            key_of(&keyboard::Key::Named(NamedKey::F13), NOWHERE, NONE),
            key("<f13>")
        );
        assert_eq!(
            key_of(
                &keyboard::Key::Named(NamedKey::Escape),
                NOWHERE,
                Mods::SUPER
            ),
            key("s-<esc>")
        );
    }

    #[test]
    fn test_keys_misty_cannot_name_are_no_key() {
        let unnamed = [
            keyboard::Key::Dead(Some('`')),
            keyboard::Key::Unidentified(NativeKey::Unidentified),
            keyboard::Key::Named(NamedKey::Shift),
            keyboard::Key::Named(NamedKey::AudioVolumeUp),
            character("ab"),
            character(""),
        ];
        for logical in unnamed {
            assert_eq!(key_of(&logical, NOWHERE, NONE), None, "{logical:?}");
        }
    }

    #[test]
    fn test_a_physical_key_keeps_shift_on_symbols_where_the_key_drops_it() {
        let ctrl_shift = Mods::CTRL | Mods::SHIFT;
        let slash = keyboard::PhysicalKey::Code(Physical::Slash);
        assert_eq!(key_of(&character("?"), slash, ctrl_shift), key("C-?"));
        assert_eq!(
            physical_of(slash, ctrl_shift),
            Some(PhysicalKey::new(KeyCode::Char('/'), ctrl_shift)),
            "the same chord by place, Shift kept"
        );
        let at_a = keyboard::PhysicalKey::Code(Physical::KeyA);
        assert_eq!(
            physical_of(at_a, Mods::SHIFT),
            Some(PhysicalKey::new(KeyCode::Char('a'), Mods::SHIFT))
        );
        let f12 = keyboard::PhysicalKey::Code(Physical::F12);
        assert_eq!(
            physical_of(f12, NONE).map(|place| place.code),
            Some(KeyCode::F(12))
        );
        let numpad = keyboard::PhysicalKey::Code(Physical::Numpad5);
        assert_eq!(physical_of(numpad, NONE), None);
        assert_eq!(physical_of(NOWHERE, NONE), None);
    }

    #[test]
    fn test_a_release_is_its_key_released_and_an_unnamed_letter_is_named_by_its_place() {
        let g = character("g");
        let pressed = read_key(ElementState::Pressed, &g, NOWHERE, Mods::CTRL);
        let released = read_key(ElementState::Released, &g, NOWHERE, Mods::CTRL);
        assert_eq!(pressed, key("C-g").map(Input::Key));
        assert_eq!(released, key("C-g").map(Input::KeyReleased));
        let unnamed = keyboard::Key::Unidentified(NativeKey::Unidentified);
        let at_z = keyboard::PhysicalKey::Code(Physical::KeyZ);
        assert_eq!(
            key_of(&unnamed, at_z, Mods::CTRL | Mods::ALT),
            key("C-M-z"),
            "the key a layout gave no character, by where it sits"
        );
        let at_seven = keyboard::PhysicalKey::Code(Physical::Digit7);
        assert_eq!(key_of(&unnamed, at_seven, Mods::ALT), key("M-7"));
        let at_shift = keyboard::PhysicalKey::Code(Physical::ShiftLeft);
        assert_eq!(key_of(&unnamed, at_shift, NONE), None);
    }
}
