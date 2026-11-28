//! Split panes: `v` splits the focused pane side by side, `s` splits it one
//! above the other, `x` closes it (the last pane stays), and Alt+H, Alt+J,
//! Alt+K, Alt+L move focus left, down, up, right (never wrapping). Drag a
//! divider to resize; click a pane to focus it. Esc quits.
//!
//! ```sh
//! cargo run -p misty --example panes
//! ```

use misty::grid::{Cell, CellBuffer, CellStyle};
use misty::widgets::{SplitView, SplitViewResponse};
use misty::{
    AppContext, Application, Axis, Direction, Error, Frame, Input, Key, KeyCode, Mods, Point,
    SplitTree, Theme, Widget, WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty panes";

/// The blank cells around each pane's text, in columns and rows.
const MARGIN: usize = 1;

/// The help shown in every pane, under its name.
const HELP: [&str; 3] = [
    "v: split side by side   s: split below   x: close",
    "Alt+H/J/K/L: move focus   drag a divider to resize",
    "Esc quits.",
];

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a split tree and the view that draws it.
struct Panes {
    /// Each pane's text, refilled for every pane drawn.
    content: CellBuffer,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The panes.
    tree: SplitTree,
    /// The pointer handling and drawing of the panes.
    view: SplitView,
}

// ========================================================================
// Panes Implementations
// ========================================================================

impl Panes {
    /// Carries out a press of `key`; whether it changed anything.
    fn press_key(&mut self, cx: &mut AppContext<'_, ()>, key: Key) -> bool {
        let KeyCode::Char(c) = key.code else {
            if key.code == KeyCode::Esc {
                cx.exit();
            }
            return false;
        };
        let focused = self.tree.focused();
        if key.mods == Mods::ALT {
            let direction = match c {
                'h' => Direction::Left,
                'j' => Direction::Down,
                'k' => Direction::Up,
                'l' => Direction::Right,
                _ => return false,
            };
            return self.view.focus_toward(&mut self.tree, direction).is_some();
        }
        match c {
            'v' => self.tree.split(focused, Axis::Horizontal).is_some(),
            's' => self.tree.split(focused, Axis::Vertical).is_some(),
            'x' => self.tree.remove(focused),
            _ => false,
        }
    }
}

impl Application for Panes {
    type Message = ();

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        match self
            .view
            .lend(&mut self.tree)
            .handle(input, &mut cx.event_cx(window))
        {
            SplitViewResponse::Focused(_) | SplitViewResponse::Handled => return,
            SplitViewResponse::Ignored => {}
            _ => return,
        }
        match input {
            Input::CloseRequested => cx.exit(),
            Input::Key(key) if self.press_key(cx, *key) => cx.event_cx(window).request_redraw(),
            _ => {}
        }
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.background.into());
        let bounds = frame.bounds();
        let panes = self
            .view
            .lend(&mut self.tree)
            .draw(frame, bounds, &self.theme);
        let metrics = frame.cell_metrics();
        for pane in panes {
            let rect = pane.rect;
            self.content
                .resize(metrics.cols_in(rect.width), metrics.rows_in(rect.height));
            self.content.clear();
            let name = format!("{:?} ({} x {} px)", pane.id, rect.width, rect.height);
            for (row, line) in [name.as_str()].into_iter().chain(HELP).enumerate() {
                for (col, ch) in line.chars().enumerate() {
                    self.content.set(
                        MARGIN + row,
                        MARGIN + col,
                        Cell::new(ch, CellStyle::default()),
                    );
                }
            }
            frame.draw_cells(&self.content, Point::new(rect.x, rect.y), &self.theme);
        }
        self.view.draw_focus_outline(frame, &self.tree, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

fn main() -> Result<(), Error> {
    let app = Panes {
        content: CellBuffer::new(0, 0),
        theme: Theme::default(),
        tree: SplitTree::new(),
        view: SplitView::new(),
    };
    misty::run(app, WindowOptions::new(TITLE))
}
