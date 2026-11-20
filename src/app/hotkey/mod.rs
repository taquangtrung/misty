//! [`GlobalHotkeys`]: keys that reach the app while another app has the
//! focus, behind the `global-hotkey` feature, such as the key that opens
//! a dictionary's lookup popup over whatever the user reads. Each press
//! comes back to [`Application::message`](crate::Application::message)
//! as a message the app makes from the hotkey's id.
//!
//! Where it works: X11, Windows, and macOS grab the keys themselves
//! (the `native` module, through the `global-hotkey` crate). Wayland lets
//! no app grab keys; there the desktop portal's GlobalShortcuts interface
//! binds them (the `portal` module), which needs a portal backend that
//! offers it (KDE Plasma, GNOME 48 or later, Hyprland) and may ask the
//! user to confirm or change the keys first. The portal names the app by
//! its desktop file, so a Wayland app should ship one.

mod native;
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
mod portal;

use std::collections::HashSet;
use std::sync::Arc;

use thiserror::Error;

use super::super::base::key::{Key, KeyCode, Mods};
use super::application::AppContext;

// ========================================================================
// Constants
// ========================================================================

/// The highest function key a hotkey may use.
const MAX_FUNCTION_KEY: u8 = 24;

/// The keys a US keyboard types punctuation with: the character without
/// Shift, the character with Shift, the key's W3C code name, and its X
/// keysym name.
const PUNCTUATION: [(char, char, &str, &str); 11] = [
    ('`', '~', "Backquote", "grave"),
    ('-', '_', "Minus", "minus"),
    ('=', '+', "Equal", "equal"),
    ('[', '{', "BracketLeft", "bracketleft"),
    (']', '}', "BracketRight", "bracketright"),
    ('\\', '|', "Backslash", "backslash"),
    (';', ':', "Semicolon", "semicolon"),
    ('\'', '"', "Quote", "apostrophe"),
    (',', '<', "Comma", "comma"),
    ('.', '>', "Period", "period"),
    ('/', '?', "Slash", "slash"),
];

/// What a US keyboard types with Shift and each digit, from 0 to 9.
const SHIFTED_DIGITS: [char; 10] = [')', '!', '@', '#', '$', '%', '^', '&', '*', '('];

// ========================================================================
// Data Structures
// ========================================================================

/// Global hotkeys an app registered, delivering presses while this value
/// lives; dropping it lets the keys go. Make it in a callback (such as
/// [`Application::init`](crate::Application::init)), on the main thread
/// as Windows and macOS require, and keep it in the app.
///
/// ```no_run
/// use misty::{AppContext, GlobalHotkeys, Hotkey, HotkeyEvent, parse_key};
///
/// enum Message {
///     Hotkey(HotkeyEvent),
/// }
///
/// fn register(cx: &AppContext<'_, Message>) -> Option<GlobalHotkeys> {
///     let key = parse_key("C-M-d").expect("valid notation");
///     let lookup = Hotkey::new("lookup", "Look up the selected word", key);
///     GlobalHotkeys::register(cx, vec![lookup], Message::Hotkey)
///         .map_err(|error| eprintln!("{error}"))
///         .ok()
/// }
/// ```
#[derive(Debug)]
pub struct GlobalHotkeys {
    /// What grabs the keys on this system, held only to be dropped.
    _backend: Backend,
}

/// One global hotkey to register: the key and what the app calls it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Hotkey {
    /// What the key does, in words the user reads where the system lists
    /// an app's hotkeys (the Wayland portal's settings).
    pub description: String,
    /// The app's name for the hotkey, handed back with each press.
    pub id: String,
    /// The key, such as `parse_key("C-M-d")`. Letters, digits, the
    /// punctuation of a US keyboard, the space bar, F1 to F24, and the
    /// named keys may be used, with any modifiers.
    pub key: Key,
}

/// What [`GlobalHotkeys`] tells the app.
#[derive(Debug)]
#[non_exhaustive]
pub enum HotkeyEvent {
    /// The hotkeys could not be bound after
    /// [`GlobalHotkeys::register`] returned: on Wayland, where the portal
    /// answers later, the portal refused or the user declined.
    Failed(HotkeyError),
    /// The hotkey with this id was pressed.
    Pressed(String),
}

/// Why global hotkeys could not be registered.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HotkeyError {
    /// The user declined the hotkeys in the system's dialog.
    #[error(
        "the user declined the global hotkeys: offer them again only when the user asks, \
         such as from a setting"
    )]
    Declined,
    /// Two hotkeys share this id.
    #[error("two global hotkeys share the id `{0}`: give each hotkey its own id")]
    DuplicateId(String),
    /// The desktop portal could not bind the hotkeys, for this reason.
    #[error(
        "the desktop portal could not bind the global hotkeys ({0}): on Wayland they need \
         xdg-desktop-portal with a GlobalShortcuts backend (KDE Plasma, GNOME 48 or later, \
         Hyprland) and a desktop file for the app"
    )]
    Portal(String),
    /// Another app holds this key already.
    #[error("`{0}` is already a global hotkey of another app: choose another key")]
    Taken(Key),
    /// The system's hotkey service could not be reached, for this reason.
    #[error(
        "global hotkeys are unavailable ({0}): on Linux they need an X11 session, or the \
         desktop portal on Wayland; register them from an app callback on the main thread"
    )]
    Unavailable(String),
    /// This key cannot be a global hotkey.
    #[error(
        "`{0}` cannot be a global hotkey: use a letter, a digit, US keyboard punctuation, \
         the space bar, F1 to F24, or a named key such as `<home>`, with any modifiers"
    )]
    UnsupportedKey(Key),
}

/// A key as it lies on a US keyboard, which is how hotkey services name
/// keys: the key itself (a character as typed without Shift) and the
/// modifiers, Shift among them when the character needs it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Chord {
    /// The key, its character unshifted.
    code: KeyCode,
    /// The modifiers held.
    mods: Mods,
}

/// A hotkey ready for a backend: its id, description, key, and chord.
#[derive(Clone, Debug)]
struct Binding {
    /// The chord the key is on.
    chord: Chord,
    /// What the key does, which only the portal shows.
    #[cfg_attr(
        not(all(
            unix,
            not(any(target_os = "macos", target_os = "ios", target_os = "android"))
        )),
        allow(dead_code)
    )]
    description: String,
    /// The app's name for it.
    id: String,
    /// The key as the app gave it, for errors.
    key: Key,
}

/// Hands a backend's events to the app.
type Sender = Arc<dyn Fn(HotkeyEvent) + Send + Sync>;

/// What grabs the keys, held for its `Drop`, which lets them go.
#[derive(Debug)]
#[expect(dead_code, reason = "held only to be dropped with the hotkeys")]
enum Backend {
    /// The `global-hotkey` crate: X11, Windows, macOS.
    Native(native::Native),
    /// The desktop portal, on Wayland.
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    Portal(portal::Portal),
}

// ========================================================================
// GlobalHotkeys Implementations
// ========================================================================

impl GlobalHotkeys {
    /// Registers `hotkeys` for as long as the value returned lives, each
    /// press reaching [`Application::message`](crate::Application::message)
    /// as `message(HotkeyEvent::Pressed(id))`. Refused when a key cannot
    /// be a hotkey, two share an id, another app holds a key, or the
    /// system has no hotkey service; on Wayland the portal answers later,
    /// and a refusal then comes as [`HotkeyEvent::Failed`].
    pub fn register<M: Send + 'static>(
        cx: &AppContext<'_, M>,
        hotkeys: Vec<Hotkey>,
        message: impl Fn(HotkeyEvent) -> M + Send + Sync + 'static,
    ) -> Result<Self, HotkeyError> {
        let bindings = bindings(hotkeys)?;
        let waker = cx.waker();
        let send: Sender = Arc::new(move |event| {
            // An app that has quit wants no more presses.
            let _ = waker.send(message(event));
        });
        #[cfg(all(
            unix,
            not(any(target_os = "macos", target_os = "ios", target_os = "android"))
        ))]
        if wayland_session() {
            return Ok(Self {
                _backend: Backend::Portal(portal::Portal::bind(bindings, send)?),
            });
        }
        Ok(Self {
            _backend: Backend::Native(native::Native::register(&bindings, send)?),
        })
    }
}

// ========================================================================
// Hotkey Implementations
// ========================================================================

impl Hotkey {
    /// The hotkey `key`, called `id` by the app and described to the user
    /// as `description`.
    pub fn new(id: impl Into<String>, description: impl Into<String>, key: Key) -> Self {
        Self {
            description: description.into(),
            id: id.into(),
            key,
        }
    }
}

// ========================================================================
// Chord Implementations
// ========================================================================

impl Chord {
    /// The chord `key` is typed with on a US keyboard, or why it cannot be
    /// a hotkey.
    fn of(key: Key) -> Result<Self, HotkeyError> {
        let unsupported = || HotkeyError::UnsupportedKey(key);
        let shifted = key.mods | Mods::SHIFT;
        let (code, mods) = match key.code {
            KeyCode::Char(c) if c.is_ascii_uppercase() => {
                (KeyCode::Char(c.to_ascii_lowercase()), shifted)
            }
            KeyCode::Char(c) if c.is_ascii_lowercase() || c.is_ascii_digit() || c == ' ' => {
                (key.code, key.mods)
            }
            KeyCode::Char(c) => {
                if let Some(digit) = SHIFTED_DIGITS.iter().position(|&symbol| symbol == c) {
                    let digit = char::from_digit(digit as u32, 10).ok_or_else(unsupported)?;
                    (KeyCode::Char(digit), shifted)
                } else if let Some(&(plain, ..)) =
                    PUNCTUATION.iter().find(|&&(plain, _, _, _)| plain == c)
                {
                    (KeyCode::Char(plain), key.mods)
                } else if let Some(&(plain, ..)) =
                    PUNCTUATION.iter().find(|&&(_, symbol, _, _)| symbol == c)
                {
                    (KeyCode::Char(plain), shifted)
                } else {
                    return Err(unsupported());
                }
            }
            KeyCode::F(number) if (1..=MAX_FUNCTION_KEY).contains(&number) => (key.code, key.mods),
            KeyCode::F(_) => return Err(unsupported()),
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
            | KeyCode::Up => (key.code, key.mods),
        };
        Ok(Self { code, mods })
    }

    /// The key's W3C code name (`KeyA`, `Digit1`, `Slash`, `F5`,
    /// `ArrowUp`), as the `global-hotkey` crate names keys.
    fn code_name(self) -> String {
        match self.code {
            KeyCode::Char(' ') => "Space".to_owned(),
            KeyCode::Char(c) if c.is_ascii_digit() => format!("Digit{c}"),
            KeyCode::Char(c) => match punctuation(c) {
                Some((_, _, name, _)) => name.to_owned(),
                None => format!("Key{}", c.to_ascii_uppercase()),
            },
            KeyCode::F(number) => format!("F{number}"),
            KeyCode::Backspace => "Backspace".to_owned(),
            KeyCode::Delete => "Delete".to_owned(),
            KeyCode::Down => "ArrowDown".to_owned(),
            KeyCode::End => "End".to_owned(),
            KeyCode::Enter => "Enter".to_owned(),
            KeyCode::Esc => "Escape".to_owned(),
            KeyCode::Home => "Home".to_owned(),
            KeyCode::Insert => "Insert".to_owned(),
            KeyCode::Left => "ArrowLeft".to_owned(),
            KeyCode::PageDown => "PageDown".to_owned(),
            KeyCode::PageUp => "PageUp".to_owned(),
            KeyCode::Right => "ArrowRight".to_owned(),
            KeyCode::Tab => "Tab".to_owned(),
            KeyCode::Up => "ArrowUp".to_owned(),
        }
    }

    /// The chord as the desktop portal's shortcut triggers write it:
    /// modifiers then the X keysym name, joined by `+`, such as
    /// `CTRL+SHIFT+slash`.
    #[cfg_attr(
        not(all(
            unix,
            not(any(target_os = "macos", target_os = "ios", target_os = "android"))
        )),
        allow(dead_code)
    )]
    fn trigger(self) -> String {
        let keysym = match self.code {
            KeyCode::Char(' ') => "space".to_owned(),
            KeyCode::Char(c) => match punctuation(c) {
                Some((_, _, _, keysym)) => keysym.to_owned(),
                None => c.to_string(),
            },
            KeyCode::F(number) => format!("F{number}"),
            KeyCode::Backspace => "BackSpace".to_owned(),
            KeyCode::Delete => "Delete".to_owned(),
            KeyCode::Down => "Down".to_owned(),
            KeyCode::End => "End".to_owned(),
            KeyCode::Enter => "Return".to_owned(),
            KeyCode::Esc => "Escape".to_owned(),
            KeyCode::Home => "Home".to_owned(),
            KeyCode::Insert => "Insert".to_owned(),
            KeyCode::Left => "Left".to_owned(),
            KeyCode::PageDown => "Page_Down".to_owned(),
            KeyCode::PageUp => "Page_Up".to_owned(),
            KeyCode::Right => "Right".to_owned(),
            KeyCode::Tab => "Tab".to_owned(),
            KeyCode::Up => "Up".to_owned(),
        };
        let names = [
            (Mods::CTRL, "CTRL+"),
            (Mods::ALT, "ALT+"),
            (Mods::SHIFT, "SHIFT+"),
            (Mods::SUPER, "LOGO+"),
        ];
        let mut trigger: String = names
            .iter()
            .filter(|(mods, _)| self.mods.contains(*mods))
            .map(|(_, name)| *name)
            .collect();
        trigger.push_str(&keysym);
        trigger
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The punctuation key typing `c` without Shift, if any.
fn punctuation(c: char) -> Option<(char, char, &'static str, &'static str)> {
    PUNCTUATION.iter().copied().find(|&(plain, ..)| plain == c)
}

/// `hotkeys` with their chords, or why one cannot be registered.
fn bindings(hotkeys: Vec<Hotkey>) -> Result<Vec<Binding>, HotkeyError> {
    let mut ids = HashSet::new();
    hotkeys
        .into_iter()
        .map(|hotkey| {
            if !ids.insert(hotkey.id.clone()) {
                return Err(HotkeyError::DuplicateId(hotkey.id));
            }
            Ok(Binding {
                chord: Chord::of(hotkey.key)?,
                description: hotkey.description,
                id: hotkey.id,
                key: hotkey.key,
            })
        })
        .collect()
}

/// Whether the desktop runs Wayland, where only the portal binds hotkeys.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
fn wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::super::base::key::parse_key;
    use super::*;

    /// The chord `notation` is typed with.
    fn chord(notation: &str) -> Chord {
        Chord::of(parse_key(notation).expect(notation)).expect(notation)
    }

    #[test]
    fn test_keys_land_on_the_us_keyboard_key_with_shift_where_the_character_needs_it() {
        let cases = [
            ("C-M-d", "KeyD", "CTRL+ALT+d"),
            ("C-G", "KeyG", "CTRL+SHIFT+g"),
            ("G", "KeyG", "SHIFT+g"),
            ("M-?", "Slash", "ALT+SHIFT+slash"),
            ("C-/", "Slash", "CTRL+slash"),
            ("s-!", "Digit1", "SHIFT+LOGO+1"),
            ("C-<space>", "Space", "CTRL+space"),
            ("C-<f12>", "F12", "CTRL+F12"),
            ("S-<tab>", "Tab", "SHIFT+Tab"),
            ("M-<pagedown>", "PageDown", "ALT+Page_Down"),
            ("<esc>", "Escape", "Escape"),
            ("C-'", "Quote", "CTRL+apostrophe"),
            ("C-\"", "Quote", "CTRL+SHIFT+apostrophe"),
        ];
        for (notation, code, trigger) in cases {
            let chord = chord(notation);
            assert_eq!(chord.code_name(), code, "{notation}");
            assert_eq!(chord.trigger(), trigger, "{notation}");
        }
    }

    #[test]
    fn test_keys_off_the_us_keyboard_are_refused_by_name() {
        let keys = [
            parse_key("C-é").expect("valid notation"),
            Key::new(KeyCode::F(MAX_FUNCTION_KEY + 1), Mods::CTRL),
        ];
        for key in keys {
            let error = Chord::of(key).expect_err("refused");
            assert!(matches!(error, HotkeyError::UnsupportedKey(refused) if refused == key));
            assert!(error.to_string().contains("F1 to F24"), "{error}");
        }
    }

    #[test]
    fn test_hotkeys_sharing_an_id_are_refused() {
        let key = parse_key("C-a").expect("valid");
        let hotkeys = vec![Hotkey::new("a", "A", key), Hotkey::new("a", "Again", key)];
        assert!(matches!(
            bindings(hotkeys),
            Err(HotkeyError::DuplicateId(id)) if id == "a"
        ));
    }
}
