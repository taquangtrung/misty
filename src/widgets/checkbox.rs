//! [`Checkbox`]: an option the user turns on or off, with a label to its
//! right, drawn as a box with a check mark or, made with
//! [`Checkbox::switch`], as a switch: a pill track whose knob slides right
//! and fills with the accent when on.
//!
//! Keys, while the app says the checkbox is focused: Space or Enter toggles
//! it. A click on the box or the label toggles it, focused or not. A
//! disabled checkbox is drawn muted and leaves every input alone.

use accesskit::{Action, Node, Role};

use super::super::app::access::AccessAction;
use super::super::app::context::EventCx;
use super::super::app::focus::FocusId;
use super::super::app::frame::Frame;
use super::super::app::widget::{Outcome, Widget};
use super::super::base::box_style::{Border, BoxStyle};
use super::super::base::color::{Rgb, Rgba};
use super::super::base::geometry::{Point, Rect};
use super::super::base::key::{Key, KeyCode, Mods};
use super::super::base::theme::Theme;
use super::super::input::event::{Input, PointerButton};
use super::super::render::font::CellMetrics;
use super::super::render::icon::logical;
use super::label::{draw_label, label_width};

// ========================================================================
// Constants
// ========================================================================

/// The share of a cell's height a check box, a switch track, or a radio
/// dot is tall.
pub(crate) const MARK_SHARE: f32 = 0.75;

/// The blank cells between a mark and its label.
pub(crate) const LABEL_GAP_CELLS: f32 = 1.0;

/// The thickness of an empty mark's outline, in logical pixels: drawn even
/// when the theme's boxes have no border, so the mark never vanishes.
pub(crate) const MARK_BORDER: f32 = 1.0;

/// How many times its height a switch's track is wide.
const SWITCH_WIDTH_SHARE: f32 = 1.8;

/// The gap between a switch's knob and the edge of its track, in logical
/// pixels.
const KNOB_INSET: f32 = 2.0;

/// The thickness of the check mark's strokes, in logical pixels.
const CHECK_STROKE: f32 = 2.0;

/// The check mark's corners, as shares of the box's width and height: the
/// start of the short stroke, the bend, and the end of the long stroke.
const CHECK_POINTS: [(f32, f32); 3] = [(0.22, 0.5), (0.42, 0.7), (0.78, 0.3)];

/// How many squares draw each stroke-wide stretch of a check mark's line,
/// so the steps overlap into a smooth line.
const CHECK_STEPS_PER_STROKE: f32 = 2.0;

// ========================================================================
// Data Structures
// ========================================================================

/// An option the user turns on or off. The app owns it, hands it the
/// window's pointer events (and its keys while it has the focus), and draws
/// it each frame.
#[derive(Clone, Debug)]
pub struct Checkbox {
    /// Whether it is on.
    checked: bool,
    /// Whether it takes input; a disabled checkbox is drawn muted.
    enabled: bool,
    /// Whether the app gave it the keyboard.
    focused: bool,
    /// Whether the pointer is over it.
    hovered: bool,
    /// What it says.
    label: String,
    /// Where it was drawn last, for the pointer.
    layout: Option<CheckboxLayout>,
    /// How it looks: a box or a switch.
    look: Look,
}

/// What became of an input offered to a [`Checkbox`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CheckboxResponse {
    /// The user turned it on (`true`) or off (`false`).
    Changed(bool),
    /// The checkbox used the input; nothing changed.
    Handled,
    /// The input is not the checkbox's.
    Ignored,
}

/// Where a checkbox's parts were drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CheckboxLayout {
    /// The box or the switch's track.
    mark: Rect,
    /// The mark and the label together: what a click toggles.
    row: Rect,
}

/// How a [`Checkbox`] is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Look {
    /// A box with a check mark when on.
    Box,
    /// A pill track with a knob, at the right when on.
    Switch,
}

// ========================================================================
// Checkbox Implementations
// ========================================================================

impl Checkbox {
    /// An enabled checkbox saying `label`, off, drawn as a box.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            checked: false,
            enabled: true,
            focused: false,
            hovered: false,
            label: label.into(),
            layout: None,
            look: Look::Box,
        }
    }

    /// This checkbox on (`true`) or off.
    pub fn with_checked(self, checked: bool) -> Self {
        Self { checked, ..self }
    }

    /// This checkbox taking input (`true`), or disabled: drawn muted and
    /// leaving every input alone.
    pub fn with_enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    /// This checkbox drawn as a switch: a pill track with a knob, which
    /// slides right and fills with the accent when on.
    pub fn switch(self) -> Self {
        Self {
            look: Look::Switch,
            ..self
        }
    }

    /// Whether it is on.
    pub fn checked(&self) -> bool {
        self.checked
    }

    /// Turns it on (`true`) or off, as the app's state says, without a
    /// response.
    pub fn set_checked(&mut self, checked: bool) {
        self.checked = checked;
    }

    /// Whether it takes input.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Lets it take input (`true`), or disables it.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.hovered &= enabled;
    }

    /// How tall it is drawn with `metrics`: one line.
    /// The scale factor is unused here; every control takes it, so an app
    /// asks each control's height the same way.
    pub fn height(&self, metrics: CellMetrics, _scale_factor: f64) -> f32 {
        metrics.height.ceil()
    }

    /// Describes the checkbox as last drawn to screen readers as `id`: a
    /// check box (or a switch) named by its label, saying whether it is
    /// on, clickable while enabled. Nothing before the first frame.
    pub fn access(&self, frame: &mut Frame, id: FocusId) {
        let Some(layout) = self.layout else {
            return;
        };
        let role = match self.look {
            Look::Box => Role::CheckBox,
            Look::Switch => Role::Switch,
        };
        let mut node = Node::new(role);
        node.set_label(self.label.as_str());
        node.set_toggled(self.checked.into());
        match self.enabled {
            true => node.add_action(Action::Click),
            false => node.set_disabled(),
        }
        frame.access(id, layout.row, node);
    }

    /// Does what a screen reader's `action` on the checkbox described as
    /// `id` asks: a click toggles it. Ignored for other actions and nodes.
    pub fn access_action(&mut self, id: FocusId, action: AccessAction) -> CheckboxResponse {
        match action {
            AccessAction::Click(target) if target == id => self.toggle(),
            AccessAction::Click(_)
            | AccessAction::Decrement(_)
            | AccessAction::Focus(_)
            | AccessAction::Increment(_) => CheckboxResponse::Ignored,
        }
    }

    /// Turns it over, when it is enabled.
    fn toggle(&mut self) -> CheckboxResponse {
        if !self.enabled {
            return CheckboxResponse::Ignored;
        }
        self.checked = !self.checked;
        CheckboxResponse::Changed(self.checked)
    }

    /// Reacts to `key`, as the module documentation lists, while focused.
    fn handle_key(&mut self, key: Key) -> CheckboxResponse {
        if !self.focused {
            return CheckboxResponse::Ignored;
        }
        match key.code {
            KeyCode::Char(' ') | KeyCode::Enter if key.mods == Mods::NONE => self.toggle(),
            KeyCode::Backspace
            | KeyCode::Char(_)
            | KeyCode::Delete
            | KeyCode::Down
            | KeyCode::End
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::Up => CheckboxResponse::Ignored,
        }
    }

    /// Whether `point` is over the checkbox as last drawn.
    fn is_over(&self, point: Point) -> bool {
        self.layout.is_some_and(|layout| layout.row.contains(point))
    }

    /// Follows the pointer to `point` (`None` outside the window), asking a
    /// redraw when it entered or left; says whether it is over.
    fn follow_pointer(&mut self, point: Option<Point>, cx: &mut EventCx<'_>) -> bool {
        let over = self.enabled && point.is_some_and(|point| self.is_over(point));
        if over != self.hovered {
            self.hovered = over;
            cx.request_redraw();
        }
        over
    }

    /// Draws the box in `mark`: filled with the accent and checked when on,
    /// outlined when off.
    fn draw_box(&self, frame: &mut Frame, mark: Rect, theme: &Theme) {
        let on = self.mark_color(theme);
        draw_check_box(frame, mark, self.checked, on, self.empty_fill(theme), theme);
    }

    /// Draws the switch in `track`: the track in the accent when on, the
    /// border color when off, the knob at the end it is switched to.
    fn draw_switch(&self, frame: &mut Frame, track: Rect, theme: &Theme) {
        let scale_factor = frame.scale_factor();
        let fill = match self.checked {
            true => self.mark_color(theme),
            false => theme.border,
        };
        frame.draw_box(track, &pill(fill, track.height));
        let inset = logical(KNOB_INSET, scale_factor);
        let size = track.height - 2.0 * inset;
        let x = match self.checked {
            true => track.x + track.width - inset - size,
            false => track.x + inset,
        };
        let knob = Rect::new(x, track.y + inset, size, size);
        frame.draw_box(knob, &pill(theme.background, size));
    }

    /// The color of a mark that is on: the accent, or muted when disabled.
    fn mark_color(&self, theme: &Theme) -> Rgb {
        match self.enabled {
            true => theme.accent,
            false => theme.muted,
        }
    }

    /// The fill of an empty box: the hover color under the pointer.
    fn empty_fill(&self, theme: &Theme) -> Rgb {
        match self.hovered {
            true => theme.hover,
            false => theme.background,
        }
    }
}

impl Widget for Checkbox {
    type Output = Rect;
    type Response = CheckboxResponse;

    /// Reacts to `input`: keys as the module documentation lists while
    /// focused, a left press over the box or the label toggling it, and
    /// the pointer lighting it. Asks for a redraw whenever it used the
    /// input.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> CheckboxResponse {
        let response = match input {
            Input::Key(key) if self.enabled => self.handle_key(*key),
            Input::PointerButton(button)
                if button.pressed
                    && button.button == PointerButton::Left
                    && self.enabled
                    && self.is_over(button.position) =>
            {
                self.toggle()
            }
            Input::PointerMoved(position) => match self.follow_pointer(Some(*position), cx) {
                true => CheckboxResponse::Handled,
                false => CheckboxResponse::Ignored,
            },
            Input::PointerLeft => {
                self.follow_pointer(None, cx);
                CheckboxResponse::Ignored
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
            | Input::Wheel(_) => CheckboxResponse::Ignored,
        };
        if !response.is_ignored() {
            cx.request_redraw();
        }
        response
    }

    /// Draws the checkbox one line tall at the top left of `area`, the mark
    /// then its label, and returns the line it took (mark and label).
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) -> Rect {
        let metrics = frame.cell_metrics();
        let size = (metrics.height * MARK_SHARE).round();
        let width = match self.look {
            Look::Box => size,
            Look::Switch => (size * SWITCH_WIDTH_SHARE).round(),
        };
        let y = (area.y + (metrics.height - size) / 2.0).round();
        let mark = Rect::new(area.x, y, width, size);
        let label_x = (mark.x + mark.width + LABEL_GAP_CELLS * metrics.width).round();
        let label_color = label_color(self.enabled, theme);
        draw_label(
            frame,
            theme,
            &self.label,
            Point::new(label_x, area.y),
            label_color,
        );
        match self.look {
            Look::Box => self.draw_box(frame, mark, theme),
            Look::Switch => self.draw_switch(frame, mark, theme),
        }
        let label = label_width(frame, theme, &self.label);
        let row = Rect::new(area.x, area.y, label_x + label - area.x, metrics.height);
        self.layout = Some(CheckboxLayout { mark, row });
        row
    }

    /// Tells it whether it has the keyboard: it takes keys only while it
    /// does (a [`FocusTree`](crate::FocusTree)
    /// draws the focus ring around it).
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Whether the app last said it has the keyboard.
    fn is_focused(&self) -> bool {
        self.focused
    }
}

impl Outcome for CheckboxResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The color of a control's label: the foreground, or muted when the
/// control is disabled.
pub(crate) fn label_color(enabled: bool, theme: &Theme) -> Rgb {
    match enabled {
        true => theme.foreground,
        false => theme.muted,
    }
}

/// Draws a check box in `mark`: filled with `on` and checked when
/// `checked`, else filled with `empty` and outlined in the border color
/// (`on` and `empty` theme colors).
pub(crate) fn draw_check_box(
    frame: &mut Frame,
    mark: Rect,
    checked: bool,
    on: Rgb,
    empty: Rgb,
    theme: &Theme,
) {
    let scale_factor = frame.scale_factor();
    match checked {
        true => {
            frame.draw_box(mark, &theme.shape.highlight(on, scale_factor));
            let stroke = logical(CHECK_STROKE, scale_factor);
            for step in check_mark(mark, stroke) {
                frame.fill_rect(step, theme.background);
            }
        }
        false => frame.draw_box(mark, &empty_mark(empty, theme.border, scale_factor, theme)),
    }
}

/// The style of an empty mark: filled with `fill` and outlined in `edge`
/// (theme colors), rounded as the theme rounds controls.
pub(crate) fn empty_mark(fill: Rgb, edge: Rgb, scale_factor: f64, theme: &Theme) -> BoxStyle {
    BoxStyle {
        border: Some(Border {
            color: Rgba::from(edge),
            width: logical(MARK_BORDER, scale_factor),
        }),
        ..theme.shape.highlight(fill, scale_factor)
    }
}

/// The style of a round shape `height` tall filled with `fill` (a theme
/// color): a pill, or a circle when as wide as it is tall.
pub(crate) fn pill(fill: Rgb, height: f32) -> BoxStyle {
    BoxStyle {
        border: None,
        corner_radius: height / 2.0,
        fill: Rgba::from(fill),
        shadow: None,
    }
}

/// The squares that draw a check mark in `square` with strokes `stroke`
/// pixels thick, overlapping along its two lines.
fn check_mark(square: Rect, stroke: f32) -> Vec<Rect> {
    let corner =
        |(x, y): (f32, f32)| Point::new(square.x + x * square.width, square.y + y * square.height);
    let [start, bend, end] = CHECK_POINTS.map(corner);
    [(start, bend), (bend, end)]
        .into_iter()
        .flat_map(|(from, to)| {
            let length = (to.x - from.x).hypot(to.y - from.y);
            let steps = (length / stroke * CHECK_STEPS_PER_STROKE).ceil().max(1.0) as usize;
            (0..=steps).map(move |step| {
                let t = step as f32 / steps as f32;
                let x = from.x + (to.x - from.x) * t - stroke / 2.0;
                let y = from.y + (to.y - from.y) * t - stroke / 2.0;
                Rect::new(x.round(), y.round(), stroke, stroke)
            })
        })
        .collect()
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
    use crate::testing::{TestWindow, ui_text};

    fn key(spec: &str) -> Input {
        Input::Key(parse_key(spec).expect("a valid key"))
    }

    fn click(point: Point) -> Input {
        Input::PointerButton(ButtonInput::press(PointerButton::Left, point))
    }

    /// `checkbox` drawn at the top left of `window`, returning its row.
    fn draw(window: &TestWindow, checkbox: &mut Checkbox) -> Rect {
        let mut frame = window.frame();
        let bounds = frame.bounds();
        checkbox.draw(&mut frame, bounds, &Theme::dark())
    }

    #[test]
    fn test_space_and_enter_toggle_only_while_focused() {
        let mut window = TestWindow::new(300, 60);
        let mut checkbox = Checkbox::new("Wrap lines");
        assert_eq!(
            window.feed(&mut checkbox, &key("<space>")),
            CheckboxResponse::Ignored
        );
        assert!(!checkbox.checked(), "unfocused: the key is not its own");
        checkbox.set_focused(true);
        let toggled = window.feed(&mut checkbox, &key("<space>"));
        assert_eq!(toggled, CheckboxResponse::Changed(true));
        assert!(window.take_redraw());
        let back = window.feed(&mut checkbox, &key("<cr>"));
        assert_eq!(back, CheckboxResponse::Changed(false));
        assert!(window.take_redraw());
        assert_eq!(
            window.feed(&mut checkbox, &key("C-<space>")),
            CheckboxResponse::Ignored
        );
        assert_eq!(
            window.feed(&mut checkbox, &key("x")),
            CheckboxResponse::Ignored
        );
        assert!(!window.take_redraw(), "nothing used, nothing redrawn");
    }

    #[test]
    fn test_a_click_on_the_label_toggles_and_one_past_it_does_not() {
        let mut window = TestWindow::new(400, 60);
        let mut checkbox = Checkbox::new("Wrap lines");
        let row = draw(&window, &mut checkbox);
        let on_label = Point::new(row.x + row.width - 2.0, row.y + row.height / 2.0);
        assert_eq!(
            window.feed(&mut checkbox, &click(on_label)),
            CheckboxResponse::Changed(true)
        );
        let past = Point::new(row.x + row.width + 4.0, on_label.y);
        assert_eq!(
            window.feed(&mut checkbox, &click(past)),
            CheckboxResponse::Ignored
        );
        assert!(checkbox.checked());
    }

    #[test]
    fn test_a_disabled_checkbox_leaves_keys_and_clicks_alone() {
        let mut window = TestWindow::new(400, 60);
        let mut checkbox = Checkbox::new("Wrap lines").with_enabled(false);
        checkbox.set_focused(true);
        let row = draw(&window, &mut checkbox);
        let inside = Point::new(row.x + 2.0, row.y + row.height / 2.0);
        assert_eq!(
            window.feed(&mut checkbox, &click(inside)),
            CheckboxResponse::Ignored
        );
        assert_eq!(
            window.feed(&mut checkbox, &key("<space>")),
            CheckboxResponse::Ignored
        );
        let id = FocusId::new("wrap");
        assert_eq!(
            checkbox.access_action(id, AccessAction::Click(id)),
            CheckboxResponse::Ignored
        );
        assert!(!checkbox.checked());
    }

    #[test]
    fn test_a_checkbox_and_a_switch_describe_their_role_state_and_availability() {
        let window = TestWindow::new(400, 60);
        let mut frame = window.frame();
        let bounds = frame.bounds();
        let mut wrap = Checkbox::new("Wrap lines").with_checked(true);
        let mut sync = Checkbox::new("Sync").switch().with_enabled(false);
        wrap.draw(&mut frame, bounds, &Theme::dark());
        sync.draw(&mut frame, bounds, &Theme::dark());
        let (wrap_id, sync_id) = (FocusId::new("wrap"), FocusId::new("sync"));
        wrap.access(&mut frame, wrap_id);
        sync.access(&mut frame, sync_id);
        assert_eq!(
            ui_text(&frame, "t"),
            "Window \"t\" focused\n\
             \x20 CheckBox \"Wrap lines\" checked\n\
             \x20 Switch \"Sync\" unchecked disabled\n"
        );
        assert_eq!(
            wrap.access_action(wrap_id, AccessAction::Click(wrap_id)),
            CheckboxResponse::Changed(false)
        );
        assert_eq!(
            wrap.access_action(wrap_id, AccessAction::Click(sync_id)),
            CheckboxResponse::Ignored,
            "another node"
        );
    }

    #[test]
    fn test_a_checked_box_is_filled_and_marked_and_a_switch_moves_its_knob() {
        let theme = Theme::dark();
        let mut checked = Checkbox::new("On").with_checked(true);
        let mut off = Checkbox::new("Off").switch();
        let mut on = Checkbox::new("On").switch().with_checked(true);
        let Some(snapshot) = offscreen::render(400, 120, |frame| {
            frame.clear(Rgba::from(theme.background));
            let row = frame.cell_metrics().height.ceil() * 2.0;
            checked.draw(frame, Rect::new(10.0, 0.0, 300.0, row), &theme);
            off.draw(frame, Rect::new(10.0, row, 300.0, row), &theme);
            on.draw(frame, Rect::new(10.0, 2.0 * row, 300.0, row), &theme);
        }) else {
            return;
        };
        let mark = checked.layout.expect("drawn").mark;
        snapshot.assert_pixel(mark.x + 1.0, mark.y + 1.0, theme.accent, "a filled box");
        let [_, bend, _] = CHECK_POINTS;
        let (x, y) = (mark.x + bend.0 * mark.width, mark.y + bend.1 * mark.height);
        snapshot.assert_pixel(x, y, theme.background, "the check mark's bend");
        let track = off.layout.expect("drawn").mark;
        let middle = track.y + track.height / 2.0;
        snapshot.assert_pixel(
            track.x + track.width / 4.0,
            middle,
            theme.background,
            "off: knob left",
        );
        snapshot.assert_pixel(
            track.x + track.width * 0.85,
            middle,
            theme.border,
            "off: a border-colored track",
        );
        let track = on.layout.expect("drawn").mark;
        let middle = track.y + track.height / 2.0;
        snapshot.assert_pixel(
            track.x + track.width * 0.15,
            middle,
            theme.accent,
            "on: accent track",
        );
        snapshot.assert_pixel(
            track.x + track.width * 0.75,
            middle,
            theme.background,
            "on: knob right",
        );
    }
}
