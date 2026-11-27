//! The control elements of a tree, each built by a free function and
//! changed by `with`-style methods: [`button`] (and [`icon_button`]),
//! [`checkbox`], [`radio`],
//! [`select`], [`slider`], [`stepper`], [`progress`], [`text_field`], and
//! [`text_area`]. Each is *controlled*: its value comes from the app's
//! state in [`App::view`](crate::App::view), and the only way it changes
//! is through the callback it was given (`on_change`, or `on_click` for a
//! button), a closure on the app's state that misty runs once the tree is
//! dropped. A control without one is read-only: drawn disabled, and
//! described so to screen readers.
//!
//! Callbacks are `'static`: misty runs them after it drops the tree, which
//! borrows the app, so a callback cannot borrow from it too. One that needs
//! a value from `view` moves an owned copy in.

use std::convert::Infallible;
use std::rc::Rc;

use super::super::app::access::AccessAction;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::Widget;
use super::super::base::geometry::Rect;
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::input::event::Input;
use super::super::render::font::CellMetrics;
use super::super::render::image::Svg;
use super::super::widgets::button::{self, ButtonProps, ButtonResponse, ButtonVariant};
use super::super::widgets::checkbox::{self, CheckboxProps, CheckboxResponse};
use super::super::widgets::number_range::NumberRange;
use super::super::widgets::progress::{ProgressBar, ProgressProps};
use super::super::widgets::radio::{RadioGroup, RadioGroupProps, RadioGroupResponse};
use super::super::widgets::select::{self, SelectProps, SelectResponse};
use super::super::widgets::slider::{self, NumberProps, SliderResponse};
use super::super::widgets::stepper::{self, StepperResponse};
use super::super::widgets::text_area::{self, TextAreaProps, TextAreaResponse};
use super::super::widgets::text_field::{self, TextFieldProps, TextFieldResponse};
use super::element::{Element, Kind};
use super::node::{Call, Drawn, Spec};

// ========================================================================
// Data Structures
// ========================================================================

/// What a control runs on the app's state with the value the user changed
/// it to.
pub(crate) type Callback<A, V> = Rc<dyn Fn(&mut A, V)>;

/// A push button, or a toggle that stays pressed in, as an element. Made
/// by [`button`], or by [`icon_button`] for one showing only an icon.
pub struct Button<'a, A> {
    /// Whether a toggle is pressed in, or `None` for a push button.
    active: Option<bool>,
    /// Whether it takes input.
    enabled: bool,
    /// The icon left of its label, if any.
    icon: Option<&'a Svg>,
    /// What it says; empty for an icon-only button.
    label: &'a str,
    /// What an icon-only button is called: its tooltip and its name for
    /// screen readers.
    name: Option<&'a str>,
    /// What a click runs.
    on_click: Option<Callback<A, ()>>,
    /// How it looks.
    variant: ButtonVariant,
}

/// A checkbox or a switch as an element. Made by [`checkbox`].
pub struct Checkbox<'a, A> {
    /// Whether it is on.
    checked: bool,
    /// Whether it takes input.
    enabled: bool,
    /// What it says beside its box.
    label: &'a str,
    /// What turning it on or off runs.
    on_change: Option<Callback<A, bool>>,
    /// Whether it is drawn as a switch rather than a box.
    switch: bool,
}

/// A column of options of which the user picks one, as an element. Made
/// by [`radio`].
pub struct Radio<'a, A, T> {
    /// The value of the chosen option.
    chosen: T,
    /// Whether it takes input.
    enabled: bool,
    /// What choosing another option runs.
    on_change: Option<Callback<A, T>>,
    /// The options, top to bottom: what each hands back, and what it says.
    options: &'a [(T, &'a str)],
}

/// A dropdown of options of which the user picks one, as an element. Made
/// by [`select`].
pub struct Select<'a, A, T> {
    /// The value of the chosen option.
    chosen: T,
    /// Whether it takes input.
    enabled: bool,
    /// What choosing another option runs.
    on_change: Option<Callback<A, T>>,
    /// The options, top to bottom: what each hands back, and what it says.
    options: Options<'a, T>,
}

/// A dropdown's options: lent by the app, or built for the view.
enum Options<'a, T> {
    /// Lent, as the app holds them.
    Lent(&'a [(T, &'a str)]),
    /// Built for the view from what the app holds.
    Owned(Vec<(T, &'a str)>),
}

/// A number picked along a track, or stepped with minus and plus buttons,
/// as an element. Made by [`slider`] and [`stepper`].
pub struct Number<A> {
    /// Whether it takes input.
    enabled: bool,
    /// Whether it is drawn as a stepper rather than a slider.
    look: NumberLook,
    /// What moving it runs.
    on_change: Option<Callback<A, f64>>,
    /// The values it takes.
    range: NumberRange,
    /// Its value.
    value: f64,
}

/// A progress bar as an element. Made by [`progress`].
pub struct Progress {
    /// How far the task has come, from 0 to 1, or `None` when not known.
    fraction: Option<f32>,
}

/// A one-line text input as an element. Made by [`text_field`].
pub struct TextField<'a, A> {
    /// Whether its text is masked, as a password's.
    masked: bool,
    /// What Esc runs while it has the keyboard, if anything.
    on_cancel: Option<Callback<A, ()>>,
    /// What typing runs.
    on_change: Option<Callback<A, String>>,
    /// What Enter runs while it has the keyboard, if anything.
    on_submit: Option<Callback<A, ()>>,
    /// What it shows while empty.
    placeholder: &'a str,
    /// Its text.
    text: &'a str,
}

/// A text input of several lines as an element. Made by [`text_area`].
pub struct TextArea<'a, A> {
    /// What typing runs.
    on_change: Option<Callback<A, String>>,
    /// What it shows while empty.
    placeholder: &'a str,
    /// How many rows tall it is, or `None` to grow with its text.
    rows: Option<usize>,
    /// Its text.
    text: &'a str,
    /// The number the app changes whenever it changes the text.
    version: u64,
}

/// How a number element is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NumberLook {
    /// A track with a knob.
    Slider,
    /// The value between minus and plus buttons.
    Stepper,
}

// ========================================================================
// Button Implementations
// ========================================================================

impl<'a, A> Button<'a, A> {
    /// This button a toggle, pressed in while `active`; its `on_click`
    /// stores the opposite.
    pub fn toggle(self, active: bool) -> Self {
        Self {
            active: Some(active),
            ..self
        }
    }

    /// This button taking input (`true`, the default), or disabled.
    pub fn enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    /// This button showing `icon`, a one-color SVG (as a title bar's app
    /// buttons take), left of its label, drawn a row square in the theme's
    /// text color (muted while disabled) whatever the button's state; the
    /// button is as much wider. A button with no label shows only the icon
    /// and must be named: build it with [`icon_button`], which takes the
    /// name.
    pub fn icon(self, icon: &'a Svg) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }

    /// This button in the look `variant` rather than the default filled
    /// one, alike for a label, an icon, or both: `ButtonVariant::Ghost` is
    /// flat at rest and takes the theme's subtle hover fill only under the
    /// pointer, while pressed, and the selection fill while a toggle is
    /// pressed in, as a toolbar's buttons are; `Primary` and `Danger` fill
    /// it with the accent or the danger color. The focus ring is drawn
    /// around it in every look.
    pub fn variant(self, variant: ButtonVariant) -> Self {
        Self { variant, ..self }
    }

    /// What runs on the app's state when the user clicks the button (or
    /// presses Enter or Space on it).
    pub fn on_click(self, apply: impl Fn(&mut A) + 'static) -> Self {
        Self {
            on_click: Some(Rc::new(move |app: &mut A, ()| apply(app))),
            ..self
        }
    }
}

impl<A: 'static> Spec<A> for Button<'_, A> {
    type Widget = button::Button;

    const KIND: &'static str = "button";

    fn create(&self) -> button::Button {
        if self.label.is_empty() && self.name.is_none() {
            log::warn!(
                "view::button: a button with no label has no name for screen readers or a \
                 tooltip; build an icon-only button with `view::icon_button(icon, name)`"
            );
        }
        button::Button::new(self.label)
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> ButtonProps<'b> {
        ButtonProps {
            active: self.active,
            enabled: self.enabled && self.on_click.is_some(),
            label: self.label,
        }
    }

    fn call(&self, (): ()) -> Option<Call<A>> {
        call(self.on_click.as_ref(), ())
    }

    fn draw(
        &self,
        widget: &mut button::Button,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.show_icon(self.icon, self.name);
        widget.show_variant(self.variant);
        widget.draw(frame, rect, theme).into()
    }

    /// Its label, icon, and padding, as the widget draws them.
    fn natural_width(&self, frame: &Frame, theme: &Theme) -> Option<f32> {
        Some(button::natural_width(frame, theme, self.label, self.icon))
    }

    fn describe(&self, widget: &button::Button, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn tips(&self, widget: &button::Button) -> Vec<(Rect, String)> {
        match (widget.drawn(), self.name) {
            (Some(rect), Some(name)) => vec![(rect, name.to_owned())],
            (None, _) | (_, None) => Vec::new(),
        }
    }

    fn act(
        &self,
        widget: &mut button::Button,
        id: FocusId,
        action: AccessAction,
    ) -> Option<ButtonResponse> {
        Some(widget.access_action(id, action))
    }
}

impl<'a, A: 'static> From<Button<'a, A>> for Element<'a, A> {
    fn from(button: Button<'a, A>) -> Self {
        Element::new(Kind::Control(Box::new(button)))
    }
}

// ========================================================================
// Checkbox Implementations
// ========================================================================

impl<'a, A> Checkbox<'a, A> {
    /// This checkbox drawn as a switch: a pill track with a knob.
    pub fn switch(self) -> Self {
        Self {
            switch: true,
            ..self
        }
    }

    /// This checkbox taking input (`true`, the default), or disabled.
    pub fn enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    /// What runs on the app's state with the new state when the user turns
    /// the checkbox on or off.
    pub fn on_change(self, apply: impl Fn(&mut A, bool) + 'static) -> Self {
        Self {
            on_change: Some(Rc::new(apply)),
            ..self
        }
    }
}

impl<A: 'static> Spec<A> for Checkbox<'_, A> {
    type Widget = checkbox::Checkbox;

    const KIND: &'static str = "checkbox";

    fn create(&self) -> checkbox::Checkbox {
        let widget = checkbox::Checkbox::new(self.label);
        match self.switch {
            true => widget.switch(),
            false => widget,
        }
    }

    fn props<'b>(&'b self, _name: Option<&'b str>) -> CheckboxProps<'b> {
        CheckboxProps {
            checked: self.checked,
            enabled: self.enabled && self.on_change.is_some(),
            label: self.label,
        }
    }

    fn call(&self, checked: bool) -> Option<Call<A>> {
        call(self.on_change.as_ref(), checked)
    }

    fn draw(
        &self,
        widget: &mut checkbox::Checkbox,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.draw(frame, rect, theme).into()
    }

    fn describe(&self, widget: &checkbox::Checkbox, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        widget: &mut checkbox::Checkbox,
        id: FocusId,
        action: AccessAction,
    ) -> Option<CheckboxResponse> {
        Some(widget.access_action(id, action))
    }
}

impl<'a, A: 'static> From<Checkbox<'a, A>> for Element<'a, A> {
    fn from(checkbox: Checkbox<'a, A>) -> Self {
        Element::new(Kind::Control(Box::new(checkbox)))
    }
}

// ========================================================================
// Radio Implementations
// ========================================================================

impl<'a, A, T> Radio<'a, A, T> {
    /// This group taking input (`true`, the default), or disabled.
    pub fn enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    /// What runs on the app's state with the chosen option's value when the
    /// user picks another option.
    pub fn on_change(self, apply: impl Fn(&mut A, T) + 'static) -> Self {
        Self {
            on_change: Some(Rc::new(apply)),
            ..self
        }
    }
}

impl<A: 'static, T: Clone + PartialEq + 'static> Spec<A> for Radio<'_, A, T> {
    type Widget = RadioGroup<T>;

    const KIND: &'static str = "radio";

    fn create(&self) -> RadioGroup<T> {
        RadioGroup::new()
    }

    fn props<'b>(&'b self, name: Option<&'b str>) -> RadioGroupProps<'b, T> {
        RadioGroupProps {
            chosen: &self.chosen,
            enabled: self.enabled && self.on_change.is_some(),
            label: name,
            options: self.options,
        }
    }

    fn call(&self, value: T) -> Option<Call<A>> {
        call(self.on_change.as_ref(), value)
    }

    fn draw(
        &self,
        widget: &mut RadioGroup<T>,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.draw(frame, rect, theme).into()
    }

    fn describe(&self, widget: &RadioGroup<T>, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        widget: &mut RadioGroup<T>,
        id: FocusId,
        action: AccessAction,
    ) -> Option<RadioGroupResponse<T>> {
        Some(widget.access_action(id, action))
    }
}

impl<'a, A: 'static, T: Clone + PartialEq + 'static> From<Radio<'a, A, T>> for Element<'a, A> {
    fn from(radio: Radio<'a, A, T>) -> Self {
        Element::new(Kind::Control(Box::new(radio)))
    }
}

// ========================================================================
// Select Implementations
// ========================================================================

impl<'a, A, T> Select<'a, A, T> {
    /// This select taking input (`true`, the default), or disabled.
    pub fn enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    /// What runs on the app's state with the chosen option's value when the
    /// user picks another option.
    pub fn on_change(self, apply: impl Fn(&mut A, T) + 'static) -> Self {
        Self {
            on_change: Some(Rc::new(apply)),
            ..self
        }
    }
}

impl<A: 'static, T: Clone + PartialEq + 'static> Spec<A> for Select<'_, A, T> {
    type Widget = select::Select<T>;

    const KIND: &'static str = "select";

    fn create(&self) -> select::Select<T> {
        select::Select::new()
    }

    fn props<'b>(&'b self, name: Option<&'b str>) -> SelectProps<'b, T> {
        SelectProps {
            chosen: &self.chosen,
            enabled: self.enabled && self.on_change.is_some(),
            label: name,
            options: match &self.options {
                Options::Lent(options) => options,
                Options::Owned(options) => options,
            },
        }
    }

    fn call(&self, value: T) -> Option<Call<A>> {
        call(self.on_change.as_ref(), value)
    }

    /// Draws the box in `rect` and, while open, the list in the frame's
    /// overlay: the popup the store traps the focus in.
    fn draw(
        &self,
        widget: &mut select::Select<T>,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        let layer = widget.draw(frame, rect, theme);
        Drawn {
            hit: rect,
            layer,
            over: None,
        }
    }

    fn describe(&self, widget: &select::Select<T>, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        widget: &mut select::Select<T>,
        id: FocusId,
        action: AccessAction,
    ) -> Option<SelectResponse<T>> {
        Some(widget.access_action(id, action))
    }

    fn has_layer(widget: &select::Select<T>) -> bool {
        widget.is_open()
    }
}

impl<'a, A: 'static, T: Clone + PartialEq + 'static> From<Select<'a, A, T>> for Element<'a, A> {
    fn from(select: Select<'a, A, T>) -> Self {
        Element::new(Kind::Control(Box::new(select)))
    }
}

// ========================================================================
// Number Implementations
// ========================================================================

impl<A> Number<A> {
    /// This control taking input (`true`, the default), or disabled.
    pub fn enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    /// What runs on the app's state with the new value when the user moves
    /// it.
    pub fn on_change(self, apply: impl Fn(&mut A, f64) + 'static) -> Self {
        Self {
            on_change: Some(Rc::new(apply)),
            ..self
        }
    }

    /// The props for its widget, named `name`.
    fn number_props<'b>(&self, name: Option<&'b str>) -> NumberProps<'b> {
        NumberProps {
            enabled: self.enabled && self.on_change.is_some(),
            label: name,
            range: self.range,
            value: self.value,
        }
    }
}

impl<A: 'static> From<Number<A>> for Element<'_, A> {
    fn from(number: Number<A>) -> Self {
        let kind = match number.look {
            NumberLook::Slider => Kind::Control(Box::new(SliderSpec(number))),
            NumberLook::Stepper => Kind::Control(Box::new(StepperSpec(number))),
        };
        Element::new(kind)
    }
}

/// A [`Number`] drawn as a slider.
struct SliderSpec<A>(Number<A>);

/// A [`Number`] drawn as a stepper.
struct StepperSpec<A>(Number<A>);

impl<A: 'static> Spec<A> for SliderSpec<A> {
    type Widget = slider::Slider;

    const KIND: &'static str = "slider";

    fn create(&self) -> slider::Slider {
        slider::Slider::new(self.0.range, self.0.value)
    }

    fn props<'b>(&'b self, name: Option<&'b str>) -> NumberProps<'b> {
        self.0.number_props(name)
    }

    fn call(&self, value: f64) -> Option<Call<A>> {
        call(self.0.on_change.as_ref(), value)
    }

    fn draw(
        &self,
        widget: &mut slider::Slider,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &slider::Slider, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        widget: &mut slider::Slider,
        id: FocusId,
        action: AccessAction,
    ) -> Option<SliderResponse> {
        Some(widget.access_action(id, action))
    }
}

impl<A: 'static> Spec<A> for StepperSpec<A> {
    type Widget = stepper::Stepper;

    const KIND: &'static str = "stepper";

    fn create(&self) -> stepper::Stepper {
        stepper::Stepper::new(self.0.range, self.0.value)
    }

    fn props<'b>(&'b self, name: Option<&'b str>) -> NumberProps<'b> {
        self.0.number_props(name)
    }

    fn call(&self, value: f64) -> Option<Call<A>> {
        call(self.0.on_change.as_ref(), value)
    }

    fn draw(
        &self,
        widget: &mut stepper::Stepper,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &stepper::Stepper, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        widget: &mut stepper::Stepper,
        id: FocusId,
        action: AccessAction,
    ) -> Option<StepperResponse> {
        Some(widget.access_action(id, action))
    }
}

// ========================================================================
// Progress Implementations
// ========================================================================

impl<A: 'static> Spec<A> for Progress {
    type Widget = ProgressBar;

    const KIND: &'static str = "progress";

    fn create(&self) -> ProgressBar {
        ProgressBar::new()
    }

    fn props<'b>(&'b self, name: Option<&'b str>) -> ProgressProps<'b> {
        ProgressProps {
            fraction: self.fraction,
            label: name,
        }
    }

    fn call(&self, value: Infallible) -> Option<Call<A>> {
        match value {}
    }

    fn draw(
        &self,
        widget: &mut ProgressBar,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &ProgressBar, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(&self, _widget: &mut ProgressBar, _id: FocusId, _action: AccessAction) -> Option<()> {
        None
    }

    fn is_focusable(&self) -> bool {
        false
    }
}

impl<A: 'static> From<Progress> for Element<'_, A> {
    fn from(progress: Progress) -> Self {
        Element::new(Kind::Control(Box::new(progress)))
    }
}

// ========================================================================
// TextField Implementations
// ========================================================================

impl<'a, A> TextField<'a, A> {
    /// This field masking its text, as a password field does, while the
    /// app builds it so: a dot for each character, nothing copied or cut
    /// to the clipboard, and a password field to screen readers, as
    /// [`TextField::with_mask`](crate::widgets::TextField::with_mask) says. The
    /// text still reaches [`TextField::on_change`].
    pub fn masked(self) -> Self {
        Self {
            masked: true,
            ..self
        }
    }

    /// This field showing `placeholder` while it is empty.
    pub fn placeholder(self, placeholder: &'a str) -> Self {
        Self {
            placeholder,
            ..self
        }
    }

    /// What runs on the app's state with the new text when the user edits
    /// it.
    pub fn on_change(self, apply: impl Fn(&mut A, String) + 'static) -> Self {
        Self {
            on_change: Some(Rc::new(apply)),
            ..self
        }
    }

    /// What runs on the app's state when the user presses Enter (no
    /// modifiers) in the field: run a search, accept a page number. The
    /// field takes the key, so it reaches no other element nor
    /// [`App::key`](crate::App::key); the text is the app's already,
    /// stored by [`TextField::on_change`]. Without it, Enter goes on as
    /// any key the field leaves.
    pub fn on_submit(self, apply: impl Fn(&mut A) + 'static) -> Self {
        Self {
            on_submit: Some(Rc::new(move |app: &mut A, ()| apply(app))),
            ..self
        }
    }

    /// What runs on the app's state when the user presses Esc (no
    /// modifiers) in the field: close a find bar, give up an edit. The
    /// field takes the key, as [`TextField::on_submit`] takes Enter;
    /// without it, Esc goes on (a modal layer's or the app's).
    pub fn on_cancel(self, apply: impl Fn(&mut A) + 'static) -> Self {
        Self {
            on_cancel: Some(Rc::new(move |app: &mut A, ()| apply(app))),
            ..self
        }
    }
}

impl<A: 'static> Spec<A> for TextField<'_, A> {
    type Widget = text_field::TextField;

    const KIND: &'static str = "text_field";

    fn create(&self) -> text_field::TextField {
        let mut field = text_field::TextField::new().with_frame();
        field.set_masked(self.masked);
        field
    }

    fn props<'b>(&'b self, name: Option<&'b str>) -> TextFieldProps<'b> {
        TextFieldProps {
            label: name,
            placeholder: self.placeholder,
            text: self.text,
        }
    }

    fn call(&self, text: String) -> Option<Call<A>> {
        call(self.on_change.as_ref(), text)
    }

    fn draw(
        &self,
        widget: &mut text_field::TextField,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        // Copied in each frame rather than through the props, whose
        // fields apps build as literals.
        widget.set_masked(self.masked);
        widget.draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &text_field::TextField, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        _widget: &mut text_field::TextField,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<TextFieldResponse> {
        None
    }

    fn caret(&self, widget: &text_field::TextField) -> Option<Rect> {
        widget.cursor_area()
    }

    /// Enter with `on_submit`, and Esc with `on_cancel`.
    fn unused(&self, _widget: &text_field::TextField, input: &Input) -> Option<Call<A>> {
        let apply = match input.key()? {
            key if key == Key::new(KeyCode::Enter, Mods::NONE) => self.on_submit.as_ref(),
            key if key == Key::new(KeyCode::Esc, Mods::NONE) => self.on_cancel.as_ref(),
            _ => None,
        };
        call(apply, ())
    }
}

impl<'a, A: 'static> From<TextField<'a, A>> for Element<'a, A> {
    fn from(field: TextField<'a, A>) -> Self {
        Element::new(Kind::Control(Box::new(field)))
    }
}

// ========================================================================
// TextArea Implementations
// ========================================================================

impl<'a, A> TextArea<'a, A> {
    /// This area showing `placeholder` while it is empty.
    pub fn placeholder(self, placeholder: &'a str) -> Self {
        Self {
            placeholder,
            ..self
        }
    }

    /// This area `rows` rows tall, scrolling a longer text, rather than
    /// growing with its text.
    pub fn rows(self, rows: usize) -> Self {
        Self {
            rows: Some(rows),
            ..self
        }
    }

    /// What runs on the app's state with the new text when the user edits
    /// it. The callback stores the text and changes the version the area
    /// was built with, as every change of the text must.
    pub fn on_change(self, apply: impl Fn(&mut A, String) + 'static) -> Self {
        Self {
            on_change: Some(Rc::new(apply)),
            ..self
        }
    }
}

impl<A: 'static> Spec<A> for TextArea<'_, A> {
    type Widget = text_area::TextArea;

    const KIND: &'static str = "text_area";

    fn create(&self) -> text_area::TextArea {
        text_area::TextArea::new().with_frame()
    }

    fn props<'b>(&'b self, name: Option<&'b str>) -> TextAreaProps<'b> {
        TextAreaProps {
            label: name,
            placeholder: self.placeholder,
            text: self.text,
            version: self.version,
        }
    }

    fn call(&self, text: String) -> Option<Call<A>> {
        call(self.on_change.as_ref(), text)
    }

    fn draw(
        &self,
        widget: &mut text_area::TextArea,
        frame: &mut Frame,
        rect: Rect,
        theme: &Theme,
    ) -> Drawn {
        widget.draw(frame, rect, theme);
        rect.into()
    }

    fn describe(&self, widget: &text_area::TextArea, frame: &mut Frame, id: FocusId) {
        widget.access(frame, id);
    }

    fn act(
        &self,
        _widget: &mut text_area::TextArea,
        _id: FocusId,
        _action: AccessAction,
    ) -> Option<TextAreaResponse> {
        None
    }

    fn fixed_height(&self, metrics: CellMetrics) -> Option<f32> {
        self.rows.map(|rows| (rows as f32 * metrics.height).round())
    }

    fn caret(&self, widget: &text_area::TextArea) -> Option<Rect> {
        widget.cursor_area()
    }
}

impl<'a, A: 'static> From<TextArea<'a, A>> for Element<'a, A> {
    fn from(area: TextArea<'a, A>) -> Self {
        Element::new(Kind::Control(Box::new(area)))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A push button saying `label`; [`Button::toggle`] makes it a toggle.
/// In a row it is as wide as its label, icon, and padding, so a row of
/// buttons packs to their sizes; [`Element::grow`] or [`Element::width`]
/// gives it a share of the row or a width instead, its face still drawn
/// at the left of that room.
pub fn button<'a, A>(label: &'a str) -> Button<'a, A> {
    Button {
        active: None,
        enabled: true,
        icon: None,
        label,
        name: None,
        on_click: None,
        variant: ButtonVariant::Default,
    }
}

/// A push button showing only `icon`, a one-color SVG drawn a row square
/// in the theme's text color, called `name`: the tooltip shown while the
/// pointer rests on it and its name for screen readers, which an icon
/// alone cannot give. As square as it is tall; [`Button::toggle`] makes it
/// a toggle.
pub fn icon_button<'a, A>(icon: &'a Svg, name: &'a str) -> Button<'a, A> {
    Button {
        icon: Some(icon),
        name: Some(name),
        ..button("")
    }
}

/// A checkbox saying `label`, on while `checked`.
pub fn checkbox<'a, A>(label: &'a str, checked: bool) -> Checkbox<'a, A> {
    Checkbox {
        checked,
        enabled: true,
        label,
        on_change: None,
        switch: false,
    }
}

/// A radio group of `options`, each a value and what it says, top to
/// bottom; the option whose value is `chosen` is checked (a value no option
/// has leaves none checked).
pub fn radio<'a, A, T: Clone>(options: &'a [(T, &'a str)], chosen: &T) -> Radio<'a, A, T> {
    Radio {
        chosen: chosen.clone(),
        enabled: true,
        on_change: None,
        options,
    }
}

/// A dropdown of `options`, each a value and what it says, top to bottom,
/// showing the option whose value is `chosen` (the first when none has
/// it). Enter, Space, or a click opens its list over the window, which
/// keeps the keyboard until Esc, a choice, or a click outside closes it,
/// and the focus goes back to the box.
pub fn select<'a, A, T: Clone>(options: &'a [(T, &'a str)], chosen: &T) -> Select<'a, A, T> {
    Select {
        chosen: chosen.clone(),
        enabled: true,
        on_change: None,
        options: Options::Lent(options),
    }
}

/// A dropdown as [`select`] makes, of `options` built for the view (what
/// the app holds paired with its values) rather than lent.
pub fn select_owned<'a, A, T: Clone>(options: Vec<(T, &'a str)>, chosen: &T) -> Select<'a, A, T> {
    Select {
        options: Options::Owned(options),
        ..select(&[], chosen)
    }
}

/// A slider picking a number in `range` along a track, at `value`.
pub fn slider<A>(range: NumberRange, value: f64) -> Number<A> {
    number(NumberLook::Slider, range, value)
}

/// A stepper stepping a number in `range` with minus and plus buttons, at
/// `value`.
pub fn stepper<A>(range: NumberRange, value: f64) -> Number<A> {
    number(NumberLook::Stepper, range, value)
}

/// A progress bar `fraction` (0 to 1) of the way along, or sweeping when
/// `fraction` is `None`, for a task of unknown length.
pub fn progress(fraction: Option<f32>) -> Progress {
    Progress { fraction }
}

/// A framed one-line text input holding `text`.
pub fn text_field<'a, A>(text: &'a str) -> TextField<'a, A> {
    TextField {
        masked: false,
        on_cancel: None,
        on_change: None,
        on_submit: None,
        placeholder: "",
        text,
    }
}

/// A framed text input of several lines holding `text`, at `version`: a
/// number the app changes whenever it changes the text, so a rebuild
/// compares the number rather than the whole text.
pub fn text_area<'a, A>(text: &'a str, version: u64) -> TextArea<'a, A> {
    TextArea {
        on_change: None,
        placeholder: "",
        rows: None,
        text,
        version,
    }
}

/// A number element of `look`.
fn number<A>(look: NumberLook, range: NumberRange, value: f64) -> Number<A> {
    Number {
        enabled: true,
        look,
        on_change: None,
        range,
        value,
    }
}

/// The callback call of `apply`, if any, with `value`.
pub(crate) fn call<A: 'static, V: 'static>(
    apply: Option<&Callback<A, V>>,
    value: V,
) -> Option<Call<A>> {
    let apply = Rc::clone(apply?);
    Some(Box::new(move |app: &mut A| apply(app, value)))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TreeWindow;
    use crate::view::element::column;
    use crate::{App, AppCx, WindowId};

    /// An app with a find field and a button after it, recording what
    /// reaches it.
    struct Finder {
        /// Whether the field takes Esc.
        cancels: bool,
        /// How many times the field was cancelled.
        cancelled: usize,
        /// How many times the button was clicked.
        clicks: usize,
        /// The keys no element took.
        keys: Vec<Key>,
        /// The field's text.
        query: String,
        /// The queries submitted.
        submitted: Vec<String>,
        /// The colors it is drawn in.
        theme: Theme,
    }

    impl App for Finder {
        fn view(&self, _window: WindowId) -> Element<'_, Self> {
            let field = text_field(&self.query)
                .on_change(|app: &mut Self, query| app.query = query)
                .on_submit(|app: &mut Self| app.submitted.push(app.query.clone()));
            let field = match self.cancels {
                true => field.on_cancel(|app: &mut Self| app.cancelled += 1),
                false => field,
            };
            column([
                field.into(),
                button("Next")
                    .on_click(|app: &mut Self| app.clicks += 1)
                    .into(),
            ])
        }

        fn key(&mut self, key: Key, _cx: &mut AppCx<'_>) -> bool {
            self.keys.push(key);
            true
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    /// A window showing a finder whose field takes Esc when `cancels`.
    fn window(cancels: bool) -> TreeWindow<Finder> {
        let finder = Finder {
            cancels,
            cancelled: 0,
            clicks: 0,
            keys: Vec::new(),
            query: String::new(),
            submitted: Vec::new(),
            theme: Theme::dark(),
        };
        TreeWindow::new(finder, "t", 400, 200)
    }

    #[test]
    fn test_enter_in_a_field_submits_it_and_esc_cancels_it_when_asked() {
        let mut window = window(true);
        window.type_text("pdf");
        window.press("<cr> <esc>");
        let app = window.app();
        assert_eq!(app.submitted, ["pdf"]);
        assert_eq!(app.cancelled, 1);
        assert!(app.keys.is_empty(), "the field took both keys");
        assert_eq!(app.clicks, 0);
        // Shift+Enter is not a submit; it goes on as before.
        window.press("S-<cr>");
        assert_eq!(window.app().submitted.len(), 1);
        assert_eq!(window.app().keys.len(), 1);
    }

    #[test]
    fn test_esc_in_a_field_without_on_cancel_goes_on_to_the_app() {
        let mut window = window(false);
        window.press("<esc>");
        assert_eq!(window.app().keys, [Key::new(KeyCode::Esc, Mods::NONE)]);
    }
}
