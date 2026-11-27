//! A frameless window described by one tree: a title bar whose menu button
//! opens the app's menu, a strip of tabs over the open documents, a split
//! of a file list beside the active document's text, and a status bar.
//!
//! - **Tabs:** click one to show it, drag one to move it, click its close
//!   button (or middle-click it) to close it, and `+` for a new one; a
//!   right click on a tab opens its menu.
//! - **Files:** Up and Down move along the list, Enter opens the file in a
//!   tab; a right click, or Shift+F10, opens the list's menu.
//! - **Panes:** drag the divider between the list and the text.
//! - **Settings:** the menu's Settings opens a layer of form controls over
//!   the window (the theme and the hints' delay); Done closes it.
//! - **Keys:** Ctrl+X is a prefix: wait a moment and the keys that may
//!   follow show along the bottom (`n` new tab, `k` close tab, `s` flip the
//!   split, `q` quit). Tab moves between the list, the text, and the menu
//!   button; Ctrl+Q quits.
//!
//! ```sh
//! cargo run -p misty --example bars
//! ```

use std::borrow::Cow;
use std::time::Duration;

use misty::view::{
    MenuRequest, button, column, context_menu, field, form, list, overlay, radio, split,
    status_bar, stepper, tab, tab_strip, text, text_area, title_bar, which_key,
};
use misty::{App, AppCx, Axis, Decorations, Element, Key, KeyCode, Mods, Point, Size, StyledText, Theme, ThemeToken, WindowId, WindowOptions};
use misty::widgets::{ListSource, MenuEntry, MenuItem, NumberRange, StatusLine, StatusSegment, WhichKeyEntry};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty bars";

/// The size the window opens at, in logical pixels.
const SIZE: Size = Size::new(900, 560);

/// The files the list shows, each opened with this text.
const FILES: [(&str, &str); 5] = [
    ("README.md", "# Notes\n\nA frameless window, one view."),
    ("Cargo.toml", "[package]\nname = \"notes\""),
    ("main.rs", "fn main() {\n    println!(\"hello\");\n}"),
    ("todo.txt", "- tabs\n- panes\n- menus"),
    ("LICENSE", "MIT"),
];

/// The share of the window the file list starts with.
const START_RATIO: f32 = 0.3;

/// How long the Ctrl+X prefix waits before its keys show.
const HINT_DELAY: Duration = Duration::from_millis(500);

/// The id of the status segment that flips the split.
const SPLIT_SEGMENT: &str = "split";

/// The themes the settings offer: whether dark, and its name.
const THEMES: [(bool, &str); 2] = [(true, "Dark"), (false, "Light")];

/// The delays the settings offer for the prefix's hints, in milliseconds.
const HINT_DELAYS: NumberRange = NumberRange::new(0.0, 2000.0, 100.0);

/// The blank cell columns and rows around the settings layer's controls.
const SETTINGS_PADDING: (f32, f32) = (4.0, 2.0);

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the open documents, the panes, and the menu the user asked
/// for.
struct Notes {
    /// The document shown, by id.
    active: Option<u32>,
    /// How the panes are placed.
    axis: Axis,
    /// The file list's cursor.
    cursor: usize,
    /// The open documents, in tab order.
    docs: Vec<Doc>,
    /// How long the prefix waits before its keys show.
    hint_delay: Duration,
    /// The menu open, and where.
    menu: Option<(MenuKind, MenuRequest)>,
    /// The id the next document gets.
    next_id: u32,
    /// The keys typed toward a command, empty when none is pending.
    prefix: Vec<Key>,
    /// Whether the user asked to quit.
    quit: bool,
    /// The share of the window the file list takes.
    ratio: f32,
    /// Whether the settings layer shows.
    settings: bool,
    /// The colors the window is drawn in.
    theme: Theme,
}

/// One open document.
struct Doc {
    /// What the app knows it by.
    id: u32,
    /// Whether it holds edits.
    modified: bool,
    /// Its text.
    text: String,
    /// Its name, shown on its tab.
    title: String,
    /// The number that changes with its text.
    version: u64,
}

/// The file list's rows.
struct Files;

/// Which menu is open.
#[derive(Clone, Copy, Debug, PartialEq)]
enum MenuKind {
    /// The file list's.
    Files,
    /// The title bar's menu button's.
    Main,
    /// A tab's, by its document's id.
    Tab(u32),
}

/// What a menu item or a key after the prefix does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Command {
    /// Closes every tab.
    CloseAll,
    /// Closes every tab but this document's.
    CloseOthers(u32),
    /// Closes this document's tab.
    CloseTab(u32),
    /// Flips the split between side by side and one above the other.
    FlipSplit,
    /// Opens a new, empty document.
    NewTab,
    /// Opens the file under the list's cursor.
    OpenFile,
    /// Quits.
    Quit,
    /// Shows the settings layer.
    Settings,
}

// ========================================================================
// Notes Implementations
// ========================================================================

impl Notes {
    /// The demo with the first file open, the file list on the left.
    fn new() -> Self {
        let mut notes = Self {
            active: None,
            axis: Axis::Horizontal,
            cursor: 0,
            docs: Vec::new(),
            hint_delay: HINT_DELAY,
            menu: None,
            next_id: 1,
            prefix: Vec::new(),
            quit: false,
            ratio: START_RATIO,
            settings: false,
            theme: Theme::dark(),
        };
        notes.open(0);
        notes
    }

    /// The document shown, if any.
    fn active_doc(&self) -> Option<&Doc> {
        self.docs.iter().find(|doc| Some(doc.id) == self.active)
    }

    /// Opens a document titled `title` holding `text`, and shows it.
    fn add(&mut self, title: &str, text: &str) {
        let id = self.next_id;
        self.next_id += 1;
        self.docs.push(Doc {
            id,
            modified: false,
            text: text.to_owned(),
            title: title.to_owned(),
            version: 0,
        });
        self.active = Some(id);
    }

    /// Shows the file at `row` of the list, opening it when no tab has it.
    fn open(&mut self, row: usize) {
        let Some((name, body)) = FILES.get(row) else {
            return;
        };
        match self.docs.iter().find(|doc| doc.title == *name) {
            Some(doc) => self.active = Some(doc.id),
            None => self.add(name, body),
        }
    }

    /// Closes the documents `close` picks, showing the one in the closed
    /// tab's place when the active one goes.
    fn close_where(&mut self, close: impl Fn(&Doc) -> bool) {
        let place = self.docs.iter().position(|doc| Some(doc.id) == self.active);
        self.docs.retain(|doc| !close(doc));
        if self.active_doc().is_none() {
            let place = place.unwrap_or(0).min(self.docs.len().saturating_sub(1));
            self.active = self.docs.get(place).map(|doc| doc.id);
        }
    }

    /// Moves document `id` to `index` among the tabs.
    fn move_tab(&mut self, id: u32, index: usize) {
        if let Some(from) = self.docs.iter().position(|doc| doc.id == id) {
            let doc = self.docs.remove(from);
            self.docs.insert(index.min(self.docs.len()), doc);
        }
    }

    /// Stores `text` as document `id`'s, marking it modified.
    fn edit(&mut self, id: u32, text: String) {
        if let Some(doc) = self.docs.iter_mut().find(|doc| doc.id == id) {
            doc.text = text;
            doc.version += 1;
            doc.modified = true;
        }
    }

    /// Carries out `command`, closing any menu.
    fn run(&mut self, command: Command) {
        self.menu = None;
        match command {
            Command::CloseAll => self.close_where(|_| true),
            Command::CloseOthers(id) => self.close_where(|doc| doc.id != id),
            Command::CloseTab(id) => self.close_where(|doc| doc.id == id),
            Command::FlipSplit => {
                self.axis = match self.axis {
                    Axis::Horizontal => Axis::Vertical,
                    Axis::Vertical => Axis::Horizontal,
                }
            }
            Command::NewTab => self.add("untitled", ""),
            Command::OpenFile => self.open(self.cursor),
            Command::Quit => self.quit = true,
            Command::Settings => self.settings = true,
        }
    }

    /// Whether the window is drawn dark.
    fn is_dark(&self) -> bool {
        self.theme.background == Theme::dark().background
    }

    /// The settings layer: form controls over the window, each changing
    /// the app's state as the user moves it.
    fn settings_layer(&self) -> Element<'_, Self> {
        let delay = self.hint_delay.as_millis() as f64;
        let (columns, rows) = SETTINGS_PADDING;
        column([
            text("Settings").into(),
            form([
                field(
                    "Theme",
                    radio(&THEMES, &self.is_dark()).on_change(|app: &mut Self, dark| {
                        app.theme = if dark { Theme::dark() } else { Theme::light() };
                    }),
                ),
                field(
                    "Hint delay (ms)",
                    stepper(HINT_DELAYS, delay).on_change(|app: &mut Self, millis| {
                        app.hint_delay = Duration::from_millis(millis as u64);
                    }),
                ),
            ]),
            button("Done")
                .on_click(|app: &mut Self| app.settings = false)
                .into(),
        ])
        .padding(columns, rows)
    }

    /// The items of the menu `kind`.
    fn menu_entries(&self, kind: MenuKind) -> Vec<MenuEntry<Command>> {
        let item = |command: Command, label: &str| MenuEntry::from(MenuItem::new(command, label));
        match kind {
            MenuKind::Files => vec![
                item(Command::OpenFile, "Open"),
                item(Command::CloseAll, "Close All Tabs"),
            ],
            MenuKind::Main => vec![
                item(Command::NewTab, "New Tab"),
                item(Command::FlipSplit, "Flip Split"),
                item(Command::Settings, "Settings"),
                MenuEntry::Separator,
                item(Command::Quit, "Quit"),
            ],
            MenuKind::Tab(id) => vec![
                item(Command::CloseTab(id), "Close"),
                item(Command::CloseOthers(id), "Close Others"),
            ],
        }
    }

    /// The commands the keys after the prefix run, and the hints naming
    /// them.
    fn prefix_commands(&self) -> Vec<(char, Command, &'static str)> {
        let active = self.active.unwrap_or_default();
        vec![
            ('n', Command::NewTab, "New tab"),
            ('k', Command::CloseTab(active), "Close tab"),
            ('s', Command::FlipSplit, "Flip split"),
            ('q', Command::Quit, "Quit"),
        ]
    }

    /// What the status bar shows.
    fn status_line(&self) -> StatusLine {
        let plain = |text: &str| StyledText::plain(text, ThemeToken::Foreground.into());
        let primary = match self.active_doc() {
            Some(doc) if doc.modified => format!("{} (modified)", doc.title),
            Some(doc) => doc.title.clone(),
            None => String::from("no document"),
        };
        let lines = self.active_doc().map_or(0, |doc| doc.text.lines().count());
        let split = match self.axis {
            Axis::Horizontal => "side by side",
            Axis::Vertical => "stacked",
        };
        let line = StatusLine::new(plain(&primary))
            .with_segment(StatusSegment::new(plain(&format!("{lines} lines"))))
            .with_segment(StatusSegment::new(plain(split)).with_id(SPLIT_SEGMENT));
        match self.prefix.is_empty() {
            true => line,
            false => line.with_pending("C-x"),
        }
    }

    /// The active document's text area, or a note when none is open.
    fn editor(&self) -> Element<'_, Self> {
        let Some(doc) = self.active_doc() else {
            return text("No document open.").muted().into();
        };
        let id = doc.id;
        Element::from(
            text_area(&doc.text, doc.version)
                .on_change(move |app: &mut Self, text| app.edit(id, text)),
        )
        .id(format!("doc-{id}"))
    }
}

impl App for Notes {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        let tabs = self
            .docs
            .iter()
            .map(|doc| tab(doc.id, &doc.title).modified(doc.modified));
        let files = Element::from(
            list(&Files, self.cursor)
                .on_cursor_move(|app: &mut Self, cursor| app.cursor = cursor)
                .on_activate(|app: &mut Self, row| app.open(row)),
        )
        .on_context_menu(|app: &mut Self, at| app.menu = Some((MenuKind::Files, at)));
        let (kind, at) = self
            .menu
            .unwrap_or((MenuKind::Main, MenuRequest::at(Point::default())));
        let menu = context_menu(self.menu_entries(kind), at)
            .on_choose(|app: &mut Self, command| app.run(command))
            .on_close(|app: &mut Self| app.menu = None);
        let commands = self.prefix_commands();
        let hints = commands
            .iter()
            .map(|(key, _, title)| {
                WhichKeyEntry::new(Key::new(KeyCode::Char(*key), Mods::NONE), *title, false)
            })
            .collect();
        let hints = which_key(&self.prefix, hints)
            .delay(self.hint_delay)
            .on_activate(move |app: &mut Self, index| {
                app.prefix.clear();
                if let Some((_, command, _)) = commands.get(index) {
                    app.run(*command);
                }
            })
            .on_close(|app: &mut Self| app.prefix.clear());
        column([
            title_bar(TITLE)
                .on_menu(|app: &mut Self, at| app.menu = Some((MenuKind::Main, at)))
                .into(),
            tab_strip(tabs, self.active.as_ref())
                .on_activate(|app: &mut Self, id| app.active = Some(id))
                .on_close(|app: &mut Self, id| app.run(Command::CloseTab(id)))
                .on_move(|app: &mut Self, (id, index)| app.move_tab(id, index))
                .on_new(|app: &mut Self| app.run(Command::NewTab))
                .on_context_menu(|app: &mut Self, (id, at)| {
                    app.menu = Some((MenuKind::Tab(id), at));
                })
                .into(),
            split(self.axis, self.ratio, files, self.editor())
                .on_resize(|app: &mut Self, ratio| app.ratio = ratio)
                .into(),
            status_bar(self.status_line())
                .on_click(|app: &mut Self, id| {
                    if id == SPLIT_SEGMENT {
                        app.run(Command::FlipSplit);
                    }
                })
                .into(),
        ])
        .overlay_if(self.menu.is_some(), menu)
        .overlay(hints)
        .overlay_if(self.settings, overlay(self.settings_layer()).modal())
    }

    /// Ctrl+X starts a prefix whose next key runs a command; Ctrl+Q quits.
    fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
        if !self.prefix.is_empty() {
            self.prefix.clear();
            let command = self
                .prefix_commands()
                .into_iter()
                .find(|(c, _, _)| key == Key::new(KeyCode::Char(*c), Mods::NONE));
            if let Some((_, command, _)) = command {
                self.run(command);
            }
            return true;
        }
        match (key.code, key.mods) {
            (KeyCode::Char('x'), Mods::CTRL) => self.prefix = vec![key],
            (KeyCode::Char('q'), Mods::CTRL) => cx.exit(),
            _ => return false,
        }
        true
    }

    fn should_exit(&self) -> bool {
        self.quit
    }

    fn theme(&self) -> &Theme {
        &self.theme
    }
}

// ========================================================================
// Files Implementations
// ========================================================================

impl ListSource for Files {
    fn row_count(&self) -> usize {
        FILES.len()
    }

    fn row_text(&self, row: usize) -> Cow<'_, str> {
        Cow::Borrowed(FILES[row].0)
    }
}

// ========================================================================
// Functions
// ========================================================================

fn main() -> Result<(), misty::Error> {
    let options = WindowOptions::new(TITLE)
        .with_size(SIZE)
        .with_decorations(Decorations::Frameless);
    misty::run_app(Notes::new(), options)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, feature = "testing"))]
mod tests {
    use misty::AccessAction;
    use misty::accesskit::Role;
    use misty::testing::TreeWindow;

    use super::*;

    /// The window's width in the tests, in physical pixels.
    const WIDTH: u32 = 900;

    /// The window's height in the tests.
    const HEIGHT: u32 = 560;

    /// The window showing the demo, its hints showing as soon as asked.
    fn window() -> TreeWindow<Notes> {
        let notes = Notes {
            hint_delay: Duration::ZERO,
            ..Notes::new()
        };
        TreeWindow::new(notes, TITLE, WIDTH, HEIGHT)
    }

    /// The titles of the tabs `window` describes, left to right.
    fn tab_titles(window: &TreeWindow<Notes>) -> Vec<String> {
        window
            .ui_text()
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("Tab \""))
            .map(|rest| rest.split('"').next().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn test_the_window_describes_its_bars_and_panes_at_start() {
        let window = window();
        let text = window.ui_text();
        for part in [
            "TitleBar \"misty bars\"",
            "Button \"Menu\"",
            "Button \"Close\"",
            "TabList",
            "Tab \"README.md\" selected",
            "Button \"New Tab\"",
            "ListItem \"README.md\"",
            "TextInput",
        ] {
            assert!(text.contains(part), "{part} in {text}");
        }
        assert!(
            text.find("TitleBar") < text.find("TabList")
                && text.find("TabList") < text.find("ListItem"),
            "the bar, the tabs, then the panes: {text}"
        );
    }

    #[test]
    fn test_enter_on_a_file_opens_it_in_a_tab_and_shows_it() {
        let mut window = window();
        window.press("<down> <down> RET");
        assert_eq!(tab_titles(&window), ["README.md", "main.rs"]);
        assert!(window.ui_text().contains("Tab \"main.rs\" selected"));
        window.press("<up> <up> RET");
        assert_eq!(
            tab_titles(&window),
            ["README.md", "main.rs"],
            "no second tab"
        );
        assert!(window.ui_text().contains("Tab \"README.md\" selected"));
    }

    #[test]
    fn test_a_click_on_a_tab_shows_its_document() {
        let mut window = window();
        window.press("<down> RET");
        window.click(Role::Tab, "README.md");
        assert_eq!(
            window.app().active_doc().map(|doc| doc.title.as_str()),
            Some("README.md")
        );
        let text = window.ui_text();
        assert!(text.contains("value=\"# Notes"), "{text}");
    }

    #[test]
    fn test_the_new_tab_button_opens_a_tab_and_its_menu_closes_it() {
        let mut window = window();
        window.click(Role::Button, "New Tab");
        assert_eq!(tab_titles(&window), ["README.md", "untitled"]);
        window.right_click(Role::Tab, "untitled");
        assert!(window.ui_text().contains("MenuItem \"Close Others\""));
        window.click(Role::MenuItem, "Close");
        assert_eq!(tab_titles(&window), ["README.md"]);
        assert!(!window.ui_text().contains("Close Others"), "the menu went");
    }

    #[test]
    fn test_a_tab_dragged_past_another_moves_there() {
        let mut window = window();
        window.press("<down> RET <down> RET");
        assert_eq!(tab_titles(&window), ["README.md", "Cargo.toml", "main.rs"]);
        let from = window.center(Role::Tab, "README.md");
        let to = window.center(Role::Tab, "main.rs");
        window.drag(from, Point::new(to.x + 20.0, to.y));
        assert_eq!(tab_titles(&window), ["Cargo.toml", "main.rs", "README.md"]);
    }

    #[test]
    fn test_typing_edits_the_active_document_and_marks_it_modified() {
        let mut window = window();
        window.press("<tab>");
        window.type_text("x");
        let doc = window.app().active_doc().expect("a document is open");
        assert!(doc.modified);
        assert!(
            doc.text.starts_with('x') || doc.text.ends_with('x'),
            "{:?}",
            doc.text
        );
        assert!(
            window
                .drawn_text()
                .iter()
                .any(|line| line.contains("README.md (modified)")),
            "{:?}",
            window.drawn_text()
        );
    }

    #[test]
    fn test_dragging_the_divider_widens_the_file_list() {
        let mut window = window();
        let list = window.center(Role::ListItem, "README.md");
        let divider = (WIDTH as f32 * START_RATIO).round();
        let y = list.y + 100.0;
        window.drag(Point::new(divider, y), Point::new(divider + 180.0, y));
        assert!(
            window.app().ratio > START_RATIO + 0.15,
            "{}",
            window.app().ratio
        );
        let after = window.center(Role::ListItem, "README.md");
        assert!(after.x > list.x, "the row is wider: {after:?} {list:?}");
    }

    #[test]
    fn test_the_menu_button_opens_the_main_menu_from_the_pointer_and_the_keyboard() {
        let mut window = window();
        window.click(Role::Button, "Menu");
        assert!(window.ui_text().contains("MenuItem \"Flip Split\""));
        window.click(Role::MenuItem, "Flip Split");
        assert_eq!(window.app().axis, Axis::Vertical);
        window.press("RET");
        let text = window.ui_text();
        assert!(text.contains("MenuItem \"New Tab\" focused"), "{text}");
        window.press("<esc>");
        assert!(window.app().menu.is_none());
    }

    #[test]
    fn test_the_file_list_menu_opens_on_a_right_click_and_from_the_keyboard() {
        let mut window = window();
        window.press("<down>");
        // The right click moves the cursor to its row: the menu is for it.
        window.right_click(Role::ListItem, "LICENSE");
        window.click(Role::MenuItem, "Open");
        assert_eq!(tab_titles(&window), ["README.md", "LICENSE"]);
        window.press("S-<f10>");
        assert!(window.ui_text().contains("MenuItem \"Open\" focused"));
        window.press("<down> RET");
        assert!(tab_titles(&window).is_empty(), "Close All Tabs ran");
        assert!(window.ui_text().contains("ListItem \"Cargo.toml\""));
    }

    #[test]
    fn test_the_prefix_shows_which_key_hints_and_the_next_key_runs_a_command() {
        let mut window = window();
        window.press("C-x");
        window.about_to_wait();
        let hints = window.drawn_text();
        assert!(
            hints.iter().any(|line| line.contains("New tab")),
            "{hints:?}"
        );
        assert!(
            hints.iter().any(|line| line.contains("C-x")),
            "the status bar names the prefix"
        );
        window.press("n");
        assert_eq!(tab_titles(&window), ["README.md", "untitled"]);
        assert!(
            !window
                .drawn_text()
                .iter()
                .any(|line| line.contains("Flip split"))
        );
        window.press("C-x s");
        assert_eq!(window.app().axis, Axis::Vertical);
    }

    #[test]
    fn test_the_close_button_closes_the_window_and_quit_quits() {
        let mut window = window();
        window.click(Role::Button, "Close");
        assert!(window.is_closed());
        let mut quitting = self::window();
        quitting.press("C-x q");
        assert!(quitting.is_closed());
    }

    #[test]
    fn test_the_settings_layer_changes_the_apps_state_and_done_closes_it() {
        let mut window = window();
        window.click(Role::Button, "Menu");
        window.click(Role::MenuItem, "Settings");
        let text = window.ui_text();
        assert!(text.contains("RadioButton \"Dark\""), "{text}");
        assert!(text.contains("SpinButton \"Hint delay (ms)\""), "{text}");
        window.click(Role::RadioButton, "Light");
        assert!(!window.app().is_dark());
        window.click(Role::Button, "New Tab");
        assert_eq!(
            tab_titles(&window),
            ["README.md"],
            "the layer keeps the pointer"
        );
        window.click(Role::Button, "Done");
        assert!(!window.app().settings);
        window.click(Role::Button, "New Tab");
        assert_eq!(tab_titles(&window), ["README.md", "untitled"]);
    }

    #[test]
    fn test_a_screen_reader_opens_the_menu_and_chooses_new_tab() {
        let mut window = window();
        let menu = window.find(Role::Button, "Menu");
        window.act(AccessAction::Click(menu));
        let new_tab = window.find(Role::MenuItem, "New Tab");
        window.act(AccessAction::Click(new_tab));
        assert_eq!(tab_titles(&window), ["README.md", "untitled"]);
    }
}
