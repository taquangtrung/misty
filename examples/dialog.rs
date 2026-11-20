//! A dialog over a window described as a tree: `q` or the window's
//! Quit... button asks whether to quit. Tab, Shift+Tab, Left, and Right
//! move between the dialog's buttons, Enter chooses, Esc dismisses; `d`
//! asks again. The body is rich text in the proportional font, wrapped to
//! the dialog, with a link that a click follows.
//!
//! The window's own two buttons take the focus in turn: Tab moves between
//! them with a focus ring, Enter or Space presses the focused one, and a
//! click focuses one. The dialog is modal: while it is open no key or
//! click reaches the window behind it, and when it closes the focus goes
//! back to the button that had it.
//!
//! Screen readers see the window's buttons and the open dialog with its
//! paragraphs and buttons; they can focus or click any button.
//!
//! ```sh
//! cargo run -p misty --example dialog
//! ```

use misty::view::{button, column, dialog, row, text};
use misty::{
    App, AppCx, Element, Key, KeyCode, Mods, Paragraph, Span, TextAlign, Theme, ThemeToken,
    WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty dialog";

/// The blank cells around the content, in columns and rows.
const MARGIN: f32 = 1.0;

/// The dialog's title.
const QUESTION: &str = "Quit misty dialog?";

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a line of status, two buttons, and, at times, a dialog.
struct Demo {
    /// Whether the dialog asking to quit is open.
    asking: bool,
    /// How many times Count was pressed.
    count: usize,
    /// What the last dialog was answered with.
    last: String,
    /// Whether the user chose to quit.
    quitting: bool,
    /// The colors the window is drawn in.
    theme: Theme,
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// The demo at start, asking whether to quit.
    fn new() -> Self {
        Self {
            asking: true,
            count: 0,
            last: "none yet".to_owned(),
            quitting: false,
            theme: Theme::dark(),
        }
    }

    /// Closes the dialog, its answer `answer`.
    fn answer(&mut self, answer: &str) {
        self.asking = false;
        self.last = answer.to_owned();
    }
}

impl App for Demo {
    fn view(&self, _window: WindowId) -> Element<'_, Self> {
        let status = format!(
            "q asks to quit; counted {}; last answer: {}",
            self.count, self.last
        );
        column([
            text(status).muted().into(),
            row([
                button("Count")
                    .on_click(|app: &mut Self| app.count += 1)
                    .into(),
                button("Quit...")
                    .on_click(|app: &mut Self| app.asking = true)
                    .into(),
            ]),
        ])
        .padding(MARGIN, MARGIN)
        .overlay_if(
            self.asking,
            dialog(QUESTION, quit_body())
                .button("Cancel", |app: &mut Self| app.answer("cancelled"))
                .primary("Quit", |app: &mut Self| app.quitting = true)
                .on_close(|app: &mut Self| app.answer("dismissed"))
                .on_link(|app: &mut Self, target| app.last = format!("followed {target}")),
        )
    }

    /// Asks whether to quit on `q` or `d`.
    fn key(&mut self, key: Key, _cx: &mut AppCx<'_>) -> bool {
        let asks = key.mods == Mods::NONE && matches!(key.code, KeyCode::Char('q' | 'd'));
        if asks {
            self.asking = true;
        }
        asks
    }

    /// Quits once the dialog's Quit button was chosen.
    fn should_exit(&self) -> bool {
        self.quitting
    }

    fn theme(&self) -> &Theme {
        &self.theme
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The body of the dialog asking whether to quit.
fn quit_body() -> Vec<Paragraph> {
    let text = |text: &str| Span::new(text, ThemeToken::Foreground.into());
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
    ]
}

fn main() -> Result<(), misty::Error> {
    misty::run_app(Demo::new(), WindowOptions::new(TITLE))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, feature = "testing"))]
mod tests {
    use misty::AccessAction;
    use misty::accesskit::Role;
    use misty::testing::TreeWindow;

    use super::*;

    /// The window showing the demo, its dialog closed.
    fn window() -> TreeWindow<Demo> {
        let demo = Demo {
            asking: false,
            ..Demo::new()
        };
        TreeWindow::new(demo, TITLE, 800, 600)
    }

    #[test]
    fn test_the_dialog_opens_over_the_window_and_screen_readers_see_it() {
        let mut window = window();
        assert!(!window.ui_text().contains("Dialog"));
        window.press("q");
        let text = window.ui_text();
        assert!(text.contains(&format!("Dialog \"{QUESTION}\"")), "{text}");
        assert!(text.contains("Button \"Quit\" focused"), "{text}");
        let count = text.find("Button \"Count\"");
        assert!(
            count.is_some() && count < text.find("Dialog"),
            "the dialog is described after the window under it: {text}"
        );
    }

    #[test]
    fn test_the_dialog_is_open_at_start() {
        let window = TreeWindow::new(Demo::new(), TITLE, 800, 600);
        assert!(window.ui_text().contains("Button \"Quit\" focused"));
    }

    #[test]
    fn test_esc_dismisses_and_the_focus_goes_back_to_the_opener() {
        let mut window = window();
        window.press("<tab>");
        assert!(window.ui_text().contains("Button \"Quit...\" focused"));
        window.press("RET");
        assert!(window.app().asking);
        window.press("<esc>");
        assert!(!window.app().asking);
        assert_eq!(window.app().last, "dismissed");
        assert!(window.ui_text().contains("Button \"Quit...\" focused"));
    }

    #[test]
    fn test_no_key_or_click_reaches_the_window_while_the_dialog_is_open() {
        let mut window = window();
        let count = window.find(Role::Button, "Count");
        window.press("q");
        window.click(Role::Button, "Count");
        window.act(AccessAction::Click(count));
        assert_eq!(window.app().count, 0);
        let quit = window.find(Role::Button, "Quit");
        window.act(AccessAction::Click(quit));
        assert!(window.is_closed(), "Quit quits");
    }

    #[test]
    fn test_tab_moves_between_the_dialogs_buttons_and_enter_chooses() {
        let mut window = window();
        window.press("q <tab> RET");
        assert_eq!(window.app().last, "cancelled");
        assert!(!window.is_closed());
        window.click(Role::Button, "Count");
        assert_eq!(window.app().count, 1, "the window takes clicks again");
    }

    #[test]
    fn test_a_click_on_a_dialog_button_chooses_it() {
        let mut window = window();
        window.press("q");
        window.click(Role::Button, "Cancel");
        assert_eq!(window.app().last, "cancelled");
    }
}
