//! How a [`SettingsPage`] answers keys: moving through the rows and
//! sections, editing a row in place, capturing a new chord for a command,
//! and typing a filter, and the actions those keys take.

use super::super::super::app::context::EventCx;
use super::super::super::app::widget::Widget;
use super::super::super::base::geometry::Rect;
use super::super::super::base::key::{Key, KeyCode, Mods};
use super::super::super::base::list::ListState;
use super::super::command::CommandRegistry;
use super::super::list_keys::{CountedKey, ListKey, Motion, Read};
use super::super::text_field::{TextField, TextFieldResponse};
use super::model::SettingKind;
use super::{Editing, Focus, Setting, SettingValue, SettingsPage, SettingsPageResponse};

// ========================================================================
// SettingsPage Implementations
// ========================================================================

impl SettingsPage {
    // --------------------------------------------------------------------
    // Keys
    // --------------------------------------------------------------------

    /// A key while nothing is edited: move, step, activate, reset, or
    /// close.
    pub(super) fn browse_key(&mut self, key: Key) -> SettingsPageResponse {
        match self.keys.read(key) {
            Read::Consumed => return SettingsPageResponse::Handled,
            Read::Key(CountedKey {
                count,
                key: ListKey::Move(motion),
            }) => return self.move_focus(motion, count),
            Read::Ignored | Read::Key(_) => {}
        }
        let plain = key.mods == Mods::NONE;
        let shifted = key.mods == Mods::SHIFT;
        match key.code {
            KeyCode::Esc if plain => SettingsPageResponse::Closed,
            KeyCode::Char(']') if plain && self.scopes.len() > 1 => self.step_scope(1),
            KeyCode::Char('[') if plain && self.scopes.len() > 1 => self.step_scope(-1),
            KeyCode::Tab if plain || shifted => {
                self.focus = match self.focus {
                    Focus::Rows => Focus::Sections,
                    Focus::Sections => Focus::Rows,
                };
                SettingsPageResponse::Handled
            }
            KeyCode::Char('/') if self.is_bindings() => {
                self.editing = Some(Editing::Filter);
                SettingsPageResponse::Handled
            }
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::End
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::Up => match self.focus {
                Focus::Rows => self.row_key(key),
                Focus::Sections => self.sections_key(key),
            },
        }
    }

    /// A key on the focused row that is not a move: activate, step, or
    /// reset it.
    fn row_key(&mut self, key: Key) -> SettingsPageResponse {
        if key.mods != Mods::NONE {
            return SettingsPageResponse::Ignored;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => self.activate(),
            KeyCode::Char('i') if self.focused_setting().is_some_and(Setting::is_typed) => {
                self.activate()
            }
            KeyCode::Left | KeyCode::Char('h') => self.step(-1),
            KeyCode::Right | KeyCode::Char('l') => self.step(1),
            KeyCode::Backspace => self.reset_focused(),
            KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::End
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Tab
            | KeyCode::Up => SettingsPageResponse::Ignored,
        }
    }

    /// A key while the sidebar has focus and it is not a move: Enter,
    /// Space, Right, or `l` go into the rows.
    fn sections_key(&mut self, key: Key) -> SettingsPageResponse {
        let into_rows = key.mods == Mods::NONE
            && matches!(
                key.code,
                KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ' | 'l')
            );
        match into_rows {
            true => {
                self.focus = Focus::Rows;
                SettingsPageResponse::Handled
            }
            false => SettingsPageResponse::Ignored,
        }
    }

    /// The next chord while keys are captured: the command's new key, or
    /// Backspace to reset it and Delete to unbind it.
    pub(super) fn capture_key(
        &mut self,
        key: Key,
        registry: &mut CommandRegistry,
    ) -> SettingsPageResponse {
        let Some(id) = self.focused_binding(registry).map(|row| row.id) else {
            return SettingsPageResponse::Handled;
        };
        let plain = key.mods == Mods::NONE;
        match key.code {
            KeyCode::Backspace if plain => registry.reset_keys(id),
            KeyCode::Delete if plain => {
                // The id is the registry's own, so it is known.
                let _ = registry.override_keys(id, &[]);
            }
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::End
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::Up => {
                if registry.override_keys(id, &[&key.to_string()]).is_err() {
                    // A key the notation cannot write back: keep capturing.
                    self.editing = Some(Editing::Capture);
                    return SettingsPageResponse::Handled;
                }
            }
        }
        // The row may have left a filtered list; keep the cursor on it.
        self.rows.set_count(self.row_count(registry));
        SettingsPageResponse::KeysChanged(id)
    }

    /// A key while the focused setting is edited in `line`: Enter applies
    /// a value that reads as the setting's kind, Esc cancels, and the rest
    /// edits the text.
    pub(super) fn field_key(
        &mut self,
        mut line: Box<TextField>,
        key: Key,
        cx: &mut EventCx<'_>,
    ) -> SettingsPageResponse {
        let plain = key.mods == Mods::NONE;
        match key.code {
            KeyCode::Esc if plain && !self.escape_keeps => SettingsPageResponse::Handled,
            KeyCode::Enter | KeyCode::Esc if plain => {
                let Some(setting) = self.focused_setting_mut() else {
                    return SettingsPageResponse::Handled;
                };
                let Some(value) = setting.parse(line.text()) else {
                    // Refused: the field stays open to be fixed.
                    self.editing = Some(Editing::Field(line));
                    return SettingsPageResponse::Handled;
                };
                let id = setting.id().to_owned();
                match setting.set_value(value) {
                    Ok(true) => SettingsPageResponse::Changed(id),
                    Ok(false) | Err(_) => SettingsPageResponse::Handled,
                }
            }
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::End
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::Up => {
                line.handle_key(key, cx);
                self.editing = Some(Editing::Field(line));
                SettingsPageResponse::Handled
            }
        }
    }

    /// A key while the filter line has focus: Enter keeps the filter, Esc
    /// clears it, Up and Down move through the rows it leaves, and the rest
    /// edits it.
    pub(super) fn filter_key(
        &mut self,
        key: Key,
        registry: &CommandRegistry,
        cx: &mut EventCx<'_>,
    ) -> SettingsPageResponse {
        let plain = key.mods == Mods::NONE;
        match key.code {
            KeyCode::Enter if plain => {
                self.focus = Focus::Rows;
                return SettingsPageResponse::Handled;
            }
            KeyCode::Esc if plain => {
                self.filter.set_text("");
                self.rows.set_count(self.row_count(registry));
                return SettingsPageResponse::Handled;
            }
            KeyCode::Down if plain => self.rows.move_cursor(1),
            KeyCode::Up if plain => self.rows.move_cursor(-1),
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::End
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::Up => {
                if self.filter.handle_key(key, cx) == TextFieldResponse::Changed {
                    self.refilter(registry);
                }
            }
        }
        self.editing = Some(Editing::Filter);
        SettingsPageResponse::Handled
    }

    /// Shows the rows a changed filter leaves, from the top.
    pub(super) fn refilter(&mut self, registry: &CommandRegistry) {
        self.rows.set_count(self.row_count(registry));
        self.rows.set_cursor(0);
        self.glide.jump_next_frame();
    }

    // --------------------------------------------------------------------
    // Actions
    // --------------------------------------------------------------------

    /// Moves the focus by `motion` over the rows, or over the sections in
    /// the sidebar, as a list moves.
    fn move_focus(&mut self, motion: Motion, count: Option<usize>) -> SettingsPageResponse {
        match self.focus {
            Focus::Rows => self.keys.move_cursor(&mut self.rows, motion, count),
            Focus::Sections => {
                let mut sections = ListState::new(self.sections.len());
                sections.set_visible(self.sections.len());
                sections.set_cursor(self.section);
                self.keys.move_cursor(&mut sections, motion, count);
                self.show_section(sections.cursor());
            }
        }
        SettingsPageResponse::Handled
    }

    /// Shows section `index` with its first row focused, jumping to it.
    fn show_section(&mut self, index: usize) {
        if index != self.section {
            self.section = index;
            self.rows.set_cursor(0);
            self.glide.jump_next_frame();
        }
    }

    /// What Enter does on the focused row: flip a toggle, step a choice,
    /// edit a number, text, or color, or capture a command's keys.
    fn activate(&mut self) -> SettingsPageResponse {
        if self.is_bindings() {
            if self.rows.count() > 0 {
                self.editing = Some(Editing::Capture);
            }
            return SettingsPageResponse::Handled;
        }
        let Some(setting) = self.focused_setting_mut() else {
            return SettingsPageResponse::Handled;
        };
        match (setting.kind(), setting.value()) {
            (SettingKind::Toggle, SettingValue::Toggle(on)) => {
                let flipped = SettingValue::Toggle(!on);
                let id = setting.id().to_owned();
                match setting.set_value(flipped) {
                    Ok(true) => SettingsPageResponse::Changed(id),
                    Ok(false) | Err(_) => SettingsPageResponse::Handled,
                }
            }
            (SettingKind::Choice(_), _) => {
                let id = setting.id().to_owned();
                match setting.step(1) {
                    true => SettingsPageResponse::Changed(id),
                    false => SettingsPageResponse::Handled,
                }
            }
            (
                SettingKind::Color
                | SettingKind::Names(_)
                | SettingKind::Number(_)
                | SettingKind::Text,
                _,
            ) => {
                let mut line = Box::new(TextField::with_text(&setting.edit_text()));
                line.set_focused(true);
                self.editing = Some(Editing::Field(line));
                SettingsPageResponse::Handled
            }
            (SettingKind::Toggle, _) => SettingsPageResponse::Handled,
        }
    }

    /// Steps the focused setting `delta` places.
    fn step(&mut self, delta: isize) -> SettingsPageResponse {
        let Some(setting) = self.focused_setting_mut() else {
            return SettingsPageResponse::Handled;
        };
        match setting.step(delta) {
            true => SettingsPageResponse::Changed(setting.id().to_owned()),
            false => SettingsPageResponse::Handled,
        }
    }

    /// Puts the focused setting's default back.
    fn reset_focused(&mut self) -> SettingsPageResponse {
        if self.scope > 0 {
            return match self.focused_setting().map(Setting::id) {
                Some(id) if self.overridden.contains(id) => {
                    SettingsPageResponse::Cleared(id.to_owned())
                }
                Some(_) | None => SettingsPageResponse::Handled,
            };
        }
        let Some(setting) = self.focused_setting_mut() else {
            return SettingsPageResponse::Handled;
        };
        match setting.reset() {
            true => SettingsPageResponse::Changed(setting.id().to_owned()),
            false => SettingsPageResponse::Handled,
        }
    }

    /// Moves to the scope `delta` places on, held to the ends.
    fn step_scope(&mut self, delta: isize) -> SettingsPageResponse {
        let last = self.scopes.len().saturating_sub(1);
        let scope = self.scope.saturating_add_signed(delta).min(last);
        self.show_scope(scope)
    }

    /// Moves to the scope at `index`, answering
    /// [`SettingsPageResponse::ScopeChanged`] when it is another one.
    fn show_scope(&mut self, index: usize) -> SettingsPageResponse {
        if index == self.scope {
            return SettingsPageResponse::Handled;
        }
        self.scope = index;
        self.editing = None;
        SettingsPageResponse::ScopeChanged(index)
    }

    /// A left press at the pointer.
    pub(super) fn press(&mut self) -> SettingsPageResponse {
        if matches!(self.editing, Some(Editing::Capture)) {
            self.editing = None;
            return SettingsPageResponse::Handled;
        }
        let (Some(point), Some(layout)) = (self.pointer, self.layout) else {
            return SettingsPageResponse::Handled;
        };
        if !layout.page.contains(point) {
            return SettingsPageResponse::Closed;
        }
        if let Some(index) = self.scope_tabs.iter().position(|tab| tab.contains(point)) {
            return self.show_scope(index);
        }
        let row_at = |area: Rect, scroll: f32| {
            let row = (scroll + (point.y - area.y) / layout.row_height.max(1.0)).floor();
            area.contains(point).then_some(row as usize)
        };
        if let Some(offset) = row_at(layout.sidebar, 0.0) {
            self.editing = None;
            if offset < self.sections.len() {
                self.focus = Focus::Sections;
                self.show_section(offset);
            }
            return SettingsPageResponse::Handled;
        }
        // The rows at the scroll drawn.
        let drawn = self.glide.drawn().unwrap_or_default();
        if let Some(row) = row_at(layout.rows, drawn) {
            self.editing = None;
            if row < self.rows.count() {
                self.focus = Focus::Rows;
                self.rows.set_cursor(row);
                return self.activate();
            }
        }
        SettingsPageResponse::Handled
    }
}
