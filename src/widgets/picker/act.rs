//! [`PickerAct`]: what a picker key does. Misty resolves no key presses,
//! so a picker is told acts: an app with a keymap of its own binds them
//! there, starting from [`PickerAct::default_keys`], and a key it binds to
//! no act goes to the picker's query. [`PickerAct::for_default_key`] is
//! the act a key does when the app keeps the defaults.

use super::super::super::base::key::{Key, parse_key};

// ========================================================================
// Constants
// ========================================================================

/// Every act, in the order a keymap lists them.
const ALL: [PickerAct; 11] = [
    PickerAct::Accept,
    PickerAct::ActionMenu,
    PickerAct::TogglePanel,
    PickerAct::Save,
    PickerAct::Next,
    PickerAct::Previous,
    PickerAct::PageDown,
    PickerAct::PageUp,
    PickerAct::HistoryPrevious,
    PickerAct::HistoryNext,
    PickerAct::Close,
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a picker key does.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum PickerAct {
    /// Run the selected row's first action, or the answer typed.
    Accept,
    /// Show the selected row's actions.
    ActionMenu,
    /// Close the picker, or the action menu over it.
    Close,
    /// Bring back a later query.
    HistoryNext,
    /// Bring back an earlier query.
    HistoryPrevious,
    /// Select the next row.
    Next,
    /// Move the selection a page down.
    PageDown,
    /// Move the selection a page up.
    PageUp,
    /// Select the previous row.
    Previous,
    /// Hand over the rows that match, to keep.
    Save,
    /// Show or hide the side panel.
    TogglePanel,
}

// ========================================================================
// PickerAct Implementations
// ========================================================================

impl PickerAct {
    /// Every act, in the order a keymap lists them.
    pub fn all() -> &'static [PickerAct] {
        &ALL
    }

    /// The keys that do the act by default, in misty's key notation, the
    /// first the one a hint names.
    pub fn default_keys(self) -> &'static [&'static str] {
        match self {
            Self::Accept => &["RET"],
            Self::ActionMenu => &["<f4>", "C-RET"],
            Self::Close => &["ESC", "C-g"],
            Self::HistoryNext => &["M-n"],
            Self::HistoryPrevious => &["M-p"],
            Self::Next => &["C-n", "<down>"],
            Self::PageDown => &["<pagedown>"],
            Self::PageUp => &["<pageup>"],
            Self::Previous => &["C-p", "<up>"],
            Self::Save => &["<f3>", "C-S-s"],
            Self::TogglePanel => &["<f6>"],
        }
    }

    /// The act `key` does by default, if it does one.
    pub fn for_default_key(key: Key) -> Option<Self> {
        ALL.into_iter().find(|act| {
            act.default_keys()
                .iter()
                .any(|spec| parse_key(spec).is_ok_and(|bound| bound == key))
        })
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_default_key_parses_and_does_one_act() {
        let mut seen = Vec::new();
        for act in PickerAct::all() {
            for spec in act.default_keys() {
                let key = parse_key(spec).unwrap_or_else(|_| panic!("{spec} parses"));
                assert!(!seen.contains(&key), "{spec} does two acts");
                seen.push(key);
                assert_eq!(PickerAct::for_default_key(key), Some(*act), "{spec}");
            }
        }
        assert_eq!(
            PickerAct::for_default_key(parse_key("a").expect("a key")),
            None
        );
    }
}
