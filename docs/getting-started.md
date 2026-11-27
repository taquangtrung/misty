# Getting started

This guide builds a small but complete misty app, a list of notes, one idea at a time: a window, state and the view, a list, buttons, dialogs, keys, and tests that run without a display. The finished app is the `getting_started` example; run it to see where the guide is going:

```sh
cargo run -p misty --example getting_started
```

`a` adds a note through a dialog with a text field, `d` asks whether to delete the note under the cursor, and `q` quits. Every snippet below is taken from `examples/getting_started.rs`, which is compiled and tested with misty.

## 1. Add misty

```sh
cargo add misty
```

For the headless tests in section 8, add misty again as a dev-dependency with its `testing` feature:

```toml
[dev-dependencies]
misty = { version = "0.0", features = ["testing"] }
```

## 2. A window

An app is a type that implements `App`. Its one required method, `view`, describes the window as a tree of elements; `run_app` opens the window and keeps it in step with the app:

```rust
use misty::view::text;
use misty::{App, Element, Theme, WindowId, WindowOptions};

struct Notes {
    theme: Theme,
}

impl App for Notes {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        text("No notes yet").into()
    }

    fn theme(&self) -> &Theme {
        &self.theme
    }
}

fn main() -> Result<(), misty::Error> {
    let notes = Notes { theme: Theme::dark() };
    misty::run_app(notes, WindowOptions::new("Notes"))
}
```

misty draws everything itself, on the GPU, so the window looks the same on every platform. `Theme::dark()` and `Theme::light()` are built in; a theme can also be read from a KDL file (the `kdl` feature).

## 3. State and the view

The app holds its state as plain fields. `view` reads that state and returns a fresh description of the window; misty compares it with the last one and keeps each widget (its scroll position, its focus) between descriptions. Elements never change the state themselves: they call back into the app with `&mut Self`, and misty asks for the view again afterwards.

```rust
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Asking {
    Delete,
    NewNote,
    Nothing,
}
```

Containers lay elements out: `column` stacks them top to bottom, `row` puts them side by side, and `.padding(columns, rows)` keeps blank cells around them.

Text, and the rows of lists, tables, and trees, is set in the proportional UI font. `.monospace()` puts it on the monospace cell grid instead, for code, keys, or columns that line up by character.

## 4. A list

A `Vec<String>` is already a list's rows. `list` takes the rows and the row the cursor is on; `on_cursor_move` hears where the user moved it, by key or click:

```rust
list(&self.notes, self.cursor)
    .on_cursor_move(|app: &mut Self, row| app.cursor = row)
    .into(),
```

The list brings its own keys, as a keyboard-first app expects: Up and Down or `j` and `k`, Page Up and Page Down, Home and End, and a count before a motion (`5j`). It draws only the rows on screen, so it is as fast with a hundred thousand rows as with ten. For rows of your own type, implement `ListSource` (`row_count` and `row_text`).

## 5. Buttons

```rust
row([
    button("Add Note")
        .on_click(|app: &mut Self| app.asking = Asking::NewNote)
        .into(),
    button("Delete").on_click(Self::ask_delete).into(),
]),
```

A callback can be a closure or a method of the app, such as `Self::ask_delete`. Tab and Shift+Tab move the keyboard between the list and the buttons; Enter or Space presses the focused button; the pointer lights a button under it and a click presses it. Screen readers see each button by its label, with nothing more to write.

## 6. Dialogs

A dialog floats over the window. `overlay_if` shows it while a condition holds, so opening and closing a dialog is changing a field:

```rust
column([/* the list, the buttons, the status line */])
    .padding(MARGIN, MARGIN)
    .overlay_if(self.asking == Asking::NewNote, self.new_note_dialog())
    .overlay_if(self.asking == Asking::Delete, self.delete_dialog())
```

A dialog with a text field: what is typed reaches the app through `field`, Enter chooses the primary button, and Esc runs `on_close`:

```rust
fn new_note_dialog(&self) -> misty::view::Dialog<'_, Self> {
    dialog("New note", Vec::new())
        .field(&self.draft, |app: &mut Self, typed| app.draft = typed)
        .field_placeholder("What to remember")
        .button("Cancel", Self::cancel)
        .primary("Add", Self::add)
        .on_close(Self::cancel)
}
```

A question that loses something warns: `severity` marks the dialog with the warning sign, and `key` lets a letter choose the button just added, underlined in its label:

```rust
dialog("Delete this note?", body)
    .severity(Severity::Warning)
    .button("Cancel", Self::cancel)
    .key('c')
    .primary("Delete", Self::delete)
    .key('d')
    .on_close(Self::cancel)
```

A dialog is modal: while it is open no key or click reaches the window behind it, and a click outside it answers nothing, so a stray click never deletes a note.

## 7. Keys

Elements take the keys they know first (the list its motions, a focused button Enter). A key none of them took reaches the app's `key` method; return `true` for a key you used:

```rust
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
```

misty owns its key values (`Key`, `KeyCode`, `Mods`) and a key notation (`C-x`, `<esc>`, `g g`, read by `parse_key`), but resolves no key presses itself: prefixes, timeouts, and layers are the app's, with the keymap library it chooses.

## 8. Tests without a display

With the `testing` feature, `TreeWindow` runs the app in a headless window: press keys in misty's notation, type text, click elements by their role and name, and read the app's state back. No display, no GPU needed:

```rust
#[cfg(test)]
mod tests {
    use misty::accesskit::Role;
    use misty::testing::TreeWindow;

    use super::*;

    #[test]
    fn test_a_note_typed_into_the_dialog_is_added_under_the_cursor() {
        let mut window = TreeWindow::new(Notes::new(), "Notes", 800, 600);
        window.press("a");
        window.type_text("Buy bread");
        window.press("RET");
        assert_eq!(window.app().notes.last().map(String::as_str), Some("Buy bread"));
    }

    #[test]
    fn test_the_buttons_work_with_the_pointer_and_screen_readers() {
        let mut window = TreeWindow::new(Notes::new(), "Notes", 800, 600);
        window.click(Role::Button, "Delete");
        assert_eq!(window.app().asking, Asking::Delete);
    }
}
```

Clicking by role and name goes through the same path a screen reader uses, so a test that passes also says the app is usable without a mouse or without eyes.

## 9. Where to go next

The examples each show one part of misty, most of them as tree apps like this one:

| Example | Shows |
| --- | --- |
| `controls` | Form controls: checkbox, radio buttons, slider, stepper, select, text field. |
| `dialog` | A dialog with rich text and a link in its body. |
| `list` | A list of 100,000 rows: selection, dragging rows, which-key hints. |
| `table` | A table of 100,000 rows in columns. |
| `tree` | A folder tree over the file system. |
| `chrome` | A frameless window with misty's title bar, menus, tabs, and status bar. |
| `notify` | Tooltips and toasts. |
| `panes` | Split panes. |
| `workbench` | A larger app in one tree: what misty looks like at scale. |

Run any of them with `cargo run -p misty --example <name>`. The full API is documented on [docs.rs](https://docs.rs/misty); [element-tree.md](element-tree.md) explains why the element tree works as it does.
