//! [`SettingsPage`]: an app's settings as a modal page, every control
//! reachable from the keyboard. A sidebar lists the sections and one
//! section's rows show at a time; each row is a [`Setting`] of a few kinds
//! (on or off, a choice, a number, text, a color) edited in place, and a
//! keybindings section lists every command of a [`CommandRegistry`] with
//! its keys, rebound by pressing the new chord.
//!
//! The app owns persistence: the page edits its own copy of the values and
//! answers [`SettingsPageResponse::Changed`] with the id of the setting that
//! changed, or [`SettingsPageResponse::KeysChanged`] after it rebound a command
//! in the registry; the app reads the value with [`SettingsPage::value`]
//! (or the registry's keys) and saves it however it saves its config.
//!
//! A page may keep its edits in one of several scopes, as VS Code's User
//! and Workspace tabs do ([`SettingsPage::with_scopes`]): the scopes show
//! as tabs beside the heading, `[` and `]` (or a click) switch between
//! them, and the page answers [`SettingsPageResponse::ScopeChanged`] so the
//! app loads that scope's values with [`SettingsPage::set_value`]. A row
//! the app marks with [`SettingsPage::set_overridden`] has a value of its
//! own in a later scope; in a later scope, Backspace on it answers
//! [`SettingsPageResponse::Cleared`] to drop that value rather than resetting
//! it to the default.
//!
//! The rows glide to a new scroll rather than jump; call the lent page's
//! [`Widget::about_to_wait`] so each step is drawn. While motion is
//! reduced ([`Frame::reduced_motion`]), another section, or a new filter,
//! they jump.

mod draw;
mod keys;
mod model;

use std::collections::BTreeSet;
use std::time::Instant;

use super::super::app::context::EventCx;
use super::super::app::frame::Frame;
use super::super::app::widget::{Lent, Outcome, Widget};
use super::super::base::fuzzy;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::Key;
use super::super::base::list::ListState;
use super::super::base::theme::Theme;
use super::super::input::event::{ImeInput, Input, PointerButton, WheelDelta};
use super::command::{CommandKeys, CommandRegistry, KeyScope};
use super::glide::RowGlide;
use super::list_keys::ListKeys;
use super::text_field::{TextField, TextFieldResponse};

pub(crate) use model::PAGE_STEPS;
pub use model::{NumberRange, Setting, SettingValue, SettingsSection};

// ========================================================================
// Constants
// ========================================================================

/// The rows one notch of a mouse wheel scrolls.
const WHEEL_ROWS: f32 = 3.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A modal settings page: sections of settings and, optionally, the
/// keybindings of a [`CommandRegistry`].
///
/// # Keys
///
/// The app hands the page every key while it is open
/// (through its [`Widget::handle`](crate::Widget::handle)). The rows and the sections move by the
/// Vim keys every list-like widget shares (see [`List`](crate::List)): Up
/// and Down (or `k` and `j`, `C-p` and `C-n`), `C-f` and `C-b` (Page Down
/// and Page Up), `C-d` and `C-u`, `C-e` and `C-y`, `H`, `M`, `L`, `g g`
/// and `G` (Home and End), `z z` and its kin, and a count before a motion
/// (`3j`); [`SettingsPage::keys`] tells what count or prefix is pending.
/// Tab moves between the sidebar and the rows. On a row, Enter or
/// Space flips a toggle, steps a choice, starts editing a number, text,
/// names (as words), or color in place (Enter applies, Esc cancels, or
/// applies too with [`SettingsPage::with_edits_kept_on_escape`]; `i` starts
/// such an edit as well), or starts capturing a command's keys; Left and
/// Right (or `h` and `l`) step a choice or a number, or turn a toggle off
/// and on; Backspace puts the default back.
/// In the keybindings section `/` filters the commands by fuzzy match.
/// Esc closes the page, except while a field is edited, keys are
/// captured, or a count or prefix is pending, which it drops instead.
///
/// While capturing, the next chord, Esc included, becomes the command's
/// only key at once; Backspace gives the command its default keys back and
/// Delete unbinds it. Since Esc is a key like any other there, the row
/// shows a Cancel button, and a click on it or anywhere else stops
/// capturing with the keys unchanged (and does nothing more). Keys two commands claim are drawn in the danger
/// color and named in the footer; nothing blocks a conflict, since the
/// same key may rightly mean different things in different layers.
#[derive(Clone, Debug)]
pub struct SettingsPage {
    /// What is being edited, if anything.
    editing: Option<Editing>,
    /// Whether Esc in a field applies the edit, as Enter does, rather than
    /// cancelling it.
    escape_keeps: bool,
    /// The keybindings filter.
    filter: TextField,
    /// Whether the sidebar or the rows have focus.
    focus: Focus,
    /// The rows' scroll as drawn, gliding to the shown section's.
    glide: RowGlide,
    /// The count and prefix typed toward a motion.
    keys: ListKeys,
    /// Where the page was drawn last, for the pointer.
    layout: Option<PageLayout>,
    /// Where the pointer is, or `None` outside the window.
    pointer: Option<Point>,
    /// The cursor and scroll of the shown section's rows.
    rows: ListState,
    /// The settings with a value of their own in a later scope, by id.
    overridden: BTreeSet<String>,
    /// The index of the scope edits are kept in.
    scope: usize,
    /// Where each scope tab was drawn last, for the pointer.
    scope_tabs: Vec<Rect>,
    /// The scopes edits may be kept in, by name, first the widest; empty
    /// for a page with one.
    scopes: Vec<String>,
    /// The index of the shown section.
    section: usize,
    /// The sections, in sidebar order.
    sections: Vec<Section>,
    /// The page's heading.
    title: String,
}

/// What became of a key or event offered to a [`SettingsPage`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum SettingsPageResponse {
    /// The setting with this id took a new value; read it with
    /// [`SettingsPage::value`] and save it in the page's scope.
    Changed(String),
    /// Backspace on the setting with this id, marked overridden, in a
    /// scope after the first: drop the scope's own value, then give the
    /// page the value now in force and clear the mark.
    Cleared(String),
    /// The user closed the page: drop it.
    Closed,
    /// The page used the key or event.
    Handled,
    /// The key is not the page's; the app may act on it.
    Ignored,
    /// The command with this id was rebound in the registry; save its
    /// keys.
    KeysChanged(&'static str),
    /// The page moved to the scope at this index: give it that scope's
    /// values and marks.
    ScopeChanged(usize),
}

/// One section of the page.
#[derive(Clone, Debug, PartialEq)]
enum Section {
    /// The commands of the registry and their keys, under this title.
    Keybindings(String),
    /// A group of settings.
    Settings(SettingsSection),
}

/// What is being edited.
#[derive(Clone, Debug)]
enum Editing {
    /// The focused command's next chord is its new key.
    Capture,
    /// The focused setting, as text.
    Field(Box<TextField>),
    /// The keybindings filter.
    Filter,
}

/// Which part of the page has focus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Focus {
    /// The rows of the shown section.
    Rows,
    /// The sidebar of sections.
    Sections,
}

/// Where the parts of the page were drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageLayout {
    /// The whole page, border included.
    page: Rect,
    /// The height of one row.
    row_height: f32,
    /// The rows of the shown section.
    rows: Rect,
    /// The section names.
    sidebar: Rect,
}

/// One row of the shown section, as drawn.
enum Row<'a> {
    /// A command and its keys.
    Binding(CommandKeys),
    /// A setting.
    Setting(&'a Setting),
}

// ========================================================================
// SettingsPage Implementations
// ========================================================================

impl SettingsPage {
    /// The page lent the app's `registry`, whose keybindings it lists and
    /// rebinds, as a [`Widget`] to hand inputs and draw.
    pub fn lend<'a>(
        &'a mut self,
        registry: &'a mut CommandRegistry,
    ) -> Lent<'a, Self, &'a mut CommandRegistry> {
        Lent::new(self, registry)
    }

    // --------------------------------------------------------------------
    // Building
    // --------------------------------------------------------------------

    /// An empty page headed `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            editing: None,
            escape_keeps: false,
            filter: TextField::new(),
            focus: Focus::Rows,
            glide: RowGlide::default(),
            keys: ListKeys::default(),
            layout: None,
            pointer: None,
            rows: ListState::new(0),
            overridden: BTreeSet::new(),
            scope: 0,
            scope_tabs: Vec::new(),
            scopes: Vec::new(),
            section: 0,
            sections: Vec::new(),
            title: title.into(),
        }
    }

    /// This page with `section` added at the end of its sidebar.
    pub fn with_section(mut self, section: SettingsSection) -> Self {
        self.sections.push(Section::Settings(section));
        self
    }

    /// This page with Esc in a field applying the edit, as Enter does,
    /// rather than cancelling it: for an app whose fields are left with
    /// Esc, as a modal editor's are.
    pub fn with_edits_kept_on_escape(self) -> Self {
        Self {
            escape_keeps: true,
            ..self
        }
    }

    /// This page keeping its edits in one of `scopes`, by name, the first
    /// (the widest, such as `User`) shown at first. One scope or none
    /// shows no tabs.
    pub fn with_scopes(self, scopes: Vec<String>) -> Self {
        Self { scopes, ..self }
    }

    /// The index of the scope edits are kept in.
    pub fn scope(&self) -> usize {
        self.scope
    }

    /// Moves to the scope at `index`, as loading a page opened in it,
    /// without answering [`SettingsPageResponse::ScopeChanged`]; `false` past
    /// the last scope.
    pub fn set_scope(&mut self, index: usize) -> bool {
        let found = index < self.scopes.len().max(1);
        if found {
            self.scope = index;
        }
        found
    }

    /// Marks the setting `id` as having a value of its own in a scope
    /// after the first, or not; `false` when no setting has `id`.
    pub fn set_overridden(&mut self, id: &str, overridden: bool) -> bool {
        let Some(id) = self.find(id).map(|setting| setting.id().to_owned()) else {
            return false;
        };
        match overridden {
            true => self.overridden.insert(id),
            false => self.overridden.remove(&id),
        };
        true
    }

    /// This page with a keybindings section titled `title` added at the end
    /// of its sidebar, listing the commands of the registry the page is
    /// lent with [`SettingsPage::lend`].
    pub fn with_keybindings(mut self, title: impl Into<String>) -> Self {
        self.sections.push(Section::Keybindings(title.into()));
        self
    }

    // --------------------------------------------------------------------
    // Values
    // --------------------------------------------------------------------

    /// The value now of the setting `id`, or `None` when there is none.
    pub fn value(&self, id: &str) -> Option<&SettingValue> {
        self.find(id).map(Setting::value)
    }

    /// Gives the setting `id` the value `value`, as when loading saved
    /// config, without answering [`SettingsPageResponse::Changed`]; a number is
    /// held to its range. Whether the setting took it: not when no setting
    /// has `id`, the value is of another kind, or names no option.
    pub fn set_value(&mut self, id: &str, value: SettingValue) -> bool {
        self.find_mut(id)
            .is_some_and(|setting| setting.set_value(value).is_ok())
    }

    /// Every setting, section by section, for saving them all.
    pub fn settings(&self) -> impl Iterator<Item = &Setting> + '_ {
        self.sections.iter().flat_map(|section| match section {
            Section::Keybindings(_) => [].iter(),
            Section::Settings(section) => section.settings.iter(),
        })
    }

    // --------------------------------------------------------------------
    // Input
    // --------------------------------------------------------------------

    /// Reacts to `key`, rebinding commands in `registry` from the
    /// keybindings section; a text being edited copies to and pastes from
    /// the context's clipboard. Hand the page every key while it is open.
    pub(crate) fn handle_key(
        &mut self,
        key: Key,
        registry: &mut CommandRegistry,
        cx: &mut EventCx<'_>,
    ) -> SettingsPageResponse {
        self.rows.set_count(self.row_count(registry));
        match self.editing.take() {
            Some(Editing::Capture) => self.capture_key(key, registry),
            Some(Editing::Field(field)) => self.field_key(field, key, cx),
            Some(Editing::Filter) => self.filter_key(key, registry, cx),
            None => self.browse_key(key),
        }
    }

    /// Reacts to an input method event while a text is edited (a setting
    /// or the keybindings filter): composed text shows underlined at the
    /// caret and committed text is typed.
    /// [`SettingsPageResponse::Ignored`] when nothing is edited or nothing
    /// changed.
    pub(crate) fn handle_ime(
        &mut self,
        ime: &ImeInput,
        registry: &CommandRegistry,
    ) -> SettingsPageResponse {
        let response = match &mut self.editing {
            Some(Editing::Field(field)) => field.handle_ime(ime),
            Some(Editing::Filter) => {
                let response = self.filter.handle_ime(ime);
                if response == TextFieldResponse::Changed {
                    self.refilter(registry);
                }
                response
            }
            Some(Editing::Capture) | None => TextFieldResponse::Ignored,
        };
        match response {
            TextFieldResponse::Changed | TextFieldResponse::Handled => {
                SettingsPageResponse::Handled
            }
            TextFieldResponse::Ignored => SettingsPageResponse::Ignored,
        }
    }

    /// The count and prefix typed toward a motion over the rows or
    /// sections, to show in a status bar or a
    /// [`WhichKey`](crate::WhichKey) popup.
    pub fn keys(&self) -> &ListKeys {
        &self.keys
    }

    /// The count and prefix typed toward a motion, to drop them with
    /// [`ListKeys::reset`].
    pub fn keys_mut(&mut self) -> &mut ListKeys {
        &mut self.keys
    }

    /// The caret's cell of the text being edited in the last frame, in
    /// physical pixels, for [`ImeFocus::follow`](crate::ImeFocus::follow);
    /// `None` when no text is edited, so the input method is off.
    pub fn cursor_area(&self) -> Option<Rect> {
        match &self.editing {
            Some(Editing::Field(field)) => field.cursor_area(),
            Some(Editing::Filter) => self.filter.cursor_area(),
            Some(Editing::Capture) | None => None,
        }
    }

    /// Reacts to pointer `input`: a click on a section shows
    /// it, a click on a row focuses and activates it (as Enter does), a
    /// click outside the page closes it, and the wheel scrolls the rows.
    /// While keys are captured, a click anywhere (the row's Cancel button
    /// or elsewhere) only stops capturing. The page is modal: every
    /// pointer press is its own, so any press or wheel scroll, like the
    /// window losing focus, drops a pending count or prefix. The text
    /// being edited takes the pointer as a [`TextField`] does, writing to
    /// and pasting from the clipboard's primary selection.
    fn handle_pointer(&mut self, input: &Input, cx: &mut EventCx<'_>) -> SettingsPageResponse {
        if self.keys.reset_on(input, true) {
            cx.request_redraw();
        }
        if self.text_event(cx, input) {
            return SettingsPageResponse::Handled;
        }
        match input {
            Input::PointerButton(button) => {
                self.pointer = Some(button.position);
                match button.pressed && button.button == PointerButton::Left {
                    true => {
                        cx.request_redraw();
                        self.press()
                    }
                    false => SettingsPageResponse::Handled,
                }
            }
            Input::PointerLeft => {
                self.pointer = None;
                SettingsPageResponse::Handled
            }
            Input::PointerMoved(position) => {
                self.pointer = Some(*position);
                SettingsPageResponse::Handled
            }
            Input::Wheel(wheel) => {
                let row_height = self.layout.map_or(1.0, |layout| layout.row_height.max(1.0));
                let rows = match wheel.delta {
                    WheelDelta::Lines(lines) => -lines.y * WHEEL_ROWS,
                    WheelDelta::Pixels(pixels) => -pixels.y / row_height,
                };
                self.rows.scroll_by(rows.round() as isize);
                cx.request_redraw();
                SettingsPageResponse::Handled
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_) => SettingsPageResponse::Ignored,
        }
    }

    /// Hands pointer `input` to the text being edited; whether it took it
    /// (a press placing its caret, a drag selecting).
    fn text_event(&mut self, cx: &mut EventCx<'_>, input: &Input) -> bool {
        let field = match &mut self.editing {
            Some(Editing::Field(field)) => &mut **field,
            Some(Editing::Filter) => &mut self.filter,
            Some(Editing::Capture) | None => return false,
        };
        field.handle_pointer(input, cx) != TextFieldResponse::Ignored
    }

    // --------------------------------------------------------------------
    // Lookups
    // --------------------------------------------------------------------

    /// Whether the shown section is the keybindings.
    fn is_bindings(&self) -> bool {
        matches!(
            self.sections.get(self.section),
            Some(Section::Keybindings(_))
        )
    }

    /// How many rows the shown section has.
    fn row_count(&self, registry: &CommandRegistry) -> usize {
        match self.sections.get(self.section) {
            Some(Section::Keybindings(_)) => self.binding_rows(registry).len(),
            Some(Section::Settings(section)) => section.settings.len(),
            None => 0,
        }
    }

    /// The rows of the shown section.
    fn rows_of<'a>(&'a self, registry: &CommandRegistry) -> Vec<Row<'a>> {
        match self.sections.get(self.section) {
            Some(Section::Keybindings(_)) => self
                .binding_rows(registry)
                .into_iter()
                .map(Row::Binding)
                .collect(),
            Some(Section::Settings(section)) => section.settings.iter().map(Row::Setting).collect(),
            None => Vec::new(),
        }
    }

    /// The commands the filter leaves, in table order.
    fn binding_rows(&self, registry: &CommandRegistry) -> Vec<CommandKeys> {
        let filter = self.filter.text();
        registry
            .bindings(KeyScope::All)
            .into_iter()
            .filter(|row| {
                filter.is_empty()
                    || fuzzy::score(filter, &format!("{}: {}", row.group, row.title)).is_some()
            })
            .collect()
    }

    /// The focused command, in the keybindings section.
    fn focused_binding(&self, registry: &CommandRegistry) -> Option<CommandKeys> {
        if !self.is_bindings() {
            return None;
        }
        self.binding_rows(registry)
            .into_iter()
            .nth(self.rows.cursor())
    }

    /// The focused setting, in a section of settings.
    fn focused_setting(&self) -> Option<&Setting> {
        match self.sections.get(self.section)? {
            Section::Keybindings(_) => None,
            Section::Settings(section) => section.settings.get(self.rows.cursor()),
        }
    }

    /// The focused setting, to change.
    fn focused_setting_mut(&mut self) -> Option<&mut Setting> {
        let cursor = self.rows.cursor();
        match self.sections.get_mut(self.section)? {
            Section::Keybindings(_) => None,
            Section::Settings(section) => section.settings.get_mut(cursor),
        }
    }

    /// The setting `id`.
    fn find(&self, id: &str) -> Option<&Setting> {
        self.settings().find(|setting| setting.id() == id)
    }

    /// The setting `id`, to change.
    fn find_mut(&mut self, id: &str) -> Option<&mut Setting> {
        self.sections
            .iter_mut()
            .flat_map(|section| match section {
                Section::Keybindings(_) => [].iter_mut(),
                Section::Settings(section) => section.settings.iter_mut(),
            })
            .find(|setting| setting.id() == id)
    }
}

impl Widget for Lent<'_, SettingsPage, &mut CommandRegistry> {
    type Output = Rect;
    type Response = SettingsPageResponse;

    /// Reacts to `input`: keys and input method text as the module
    /// documentation lists, rebinding commands in the lent registry from
    /// the keybindings section, and the pointer: a click on a section shows
    /// it, a click on a row focuses and activates it, a click outside the
    /// page closes it, and the wheel scrolls the rows. The page is modal:
    /// hand it every input while it is open. Asks for a redraw whenever it
    /// used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> SettingsPageResponse {
        let response = match input {
            Input::Ime(ime) => self.widget.handle_ime(ime, self.model),
            Input::Key(key) => self.widget.handle_key(*key, self.model, cx),
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => return self.widget.handle_pointer(input, cx),
        };
        if !matches!(response, SettingsPageResponse::Ignored) {
            cx.request_redraw();
        }
        response
    }

    /// Draws the page centered in `area` in the frame's overlay over a
    /// dimmed backdrop, the keybindings section from the lent registry,
    /// and returns where the page itself was drawn (for a
    /// [`FocusTree`](crate::FocusTree) trap).
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        self.widget.draw_page(frame, area, self.model, theme)
    }

    /// Asks for the next frame of a glide still under way and blinks the
    /// caret of the field being typed in, returning when the earlier is
    /// due; `None` at rest.
    fn about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let page = &mut *self.widget;
        let field = match &mut page.editing {
            Some(Editing::Field(line)) => line.about_to_wait(cx),
            Some(Editing::Capture | Editing::Filter) | None => None,
        };
        let glide = page.glide.about_to_wait(cx);
        let filter = page.filter.about_to_wait(cx);
        [glide, filter, field].into_iter().flatten().min()
    }
}

impl Outcome for SettingsPageResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::thread;

    use super::super::command::Command;
    use super::draw::{cancel_button, is_conflicted};
    use super::*;
    use crate::app::offscreen;
    use crate::base::color::{Rgb, Rgba};
    use crate::base::key::{parse_key, parse_key_sequence};
    use crate::base::scroll::DEFAULT_DURATION;
    use crate::base::strings::{self, Strings};
    use crate::base::text;
    use crate::testing::TestWindow;

    const COMMANDS: &[Command] = &[
        Command::new("tab.new", "New Tab", "Tabs").with_keys(&["C-t"]),
        Command::new("tab.close", "Close Tab", "Tabs").with_keys(&["C-w"]),
        Command::new("palette", "Command Palette", "View").with_keys(&["M-x"]),
    ];

    fn registry() -> CommandRegistry {
        CommandRegistry::new(COMMANDS).expect("a valid table")
    }

    /// A General section (wrap, theme, size, name, accent) and the
    /// keybindings.
    fn page() -> SettingsPage {
        SettingsPage::new("Settings")
            .with_section(
                SettingsSection::new("General")
                    .with(Setting::toggle("wrap", "Wrap lines", true))
                    .with(Setting::choice("theme", "Theme", &["Dark", "Light"], 0))
                    .with(Setting::number(
                        "size",
                        "Font size",
                        14.0,
                        NumberRange::new(8.0, 32.0, 1.0),
                    ))
                    .with(Setting::text("name", "Name", "misty"))
                    .with(Setting::color("accent", "Accent", Rgb::new(0, 0, 0xff))),
            )
            .with_keybindings("Keybindings")
    }

    /// Two sections of 60 toggles each, drawn once in a short frame.
    fn long_page(frame: &mut Frame, registry: &mut CommandRegistry) -> SettingsPage {
        let section = |title| {
            (0..60).fold(SettingsSection::new(title), |section, _| {
                section.with(Setting::toggle("row", "A row", false))
            })
        };
        let mut page = SettingsPage::new("Settings")
            .with_section(section("First"))
            .with_section(section("Second"));
        let bounds = frame.bounds();
        page.lend(registry).draw(frame, bounds, &Theme::dark());
        page
    }

    /// Hands `page` each key of `specs`, returning the last response.
    fn press(
        page: &mut SettingsPage,
        registry: &mut CommandRegistry,
        specs: &[&str],
    ) -> SettingsPageResponse {
        let mut last = SettingsPageResponse::Ignored;
        let mut window = TestWindow::new(400, 300);
        for spec in specs {
            let key = parse_key(spec).expect("a valid key");
            last = page.handle_key(key, registry, &mut window.cx());
        }
        last
    }

    /// [`page`] keeping its edits in a User or a Project scope.
    fn scoped_page() -> SettingsPage {
        page().with_scopes(vec!["User".into(), "Project".into()])
    }

    /// A page of one setting of names, `mode` and `branch` held of
    /// `mode`, `branch`, and `clock`.
    fn names_page() -> SettingsPage {
        let held = vec!["mode".to_owned(), "branch".to_owned()];
        let choices = ["mode", "branch", "clock"].map(String::from).to_vec();
        SettingsPage::new("Settings").with_section(
            SettingsSection::new("Status")
                .with(Setting::names("segments", "Segments", held, choices)),
        )
    }

    #[test]
    fn test_a_setting_of_names_offers_the_rest_and_is_edited_as_words() {
        let (mut page, mut registry) = (names_page(), registry());
        let words = Strings::english();
        let footer = page.footer(&registry, &[], &words);
        assert_eq!(
            footer,
            strings::fill(&words.settings_names_hint, &["clock"])
        );
        let typed = ["<return>", "<space>", "c", "l", "o", "c", "k", "<return>"];
        assert_eq!(
            press(&mut page, &mut registry, &typed),
            SettingsPageResponse::Changed("segments".into())
        );
        let held = ["mode", "branch", "clock"].map(String::from).to_vec();
        assert_eq!(page.value("segments"), Some(&SettingValue::Names(held)));
    }

    #[test]
    fn test_i_edits_a_typed_row_and_esc_keeps_the_edit_when_the_page_says_so() {
        let mut registry = registry();
        let mut kept = page().with_edits_kept_on_escape();
        assert_eq!(
            press(
                &mut kept,
                &mut registry,
                &["j", "j", "j", "i", "x", "<esc>"]
            ),
            SettingsPageResponse::Changed("name".into())
        );
        assert_eq!(
            kept.value("name"),
            Some(&SettingValue::Text("mistyx".into()))
        );
        let mut cancelled = page();
        press(
            &mut cancelled,
            &mut registry,
            &["j", "j", "j", "i", "x", "<esc>"],
        );
        assert_eq!(
            cancelled.value("name"),
            Some(&SettingValue::Text("misty".into()))
        );
        // `i` on a toggle is no edit.
        assert_eq!(
            press(&mut page(), &mut registry, &["i"]),
            SettingsPageResponse::Ignored
        );
    }

    #[test]
    fn test_brackets_switch_scopes_held_to_the_ends() {
        let (mut page, mut registry) = (scoped_page(), registry());
        assert_eq!(
            press(&mut page, &mut registry, &["["]),
            SettingsPageResponse::Handled
        );
        assert_eq!(
            press(&mut page, &mut registry, &["]"]),
            SettingsPageResponse::ScopeChanged(1)
        );
        assert_eq!(page.scope(), 1);
        assert_eq!(
            press(&mut page, &mut registry, &["]"]),
            SettingsPageResponse::Handled
        );
        assert_eq!(
            press(&mut page, &mut registry, &["["]),
            SettingsPageResponse::ScopeChanged(0)
        );
        let mut single = scoped_page().with_scopes(Vec::new());
        assert_eq!(
            press(&mut single, &mut registry, &["]"]),
            SettingsPageResponse::Ignored,
            "one scope: the key is not the page's"
        );
    }

    #[test]
    fn test_backspace_in_a_later_scope_clears_an_overridden_setting_only() {
        let (mut page, mut registry) = (scoped_page(), registry());
        assert!(page.set_overridden("wrap", true));
        assert!(!page.set_overridden("nope", true));
        assert_eq!(
            press(&mut page, &mut registry, &["<backspace>"]),
            SettingsPageResponse::Handled,
            "the first scope resets to the default, which wrap has"
        );
        press(&mut page, &mut registry, &["<space>"]);
        assert_eq!(page.value("wrap"), Some(&SettingValue::Toggle(false)));
        assert!(page.set_scope(1));
        assert!(!page.set_scope(2));
        assert_eq!(
            press(&mut page, &mut registry, &["<backspace>"]),
            SettingsPageResponse::Cleared("wrap".into())
        );
        assert_eq!(
            page.value("wrap"),
            Some(&SettingValue::Toggle(false)),
            "the app loads the value in force"
        );
        assert_eq!(
            press(&mut page, &mut registry, &["j", "<backspace>"]),
            SettingsPageResponse::Handled,
            "theme has no value of its own here"
        );
    }

    #[test]
    fn test_a_click_on_a_scope_tab_switches_to_it() {
        let mut frame = offscreen::plain_frame(800, 400);
        let (mut page, mut registry) = (scoped_page(), registry());
        let bounds = frame.bounds();
        page.lend(&mut registry)
            .draw(&mut frame, bounds, &Theme::dark());
        let [user, project] = page.scope_tabs[..] else {
            panic!("two tabs: {:?}", page.scope_tabs);
        };
        assert!(user.x + user.width <= project.x, "User before Project");
        let layout = page.layout.expect("drawn");
        assert!(
            project.x + project.width <= layout.page.x + layout.page.width,
            "inside the page"
        );
        page.pointer = Some(Point::new(project.x + 2.0, project.y + 2.0));
        assert_eq!(page.press(), SettingsPageResponse::ScopeChanged(1));
        assert_eq!(page.press(), SettingsPageResponse::Handled, "already there");
    }

    #[test]
    fn test_the_footer_names_an_overridden_setting_by_the_scope_shown() {
        let (mut page, registry) = (scoped_page(), registry());
        let words = Strings::english();
        let footer = |page: &SettingsPage| page.footer(&registry, &[], &words);
        assert!(footer(&page).ends_with(words.settings_scopes_hint.as_ref()));
        page.set_overridden("wrap", true);
        assert_eq!(footer(&page), words.settings_overridden);
        page.set_scope(1);
        assert_eq!(footer(&page), words.settings_own_value);
    }

    #[test]
    fn test_rows_glide_to_a_new_page_and_a_click_mid_glide_takes_the_row_drawn() {
        let mut frame = offscreen::plain_frame(800, 400);
        let mut registry = registry();
        let mut page = long_page(&mut frame, &mut registry);
        press(&mut page, &mut registry, &["<pagedown>"]);
        let scroll = page.rows.scroll();
        assert!(scroll > 0, "a page down scrolled");
        let bounds = frame.bounds();
        page.lend(&mut registry)
            .draw(&mut frame, bounds, &Theme::dark());
        let drawn = page.glide.drawn().expect("drawn");
        assert!(drawn < scroll as f32, "{drawn} still gliding to {scroll}");
        let layout = page.layout.expect("drawn");
        page.pointer = Some(Point::new(
            layout.rows.x + 1.0,
            layout.rows.y + layout.row_height / 2.0,
        ));
        page.press();
        assert_eq!(page.rows.cursor(), drawn.floor() as usize, "the row drawn");
    }

    #[test]
    fn test_another_section_jumps_to_its_top_instead_of_gliding() {
        let mut frame = offscreen::plain_frame(800, 400);
        let mut registry = registry();
        let mut page = long_page(&mut frame, &mut registry);
        press(&mut page, &mut registry, &["<pagedown>"]);
        let bounds = frame.bounds();
        page.lend(&mut registry)
            .draw(&mut frame, bounds, &Theme::dark());
        // Let the glide reach the page, so only a jump draws row 0 next.
        thread::sleep(DEFAULT_DURATION);
        let bounds = frame.bounds();
        page.lend(&mut registry)
            .draw(&mut frame, bounds, &Theme::dark());
        press(&mut page, &mut registry, &["<tab>", "j"]);
        let bounds = frame.bounds();
        page.lend(&mut registry)
            .draw(&mut frame, bounds, &Theme::dark());
        assert_eq!(page.glide.drawn(), Some(0.0));
    }

    #[test]
    fn test_keys_move_through_rows_and_sections() {
        let (mut page, mut registry) = (page(), registry());
        press(&mut page, &mut registry, &["j", "j", "<down>"]);
        assert_eq!(page.focused_setting().map(Setting::id), Some("name"));
        press(&mut page, &mut registry, &["<tab>", "j"]);
        assert!(
            page.is_bindings(),
            "Down in the sidebar shows the next section"
        );
        assert_eq!(
            page.rows.cursor(),
            0,
            "a new section starts at its first row"
        );
        press(&mut page, &mut registry, &["<cr>", "<end>"]);
        assert_eq!(
            page.focused_binding(&registry).map(|row| row.id),
            Some("palette")
        );
        assert_eq!(
            press(&mut page, &mut registry, &["<esc>"]),
            SettingsPageResponse::Closed
        );
    }

    #[test]
    fn test_vim_counts_and_prefixes_move_rows_and_sections_and_esc_drops_them_first() {
        let (mut page, mut registry) = (page(), registry());
        press(&mut page, &mut registry, &["3", "j"]);
        assert_eq!(page.focused_setting().map(Setting::id), Some("name"));
        press(&mut page, &mut registry, &["g", "g"]);
        assert_eq!(page.rows.cursor(), 0);
        assert_eq!(
            press(&mut page, &mut registry, &["g", "<esc>"]),
            SettingsPageResponse::Handled,
            "Esc drops the pending g"
        );
        assert!(!page.keys().is_pending());
        press(&mut page, &mut registry, &["<tab>", "G"]);
        assert!(
            page.is_bindings(),
            "G in the sidebar shows the last section"
        );
        press(&mut page, &mut registry, &["g", "g"]);
        assert!(!page.is_bindings());
        assert_eq!(
            press(&mut page, &mut registry, &["<esc>"]),
            SettingsPageResponse::Closed
        );
    }

    #[test]
    fn test_toggles_choices_and_numbers_change_in_place_and_reset() {
        let (mut page, mut registry) = (page(), registry());
        assert_eq!(
            press(&mut page, &mut registry, &["<space>"]),
            SettingsPageResponse::Changed("wrap".into())
        );
        assert_eq!(page.value("wrap"), Some(&SettingValue::Toggle(false)));
        assert_eq!(
            press(&mut page, &mut registry, &["l"]),
            SettingsPageResponse::Changed("wrap".into()),
            "Right turns a toggle on"
        );
        assert_eq!(
            press(&mut page, &mut registry, &["l"]),
            SettingsPageResponse::Handled,
            "already on"
        );
        press(&mut page, &mut registry, &["j", "<left>"]);
        assert_eq!(
            page.value("theme"),
            Some(&SettingValue::Choice(1)),
            "wrapped"
        );
        press(&mut page, &mut registry, &["j", "h", "h"]);
        assert_eq!(page.value("size"), Some(&SettingValue::Number(12.0)));
        assert_eq!(
            press(&mut page, &mut registry, &["<bs>"]),
            SettingsPageResponse::Changed("size".into())
        );
        assert_eq!(page.value("size"), Some(&SettingValue::Number(14.0)));
    }

    #[test]
    fn test_a_field_applies_on_enter_refuses_bad_input_and_cancels_on_esc() {
        let (mut page, mut registry) = (page(), registry());
        press(
            &mut page,
            &mut registry,
            &["j", "j", "<cr>", "C-u", "2", "0"],
        );
        assert_eq!(
            press(&mut page, &mut registry, &["<cr>"]),
            SettingsPageResponse::Changed("size".into())
        );
        assert_eq!(page.value("size"), Some(&SettingValue::Number(20.0)));
        press(
            &mut page,
            &mut registry,
            &["j", "j", "<cr>", "C-u", "#", "z"],
        );
        assert_eq!(
            press(&mut page, &mut registry, &["<cr>"]),
            SettingsPageResponse::Handled
        );
        assert!(
            matches!(page.editing, Some(Editing::Field(_))),
            "a bad color keeps the field open"
        );
        assert_eq!(
            press(&mut page, &mut registry, &["<esc>"]),
            SettingsPageResponse::Handled,
            "Esc cancels the field, not the page"
        );
        assert_eq!(
            page.value("accent"),
            Some(&SettingValue::Color(Rgb::new(0, 0, 0xff)))
        );
        assert!(page.editing.is_none());
    }

    #[test]
    fn test_capturing_rebinds_shows_conflicts_and_resets_to_default() {
        let (mut page, mut registry) = (page(), registry());
        press(
            &mut page,
            &mut registry,
            &["<tab>", "<end>", "<tab>", "<end>", "<cr>"],
        );
        assert_eq!(
            press(&mut page, &mut registry, &["C-t"]),
            SettingsPageResponse::KeysChanged("palette")
        );
        assert_eq!(
            registry.keys("tab.new"),
            [parse_key_sequence("C-t").expect("keys")]
        );
        assert_eq!(
            registry.keys("palette"),
            [parse_key_sequence("C-t").expect("keys")]
        );
        let conflicts = registry.conflicts(KeyScope::All);
        let palette = page.focused_binding(&registry).expect("a focused row");
        assert!(is_conflicted(&palette, &conflicts));
        assert_eq!(
            page.footer(&registry, &conflicts, &Strings::english()),
            "Ctrl+T is also bound to New Tab"
        );
        let german = Strings {
            key_conflict: "{1} nutzt schon {0}".into(),
            ..Strings::english()
        };
        assert_eq!(
            page.footer(&registry, &conflicts, &german),
            "New Tab nutzt schon Ctrl+T",
            "a translation orders the keys and commands its own way"
        );
        press(&mut page, &mut registry, &["<cr>", "<esc>"]);
        assert_eq!(
            registry.keys("palette"),
            [parse_key_sequence("<esc>").expect("keys")],
            "Esc is captured as a key, not closing the page"
        );
        press(&mut page, &mut registry, &["<cr>", "<del>"]);
        assert!(registry.keys("palette").is_empty(), "Delete unbinds");
        press(&mut page, &mut registry, &["<cr>", "<bs>"]);
        assert_eq!(
            registry.keys("palette"),
            [parse_key_sequence("M-x").expect("keys")]
        );
        assert!(registry.conflicts(KeyScope::All).is_empty());
    }

    #[test]
    fn test_the_filters_caret_blinks_while_it_is_typed_in() {
        let (mut page, mut registry) = (page(), registry());
        let mut window = TestWindow::new(400, 300);
        window.set_reduced_motion(true);
        let mut drawn = |page: &mut SettingsPage, registry: &mut CommandRegistry| {
            let mut frame = window.frame();
            let bounds = frame.bounds();
            page.lend(registry).draw(&mut frame, bounds, &Theme::dark());
            page.lend(registry).about_to_wait(&mut window.cx())
        };
        press(&mut page, &mut registry, &["<tab>", "j"]);
        assert_eq!(drawn(&mut page, &mut registry), None, "at rest");
        press(&mut page, &mut registry, &["/"]);
        assert!(
            drawn(&mut page, &mut registry).is_some(),
            "the caret blinks"
        );
    }

    #[test]
    fn test_the_filter_narrows_the_commands_and_esc_clears_it() {
        let (mut page, mut registry) = (page(), registry());
        press(
            &mut page,
            &mut registry,
            &["<tab>", "j", "/", "c", "l", "o"],
        );
        assert_eq!(page.rows.count(), 1);
        assert_eq!(
            page.focused_binding(&registry).map(|row| row.id),
            Some("tab.close")
        );
        press(&mut page, &mut registry, &["<esc>"]);
        assert_eq!(page.rows.count(), COMMANDS.len());
    }

    #[test]
    fn test_loading_a_value_checks_its_kind_and_answers_nothing() {
        let mut page = page();
        assert!(page.set_value("theme", SettingValue::Choice(1)));
        assert!(!page.set_value("theme", SettingValue::Choice(5)));
        assert!(!page.set_value("wrap", SettingValue::Text("yes".to_owned())));
        assert!(!page.set_value("nope", SettingValue::Toggle(true)));
        assert_eq!(page.settings().count(), 5);
    }

    #[test]
    fn test_the_page_draws_its_focus_and_a_conflicting_key_in_danger() {
        let theme = Theme::dark();
        let (mut page, mut registry) = (page(), registry());
        registry
            .override_keys("tab.close", &["C-t"])
            .expect("a known id");
        press(&mut page, &mut registry, &["<tab>", "j", "<tab>"]);
        let (width, height) = (720, 400);
        let Some(snapshot) = offscreen::render(width, height, |frame| {
            frame.clear(Rgba::from(theme.background));
            let bounds = frame.bounds();
            page.lend(&mut registry).draw(frame, bounds, &theme);
        }) else {
            return;
        };
        let layout = page.layout.expect("drawn");
        let middle = |area: Rect, row: f32| area.y + (row + 0.5) * layout.row_height;
        snapshot.assert_pixel(
            layout.sidebar.x + layout.sidebar.width - 2.0,
            middle(layout.sidebar, 1.0),
            theme.hover,
            "the shown section",
        );
        let rows = layout.rows;
        snapshot.assert_pixel(rows.x, middle(rows, 0.0), theme.accent, "the focus bar");
        snapshot.assert_pixel(
            rows.x + rows.width - 2.0,
            middle(rows, 0.0),
            theme.hover,
            "the focused row",
        );
        let value_row = Rect::new(
            rows.x + rows.width / 2.0,
            rows.y,
            rows.width / 2.0,
            layout.row_height,
        );
        assert!(
            snapshot.contains_color(value_row, theme.danger),
            "New Tab's key is claimed twice"
        );
        let page_rect = layout.page;
        snapshot.assert_pixel(
            page_rect.x + 2.0,
            page_rect.y + page_rect.height - 2.0,
            theme.surface,
            "the page",
        );
    }

    #[test]
    fn test_a_click_anywhere_stops_capturing_without_rebinding_or_closing() {
        let (mut page, mut registry) = (page(), registry());
        press(
            &mut page,
            &mut registry,
            &["<tab>", "<end>", "<tab>", "<end>", "<cr>"],
        );
        assert!(matches!(page.editing, Some(Editing::Capture)));
        page.layout = Some(PageLayout {
            page: Rect::new(100.0, 100.0, 400.0, 300.0),
            row_height: 20.0,
            rows: Rect::new(250.0, 140.0, 250.0, 200.0),
            sidebar: Rect::new(100.0, 140.0, 150.0, 200.0),
        });
        page.pointer = Some(Point::new(10.0, 10.0));
        assert_eq!(
            page.press(),
            SettingsPageResponse::Handled,
            "outside the page, the click cancels instead of closing"
        );
        assert!(page.editing.is_none());
        assert_eq!(
            registry.keys("palette"),
            [parse_key_sequence("M-x").expect("keys")],
            "the keys are unchanged"
        );
        assert_eq!(page.press(), SettingsPageResponse::Closed, "then it closes");
    }

    #[test]
    fn test_a_capturing_row_draws_a_cancel_button_at_its_right_end() {
        let theme = Theme::dark();
        let (mut page, mut registry) = (page(), registry());
        press(&mut page, &mut registry, &["<tab>", "j", "<tab>", "<cr>"]);
        let (width, height) = (720, 400);
        let Some(snapshot) = offscreen::render(width, height, |frame| {
            frame.clear(Rgba::from(theme.background));
            let bounds = frame.bounds();
            page.lend(&mut registry).draw(frame, bounds, &theme);
        }) else {
            return;
        };
        let layout = page.layout.expect("drawn");
        let button = cancel_button(
            layout.rows,
            layout.rows.y,
            snapshot.metrics(),
            text::width(&Strings::english().cancel) as f32 * snapshot.metrics().width,
        );
        snapshot.assert_pixel(button.x + 0.5, button.y + 0.5, theme.border, "its outline");
        snapshot.assert_pixel(button.x + 3.0, button.y + 3.0, theme.surface, "the button");
        snapshot.assert_pixel(
            button.x - 2.0,
            button.y + 1.0,
            theme.hover,
            "the row left of it",
        );
        assert!(
            snapshot.contains_color(button, theme.foreground),
            "its label"
        );
    }
}
