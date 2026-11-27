//! [`Stepper`]: a number shown between a minus and a plus button, for a
//! count, a tab width, or a font size. It keeps its value inside a
//! [`NumberRange`], snapped to the range's steps, and writes it with as
//! many decimals as the step has.
//!
//! Keys, while the app says the stepper is focused: Up and `k` raise the
//! value one step, Down and `j` lower it; PageUp and PageDown move it ten
//! steps; Home and End set the smallest and largest value. A click on the
//! minus or plus button steps it; a button is drawn muted at its end of
//! the range. The number is not typed: an app that wants a typed number
//! uses a [`TextField`](crate::widgets::TextField). A disabled stepper is drawn
//! muted and takes no input.

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::Border;
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::strings::Strings;
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::FOCUS_STROKE;
use super::label::{draw_label, label_width};
use super::number_range::{NumberRange, PAGE_STEPS};
use super::slider::NumberProps;

// ========================================================================
// Constants
// ========================================================================

/// The child index of the minus button's accessibility node.
const DECREMENT_NODE: u64 = 0;

/// The child index of the plus button's accessibility node.
const INCREMENT_NODE: u64 = 1;

/// What the minus button shows.
const DECREMENT_GLYPH: &str = "-";

/// What the plus button shows.
const INCREMENT_GLYPH: &str = "+";

/// How wide each button is, in cells.
const BUTTON_CELLS: f32 = 3.0;

/// The blank cells on each side of the number.
const VALUE_PADDING_CELLS: f32 = 1.0;

/// The space above and below the number inside the box, in logical pixels.
const PADDING_Y: f32 = 3.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A number stepped with a minus and a plus button. The app owns it, hands
/// it the keys while it has the focus and the pointer input over it, and
/// draws it each frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Stepper {
    /// Whether it takes input.
    enabled: bool,
    /// Whether the app said it has the keyboard.
    focused: bool,
    /// The button under the pointer, or `None`.
    hovered: Option<StepButton>,
    /// The name drawn at its left, or `None`.
    label: Option<String>,
    /// Where it was drawn last, for the pointer.
    layout: Option<StepperLayout>,
    /// The values it takes.
    range: NumberRange,
    /// The words of the theme last drawn in, for the buttons' names.
    strings: Strings,
    /// The value now, inside the range.
    value: f64,
}

/// What became of an input offered to a [`Stepper`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum StepperResponse {
    /// The value changed to this.
    Changed(f64),
    /// The stepper used the input; the value stayed.
    Handled,
    /// The input is not the stepper's.
    Ignored,
}

/// One of a stepper's buttons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepButton {
    /// The minus button, at the left.
    Decrement,
    /// The plus button, at the right.
    Increment,
}

/// Where a stepper's parts were drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StepperLayout {
    /// The whole stepper, label included.
    area: Rect,
    /// The minus button.
    decrement: Rect,
    /// The plus button.
    increment: Rect,
}

// ========================================================================
// Stepper Implementations
// ========================================================================

impl Stepper {
    /// An enabled stepper over `range` at `value` (snapped to the range),
    /// without a label.
    pub fn new(range: NumberRange, value: f64) -> Self {
        Self {
            enabled: true,
            focused: false,
            hovered: None,
            label: None,
            layout: None,
            range,
            strings: Strings::english(),
            value: range.snap(value),
        }
    }

    /// This stepper named `label` for screen readers (the stepper does not
    /// draw it; the app draws its own label beside it, as for every
    /// control but a checkbox or a radio button).
    pub fn with_label(self, label: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            ..self
        }
    }

    /// This stepper taking input (`true`, the default) or drawn muted and
    /// taking none.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.set_enabled(enabled);
        self
    }

    /// Lets the stepper take input (`true`) or draws it muted and has it
    /// take none.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.hovered = None;
        }
    }

    /// Whether the stepper takes input.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The value, inside the range.
    pub fn value(&self) -> f64 {
        self.value
    }

    /// Sets the value to `value`, snapped to the range.
    pub fn set_value(&mut self, value: f64) {
        self.value = self.range.snap(value);
    }

    /// How tall it is drawn: a row, plus its box's border and padding.
    fn box_height(metrics: CellMetrics, scale_factor: f64) -> f32 {
        let frame = logical(FOCUS_STROKE, scale_factor) + logical(PADDING_Y, scale_factor);
        (metrics.height + 2.0 * frame).round()
    }

    /// Describes the stepper as last drawn to screen readers as `id`: a
    /// spin button named by its label, with its value, range, and step,
    /// that steps up and down, holding its minus (child 0) and plus
    /// (child 1) buttons, each clickable. Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = self.layout else {
            return;
        };
        let mut node = Node::new(Role::SpinButton);
        if let Some(label) = &self.label {
            node.set_label(label.as_str());
        }
        node.set_numeric_value(self.value);
        node.set_min_numeric_value(self.range.min);
        node.set_max_numeric_value(self.range.max);
        node.set_numeric_value_step(self.range.step);
        node.set_value(self.range.format(self.value));
        match self.enabled {
            true => {
                node.add_action(Action::Increment);
                node.add_action(Action::Decrement);
            }
            false => node.set_disabled(),
        }
        frame.enter_access(id, layout.area, node);
        let buttons = [
            (DECREMENT_NODE, &self.strings.decrease, layout.decrement),
            (INCREMENT_NODE, &self.strings.increase, layout.increment),
        ];
        for (child, label, rect) in buttons {
            let mut button = Node::new(Role::Button);
            button.set_label(label.as_ref());
            match self.enabled {
                true => button.add_action(Action::Click),
                false => button.set_disabled(),
            }
            frame.access(id.child(child), rect, button);
        }
        frame.leave_access();
    }

    /// Does what a screen reader's `action` on the stepper described as
    /// `id` asks: a click on the minus or plus button, or an increment or
    /// decrement of the stepper, moves the value one step. Ignored for
    /// other actions and nodes, and while disabled.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> StepperResponse {
        if !self.enabled {
            return StepperResponse::Ignored;
        }
        match action {
            AccessAction::Click(target) if target == id.child(DECREMENT_NODE) => self.step(-1.0),
            AccessAction::Click(target) if target == id.child(INCREMENT_NODE) => self.step(1.0),
            AccessAction::Decrement(target) if target == id => self.step(-1.0),
            AccessAction::Increment(target) if target == id => self.step(1.0),
            AccessAction::Click(_)
            | AccessAction::Decrement(_)
            | AccessAction::Focus(_)
            | AccessAction::Increment(_) => StepperResponse::Ignored,
        }
    }

    /// Reacts to `key`, while focused and enabled.
    fn handle_key(&mut self, key: Key, vim: bool) -> StepperResponse {
        if key.mods != Mods::NONE {
            return StepperResponse::Ignored;
        }
        match key.code {
            KeyCode::Up => self.step(1.0),
            KeyCode::Char('k') if vim => self.step(1.0),
            KeyCode::Down => self.step(-1.0),
            KeyCode::Char('j') if vim => self.step(-1.0),
            KeyCode::PageUp => self.step(PAGE_STEPS),
            KeyCode::PageDown => self.step(-PAGE_STEPS),
            KeyCode::Home => self.change(self.range.min),
            KeyCode::End => self.change(self.range.max),
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Tab => StepperResponse::Ignored,
        }
    }

    /// Reacts to pointer `input`: a click on a button steps the value, a
    /// click elsewhere on the stepper is used without a change, and moves
    /// follow which button is under the pointer.
    fn handle_pointer(&mut self, input: &Input) -> StepperResponse {
        let Some(layout) = self.layout else {
            return StepperResponse::Ignored;
        };
        if let Some(press) = input.press(PointerButton::Left) {
            return match self.button_at(layout, press.position) {
                Some(StepButton::Decrement) => self.step(-1.0),
                Some(StepButton::Increment) => self.step(1.0),
                None if layout.area.contains(press.position) => StepperResponse::Handled,
                None => StepperResponse::Ignored,
            };
        }
        let hovered = match input {
            Input::PointerMoved(position) => self.button_at(layout, *position),
            Input::PointerLeft => None,
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_)
            | Input::PointerButton(_)
            | Input::Wheel(_) => return StepperResponse::Ignored,
        };
        match hovered == self.hovered {
            true => StepperResponse::Ignored,
            false => {
                self.hovered = hovered;
                StepperResponse::Handled
            }
        }
    }

    /// The button at `point` in `layout`, or `None`.
    fn button_at(&self, layout: StepperLayout, point: Point) -> Option<StepButton> {
        if layout.decrement.contains(point) {
            return Some(StepButton::Decrement);
        }
        layout
            .increment
            .contains(point)
            .then_some(StepButton::Increment)
    }

    /// Whether `button` can still move the value.
    fn can_step(&self, button: StepButton) -> bool {
        match button {
            StepButton::Decrement => self.value > self.range.min,
            StepButton::Increment => self.value < self.range.max,
        }
    }

    /// Moves the value `steps` steps (negative for down).
    fn step(&mut self, steps: f64) -> StepperResponse {
        self.change(self.range.stepped(self.value, steps))
    }

    /// Sets the value to `value` (snapped), saying whether it changed.
    fn change(&mut self, value: f64) -> StepperResponse {
        let value = self.range.snap(value);
        if value == self.value {
            return StepperResponse::Handled;
        }
        self.value = value;
        StepperResponse::Changed(value)
    }

    /// Draws `button` in `rect`: its glyph centered, on the hover color
    /// while the pointer is over it, muted when it cannot step.
    fn draw_button(&self, frame: &mut Frame, button: StepButton, rect: Rect, theme: &Theme) {
        let usable = self.enabled && self.can_step(button);
        if usable && self.hovered == Some(button) {
            let style = theme.shape.highlight(theme.hover, frame.scale_factor());
            frame.draw_box(rect, &style);
        }
        let color = match usable {
            true => theme.foreground,
            false => theme.muted,
        };
        let glyph = match button {
            StepButton::Decrement => DECREMENT_GLYPH,
            StepButton::Increment => INCREMENT_GLYPH,
        };
        draw_centered(frame, theme, glyph, rect, color);
    }
}

impl Widget for Stepper {
    type Output = ();
    type Response = StepperResponse;

    /// Reacts to `input`: keys as the module documentation lists, while
    /// focused; a click on a button steps the value. A disabled stepper
    /// ignores everything. Asks for a redraw whenever it used the input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> StepperResponse {
        if !self.enabled {
            return StepperResponse::Ignored;
        }
        let response = match input {
            Input::Key(key) if self.focused => self.handle_key(*key, cx.key_scheme().vim),
            Input::Key(_) => StepperResponse::Ignored,
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => self.handle_pointer(input),
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the stepper at the left of `area`, centered on its height: a box
    /// holding the minus button, the number, and the plus
    /// button, as wide as the range's widest number. The box's border is the
    /// accent while focused.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if self.strings != theme.strings {
            self.strings = theme.strings.clone();
        }
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let height = Self::box_height(metrics, scale_factor).min(area.height);
        let top = (area.y + (area.height - height) / 2.0).round();
        let text = match self.enabled {
            true => theme.foreground,
            false => theme.muted,
        };
        let left = area.x;
        let widest = [self.range.min, self.range.max]
            .map(|number| label_width(frame, theme, &self.range.format(number)))
            .into_iter()
            .fold(0.0, f32::max);
        let button = (BUTTON_CELLS * metrics.width).round();
        let number = (widest + 2.0 * VALUE_PADDING_CELLS * metrics.width).ceil();
        let boxed = Rect::new(left, top, 2.0 * button + number, height);
        let (border, width) = match self.focused && self.enabled {
            true => (theme.accent, logical(FOCUS_STROKE, scale_factor)),
            false => (theme.border, theme.shape.border(scale_factor)),
        };
        let mut style = theme.shape.flat_box(theme.background, border, scale_factor);
        style.border = (width > 0.0).then_some(Border {
            color: Rgba::from(border),
            width,
        });
        frame.draw_box(boxed, &style);
        let inset = theme.shape.content_inset(scale_factor).max(width);
        let inner = |x: f32, width: f32| Rect::new(x, top + inset, width, height - 2.0 * inset);
        let decrement = inner(boxed.x + inset, button - inset);
        let increment = inner(boxed.x + button + number, button - inset);
        self.draw_button(frame, StepButton::Decrement, decrement, theme);
        self.draw_button(frame, StepButton::Increment, increment, theme);
        let value = inner(boxed.x + button, number);
        draw_centered(frame, theme, &self.range.format(self.value), value, text);
        self.layout = Some(StepperLayout {
            area: Rect::new(area.x, top, boxed.x + boxed.width - area.x, height),
            decrement,
            increment,
        });
    }

    /// Tells the stepper whether it has the keyboard: it takes keys and
    /// draws its border in the accent only while it does.
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Whether the app said the stepper has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl Control for Stepper {
    type Props<'a> = NumberProps<'a>;
    type Value = f64;

    /// Takes the range, the value (snapped to the range), whether it is
    /// enabled, and its name from `props`.
    fn sync(&mut self, props: NumberProps<'_>) {
        self.range = props.range;
        self.set_value(props.value);
        if self.label.as_deref() != props.label {
            self.label = props.label.map(str::to_owned);
        }
        self.set_enabled(props.enabled);
    }

    /// A row, plus its box's border and padding, whatever the width.
    fn measure(&self, _width: f32, metrics: CellMetrics, scale_factor: f64) -> f32 {
        Self::box_height(metrics, scale_factor)
    }

    /// The value the user stepped it to.
    fn changed(&self, response: StepperResponse) -> Option<f64> {
        match response {
            StepperResponse::Changed(value) => Some(value),
            StepperResponse::Handled | StepperResponse::Ignored => None,
        }
    }
}

impl Outcome for StepperResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Draws `text` as a label in `color` centered in `rect`.
fn draw_centered(frame: &mut Frame, theme: &Theme, text: &str, rect: Rect, color: Rgb) {
    let height = frame.cell_metrics().height;
    let width = label_width(frame, theme, text);
    let origin = Point::new(
        (rect.x + (rect.width - width) / 2.0).round(),
        (rect.y + (rect.height - height) / 2.0).round(),
    );
    draw_label(frame, theme, text, origin, color);
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::offscreen;
    use crate::base::key::parse_key;
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, drawn_text, ui_text};

    const TAB_WIDTH: NumberRange = NumberRange::new(1.0, 16.0, 1.0);

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    fn click(window: &mut TestWindow, stepper: &mut Stepper, rect: Rect) -> StepperResponse {
        let center = Point::new(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let press = ButtonInput::press(PointerButton::Left, center);
        window.feed(stepper, &Input::PointerButton(press))
    }

    #[test]
    fn test_keys_step_page_and_jump_only_while_focused_and_stop_at_the_ends() {
        let mut window = TestWindow::keyboard_first(300, 40);
        let mut stepper = Stepper::new(TAB_WIDTH, 4.0);
        assert_eq!(
            window.feed(&mut stepper, &key("k")),
            StepperResponse::Ignored
        );
        stepper.set_focused(true);
        assert_eq!(
            window.feed(&mut stepper, &key("k")),
            StepperResponse::Changed(5.0)
        );
        assert!(window.take_redraw());
        assert_eq!(
            window.feed(&mut stepper, &key("<pageup>")),
            StepperResponse::Changed(15.0)
        );
        assert_eq!(
            window.feed(&mut stepper, &key("<pageup>")),
            StepperResponse::Changed(16.0),
            "held to the range"
        );
        assert_eq!(
            window.feed(&mut stepper, &key("<up>")),
            StepperResponse::Handled
        );
        assert_eq!(
            window.feed(&mut stepper, &key("<home>")),
            StepperResponse::Changed(1.0)
        );
        assert_eq!(
            window.feed(&mut stepper, &key("<left>")),
            StepperResponse::Ignored
        );
    }

    #[test]
    fn test_clicks_on_the_buttons_step_and_moves_light_the_button_under_the_pointer() {
        let mut window = TestWindow::keyboard_first(300, 40);
        let mut stepper = Stepper::new(TAB_WIDTH, 4.0).with_label("Tab width");
        let mut frame = window.frame();
        let bounds = frame.bounds();
        stepper.draw(&mut frame, bounds, &Theme::dark());
        assert_eq!(
            drawn_text(&frame),
            ["-", "+", "4"],
            "the label is the app's to draw"
        );
        let layout = stepper.layout.expect("drawn");
        assert_eq!(
            click(&mut window, &mut stepper, layout.increment),
            StepperResponse::Changed(5.0)
        );
        assert_eq!(
            click(&mut window, &mut stepper, layout.decrement),
            StepperResponse::Changed(4.0)
        );
        let over = Point::new(layout.increment.x + 1.0, layout.increment.y + 1.0);
        assert_eq!(
            window.feed(&mut stepper, &Input::PointerMoved(over)),
            StepperResponse::Handled
        );
        assert_eq!(stepper.hovered, Some(StepButton::Increment));
        assert_eq!(
            window.feed(&mut stepper, &Input::PointerMoved(over)),
            StepperResponse::Ignored,
            "no change, no redraw"
        );
        let beyond = Rect::new(layout.area.x + layout.area.width + 10.0, 0.0, 4.0, 4.0);
        assert_eq!(
            click(&mut window, &mut stepper, beyond),
            StepperResponse::Ignored
        );
    }

    #[test]
    fn test_the_buttons_take_their_names_from_the_theme_last_drawn_in() {
        let window = TestWindow::keyboard_first(300, 40);
        let theme = Theme {
            strings: Strings {
                decrease: "Weniger".into(),
                increase: "Mehr".into(),
                ..Strings::english()
            },
            ..Theme::dark()
        };
        let mut stepper = Stepper::new(TAB_WIDTH, 4.0);
        let mut frame = window.frame();
        let bounds = frame.bounds();
        stepper.draw(&mut frame, bounds, &theme);
        stepper.access(&mut frame, FocusId::new("tabs"));
        let text = ui_text(&frame, "t");
        assert!(text.contains("Button \"Weniger\"\n"), "{text}");
        assert!(text.contains("Button \"Mehr\"\n"), "{text}");
    }

    #[test]
    fn test_the_stepper_describes_its_value_and_buttons_for_a_screen_reader() {
        let window = TestWindow::keyboard_first(300, 40);
        let id = FocusId::new("size");
        let mut stepper =
            Stepper::new(NumberRange::new(8.0, 32.0, 0.5), 12.0).with_label("Font size");
        let mut frame = window.frame();
        let bounds = frame.bounds();
        stepper.draw(&mut frame, bounds, &Theme::dark());
        stepper.access(&mut frame, id);
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\" focused\n\
             \x20 SpinButton \"Font size\" value=\"12.0\" value=12\n\
             \x20   Button \"Decrease\"\n\
             \x20   Button \"Increase\"\n"
        );
        assert_eq!(
            stepper.access_action(id, AccessAction::Click(id.child(INCREMENT_NODE))),
            StepperResponse::Changed(12.5)
        );
        assert_eq!(
            stepper.access_action(id, AccessAction::Decrement(id)),
            StepperResponse::Changed(12.0)
        );
        assert_eq!(
            stepper.access_action(id, AccessAction::Click(id)),
            StepperResponse::Ignored
        );
    }

    #[test]
    fn test_the_focused_box_is_lined_in_the_accent_and_the_hovered_button_lit() {
        let theme = Theme::dark();
        let mut stepper = Stepper::new(TAB_WIDTH, 4.0);
        stepper.set_focused(true);
        stepper.hovered = Some(StepButton::Increment);
        let Some(snapshot) = offscreen::render(300, 40, |frame| {
            frame.clear(Rgba::from(theme.background));
            let bounds = frame.bounds();
            stepper.draw(frame, bounds, &theme);
        }) else {
            return;
        };
        let layout = stepper.layout.expect("drawn");
        let middle = layout.area.y + layout.area.height / 2.0;
        snapshot.assert_pixel(layout.area.x, middle, theme.accent, "the focus border");
        let plus = layout.increment;
        snapshot.assert_pixel(plus.x + 1.0, plus.y + 1.0, theme.hover, "hovered");
        let minus = layout.decrement;
        snapshot.assert_pixel(
            minus.x + 1.0,
            minus.y + 1.0,
            theme.background,
            "not hovered",
        );
    }
}
