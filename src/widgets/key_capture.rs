//! [`KeyCapture`]: a command's keys shown as key caps, rebound by pressing
//! the new one. Enter, Space, or a click starts capturing; the next key is
//! the command's key ([`KeyEdit::Bind`]), Backspace puts back its default
//! ([`KeyEdit::Reset`]), Delete takes its keys away ([`KeyEdit::Unbind`]),
//! and Esc, or the keyboard leaving, stops capturing with the keys as they
//! were. The app keeps the keys: the capture shows what it is given.

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::styled::{Span, StyledText};
use super::super::base::theme::{Theme, ThemeToken};
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::label::{draw_keycap_in, draw_styled_label};

// ========================================================================
// Constants
// ========================================================================

/// The blank cells between two key caps.
const KEYCAP_GAP_CELLS: f32 = 1.0;

/// What stands between a command's keys in what screen readers hear.
const KEYS_SEPARATOR: &str = ", ";

// ========================================================================
// Data Structures
// ========================================================================

/// A command's keys, rebound by pressing the new one.
#[derive(Debug, Default)]
pub struct KeyCapture {
    /// Whether the next key is taken as the command's.
    capturing: bool,
    /// Whether another command claims one of the keys.
    conflicted: bool,
    /// Where it was drawn last, for a click.
    drawn: Option<Rect>,
    /// Whether it has the keyboard.
    focused: bool,
    /// The keys shown, as the app writes them (`C-S-t`).
    keys: Vec<String>,
    /// What screen readers call it, if anything.
    label: Option<String>,
}

/// What a [`KeyCapture`] is synced to: [`Control::sync`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyCaptureProps<'a> {
    /// Whether another command claims one of the keys, which are then
    /// drawn in the danger color.
    pub conflicted: bool,
    /// The command's keys, as the app writes them.
    pub keys: &'a [String],
    /// What screen readers call it, if anything.
    pub label: Option<&'a str>,
}

/// What the user asked of a command's keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyEdit {
    /// Make this key, as [`Key`]'s notation writes it, the command's own.
    Bind(String),
    /// Put back the command's default keys.
    Reset,
    /// Take the command's keys away.
    Unbind,
}

/// What became of an input offered to a [`KeyCapture`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyCaptureResponse {
    /// The user asked this of the command's keys.
    Changed(KeyEdit),
    /// The capture used the input: it started or stopped capturing.
    Handled,
    /// The input is not the capture's.
    Ignored,
}

// ========================================================================
// KeyCapture Implementations
// ========================================================================

impl KeyCapture {
    /// A capture showing no keys, not capturing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the next key is taken as the command's.
    pub fn is_capturing(&self) -> bool {
        self.capturing
    }

    /// Describes it to screen readers as `id`: a button named for its
    /// label, its keys its value, pressed to capture.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(rect) = self.drawn else {
            return;
        };
        let mut node = Node::new(Role::Button);
        if let Some(label) = &self.label {
            node.set_label(label.as_str());
        }
        node.set_value(self.keys.join(KEYS_SEPARATOR));
        node.add_action(Action::Click);
        node.add_action(Action::Focus);
        frame.access(id, rect, node);
    }

    /// The edit a key asks while capturing.
    fn capture(&mut self, key: Key) -> KeyCaptureResponse {
        self.capturing = false;
        let plain = key.mods == Mods::NONE;
        let edit = match key.code {
            KeyCode::Esc if plain => return KeyCaptureResponse::Handled,
            KeyCode::Backspace if plain => KeyEdit::Reset,
            KeyCode::Delete if plain => KeyEdit::Unbind,
            _ => KeyEdit::Bind(key.to_string()),
        };
        KeyCaptureResponse::Changed(edit)
    }
}

impl Widget for KeyCapture {
    type Output = ();
    type Response = KeyCaptureResponse;

    /// While capturing, the next key; otherwise Enter or Space while
    /// focused, or a click on it, starts capturing.
    fn handle(&mut self, input: &Input, _cx: &mut EventCx<'_>) -> KeyCaptureResponse {
        match input {
            Input::Key(key) if self.capturing => self.capture(*key),
            Input::Key(key)
                if self.focused
                    && key.mods == Mods::NONE
                    && matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) =>
            {
                self.capturing = true;
                KeyCaptureResponse::Handled
            }
            Input::PointerButton(button)
                if button.pressed
                    && button.button == PointerButton::Left
                    && self
                        .drawn
                        .is_some_and(|rect| rect.contains(button.position)) =>
            {
                self.capturing = true;
                KeyCaptureResponse::Handled
            }
            _ => KeyCaptureResponse::Ignored,
        }
    }

    /// Draws the keys as key caps from the left of `area` (in the danger
    /// color while another command claims one), or while
    /// capturing, the prompt to press one in the accent color.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.drawn = Some(area);
        let metrics = frame.cell_metrics();
        let top = area.y + ((area.height - metrics.height) / 2.0).round();
        if self.capturing {
            let prompt = StyledText::from(
                Span::new(theme.strings.key_prompt.as_ref(), ThemeToken::Accent.into())
                    .emphasized(theme.emphasis),
            );
            draw_styled_label(frame, theme, &prompt, Point::new(area.x, top), area);
            return;
        }
        let gap = (KEYCAP_GAP_CELLS * metrics.width).round();
        let color = match self.conflicted {
            true => ThemeToken::Danger,
            false => ThemeToken::Foreground,
        };
        let mut x = area.x;
        for key in &self.keys {
            let at = Point::new(x, top);
            x += draw_keycap_in(frame, theme, key, color.into(), at, area) + gap;
        }
    }

    /// Losing the keyboard stops capturing.
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.capturing = false;
        }
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl Control for KeyCapture {
    type Props<'a> = KeyCaptureProps<'a>;
    type Value = KeyEdit;

    /// Takes the command's keys and its name.
    fn sync(&mut self, props: KeyCaptureProps<'_>) {
        self.conflicted = props.conflicted;
        if self.keys != props.keys {
            self.keys = props.keys.to_vec();
        }
        if self.label.as_deref() != props.label {
            self.label = props.label.map(str::to_owned);
        }
    }

    /// One row, whatever the width.
    fn measure(&self, _width: f32, metrics: CellMetrics, _scale_factor: f64) -> f32 {
        metrics.height.ceil()
    }

    /// What the user asked of the keys.
    fn changed(&self, response: KeyCaptureResponse) -> Option<KeyEdit> {
        match response {
            KeyCaptureResponse::Changed(edit) => Some(edit),
            KeyCaptureResponse::Handled | KeyCaptureResponse::Ignored => None,
        }
    }
}

impl Outcome for KeyCaptureResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

/// A screen reader's click captures, as Enter does.
pub(crate) fn act(capture: &mut KeyCapture, action: AccessAction) -> Option<KeyCaptureResponse> {
    match action {
        AccessAction::Click(_) => {
            capture.capturing = true;
            Some(KeyCaptureResponse::Handled)
        }
        _ => None,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestWindow;

    fn key(code: KeyCode, mods: Mods) -> Input {
        Input::Key(Key::new(code, mods))
    }

    #[test]
    fn test_enter_captures_the_next_key_and_backspace_delete_and_esc_mean_their_edits() {
        let mut window = TestWindow::new(200, 40);
        let mut capture = KeyCapture::new();
        let mut cx = window.cx();
        let plain = |code| key(code, Mods::NONE);
        assert_eq!(
            capture.handle(&plain(KeyCode::Enter), &mut cx),
            KeyCaptureResponse::Ignored,
            "nothing while unfocused"
        );
        capture.set_focused(true);
        let mut edit = |capture: &mut KeyCapture, input: Input| {
            capture.handle(&plain(KeyCode::Enter), &mut cx);
            assert!(capture.is_capturing());
            capture.handle(&input, &mut cx)
        };
        let ctrl_t = key(KeyCode::Char('t'), Mods::CTRL);
        assert_eq!(
            edit(&mut capture, ctrl_t),
            KeyCaptureResponse::Changed(KeyEdit::Bind("C-t".to_owned()))
        );
        assert_eq!(
            edit(&mut capture, plain(KeyCode::Backspace)),
            KeyCaptureResponse::Changed(KeyEdit::Reset)
        );
        assert_eq!(
            edit(&mut capture, plain(KeyCode::Delete)),
            KeyCaptureResponse::Changed(KeyEdit::Unbind)
        );
        assert_eq!(
            edit(&mut capture, plain(KeyCode::Esc)),
            KeyCaptureResponse::Handled
        );
        assert!(!capture.is_capturing());
        capture.handle(&plain(KeyCode::Enter), &mut cx);
        capture.set_focused(false);
        assert!(!capture.is_capturing(), "leaving stops it");
    }
}
