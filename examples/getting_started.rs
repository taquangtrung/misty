//! The app the getting-started guide (`docs/getting-started.md`) builds: a
//! list of notes. `a` (or the Add Note button) asks for a new note in a
//! dialog with a text field; `d` (or Delete) asks whether to delete the
//! note under the cursor, in a warning dialog whose buttons answer to their
//! letters too; `q` quits. Up and Down (or `j` and `k`) move the cursor, a
//! click moves it too, and Tab moves between the list and the buttons.
//!
//! ```sh
//! cargo run -p misty --example getting_started
//! ```

use misty::view::{button, column, dialog, list, row, text};
use misty::{
    App, AppCx, Element, Key, KeyCode, Mods, Paragraph, Severity, Theme, ThemeToken, WindowId,
    WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "Notes";

/// The blank cells around the content, in columns and rows.
const MARGIN: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The whole app: its notes, where the cursor is, and what it is asking.
struct Notes {
    /// The question open over the window, if any.
    asking: Asking,
    /// The row the list's cursor is on.
    cursor: usize,
    /// What has been typed for a new note so far.
    draft: String,
    /// The notes, one a row.
    notes: Vec<String>,
    /// The colors the window is drawn in.
    theme: Theme,
}

/// A question the app is asking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Asking {
    /// Whether to delete the note under the cursor.
    Delete,
    /// What a new note should say.
    NewNote,
    /// Nothing: no dialog is open.
    Nothing,
}

// ========================================================================
// Notes Implementations
// ========================================================================

impl Notes {
    /// The app at start, with a few notes.
    fn new() -> Self {
        Self {
            asking: Asking::Nothing,
            cursor: 0,
            draft: String::new(),
            notes: ["Water the plants", "Call Ana back", "Read the misty guide"]
                .map(String::from)
                .to_vec(),
            theme: Theme::dark(),
        }
    }

    /// Adds the draft as a note, if it says anything, and closes the dialog.
    fn add(&mut self) {
        let note = self.draft.trim();
        if !note.is_empty() {
            self.notes.push(note.to_owned());
            self.cursor = self.notes.len() - 1;
        }
        self.draft.clear();
        self.asking = Asking::Nothing;
    }

    /// Deletes the note under the cursor and closes the dialog.
    fn delete(&mut self) {
        if self.cursor < self.notes.len() {
            self.notes.remove(self.cursor);
            self.cursor = self.cursor.min(self.notes.len().saturating_sub(1));
        }
        self.asking = Asking::Nothing;
    }

    /// Closes whatever dialog is open, doing nothing.
    fn cancel(&mut self) {
        self.draft.clear();
        self.asking = Asking::Nothing;
    }

    /// Asks whether to delete the note under the cursor, when there is one.
    fn ask_delete(&mut self) {
        if !self.notes.is_empty() {
            self.asking = Asking::Delete;
        }
    }

    /// The dialog asking what a new note should say.
    fn new_note_dialog(&self) -> misty::view::Dialog<'_, Self> {
        dialog("New note", Vec::new())
            .field(&self.draft, |app: &mut Self, typed| app.draft = typed)
            .field_placeholder("What to remember")
            .button("Cancel", Self::cancel)
            .primary("Add", Self::add)
            .on_close(Self::cancel)
    }

    /// The dialog asking whether to delete the note under the cursor.
    fn delete_dialog(&self) -> misty::view::Dialog<'_, Self> {
        let note = self.notes.get(self.cursor).map_or("", String::as_str);
        let body = vec![Paragraph::plain(
            format!("\u{201c}{note}\u{201d} will be gone for good."),
            ThemeToken::Foreground.into(),
        )];
        dialog("Delete this note?", body)
            .severity(Severity::Warning)
            .button("Cancel", Self::cancel)
            .key('c')
            .primary("Delete", Self::delete)
            .key('d')
            .on_close(Self::cancel)
    }
}

impl App for Notes {
    /// The window, described afresh from the app's state each time it
    /// changes: the list, the buttons, a status line, and the dialog open.
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        let status = format!(
            "{} notes \u{00b7} a adds \u{00b7} d deletes \u{00b7} q quits",
            self.notes.len()
        );
        column([
            list(&self.notes, self.cursor)
                .on_cursor_move(|app: &mut Self, row| app.cursor = row)
                .into(),
            row([
                button("Add Note")
                    .on_click(|app: &mut Self| app.asking = Asking::NewNote)
                    .into(),
                button("Delete").on_click(Self::ask_delete).into(),
            ]),
            text(status).muted().into(),
        ])
        .padding(MARGIN, MARGIN)
        .overlay_if(self.asking == Asking::NewNote, self.new_note_dialog())
        .overlay_if(self.asking == Asking::Delete, self.delete_dialog())
    }

    /// The app's own keys: `a`, `d`, and `q`. A key no element took comes
    /// here; returning `false` leaves it unused.
    fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
        if key.mods != Mods::NONE {
            return false;
        }
        match key.code {
            KeyCode::Char('a') => self.asking = Asking::NewNote,
            KeyCode::Char('d') => self.ask_delete(),
            KeyCode::Char('q') => cx.exit(),
            _ => return false,
        }
        true
    }

    fn theme(&self) -> &Theme {
        &self.theme
    }
}

// ========================================================================
// Functions
// ========================================================================

fn main() -> Result<(), misty::Error> {
    misty::run_app(Notes::new(), WindowOptions::new(TITLE))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, feature = "testing"))]
mod tests {
    use misty::accesskit::Role;
    use misty::testing::TreeWindow;

    use super::*;

    /// The app in a headless window.
    fn window() -> TreeWindow<Notes> {
        TreeWindow::new(Notes::new(), TITLE, 800, 600)
    }

    #[test]
    fn test_a_note_typed_into_the_dialog_is_added_under_the_cursor() {
        let mut window = window();
        window.press("a");
        window.type_text("Buy bread");
        window.press("RET");
        let notes = &window.app().notes;
        assert_eq!(notes.last().map(String::as_str), Some("Buy bread"));
        assert_eq!(window.app().cursor, notes.len() - 1, "the cursor on it");
        assert_eq!(window.app().asking, Asking::Nothing, "the dialog closed");
    }

    #[test]
    fn test_d_then_its_letter_deletes_the_note_and_c_keeps_it() {
        let mut window = window();
        window.press("d c");
        assert_eq!(window.app().notes.len(), 3, "Cancel kept it");
        window.press("d d");
        assert_eq!(
            window.app().notes,
            ["Call Ana back", "Read the misty guide"],
            "Delete took the first"
        );
    }

    #[test]
    fn test_the_buttons_work_with_the_pointer_and_screen_readers() {
        let mut window = window();
        window.click(Role::Button, "Delete");
        assert_eq!(window.app().asking, Asking::Delete);
        window.click(Role::Button, "Cancel");
        assert_eq!(window.app().asking, Asking::Nothing);
        assert_eq!(window.app().notes.len(), 3);
    }
}
