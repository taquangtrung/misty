//! Draws a grid of styled text, described as a tree app: the 16 ANSI
//! colors, bold and italic, every underline style, wide characters, and a
//! blinking cursor. A background thread counts seconds through the app's
//! waker. Tab cycles the cursor shape, `p` panics (the app carries on),
//! and Esc or the window's close button quits. Pass `--light` for the
//! built-in light theme.
//!
//! ```sh
//! cargo run -p misty --example grid -- --light
//! ```

use std::env;
use std::thread;
use std::time::Duration;

use misty::view::{cells, column};
use misty::{
    ANSI_COLORS, App, AppCx, Attributes, Cell, CellBuffer, CellColor, CellStyle, CellWidth, Cursor,
    CursorShape, Element, Key, KeyCode, Mods, Theme, UnderlineStyle, Update, Waker, WindowId,
    WindowOptions,
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

/// The grid's width in cells: room for its widest line.
const GRID_COLS: usize = 80;

/// The grid's height in cells: its lines and the margins around them.
const GRID_ROWS: usize = 15;

/// Where the cursor sits: row, then column, inside the margin.
const CURSOR_AT: (usize, usize) = (9, 7);

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
    /// The cells, filled again whenever what they show changes.
    buffer: CellBuffer,
    /// Which of [`CURSOR_SHAPES`] the cursor has.
    cursor_shape: usize,
    /// The last panic the app carried on from.
    last_panic: Option<String>,
    /// The colors the window is drawn in.
    theme: Theme,
    /// How many ticks the background thread has sent.
    ticks: u64,
}

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
    /// The demo at start, in `theme`, its grid filled.
    fn new(theme: Theme) -> Self {
        let mut demo = Self {
            buffer: CellBuffer::new(GRID_COLS, GRID_ROWS),
            cursor_shape: 0,
            last_panic: None,
            theme,
            ticks: 0,
        };
        demo.fill();
        demo
    }

    /// Fills the grid again from the demo's state.
    fn fill(&mut self) {
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
        let cursor = Cursor::new(MARGIN + row, MARGIN + col, CURSOR_SHAPES[self.cursor_shape]);
        self.buffer.set_cursor(Some(cursor));
    }

    /// Counts a tick from the background thread.
    fn tick(&mut self) {
        self.ticks += 1;
        self.fill();
    }
}

impl App for GridDemo {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        column([cells(&self.buffer).into()])
    }

    /// Starts the background thread counting seconds.
    fn start(&mut self, waker: Waker<Update<Self>>) {
        thread::spawn(move || {
            loop {
                thread::sleep(TICK);
                if waker.send(Box::new(Self::tick)).is_err() {
                    return;
                }
            }
        });
    }

    /// Tab cycles the cursor shape, the blink starting over; `p` panics;
    /// Esc quits.
    fn key(&mut self, key: Key, cx: &mut AppCx<'_>) -> bool {
        match key {
            key if key == Key::new(KeyCode::Esc, Mods::NONE) => cx.exit(),
            key if key == Key::new(KeyCode::Tab, Mods::NONE) => {
                self.cursor_shape = (self.cursor_shape + 1) % CURSOR_SHAPES.len();
                self.fill();
            }
            key if key == Key::new(KeyCode::Char(PANIC_KEY), Mods::NONE) => {
                panic!("the {PANIC_KEY} key was pressed")
            }
            _ => return false,
        }
        true
    }

    /// Shows the panic the app carried on from.
    fn on_panic(&mut self, why: &str) {
        self.last_panic = Some(why.to_owned());
        self.fill();
    }

    fn theme(&self) -> &Theme {
        &self.theme
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

fn main() -> Result<(), misty::Error> {
    let theme = match env::args().any(|arg| arg == LIGHT_FLAG) {
        true => Theme::light(),
        false => Theme::dark(),
    };
    misty::run_app(GridDemo::new(theme), WindowOptions::new(TITLE))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, feature = "testing"))]
mod tests {
    use misty::CellSource;
    use misty::testing::TreeWindow;

    use super::*;

    /// The window's width in the tests, in physical pixels.
    const WIDTH: u32 = 800;

    /// The window's height in the tests.
    const HEIGHT: u32 = 600;

    /// What the hand-driven version of this example (an `Application`
    /// drawing a window-sized `CellBuffer` by hand) described to screen
    /// readers at start, captured from it before it was rewritten.
    const HAND_DRIVEN: &str = "Window \"misty grid\" focused\n";

    /// The text the hand-driven version drew at start, in the pieces its
    /// cell grid drew it in.
    const HAND_DRIVEN_DRAWN: [&str; 58] = [
        "misty",
        "draws",
        "a",
        "CellSource",
        "as",
        "a",
        "grid",
        "0",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        "10",
        "11",
        "12",
        "13",
        "14",
        "15",
        "bold",
        "italic",
        "bold",
        "italic",
        "dim",
        "reverse",
        "struck",
        "single",
        "double",
        "curly",
        "dotted",
        "dashed",
        "true",
        "color,",
        "cube",
        "wide:",
        "世",
        "界",
        "box:",
        "┌",
        "─",
        "┐",
        "accents:",
        "é",
        "à",
        "ü",
        "Tab",
        "changes",
        "the",
        "cursor:",
        "cursor>",
        "<",
        "background",
        "ticks:",
        "0",
    ];

    /// The window showing the demo at start, in the dark theme.
    fn window() -> TreeWindow<GridDemo> {
        TreeWindow::new(GridDemo::new(Theme::dark()), TITLE, WIDTH, HEIGHT)
    }

    #[test]
    fn test_the_tree_describes_and_draws_what_the_hand_driven_example_did() {
        let window = window();
        assert_eq!(window.ui_text(), HAND_DRIVEN);
        assert_eq!(window.drawn_text(), HAND_DRIVEN_DRAWN);
    }

    #[test]
    fn test_ticks_and_a_panic_carried_on_from_show_as_in_the_hand_driven_example() {
        let mut window = window();
        let waker = window.waker();
        for _ in 0..3 {
            waker
                .send(Box::new(GridDemo::tick))
                .expect("the window is open");
        }
        assert_eq!(window.deliver(), 3);
        window.update(|demo| demo.on_panic("boom"));
        let drawn = window.drawn_text();
        // The hand-driven example's last pieces with 3 ticks and a panic.
        let last = [
            "background",
            "ticks:",
            "3",
            "carried",
            "on",
            "from:",
            "boom",
        ];
        assert_eq!(drawn[drawn.len() - last.len()..], last);
    }

    #[test]
    fn test_tab_reaches_the_app_and_cycles_the_cursor_shape() {
        let mut window = window();
        window.press("<tab>");
        assert_eq!(window.app().cursor_shape, 1);
        let cursor = window.app().buffer.cursor().map(|cursor| cursor.shape);
        assert_eq!(cursor, Some(CursorShape::Bar));
        window.press("<tab> <tab>");
        assert_eq!(window.app().cursor_shape, 0);
    }

    #[test]
    fn test_the_blinking_cursor_asks_for_its_next_frame() {
        let mut window = window();
        assert!(
            window.about_to_wait().is_some(),
            "the blink keeps a frame coming"
        );
    }

    #[test]
    fn test_esc_quits() {
        let mut window = window();
        window.press("<esc>");
        assert!(window.is_closed());
    }
}
