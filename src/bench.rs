//! Frame timings for the views that draw the most text: a list of 100,000
//! rows scrolled a row a frame, a window-filling cell grid, both still
//! and scrolling, and a dialog of wrapped paragraphs. Ignored by `cargo test`; `make bench` runs them in a
//! release build and prints the average and slowest frame.

use std::borrow::Cow;

use super::app::offscreen::{self, FrameTimes};
use super::app::widget::Widget;
use super::base::cell::{Cell, CellBuffer, CellSource, CellStyle};
use super::base::color::Rgba;
use super::base::geometry::{Point, Rect};
use super::base::styled::{Span, TextColor};
use super::base::theme::{Theme, ThemeToken};
use super::render::paragraph::Paragraph;
use super::widgets::dialog::{Dialog, DialogButton};
use super::widgets::list_view::{List, ListSource};

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

// ========================================================================
// Data Structures
// ========================================================================

/// Generated rows, as the list example shows.
struct Rows;

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

// ========================================================================
// Functions
// ========================================================================

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
}
