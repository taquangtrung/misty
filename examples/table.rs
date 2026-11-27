//! A table of 100,000 generated files in four columns, described as a
//! tree: name, size, modified, and kind. Only the rows on screen are asked
//! for and drawn.
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
use std::collections::BTreeSet;

use misty::view::{column, table, text};
use misty::{
    App, AppCx, Element, HeaderMenu, Key, KeyCode, KeyScheme, ListKeys, Mods, RowDrop,
    SortDirection, SortKey, TableColumn, TableSource, TextAlign, Theme, ThemeToken, WindowId,
    WindowOptions, key_label,
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

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the table of files and a status line.
struct TableDemo {
    /// The columns, left to right, hidden ones included.
    columns: Vec<TableColumn>,
    /// The row the cursor is on.
    cursor: usize,
    /// The files, in the order shown.
    files: Files,
    /// The count and prefix typed toward a motion, for the status line.
    keys: ListKeys,
    /// The rows selected.
    selected: BTreeSet<usize>,
    /// The column the files are sorted by, if any.
    sort: Option<SortKey>,
    /// What happened last, for the status line.
    status: String,
    /// The colors the window is drawn in.
    theme: Theme,
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
    /// The demo at start: the files in the order made, unsorted.
    fn new() -> Self {
        Self {
            columns: vec![
                TableColumn::new("Name", 280.0)
                    .with_min_width(80.0)
                    .with_fill(true),
                TableColumn::new("Size", 100.0).with_align(TextAlign::End),
                TableColumn::new("Modified", 120.0),
                TableColumn::new("Kind", 160.0),
            ],
            cursor: 0,
            files: Files::generate(),
            keys: ListKeys::default(),
            selected: BTreeSet::new(),
            sort: None,
            status: String::from("click a title to sort, drag a border to resize"),
            theme: Theme::dark(),
        }
    }

    /// Notes the file at `row` opened.
    fn open(&mut self, row: usize) {
        self.status = format!("opened {}", self.files.at(row).name);
    }

    /// Prints the widths after a drag resized `column`, where a real app
    /// would save them.
    fn resized(&mut self, (column, widths): (usize, Vec<f32>)) {
        println!("save column widths {widths:?}");
        let title = self.columns[column].title().to_owned();
        self.status = format!("resized {title}: save widths {widths:?}");
    }

    /// Moves a dropped row into the gap it was dropped in, the cursor with
    /// it; a moved row breaks the column order.
    fn drop_row(&mut self, drop: RowDrop) {
        if let Some(before) = drop.before {
            self.cursor = self.files.move_row(drop.row, before);
            self.sort = None;
        }
    }

    /// Hides the column a right click on the header landed on, or brings
    /// every column back on one past the last. A real column chooser
    /// opens a `Menu` of the columns at `menu.position`.
    fn header_menu(&mut self, menu: HeaderMenu) {
        match menu.column.filter(|&column| column > 0) {
            Some(column) => {
                self.columns[column] = self.columns[column].clone().with_hidden(true);
                let title = self.columns[column].title().to_owned();
                self.status = format!("hid {title}; right-click past the last column to show all");
            }
            None => {
                for column in &mut self.columns {
                    *column = column.clone().with_hidden(false);
                }
                self.status = "showing every column".into();
            }
        }
    }

    /// Sorts the files by `sort`, keeping the cursor on the file it was on;
    /// the selection, by row, is dropped.
    fn sort(&mut self, sort: SortKey) {
        let file = self.files.order.get(self.cursor).copied();
        self.files.sort_by(sort);
        self.sort = Some(sort);
        self.selected.clear();
        if let Some(row) = file.and_then(|file| self.files.order.iter().position(|&f| f == file)) {
            self.cursor = row;
        }
        let title = self.columns[sort.column].title().to_owned();
        self.status = format!("sorted by {title}, {:?}", sort.direction);
    }

    /// The muted half of the status line: where the cursor is, how many
    /// rows are selected, and what is typed toward a motion.
    fn position(&self) -> String {
        format!(
            " row {} of {}, {} selected   {}{}   ",
            self.cursor + 1,
            self.files.row_count(),
            self.selected.len(),
            self.keys
                .count()
                .map_or_else(String::new, |count| count.to_string()),
            key_label(self.keys.pending_keys(), &self.theme.strings),
        )
    }
}

impl App for TableDemo {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        column([
            table(&self.files, &self.columns, self.cursor)
                .selected(&self.selected)
                .sort(self.sort)
                .on_cursor_move(|demo: &mut Self, row| demo.cursor = row)
                .on_select(|demo: &mut Self, rows| demo.selected = rows)
                .on_keys(|demo: &mut Self, keys| demo.keys = keys)
                .on_activate(Self::open)
                .on_resize(Self::resized)
                .on_drop(Self::drop_row)
                .on_header_menu(Self::header_menu)
                .on_sort(Self::sort)
                .into(),
            text(self.position())
                .muted()
                .then(self.status.as_str())
                .color(ThemeToken::Accent)
                .into(),
        ])
    }

    /// Quits on Esc.
    fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
        if key != Key::new(KeyCode::Esc, Mods::NONE) {
            return false;
        }
        cx.exit();
        true
    }

    /// The Vim and Emacs keys, as a keyboard-first app takes them.
    fn key_scheme(&self) -> KeyScheme {
        KeyScheme::VIM_AND_EMACS
    }

    fn theme(&self) -> &Theme {
        &self.theme
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

fn main() -> Result<(), misty::Error> {
    misty::run_app(TableDemo::new(), WindowOptions::new(TITLE))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, feature = "testing"))]
mod tests {
    use misty::accesskit::Role;
    use misty::testing::TreeWindow;
    use misty::{ButtonInput, Input, Point, PointerButton};

    use super::*;

    /// The window's width in the tests, in physical pixels.
    const WIDTH: u32 = 800;

    /// The window's height in the tests.
    const HEIGHT: u32 = 600;

    /// What the hand-driven version of this example (an `Application`
    /// lending its files to a `Table` by hand) described to screen readers
    /// at start, captured from it before it was rewritten, cut to the rows
    /// that fit once rows took the theme's padding.
    const HAND_DRIVEN: &str = "\
Window \"misty table\" focused
  Table
    Row
      ColumnHeader \"Name\"
      ColumnHeader \"Size\"
      ColumnHeader \"Modified\"
      ColumnHeader \"Kind\"
    Row
      Cell \"alpha_0.txt\"
      Cell \"0 B\"
      Cell \"2022-01-08\"
      Cell \"Text\"
    Row
      Cell \"alpha_1.rs\"
      Cell \"102.3 KB\"
      Cell \"2027-04-11\"
      Cell \"Rust source\"
    Row
      Cell \"alpha_2.png\"
      Cell \"204.5 KB\"
      Cell \"2027-01-20\"
      Cell \"PNG image\"
    Row
      Cell \"alpha_3.pdf\"
      Cell \"306.8 KB\"
      Cell \"2026-10-31\"
      Cell \"PDF document\"
    Row
      Cell \"alpha_4.toml\"
      Cell \"409.1 KB\"
      Cell \"2026-08-11\"
      Cell \"TOML file\"
    Row
      Cell \"alpha_5.md\"
      Cell \"511.4 KB\"
      Cell \"2026-05-22\"
      Cell \"Markdown\"
    Row
      Cell \"bravo_6.txt\"
      Cell \"613.6 KB\"
      Cell \"2026-03-02\"
      Cell \"Text\"
    Row
      Cell \"bravo_7.rs\"
      Cell \"715.9 KB\"
      Cell \"2025-12-11\"
      Cell \"Rust source\"
    Row
      Cell \"bravo_8.png\"
      Cell \"818.2 KB\"
      Cell \"2025-09-21\"
      Cell \"PNG image\"
    Row
      Cell \"bravo_9.pdf\"
      Cell \"920.5 KB\"
      Cell \"2025-07-02\"
      Cell \"PDF document\"
    Row
      Cell \"bravo_10.toml\"
      Cell \"1022.7 KB\"
      Cell \"2025-04-12\"
      Cell \"TOML file\"
    Row
      Cell \"bravo_11.md\"
      Cell \"1.1 MB\"
      Cell \"2025-01-21\"
      Cell \"Markdown\"
    Row
      Cell \"config_12.txt\"
      Cell \"1.2 MB\"
      Cell \"2024-11-01\"
      Cell \"Text\"
    Row
      Cell \"config_13.rs\"
      Cell \"1.3 MB\"
      Cell \"2024-08-12\"
      Cell \"Rust source\"
    Row
      Cell \"config_14.png\"
      Cell \"1.4 MB\"
      Cell \"2024-05-23\"
      Cell \"PNG image\"
    Row
      Cell \"config_15.pdf\"
      Cell \"1.5 MB\"
      Cell \"2024-03-03\"
      Cell \"PDF document\"
    Row
      Cell \"config_16.toml\"
      Cell \"1.6 MB\"
      Cell \"2023-12-13\"
      Cell \"TOML file\"
    Row
      Cell \"config_17.md\"
      Cell \"1.7 MB\"
      Cell \"2023-09-23\"
      Cell \"Markdown\"
    Row
      Cell \"delta_18.txt\"
      Cell \"1.8 MB\"
      Cell \"2023-07-04\"
      Cell \"Text\"
";

    /// The status line the hand-driven version drew at start.
    const HAND_DRIVEN_STATUS: &str =
        " row 1 of 100000, 0 selected      click a title to sort, drag a border to resize";

    /// The window showing the demo at start.
    fn window() -> TreeWindow<TableDemo> {
        TreeWindow::new(TableDemo::new(), TITLE, WIDTH, HEIGHT)
    }

    /// The status line `window` drew last.
    fn status(window: &TreeWindow<TableDemo>) -> String {
        window.drawn_text().last().cloned().unwrap_or_default()
    }

    /// A left click at `point` with `button`.
    fn click(window: &mut TreeWindow<TableDemo>, point: Point, button: PointerButton) {
        window.send(&Input::PointerMoved(point));
        window.send(&Input::PointerButton(ButtonInput::press(button, point)));
        window.send(&Input::PointerButton(ButtonInput::release(button, point)));
    }

    /// The hand-driven description with the edits the tree makes: the
    /// keyboard is on the table, so the cursor's row (`cursor`) is focused
    /// rather than the window, and the rows `selected` are selected.
    fn tree_described(cursor: usize, selected: &[usize]) -> String {
        // The window, the table, and the header row with its four titles
        // come first; then each row's line and its four cells.
        let row_line = |row: usize| 7 + 5 * row;
        HAND_DRIVEN
            .lines()
            .enumerate()
            .map(|(line, text)| {
                let text = match line {
                    0 => text.trim_end_matches(" focused").to_owned(),
                    _ if selected.iter().any(|row| row_line(*row) == line) => {
                        format!("{text} selected")
                    }
                    _ => text.to_owned(),
                };
                match line == row_line(cursor) {
                    true => format!("{text} focused\n"),
                    false => format!("{text}\n"),
                }
            })
            .collect()
    }

    #[test]
    fn test_the_tree_describes_what_the_hand_driven_example_did() {
        assert_eq!(window().ui_text(), tree_described(0, &[]));
    }

    #[test]
    fn test_the_status_line_says_what_the_hand_driven_example_said() {
        assert_eq!(status(&window()), HAND_DRIVEN_STATUS);
    }

    #[test]
    fn test_keys_select_and_open_as_the_hand_driven_example_did() {
        let mut window = window();
        window.press("j j <space> j RET");
        // The hand-driven example's status after the same keys.
        assert_eq!(
            status(&window),
            " row 4 of 100000, 1 selected      opened alpha_3.pdf"
        );
        assert_eq!(window.ui_text(), tree_described(3, &[2]));
    }

    #[test]
    fn test_a_click_on_a_title_sorts_and_keeps_the_cursor_on_its_file() {
        let mut window = window();
        window.press("5 j");
        let file = window.app().files.order[5];
        window.click(Role::ColumnHeader, "Size");
        let sort = window.app().sort.expect("the files are sorted");
        assert_eq!(
            (sort.column, sort.direction),
            (SIZE, SortDirection::Ascending)
        );
        assert_eq!(window.app().files.order[window.app().cursor], file);
        assert!(
            status(&window).ends_with("sorted by Size, Ascending"),
            "{}",
            status(&window)
        );
        window.click(Role::ColumnHeader, "Size");
        assert_eq!(
            window.app().sort.map(|sort| sort.direction),
            Some(SortDirection::Descending)
        );
    }

    #[test]
    fn test_a_right_click_on_a_title_hides_its_column() {
        let mut window = window();
        assert!(window.ui_text().contains("ColumnHeader \"Kind\""));
        let point = header_center(&window, "Kind");
        click(&mut window, point, PointerButton::Right);
        assert!(window.app().columns[KIND].is_hidden());
        assert!(!window.ui_text().contains("ColumnHeader \"Kind\""));
        assert!(
            status(&window).ends_with("hid Kind; right-click past the last column to show all")
        );
    }

    #[test]
    fn test_dragging_a_border_resizes_its_column_and_reports_the_widths() {
        let mut window = window();
        let tree = misty::testing::access_tree(window.frame(), TITLE);
        let (_, size) = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::ColumnHeader && node.label() == Some("Size"))
            .expect("the title is described");
        let bounds = size.bounds().expect("a title has bounds");
        let edge = Point::new(
            bounds.x1 as f32 - 1.0,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        );
        window.send(&Input::PointerMoved(edge));
        window.send(&Input::PointerButton(ButtonInput::press(
            PointerButton::Left,
            edge,
        )));
        let wider = Point::new(edge.x + 40.0, edge.y);
        window.send(&Input::PointerMoved(wider));
        window.send(&Input::PointerButton(ButtonInput::release(
            PointerButton::Left,
            wider,
        )));
        let status = status(&window);
        assert!(
            status.contains("resized Size: save widths [280.0, 140.0, 120.0, 160.0]"),
            "{status}"
        );
    }

    /// The middle of the column title `title`.
    fn header_center(window: &TreeWindow<TableDemo>, title: &str) -> Point {
        let tree = misty::testing::access_tree(window.frame(), TITLE);
        let (_, node) = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::ColumnHeader && node.label() == Some(title))
            .expect("the title is described");
        let bounds = node.bounds().expect("a title has bounds");
        Point::new(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        )
    }

    #[test]
    fn test_esc_quits() {
        let mut window = window();
        window.press("<esc>");
        assert!(window.is_closed());
    }
}
