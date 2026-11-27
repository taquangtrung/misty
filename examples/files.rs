//! A file list as a file manager shows it: each row with a folder or file
//! icon (folders tinted the accent color, files in their own colors), the
//! name with the characters a filter matched highlighted, and badges at
//! the end (a git status letter, a tag's colored dot).
//!
//! Keys in the list: the list keys (`j`, `k`, `g g`, `G`, `C-d`, ...),
//! Space toggles a row's selection, Shift with the arrows extends it, `v`
//! starts and ends a visual range that follows the cursor, `/` types a
//! filter (Enter keeps it, Esc clears it), `f` puts hint labels on the
//! rows in sight (type one to open its row, Esc to cancel), `r` or F2
//! renames the cursor's file in a field placed over its row (Enter keeps
//! the name, Esc drops it), and Esc quits. The pointer: Ctrl+click
//! toggles, Shift+click selects a range, and a drag from below the rows
//! (or with Alt anywhere) draws a rubber band, which scrolls the list
//! when held at its top or bottom edge.
//!
//! Ctrl+C and Ctrl+X put the selected entries (or the cursor's) on the
//! system clipboard as files, which a file manager pastes (as paths under
//! a made-up folder: the entries are not real files); Ctrl+V reads the
//! files on the clipboard, from this app or a file manager, into the
//! status line. With the `icon-theme` feature, the folder and file icons
//! come from the desktop's icon theme.
//!
//! ```sh
//! cargo run -p misty --example files --features icon-theme
//! ```

use std::borrow::Cow;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use misty::{AppContext, Application, Control, FileOperation, Frame, IconSet, Input, Key, KeyCode, Mods, Outcome, Point, Rect, Span, StyledText, Svg, TextColor, Theme, ThemeToken, Widget, WindowId, WindowOptions, fuzzy_highlight, fuzzy_score};
use misty::widgets::{Badge, Hints, HintsResponse, List, ListResponse, ListSource, RowContent, RowIcon, TextField, TextFieldResponse};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty files";

/// The made-up folder the entries are copied to the clipboard from.
const FOLDER: &str = "/tmp/misty-files";

/// The side of a theme icon asked for, in logical pixels.
#[cfg(feature = "icon-theme")]
const THEME_ICON_SIZE: u32 = 16;

/// A folder outline, drawn in one theme color.
const FOLDER_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
<path d="M1 3h5l2 2h7v8H1z" fill="none" stroke="black" stroke-width="1.5"/></svg>"#;

/// A document with a colored band, drawn in its own colors.
const FILE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
<path d="M3 1h7l3 3v11H3z" fill="#e8e8e8"/><rect x="3" y="10" width="10" height="3" fill="#f38ba8"/></svg>"##;

/// The names the generated entries are made of.
const STEMS: [&str; 12] = [
    "README", "main", "lib", "config", "notes", "build", "theme", "report", "draft", "index",
    "style", "layout",
];

/// The extensions of generated files.
const EXTENSIONS: [&str; 4] = ["rs", "md", "toml", "txt"];

/// How many entries the folder holds.
const ENTRY_COUNT: usize = 120;

/// Every how many entries one is a folder.
const FOLDER_EVERY: usize = 7;

/// Every how many entries one is modified in git.
const MODIFIED_EVERY: usize = 5;

/// Every how many entries one is tagged.
const TAGGED_EVERY: usize = 9;

/// The rows the status line takes at the bottom.
const STATUS_ROWS: f32 = 1.0;

/// The cells of padding before a list row's icon, and of gap after it, as
/// [`List`] lays a row out: the icon is a row's height wide.
const ICON_SIDE_CELLS: f32 = 1.5;

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a folder's entries in a list, with a filter, hints, and a
/// rename field.
struct FilesDemo {
    /// The entries and the filter over them.
    files: Files,
    /// The filter field, while `/` is typed.
    filter: Option<TextField>,
    /// The hint labels over the rows in sight, while `f` waits.
    hints: Hints,
    /// The list of the entries shown.
    list: List,
    /// The rename field, while a file is renamed, and the entry it renames.
    rename: Option<(TextField, usize)>,
    /// What the last action did.
    status: String,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

/// The entries of a folder and those a filter shows.
struct Files {
    /// Every entry.
    entries: Vec<Entry>,
    /// The icon of a file.
    file_icon: Svg,
    /// The icon of a folder.
    folder_icon: Svg,
    /// The entries shown, by index, with the characters the filter matched.
    shown: Vec<(usize, Vec<usize>)>,
}

/// One entry of the folder.
struct Entry {
    /// Whether it is a folder.
    is_folder: bool,
    /// Whether git sees it modified.
    is_modified: bool,
    /// Its name.
    name: String,
    /// The color of its tag, if it has one.
    tag: Option<ThemeToken>,
}

// ========================================================================
// Files Implementations
// ========================================================================

impl Files {
    /// Shows the entries matching `query`, best first; every entry in
    /// folder order for an empty query.
    fn filter(&mut self, query: &str) {
        let mut shown: Vec<(usize, i32, Vec<usize>)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let found = fuzzy_score(query, &entry.name)?;
                Some((index, found.score, found.positions))
            })
            .collect();
        if !query.trim().is_empty() {
            shown.sort_by_key(|(_, score, _)| -score);
        }
        self.shown = shown
            .into_iter()
            .map(|(index, _, positions)| (index, positions))
            .collect();
    }
}

impl ListSource for Files {
    fn row_count(&self) -> usize {
        self.shown.len()
    }

    fn row_text(&self, row: usize) -> Cow<'_, str> {
        Cow::Borrowed(&self.entries[self.shown[row].0].name)
    }

    fn row_content(&self, row: usize) -> RowContent {
        let (index, positions) = &self.shown[row];
        let entry = &self.entries[*index];
        let name = fuzzy_highlight(
            &entry.name,
            positions,
            ThemeToken::Foreground.into(),
            ThemeToken::Accent.into(),
        );
        let icon = match entry.is_folder {
            true => RowIcon::tinted(self.folder_icon.clone(), ThemeToken::Accent),
            false => RowIcon::colored(self.file_icon.clone()),
        };
        let mut content = RowContent::styled(name).with_icon(icon);
        if entry.is_modified {
            let letter = Span::new("M", ThemeToken::Warning.into());
            content = content.with_badge(Badge::Text(letter));
        }
        if let Some(tag) = entry.tag {
            content = content.with_badge(Badge::Dot(tag));
        }
        content
    }
}

// ========================================================================
// FilesDemo Implementations
// ========================================================================

impl FilesDemo {
    /// Reacts to a key while the list has the keyboard; whether the app
    /// should quit.
    fn list_key(&mut self, key: Key, cx: &mut AppContext<'_, ()>, window: WindowId) -> bool {
        let response = self
            .list
            .lend(&self.files)
            .handle(&Input::Key(key), &mut cx.event_cx(window));
        match response {
            ListResponse::Activated(row) => self.open(row),
            ListResponse::CursorMoved(_) | ListResponse::Dropped(_) | ListResponse::Handled => {}
            ListResponse::Ignored if self.clipboard_key(key, cx) => {}
            ListResponse::Ignored => return self.command(key),
            _ => {}
        }
        false
    }

    /// Copies, cuts, or pastes files for Ctrl+C, Ctrl+X, and Ctrl+V;
    /// whether `key` was one of them.
    fn clipboard_key(&mut self, key: Key, cx: &mut AppContext<'_, ()>) -> bool {
        if key.mods != Mods::CTRL {
            return false;
        }
        let operation = match key.code {
            KeyCode::Char('c') => FileOperation::Copy,
            KeyCode::Char('x') => FileOperation::Cut,
            KeyCode::Char('v') => {
                self.status = match cx.clipboard().files() {
                    Some((files, operation)) => format!(
                        "paste {} file(s) ({operation:?}), first {}",
                        files.len(),
                        files[0].display()
                    ),
                    None => "no files on the clipboard".into(),
                };
                return true;
            }
            _ => return false,
        };
        let state = self.list.state();
        let mut rows: Vec<usize> = state.selected().collect();
        if rows.is_empty() {
            rows.push(state.cursor());
        }
        let paths: Vec<PathBuf> = rows
            .into_iter()
            .filter_map(|row| self.files.shown.get(row))
            .map(|(index, _)| Path::new(FOLDER).join(&self.files.entries[*index].name))
            .collect();
        self.status = match cx.clipboard().set_files(&paths, operation) {
            Ok(()) => format!("{operation:?}: {} file(s) on the clipboard", paths.len()),
            Err(error) => format!("{operation:?} kept in the app: {error}"),
        };
        true
    }

    /// Runs the app command `key` names, if any; whether to quit.
    fn command(&mut self, key: Key) -> bool {
        let plain = key.mods == Mods::NONE;
        match key.code {
            KeyCode::Esc if plain => return true,
            KeyCode::Char('/') if plain => {
                let mut field = TextField::new().with_placeholder("filter");
                field.set_focused(true);
                self.filter = Some(field);
            }
            KeyCode::Char('f') if plain => {
                let rects = self
                    .list
                    .visible_range()
                    .filter_map(|row| self.list.row_rect(row))
                    .collect();
                self.hints.show(rects);
            }
            KeyCode::Char('r') | KeyCode::F(2) if plain => self.start_rename(),
            KeyCode::Char('v') if plain => {
                let state = self.list.state_mut();
                match state.is_ranging() {
                    true => state.end_range(),
                    false => state.start_range(),
                }
            }
            _ => {}
        }
        false
    }

    /// Opens the rename field over the cursor's row, the name selected up
    /// to its extension.
    fn start_rename(&mut self) {
        let Some(&(index, _)) = self.files.shown.get(self.list.state().cursor()) else {
            return;
        };
        let name = &self.files.entries[index].name;
        let mut field = TextField::with_text(name).with_frame();
        field.select(0..name.rfind('.').unwrap_or(name.len()));
        field.set_focused(true);
        self.rename = Some((field, index));
    }

    /// Says row `row` was opened.
    fn open(&mut self, row: usize) {
        if let Some(&(index, _)) = self.files.shown.get(row) {
            self.status = format!("opened {}", self.files.entries[index].name);
        }
    }

    /// Hands `input` to the filter field; whether it took it.
    fn filter_input(
        &mut self,
        input: &Input,
        cx: &mut AppContext<'_, ()>,
        window: WindowId,
    ) -> bool {
        let Some(field) = &mut self.filter else {
            return false;
        };
        match field.handle(input, &mut cx.event_cx(window)) {
            TextFieldResponse::Changed => {
                self.files.filter(field.text());
                self.list.state_mut().clear_selection();
                self.list.state_mut().set_cursor(0);
            }
            TextFieldResponse::Handled => {}
            TextFieldResponse::Ignored => match input.key().map(|key| key.code) {
                Some(KeyCode::Enter) => self.filter = None,
                Some(KeyCode::Esc) => {
                    self.filter = None;
                    self.files.filter("");
                }
                _ => return input.key().is_some(),
            },
            _ => {}
        }
        true
    }

    /// Hands `input` to the rename field; whether it took it.
    fn rename_input(
        &mut self,
        input: &Input,
        cx: &mut AppContext<'_, ()>,
        window: WindowId,
    ) -> bool {
        let Some((field, index)) = &mut self.rename else {
            return false;
        };
        let response = field.handle(input, &mut cx.event_cx(window));
        if !response.is_ignored() {
            return true;
        }
        match input.key().map(|key| key.code) {
            Some(KeyCode::Enter) => {
                let name = field.text().to_owned();
                self.status = format!("renamed to {name}");
                self.files.entries[*index].name = name;
                self.rename = None;
            }
            Some(KeyCode::Esc) => self.rename = None,
            _ => return input.key().is_some(),
        }
        true
    }

    /// Draws the status line at `y`, `width` wide.
    fn draw_status(&self, frame: &mut Frame, y: f32, width: f32) {
        let state = self.list.state();
        let mode = match (self.hints.is_active(), state.is_ranging()) {
            (true, _) => format!("HINT {}", self.hints.typed()),
            (false, true) => "VISUAL".to_owned(),
            (false, false) => String::new(),
        };
        let muted: TextColor = ThemeToken::Muted.into();
        let line = StyledText::new()
            .with(Span::new(format!(" {mode} "), ThemeToken::Accent.into()).bold())
            .with(Span::new(
                format!(
                    "{} of {} selected   {}",
                    state.selected().count(),
                    state.count(),
                    self.status
                ),
                muted,
            ))
            .with_ellipsis();
        let height = frame.cell_metrics().height;
        let clip = Rect::new(0.0, y, width, height);
        frame.draw_text(&line, Point::new(0.0, y), clip, &self.theme);
    }
}

impl Application for FilesDemo {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        self.list
            .lend(&self.files)
            .about_to_wait(&mut cx.event_cx(window))
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        cx.event_cx(window).request_redraw();
        if *input == Input::CloseRequested {
            cx.exit();
            return;
        }
        match self.hints.handle(input, &mut cx.event_cx(window)) {
            HintsResponse::Activated(index) => {
                let row = self.list.visible_range().start + index;
                self.list.state_mut().set_cursor(row);
                self.open(row);
                return;
            }
            HintsResponse::Cancelled | HintsResponse::Handled => return,
            HintsResponse::Ignored => {}
            _ => {}
        }
        if self.filter_input(input, cx, window) || self.rename_input(input, cx, window) {
            return;
        }
        let quits = match input.key() {
            Some(key) => self.list_key(key, cx, window),
            None => {
                let response = self
                    .list
                    .lend(&self.files)
                    .handle(input, &mut cx.event_cx(window));
                if let ListResponse::Activated(row) = response {
                    self.open(row);
                }
                false
            }
        };
        if quits {
            cx.exit();
        }
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.background.into());
        let bounds = frame.bounds();
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let status_height = STATUS_ROWS * metrics.height;
        let mut list_area = Rect::new(
            bounds.x,
            bounds.y,
            bounds.width,
            (bounds.height - status_height).max(0.0),
        );
        if let Some(field) = &mut self.filter {
            let height = field.measure(bounds.width, metrics, scale_factor);
            list_area.height = (list_area.height - height).max(0.0);
            let rect = Rect::new(
                bounds.x,
                list_area.y + list_area.height,
                bounds.width,
                height,
            );
            field.draw(frame, rect, &self.theme);
        }
        self.list
            .lend(&self.files)
            .draw(frame, list_area, &self.theme);
        if let Some((field, _)) = &mut self.rename
            && let Some(row) = self.list.row_rect(self.list.state().cursor())
        {
            let x = row.x + (ICON_SIDE_CELLS * metrics.width).round() + metrics.height;
            let height = field.measure(row.x + row.width - x, metrics, scale_factor);
            let y = (row.y + (row.height - height) / 2.0).round();
            let rect = Rect::new(x, y, (row.x + row.width - x).max(0.0), height);
            field.draw(frame, rect, &self.theme);
        }
        self.hints.draw(frame, list_area, &self.theme);
        self.draw_status(
            frame,
            bounds.y + bounds.height - status_height,
            bounds.width,
        );
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The generated folder: every few entries a folder, some modified, some
/// tagged.
fn entries() -> Vec<Entry> {
    let tags = [ThemeToken::Accent, ThemeToken::Danger, ThemeToken::Warning];
    (0..ENTRY_COUNT)
        .map(|index| {
            let stem = STEMS[index % STEMS.len()];
            let is_folder = index % FOLDER_EVERY == 0;
            let name = match is_folder {
                true => format!("{stem}_{index}"),
                false => format!("{stem}_{index}.{}", EXTENSIONS[index % EXTENSIONS.len()]),
            };
            Entry {
                is_folder,
                is_modified: index % MODIFIED_EVERY == 1,
                name,
                tag: (index % TAGGED_EVERY == 2).then(|| tags[index % tags.len()]),
            }
        })
        .collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let icons = IconSet::new()
        .with("folder", FOLDER_SVG.as_bytes())?
        .with("file", FILE_SVG.as_bytes())?;
    let icon = |name: &str| icons.get(name).cloned().ok_or("an icon went missing");
    let mut files = Files {
        entries: entries(),
        file_icon: icon("file")?,
        folder_icon: icon("folder")?,
        shown: Vec::new(),
    };
    #[cfg(feature = "icon-theme")]
    {
        let mut theme = misty::IconTheme::system();
        let svg = |icon| match icon {
            Some(misty::ThemeIcon::Svg(svg)) => Some(svg),
            Some(misty::ThemeIcon::Image(_)) | None => None,
        };
        if let Some(folder) = svg(theme.icon("folder", THEME_ICON_SIZE, 1)) {
            files.folder_icon = folder;
        }
        if let Some(file) = svg(theme.icon("text-x-generic", THEME_ICON_SIZE, 1)) {
            files.file_icon = file;
        }
    }
    files.filter("");
    let app = FilesDemo {
        files,
        filter: None,
        hints: Hints::new(),
        list: List::new(),
        rename: None,
        status: String::from(
            "f: hints   r: rename   /: filter   v: visual   C-c C-x C-v: files   Esc: quit",
        ),
        theme: Theme::default(),
        window: None,
    };
    misty::run(app, WindowOptions::new(TITLE))?;
    Ok(())
}
