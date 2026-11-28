//! Tooltips and toasts. Rest the pointer on a button for its tooltip; click
//! it (or press `i`, `w`, `e`) for an info, warning, or error toast. Toasts
//! stack in the bottom right corner, newest at the bottom, three at most,
//! and go away on their own. Nothing polls: the event loop sleeps until the
//! next tooltip or toast deadline. Esc quits.
//!
//! ```sh
//! cargo run -p misty --example notify
//! ```

use std::time::Instant;

use misty::grid::{Cell, CellBuffer, CellStyle};
use misty::widgets::{Toasts, Tooltip};
use misty::{
    AppContext, Application, Error, Frame, Input, KeyCode, Notice, Notifier, Point, PointerButton,
    Rect, Routing, Severity, Theme, Widget, WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty notify";

/// The buttons: label, the toast each pushes, and its tooltip.
const BUTTONS: [(&str, Severity, &str); 3] = [
    (
        "[ Info ]",
        Severity::Info,
        "Push an info toast (i); it stays 4 s",
    ),
    (
        "[ Warning ]",
        Severity::Warning,
        "Push a warning toast (w); it stays 6 s",
    ),
    (
        "[ Error ]",
        Severity::Error,
        "Push an error toast (e); it stays 10 s",
    ),
];

/// The row the buttons sit on, and the column the first starts at.
const BUTTON_ROW: usize = 1;

/// The cells before the first button and between buttons.
const BUTTON_GAP: usize = 2;

/// The help line, under the buttons.
const HELP: &str = "Rest on a button for its tooltip; click it or press i, w, e. Esc quits.";

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a few buttons with tooltips that push toasts.
struct Notify {
    /// Where each button was last drawn.
    buttons: Vec<Rect>,
    /// The text, rebuilt every frame.
    content: CellBuffer,
    /// Where the pointer is.
    pointer: Point,
    /// How many toasts have been pushed, to number them.
    pushed: usize,
    /// The colors the window is drawn in.
    theme: Theme,
    /// What is shown, and for how long.
    notifier: Notifier,
    /// The toasts' cards.
    toasts: Toasts,
    /// The tooltip.
    tooltip: Tooltip,
    /// The window, once open.
    window: Option<WindowId>,
}

// ========================================================================
// Notify Implementations
// ========================================================================

impl Notify {
    /// Pushes a numbered toast at `severity`.
    fn push(&mut self, severity: Severity) {
        self.pushed += 1;
        let message = format!("{severity:?} number {}", self.pushed);
        let notice = match severity {
            Severity::Error => Notice::error(message),
            Severity::Info => Notice::info(message),
            Severity::Refusal => Notice::refusal(message),
            Severity::Warning => Notice::warning(message),
        };
        self.notifier.notify(notice);
    }

    /// The button under the pointer, by index.
    fn button_under_pointer(&self) -> Option<usize> {
        self.buttons
            .iter()
            .position(|rect| rect.contains(self.pointer))
    }
}

impl Application for Notify {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        match input {
            Input::CloseRequested => cx.exit(),
            Input::PointerMoved(position) => {
                self.pointer = *position;
                match self.button_under_pointer() {
                    Some(index) => self.tooltip.hover(self.buttons[index], BUTTONS[index].2),
                    None => {
                        if self.tooltip.leave() {
                            cx.event_cx(window).request_redraw();
                        }
                    }
                }
            }
            Input::PointerButton(button)
                if button.pressed && button.button == PointerButton::Left =>
            {
                if let Some(index) = self.button_under_pointer() {
                    self.push(BUTTONS[index].1);
                    cx.event_cx(window).request_redraw();
                }
            }
            Input::Key(key) => {
                let severity = match key.code {
                    KeyCode::Esc => {
                        cx.exit();
                        return;
                    }
                    KeyCode::Char('i') => Severity::Info,
                    KeyCode::Char('w') => Severity::Warning,
                    KeyCode::Char('e') => Severity::Error,
                    _ => return,
                };
                self.push(severity);
                cx.event_cx(window).request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        let mut window_cx = cx.event_cx(window);
        let tooltip = self.tooltip.about_to_wait(&mut window_cx);
        let toasts = self
            .toasts
            .lend(&mut self.notifier)
            .about_to_wait(&mut window_cx);
        tooltip.into_iter().chain(toasts).min()
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.background.into());
        let bounds = frame.bounds();
        let metrics = frame.cell_metrics();
        self.content.resize(
            metrics.cols_in(bounds.width),
            metrics.rows_in(bounds.height),
        );
        self.content.clear();
        let style = CellStyle::default();
        self.buttons.clear();
        let mut col = BUTTON_GAP;
        for (label, _, _) in BUTTONS {
            for (offset, ch) in label.chars().enumerate() {
                self.content
                    .set(BUTTON_ROW, col + offset, Cell::new(ch, style));
            }
            let width = label.chars().count();
            self.buttons.push(Rect::new(
                bounds.x + col as f32 * metrics.width,
                bounds.y + BUTTON_ROW as f32 * metrics.height,
                width as f32 * metrics.width,
                metrics.height,
            ));
            col += width + BUTTON_GAP;
        }
        for (offset, ch) in HELP.chars().enumerate() {
            self.content
                .set(BUTTON_ROW + 2, BUTTON_GAP + offset, Cell::new(ch, style));
        }
        frame.draw_cells(&self.content, Point::new(bounds.x, bounds.y), &self.theme);
        let bounds = frame.bounds();
        self.tooltip.draw(frame, bounds, &self.theme);
        self.toasts
            .lend(&mut self.notifier)
            .draw(frame, bounds, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

fn main() -> Result<(), Error> {
    let app = Notify {
        buttons: Vec::new(),
        content: CellBuffer::new(0, 0),
        pointer: Point::default(),
        pushed: 0,
        theme: Theme::default(),
        notifier: {
            // No status bar here: every notice is a card.
            let mut notifier = Notifier::new();
            notifier.set_routing(Routing {
                cards_from: None,
                status_bar: false,
            });
            notifier
        },
        toasts: Toasts::new(),
        tooltip: Tooltip::new(),
        window: None,
    };
    misty::run(app, WindowOptions::new(TITLE))
}
