//! A virtualized list of 100,000 rows, described as a tree: only the rows
//! on screen are asked for and drawn. The list's own keys move it as Vim
//! moves over lines: Up and Down (or `j` and `k`, `C-p` and `C-n`) move
//! the cursor, `C-f` and `C-b` (Page Down and Page Up) page, `C-d` and
//! `C-u` move half a page, `C-e` and `C-y` scroll, `H`, `M`, and `L` go to
//! the top, middle, and bottom of the screen, `g g` and `G` (Home and End)
//! jump to the ends, `z t`, `z z`, and `z b` scroll the cursor's row to
//! the top, middle, or bottom, and a count repeats a motion or names a row
//! (`5j`, `10G`). The status line shows a pending count or prefix, and a
//! prefix left waiting opens a which-key popup of what may follow. Space
//! toggles the cursor row's selection, Enter activates it. Click a row to
//! move the cursor, Ctrl+click to toggle its selection, double-click to
//! activate it, and use the wheel to scroll. Drag a row to move it: a
//! line shows where it will land. The rows glide to each new scroll
//! position; run with `--reduced-motion` to make them jump. Esc quits.
//!
//! ```sh
//! cargo run -p misty --example list
//! ```

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::env;

use misty::view::{column, list, text};
use misty::{
    App, AppCx, Element, Key, KeyCode, KeyScheme, ListKeys, ListSource, Mods, RowDrop, Theme,
    WindowId, WindowOptions, key_label,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty list";

/// How many rows the list has.
const ROW_COUNT: usize = 100_000;

/// The flag that makes the rows jump rather than glide.
const REDUCED_MOTION: &str = "--reduced-motion";

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a list over generated rows and a status line.
struct ListDemo {
    /// The row the cursor is on.
    cursor: usize,
    /// The count and prefix typed toward a motion, for the status line.
    keys: ListKeys,
    /// The row last activated, shown in the status line.
    last_activated: Option<usize>,
    /// Whether `--reduced-motion` asked for jumps whatever the system says.
    reduced_motion: bool,
    /// The rows.
    rows: Rows,
    /// The rows selected.
    selected: BTreeSet<usize>,
    /// The colors the window is drawn in.
    theme: Theme,
}

/// Rows generated from their index, in an order drags change.
struct Rows {
    /// The generated row shown at each place, moved by drags.
    order: Vec<usize>,
}

// ========================================================================
// ListSource Implementations
// ========================================================================

impl ListSource for Rows {
    fn row_count(&self) -> usize {
        self.order.len()
    }

    fn row_text(&self, row: usize) -> Cow<'_, str> {
        let row = self.order[row];
        Cow::Owned(format!(
            "Row {row:>6}   square {:>12}   hex {row:#07x}",
            row * row
        ))
    }
}

// ========================================================================
// Rows Implementations
// ========================================================================

impl Rows {
    /// Moves the row at `from` into the gap before row `before` (counted
    /// before it is taken out); where it ends up.
    fn move_row(&mut self, from: usize, before: usize) -> usize {
        let to = match before > from {
            true => before - 1,
            false => before,
        };
        let row = self.order.remove(from);
        self.order.insert(to, row);
        to
    }
}

// ========================================================================
// ListDemo Implementations
// ========================================================================

impl ListDemo {
    /// The demo at start, its rows in order, jumping rather than gliding
    /// when `reduced_motion`.
    fn new(reduced_motion: bool) -> Self {
        Self {
            cursor: 0,
            keys: ListKeys::default(),
            last_activated: None,
            reduced_motion,
            rows: Rows {
                order: (0..ROW_COUNT).collect(),
            },
            selected: BTreeSet::new(),
            theme: Theme::default(),
        }
    }

    /// Moves a dropped row into the gap it was dropped in, the cursor with
    /// it; a row dropped off the list stays.
    fn drop_row(&mut self, drop: RowDrop) {
        if let Some(before) = drop.before {
            self.cursor = self.rows.move_row(drop.row, before);
        }
    }

    /// What the status line says.
    fn status(&self) -> String {
        let activated = self
            .last_activated
            .map_or_else(|| "none".to_owned(), |row| row.to_string());
        let count = self
            .keys
            .count()
            .map_or_else(String::new, |count| count.to_string());
        format!(
            " cursor on row {} of {}   {} selected   last activated: {activated}   {count}{}",
            self.cursor,
            self.rows.row_count(),
            self.selected.len(),
            key_label(self.keys.pending_keys(), &self.theme.strings),
        )
    }
}

impl App for ListDemo {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        column([
            list(&self.rows, self.cursor)
                .selected(&self.selected)
                .which_key()
                .on_cursor_move(|demo: &mut Self, row| demo.cursor = row)
                .on_select(|demo: &mut Self, rows| demo.selected = rows)
                .on_keys(|demo: &mut Self, keys| demo.keys = keys)
                .on_activate(|demo: &mut Self, row| demo.last_activated = Some(row))
                .on_drop(Self::drop_row)
                .into(),
            text(self.status()).into(),
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

    fn reduced_motion(&self) -> Option<bool> {
        self.reduced_motion.then_some(true)
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

fn main() -> Result<(), misty::Error> {
    let demo = ListDemo::new(env::args().any(|arg| arg == REDUCED_MOTION));
    misty::run_app(demo, WindowOptions::new(TITLE))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, feature = "testing"))]
mod tests {
    use misty::accesskit::Role;
    use misty::testing::{TreeWindow, access_tree};
    use misty::{ButtonInput, Input, Point, PointerButton};

    use super::*;

    /// The window's width in the tests, in physical pixels.
    const WIDTH: u32 = 800;

    /// The window's height in the tests.
    const HEIGHT: u32 = 600;

    /// What the hand-driven version of this example (an `Application`
    /// lending its rows to a `List` by hand) described to screen readers at
    /// start, captured from it before it was rewritten: nothing but the
    /// window, since it never called `List::access`.
    const HAND_DRIVEN: &str = "Window \"misty list\" focused\n";

    /// The status line the hand-driven version drew at start, in the
    /// pieces its cell grid drew it in.
    const HAND_DRIVEN_STATUS: [&str; 11] = [
        "cursor",
        "on",
        "row",
        "0",
        "of",
        "100000",
        "0",
        "selected",
        "last",
        "activated:",
        "none",
    ];

    /// How many rows the hand-driven version drew at start.
    const HAND_DRIVEN_ROWS: usize = 20;

    /// The window showing the demo at start, the rows jumping.
    fn window() -> TreeWindow<ListDemo> {
        TreeWindow::new(ListDemo::new(true), TITLE, WIDTH, HEIGHT)
    }

    /// The status line `window` drew last.
    fn status(window: &TreeWindow<ListDemo>) -> String {
        window.drawn_text().last().cloned().unwrap_or_default()
    }

    /// The status line's words.
    fn words(status: &str) -> Vec<&str> {
        status.split_whitespace().collect()
    }

    /// The middle of row `row`'s place in `window`'s description.
    fn row_center(window: &TreeWindow<ListDemo>, row: usize) -> Point {
        let name = window.app().rows.row_text(row).into_owned();
        let tree = access_tree(window.frame(), TITLE);
        let (_, node) = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::ListItem && node.label() == Some(&name))
            .unwrap_or_else(|| panic!("row {row} is not in sight"));
        let bounds = node.bounds().expect("a row has bounds");
        Point::new(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        )
    }

    /// A left click at `point` with `mods` held.
    fn click(window: &mut TreeWindow<ListDemo>, point: Point, mods: Mods) {
        window.send(&Input::PointerMoved(point));
        let mut press = ButtonInput::press(PointerButton::Left, point);
        press.mods = mods;
        window.send(&Input::PointerButton(press));
        let mut release = ButtonInput::release(PointerButton::Left, point);
        release.mods = mods;
        window.send(&Input::PointerButton(release));
    }

    #[test]
    fn test_the_tree_draws_what_the_hand_driven_example_drew() {
        let window = window();
        let drawn = window.drawn_text();
        let rows: Vec<String> = (0..HAND_DRIVEN_ROWS)
            .map(|row| window.app().rows.row_text(row).into_owned())
            .collect();
        assert_eq!(drawn[..HAND_DRIVEN_ROWS], rows[..]);
        assert_eq!(
            drawn.len(),
            HAND_DRIVEN_ROWS + 1,
            "the rows, then the status"
        );
        // The status is one line of text now, not cells word by word.
        assert_eq!(words(&status(&window)), HAND_DRIVEN_STATUS);
    }

    #[test]
    fn test_the_tree_describes_the_rows_the_hand_driven_example_left_out() {
        // The tree describes every element: the list and its rows in sight,
        // the cursor's row focused rather than the window.
        let window = window();
        let mut expected = HAND_DRIVEN.replace(" focused", "") + "  List\n";
        for row in 0..HAND_DRIVEN_ROWS {
            let focused = if row == 0 { " focused" } else { "" };
            let name = window.app().rows.row_text(row).into_owned();
            expected += &format!("    ListItem {name:?}{focused}\n");
        }
        assert_eq!(window.ui_text(), expected);
    }

    #[test]
    fn test_keys_move_select_count_and_activate_as_the_hand_driven_example_did() {
        let mut window = window();
        window.press("j j <space> j 5");
        // The hand-driven example's status after the same keys.
        let after = "cursor on row 3 of 100000 1 selected last activated: none 5";
        assert_eq!(words(&status(&window)).join(" "), after);
        window.press("j RET");
        let after = "cursor on row 8 of 100000 1 selected last activated: 8";
        assert_eq!(words(&status(&window)).join(" "), after);
        assert_eq!(window.app().selected, BTreeSet::from([2]));
    }

    #[test]
    fn test_a_prefix_waits_in_the_status_line_until_its_motion() {
        let mut window = window();
        window.press("g");
        assert!(status(&window).ends_with('g'), "{}", status(&window));
        window.press("g G");
        assert_eq!(window.app().cursor, ROW_COUNT - 1);
        assert!(status(&window).contains("cursor on row 99999 of 100000"));
    }

    #[test]
    fn test_a_click_moves_the_cursor_and_ctrl_click_selects() {
        let mut window = window();
        let point = row_center(&window, 4);
        click(&mut window, point, Mods::NONE);
        assert_eq!(window.app().cursor, 4);
        let point = row_center(&window, 6);
        click(&mut window, point, Mods::CTRL);
        assert_eq!(window.app().selected, BTreeSet::from([6]));
    }

    #[test]
    fn test_dragging_a_row_moves_it_and_the_cursor_with_it() {
        let mut window = window();
        let from = row_center(&window, 1);
        let to = row_center(&window, 4);
        window.send(&Input::PointerMoved(from));
        let press = ButtonInput::press(PointerButton::Left, from);
        window.send(&Input::PointerButton(press));
        let mut at = from;
        while at.y < to.y {
            at.y = (at.y + 4.0).min(to.y);
            window.send(&Input::PointerMoved(at));
        }
        let release = ButtonInput::release(PointerButton::Left, to);
        window.send(&Input::PointerButton(release));
        let order = &window.app().rows.order;
        assert_ne!(order[1], 1, "row 1 moved: {:?}", &order[..6]);
        let moved_to = order.iter().position(|row| *row == 1);
        assert!(moved_to.is_some_and(|at| at > 1), "{:?}", &order[..6]);
        assert_eq!(
            Some(window.app().cursor),
            moved_to,
            "the cursor went with it"
        );
    }

    #[test]
    fn test_esc_quits_and_other_unused_keys_do_not() {
        let mut window = window();
        window.press("C-x");
        assert!(!window.is_closed());
        window.press("<esc>");
        assert!(window.is_closed());
    }
}
