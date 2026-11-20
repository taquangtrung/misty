//! Keys for people: [`key_label`] writes keys the way menus and the palette
//! show them to people (`Ctrl+Shift+T`), as against the key notation
//! keymaps are written in (`C-S-t`).

use super::super::strings::Strings;
use super::{Key, KeyCode, Mods};

// ========================================================================
// Constants
// ========================================================================

/// What follows each modifier's name in a label, as in `Ctrl+S`.
const MODIFIER_SEPARATOR: char = '+';

/// What separates the keys of a sequence in a label.
const SEQUENCE_SEPARATOR: &str = " ";

// ========================================================================
// Functions
// ========================================================================

/// `keys` as people read them in a menu or a palette: modifiers spelled
/// out in the order Ctrl, Alt, Shift, Super, named in `strings`' words
/// (pass the theme's, `&theme.strings`), a letter held with Ctrl, Alt, or
/// Super in uppercase, keys of a sequence separated by spaces:
/// `Ctrl+Shift+T`, `Ctrl+X Ctrl+S`, `Alt+X`, `F2`, `g g`.
pub fn key_label(keys: &[Key], strings: &Strings) -> String {
    keys.iter()
        .map(|key| chord_label(key, strings))
        .collect::<Vec<_>>()
        .join(SEQUENCE_SEPARATOR)
}

/// One key as [`key_label`] writes it.
fn chord_label(key: &Key, strings: &Strings) -> String {
    let modifiers = [
        (Mods::CTRL, &strings.key_ctrl),
        (Mods::ALT, &strings.key_alt),
        (Mods::SHIFT, &strings.key_shift),
        (Mods::SUPER, &strings.key_super),
    ];
    let mut label = String::new();
    for (modifier, name) in modifiers {
        if key.mods.contains(modifier) {
            label.push_str(name);
            label.push(MODIFIER_SEPARATOR);
        }
    }
    let chorded = key.mods.intersects(Mods::CTRL | Mods::ALT | Mods::SUPER);
    let name = match key.code {
        KeyCode::Backspace => "Backspace",
        KeyCode::Char(' ') => "Space",
        KeyCode::Char(c) if chorded => {
            label.extend(c.to_uppercase());
            return label;
        }
        KeyCode::Char(c) => {
            label.push(c);
            return label;
        }
        KeyCode::Delete => "Delete",
        KeyCode::Down => "Down",
        KeyCode::End => "End",
        KeyCode::Enter => "Enter",
        KeyCode::Esc => "Esc",
        KeyCode::F(number) => {
            label.push_str(&format!("F{number}"));
            return label;
        }
        KeyCode::Home => "Home",
        KeyCode::Insert => "Insert",
        KeyCode::Left => "Left",
        KeyCode::PageDown => "PageDown",
        KeyCode::PageUp => "PageUp",
        KeyCode::Right => "Right",
        KeyCode::Tab => "Tab",
        KeyCode::Up => "Up",
    };
    label.push_str(name);
    label
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::key::parse_key_sequence;

    #[test]
    fn test_labels_spell_out_modifiers_and_capitalize_only_chorded_letters() {
        let english = Strings::english();
        let label_of = |spec: &str| {
            key_label(
                &parse_key_sequence(spec).expect("a valid sequence"),
                &english,
            )
        };
        assert_eq!(label_of("C-S-<tab>"), "Ctrl+Shift+Tab");
        assert_eq!(label_of("C-x C-s"), "Ctrl+X Ctrl+S");
        assert_eq!(label_of("M-?"), "Alt+?");
        assert_eq!(label_of("s-M-C-t"), "Ctrl+Alt+Super+T");
        assert_eq!(label_of("g g"), "g g", "unchorded keys stay as typed");
        assert_eq!(label_of("G"), "G");
        assert_eq!(label_of("<f2> <space>"), "F2 Space");
        let german = Strings {
            key_ctrl: "Strg".into(),
            key_shift: "Umschalt".into(),
            ..Strings::english()
        };
        let keys = parse_key_sequence("C-S-t").expect("a valid sequence");
        assert_eq!(
            key_label(&keys, &german),
            "Strg+Umschalt+T",
            "named in the strings"
        );
    }
}
