//! Frame timings for the views that draw the most text: a list of 100,000
//! rows scrolled a row a frame, a window-filling cell grid, both still
//! and scrolling, a dialog of wrapped paragraphs, and a book chapter as
//! rich text, laid out and scrolled or paged and turned; the cost of
//! rebuilding an element tree (a form of controls) on every frame and
//! every input; and the list and a table of 100,000 rows drawn by hand
//! beside the same views as tree elements lent the rows. Ignored by
//! `cargo test`; `make bench` runs them in a release build and prints the
//! average and slowest frame.

use std::borrow::Cow;
use std::iter;
use std::time::{Duration, Instant};

use super::app::focus::FocusId;
use super::app::offscreen::{self, FrameTimes};
use super::app::widget::Widget;
use super::base::cell::{Cell, CellBuffer, CellSource, CellStyle};
use super::base::color::Rgba;
use super::base::geometry::{Point, Rect};
use super::base::key::{Key, KeyCode, Mods};
use super::base::styled::{Span, TextColor};
use super::base::theme::{Theme, ThemeToken};
use super::input::event::Input;
use super::platform::window_id::WindowId;
use super::render::paragraph::Paragraph;
use super::testing::TestWindow;
use super::view::app::App;
use super::view::controls::{checkbox, slider, text_field};
use super::view::data::{list, table};
use super::view::element::{Element, column, field, form};
use super::view::store::Store;
use super::widgets::dialog::{Dialog, DialogButton};
use super::widgets::document::{Block, Document, Heading, Inline};
use super::widgets::list_view::{List, ListSource};
use super::widgets::number_range::NumberRange;
use super::widgets::rich_text::{Paged, RichText};
use super::widgets::table::{Table, TableColumn, TableSource};

// ========================================================================
// Constants
// ========================================================================

/// The frame size, a typical laptop window.
const WIDTH: u32 = 1280;

/// The frame height.
const HEIGHT: u32 = 800;

/// The frames each timing draws.
const FRAMES: usize = 120;

/// The rows of the scrolled list.
const LIST_ROWS: usize = 100_000;

/// The paragraphs in the dialog's body.
const DIALOG_PARAGRAPHS: usize = 4;

/// The sentence each dialog paragraph repeats.
const DIALOG_SENTENCE: &str = "A dialog body wraps its prose in the proportional font. ";

/// How many times each dialog paragraph repeats its sentence.
const DIALOG_REPEATS: usize = 6;

/// The sections of the rich text chapter, each a heading and paragraphs.
const CHAPTER_SECTIONS: usize = 30;

/// The paragraphs under each heading of the chapter.
const SECTION_PARAGRAPHS: usize = 10;

/// The sentence each chapter paragraph repeats.
const CHAPTER_SENTENCE: &str = "A chapter of a book wraps its prose across the column. ";

/// How many times each chapter paragraph repeats its sentence.
const CHAPTER_REPEATS: usize = 8;

/// The pages of a spread when the chapter is paged.
const SPREAD_COLUMNS: usize = 2;

/// How far the chapter scrolls a frame, in physical pixels.
const CHAPTER_SCROLL: f32 = 7.0;

/// The fields of the small tree form, the controls example's size.
const SMALL_FORM: usize = 9;

/// The fields of the large tree form.
const LARGE_FORM: usize = 300;

/// The range of the tree form's sliders.
const FORM_RANGE: NumberRange = NumberRange::new(0.0, 100.0, 1.0);

/// The label of each field of the tree form.
const FORM_LABEL: &str = "Setting";

/// The row the lent views' cursor starts on: below the rows a frame
/// shows, so moving it a row scrolls the view a row.
const CURSOR_START: usize = 100;

/// The table's columns: title and width in logical pixels.
const TABLE_COLUMNS: [(&str, f32); 4] = [
    ("Name", 400.0),
    ("Size", 200.0),
    ("Modified", 200.0),
    ("Kind", 200.0),
];

// ========================================================================
// Data Structures
// ========================================================================

/// Generated rows, as the list example shows.
struct Rows;

/// A tree app showing a scrolling form of sliders, checkboxes, and text
/// fields, one value of each per field.
struct Settings {
    /// Each field's checkbox.
    checks: Vec<bool>,
    /// Each field's text.
    names: Vec<String>,
    /// The colors it is drawn in.
    theme: Theme,
    /// Each field's slider value.
    values: Vec<f64>,
}

/// A tree app showing the 100,000 rows as a list or a table, its cursor
/// moved by the bench.
struct Scroller {
    /// The table's columns.
    columns: Vec<TableColumn>,
    /// The row the cursor is on.
    cursor: usize,
    /// Whether it shows a table rather than a list.
    is_table: bool,
    /// The colors it is drawn in.
    theme: Theme,
}

/// Which lent view a bench draws.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LentView {
    /// A list.
    List,
    /// A table.
    Table,
}

// ========================================================================
// ListSource Implementations
// ========================================================================

impl ListSource for Rows {
    fn row_count(&self) -> usize {
        LIST_ROWS
    }

    fn row_text(&self, row: usize) -> Cow<'_, str> {
        Cow::Owned(format!(
            "Row {row:>6}   square {:>12}   hex {row:#07x}",
            row * row
        ))
    }
}

impl TableSource for Rows {
    fn row_count(&self) -> usize {
        LIST_ROWS
    }

    fn cell_text(&self, row: usize, column: usize) -> Cow<'_, str> {
        Cow::Owned(format!("row {row} column {column}"))
    }
}

// ========================================================================
// Scroller Implementations
// ========================================================================

impl App for Scroller {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        let rows = match self.is_table {
            true => table(&Rows, &self.columns, self.cursor)
                .monospace()
                .on_cursor_move(|app: &mut Self, row| app.cursor = row)
                .into(),
            false => list(&Rows, self.cursor)
                .monospace()
                .on_cursor_move(|app: &mut Self, row| app.cursor = row)
                .into(),
        };
        column([rows])
    }

    fn theme(&self) -> &Theme {
        &self.theme
    }
}

// ========================================================================
// Settings Implementations
// ========================================================================

impl Settings {
    /// A form of `fields` fields at their start values.
    fn new(fields: usize) -> Self {
        Self {
            checks: vec![false; fields],
            names: vec![String::from("name"); fields],
            theme: Theme::dark(),
            values: vec![0.0; fields],
        }
    }
}

impl App for Settings {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        let fields = (0..self.values.len()).map(|index| {
            let control: Element<'_, Self> = match index % 3 {
                0 => slider(FORM_RANGE, self.values[index])
                    .on_change(move |app: &mut Self, value| app.values[index] = value)
                    .into(),
                1 => checkbox("Enabled", self.checks[index])
                    .on_change(move |app: &mut Self, on| app.checks[index] = on)
                    .into(),
                _ => text_field(&self.names[index])
                    .on_change(move |app: &mut Self, name| app.names[index] = name)
                    .into(),
            };
            field(FORM_LABEL, control)
        });
        column([form(fields).padding(2.0, 1.0).scroll()])
    }

    fn theme(&self) -> &Theme {
        &self.theme
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The mean of the times `FRAMES` runs of `step` say they took.
fn mean_of(mut step: impl FnMut() -> Duration) -> Duration {
    (0..FRAMES).map(|_| step()).sum::<Duration>() / FRAMES as u32
}

/// How long `step` took.
fn time(step: impl FnOnce()) -> Duration {
    let start = Instant::now();
    step();
    start.elapsed()
}

/// Times the tree app `Settings` of `fields` fields: rebuilding, laying
/// out, and drawing it into a frame with no GPU; handling a key (a
/// rebuild, the dispatch, and the callback); and whole frames on the GPU.
fn bench_tree_form(fields: usize) {
    let mut app = Settings::new(fields);
    let mut store = Store::default();
    let mut window = TestWindow::new(WIDTH, HEIGHT);
    let id = window.id();
    let build = mean_of(|| {
        let mut frame = window.frame();
        time(|| store.render(&app, id, &mut frame))
    });
    let right = Input::Key(Key::new(KeyCode::Right, Mods::NONE));
    let input = mean_of(|| {
        let cx = window.cx();
        time(|| {
            store.input(&mut app, id, &right, cx);
        })
    });
    println!(
        "bench tree form of {fields}: rebuild, layout, and draw without GPU {:.3} ms, \
         one key {:.3} ms",
        build.as_secs_f64() * 1e3,
        input.as_secs_f64() * 1e3
    );
    let times = offscreen::time_frames(WIDTH, HEIGHT, FRAMES, |frame, _| {
        store.render(&app, id, frame);
    });
    report(&format!("tree form of {fields}"), times);
}

/// The table's columns.
fn table_columns() -> Vec<TableColumn> {
    TABLE_COLUMNS
        .iter()
        .map(|(title, width)| TableColumn::new(*title, *width))
        .collect()
}

/// Times `view` of 100,000 rows drawn by hand (a `List` or `Table` lent
/// the rows, drawn and described) beside the same view as a tree element:
/// both without the GPU, the cursor moved a row each frame so the view
/// scrolls a row, then the tree's key handling and its whole frames on
/// the GPU.
fn bench_lent_view(view: LentView) {
    let theme = Theme::dark();
    let mut window = TestWindow::new(WIDTH, HEIGHT);
    let id = window.id();
    let area = Rect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32);
    let described = FocusId::new("bench");
    let mut row = CURSOR_START;
    let mut list = List::new();
    let mut table = Table::new(table_columns());
    let hand = mean_of(|| {
        row += 1;
        let mut frame = window.frame();
        time(|| match view {
            LentView::List => {
                let state = list.state_mut();
                if state.count() != LIST_ROWS {
                    state.set_count(LIST_ROWS);
                }
                state.set_cursor(row);
                list.lend(&Rows).draw(&mut frame, area, &theme);
                list.access(&mut frame, described, &Rows);
            }
            LentView::Table => {
                let state = table.state_mut();
                if state.count() != LIST_ROWS {
                    state.set_count(LIST_ROWS);
                }
                state.set_cursor(row);
                table.lend(&Rows).draw(&mut frame, area, &theme);
                table.access(&mut frame, described, &Rows);
            }
        })
    });
    let mut app = Scroller {
        columns: table_columns(),
        cursor: CURSOR_START,
        is_table: view == LentView::Table,
        theme: Theme::dark(),
    };
    let mut store = Store::default();
    let tree = mean_of(|| {
        app.cursor += 1;
        let mut frame = window.frame();
        time(|| store.render(&app, id, &mut frame))
    });
    let down = Input::Key(Key::new(KeyCode::Char('j'), Mods::NONE));
    let input = mean_of(|| {
        let cx = window.cx();
        time(|| {
            store.input(&mut app, id, &down, cx);
        })
    });
    println!(
        "bench {view:?} of {LIST_ROWS} rows without GPU: by hand {:.3} ms, as a tree element \
         {:.3} ms a frame, one key {:.4} ms",
        hand.as_secs_f64() * 1e3,
        tree.as_secs_f64() * 1e3,
        input.as_secs_f64() * 1e3
    );
    let start = app.cursor;
    let times = offscreen::time_frames(WIDTH, HEIGHT, FRAMES, |frame, number| {
        app.cursor = start + number;
        store.render(&app, id, frame);
    });
    report(&format!("tree {view:?} scroll"), times);
    if view == LentView::Table {
        let mut table = Table::new(table_columns());
        let times = offscreen::time_frames(WIDTH, HEIGHT, FRAMES, |frame, number| {
            frame.clear(Rgba::from(theme.background));
            table.state_mut().set_count(LIST_ROWS);
            table.state_mut().scroll_by(isize::from(number > 0));
            table.lend(&Rows).draw(frame, area, &theme);
        });
        report("table scroll", times);
    }
}

/// A chapter of headings, each over paragraphs of wrapped prose.
fn chapter() -> Document {
    let paragraph = || {
        Block::Paragraph(vec![Inline::plain(
            CHAPTER_SENTENCE.repeat(CHAPTER_REPEATS),
        )])
    };
    let blocks = (0..CHAPTER_SECTIONS)
        .flat_map(|section| {
            let heading = Block::Heading(Heading {
                id: None,
                inlines: vec![Inline::plain(format!("Section {section}"))],
                level: 2,
            });
            iter::once(heading).chain((0..SECTION_PARAGRAPHS).map(|_| paragraph()))
        })
        .collect();
    Document::new(blocks)
}

/// Times the rich text chapter: laid out anew and drawn without the GPU
/// (the cost of opening it or resizing the window), then scrolled a few
/// pixels a frame on the GPU.
fn bench_rich_text() {
    let theme = Theme::dark();
    let window = TestWindow::new(WIDTH, HEIGHT);
    let area = Rect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32);
    let document = chapter();
    let layout = mean_of(|| {
        let mut view = RichText::document(document.clone());
        let mut frame = window.frame();
        time(|| view.draw(&mut frame, area, &theme))
    });
    println!(
        "bench rich text chapter: layout and draw without GPU {:.3} ms",
        layout.as_secs_f64() * 1e3
    );
    let mut view = RichText::document(document);
    let times = offscreen::time_frames(WIDTH, HEIGHT, FRAMES, |frame, number| {
        frame.clear(Rgba::from(theme.background));
        view.set_scroll_position(0, number as f32 * CHAPTER_SCROLL);
        view.draw(frame, area, &theme);
    });
    report("rich text scroll", times);
    let paged = Some(Paged::new(SPREAD_COLUMNS));
    let cut = mean_of(|| {
        let mut view = RichText::document(chapter());
        view.set_paged(paged);
        let mut frame = window.frame();
        time(|| view.draw(&mut frame, area, &theme))
    });
    println!(
        "bench rich text chapter paged: layout, pages, and draw without GPU {:.3} ms",
        cut.as_secs_f64() * 1e3
    );
    let mut view = RichText::document(chapter());
    view.set_paged(paged);
    let times = offscreen::time_frames(WIDTH, HEIGHT, FRAMES, |frame, _| {
        frame.clear(Rgba::from(theme.background));
        if !view.turn_pages(1) {
            view.set_page(0);
        }
        view.draw(frame, area, &theme);
    });
    report("rich text page turns", times);
}

/// Prints `times` under `name`.
fn report(name: &str, times: Option<FrameTimes>) {
    match times {
        Some(times) => println!(
            "bench {name}: mean {:.2} ms, max {:.2} ms over {FRAMES} frames",
            times.mean.as_secs_f64() * 1e3,
            times.max.as_secs_f64() * 1e3
        ),
        None => println!("bench {name}: skipped, no GPU adapter"),
    }
}

/// A dialog of a few long paragraphs, each with a bold run and a link.
fn long_dialog() -> Dialog<usize> {
    let text = TextColor::Token(ThemeToken::Foreground);
    let body = (0..DIALOG_PARAGRAPHS)
        .map(|index| {
            Paragraph::plain(DIALOG_SENTENCE.repeat(DIALOG_REPEATS), text)
                .with(Span::new("Bold words, then ", text).bold())
                .with_link(
                    Span::new("a link", TextColor::Token(ThemeToken::Accent)),
                    format!("link {index}"),
                )
        })
        .collect();
    Dialog::new("A long dialog", body)
        .with_button(DialogButton::new(0, "Cancel"))
        .with_button(DialogButton::new(1, "OK").primary())
}

/// A grid filling the frame, its rows of text starting at line `first`.
fn fill_grid(grid: &mut CellBuffer, first: usize) {
    let (cols, rows) = (grid.cols(), grid.rows());
    for row in 0..rows {
        let line = format!(
            "{:>6}  fn draw_frame(&mut self, frame: &mut Frame) {{ let x = {}; }}",
            first + row,
            (first + row) * 7
        );
        let mut chars = line.chars().cycle();
        for col in 0..cols {
            let ch = chars.next().unwrap_or(' ');
            grid.set(row, col, Cell::new(ch, CellStyle::default()));
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "a timing harness: make bench"]
    fn bench_list_scrolling_a_row_a_frame() {
        let theme = Theme::dark();
        let mut list = List::new();
        let area = Rect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32);
        let times = offscreen::time_frames(WIDTH, HEIGHT, FRAMES, |frame, number| {
            frame.clear(Rgba::from(theme.background));
            list.state_mut().set_count(LIST_ROWS);
            list.state_mut().scroll_by(isize::from(number > 0));
            list.lend(&Rows).draw(frame, area, &theme);
        });
        report("list scroll", times);
    }

    #[test]
    #[ignore = "a timing harness: make bench"]
    fn bench_cell_grid_still_and_scrolling() {
        let theme = Theme::dark();
        let mut grid = CellBuffer::new(0, 0);
        let mut first = None;
        let still = offscreen::time_frames(WIDTH, HEIGHT, FRAMES, |frame, _| {
            let metrics = frame.cell_metrics();
            if first.is_none() {
                grid.resize(
                    metrics.cols_in(WIDTH as f32),
                    metrics.rows_in(HEIGHT as f32),
                );
                fill_grid(&mut grid, 0);
                first = Some(0);
            }
            frame.draw_cells(&grid, Point::new(0.0, 0.0), &theme);
        });
        report("grid still", still);
        let scrolling = offscreen::time_frames(WIDTH, HEIGHT, FRAMES, |frame, number| {
            fill_grid(&mut grid, number);
            frame.draw_cells(&grid, Point::new(0.0, 0.0), &theme);
        });
        report("grid scroll", scrolling);
    }

    #[test]
    #[ignore = "a timing harness: make bench"]
    fn bench_dialog_of_wrapped_paragraphs() {
        let theme = Theme::dark();
        let mut dialog = long_dialog();
        let times = offscreen::time_frames(WIDTH, HEIGHT, FRAMES, |frame, _| {
            frame.clear(Rgba::from(theme.background));
            let bounds = frame.bounds();
            dialog.draw(frame, bounds, &theme);
        });
        report("dialog", times);
    }

    #[test]
    #[ignore = "a timing harness: make bench"]
    fn bench_rich_text_chapter() {
        bench_rich_text();
    }

    #[test]
    #[ignore = "a timing harness: make bench"]
    fn bench_tree_list_and_table_lent_their_rows() {
        bench_lent_view(LentView::List);
        bench_lent_view(LentView::Table);
    }

    #[test]
    #[ignore = "a timing harness: make bench"]
    fn bench_tree_form_rebuilt_on_every_frame_and_input() {
        bench_tree_form(SMALL_FORM);
        bench_tree_form(LARGE_FORM);
    }
}
