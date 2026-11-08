//! Global hotkeys grabbed through the `global-hotkey` crate: X11, Windows,
//! and macOS. The crate reports presses to one handler for the whole
//! process, set once; it hands them to the route of the hotkeys
//! registered now, if any.

use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};
use std::str::FromStr;
use std::sync::{Mutex, Once, PoisonError};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{Error as CrateError, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use super::super::super::base::key::Mods;
use super::{Binding, Chord, HotkeyError, HotkeyEvent, Sender};

// ========================================================================
// Constants
// ========================================================================

/// Which misty modifier is which of the crate's.
const MODIFIERS: [(Mods, Modifiers); 4] = [
    (Mods::ALT, Modifiers::ALT),
    (Mods::CTRL, Modifiers::CONTROL),
    (Mods::SHIFT, Modifiers::SHIFT),
    (Mods::SUPER, Modifiers::META),
];

/// The route of the hotkeys registered now, or `None`.
static ROUTE: Mutex<Option<Route>> = Mutex::new(None);

/// Sets the crate's handler, which may be set only once a process.
static HANDLER: Once = Once::new();

// ========================================================================
// Data Structures
// ========================================================================

/// Hotkeys registered with the crate, let go when dropped.
pub(super) struct Native {
    /// The hotkeys registered.
    hotkeys: Vec<HotKey>,
    /// The crate's handle on the system's hotkeys.
    manager: GlobalHotKeyManager,
}

/// Where the crate's presses go: the ids of the hotkeys registered now by
/// the crate's number for each, and the app to tell.
struct Route {
    /// The app's id of each hotkey, by the crate's number for it.
    ids: HashMap<u32, String>,
    /// Hands presses to the app.
    send: Sender,
}

// ========================================================================
// Native Implementations
// ========================================================================

impl Native {
    /// Registers `bindings`, each press going to `send`, replacing the
    /// route of any hotkeys registered before.
    pub(super) fn register(bindings: &[Binding], send: Sender) -> Result<Self, HotkeyError> {
        let manager = GlobalHotKeyManager::new()
            .map_err(|error| HotkeyError::Unavailable(error.to_string()))?;
        let mut native = Self {
            hotkeys: Vec::new(),
            manager,
        };
        let mut ids = HashMap::new();
        for binding in bindings {
            let code = code_of(binding.chord).ok_or(HotkeyError::UnsupportedKey(binding.key))?;
            let hotkey = HotKey::new(Some(modifiers_of(binding.chord.mods)), code);
            // Dropping `native` lets go of those registered so far.
            native
                .manager
                .register(hotkey)
                .map_err(|error| match error {
                    CrateError::AlreadyRegistered(_) => HotkeyError::Taken(binding.key),
                    other => HotkeyError::Unavailable(other.to_string()),
                })?;
            native.hotkeys.push(hotkey);
            ids.insert(hotkey.id(), binding.id.clone());
        }
        HANDLER.call_once(|| GlobalHotKeyEvent::set_event_handler(Some(deliver)));
        *ROUTE.lock().unwrap_or_else(PoisonError::into_inner) = Some(Route { ids, send });
        Ok(native)
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        *ROUTE.lock().unwrap_or_else(PoisonError::into_inner) = None;
        // A key the system already dropped needs no letting go.
        let _ = self.manager.unregister_all(&self.hotkeys);
    }
}

impl Debug for Native {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Native")
            .field("hotkeys", &self.hotkeys)
            .finish_non_exhaustive()
    }
}

// ========================================================================
// Route Implementations
// ========================================================================

impl Route {
    /// Tells the app of `event` when it is a press of one of its hotkeys.
    fn deliver(&self, event: GlobalHotKeyEvent) {
        if event.state() != HotKeyState::Pressed {
            return;
        }
        if let Some(id) = self.ids.get(&event.id()) {
            (self.send)(HotkeyEvent::Pressed(id.clone()));
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The crate's handler: hands `event` to the route of the hotkeys
/// registered now.
fn deliver(event: GlobalHotKeyEvent) {
    let route = ROUTE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(route) = route.as_ref() {
        route.deliver(event);
    }
}

/// The crate's key for `chord`, or `None` when it names none.
fn code_of(chord: Chord) -> Option<Code> {
    Code::from_str(&chord.code_name()).ok()
}

/// The crate's modifiers for `mods`.
fn modifiers_of(mods: Mods) -> Modifiers {
    MODIFIERS
        .iter()
        .filter(|(misty, _)| mods.contains(*misty))
        .fold(Modifiers::empty(), |all, (_, crate_mods)| all | *crate_mods)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::super::super::base::key::parse_key;
    use super::*;

    #[test]
    fn test_every_kind_of_key_has_a_crate_code_and_modifiers_carry_over() {
        for notation in [
            "C-M-d",
            "s-5",
            "C-?",
            "<space>",
            "<f24>",
            "<pagedown>",
            "<cr>",
        ] {
            let chord = Chord::of(parse_key(notation).expect(notation)).expect(notation);
            assert!(
                code_of(chord).is_some(),
                "{notation}: {}",
                chord.code_name()
            );
        }
        let chord = Chord::of(parse_key("C-s-G").expect("valid")).expect("a chord");
        let hotkey = HotKey::new(Some(modifiers_of(chord.mods)), code_of(chord).expect("G"));
        assert!(hotkey.matches(
            Modifiers::CONTROL | Modifiers::META | Modifiers::SHIFT,
            Code::KeyG
        ));
    }

    #[test]
    fn test_only_presses_of_registered_hotkeys_reach_the_app() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&got);
        let route = Route {
            ids: HashMap::from([(7, "lookup".to_owned())]),
            send: Arc::new(move |event| {
                if let HotkeyEvent::Pressed(id) = event {
                    sink.lock().expect("unpoisoned").push(id);
                }
            }),
        };
        for (id, state) in [
            (7, HotKeyState::Pressed),
            (7, HotKeyState::Released),
            (8, HotKeyState::Pressed),
        ] {
            route.deliver(GlobalHotKeyEvent { id, state });
        }
        assert_eq!(*got.lock().expect("unpoisoned"), ["lookup"]);
    }
}
