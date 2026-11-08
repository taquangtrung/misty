//! Opens a window titled "misty" that clears to a dark background. Esc or
//! the window's close button quits. Pass `--frameless` for a window without
//! the system title bar and border.
//!
//! ```sh
//! cargo run -p misty --example hello -- --frameless
//! ```

use std::env;

use misty::{
    AppContext, Application, Decorations, Error, Frame, Input, Key, KeyCode, Mods, Rgba, WindowId,
    WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window's background, a dark theme's base color.
const BACKGROUND: Rgba = Rgba::rgb(0x1e, 0x1e, 0x2e);

/// The argument that opens the window without system decorations.
const FRAMELESS_FLAG: &str = "--frameless";

/// The window title.
const TITLE: &str = "misty";

// ========================================================================
// Data Structures
// ========================================================================

/// An app that only clears its window.
struct Hello;

// ========================================================================
// Hello Implementations
// ========================================================================

impl Application for Hello {
    type Message = ();

    fn input(&mut self, cx: &mut AppContext<'_, ()>, _window: WindowId, input: &Input) {
        let escape = Input::Key(Key::new(KeyCode::Esc, Mods::NONE));
        if *input == escape || *input == Input::CloseRequested {
            cx.exit();
        }
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(BACKGROUND);
    }
}

// ========================================================================
// Functions
// ========================================================================

fn main() -> Result<(), Error> {
    let decorations = if env::args().any(|arg| arg == FRAMELESS_FLAG) {
        Decorations::Frameless
    } else {
        Decorations::System
    };
    misty::run(
        Hello,
        WindowOptions::new(TITLE).with_decorations(decorations),
    )
}
