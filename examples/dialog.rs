//! A dialog laid out by misty's layout engine: `q`, the close button, or
//! the window's Quit button asks whether to quit. Tab, Shift+Tab, Left,
//! and Right move between the dialog's buttons, Enter chooses, Esc
//! dismisses; `d` asks again. The body is rich text in the proportional
//! font, wrapped to the dialog, with a link that a click follows.
//!
//! The window's own two buttons share a focus tree with the dialog: Tab
//! moves between them with a focus ring, Enter or Space presses the
//! focused one, and a click focuses one. The dialog is a focus trap:
//! while it is open no key reaches the window behind it, and when it
//! closes the focus goes back to the button that had it.
//!
//! Screen readers see the status line, the window's buttons, and the open
//! dialog with its paragraphs and buttons; they can focus or click any
//! button.
//!
//! ```sh
//! cargo run -p misty --example dialog
//! ```

use misty::accesskit::{Action, Node, Role};
use misty::{
    AccessAction, AppContext, Application, Dialog, DialogButton, DialogResponse, Error, FocusId,
    FocusTree, Frame, Input, Key, KeyCode, Mods, Outcome, Paragraph, Point, Rect, Route, Span,
    StyledText, TextAlign, Theme, ThemeToken, Widget, WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty dialog";

/// The blank cells around the text, in columns and rows.
const MARGIN: f32 = 1.0;

/// The blank cells on each side of a button's label.
const BUTTON_PADDING: f32 = 1.0;

/// The blank cells between the buttons.
const BUTTON_GAP: f32 = 2.0;

/// The accessibility id of the status line, which never takes the focus.
const STATUS: FocusId = FocusId::new("status");

/// The focus id of the button that counts.
const COUNT: FocusId = FocusId::new("count");

/// The focus id of the button that asks to quit.
const ASK: FocusId = FocusId::new("ask");

/// The focus id of the quit dialog.
const DIALOG: FocusId = FocusId::new("dialog");

/// The window's buttons, in tab order: their focus ids and labels.
const BUTTONS: [(FocusId, &str); 2] = [(COUNT, "Count"), (ASK, "Quit...")];

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a line of status and, at times, a dialog.
struct Demo {
    /// How many times Count was pressed.
    count: usize,
    /// The open dialog, if any.
    dialog: Option<Dialog<Answer>>,
    /// Which of the window's widgets has the keyboard.
    focus: FocusTree,
    /// What the last dialog was answered with.
    last: String,
    /// The colors the window is drawn in.
    theme: Theme,
}

/// The answers of the quit dialog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Answer {
    /// Stay in the app.
    Cancel,
    /// Quit.
    Quit,
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// Opens the quit dialog, trapping the focus in it.
    fn ask(&mut self) {
        self.dialog = Some(Self::quit_dialog());
        self.focus.open_trap(DIALOG);
    }

    /// Presses the window's button `id`.
    fn press(&mut self, id: FocusId) {
        match id {
            ASK => self.ask(),
            COUNT => self.count += 1,
            _ => {}
        }
    }

    /// Draws the window's buttons from `origin`, registering each with the
    /// focus tree and describing it to screen readers; the focus ring
    /// marks the focused one.
    fn draw_buttons(&mut self, frame: &mut Frame, origin: Point) {
        let metrics = frame.cell_metrics();
        let mut x = origin.x;
        for (id, label) in BUTTONS {
            let cells = label.chars().count() as f32 + 2.0 * BUTTON_PADDING;
            let rect = Rect::new(x, origin.y, cells * metrics.width, metrics.height);
            frame.draw_text(
                &StyledText::plain(label, ThemeToken::Foreground.into()),
                Point::new(x + BUTTON_PADDING * metrics.width, origin.y),
                rect,
                &self.theme,
            );
            self.focus.register(id, rect);
            let mut button = Node::new(Role::Button);
            button.set_label(label);
            button.add_action(Action::Click);
            frame.access(id, rect, button);
            x += rect.width + BUTTON_GAP * metrics.width;
        }
    }

    /// The dialog asking whether to quit.
    fn quit_dialog() -> Dialog<Answer> {
        let text = |text: &str| Span::new(text, ThemeToken::Foreground.into());
        Dialog::new(
            "Quit misty dialog?",
            vec![
                Paragraph::new()
                    .with(text(
                        "Nothing is unsaved: this dialog only shows how a body of ",
                    ))
                    .with(text("proportional, wrapped").bold())
                    .with(text(" text reads, with "))
                    .with(text("italics").italic())
                    .with(text(" and a "))
                    .with_link(
                        Span::new("link to the layout docs", ThemeToken::Accent.into()),
                        "misty::layout",
                    )
                    .with(text(" you can click.")),
                Paragraph::plain("Esc keeps the app open.", ThemeToken::Muted.into())
                    .with_align(TextAlign::End),
            ],
        )
        .with_button(DialogButton::new(Answer::Cancel, "Cancel"))
        .with_button(DialogButton::new(Answer::Quit, "Quit").primary())
    }

    /// Acts on what the dialog made of a key or an event.
    fn settle(&mut self, cx: &mut AppContext<'_, ()>, response: DialogResponse<Answer>) {
        match response {
            DialogResponse::Activated(Answer::Quit) => cx.exit(),
            DialogResponse::Activated(Answer::Cancel) => {
                self.dialog = None;
                self.last = "cancelled".to_owned();
            }
            DialogResponse::Closed => {
                self.dialog = None;
                self.last = "dismissed".to_owned();
            }
            DialogResponse::Handled | DialogResponse::Ignored => {}
            DialogResponse::Link(target) => self.last = format!("followed {target}"),
            _ => {}
        }
    }
}

impl Application for Demo {
    type Message = ();

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        cx.event_cx(window).request_redraw();
        let (mut answer, mut pressed) = (None, None);
        let mut dialog_cx = cx.event_cx(window);
        let dialog = &mut self.dialog;
        let route = self.focus.dispatch(input, |id, input| match id {
            DIALOG => dialog
                .as_mut()
                .is_some_and(|dialog| dialog.handle(input, &mut dialog_cx).keep(&mut answer)),
            ASK | COUNT if input.key().is_some_and(is_press) => {
                pressed = Some(id);
                true
            }
            _ => false,
        });
        if let Some(response) = answer {
            self.settle(cx, response);
        }
        if let Some(id) = pressed {
            self.press(id);
        }
        let asks = input.key().is_some_and(|key| {
            key.mods == Mods::NONE && matches!(key.code, KeyCode::Char('q' | 'd'))
        });
        if route == Route::Unhandled && (asks || *input == Input::CloseRequested) {
            self.ask();
        }
    }

    fn access_action(
        &mut self,
        cx: &mut AppContext<'_, ()>,
        window: WindowId,
        action: AccessAction,
    ) {
        cx.event_cx(window).request_redraw();
        match action {
            AccessAction::Click(id @ (ASK | COUNT)) if self.dialog.is_none() => self.press(id),
            AccessAction::Click(_) => {
                if let Some(dialog) = &mut self.dialog {
                    let response = dialog.access_action(DIALOG, action);
                    self.settle(cx, response);
                }
            }
            AccessAction::Focus(id) => {
                self.focus.focus(id);
            }
            AccessAction::Decrement(_) | AccessAction::Increment(_) => {}
            _ => {}
        }
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.background.into());
        let metrics = frame.cell_metrics();
        let area = frame.bounds();
        self.focus.begin_frame();
        let status = StyledText::plain(
            format!(
                "q asks to quit; counted {}; last answer: {}",
                self.count, self.last
            ),
            ThemeToken::Muted.into(),
        );
        let y = area.y + MARGIN * metrics.height;
        let clip = Rect::new(area.x, y, area.width, metrics.height);
        frame.draw_text(
            &status,
            Point::new(area.x + MARGIN * metrics.width, y),
            clip,
            &self.theme,
        );
        let mut label = Node::new(Role::Label);
        label.set_value(status.spans()[0].text.as_str());
        frame.access(STATUS, clip, label);
        let buttons = Point::new(
            area.x + MARGIN * metrics.width,
            y + (MARGIN + 1.0) * metrics.height,
        );
        self.draw_buttons(frame, buttons);
        if let Some(dialog) = &mut self.dialog {
            let rect = dialog.draw(frame, area, &self.theme);
            dialog.access(frame, DIALOG);
            self.focus.register(DIALOG, rect);
        }
        self.focus.end_frame();
        self.focus.draw(frame, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `key` presses a button: Enter or Space.
fn is_press(key: Key) -> bool {
    key.mods == Mods::NONE && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
}

fn main() -> Result<(), Error> {
    let mut app = Demo {
        count: 0,
        dialog: None,
        focus: FocusTree::new(),
        last: "none yet".to_owned(),
        theme: Theme::dark(),
    };
    app.ask();
    misty::run(app, WindowOptions::new(TITLE))
}
