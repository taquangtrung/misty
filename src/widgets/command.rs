//! The command registry: every command of an app written once, as a
//! [`Command`] in one table, and the key bindings, the menus, the
//! keybinding editor, and the command palette all derived from it through
//! a [`CommandRegistry`].
//!
//! The registry holds keys as misty [`Key`] sequences and reports the ones
//! that clash, but resolves no key presses: turning the keys typed into
//! commands (prefixes, timeouts, layers) is the app's, with the keymap
//! library of its choice, into which it binds the registry's keys.

use std::collections::{HashMap, HashSet};

use thiserror::Error;

use super::super::base::key::{Key, KeyCode, KeyParseError, Mods, key_label, parse_key_sequence};
use super::super::base::strings::Strings;
use super::key_capture::KeyEdit;
use super::menu::{MenuEntry, MenuItem};

// ========================================================================
// Constants
// ========================================================================

/// The letters a focused text field edits with under Alt: `M-b` and `M-f`
/// move by words, `M-d` kills a word, and `M-y` pops the kill ring.
const FIELD_ALT_KEYS: [char; 4] = ['b', 'd', 'f', 'y'];

// ========================================================================
// Data Structures
// ========================================================================

/// One command of an app, written once in the app's `const` table and
/// read by the keymap, the menus, the palette, and the keybinding editor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Command {
    /// The heading menus, the palette, and the keybinding editor group the
    /// command under: `Tabs`.
    pub group: &'static str,
    /// The name the command is dispatched by and the user rebinds it by,
    /// unique in its table: `tab.new`.
    pub id: &'static str,
    /// Every key sequence that runs it by default, in the key notation
    /// (`C-t`, `C-x C-s`), the first one shown as its hint.
    pub keys: &'static [&'static str],
    /// Whether its keys are left to a focused text field, as described
    /// under [`CommandRegistry`]: copy, paste, undo, and the like.
    pub local_only: bool,
    /// What it does, as menus and the palette show it: `New Tab`.
    pub title: &'static str,
}

/// The commands of an app, with the user's key overrides and which
/// commands can run now. It lists the key sequences in force for the
/// app's key dispatch, builds menus from command ids, lists the rows of a
/// keybinding editor with the user's overrides applied, and reports
/// sequences that clash; the command palette lists the same commands, so
/// no key and no title is written twice.
///
/// Keys are written in the key notation of [`Key`](crate::Key) (`C-t`,
/// `C-x C-s`, `M-x`, `<f2>`); menus and the palette show them as people
/// read them (`Ctrl+T`) through [`key_label`](crate::key_label).
///
/// # Keys while text is focused
///
/// A focused text field owns the keys that type or edit text: unmodified
/// and shifted keys, bare Ctrl+letter chords (the Emacs and readline
/// editing keys, `C-a`, `C-e`, `C-k`), and the Alt chords a field edits
/// with (`M-b`, `M-d`, `M-f`, `M-y`). Binding with [`KeyScope::TextFocused`]
/// leaves those keys out, along with every command marked
/// [`Command::local_only`] (copy, paste, undo: commands whose keys a text
/// field needs for its own copy, paste, and undo), so an app dispatching
/// through that keymap while a field has focus never steals the field's
/// keys. Chords with Shift and Ctrl, the other Alt chords, chords with
/// Super, and function keys still reach the app.
#[derive(Clone, Debug)]
pub struct CommandRegistry {
    /// The ids of commands that cannot run now.
    disabled: HashSet<&'static str>,
    /// The commands, in table order, with their default keys parsed.
    entries: Vec<Entry>,
    /// The user's keys, by the id they override.
    overrides: HashMap<&'static str, Vec<Vec<Key>>>,
}

/// One row of a keybinding editor: a command and the keys in force for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandKeys {
    /// The heading the command is listed under.
    pub group: &'static str,
    /// The command's id.
    pub id: &'static str,
    /// The key sequences in force: the user's when overridden, else the
    /// defaults. Empty when the user unbound it.
    pub keys: Vec<Vec<Key>>,
    /// Whether the keys are the user's rather than the defaults.
    pub overridden: bool,
    /// What the command does.
    pub title: &'static str,
}

/// Key sequences in force for more than one command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyConflict {
    /// Every command claiming the sequence, in table order (for a prefix,
    /// the command it belongs to first, then those whose longer sequences
    /// start with it).
    pub commands: Vec<&'static str>,
    /// The sequence: the one claimed twice, or the prefix.
    pub keys: Vec<Key>,
    /// How the sequences clash.
    pub kind: ConflictKind,
    /// The command the sequence runs: for the same sequence, a user's
    /// binding over a default, else the last in table order; for a
    /// prefix, the command it belongs to.
    pub winner: &'static str,
}

/// How two commands' key sequences clash.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConflictKind {
    /// One sequence starts another, as `C-x` starts `C-x C-s`: typed
    /// alone it can only run after a wait for more keys, and an app that
    /// runs a complete sequence at once never reaches the longer one.
    Prefix,
    /// The very same sequence.
    Same,
}

/// Why a command table, an override, or a menu was refused.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum CommandError {
    /// A key sequence written for a command does not parse.
    #[error("command {:?} has keys {:?} that do not parse: {}", .0.id, .0.spec, .0.error)]
    InvalidKeys(InvalidKeys),
    /// Two commands of one table share an id.
    #[error("the command id {0:?} is used twice")]
    RepeatedId(String),
    /// No command has the id.
    #[error("no command has the id {0:?}")]
    UnknownId(String),
}

/// A key sequence that does not parse, and whose it is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidKeys {
    /// Why it does not parse.
    pub error: KeyParseError,
    /// The command it was written for.
    pub id: String,
    /// The sequence as written.
    pub spec: String,
}

/// Which commands and keys [`CommandRegistry::bindings`] lists and
/// [`CommandRegistry::conflicts`] compares.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyScope {
    /// Every key of every command.
    All,
    /// Only what may run while a text field has focus: no local-only
    /// command, and no sequence starting with a key the field types or
    /// edits with.
    TextFocused,
}

/// One line of a menu built from the registry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MenuSlot {
    /// The command with this id.
    Command(&'static str),
    /// A line between groups of commands.
    Separator,
}

/// The commands claiming one key sequence.
#[derive(Clone, Debug)]
struct Claims {
    /// The claims, in table order.
    claimants: Vec<Claim>,
    /// The sequence.
    keys: Vec<Key>,
}

/// One command's claim on a key sequence.
#[derive(Clone, Copy, Debug)]
struct Claim {
    /// The command's id.
    id: &'static str,
    /// Whether the claim is the user's override rather than a default.
    user: bool,
}

/// One command with its default keys parsed.
#[derive(Clone, Debug)]
struct Entry {
    /// The command as written.
    command: &'static Command,
    /// Its default key sequences, in the order written.
    defaults: Vec<Vec<Key>>,
}

// ========================================================================
// Command Implementations
// ========================================================================

impl Command {
    /// The command `id` titled `title` under `group`, with no keys and not
    /// local-only.
    pub const fn new(id: &'static str, title: &'static str, group: &'static str) -> Self {
        Self {
            group,
            id,
            keys: &[],
            local_only: false,
            title,
        }
    }

    /// This command run by default by `keys`, in the key notation.
    pub const fn with_keys(self, keys: &'static [&'static str]) -> Self {
        Self { keys, ..self }
    }

    /// This command with its keys left to a focused text field.
    pub const fn local_only(self) -> Self {
        Self {
            local_only: true,
            ..self
        }
    }
}

// ========================================================================
// CommandRegistry Implementations
// ========================================================================

impl CommandRegistry {
    /// A registry of `commands`, all enabled, with no overrides. Rejected
    /// when a default key spec does not parse or an id is repeated.
    pub fn new(commands: &'static [Command]) -> Result<Self, CommandError> {
        let mut ids = HashSet::new();
        let mut entries = Vec::with_capacity(commands.len());
        for command in commands {
            if !ids.insert(command.id) {
                return Err(CommandError::RepeatedId(command.id.to_owned()));
            }
            entries.push(Entry {
                command,
                defaults: parse_specs(command.id, command.keys)?,
            });
        }
        Ok(Self {
            disabled: HashSet::new(),
            entries,
            overrides: HashMap::new(),
        })
    }

    /// The command `id`, or `None` when the registry has none.
    pub fn command(&self, id: &str) -> Option<&'static Command> {
        self.entry(id).map(|entry| entry.command)
    }

    /// Every command, in table order.
    pub fn commands(&self) -> impl Iterator<Item = &'static Command> + '_ {
        self.entries.iter().map(|entry| entry.command)
    }

    /// Whether command `id` can run now; `false` for an unknown id.
    pub fn is_enabled(&self, id: &str) -> bool {
        self.entry(id).is_some() && !self.disabled.contains(id)
    }

    /// Lets command `id` run, or not: a disabled command is shown muted in
    /// menus and cannot be chosen. Unknown ids are ignored.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        let Some(id) = self.entry(id).map(|entry| entry.command.id) else {
            return;
        };
        match enabled {
            true => self.disabled.remove(id),
            false => self.disabled.insert(id),
        };
    }

    /// Gives command `id` the keys `specs` in place of its defaults and of
    /// any earlier override; no specs unbinds it. Rejected, changing
    /// nothing, when no command has `id` or a spec does not parse.
    pub fn override_keys(&mut self, id: &str, specs: &[&str]) -> Result<(), CommandError> {
        let entry = self
            .entry(id)
            .ok_or_else(|| CommandError::UnknownId(id.to_owned()))?;
        let (id, keys) = (entry.command.id, parse_specs(id, specs)?);
        self.overrides.insert(id, keys);
        Ok(())
    }

    /// Gives command `id` its default keys back.
    pub fn reset_keys(&mut self, id: &str) {
        self.overrides.remove(id);
    }

    /// Makes the change `edit` asks of the keys of the command `id`: one
    /// key of its own, its defaults again, or none. Errors on an id the
    /// registry does not know, or a key its notation cannot read.
    pub fn edit_keys(&mut self, id: &str, edit: &KeyEdit) -> Result<(), CommandError> {
        match edit {
            KeyEdit::Bind(key) => self.override_keys(id, &[key]),
            KeyEdit::Reset => {
                self.entry(id)
                    .ok_or_else(|| CommandError::UnknownId(id.to_owned()))?;
                self.reset_keys(id);
                Ok(())
            }
            KeyEdit::Unbind => self.override_keys(id, &[]),
        }
    }

    /// The key sequences in force for command `id`: the user's when
    /// overridden, else the defaults; empty for an unknown id.
    pub fn keys(&self, id: &str) -> &[Vec<Key>] {
        self.entry(id)
            .map_or(&[], |entry| self.keys_in_force(entry))
    }

    /// The first key sequence in force for command `id`, as menus and the
    /// palette show it (`Ctrl+T`) in `strings`' words (pass the theme's,
    /// `&theme.strings`), or `None` when it has no keys.
    pub fn key_hint(&self, id: &str, strings: &Strings) -> Option<String> {
        self.keys(id)
            .first()
            .map(|sequence| key_label(sequence, strings))
    }

    /// The menu entries of `slots`: each command as an item with its title,
    /// its key hint in `strings`' words, and whether it can run now.
    /// Rejected when a slot names no command.
    pub fn menu(
        &self,
        slots: &[MenuSlot],
        strings: &Strings,
    ) -> Result<Vec<MenuEntry<&'static str>>, CommandError> {
        slots
            .iter()
            .map(|slot| match slot {
                MenuSlot::Command(id) => {
                    let command = self
                        .command(id)
                        .ok_or_else(|| CommandError::UnknownId((*id).to_owned()))?;
                    let item =
                        MenuItem::new(command.id, command.title).with_enabled(self.is_enabled(id));
                    let item = match self.key_hint(id, strings) {
                        Some(hint) => item.with_accelerator(hint),
                        None => item,
                    };
                    Ok(MenuEntry::Item(item))
                }
                MenuSlot::Separator => Ok(MenuEntry::Separator),
            })
            .collect()
    }

    /// One row per command `scope` takes, in table order, with the keys
    /// in force that `scope` takes: with [`KeyScope::All`], every command
    /// and all its keys, the data of a keybinding editor; with
    /// [`KeyScope::TextFocused`], what the app may dispatch while a text
    /// field has focus.
    pub fn bindings(&self, scope: KeyScope) -> Vec<CommandKeys> {
        self.entries
            .iter()
            .filter(|entry| scope == KeyScope::All || !entry.command.local_only)
            .map(|entry| CommandKeys {
                group: entry.command.group,
                id: entry.command.id,
                keys: self
                    .keys_in_force(entry)
                    .iter()
                    .filter(|keys| is_in_scope(keys, scope))
                    .cloned()
                    .collect(),
                overridden: self.overrides.contains_key(entry.command.id),
                title: entry.command.title,
            })
            .collect()
    }

    /// Every clash among the key sequences in force that `scope` takes: a
    /// sequence claimed by more than one command, in the order first
    /// claimed, each with the command that wins it; then each sequence
    /// that starts a longer one of another command.
    pub fn conflicts(&self, scope: KeyScope) -> Vec<KeyConflict> {
        let rows = self.bindings(scope);
        let mut claims: Vec<Claims> = Vec::new();
        for row in &rows {
            let claim = Claim {
                id: row.id,
                user: row.overridden,
            };
            for sequence in &row.keys {
                match claims.iter_mut().find(|claims| claims.keys == *sequence) {
                    Some(claims) => claims.claimants.push(claim),
                    None => claims.push(Claims {
                        claimants: vec![claim],
                        keys: sequence.clone(),
                    }),
                }
            }
        }
        let same = claims
            .iter()
            .filter(|claims| claims.claimants.len() > 1)
            .filter_map(|claims| {
                let mut latest_first = claims.claimants.iter().rev();
                let winner = latest_first
                    .clone()
                    .find(|claim| claim.user)
                    .or(latest_first.next())?
                    .id;
                Some(KeyConflict {
                    commands: claims.claimants.iter().map(|claim| claim.id).collect(),
                    keys: claims.keys.clone(),
                    kind: ConflictKind::Same,
                    winner,
                })
            });
        let prefixes = claims.iter().filter_map(|prefix| {
            let owner = prefix.claimants.first()?.id;
            let shadowed: Vec<&'static str> = claims
                .iter()
                .filter(|longer| longer.keys.len() > prefix.keys.len())
                .filter(|longer| longer.keys.starts_with(&prefix.keys))
                .flat_map(|longer| longer.claimants.iter().map(|claim| claim.id))
                .filter(|id| *id != owner)
                .collect();
            (!shadowed.is_empty()).then(|| KeyConflict {
                commands: [owner].into_iter().chain(shadowed).collect(),
                keys: prefix.keys.clone(),
                kind: ConflictKind::Prefix,
                winner: owner,
            })
        });
        same.chain(prefixes).collect()
    }

    /// The entry of command `id`.
    fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.command.id == id)
    }

    /// The key sequences in force for `entry`.
    fn keys_in_force<'a>(&'a self, entry: &'a Entry) -> &'a [Vec<Key>] {
        self.overrides
            .get(entry.command.id)
            .unwrap_or(&entry.defaults)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The key sequences `specs` name, or the first spec that does not parse,
/// reported against command `id`.
fn parse_specs(id: &str, specs: &[&str]) -> Result<Vec<Vec<Key>>, CommandError> {
    specs
        .iter()
        .map(|spec| {
            parse_key_sequence(spec).map_err(|error| {
                CommandError::InvalidKeys(InvalidKeys {
                    error,
                    id: id.to_owned(),
                    spec: (*spec).to_owned(),
                })
            })
        })
        .collect()
}

/// Whether `scope` binds the sequence `keys`.
fn is_in_scope(keys: &[Key], scope: KeyScope) -> bool {
    match scope {
        KeyScope::All => true,
        KeyScope::TextFocused => keys.first().is_some_and(|key| !is_text_key(key)),
    }
}

/// Whether a focused text field types or edits with `key`: unmodified or
/// shifted keys other than function keys, bare Ctrl+letter chords, and the
/// Alt chords of its word motions, word kill, and yank pop.
fn is_text_key(key: &Key) -> bool {
    let chord = key.mods.without(Mods::SHIFT);
    match key.code {
        KeyCode::F(_) => false,
        KeyCode::Char(c) if key.mods == Mods::CTRL => c.is_ascii_alphabetic(),
        KeyCode::Char(c) if key.mods == Mods::ALT => FIELD_ALT_KEYS.contains(&c),
        KeyCode::Backspace
        | KeyCode::Char(_)
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
        | KeyCode::Up => chord == Mods::NONE,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::key::parse_key;

    /// Tabs and an editing group: `tab.new` on `C-t`, `tab.close` on
    /// `C-S-q`, `palette` on `M-x` and `<f1>`, a local-only `copy` on
    /// `C-c`, `line.start` on `C-a`, and `quit` with no keys.
    const COMMANDS: &[Command] = &[
        Command::new("tab.new", "New Tab", "Tabs").with_keys(&["C-t"]),
        Command::new("tab.close", "Close Tab", "Tabs").with_keys(&["C-S-q"]),
        Command::new("palette", "Command Palette", "View").with_keys(&["M-x", "<f1>"]),
        Command::new("copy", "Copy", "Edit")
            .with_keys(&["C-c"])
            .local_only(),
        Command::new("line.start", "Line Start", "Edit").with_keys(&["C-a"]),
        Command::new("quit", "Quit", "App"),
    ];

    fn registry() -> CommandRegistry {
        CommandRegistry::new(COMMANDS).expect("a valid table")
    }

    fn keys(spec: &str) -> Vec<Key> {
        parse_key_sequence(spec).expect("a valid sequence")
    }

    /// The command `scope` binds `spec` to, as the rows list it.
    fn bound(registry: &CommandRegistry, scope: KeyScope, spec: &str) -> Option<&'static str> {
        registry
            .bindings(scope)
            .into_iter()
            .find(|row| row.keys.contains(&keys(spec)))
            .map(|row| row.id)
    }

    #[test]
    fn test_a_table_with_a_repeated_id_or_a_bad_spec_is_rejected() {
        const REPEATED: &[Command] = &[Command::new("a", "A", "G"), Command::new("a", "B", "G")];
        assert_eq!(
            CommandRegistry::new(REPEATED).err(),
            Some(CommandError::RepeatedId("a".to_owned()))
        );
        const BAD: &[Command] = &[Command::new("a", "A", "G").with_keys(&["C-<nope>"])];
        assert!(matches!(
            CommandRegistry::new(BAD),
            Err(CommandError::InvalidKeys(InvalidKeys { ref id, .. })) if id == "a"
        ));
    }

    #[test]
    fn test_an_override_replaces_the_defaults_until_reset() {
        let mut registry = registry();
        registry
            .override_keys("tab.new", &["C-n"])
            .expect("a known id");
        assert_eq!(bound(&registry, KeyScope::All, "C-n"), Some("tab.new"));
        assert_eq!(
            bound(&registry, KeyScope::All, "C-t"),
            None,
            "the default is gone"
        );
        assert_eq!(
            registry.key_hint("tab.new", &Strings::english()).as_deref(),
            Some("Ctrl+N")
        );
        registry.reset_keys("tab.new");
        assert_eq!(registry.keys("tab.new"), [keys("C-t")]);
        assert_eq!(
            registry.override_keys("nope", &[]),
            Err(CommandError::UnknownId("nope".to_owned()))
        );
    }

    #[test]
    fn test_text_focus_leaves_out_editing_keys_and_local_only_commands() {
        let registry = registry();
        let text = |spec| bound(&registry, KeyScope::TextFocused, spec);
        assert_eq!(text("C-a"), None, "C-a moves the caret");
        assert_eq!(text("C-t"), None, "a bare Ctrl+letter");
        assert_eq!(text("C-c"), None, "copy is local-only");
        assert_eq!(text("C-S-q"), Some("tab.close"));
        assert_eq!(text("M-x"), Some("palette"));
        assert_eq!(text("<f1>"), Some("palette"));
        assert_eq!(bound(&registry, KeyScope::All, "C-c"), Some("copy"));
    }

    #[test]
    fn test_text_focus_leaves_out_the_alt_chords_a_field_edits_with() {
        const ALT: &[Command] = &[
            Command::new("pinyin", "Toggle Pinyin", "View").with_keys(&["M-y"]),
            Command::new("bookmark", "Bookmark", "View").with_keys(&["M-y"]),
            Command::new("back", "Back", "History").with_keys(&["M-b"]),
            Command::new("hints", "Hints", "View").with_keys(&["M-h"]),
        ];
        let registry = CommandRegistry::new(ALT).expect("a valid table");
        let text = |spec| bound(&registry, KeyScope::TextFocused, spec);
        assert_eq!(text("M-y"), None, "M-y pops the field's kill ring");
        assert_eq!(text("M-b"), None, "M-b moves the caret a word");
        assert_eq!(text("M-h"), Some("hints"), "other Alt chords reach the app");
        assert_eq!(bound(&registry, KeyScope::All, "M-y"), Some("pinyin"));
        assert_eq!(registry.conflicts(KeyScope::All).len(), 1, "M-y twice");
        assert!(
            registry.conflicts(KeyScope::TextFocused).is_empty(),
            "the field's key is no clash while it has focus"
        );
    }

    #[test]
    fn test_a_menu_shows_titles_hints_and_availability_from_the_registry() {
        let mut registry = registry();
        registry.set_enabled("tab.close", false);
        let slots = [
            MenuSlot::Command("tab.new"),
            MenuSlot::Separator,
            MenuSlot::Command("tab.close"),
            MenuSlot::Command("quit"),
        ];
        let entries = registry
            .menu(&slots, &Strings::english())
            .expect("known ids");
        let expected: Vec<MenuEntry<&str>> = vec![
            MenuItem::new("tab.new", "New Tab")
                .with_accelerator("Ctrl+T")
                .into(),
            MenuEntry::Separator,
            MenuItem::new("tab.close", "Close Tab")
                .with_accelerator("Ctrl+Shift+Q")
                .with_enabled(false)
                .into(),
            MenuItem::new("quit", "Quit").into(),
        ];
        assert_eq!(entries, expected);
        assert_eq!(
            registry.menu(&[MenuSlot::Command("gone")], &Strings::english()),
            Err(CommandError::UnknownId("gone".to_owned()))
        );
    }

    #[test]
    fn test_keybinding_rows_mark_overrides_and_unbinding() {
        let mut registry = registry();
        registry.override_keys("palette", &[]).expect("a known id");
        let rows = registry.bindings(KeyScope::All);
        let palette = rows.iter().find(|row| row.id == "palette").expect("a row");
        assert!(palette.overridden);
        assert!(palette.keys.is_empty());
        let new_tab = rows.iter().find(|row| row.id == "tab.new").expect("a row");
        assert!(!new_tab.overridden);
        assert_eq!(new_tab.keys, [keys("C-t")]);
    }

    #[test]
    fn test_conflicts_name_every_claimant_and_the_winner() {
        const CLASHING: &[Command] = &[
            Command::new("a", "A", "G").with_keys(&["C-k"]),
            Command::new("b", "B", "G").with_keys(&["C-k", "C-j"]),
            Command::new("c", "C", "G").with_keys(&["C-l"]),
        ];
        let mut registry = CommandRegistry::new(CLASHING).expect("a valid table");
        let conflicts = registry.conflicts(KeyScope::All);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].commands, ["a", "b"]);
        assert_eq!(conflicts[0].kind, ConflictKind::Same);
        assert_eq!(conflicts[0].winner, "b", "the later default wins");
        registry.override_keys("a", &["C-j"]).expect("a known id");
        let conflicts = registry.conflicts(KeyScope::All);
        assert_eq!(conflicts.len(), 1, "C-k is b's alone now");
        assert_eq!(conflicts[0].keys, vec![parse_key("C-j").expect("a key")]);
        assert_eq!(conflicts[0].winner, "a", "the user's binding wins");
    }

    #[test]
    fn test_a_sequence_starting_another_commands_sequence_is_a_prefix_conflict_in_its_scope() {
        const PREFIXED: &[Command] = &[
            Command::new("save", "Save", "File").with_keys(&["C-x C-s"]),
            Command::new("cut", "Cut", "Edit")
                .with_keys(&["C-x"])
                .local_only(),
            Command::new("menu", "Menu", "View").with_keys(&["<f10>", "<f10> m"]),
        ];
        let registry = CommandRegistry::new(PREFIXED).expect("a valid table");
        let conflicts = registry.conflicts(KeyScope::All);
        assert_eq!(
            conflicts.len(),
            1,
            "a command's own longer keys are no clash"
        );
        assert_eq!(conflicts[0].kind, ConflictKind::Prefix);
        assert_eq!(conflicts[0].keys, keys("C-x"));
        assert_eq!(conflicts[0].commands, ["cut", "save"]);
        assert_eq!(conflicts[0].winner, "cut");
        assert!(
            registry.conflicts(KeyScope::TextFocused).is_empty(),
            "cut is left to a focused text field"
        );
    }
}
