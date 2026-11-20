//! [`NamesField`]: names, in order, edited as words in a framed field (a
//! status bar's segments, a list of back-ends). The names change as the
//! words do; while the field has focus the text is the user's draft, kept
//! as typed (a trailing space, a half-typed word), and the names the app
//! holds show again, one space apart, once it loses focus, or on Esc.

use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::geometry::Rect;
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::font::CellMetrics;
use super::draft::{Draft, DraftEvent};

// ========================================================================
// Constants
// ========================================================================

/// What stands between two names written out.
const NAME_SEPARATOR: &str = " ";

// ========================================================================
// Data Structures
// ========================================================================

/// Names, in order, edited as words.
#[derive(Debug, Default)]
pub struct NamesField {
    /// The field the names are typed in.
    draft: Draft,
    /// The names the app holds.
    value: Vec<String>,
}

/// What a [`NamesField`] is synced to: [`Control::sync`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NamesFieldProps<'a> {
    /// What screen readers call it, if anything.
    pub label: Option<&'a str>,
    /// The names the app holds, in order.
    pub value: &'a [String],
}

/// What became of an input offered to a [`NamesField`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NamesFieldResponse {
    /// The user changed the names to these.
    Changed(Vec<String>),
    /// The field used the input; the names are as they were.
    Handled,
    /// The input is not the field's.
    Ignored,
}

// ========================================================================
// NamesField Implementations
// ========================================================================

impl NamesField {
    /// A field holding no names.
    pub fn new() -> Self {
        Self::default()
    }

    /// The names the field holds, in order.
    pub fn value(&self) -> &[String] {
        &self.value
    }

    /// Describes the field to screen readers as `id`.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        self.draft.access(frame, id);
    }

    /// The caret's cell, for an input method's candidates.
    pub fn cursor_area(&self) -> Option<Rect> {
        self.draft.cursor_area()
    }
}

impl Widget for NamesField {
    type Output = ();
    type Response = NamesFieldResponse;

    /// Edits the words; words naming other names than it holds change
    /// them.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> NamesFieldResponse {
        let written = self.value.join(NAME_SEPARATOR);
        match self.draft.handle(input, cx, &written) {
            DraftEvent::Edited(text) => {
                let names: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
                match names == self.value {
                    true => NamesFieldResponse::Handled,
                    false => {
                        self.value.clone_from(&names);
                        NamesFieldResponse::Changed(names)
                    }
                }
            }
            DraftEvent::Handled => NamesFieldResponse::Handled,
            DraftEvent::Ignored => NamesFieldResponse::Ignored,
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.draft.draw(frame, area, theme);
    }

    fn set_focused(&mut self, focused: bool) {
        self.draft.set_focused(focused);
    }

    fn is_focused(&self) -> bool {
        self.draft.is_focused()
    }
}

impl Control for NamesField {
    type Props<'a> = NamesFieldProps<'a>;
    type Value = Vec<String>;

    /// Takes the app's names, shown unless the user is typing.
    fn sync(&mut self, props: NamesFieldProps<'_>) {
        if self.value != props.value {
            self.value = props.value.to_vec();
        }
        self.draft
            .sync(props.label, &props.value.join(NAME_SEPARATOR));
    }

    /// The field's height: one row and its frame.
    fn measure(&self, width: f32, metrics: CellMetrics, scale_factor: f64) -> f32 {
        self.draft.measure(width, metrics, scale_factor)
    }

    /// The names the user typed.
    fn changed(&self, response: NamesFieldResponse) -> Option<Vec<String>> {
        match response {
            NamesFieldResponse::Changed(names) => Some(names),
            NamesFieldResponse::Handled | NamesFieldResponse::Ignored => None,
        }
    }
}

impl Outcome for NamesFieldResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::key::{Key, KeyCode, Mods};
    use crate::testing::TestWindow;

    fn key(code: KeyCode) -> Input {
        Input::Key(Key::new(code, Mods::NONE))
    }

    #[test]
    fn test_names_change_by_their_words_and_a_trailing_space_is_kept_while_typing() {
        let mut window = TestWindow::new(200, 40);
        let mut field = NamesField::new();
        let held = vec!["tool".to_owned()];
        field.sync(NamesFieldProps {
            label: None,
            value: &held,
        });
        field.set_focused(true);
        let mut changes = Vec::new();
        for c in " bell ".chars() {
            if let NamesFieldResponse::Changed(names) =
                field.handle(&key(KeyCode::Char(c)), &mut window.cx())
            {
                changes.push(names);
            }
        }
        assert_eq!(changes.last().map(Vec::len), Some(2), "{changes:?}");
        let value = field.value().to_vec();
        field.sync(NamesFieldProps {
            label: None,
            value: &value,
        });
        assert_eq!(field.draft.text(), "tool bell ", "the space typed is kept");
        field.set_focused(false);
        field.sync(NamesFieldProps {
            label: None,
            value: &value,
        });
        assert_eq!(field.draft.text(), "tool bell");
    }
}
