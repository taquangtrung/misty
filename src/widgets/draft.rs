//! [`Draft`]: the framed text field a control edits a value in as text (a
//! color as `#rrggbb`, names as words), applied as soon as the text reads
//! as a value. While the field has focus and the user has typed into it,
//! the text is the user's draft, kept even when it does not read as a
//! value yet (`#12`, a trailing space); the value the app holds shows again
//! once the field loses focus, or on Esc.

use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::Widget;
use super::super::base::geometry::Rect;
use super::super::base::key::{KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::font::CellMetrics;
use super::text_field::{TextField, TextFieldProps, TextFieldResponse};

// ========================================================================
// Data Structures
// ========================================================================

/// A framed text field holding the user's draft of a value while it is
/// edited.
#[derive(Debug)]
pub(crate) struct Draft {
    /// Whether the text is the user's draft rather than the app's value.
    edited: bool,
    /// The field the text is edited in.
    field: TextField,
}

/// What became of an input offered to a [`Draft`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DraftEvent {
    /// The user changed the text to this.
    Edited(String),
    /// The field used the input and the text is as it was.
    Handled,
    /// The input is not the field's.
    Ignored,
}

// ========================================================================
// Draft Implementations
// ========================================================================

impl Default for Draft {
    fn default() -> Self {
        Self::new()
    }
}

impl Draft {
    /// An empty field.
    pub(crate) fn new() -> Self {
        Self {
            edited: false,
            field: TextField::new().with_frame(),
        }
    }

    /// Shows `text`, the app's value written out, and calls the field
    /// `label`: the text is kept as the user typed it while it is a draft.
    pub(crate) fn sync(&mut self, label: Option<&str>, text: &str) {
        let shown = match self.edited {
            true => self.field.text().to_owned(),
            false => text.to_owned(),
        };
        self.field.sync(TextFieldProps {
            label,
            placeholder: "",
            text: &shown,
        });
    }

    /// Hands `input` to the field: Esc puts back `value`, the app's value
    /// written out, ending the draft; the rest edits the text.
    pub(crate) fn handle(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
        value: &str,
    ) -> DraftEvent {
        if let Input::Key(key) = input
            && key.code == KeyCode::Esc
            && key.mods == Mods::NONE
            && self.edited
        {
            self.edited = false;
            self.field.set_text(value);
            return DraftEvent::Handled;
        }
        match self.field.handle(input, cx) {
            TextFieldResponse::Changed => {
                self.edited = true;
                DraftEvent::Edited(self.field.text().to_owned())
            }
            TextFieldResponse::Handled => DraftEvent::Handled,
            TextFieldResponse::Ignored => DraftEvent::Ignored,
        }
    }

    /// The text shown: the user's draft, or the app's value.
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        self.field.text()
    }

    /// Draws the field in `area`.
    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.field.draw(frame, area, theme);
    }

    /// Gives the field the keyboard or takes it away; losing it ends the
    /// draft, so the next sync shows the app's value.
    pub(crate) fn set_focused(&mut self, focused: bool) {
        self.field.set_focused(focused);
        if !focused {
            self.edited = false;
        }
    }

    /// Whether the field has the keyboard.
    pub(crate) fn is_focused(&self) -> bool {
        self.field.is_focused()
    }

    /// How tall the field is: one row and its frame.
    pub(crate) fn measure(&self, width: f32, metrics: CellMetrics, scale_factor: f64) -> f32 {
        self.field.measure(width, metrics, scale_factor)
    }

    /// The caret's cell, for an input method's candidates.
    pub(crate) fn cursor_area(&self) -> Option<Rect> {
        self.field.cursor_area()
    }

    /// Describes the field to screen readers as `id`.
    pub(crate) fn access(&self, frame: &mut Frame, id: FocusId) {
        self.field.access(frame, id);
    }
}
