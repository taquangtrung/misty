//! [`Slider`]: a number picked along a horizontal track, for a volume, a
//! zoom level, or a font size. It keeps its value inside a
//! [`NumberRange`], snapped to the range's steps.
//!
//! Keys, while the app says the slider is focused: Left, Down, and `h`
//! lower the value one step; Right, Up, and `l` raise it one step;
//! PageDown and PageUp move it ten steps; Home and End set the smallest
//! and largest value. A press on the track jumps the knob there and drags
//! it until the button is released. A disabled slider is drawn muted and
//! takes no input.

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::control::Control;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::BoxStyle;
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::number_range::{NumberRange, PAGE_STEPS};

// ========================================================================
// Constants
// ========================================================================

/// The thickness of the track, in logical pixels.
const TRACK_THICKNESS: f32 = 4.0;

/// The width and height of the knob, in logical pixels (at most a row).
const KNOB_SIZE: f32 = 12.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A number picked along a horizontal track. The app owns it, hands it the
/// keys while it has the focus and the pointer input over it, and draws it
/// each frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Slider {
    /// Whether a press on the track is being dragged.
    dragging: bool,
    /// Whether it takes input.
    enabled: bool,
    /// Whether the app said it has the keyboard.
    focused: bool,
    /// The name drawn at its left, or `None`.
    label: Option<String>,
    /// Where it was drawn last, for the pointer.
    layout: Option<SliderLayout>,
    /// The values it takes.
    range: NumberRange,
    /// The value now, inside the range.
    value: f64,
}

/// What became of an input offered to a [`Slider`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum SliderResponse {
    /// The value changed to this.
    Changed(f64),
    /// The slider used the input; the value stayed.
    Handled,
    /// The input is not the slider's.
    Ignored,
}

/// What the app's state says a [`Slider`] or a [`Stepper`](crate::widgets::Stepper)
/// shows, for [`Control::sync`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberProps<'a> {
    /// Whether it takes input.
    pub enabled: bool,
    /// The name screen readers give it, if any.
    pub label: Option<&'a str>,
    /// The values it takes.
    pub range: NumberRange,
    /// The value, snapped to the range.
    pub value: f64,
}

/// Where a slider's parts were drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SliderLayout {
    /// The whole slider, label included.
    area: Rect,
    /// The line the knob's center moves along, smallest value at the left.
    track: Rect,
}

// ========================================================================
// Slider Implementations
// ========================================================================

impl Slider {
    /// An enabled slider over `range` at `value` (snapped to the range),
    /// without a label.
    pub fn new(range: NumberRange, value: f64) -> Self {
        Self {
            dragging: false,
            enabled: true,
            focused: false,
            label: None,
            layout: None,
            range,
            value: range.snap(value),
        }
    }

    /// This slider named `label` for screen readers (the slider does not
    /// draw it; the app draws its own label beside it, as for every
    /// control but a checkbox or a radio button).
    pub fn with_label(self, label: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            ..self
        }
    }

    /// This slider taking input (`true`, the default) or drawn muted and
    /// taking none.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.set_enabled(enabled);
        self
    }

    /// Lets the slider take input (`true`) or draws it muted and has it take
    /// none, ending a drag.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.dragging &= enabled;
    }

    /// Whether the slider takes input.
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

    /// Describes the slider as last drawn to screen readers as `id`: a
    /// slider named by its label, with its value, range, and step, that
    /// steps up and down. Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = self.layout else {
            return;
        };
        let mut node = Node::new(Role::Slider);
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
        frame.access(id, layout.area, node);
    }

    /// Does what a screen reader's `action` on the slider described as `id`
    /// asks: an increment or decrement moves the value one step. Ignored
    /// for other actions and nodes, and while disabled.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> SliderResponse {
        if !self.enabled {
            return SliderResponse::Ignored;
        }
        match action {
            AccessAction::Increment(target) if target == id => self.step(1.0),
            AccessAction::Decrement(target) if target == id => self.step(-1.0),
            AccessAction::Click(_)
            | AccessAction::Decrement(_)
            | AccessAction::Focus(_)
            | AccessAction::Increment(_) => SliderResponse::Ignored,
        }
    }

    /// Reacts to `key`, while focused and enabled.
    fn handle_key(&mut self, key: Key) -> SliderResponse {
        if key.mods != Mods::NONE {
            return SliderResponse::Ignored;
        }
        match key.code {
            KeyCode::Left | KeyCode::Down | KeyCode::Char('h') => self.step(-1.0),
            KeyCode::Right | KeyCode::Up | KeyCode::Char('l') => self.step(1.0),
            KeyCode::PageDown => self.step(-PAGE_STEPS),
            KeyCode::PageUp => self.step(PAGE_STEPS),
            KeyCode::Home => self.change(self.range.min),
            KeyCode::End => self.change(self.range.max),
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Insert
            | KeyCode::Tab => SliderResponse::Ignored,
        }
    }

    /// Reacts to pointer `input`: a press on the slider jumps the knob
    /// under the pointer and starts a drag, which moves follow until the
    /// release.
    fn handle_pointer(&mut self, input: &Input) -> SliderResponse {
        let Some(layout) = self.layout else {
            return SliderResponse::Ignored;
        };
        if let Some(press) = input.press(PointerButton::Left) {
            if !layout.area.contains(press.position) {
                return SliderResponse::Ignored;
            }
            self.dragging = true;
            return self.change(self.value_at(layout.track, press.position));
        }
        if input.release(PointerButton::Left).is_some() && self.dragging {
            self.dragging = false;
            return SliderResponse::Handled;
        }
        match input {
            Input::PointerMoved(position) if self.dragging => {
                self.change(self.value_at(layout.track, *position))
            }
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
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => SliderResponse::Ignored,
        }
    }

    /// Moves the value `steps` steps (negative for down).
    fn step(&mut self, steps: f64) -> SliderResponse {
        self.change(self.range.stepped(self.value, steps))
    }

    /// Sets the value to `value` (snapped), saying whether it changed.
    fn change(&mut self, value: f64) -> SliderResponse {
        let value = self.range.snap(value);
        if value == self.value {
            return SliderResponse::Handled;
        }
        self.value = value;
        SliderResponse::Changed(value)
    }

    /// The value at `point` along `track`, snapped.
    fn value_at(&self, track: Rect, point: Point) -> f64 {
        let share = match track.width > 0.0 {
            true => ((point.x - track.x) / track.width).clamp(0.0, 1.0),
            false => 0.0,
        };
        let range = self.range;
        range.snap(range.min + f64::from(share) * (range.max - range.min))
    }

    /// How far along the range the value is, from 0 to 1.
    fn share(&self) -> f32 {
        let span = self.range.max - self.range.min;
        match span > 0.0 {
            true => ((self.value - self.range.min) / span) as f32,
            false => 0.0,
        }
    }
}

impl Widget for Slider {
    type Output = ();
    type Response = SliderResponse;

    /// Reacts to `input`: keys as the module documentation lists, while
    /// focused; a press on the slider jumps and drags the knob. A disabled
    /// slider ignores everything. Asks for a redraw whenever it used the
    /// input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> SliderResponse {
        if !self.enabled {
            return SliderResponse::Ignored;
        }
        let response = match input {
            Input::Key(key) if self.focused => self.handle_key(*key),
            Input::Key(_) => SliderResponse::Ignored,
            Input::FocusLost => {
                self.dragging = false;
                SliderResponse::Ignored
            }
            Input::KeyReleased(_)
            | Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
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

    /// Draws the slider across `area`, centered on its height: the track, filled in the accent up to the knob (muted
    /// while disabled).
    /// The knob is round in a theme with rounded controls and square on the
    /// grid.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = frame.cell_metrics();
        let scale_factor = frame.scale_factor();
        let middle = area.y + area.height / 2.0;
        let mark = match self.enabled {
            true => theme.accent,
            false => theme.muted,
        };
        let control = area;
        let knob = logical(KNOB_SIZE, scale_factor).min(metrics.height);
        let thickness = logical(TRACK_THICKNESS, scale_factor);
        let inset = knob / 2.0;
        let track = Rect::new(
            control.x + inset,
            (middle - thickness / 2.0).round(),
            (control.width - 2.0 * inset).max(0.0),
            thickness,
        );
        frame.draw_box(track, &theme.shape.highlight(theme.border, scale_factor));
        let center = track.x + self.share() * track.width;
        let filled = Rect::new(track.x, track.y, center - track.x, thickness);
        frame.draw_box(filled, &theme.shape.highlight(mark, scale_factor));
        let knob_rect = Rect::new(
            (center - knob / 2.0).round(),
            (middle - knob / 2.0).round(),
            knob,
            knob,
        );
        let knob_style = BoxStyle {
            corner_radius: match theme.shape.control_radius {
                0 => 0.0,
                _ => knob / 2.0,
            },
            ..theme.shape.highlight(mark, scale_factor)
        };
        frame.draw_box(knob_rect, &knob_style);
        self.layout = Some(SliderLayout { area, track });
    }

    /// Tells the slider whether it has the keyboard: it takes keys only
    /// while it does (a [`FocusTree`](crate::FocusTree)
    /// draws the focus ring around it).
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Whether the app said the slider has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl Control for Slider {
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

    /// A row, whatever the width.
    fn measure(&self, _width: f32, metrics: CellMetrics, _scale_factor: f64) -> f32 {
        metrics.height.round()
    }

    /// The value the user moved it to.
    fn changed(&self, response: SliderResponse) -> Option<f64> {
        match response {
            SliderResponse::Changed(value) => Some(value),
            SliderResponse::Handled | SliderResponse::Ignored => None,
        }
    }
}

impl Outcome for SliderResponse {
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
    use crate::app::offscreen;
    use crate::base::color::Rgba;
    use crate::base::key::parse_key;
    use crate::input::event::ButtonInput;
    use crate::testing::{TestWindow, ui_text};

    const PERCENT: NumberRange = NumberRange::new(0.0, 100.0, 1.0);

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    fn drawn(slider: &mut Slider, window: &TestWindow) -> Rect {
        let mut frame = window.frame();
        let area = Rect::new(0.0, 0.0, 200.0, 20.0);
        slider.draw(&mut frame, area, &Theme::dark());
        slider.layout.expect("drawn").track
    }

    #[test]
    fn test_keys_step_page_and_jump_only_while_focused_and_say_when_the_value_stays() {
        let mut window = TestWindow::new(200, 20);
        let mut slider = Slider::new(PERCENT, 50.0);
        assert_eq!(window.feed(&mut slider, &key("l")), SliderResponse::Ignored);
        slider.set_focused(true);
        assert_eq!(
            window.feed(&mut slider, &key("l")),
            SliderResponse::Changed(51.0)
        );
        assert!(window.take_redraw());
        assert_eq!(
            window.feed(&mut slider, &key("<pagedown>")),
            SliderResponse::Changed(41.0)
        );
        assert_eq!(
            window.feed(&mut slider, &key("<end>")),
            SliderResponse::Changed(100.0)
        );
        assert_eq!(
            window.feed(&mut slider, &key("<up>")),
            SliderResponse::Handled,
            "already at the largest value"
        );
        assert_eq!(
            window.feed(&mut slider, &key("C-l")),
            SliderResponse::Ignored
        );
        slider.set_enabled(false);
        assert_eq!(window.feed(&mut slider, &key("h")), SliderResponse::Ignored);
    }

    #[test]
    fn test_a_press_jumps_the_knob_and_a_drag_follows_until_the_release() {
        let mut window = TestWindow::new(200, 20);
        let mut slider = Slider::new(PERCENT, 0.0);
        let track = drawn(&mut slider, &window);
        let at = |share: f32| Point::new(track.x + share * track.width, 10.0);
        let press = ButtonInput::press(PointerButton::Left, at(0.25));
        assert_eq!(
            window.feed(&mut slider, &Input::PointerButton(press)),
            SliderResponse::Changed(25.0)
        );
        assert_eq!(
            window.feed(&mut slider, &Input::PointerMoved(at(2.0))),
            SliderResponse::Changed(100.0),
            "held to the end of the track"
        );
        let release = ButtonInput::release(PointerButton::Left, at(2.0));
        window.feed(&mut slider, &Input::PointerButton(release));
        assert_eq!(
            window.feed(&mut slider, &Input::PointerMoved(at(0.5))),
            SliderResponse::Ignored,
            "the drag ended"
        );
        let outside = ButtonInput::press(PointerButton::Left, Point::new(10.0, 50.0));
        assert_eq!(
            window.feed(&mut slider, &Input::PointerButton(outside)),
            SliderResponse::Ignored
        );
        assert_eq!(slider.value(), 100.0);
    }

    #[test]
    fn test_the_slider_describes_its_value_and_steps_for_a_screen_reader() {
        let window = TestWindow::new(200, 20);
        let id = FocusId::new("volume");
        let mut slider = Slider::new(NumberRange::new(0.0, 1.0, 0.1), 0.5).with_label("Volume");
        let mut frame = window.frame();
        let bounds = frame.bounds();
        slider.draw(&mut frame, bounds, &Theme::dark());
        slider.access(&mut frame, id);
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\" focused\n  Slider \"Volume\" value=\"0.5\" value=0.5\n"
        );
        assert_eq!(
            slider.access_action(id, AccessAction::Increment(id)),
            SliderResponse::Changed(0.6)
        );
        assert_eq!(
            slider.access_action(id, AccessAction::Click(id)),
            SliderResponse::Ignored
        );
        slider.set_enabled(false);
        let mut frame = window.frame();
        let bounds = frame.bounds();
        slider.draw(&mut frame, bounds, &Theme::dark());
        slider.access(&mut frame, id);
        assert!(ui_text(&frame, "t").ends_with("value=0.6 disabled\n"));
    }

    #[test]
    fn test_the_track_is_filled_in_the_accent_up_to_the_knob() {
        let theme = Theme::dark();
        let mut slider = Slider::new(PERCENT, 50.0);
        let area = Rect::new(0.0, 0.0, 200.0, 20.0);
        let Some(snapshot) = offscreen::render(200, 20, |frame| {
            frame.clear(Rgba::from(theme.background));
            slider.draw(frame, area, &theme);
        }) else {
            return;
        };
        let track = slider.layout.expect("drawn").track;
        let middle = track.y + track.height / 2.0;
        snapshot.assert_pixel(track.x + 2.0, middle, theme.accent, "filled");
        snapshot.assert_pixel(track.x + track.width - 2.0, middle, theme.border, "empty");
    }
}
