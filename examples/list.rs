//! A virtualized list of 100,000 rows: only the rows on screen are asked
//! for and drawn. The list's own keys move it as Vim moves over lines:
//! Up and Down (or `j` and `k`, `C-p` and `C-n`) move the cursor, `C-f`
//! and `C-b` (Page Down and Page Up) page, `C-d` and `C-u` move half a
//! page, `C-e` and `C-y` scroll, `H`, `M`, and `L` go to the top, middle,
//! and bottom of the screen, `g g` and `G` (Home and End) jump to the
//! ends, `z t`, `z z`, and `z b` scroll the cursor's row to the top,
//! middle, or bottom, and a count repeats a motion or names a row (`5j`,
//! `10G`). The status line shows a pending count or prefix, and a prefix
//! left waiting opens a which-key popup of what may follow. Space toggles
//! the cursor row's selection, Enter activates it. Click a row to move
//! the cursor,
//! Ctrl+click to toggle its selection, double-click to activate it, and use
//! the wheel to scroll. Drag a row to move it: a line shows where it will
//! land. The rows glide to each new scroll position; run with
//! `--reduced-motion` to make them jump. Esc quits.
//!
//! ```sh
//! cargo run -p misty --example list
//! ```

use std::borrow::Cow;
use std::env;
use std::time::Instant;

use misty::{
    AppContext, Application, Cell, CellBuffer, CellStyle, Error, Frame, Input, Key, KeyCode, List,
    ListResponse, ListSource, Mods, Point, Rect, Theme, WhichKey, Widget, WindowId, WindowOptions,
    key_label,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty list";

/// How many rows the list has.
const ROW_COUNT: usize = 100_000;

/// The rows the status line takes at the bottom.
const STATUS_ROWS: usize = 1;

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a list over generated rows and a status line.
struct ListDemo {
    /// The row last activated, shown in the status line.
    last_activated: Option<usize>,
    /// The list widget.
    list: List,
    /// Whether `--reduced-motion` asked for jumps whatever the system says.
    reduced_motion: bool,
    /// The rows.
    rows: Rows,
    /// The status line, rebuilt every frame.
    status: CellBuffer,
    /// The colors the window is drawn in.
    theme: Theme,
    /// What may follow a prefix left waiting.
    which_key: WhichKey,
    /// The open window, once it is open.
    window: Option<WindowId>,
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
    /// Acts on what the list made of a key or an event; whether it used it.
    fn settle(&mut self, response: ListResponse) -> bool {
        match response {
            ListResponse::Activated(row) => self.last_activated = Some(row),
            ListResponse::CursorMoved(_) | ListResponse::Handled => {}
            ListResponse::Dropped(drop) => {
                if let Some(before) = drop.before {
                    let to = self.rows.move_row(drop.row, before);
                    self.list.state_mut().set_cursor(to);
                }
            }
            ListResponse::Ignored => return false,
            _ => {}
        }
        true
    }
}

impl Application for ListDemo {
    type Message = ();

    fn init(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
        if self.reduced_motion {
            cx.set_reduced_motion(Some(true));
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        let mut window_cx = cx.event_cx(window);
        let glide = self.list.lend(&self.rows).about_to_wait(&mut window_cx);
        let which_key = self.which_key.about_to_wait(&mut window_cx);
        glide.into_iter().chain(which_key).min()
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        let response = self
            .list
            .lend(&self.rows)
            .handle(input, &mut cx.event_cx(window));
        let list_keys = self.list.keys();
        if self.which_key.set_pending(
            list_keys.pending_keys(),
            list_keys.hints(&self.theme.strings),
        ) {
            cx.event_cx(window).request_redraw();
        }
        if self.settle(response) {
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
        let status_height = STATUS_ROWS as f32 * metrics.height;
        let list_area = Rect::new(
            bounds.x,
            bounds.y,
            bounds.width,
            (bounds.height - status_height).max(0.0),
        );
        self.list
            .lend(&self.rows)
            .draw(frame, list_area, &self.theme);
        let state = self.list.state();
        let activated = self
            .last_activated
            .map_or_else(|| "none".to_owned(), |row| row.to_string());
        let list_keys = self.list.keys();
        let count = list_keys
            .count()
            .map_or_else(String::new, |count| count.to_string());
        let status = format!(
            " cursor on row {} of {}   {} selected   last activated: {activated}   {count}{}",
            state.cursor(),
            state.count(),
            state.selected().count(),
            key_label(list_keys.pending_keys(), &self.theme.strings),
        );
        self.status
            .resize(metrics.cols_in(bounds.width), STATUS_ROWS);
        self.status.clear();
        let style = CellStyle::default();
        for (col, ch) in status.chars().enumerate() {
            self.status.set(0, col, Cell::new(ch, style));
        }
        let origin = Point::new(bounds.x, list_area.y + list_area.height);
        frame.draw_cells(&self.status, origin, &self.theme);
        self.which_key.draw(frame, list_area, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

fn main() -> Result<(), Error> {
    let app = ListDemo {
        last_activated: None,
        list: List::new(),
        reduced_motion: env::args().any(|arg| arg == "--reduced-motion"),
        rows: Rows {
            order: (0..ROW_COUNT).collect(),
        },
        status: CellBuffer::new(0, 0),
        theme: Theme::default(),
        which_key: WhichKey::new(),
        window: None,
    };
    misty::run(app, WindowOptions::new(TITLE))
}
