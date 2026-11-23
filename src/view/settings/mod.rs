//! An app's settings as elements of the page kit, each controlled: the app
//! holds the values and the scopes, builds a [`Setting`] for each with the
//! value it holds now ([`Setting::with_value`]), and hears every edit as a
//! [`SettingChange`]. From the bottom up: [`setting_row`], one setting as
//! an [`item`] with the control its kind takes (a switch, a dropdown, a
//! stepper, a text, names, or color field, an action's button), and
//! [`settings_section`], a [`SettingsSection`]'s rows in a card, kept to
//! those a filter matches; and [`keybindings_section`], every command of a
//! [`CommandRegistry`] with its keys as key caps, rebound by pressing the
//! new one ([`key_capture`]), each change heard as a [`KeyChange`] for the
//! app to make in its registry ([`CommandRegistry::edit_keys`]). On top,
//! [`settings_page`] puts them together on a [`page`]: a sidebar of the
//! sections and the keybindings choosing the card shown, a filter field
//! at the heading's right showing what it matches from every card, and
//! the scopes as tabs; it draws nothing of its own. A row whose value a later scope sets says so,
//! and in that later scope offers a Reset button dropping the value.
//!
//! The built-in words (the names hint, the scope notes, Reset) are the
//! theme's [`Strings`], handed in by the app, as the elements of a tree do
//! not see the theme until they are drawn.

use std::borrow::Cow;
use std::rc::Rc;

use super::super::base::fuzzy;
use super::super::base::key::key_label;
use super::super::base::strings::{self, Strings};
use super::super::widgets::command::{CommandKeys, CommandRegistry, KeyConflict, KeyScope};
use super::super::widgets::key_capture::KeyEdit;
use super::controls::{Callback, button, checkbox, select_owned, stepper, text_field};
use super::element::Element;
use super::fields::{color_field, key_capture, names_field};
use super::page::{Item, Section, item, page, section, sidebar};

mod model;

pub(crate) use model::SettingKind;
pub use model::{Setting, SettingValue, SettingsSection};

// ========================================================================
// Data Structures
// ========================================================================

/// What the user did to a setting.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingEdit {
    /// Pressed an action's button.
    Activated,
    /// Changed its value to this.
    Changed(SettingValue),
    /// Dropped the value the scope shown sets, for the one before it.
    Cleared,
}

/// An edit of the setting the app knows by `id`.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingChange {
    /// What the user did.
    pub edit: SettingEdit,
    /// The setting's id.
    pub id: String,
}

/// Where a setting stands against the app's scopes, as a row says.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SettingScope {
    /// Its value is the scope's own and in force.
    #[default]
    InForce,
    /// A later scope sets it too, so the value shown is not in force there.
    Overridden,
    /// The scope shown, a later one, sets it: it may be reset.
    OwnValue,
}

/// What says where a setting, by id, stands against the app's scopes.
type ScopeOf<'a> = dyn Fn(&str) -> SettingScope + 'a;

/// A change the user asked of the keys of the command `id`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyChange {
    /// What the user asked.
    pub edit: KeyEdit,
    /// The command's id.
    pub id: &'static str,
}

/// Every command of a registry with its keys, in a card. Made by
/// [`keybindings_section`].
pub struct KeybindingsCard<'a, A> {
    /// The text the commands are kept to, matched against each's group
    /// and title; empty for all.
    filter: &'a str,
    /// What runs with each change of keys.
    on_change: Option<Callback<A, KeyChange>>,
    /// The commands and their keys.
    registry: &'a CommandRegistry,
    /// The theme's words.
    strings: &'a Strings,
    /// The card's heading.
    title: &'a str,
}

/// An app's settings and keybindings as a page. Made by [`settings_page`].
pub struct SettingsPage<'a, A> {
    /// The text the rows are kept to; while it is not empty every card
    /// shows its matches.
    filter: &'a str,
    /// The keybindings card's heading and the commands, if the page lists
    /// them, after the sections.
    keybindings: Option<(&'a str, &'a CommandRegistry)>,
    /// What runs with each change of keys.
    on_keys: Option<Callback<A, KeyChange>>,
    /// What runs with the filter typed.
    on_filter: Option<Callback<A, String>>,
    /// What runs with each edit of a setting.
    on_change: Option<Callback<A, SettingChange>>,
    /// What runs with the index of the card chosen in the sidebar.
    on_show: Option<Callback<A, usize>>,
    /// The scopes' names, the one shown, and what runs with the one
    /// chosen, if the page keeps its values in scopes.
    scopes: Option<(&'a [String], usize, Callback<A, usize>)>,
    /// Where a setting stands against the scopes, by id.
    scope_of: Option<Rc<ScopeOf<'a>>>,
    /// The sections, in the sidebar's order.
    sections: &'a [SettingsSection],
    /// The card shown: a section's index, or the keybindings' after them.
    shown: usize,
    /// The theme's words.
    strings: &'a Strings,
    /// The page's heading.
    title: &'a str,
}

/// One setting as a row of a card, with the control its kind takes. Made
/// by [`setting_row`].
pub struct SettingRow<'a, A> {
    /// What runs with each edit.
    on_change: Option<Callback<A, SettingChange>>,
    /// Where it stands against the scopes.
    scope: SettingScope,
    /// The setting, holding the app's value now.
    setting: &'a Setting,
    /// The theme's words.
    strings: &'a Strings,
}

/// A section of settings in a card, kept to the rows a filter matches.
/// Made by [`settings_section`].
pub struct SettingsCard<'a, A> {
    /// The text the rows are kept to, matched against each's section and
    /// label; empty for all.
    filter: &'a str,
    /// What runs with each edit.
    on_change: Option<Callback<A, SettingChange>>,
    /// Where a setting stands against the scopes, by id; one not named is
    /// in force.
    scopes: Option<Box<ScopeOf<'a>>>,
    /// The section.
    section: &'a SettingsSection,
    /// The theme's words.
    strings: &'a Strings,
}

// ========================================================================
// SettingRow Implementations
// ========================================================================

impl<'a, A: 'static> SettingRow<'a, A> {
    /// This row running `apply` on the app's state with each edit.
    pub fn on_change(mut self, apply: impl Fn(&mut A, SettingChange) + 'static) -> Self {
        self.on_change = Some(Rc::new(apply));
        self
    }

    /// This row standing as `scope` against the app's scopes.
    pub fn scope(self, scope: SettingScope) -> Self {
        Self { scope, ..self }
    }

    /// The row's control, running the change callback with its edits.
    fn control(&self) -> Option<Element<'a, A>> {
        let setting = self.setting;
        let apply = || self.on_change.clone();
        let control: Element<'a, A> = match (setting.kind(), setting.value()) {
            (SettingKind::Toggle, SettingValue::Toggle(on)) => checkbox(setting.label(), *on)
                .switch()
                .on_change(report(apply(), setting.id(), |on| {
                    SettingEdit::Changed(SettingValue::Toggle(on))
                }))
                .into(),
            (SettingKind::Choice(options), SettingValue::Choice(chosen)) => {
                let options = options
                    .iter()
                    .enumerate()
                    .map(|(at, option)| (at, option.as_str()))
                    .collect();
                select_owned(options, chosen)
                    .on_change(report(apply(), setting.id(), |at| {
                        SettingEdit::Changed(SettingValue::Choice(at))
                    }))
                    .into()
            }
            (SettingKind::Number(range), SettingValue::Number(number)) => stepper(*range, *number)
                .on_change(report(apply(), setting.id(), |number| {
                    SettingEdit::Changed(SettingValue::Number(number))
                }))
                .into(),
            (SettingKind::Text, SettingValue::Text(text)) => text_field(text)
                .on_change(report(apply(), setting.id(), |text| {
                    SettingEdit::Changed(SettingValue::Text(text))
                }))
                .into(),
            (SettingKind::Names(_), SettingValue::Names(names)) => names_field(names)
                .on_change(report(apply(), setting.id(), |names| {
                    SettingEdit::Changed(SettingValue::Names(names))
                }))
                .into(),
            (SettingKind::Color, SettingValue::Color(color)) => color_field(*color)
                .on_change(report(apply(), setting.id(), |color| {
                    SettingEdit::Changed(SettingValue::Color(color))
                }))
                .into(),
            (SettingKind::Action(caption), _) => {
                let press = report(apply(), setting.id(), |()| SettingEdit::Activated);
                button(caption).on_click(move |app| press(app, ())).into()
            }
            // A value of another kind than the setting's is not one a
            // setting holds (`Setting::with_value` refuses it).
            _ => return None,
        };
        Some(control)
    }

    /// What the row says under its label: the scope's note, else the
    /// setting's description, else the names it may take too.
    fn note(&self) -> Option<Cow<'a, str>> {
        match self.scope {
            SettingScope::Overridden => return Some(self.strings.settings_overridden.clone()),
            SettingScope::OwnValue => return Some(self.strings.settings_own_value.clone()),
            SettingScope::InForce => {}
        }
        if let Some(description) = self.setting.description() {
            return Some(Cow::Borrowed(description));
        }
        let offered = self.setting.names_offered();
        (!offered.is_empty()).then(|| {
            Cow::Owned(strings::fill(
                &self.strings.settings_names_hint,
                &[&offered.join(" ")],
            ))
        })
    }
}

impl<'a, A: 'static> From<SettingRow<'a, A>> for Item<'a, A> {
    fn from(row: SettingRow<'a, A>) -> Self {
        let mut shown = item(row.setting.label());
        if let Some(note) = row.note() {
            shown = shown.description(note);
        }
        // The control takes the setting's id, so focusing the setting
        // gives it the keyboard.
        if let Some(control) = row.control() {
            shown = shown.trailing(control.id(row.setting.id()));
        }
        if row.scope == SettingScope::OwnValue {
            let clear = report(row.on_change.clone(), row.setting.id(), |()| {
                SettingEdit::Cleared
            });
            let reset = button(&row.strings.settings_reset).on_click(move |app| clear(app, ()));
            shown = shown.trailing(reset);
        }
        shown
    }
}

impl<'a, A: 'static> From<SettingRow<'a, A>> for Element<'a, A> {
    fn from(row: SettingRow<'a, A>) -> Self {
        Item::from(row).into()
    }
}

// ========================================================================
// SettingsCard Implementations
// ========================================================================

impl<'a, A: 'static> SettingsCard<'a, A> {
    /// This card keeping only the settings `filter` matches, fuzzily,
    /// against `Section: Label`; an empty filter keeps them all.
    pub fn filter(self, filter: &'a str) -> Self {
        Self { filter, ..self }
    }

    /// This card running `apply` on the app's state with each edit.
    pub fn on_change(mut self, apply: impl Fn(&mut A, SettingChange) + 'static) -> Self {
        self.on_change = Some(Rc::new(apply));
        self
    }

    /// This card asking `scope` where each setting, by id, stands against
    /// the app's scopes.
    pub fn scopes(self, scope: impl Fn(&str) -> SettingScope + 'a) -> Self {
        Self {
            scopes: Some(Box::new(scope)),
            ..self
        }
    }

    /// The settings the filter keeps, top to bottom.
    pub fn shown(&self) -> impl Iterator<Item = &'a Setting> + '_ {
        let title = &self.section.title;
        self.section.settings.iter().filter(move |setting| {
            self.filter.is_empty()
                || fuzzy::score(self.filter, &format!("{title}: {}", setting.label())).is_some()
        })
    }
}

impl<'a, A: 'static> From<SettingsCard<'a, A>> for Section<'a, A> {
    fn from(card: SettingsCard<'a, A>) -> Self {
        let rows = card.shown().map(|setting| {
            let mut row = setting_row(setting, card.strings);
            row.on_change = card.on_change.clone();
            if let Some(scope) = &card.scopes {
                row.scope = scope(setting.id());
            }
            Element::from(row)
        });
        let rows: Vec<Element<'a, A>> = rows.collect();
        section(card.section.title.as_str()).items(rows)
    }
}

impl<'a, A: 'static> From<SettingsCard<'a, A>> for Element<'a, A> {
    fn from(card: SettingsCard<'a, A>) -> Self {
        Section::from(card).into()
    }
}

// ========================================================================
// SettingsPage Implementations
// ========================================================================

impl<'a, A: 'static> SettingsPage<'a, A> {
    /// The id of the page's filter field, for an app giving it the
    /// keyboard ([`ViewHost::focus`](super::ViewHost::focus)).
    pub const FILTER_ID: &'static str = "misty.settings.filter";

    /// This page listing `registry`'s commands in a card headed `title`,
    /// after the sections, running `apply` with each change of keys.
    pub fn keybindings(
        self,
        title: &'a str,
        registry: &'a CommandRegistry,
        apply: impl Fn(&mut A, KeyChange) + 'static,
    ) -> Self {
        Self {
            keybindings: Some((title, registry)),
            on_keys: Some(Rc::new(apply)),
            ..self
        }
    }

    /// This page with a filter field holding `filter`, running `apply` with
    /// each edit of it; while it is not empty, every card shows what it
    /// matches.
    pub fn filter(self, filter: &'a str, apply: impl Fn(&mut A, String) + 'static) -> Self {
        Self {
            filter,
            on_filter: Some(Rc::new(apply)),
            ..self
        }
    }

    /// This page running `apply` with each edit of a setting.
    pub fn on_change(self, apply: impl Fn(&mut A, SettingChange) + 'static) -> Self {
        Self {
            on_change: Some(Rc::new(apply)),
            ..self
        }
    }

    /// This page showing the `shown`th card (the keybindings after the
    /// sections), running `apply` with the index of one chosen in the
    /// sidebar.
    pub fn shown(self, shown: usize, apply: impl Fn(&mut A, usize) + 'static) -> Self {
        Self {
            on_show: Some(Rc::new(apply)),
            shown,
            ..self
        }
    }

    /// This page keeping its values in the scopes `names` (User and
    /// Workspace), the `shown`th in force on the page, `apply` running with
    /// the one chosen; `scope_of` says where each setting, by id, stands
    /// against them.
    pub fn scopes(
        self,
        names: &'a [String],
        shown: usize,
        apply: impl Fn(&mut A, usize) + 'static,
        scope_of: impl Fn(&str) -> SettingScope + 'a,
    ) -> Self {
        Self {
            scope_of: Some(Rc::new(scope_of)),
            scopes: Some((names, shown, Rc::new(apply))),
            ..self
        }
    }

    /// The card of section `section`, kept to the filter, asking the page
    /// where each setting stands.
    fn section_card(&self, section: &'a SettingsSection) -> SettingsCard<'a, A> {
        let mut card = settings_section(section, self.strings).filter(self.filter);
        card.on_change = self.on_change.clone();
        if let Some(scope_of) = &self.scope_of {
            let scope_of = Rc::clone(scope_of);
            card = card.scopes(move |id| scope_of(id));
        }
        card
    }

    /// The keybindings card, kept to the filter, if the page lists them.
    fn keybindings_card(&self) -> Option<KeybindingsCard<'a, A>> {
        let (title, registry) = self.keybindings?;
        let mut card = keybindings_section(title, registry, self.strings).filter(self.filter);
        card.on_change = self.on_keys.clone();
        Some(card)
    }

    /// The cards the page shows: the chosen one, or while filtering every
    /// card with a match.
    fn cards(&self) -> Vec<Element<'a, A>> {
        if self.filter.is_empty() {
            let card = match self.sections.get(self.shown) {
                Some(section) => Some(self.section_card(section).into()),
                None => self.keybindings_card().map(Element::from),
            };
            return card.into_iter().collect();
        }
        let sections = self
            .sections
            .iter()
            .map(|section| self.section_card(section))
            .filter(|card| card.shown().next().is_some())
            .map(Element::from);
        let keys = self
            .keybindings_card()
            .filter(|card| !card.shown().is_empty())
            .map(Element::from);
        sections.chain(keys).collect()
    }
}

impl<'a, A: 'static> From<SettingsPage<'a, A>> for Element<'a, A> {
    fn from(settings: SettingsPage<'a, A>) -> Self {
        let mut entries: Vec<&str> = settings
            .sections
            .iter()
            .map(|section| section.title.as_str())
            .collect();
        entries.extend(settings.keybindings.map(|(title, _)| title));
        let on_show = settings.on_show.clone();
        let choose = move |app: &mut A, at| {
            if let Some(apply) = &on_show {
                apply(app, at);
            }
        };
        let mut shown = page(settings.title)
            .sidebar(sidebar(entries, settings.shown, choose))
            .content(super::element::column(settings.cards()));
        if let Some((names, scope, apply)) = &settings.scopes {
            let apply = Rc::clone(apply);
            shown = shown.scopes(names.iter().map(String::as_str), *scope, move |app, at| {
                apply(app, at);
            });
        }
        if let Some(apply) = &settings.on_filter {
            let apply = Rc::clone(apply);
            let field = text_field(settings.filter)
                .placeholder(&settings.strings.settings_filter_placeholder)
                .on_change(move |app, filter| apply(app, filter));
            shown = shown.action(Element::from(field).id(SettingsPage::<A>::FILTER_ID));
        }
        shown.into()
    }
}

// ========================================================================
// KeybindingsCard Implementations
// ========================================================================

impl<'a, A: 'static> KeybindingsCard<'a, A> {
    /// This card keeping only the commands `filter` matches, fuzzily,
    /// against `Group: Title`; an empty filter keeps them all.
    pub fn filter(self, filter: &'a str) -> Self {
        Self { filter, ..self }
    }

    /// This card running `apply` on the app's state with each change of
    /// keys.
    pub fn on_change(mut self, apply: impl Fn(&mut A, KeyChange) + 'static) -> Self {
        self.on_change = Some(Rc::new(apply));
        self
    }

    /// The commands the filter keeps, in the registry's order.
    pub fn shown(&self) -> Vec<CommandKeys> {
        self.registry
            .bindings(KeyScope::All)
            .into_iter()
            .filter(|row| {
                self.filter.is_empty()
                    || fuzzy::score(self.filter, &format!("{}: {}", row.group, row.title)).is_some()
            })
            .collect()
    }

    /// The row of `command`: its title, its group or the conflict its keys
    /// are in, and its keys as key caps.
    fn row(&self, command: CommandKeys, conflicts: &[KeyConflict]) -> Element<'a, A> {
        let keys: Vec<String> = command
            .keys
            .iter()
            .map(|sequence| key_label(sequence, self.strings))
            .collect();
        let conflict = conflict_note(&command, conflicts, self.registry, self.strings);
        let conflicted = conflict.is_some();
        let note = conflict.map_or(Cow::Borrowed(command.group), Cow::Owned);
        let apply = self.on_change.clone();
        let id = command.id;
        let capture =
            key_capture(keys)
                .conflicted(conflicted)
                .on_change(move |app: &mut A, edit| {
                    if let Some(apply) = &apply {
                        apply(app, KeyChange { edit, id });
                    }
                });
        item(command.title)
            .id(command.id)
            .description(note)
            .trailing(capture)
            .into()
    }
}

impl<'a, A: 'static> From<KeybindingsCard<'a, A>> for Section<'a, A> {
    fn from(card: KeybindingsCard<'a, A>) -> Self {
        let conflicts = card.registry.conflicts(KeyScope::All);
        let rows: Vec<Element<'a, A>> = card
            .shown()
            .into_iter()
            .map(|command| card.row(command, &conflicts))
            .collect();
        section(card.title).items(rows)
    }
}

impl<'a, A: 'static> From<KeybindingsCard<'a, A>> for Element<'a, A> {
    fn from(card: KeybindingsCard<'a, A>) -> Self {
        Section::from(card).into()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The settings of `sections` as a page headed `title`, worded in
/// `strings`: the first section shown until [`SettingsPage::shown`] says
/// otherwise.
pub fn settings_page<'a, A>(
    title: &'a str,
    sections: &'a [SettingsSection],
    strings: &'a Strings,
) -> SettingsPage<'a, A> {
    SettingsPage {
        filter: "",
        keybindings: None,
        on_change: None,
        on_filter: None,
        on_keys: None,
        on_show: None,
        scope_of: None,
        scopes: None,
        sections,
        shown: 0,
        strings,
        title,
    }
}

/// `registry`'s commands with their keys, in a card headed `title`, worded
/// in `strings`: a command whose keys another claims says which.
pub fn keybindings_section<'a, A>(
    title: &'a str,
    registry: &'a CommandRegistry,
    strings: &'a Strings,
) -> KeybindingsCard<'a, A> {
    KeybindingsCard {
        filter: "",
        on_change: None,
        registry,
        strings,
        title,
    }
}

/// The sentence naming the other commands that claim `command`'s keys, in
/// `words`, or `None` when none do.
fn conflict_note(
    command: &CommandKeys,
    conflicts: &[KeyConflict],
    registry: &CommandRegistry,
    words: &Strings,
) -> Option<String> {
    let conflict = conflicts
        .iter()
        .find(|conflict| conflict.commands.contains(&command.id))?;
    let others: Vec<&str> = conflict
        .commands
        .iter()
        .filter(|id| **id != command.id)
        .map(|id| registry.command(id).map_or(*id, |other| other.title))
        .collect();
    let keys = key_label(&conflict.keys, words);
    Some(strings::fill(
        &words.key_conflict,
        &[&keys, &others.join(", ")],
    ))
}

/// A control's callback running `apply` with the edit `wrap` makes of the
/// value it hands back, as an edit of the setting `id`.
fn report<A, V>(
    apply: Option<Callback<A, SettingChange>>,
    id: &str,
    wrap: fn(V) -> SettingEdit,
) -> impl Fn(&mut A, V) + 'static
where
    A: 'static,
    V: 'static,
{
    let id = id.to_owned();
    move |app, value| {
        if let Some(apply) = &apply {
            let edit = wrap(value);
            apply(
                app,
                SettingChange {
                    edit,
                    id: id.clone(),
                },
            );
        }
    }
}

/// `setting`, holding the app's value now, as a row of a card: its label,
/// its description (or the names it may take too), and the control its
/// kind takes, worded in `strings`.
pub fn setting_row<'a, A>(setting: &'a Setting, strings: &'a Strings) -> SettingRow<'a, A> {
    SettingRow {
        on_change: None,
        scope: SettingScope::InForce,
        setting,
        strings,
    }
}

/// `section`'s settings as rows of a card under its title, worded in
/// `strings`.
pub fn settings_section<'a, A>(
    section: &'a SettingsSection,
    strings: &'a Strings,
) -> SettingsCard<'a, A> {
    SettingsCard {
        filter: "",
        on_change: None,
        scopes: None,
        section,
        strings,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::theme::Theme;
    use crate::testing::TreeWindow;
    use crate::view::element::column;
    use crate::widgets::command::Command;
    use crate::{App, WindowId};

    /// An app holding a section's values, recording every edit.
    struct Prefs {
        changes: Vec<SettingChange>,
        filter: String,
        own_value: bool,
        section: SettingsSection,
        theme: Theme,
    }

    impl App for Prefs {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let scope = |_: &str| match self.own_value {
                true => SettingScope::OwnValue,
                false => SettingScope::InForce,
            };
            let card = settings_section(&self.section, &self.theme.strings)
                .filter(&self.filter)
                .scopes(scope)
                .on_change(|app: &mut Self, change| app.changes.push(change));
            column([card.into()])
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    fn prefs(own_value: bool) -> Prefs {
        let section = SettingsSection::new("Status bar")
            .with(Setting::toggle("status.show", "Show status bar", true))
            .with(Setting::names(
                "status.segments",
                "Segments",
                vec!["tool".to_owned()],
                vec!["tool".to_owned(), "bell".to_owned()],
            ));
        Prefs {
            changes: Vec::new(),
            filter: String::new(),
            own_value,
            section,
            theme: Theme::dark(),
        }
    }

    /// An app showing a page of two sections and the keybindings.
    struct Page {
        filter: String,
        registry: CommandRegistry,
        scope: usize,
        scopes: Vec<String>,
        sections: Vec<SettingsSection>,
        shown: usize,
        theme: Theme,
    }

    impl App for Page {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            settings_page("Settings", &self.sections, &self.theme.strings)
                .shown(self.shown, |app: &mut Self, at| app.shown = at)
                .filter(&self.filter, |app: &mut Self, filter| app.filter = filter)
                .scopes(
                    &self.scopes,
                    self.scope,
                    |app: &mut Self, at| app.scope = at,
                    |_| SettingScope::InForce,
                )
                .keybindings("Keys", &self.registry, |_: &mut Self, _| {})
                .into()
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_the_page_shows_the_chosen_card_and_a_filter_shows_matches_from_every_card() {
        let app = Page {
            filter: String::new(),
            registry: CommandRegistry::new(COMMANDS).expect("a valid table"),
            scope: 0,
            scopes: vec!["User".to_owned(), "Workspace".to_owned()],
            sections: vec![
                SettingsSection::new("Editor").with(Setting::toggle("wrap", "Wrap lines", true)),
                SettingsSection::new("Window").with(Setting::toggle("dim", "Dim panes", false)),
            ],
            shown: 0,
            theme: Theme::dark(),
        };
        let mut window = TreeWindow::new(app, "t", 800, 400);
        let shown = window.drawn_text().concat();
        assert!(
            shown.contains("Wrap lines") && !shown.contains("Dim panes"),
            "{shown}"
        );
        window.update(|app| app.shown = 2);
        let shown = window.drawn_text().concat();
        assert!(
            shown.contains("New Tab") && !shown.contains("Wrap lines"),
            "{shown}"
        );
        window.update(|app| app.filter = "tab".to_owned());
        let shown = window.drawn_text().concat();
        assert!(shown.contains("New Tab"), "a command matches: {shown}");
        window.update(|app| app.filter = "pan".to_owned());
        let shown = window.drawn_text().concat();
        assert!(
            shown.contains("Dim panes") && !shown.contains("Wrap lines"),
            "{shown}"
        );
        assert!(shown.contains("Workspace"), "the scopes as tabs: {shown}");
    }

    /// Two commands sharing `C-t`, so their keys conflict.
    const COMMANDS: &[Command] = &[
        Command::new("tab.new", "New Tab", "Tabs").with_keys(&["C-t"]),
        Command::new("tab.reopen", "Reopen Tab", "Tabs").with_keys(&["C-t"]),
    ];

    /// An app holding a registry, rebound through the keybindings card.
    struct Keys {
        registry: CommandRegistry,
        theme: Theme,
    }

    impl App for Keys {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let card = keybindings_section("Keys", &self.registry, &self.theme.strings).on_change(
                |app: &mut Self, change| {
                    app.registry
                        .edit_keys(change.id, &change.edit)
                        .expect("a known command");
                },
            );
            column([card.into()])
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_a_conflict_is_named_and_a_pressed_key_rebinds_the_command() {
        let keys = Keys {
            registry: CommandRegistry::new(COMMANDS).expect("a valid table"),
            theme: Theme::dark(),
        };
        let mut window = TreeWindow::new(keys, "t", 600, 300);
        let shown = window.drawn_text().concat();
        assert!(
            shown.contains("Reopen Tab"),
            "the other claimant named: {shown}"
        );
        window.press("<cr> C-n");
        let rebound = window.app().registry.bindings(KeyScope::All);
        let new_tab = rebound
            .iter()
            .find(|row| row.id == "tab.new")
            .expect("listed");
        assert!(new_tab.overridden, "the user's key now");
        assert!(window.app().registry.conflicts(KeyScope::All).is_empty());
    }

    #[test]
    fn test_a_switch_reports_its_setting_and_a_names_row_offers_the_rest() {
        let mut window = TreeWindow::new(prefs(false), "t", 600, 300);
        window.press("SPC");
        let change = window.app().changes.last().cloned();
        assert_eq!(
            change,
            Some(SettingChange {
                edit: SettingEdit::Changed(SettingValue::Toggle(false)),
                id: "status.show".to_owned(),
            })
        );
        let shown = window.drawn_text().concat();
        assert!(shown.contains("bell"), "the name it may take too: {shown}");
        assert!(!shown.contains("Reset"), "in force, nothing to reset");
    }

    #[test]
    fn test_a_later_scopes_own_value_offers_reset_and_a_filter_keeps_its_matches() {
        let mut window = TreeWindow::new(prefs(true), "t", 600, 300);
        assert!(window.drawn_text().concat().contains("Reset"));
        window.press("<tab> SPC");
        let cleared = window
            .app()
            .changes
            .iter()
            .any(|change| change.edit == SettingEdit::Cleared);
        assert!(cleared, "{:?}", window.app().changes);
        let prefs = prefs(false);
        let card: SettingsCard<'_, Prefs> =
            settings_section(&prefs.section, &prefs.theme.strings).filter("segm");
        let shown: Vec<&str> = card.shown().map(Setting::id).collect();
        assert_eq!(shown, ["status.segments"]);
    }
}
