//! A table of 100,000 generated files in four columns: name, size,
//! modified, and kind. Only the rows on screen are asked for and drawn.
//!
//! The table's own keys move it as the list example's do, as Vim moves
//! over lines: Up and Down (or `j` and `k`, `C-p` and `C-n`) move the
//! cursor, `C-f` and `C-b` page, `C-d` and `C-u` move half a page, `C-e`
//! and `C-y` scroll, `H`, `M`, `L` go to the screen's rows, `g g` and `G`
//! jump to the ends, `z z` and its kin scroll the cursor's row, and a
//! count repeats a motion or names a row (the status line shows a pending
//! count or prefix); Space toggles the cursor row's selection, Enter opens
//! it.
//! Click a row to move the cursor, Ctrl+click to toggle its selection,
//! double-click to open it, and use the wheel to scroll. Drag a row to
//! move it. Click a column title to sort by it, again to reverse; the
//! table only reports the click, and the demo sorts its rows and keeps the
//! cursor on the same file. Drag a border between titles to resize a
//! column: the widths are printed when the drag ends, where a real app
//! would save them. The name column stretches to fill the window; when
//! the columns are wider than the window, Shift with the wheel, a sideways
//! swipe, or Left and Right (`h`, `l`) scroll them sideways, `0` and `$`
//! to either edge. Right-click a column title to hide that column, and
//! past the last title to show every column again. Esc quits.
//!
//! ```sh
//! cargo run -p misty --example table
//! ```

use std::borrow::Cow;
use std::cmp::Ordering;
use std::time::Instant;

use misty::{
    AppContext, Application, Error, FocusId, Frame, Input, Key, KeyCode, Mods, Point, Rect,
    SortDirection, SortKey, Span, StyledText, Table, TableColumn, TableResponse, TableSource,
    TextAlign, Theme, ThemeToken, Widget, WindowId, WindowOptions, key_label,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty table";

/// How many files there are.
const FILE_COUNT: usize = 100_000;

/// The column of names.
const NAME: usize = 0;

/// The column of sizes.
const SIZE: usize = 1;

/// The column of modification days.
const MODIFIED: usize = 2;

/// The column of kinds.
const KIND: usize = 3;

/// The extensions the generated files end in, with the kind each names.
const KINDS: [(&str, &str); 6] = [
    ("txt", "Text"),
    ("rs", "Rust source"),
    ("png", "PNG image"),
    ("pdf", "PDF document"),
    ("toml", "TOML file"),
    ("md", "Markdown"),
];

/// The words file names are made of.
const WORDS: [&str; 8] = [
    "alpha", "bravo", "config", "delta", "editor", "files", "garden", "harbor",
];

/// The bytes in a kilobyte, for sizes.
const KILOBYTE: u64 = 1024;

/// The accessibility id of the table.
const TABLE: FocusId = FocusId::new("files");

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the table of files and a status line.
struct TableDemo {
    /// The files, in the order shown.
    files: Files,
    /// What happened last, for the status line.
    status: String,
    /// The table widget.
    table: Table,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

/// The generated files, in the order shown.
struct Files {
    /// Every file.
    all: Vec<File>,
    /// Indices into `all`, in the order shown.
    order: Vec<usize>,
}

/// One generated file.
struct File {
    /// Days since the epoch it was last modified.
    day: u32,
    /// The index of its kind in [`KINDS`].
    kind: usize,
    /// Its name.
    name: String,
    /// Its size in bytes.
    size: u64,
}

// ========================================================================
// Files Implementations
// ========================================================================

impl Files {
    /// [`FILE_COUNT`] files with names, sizes, and dates made up from
    /// their index.
    fn generate() -> Self {
        let all: Vec<File> = (0..FILE_COUNT)
            .map(|index| {
                let kind = index % KINDS.len();
                let word = WORDS[(index / KINDS.len()) % WORDS.len()];
                File {
                    day: 19_000 + (index * 7919 % 2000) as u32,
                    kind,
                    name: format!("{word}_{index}.{}", KINDS[kind].0),
                    size: (index as u64 * 104_729) % (64 * KILOBYTE * KILOBYTE),
                }
            })
            .collect();
        let order = (0..all.len()).collect();
        Self { all, order }
    }

    /// The file shown at `row`.
    fn at(&self, row: usize) -> &File {
        &self.all[self.order[row]]
    }

    /// Sorts the files by `sort`, ties by name.
    fn sort_by(&mut self, sort: SortKey) {
        let all = &self.all;
        self.order.sort_by(|&a, &b| {
            let (a, b) = (&all[a], &all[b]);
            let order = match sort.column {
                SIZE => a.size.cmp(&b.size),
                MODIFIED => a.day.cmp(&b.day),
                KIND => KINDS[a.kind].1.cmp(KINDS[b.kind].1),
                _ => Ordering::Equal,
            }
            .then_with(|| a.name.cmp(&b.name));
            match sort.direction {
                SortDirection::Ascending => order,
                SortDirection::Descending => order.reverse(),
            }
        });
    }

    /// Moves the row at `from` into the gap before row `before` (counted
    /// before it is taken out); where it ends up.
    fn move_row(&mut self, from: usize, before: usize) -> usize {
        let to = match before > from {
            true => before - 1,
            false => before,
        };
        let file = self.order.remove(from);
        self.order.insert(to, file);
        to
    }
}

impl TableSource for Files {
    fn row_count(&self) -> usize {
        self.order.len()
    }

    fn cell_text(&self, row: usize, column: usize) -> Cow<'_, str> {
        let file = self.at(row);
        match column {
            NAME => Cow::Borrowed(&file.name),
            SIZE => Cow::Owned(size_text(file.size)),
            MODIFIED => Cow::Owned(date_text(file.day)),
            _ => Cow::Borrowed(KINDS[file.kind].1),
        }
    }
}

// ========================================================================
// TableDemo Implementations
// ========================================================================

impl TableDemo {
    /// Acts on what the table made of an event; whether it used it.
    fn settle(&mut self, response: TableResponse) -> bool {
        match response {
            TableResponse::Activated(row) => {
                self.status = format!("opened {}", self.files.at(row).name);
            }
            TableResponse::ColumnResized(column) => {
                let widths = self.table.column_widths();
                println!("save column widths {widths:?}");
                let title = self.table.columns()[column].title().to_owned();
                self.status = format!("resized {title}: save widths {widths:?}");
            }
            TableResponse::CursorMoved(_) | TableResponse::Handled => {}
            TableResponse::Dropped(drop) => {
                if let Some(before) = drop.before {
                    let to = self.files.move_row(drop.row, before);
                    self.table.state_mut().set_cursor(to);
                    // A moved row breaks the column order.
                    self.table.set_sort(None);
                }
            }
            TableResponse::HeaderMenu(menu) => {
                // A real column chooser opens a `Menu` of the columns at
                // `menu.position`; this demo hides the column clicked, and
                // brings every column back on a click past the last.
                match menu.column.filter(|&column| column > 0) {
                    Some(column) => {
                        self.table.set_column_visible(column, false);
                        let title = self.table.columns()[column].title().to_owned();
                        self.status =
                            format!("hid {title}; right-click past the last column to show all");
                    }
                    None => {
                        for column in 0..self.table.columns().len() {
                            self.table.set_column_visible(column, true);
                        }
                        self.status = "showing every column".into();
                    }
                }
            }
            TableResponse::Ignored => return false,
            TableResponse::Sorted(sort) => self.sort(sort),
            _ => {}
        }
        true
    }

    /// Sorts the files by `sort`, keeping the cursor on the file it was on;
    /// the selection, by row, is dropped.
    fn sort(&mut self, sort: SortKey) {
        let cursor = self.table.state().cursor();
        let file = self.files.order.get(cursor).copied();
        self.files.sort_by(sort);
        self.table.set_sort(Some(sort));
        let state = self.table.state_mut();
        state.clear_selection();
        if let Some(row) = file.and_then(|file| self.files.order.iter().position(|&f| f == file)) {
            state.set_cursor(row);
        }
        let title = self.table.columns()[sort.column].title().to_owned();
        self.status = format!("sorted by {title}, {:?}", sort.direction);
    }
}

impl Application for TableDemo {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        self.table
            .lend(&self.files)
            .about_to_wait(&mut cx.event_cx(window))
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        let response = self
            .table
            .lend(&self.files)
            .handle(input, &mut cx.event_cx(window));
        if self.settle(response) {
            cx.event_cx(window).request_redraw();
            return;
        }
        let escape = Input::Key(Key::new(KeyCode::Esc, Mods::NONE));
        if *input == escape || *input == Input::CloseRequested {
            cx.exit();
        }
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.background.into());
        let bounds = frame.bounds();
        let metrics = frame.cell_metrics();
        let table_area = Rect::new(
            bounds.x,
            bounds.y,
            bounds.width,
            (bounds.height - metrics.height).max(0.0),
        );
        self.table
            .lend(&self.files)
            .draw(frame, table_area, &self.theme);
        self.table.access(frame, TABLE, &self.files);
        let state = self.table.state();
        let status = StyledText::new()
            .with(Span::new(
                format!(
                    " row {} of {}, {} selected   {}{}   ",
                    state.cursor() + 1,
                    state.count(),
                    state.selected().count(),
                    self.table
                        .keys()
                        .count()
                        .map_or_else(String::new, |count| count.to_string()),
                    key_label(self.table.keys().pending_keys(), &self.theme.strings),
                ),
                ThemeToken::Muted.into(),
            ))
            .with(Span::new(self.status.as_str(), ThemeToken::Accent.into()))
            .with_ellipsis();
        let y = table_area.y + table_area.height;
        let clip = Rect::new(bounds.x, y, bounds.width, metrics.height);
        frame.draw_text(&status, Point::new(bounds.x, y), clip, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `bytes` as people read a size: bytes, KB, or MB with one decimal.
fn size_text(bytes: u64) -> String {
    let kilobytes = bytes as f64 / KILOBYTE as f64;
    match bytes {
        bytes if bytes < KILOBYTE => format!("{bytes} B"),
        bytes if bytes < KILOBYTE * KILOBYTE => format!("{kilobytes:.1} KB"),
        _ => format!("{:.1} MB", kilobytes / KILOBYTE as f64),
    }
}

/// `day`, counted from 1970-01-01, as a year-month-day date.
fn date_text(day: u32) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let z = day as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn main() -> Result<(), Error> {
    let table = Table::new(vec![
        TableColumn::new("Name", 280.0)
            .with_min_width(80.0)
            .with_fill(true),
        TableColumn::new("Size", 100.0).with_align(TextAlign::End),
        TableColumn::new("Modified", 120.0),
        TableColumn::new("Kind", 160.0),
    ]);
    let app = TableDemo {
        files: Files::generate(),
        status: String::from("click a title to sort, drag a border to resize"),
        table,
        theme: Theme::dark(),
        window: None,
    };
    misty::run(app, WindowOptions::new(TITLE))
}
