//! Several windows sharing one GPU and one set of fonts. `n` opens another
//! window, `w` or the close button closes the focused one, and the app
//! quits with its last window; Esc quits at once. Each window counts the
//! keys typed into it, so events visibly reach only the window they were
//! meant for.
//!
//! ```sh
//! cargo run -p misty --example windows
//! ```

use std::collections::HashMap;

use misty::{
    AppContext, Application, Error, Frame, Input, KeyCode, Point, Rect, Size, Span, StyledText,
    Theme, ThemeToken, WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The size every window opens at, in logical pixels.
const WINDOW_SIZE: Size = Size::new(480, 240);

/// The blank cells around the text, in columns and rows.
const MARGIN: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the open windows and what each has seen.
struct Windows {
    /// How many windows were ever opened, to number new ones.
    opened: usize,
    /// The colors the windows are drawn in.
    theme: Theme,
    /// Each open window's state.
    windows: HashMap<WindowId, WindowState>,
}

/// What one window shows.
struct WindowState {
    /// How many keys were typed into this window.
    keys: usize,
    /// The window's number, from 1.
    number: usize,
}

// ========================================================================
// Windows Implementations
// ========================================================================

impl Windows {
    /// The options of window number `number`.
    fn options(number: usize) -> WindowOptions {
        WindowOptions::new(format!("misty window {number}")).with_size(WINDOW_SIZE)
    }

    /// Redraws every window, whose count of open windows changed.
    fn redraw_all(&self, cx: &mut AppContext<'_, ()>) {
        for window in self.windows.keys() {
            cx.event_cx(*window).request_redraw();
        }
    }

    /// Forgets `window` and closes it.
    fn close(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.windows.remove(&window);
        cx.close_window(window);
        self.redraw_all(cx);
    }
}

impl Application for Windows {
    type Message = ();

    fn init(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.opened += 1;
        self.windows.insert(
            window,
            WindowState {
                keys: 0,
                number: self.opened,
            },
        );
        self.redraw_all(cx);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        if *input == Input::CloseRequested {
            self.close(cx, window);
            return;
        }
        let Some(state) = self.windows.get_mut(&window) else {
            return;
        };
        let Some(key) = input.key() else {
            return;
        };
        state.keys += 1;
        cx.event_cx(window).request_redraw();
        match key.code {
            KeyCode::Char('n') => {
                if let Err(error) = cx.open_window(Self::options(self.opened + 1)) {
                    eprintln!("could not open a window: {error}");
                }
            }
            KeyCode::Char('w') => self.close(cx, window),
            KeyCode::Esc => cx.exit(),
            _ => {}
        }
    }

    fn render(&mut self, window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.background.into());
        let Some(state) = self.windows.get(&window) else {
            return;
        };
        let metrics = frame.cell_metrics();
        let area = frame.bounds();
        let lines = [
            StyledText::new()
                .with(Span::new("Window ", ThemeToken::Muted.into()))
                .with(Span::new(state.number.to_string(), ThemeToken::Accent.into()).bold())
                .with(Span::new(
                    format!(" of {} open", self.windows.len()),
                    ThemeToken::Muted.into(),
                )),
            StyledText::plain(
                format!("{} keys typed here", state.keys),
                ThemeToken::Foreground.into(),
            ),
            StyledText::plain(
                "n opens a window, w closes this one, Esc quits.",
                ThemeToken::Muted.into(),
            )
            .with_ellipsis(),
        ];
        for (row, line) in lines.iter().enumerate() {
            let y = area.y + (MARGIN + row as f32) * metrics.height;
            let clip = Rect::new(area.x, y, area.width, metrics.height);
            frame.draw_text(
                line,
                Point::new(area.x + MARGIN * metrics.width, y),
                clip,
                &self.theme,
            );
        }
    }
}

fn main() -> Result<(), Error> {
    let app = Windows {
        opened: 0,
        theme: Theme::dark(),
        windows: HashMap::new(),
    };
    misty::run(app, Windows::options(1))
}
