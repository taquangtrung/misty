//! Zoomable views: an image drawn in code in an `ImageView`, a graph of
//! nodes and edges on a pannable `Viewport` canvas drawn with paths, and a
//! spreadsheet-like grid whose header row and first column stay put while
//! the rest scrolls (`GridScroll`).
//!
//! Keys: `1`, `2`, and `3` switch views; in them the arrows and `h` `j`
//! `k` `l` pan, `+` and `-` zoom, `0` fits, `w` fits the width, and in the
//! image view `r` turns, Shift+R turns back, and `m` mirrors. The wheel
//! scrolls or pans, Ctrl+wheel zooms around the pointer, and a drag pans.
//! `t` switches between the dark and the light theme; Esc or the window's
//! close button quits.
//!
//! ```sh
//! cargo run -p misty --example viewer
//! ```

use std::time::Instant;

use misty::{AppContext, Application, Error, Frame, Image, Input, Key, KeyCode, Mods, Point, Rect, Rgba, Stroke, StyledText, TextColor, Theme, VectorPath, Widget, WindowId, WindowOptions};
use misty::widgets::{GridScroll, ImageView, Viewport};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty viewer";

/// The side of the image drawn in code, in pixels.
const IMAGE_SIDE: u32 = 256;

/// The side of the image's checks, in pixels.
const IMAGE_CHECK: u32 = 32;

/// The nodes of the graph, in canvas units, with an edge from each to the
/// next.
const NODES: [(f32, f32); 6] = [
    (0.0, 0.0),
    (300.0, 120.0),
    (120.0, 360.0),
    (520.0, 420.0),
    (700.0, 80.0),
    (900.0, 300.0),
];

/// A node's radius, in canvas units.
const NODE_RADIUS: f32 = 28.0;

/// The width of an edge, in canvas units.
const EDGE_WIDTH: f32 = 3.0;

/// The size of the canvas the graph is fitted in, in its units.
const CANVAS: (f32, f32) = (1000.0, 500.0);

/// How many columns the grid has.
const GRID_COLS: usize = 40;

/// How many rows the grid has.
const GRID_ROWS: usize = 2_000;

/// The width of a grid column, in cells.
const COLUMN_CELLS: f32 = 10.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Which view shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pane {
    /// The graph canvas.
    Canvas,
    /// The frozen-pane grid.
    Grid,
    /// The image.
    Image,
}

/// The demo's state.
struct Demo {
    /// The graph's canvas.
    canvas: Viewport,
    /// The grid's scroll.
    grid: GridScroll,
    /// The image view.
    image: ImageView,
    /// Which view shows.
    pane: Pane,
    /// The colors everything is drawn in.
    theme: Theme,
    /// The one window, once open.
    window: Option<WindowId>,
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// The demo showing the image.
    fn new() -> Self {
        let mut image = ImageView::new(checks(), "Colored checks drawn in code");
        image.set_focused(true);
        let mut canvas = Viewport::new(CANVAS.0, CANVAS.1).with_free_pan();
        canvas.set_focused(true);
        canvas.fit();
        let mut grid = GridScroll::new(1, 1);
        grid.lend(&[], &[]).set_focused(true);
        Self {
            canvas,
            grid,
            image,
            pane: Pane::Image,
            theme: Theme::dark(),
            window: None,
        }
    }

    /// Draws the graph on the canvas over `area`.
    fn draw_canvas(&mut self, frame: &mut Frame, area: Rect) {
        let theme = &self.theme;
        self.canvas.draw(frame, area, theme);
        let canvas = self.canvas;
        frame.clipped(area, |frame| {
            frame.with_transform(canvas.transform(), |frame| {
                let points: Vec<Point> = NODES
                    .iter()
                    .map(|&(x, y)| Point::new(x + NODE_RADIUS, y + NODE_RADIUS))
                    .collect();
                frame.stroke_path(
                    &VectorPath::polyline(&points),
                    &Stroke::new(EDGE_WIDTH),
                    theme.border,
                );
                for &point in &points {
                    frame.fill_path(&VectorPath::circle(point, NODE_RADIUS), theme.accent);
                }
            });
        });
    }

    /// Draws the grid over `area`: a header row, a first column of row
    /// numbers, and cells naming themselves.
    fn draw_grid(&mut self, frame: &mut Frame, area: Rect) {
        let theme = self.theme.clone();
        let metrics = frame.cell_metrics();
        let widths = vec![(COLUMN_CELLS * metrics.width).round(); GRID_COLS];
        let heights = vec![metrics.height.ceil(); GRID_ROWS];
        let layout = self.grid.lend(&widths, &heights).draw(frame, area, &theme);
        for row in &layout.rows {
            for col in &layout.columns {
                let cell = Rect::new(col.start, row.start, col.size, row.size);
                let text = match (row.index, col.index) {
                    (0, 0) => String::new(),
                    (0, col) => format!("col {col}"),
                    (row, 0) => format!("{row}"),
                    (row, col) => format!("r{row}c{col}"),
                };
                frame.clipped(layout.clip(row.index, col.index), |frame| {
                    if row.index == 0 || col.index == 0 {
                        frame.draw_box(
                            cell,
                            &theme.shape.highlight(theme.surface, frame.scale_factor()),
                        );
                    }
                    frame.draw_text(
                        &StyledText::plain(text, TextColor::Rgba(Rgba::from(theme.foreground))),
                        Point::new(cell.x, cell.y),
                        cell,
                        &theme,
                    );
                });
            }
        }
    }
}

impl Application for Demo {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        let key = |code| Input::Key(Key::new(code, Mods::NONE));
        match input {
            Input::CloseRequested => return cx.exit(),
            input if *input == key(KeyCode::Esc) => return cx.exit(),
            input if *input == key(KeyCode::Char('1')) => self.pane = Pane::Image,
            input if *input == key(KeyCode::Char('2')) => self.pane = Pane::Canvas,
            input if *input == key(KeyCode::Char('3')) => self.pane = Pane::Grid,
            input if *input == key(KeyCode::Char('t')) => {
                self.theme = match self.theme == Theme::dark() {
                    true => Theme::light(),
                    false => Theme::dark(),
                };
            }
            input => {
                let mut event = cx.event_cx(window);
                match self.pane {
                    Pane::Canvas => {
                        self.canvas.handle(input, &mut event);
                    }
                    Pane::Grid => {
                        self.grid.lend(&[], &[]).handle(input, &mut event);
                    }
                    Pane::Image => {
                        self.image.handle(input, &mut event);
                    }
                }
            }
        }
        cx.event_cx(window).request_redraw();
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        self.grid
            .lend(&[], &[])
            .about_to_wait(&mut cx.event_cx(window))
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(Rgba::from(self.theme.background));
        let area = frame.bounds();
        match self.pane {
            Pane::Canvas => self.draw_canvas(frame, area),
            Pane::Grid => self.draw_grid(frame, area),
            Pane::Image => {
                let theme = self.theme.clone();
                self.image.draw(frame, area, &theme);
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// An image of colored checks, transparent in every other one so the
/// checkerboard behind shows.
fn checks() -> Image {
    let side = IMAGE_SIDE;
    let pixels = (0..side * side)
        .flat_map(|index| {
            let (x, y) = (index % side, index / side);
            let (col, row) = (x / IMAGE_CHECK, y / IMAGE_CHECK);
            let alpha = match (col + row) % 2 {
                0 => u8::MAX,
                _ => 0,
            };
            [(x * 255 / side) as u8, (y * 255 / side) as u8, 160, alpha]
        })
        .collect();
    Image::from_rgba(side, side, pixels).expect("as many pixels as the size says")
}

fn main() -> Result<(), Error> {
    misty::run(Demo::new(), WindowOptions::new(TITLE))
}
