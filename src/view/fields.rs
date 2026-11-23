//! The field elements that edit a value richer than a line of text, each
//! controlled as [`controls`](super::controls) are: [`color_field`], a color
//! typed as `#rrggbb` beside a swatch; [`names_field`], names typed as
//! words; and [`key_capture`], a command's keys rebound by pressing the
//! new one. The value comes from the app's state; the callback given to
//! `on_change` is the only way it changes, and a field without one is
//! read-only.

use std::borrow::Cow;
use std::rc::Rc;

use super::super::app::access::AccessAction;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::Widget;
use super::super::base::color::Rgb;
use super::super::base::geometry::Rect;
use super::super::base::theme::Theme;
use super::super::widgets::color_field::{self, ColorFieldProps, ColorFieldResponse};
use super::super::widgets::key_capture::{self, KeyCaptureProps, KeyCaptureResponse, KeyEdit};
use super::super::widgets::names_field::{self, NamesFieldProps, NamesFieldResponse};
use super::controls::{Callback, call};
use super::element::{Element, Kind};
use super::node::{Call, Drawn, Spec};

// ========================================================================
// Data Structures
// ========================================================================

/// A color typed as `#rrggbb` beside a swatch, as an element. Made by
/// [`color_field`].
pub struct ColorField<A> {
    /// What runs with the color typed.
    on_change: Option<Callback<A, Rgb>>,
    /// The color the app holds.
    value: Rgb,
}

/// Names typed as words, as an element. Made by [`names_field`].
pub struct NamesField<'a, A> {
    /// What runs with the names typed.
    on_change: Option<Callback<A, Vec<String>>>,
    /// The names the app holds, in order.
    value: &'a [String],
}

/// A command's keys rebound by pressing the new one, as an element. Made
/// by [`key_capture`].
pub struct KeyCapture<'a, A> {
    /// Whether another command claims one of the keys.
    conflicted: bool,
    /// The command's keys, as the app writes them.
    keys: Cow<'a, [String]>,
    /// What runs with what the user asked of the keys.
    on_change: Option<Callback<A, KeyEdit>>,
}

// ========================================================================
// ColorField Implementations
// ========================================================================

impl<A> ColorField<A> {
    /// This field running `apply` on the app's state with each color typed.
    pub fn on_change(mut self, apply: impl Fn(&mut A, Rgb) + 'static) -> Self {
        self.on_change = Some(Rc::new(apply));
        self
    }
}

impl<A: 'static> Spec<A> for ColorField<A> {
    type Widget = color_field::ColorField;

    const KIND: &'static str = "color_field";

    fn create(&self) -> color_field::ColorField {
        color_field::ColorField::new()
    }

    fn props<'b>(&'b self, name: Option<&'b str>) -> ColorFieldProps<'b> {
        ColorFieldProps {
            label: name,
            value: self.value,
        }
    }

    fn call(&self, color: Rgb) -> Option<Call<A>> {
        call(self.on_change.as_ref(), color)
    }

    fn draw(
        &self,
        widget: &mut color_field::ColorField,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &color_field::ColorField, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        _widget: &mut color_field::ColorField,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<ColorFieldResponse> {
        None
    }

    fn caret(&self, widget: &color_field::ColorField) -> Option<Rect> {
        widget.cursor_area()
    }
}

impl<'a, A: 'static> From<ColorField<A>> for Element<'a, A> {
    fn from(field: ColorField<A>) -> Self {
        Element::new(Kind::Control(Box::new(field)))
    }
}

// ========================================================================
// NamesField Implementations
// ========================================================================

impl<A> NamesField<'_, A> {
    /// This field running `apply` on the app's state with the names each
    /// edit leaves.
    pub fn on_change(mut self, apply: impl Fn(&mut A, Vec<String>) + 'static) -> Self {
        self.on_change = Some(Rc::new(apply));
        self
    }
}

impl<A: 'static> Spec<A> for NamesField<'_, A> {
    type Widget = names_field::NamesField;

    const KIND: &'static str = "names_field";

    fn create(&self) -> names_field::NamesField {
        names_field::NamesField::new()
    }

    fn props<'b>(&'b self, name: Option<&'b str>) -> NamesFieldProps<'b> {
        NamesFieldProps {
            label: name,
            value: self.value,
        }
    }

    fn call(&self, names: Vec<String>) -> Option<Call<A>> {
        call(self.on_change.as_ref(), names)
    }

    fn draw(
        &self,
        widget: &mut names_field::NamesField,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &names_field::NamesField, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        _widget: &mut names_field::NamesField,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<NamesFieldResponse> {
        None
    }

    fn caret(&self, widget: &names_field::NamesField) -> Option<Rect> {
        widget.cursor_area()
    }
}

impl<'a, A: 'static> From<NamesField<'a, A>> for Element<'a, A> {
    fn from(field: NamesField<'a, A>) -> Self {
        Element::new(Kind::Control(Box::new(field)))
    }
}

// ========================================================================
// KeyCapture Implementations
// ========================================================================

impl<A> KeyCapture<'_, A> {
    /// This capture drawing its keys in the danger color while
    /// `conflicted`: another command claims one of them.
    pub fn conflicted(self, conflicted: bool) -> Self {
        Self { conflicted, ..self }
    }

    /// This capture running `apply` on the app's state with what the user
    /// asked of the keys.
    pub fn on_change(mut self, apply: impl Fn(&mut A, KeyEdit) + 'static) -> Self {
        self.on_change = Some(Rc::new(apply));
        self
    }
}

impl<A: 'static> Spec<A> for KeyCapture<'_, A> {
    type Widget = key_capture::KeyCapture;

    const KIND: &'static str = "key_capture";

    fn create(&self) -> key_capture::KeyCapture {
        key_capture::KeyCapture::new()
    }

    fn props<'b>(&'b self, name: Option<&'b str>) -> KeyCaptureProps<'b> {
        KeyCaptureProps {
            conflicted: self.conflicted,
            keys: &self.keys,
            label: name,
        }
    }

    fn call(&self, edit: KeyEdit) -> Option<Call<A>> {
        call(self.on_change.as_ref(), edit)
    }

    fn draw(
        &self,
        widget: &mut key_capture::KeyCapture,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &key_capture::KeyCapture, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        widget: &mut key_capture::KeyCapture,
        _id: FocusId,
        action: AccessAction,
    ) -> Option<KeyCaptureResponse> {
        key_capture::act(widget, action)
    }
}

impl<'a, A: 'static> From<KeyCapture<'a, A>> for Element<'a, A> {
    fn from(capture: KeyCapture<'a, A>) -> Self {
        Element::new(Kind::Control(Box::new(capture)))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A color typed as `#rrggbb` in a framed field, a swatch of `value` at its
/// left. The color changes once the text reads as one; until then the text
/// is the user's draft, and `value` shows again when the field is left.
pub fn color_field<A>(value: Rgb) -> ColorField<A> {
    ColorField {
        on_change: None,
        value,
    }
}

/// Names, in order, typed as words in a framed field: `value` one space
/// apart, changed as the words are; a trailing space or a half-typed word
/// is kept while the field has focus.
pub fn names_field<A>(value: &[String]) -> NamesField<'_, A> {
    NamesField {
        on_change: None,
        value,
    }
}

/// A command's `keys` (as the app writes them, `C-S-t`), lent or built for
/// the view, as key caps.
/// Enter, Space, or a click captures the next key as the command's own
/// ([`KeyEdit::Bind`]); while capturing, Backspace asks for its default
/// keys ([`KeyEdit::Reset`]), Delete for none ([`KeyEdit::Unbind`]), and Esc
/// stops.
pub fn key_capture<'a, A>(keys: impl Into<Cow<'a, [String]>>) -> KeyCapture<'a, A> {
    KeyCapture {
        conflicted: false,
        keys: keys.into(),
        on_change: None,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TreeWindow;
    use crate::view::element::column;
    use crate::{App, WindowId};

    /// An app holding a color, names, and a command's keys, each edited by
    /// a field.
    struct Fields {
        color: Rgb,
        keys: Vec<String>,
        names: Vec<String>,
        theme: Theme,
    }

    impl App for Fields {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            column([
                color_field(self.color)
                    .on_change(|app: &mut Self, color| app.color = color)
                    .into(),
                names_field(&self.names)
                    .on_change(|app: &mut Self, names| app.names = names)
                    .into(),
                key_capture(self.keys.as_slice())
                    .on_change(|app: &mut Self, edit| match edit {
                        KeyEdit::Bind(key) => app.keys = vec![key],
                        KeyEdit::Reset | KeyEdit::Unbind => app.keys.clear(),
                    })
                    .into(),
            ])
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    #[test]
    fn test_each_field_changes_the_apps_value_through_its_callback() {
        let fields = Fields {
            color: Rgb::new(0, 0, 0),
            keys: Vec::new(),
            names: vec!["tool".to_owned()],
            theme: Theme::dark(),
        };
        let mut window = TreeWindow::new(fields, "t", 400, 200);
        window.press("<bs> <bs> <bs> <bs> <bs> <bs>");
        window.type_text("ff0000");
        assert_eq!(window.app().color, Rgb::new(0xff, 0, 0));
        window.press("<tab>");
        window.type_text(" bell");
        assert_eq!(window.app().names, ["tool", "bell"]);
        window.press("<tab> <cr> C-t");
        assert_eq!(window.app().keys, ["C-t"]);
    }
}
