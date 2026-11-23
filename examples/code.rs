//! Monospace viewers: this example's own source in a `CodeView` (syntax
//! colors with the `highlight` feature on), above a `HexView` of the same
//! bytes.
//!
//! Keys: Tab moves the keyboard between the two views, `w` turns soft wrap
//! on and off, `f` searches the source for `fn` and `n` and `N` step
//! through the matches, `t` switches between the dark and the light theme,
//! and Esc or the window's close button quits. In the views: Shift and the
//! arrows select, Ctrl+C copies (Ctrl+Shift+C copies the hex view's
//! selection as text), the wheel scrolls, and the pointer selects.
//!
//! ```sh
//! cargo run -p misty --example code --features highlight
//! ```

use std::time::Instant;

use misty::{
    AppContext, Application, CodeView, Error, Frame, GutterMark, HexView, Input, Key, KeyCode,
    Mods, Rect, Rgba, Theme, ThemeToken, Widget, WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty code";

/// What the views show: this file.
const SOURCE: &str = include_str!("code.rs");

/// The language of `SOURCE`, for the highlighter.
#[cfg(feature = "highlight")]
const LANGUAGE: &str = "rs";

/// What `f` searches for.
const QUERY: &str = "fn";

/// The share of the window's height the code view takes.
const CODE_SHARE: f32 = 0.65;

/// The gutter mark the example puts on its first line.
const FIRST_LINE_MARK: GutterMark = GutterMark {
    color: ThemeToken::Accent,
    glyph: '*',
};

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the two views, which of them has the keyboard, and the theme.
struct Demo {
    /// The source.
    code: CodeView,
    /// Whether the code view has the keyboard, rather than the hex view.
    code_focused: bool,
    /// The bytes of the source.
    hex: HexView,
    /// The colors everything is drawn in.
    theme: Theme,
    /// The one window, once open.
    window: Option<WindowId>,
    /// Whether soft wrap is on.
    wrap: bool,
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// The demo with the code view focused.
    fn new() -> Self {
        let mut code = CodeView::new(SOURCE);
        #[cfg(feature = "highlight")]
        code.set_highlights(misty::Highlighter::new().highlight(SOURCE, LANGUAGE));
        code.set_marks([(0, FIRST_LINE_MARK)]);
        code.set_focused(true);
        Self {
            code,
            code_focused: true,
            hex: HexView::new(),
            theme: Theme::dark(),
            window: None,
            wrap: false,
        }
    }

    /// Gives the keyboard to the code view (`true`) or the hex view.
    fn focus_code(&mut self, code: bool) {
        self.code_focused = code;
        self.code.set_focused(code);
        self.hex.lend(SOURCE.as_bytes()).set_focused(!code);
    }

    /// The areas of the code view and the hex view in `bounds`.
    fn areas(bounds: Rect) -> (Rect, Rect) {
        let split = (bounds.height * CODE_SHARE).round();
        (
            Rect::new(bounds.x, bounds.y, bounds.width, split),
            Rect::new(
                bounds.x,
                bounds.y + split,
                bounds.width,
                bounds.height - split,
            ),
        )
    }
}

impl Application for Demo {
    type Message = ();

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        let key = |code| Input::Key(Key::new(code, Mods::NONE));
        let shifted = |code| Input::Key(Key::new(code, Mods::SHIFT));
        match input {
            Input::CloseRequested => return cx.exit(),
            input if *input == key(KeyCode::Esc) => return cx.exit(),
            input if *input == key(KeyCode::Tab) => self.focus_code(!self.code_focused),
            input if *input == key(KeyCode::Char('w')) => {
                self.wrap = !self.wrap;
                self.code.set_wrap(self.wrap);
            }
            input if *input == key(KeyCode::Char('f')) => self.code.set_find(QUERY, true),
            input if *input == key(KeyCode::Char('n')) => self.code.next_match(),
            input if *input == shifted(KeyCode::Char('N')) => self.code.previous_match(),
            input if *input == key(KeyCode::Char('t')) => {
                self.theme = match self.theme == Theme::dark() {
                    true => Theme::light(),
                    false => Theme::dark(),
                };
            }
            input => {
                let mut event = cx.event_cx(window);
                let pointer = input.key().is_none();
                if self.code_focused || pointer {
                    self.code.handle(input, &mut event);
                }
                if !self.code_focused || pointer {
                    self.hex.lend(SOURCE.as_bytes()).handle(input, &mut event);
                }
            }
        }
        cx.event_cx(window).request_redraw();
    }

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        let mut window_cx = cx.event_cx(window);
        let code = self.code.about_to_wait(&mut window_cx);
        let hex = self
            .hex
            .lend(SOURCE.as_bytes())
            .about_to_wait(&mut window_cx);
        code.into_iter().chain(hex).min()
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(Rgba::from(self.theme.background));
        let (code, hex) = Self::areas(frame.bounds());
        self.code.draw(frame, code, &self.theme);
        self.hex
            .lend(SOURCE.as_bytes())
            .draw(frame, hex, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

fn main() -> Result<(), Error> {
    misty::run(Demo::new(), WindowOptions::new(TITLE))
}
