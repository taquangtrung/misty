//! Draws a grid of styled text: the 16 ANSI colors, bold and italic,
//! every underline style, wide characters, and a blinking cursor. A
//! background thread counts seconds through a [`misty::Waker`]. Tab cycles
//! the cursor shape, `p` panics (the app carries on), and Esc or the
//! window's close button quits. Pass `--light` for the built-in light theme.
//!
//! ```sh
//! cargo run -p misty --example grid -- --light
//! ```

use std::env;
use std::thread;
use std::time::{Duration, Instant};

use misty::{
    ANSI_COLORS, AppContext, Application, Attributes, Cell, CellBuffer, CellColor, CellStyle,
    CellWidth, Cursor, CursorShape, Error, Frame, Input, KeyCode, Point, Theme, UnderlineStyle,
    WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty grid";

/// The argument that picks the light theme.
const LIGHT_FLAG: &str = "--light";

/// The blank cells around the text, in columns and rows.
const MARGIN: usize = 1;

/// Where the cursor sits: row, then column, inside the margin.
const CURSOR_AT: (usize, usize) = (9, 7);

/// How long the cursor stays on, then off.
const BLINK: Duration = Duration::from_millis(530);

/// How often the background thread sends a tick.
const TICK: Duration = Duration::from_secs(1);

/// The key that makes the app panic, to show it carries on.
const PANIC_KEY: char = 'p';

/// The cursor shapes Tab cycles through, in order.
const CURSOR_SHAPES: [CursorShape; 3] =
    [CursorShape::Block, CursorShape::Bar, CursorShape::Underline];

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the grid it draws and the state shown in it.
struct GridDemo {
    /// When the cursor last turned on.
    blink_start: Instant,
    /// The cells, rebuilt for every frame.
    buffer: CellBuffer,
    /// Which of [`CURSOR_SHAPES`] the cursor has.
    cursor_shape: usize,
    /// The last panic the app carried on from.
    last_panic: Option<String>,
    /// The colors the window is drawn in.
    theme: Theme,
    /// How many ticks the background thread has sent.
    ticks: u64,
    /// The window, once open.
    window: Option<WindowId>,
}

/// What the background thread sends.
struct Tick;

/// Writes text into a [`CellBuffer`] left to right.
struct Writer<'a> {
    /// Where the text goes.
    buffer: &'a mut CellBuffer,
    /// The next column written.
    col: usize,
    /// The row written.
    row: usize,
}

// ========================================================================
// GridDemo Implementations
// ========================================================================

impl GridDemo {
    /// Rebuilds the grid at `cols` by `rows` cells.
    fn fill(&mut self, cols: usize, rows: usize) {
        self.buffer.resize(cols, rows);
        self.buffer.clear();
        let bold = style(Attributes::BOLD, UnderlineStyle::None);
        let italic = style(Attributes::ITALIC, UnderlineStyle::None);
        let buffer = &mut self.buffer;
        Writer::at(buffer, 0).text("misty draws a CellSource as a grid", bold);
        let mut colors = Writer::at(buffer, 2);
        for index in 0..ANSI_COLORS {
            let background = CellStyle {
                background: CellColor::Indexed(index as u8),
                foreground: CellColor::Indexed(0),
                ..CellStyle::default()
            };
            colors.text(&format!("{index:>3}"), background);
        }
        Writer::at(buffer, 3)
            .text("bold", bold)
            .text(" ", CellStyle::default())
            .text("italic", italic)
            .text(" ", CellStyle::default())
            .text(
                "bold italic",
                style(Attributes::BOLD | Attributes::ITALIC, UnderlineStyle::None),
            )
            .text(" ", CellStyle::default())
            .text("dim", style(Attributes::DIM, UnderlineStyle::None))
            .text(" ", CellStyle::default())
            .text("reverse", style(Attributes::REVERSE, UnderlineStyle::None))
            .text(" ", CellStyle::default())
            .text(
                "struck",
                style(Attributes::STRIKETHROUGH, UnderlineStyle::None),
            );
        let mut underlines = Writer::at(buffer, 4);
        for (name, underline) in [
            ("single", UnderlineStyle::Single),
            ("double", UnderlineStyle::Double),
            ("curly", UnderlineStyle::Curly),
            ("dotted", UnderlineStyle::Dotted),
            ("dashed", UnderlineStyle::Dashed),
        ] {
            underlines
                .text(name, style(Attributes::NONE, underline))
                .text(" ", CellStyle::default());
        }
        let truecolor = CellStyle {
            foreground: CellColor::Rgb(misty::Rgb::new(0xfa, 0xb3, 0x87)),
            ..CellStyle::default()
        };
        let mut cube = Writer::at(buffer, 5);
        cube.text("true color, cube ", truecolor);
        for index in (16..232).step_by(12) {
            let swatch = CellStyle {
                background: CellColor::Indexed(index),
                ..CellStyle::default()
            };
            cube.text(" ", swatch);
        }
        Writer::at(buffer, 6)
            .text("wide: ", CellStyle::default())
            .wide('世', CellStyle::default())
            .wide('界', CellStyle::default())
            .text(" box: ┌─┐ ", CellStyle::default())
            .text("accents: éàü", italic);
        Writer::at(buffer, 8).text("Tab changes the cursor:", CellStyle::default());
        Writer::at(buffer, 9).text("cursor>  <", CellStyle::default());
        let ticks = format!("background ticks: {}", self.ticks);
        Writer::at(buffer, 11).text(&ticks, CellStyle::default());
        if let Some(why) = &self.last_panic {
            let muted = CellStyle {
                foreground: CellColor::Rgb(self.theme.muted),
                ..CellStyle::default()
            };
            Writer::at(buffer, 12).text(&format!("carried on from: {why}"), muted);
        }
        let (row, col) = CURSOR_AT;
        let flips = self.blink_start.elapsed().as_millis() / BLINK.as_millis();
        let blink_on = flips.is_multiple_of(2);
        let cursor = Cursor::new(MARGIN + row, MARGIN + col, CURSOR_SHAPES[self.cursor_shape]);
        self.buffer.set_cursor(blink_on.then_some(cursor));
    }

    /// When the cursor next turns on or off.
    fn next_blink(&self) -> Instant {
        let flips = self.blink_start.elapsed().as_millis() / BLINK.as_millis();
        let flips = u32::try_from(flips + 1).unwrap_or(u32::MAX);
        self.blink_start + BLINK * flips
    }
}

impl Application for GridDemo {
    type Message = Tick;

    fn init(&mut self, cx: &mut AppContext<'_, Tick>, window: WindowId) {
        self.window = Some(window);
        let waker = cx.waker();
        thread::spawn(move || {
            loop {
                thread::sleep(TICK);
                if waker.send(Tick).is_err() {
                    return;
                }
            }
        });
    }

    fn input(&mut self, cx: &mut AppContext<'_, Tick>, window: WindowId, input: &Input) {
        let Input::Key(key) = input else {
            if *input == Input::CloseRequested {
                cx.exit();
            }
            return;
        };
        match key.code {
            KeyCode::Esc => cx.exit(),
            KeyCode::Tab => {
                self.cursor_shape = (self.cursor_shape + 1) % CURSOR_SHAPES.len();
                self.blink_start = Instant::now();
                cx.event_cx(window).request_redraw();
            }
            KeyCode::Char(PANIC_KEY) => panic!("the {PANIC_KEY} key was pressed"),
            _ => {}
        }
    }

    fn message(&mut self, cx: &mut AppContext<'_, Tick>, Tick: Tick) {
        self.ticks += 1;
        if let Some(window) = self.window {
            cx.event_cx(window).request_redraw();
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, Tick>) -> Option<Instant> {
        if let Some(window) = self.window {
            cx.event_cx(window).request_redraw_at(self.next_blink());
        }
        None
    }

    fn on_panic(&mut self, cx: &mut AppContext<'_, Tick>, why: &str) {
        self.last_panic = Some(why.to_owned());
        if let Some(window) = self.window {
            cx.event_cx(window).request_redraw();
        }
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        let metrics = frame.cell_metrics();
        let bounds = frame.bounds();
        self.fill(
            metrics.cols_in(bounds.width),
            metrics.rows_in(bounds.height),
        );
        frame.clear(self.theme.background.into());
        frame.draw_cells(&self.buffer, Point::new(0.0, 0.0), &self.theme);
    }
}

// ========================================================================
// Writer Implementations
// ========================================================================

impl<'a> Writer<'a> {
    /// A writer at the left margin of line `line` below the top margin.
    fn at(buffer: &'a mut CellBuffer, line: usize) -> Self {
        Self {
            buffer,
            col: MARGIN,
            row: MARGIN + line,
        }
    }

    /// Writes `text`, one cell per character, in `style`.
    fn text(&mut self, text: &str, style: CellStyle) -> &mut Self {
        for ch in text.chars() {
            self.buffer.set(self.row, self.col, Cell::new(ch, style));
            self.col += 1;
        }
        self
    }

    /// Writes the double-width `ch` in `style` and its continuation.
    fn wide(&mut self, ch: char, style: CellStyle) -> &mut Self {
        let wide = Cell {
            width: CellWidth::Wide,
            ..Cell::new(ch, style)
        };
        let continuation = Cell {
            width: CellWidth::Continuation,
            ..Cell::new(' ', style)
        };
        self.buffer.set(self.row, self.col, wide);
        self.buffer.set(self.row, self.col + 1, continuation);
        self.col += 2;
        self
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The default colors with `attributes` and `underline`.
fn style(attributes: Attributes, underline: UnderlineStyle) -> CellStyle {
    CellStyle {
        attributes,
        underline,
        ..CellStyle::default()
    }
}

fn main() -> Result<(), Error> {
    let demo = GridDemo {
        blink_start: Instant::now(),
        buffer: CellBuffer::new(0, 0),
        cursor_shape: 0,
        last_panic: None,
        ticks: 0,
        window: None,
        theme: match env::args().any(|arg| arg == LIGHT_FLAG) {
            true => Theme::light(),
            false => Theme::dark(),
        },
    };
    misty::run(demo, WindowOptions::new(TITLE))
}
